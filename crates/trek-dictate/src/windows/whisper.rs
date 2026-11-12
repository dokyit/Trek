//! Whisper (whisper.cpp, through whisper-rs) as the on-device engine runs it: the model loaded
//! (`Model`), and a pass over part of a take decoded into phrases and where they fall in it
//! (`Decoder::decode`). x86-64 only: whisper.cpp is built there for the AVX2 baseline (see
//! `vendor/whisper-rs-sys/build.rs`), and `can_run` checks the processor has it before anything
//! loads.

use super::transcript::Segment;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// A pass to decode: one channel of 16 kHz sound, and where it starts in the take.
pub struct Pass<'a> {
    pub audio: &'a [f32],
    pub offset: usize,
    /// The take's language once it's kept (Whisper's id), else Whisper guesses.
    pub language: Option<i32>,
    /// The take's last pass, after ✓: worth more care (beam search, and decoding again warmer
    /// when unsure) than the ones that show the text as it comes.
    pub last: bool,
    /// Any of these set stops the pass part of the way (✕; for a pass as the text comes, ✓).
    pub stop: Vec<Arc<AtomicBool>>,
}

/// Whisper's short name for language `id` ("en"), if it's one.
pub fn language(id: i32) -> Option<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        whisper_rs::get_lang_str(id)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = id;
        None
    }
}

/// What a pass heard: its phrases, and the language Whisper decoded them in.
#[derive(Debug)]
pub struct Decoded {
    pub segments: Vec<Segment>,
    pub language: i32,
}

/// This processor runs the whisper.cpp Trek is built with: x86-64 with AVX2, FMA, F16C and
/// BMI2 (any Intel since Haswell, any AMD since Excavator).
pub fn can_run() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
            && std::arch::is_x86_feature_detected!("bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

/// Threads a pass decodes on: about the performance cores, as whisper.cpp gains little past
/// eight and a laptop's efficiency cores slow a pass down rather than speed it up.
pub fn threads() -> i32 {
    let n = std::thread::available_parallelism().map_or(4, |n| n.get());
    (n / 2).clamp(2, 8) as i32
}

#[cfg(target_arch = "x86_64")]
pub use self::engine::{Decoder, Model};

#[cfg(target_arch = "x86_64")]
mod engine {
    use super::super::transcript::{WRITTEN, plain_english};
    use super::{Decoded, Pass, Segment, threads};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use whisper_rs::{
        FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    };

    /// The encoder's frames: one per 20 ms (320 samples), 1500 to Whisper's 30 s window.
    const AUDIO_FRAME: usize = 320;
    const FULL_WINDOW: usize = 1500;

    /// whisper.cpp asks between steps whether to stop: any of the pass's flags set.
    ///
    /// # Safety
    /// `user_data` points at a `&[Arc<AtomicBool>]` alive for the call (see `decode`).
    unsafe extern "C" fn stop_asked(user_data: *mut std::ffi::c_void) -> bool {
        // SAFETY: as the caller promises.
        let stop = unsafe { *(user_data as *const &[Arc<AtomicBool>]) };
        stop.iter().any(|s| s.load(Ordering::Relaxed))
    }

    /// A loaded model: about its file's size in memory, shared by every take.
    pub struct Model(WhisperContext);

    /// A take's own decoding state, on the model.
    pub struct Decoder(WhisperState);

    impl Model {
        pub fn load(path: &Path) -> Result<Model, String> {
            // whisper.cpp's own logging goes to Trek's (tracing), not to stderr.
            static HOOKS: std::sync::Once = std::sync::Once::new();
            HOOKS.call_once(whisper_rs::install_logging_hooks);
            let mut params = WhisperContextParameters::default();
            params.use_gpu(false);
            WhisperContext::new_with_params(path, params)
                .map(Model)
                .map_err(|e| e.to_string())
        }

        pub fn decoder(&self) -> Result<Decoder, String> {
            self.0
                .create_state()
                .map(Decoder)
                .map_err(|e| e.to_string())
        }
    }

    impl Decoder {
        pub fn decode(&mut self, pass: Pass) -> Result<Decoded, String> {
            let kept = pass.language.and_then(whisper_rs::get_lang_str);
            let decoded = self.run(&pass, kept, None)?;
            // The take's last pass, its English written plainly: again, after a prompt to write
            // as written. Only the last (beam search, and warmer again when unsure): the quick
            // greedy passes over the first words as they come, prompted, repeat themselves
            // ("Use the chain rule to use the chain rule to …").
            if pass.last {
                let language = kept.or_else(|| whisper_rs::get_lang_str(decoded.language));
                let text: String = decoded.segments.iter().map(|s| s.text.as_str()).collect();
                if plain_english(language, &text) {
                    return self.run(&pass, language, Some(WRITTEN));
                }
            }
            Ok(decoded)
        }

        /// One decoding of `pass` in `language` (Whisper's guess when `None`), after `prompt`.
        fn run(
            &mut self,
            pass: &Pass,
            language: Option<&'static str>,
            prompt: Option<&'static str>,
        ) -> Result<Decoded, String> {
            let mut params = FullParams::new(if pass.last {
                SamplingStrategy::BeamSearch {
                    beam_size: 5,
                    patience: -1.,
                }
            } else {
                SamplingStrategy::Greedy { best_of: 1 }
            });
            params.set_n_threads(threads());
            params.set_language(Some(language.unwrap_or("auto")));
            params.set_translate(false);
            // Each pass stands alone: the part of the take it decodes is all it needs.
            params.set_no_context(true);
            if let Some(prompt) = prompt {
                params.set_initial_prompt(prompt);
            }
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            if !pass.last {
                // Text as it comes is a guess anyway: no second, warmer try when unsure.
                params.set_temperature_inc(0.);
                // And the encoder looks at the sound there is (and a margin), not Whisper's
                // whole 30 s window: on this PC (a 13th-gen i7) 13 s of speech decodes in 0.56 s
                // instead of 1.0, with the same words. The last pass keeps the whole window.
                let frames = pass.audio.len().div_ceil(AUDIO_FRAME) + 64;
                if frames < FULL_WINDOW {
                    params.set_audio_ctx(frames as i32);
                }
            }
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            // Not `set_abort_callback_safe`: whisper-rs 0.16 boxes the closure as a `dyn FnMut`
            // but its trampoline reads it back as the closure's own type, and whisper.cpp then
            // calls into garbage (an access violation here). The flags are read in place
            // instead, through a trampoline of Trek's own.
            let stop: &[Arc<AtomicBool>] = &pass.stop;
            // SAFETY: `stop_asked` reads `user_data` as the slice reference it's given here,
            // which lives on this stack frame until `full` (the only caller) has returned.
            unsafe {
                params.set_abort_callback(Some(stop_asked));
                params.set_abort_callback_user_data(&stop as *const &[Arc<AtomicBool>] as *mut _);
            }
            self.0.full(params, pass.audio).map_err(|e| e.to_string())?;
            // Whisper's times are in hundredths of a second from the pass's start.
            let at = |cs: i64| pass.offset + (cs.max(0) as usize) * 160;
            let segments = (0..self.0.full_n_segments())
                .filter_map(|i| self.0.get_segment(i))
                .map(|s| Segment {
                    span: at(s.start_timestamp())..at(s.end_timestamp()),
                    text: s.to_str_lossy().map(|t| t.into_owned()).unwrap_or_default(),
                })
                .collect();
            Ok(Decoded {
                segments,
                language: self.0.full_lang_id_from_state(),
            })
        }
    }
}

/// Elsewhere there's no whisper.cpp: `can_run` is false, so nothing reaches these.
#[cfg(not(target_arch = "x86_64"))]
pub use self::none::{Decoder, Model};

#[cfg(not(target_arch = "x86_64"))]
mod none {
    use super::{Decoded, Pass};
    use std::path::Path;

    pub struct Model;
    pub struct Decoder;

    impl Model {
        pub fn load(_: &Path) -> Result<Model, String> {
            Err("no whisper.cpp in this build".into())
        }
        pub fn decoder(&self) -> Result<Decoder, String> {
            Err("no whisper.cpp in this build".into())
        }
    }

    impl Decoder {
        pub fn decode(&mut self, _: Pass) -> Result<Decoded, String> {
            Err("no whisper.cpp in this build".into())
        }
    }
}

/// Load the model at `path`, shared.
pub fn load(path: &Path) -> Result<Arc<Model>, String> {
    Model::load(path).map(Arc::new)
}
