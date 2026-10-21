//! Speak instead of type: the composer's mic opens a take that streams the microphone through an
//! `AVAudioEngine` input tap into an `SFSpeechAudioBufferRecognitionRequest` (on-device where the
//! recognizer supports it), so partial text arrives while the user speaks. Where that can't
//! start, the take records to a temp `.caf` with `AVAudioRecorder` and the Speech framework
//! transcribes the file once it stops. macOS asks for the microphone and Speech permissions the
//! first time each is needed; a build without the usage descriptions in its Info.plist — a bare
//! `cargo run` binary — hides the button instead of asking (TCC ends processes that ask without
//! declaring).

use async_channel::Sender;

/// What a take says to the composer.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The best text for the whole take so far (live takes only); later ones revise it.
    Partial(String),
    /// The recognizer is done: the take's text.
    Final(String),
    /// Why it stopped.
    Failed(String),
}

/// A meter reading in dBFS (-160 silent to 0 full scale) as the waveform's 0 to 1: speech sits
/// around -40 to -10 dB, so -50 dB and below is silence and -8 dB a full bar.
pub fn loudness_db(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.;
    }
    ((db + 50.) / 42.).clamp(0., 1.)
}

/// The root-mean-square of a buffer's samples as [`loudness_db`].
pub fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    loudness_db(20. * rms.max(1e-9).log10())
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use block2::RcBlock;
    use objc2::AllocAnyThread as _;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioCommonFormat, AVAudioEngine, AVAudioFormat, AVAudioPCMBuffer, AVAudioRecorder, AVAudioTime};
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL, ns_string};
    use objc2_speech::{SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest};
    use std::path::PathBuf;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    /// Whether the mic button shows at all: both privacy keys must be in the bundle's
    /// Info.plist, or macOS ends the process when recording is asked for. `TREK_DICTATE=1`
    /// shows it anyway in unbundled builds (the button says so when it's tapped).
    pub fn available() -> bool {
        packaged() || std::env::var_os("TREK_DICTATE").is_some_and(|v| v == "1")
    }

    /// The bundle declares microphone and speech use — and AVAudioApplication asks for them,
    /// so macOS 13 and older never show the button.
    fn packaged() -> bool {
        if objc2::runtime::AnyClass::get(c"AVAudioApplication").is_none() {
            return false;
        }
        let info = NSBundle::mainBundle();
        info.objectForInfoDictionaryKey(ns_string!("NSMicrophoneUsageDescription")).is_some()
            && info.objectForInfoDictionaryKey(ns_string!("NSSpeechRecognitionUsageDescription")).is_some()
    }

    /// `reply` hears `Ok` once the mic and Speech are both allowed, or why they aren't. The
    /// permission blocks run on other threads, so this can answer a while after it's called.
    pub fn ensure_permission(reply: Sender<Result<(), String>>) {
        if !packaged() {
            let _ = reply.try_send(Err("Dictation needs a bundled Trek build.".into()));
            return;
        }
        unsafe {
            match AVAudioApplication::sharedInstance().recordPermission() {
                p if p == AVAudioApplicationRecordPermission::Granted => speech_permission(reply),
                p if p == AVAudioApplicationRecordPermission::Denied => {
                    let _ = reply.try_send(Err("Trek has no microphone access — allow it in System Settings › Privacy.".into()));
                }
                _ => {
                    let again = reply.clone();
                    let block = RcBlock::new(move |granted: Bool| {
                        if granted.as_bool() {
                            speech_permission(again.clone());
                        } else {
                            let _ = again.try_send(Err("Trek needs the microphone to dictate.".into()));
                        }
                    });
                    AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block);
                }
            }
        }
    }

    fn speech_permission(reply: Sender<Result<(), String>>) {
        unsafe {
            match SFSpeechRecognizer::authorizationStatus() {
                s if s == SFSpeechRecognizerAuthorizationStatus::Authorized => {
                    let _ = reply.try_send(Ok(()));
                }
                s if s == SFSpeechRecognizerAuthorizationStatus::NotDetermined => {
                    let block = RcBlock::new(move |status| {
                        let _ = reply.try_send(if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
                            Ok(())
                        } else {
                            Err("Speech recognition is off for Trek — allow it in System Settings › Privacy.".into())
                        });
                    });
                    SFSpeechRecognizer::requestAuthorization(&block);
                }
                _ => {
                    let _ = reply.try_send(Err("Speech recognition is off for Trek — allow it in System Settings › Privacy.".into()));
                }
            }
        }
    }

    /// One take: the mic open while it lives (live, or recording to a file), and the Speech
    /// task that turns it into text.
    pub struct Dictation {
        source: Source,
        /// The Speech task; keeping it alive keeps the results coming.
        task: Option<Retained<SFSpeechRecognitionTask>>,
        /// Where results go (the file path hands it to its task once the recording stops).
        heard: Sender<Heard>,
    }

    enum Source {
        /// The engine's input tap feeds `request`; `level` is the loudest since last read, as
        /// f32 bits (non-negative floats order as their bits do, so `fetch_max` works).
        Live { engine: Retained<AVAudioEngine>, request: Retained<SFSpeechAudioBufferRecognitionRequest>, level: Arc<AtomicU32>, open: bool },
        /// The fallback: a recording to `file`, transcribed once it stops. `ours`: `start`
        /// made the file and Drop removes it (`transcribe_file` borrows the caller's).
        File { recorder: Option<Retained<AVAudioRecorder>>, file: PathBuf, ours: bool },
    }

    impl Dictation {
        /// Partial text comes while the user speaks (the audio-buffer request).
        pub fn live(&self) -> bool {
            matches!(self.source, Source::Live { .. })
        }

        /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
        pub fn level(&self) -> f32 {
            match &self.source {
                Source::Live { level, open: true, .. } => f32::from_bits(level.swap(0, Ordering::Relaxed)),
                Source::File { recorder: Some(r), .. } => unsafe {
                    r.updateMeters();
                    loudness_db(r.averagePowerForChannel(0))
                },
                _ => 0.,
            }
        }

        /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
        pub fn finish(&mut self) {
            match &mut self.source {
                Source::Live { engine, request, open, .. } => {
                    if std::mem::take(open) {
                        unsafe {
                            engine.inputNode().removeTapOnBus(0);
                            engine.stop();
                            request.endAudio();
                        }
                    }
                }
                Source::File { recorder, file, .. } => {
                    let Some(recorder) = recorder.take() else { return };
                    unsafe { recorder.stop() };
                    match transcribe_task(file, self.heard.clone()) {
                        Ok(task) => self.task = Some(task),
                        Err(e) => {
                            let _ = self.heard.try_send(Heard::Failed(e));
                        }
                    }
                }
            }
        }
    }

    /// A take abandoned (✕, the composer closed, the app is quitting) ends at the mic and
    /// leaves nothing in tmp.
    impl Drop for Dictation {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                unsafe { task.cancel() };
            }
            match &mut self.source {
                Source::Live { engine, request, open, .. } => {
                    if std::mem::take(open) {
                        unsafe {
                            engine.inputNode().removeTapOnBus(0);
                            engine.stop();
                            request.endAudio();
                        }
                    }
                }
                Source::File { recorder, file, ours } => {
                    if let Some(recorder) = recorder.take() {
                        unsafe { recorder.stop() };
                    }
                    if *ours {
                        let _ = std::fs::remove_file(&*file);
                    }
                }
            }
        }
    }

    /// A take over an existing audio file — no mic, but Speech permission still applies. The
    /// shots harness drives this (`dictate-file`) to prove the transcription path without a
    /// clickable permission dialog.
    #[cfg(feature = "shots")]
    pub fn transcribe_file(file: PathBuf, heard: Sender<Heard>) -> Result<Dictation, String> {
        let task = transcribe_task(&file, heard.clone())?;
        Ok(Dictation { source: Source::File { recorder: None, file, ours: false }, task: Some(task), heard })
    }

    /// The recognizer for the user's language, if Speech has one ready.
    fn recognizer() -> Result<Retained<SFSpeechRecognizer>, String> {
        let missing = || format!("Speech recognition isn't available on {}.", crate::words::words().this_computer);
        unsafe {
            // `init` is failable (no supported locale); `new` would hand nil to Retained.
            let recognizer = SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()).ok_or_else(missing)?;
            if !recognizer.isAvailable() {
                return Err(missing());
            }
            Ok(recognizer)
        }
    }

    /// Start `request` on `recognizer`; every result and the failure go to `heard`.
    fn recognize(recognizer: &SFSpeechRecognizer, request: &SFSpeechRecognitionRequest, heard: Sender<Heard>) -> Retained<SFSpeechRecognitionTask> {
        unsafe {
            request.setAddsPunctuation(true);
            let block = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                if !result.is_null() {
                    let text = (*result).bestTranscription().formattedString().to_string();
                    let _ = heard.try_send(if (*result).isFinal() { Heard::Final(text) } else { Heard::Partial(text) });
                } else if !error.is_null() {
                    let _ = heard.try_send(Heard::Failed((*error).localizedDescription().to_string()));
                }
            });
            recognizer.recognitionTaskWithRequest_resultHandler(request, &block)
        }
    }

    /// Speech over `file`: `heard` gets the transcription, or why it failed.
    fn transcribe_task(file: &std::path::Path, heard: Sender<Heard>) -> Result<Retained<SFSpeechRecognitionTask>, String> {
        let recognizer = recognizer()?;
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let request = SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url);
            request.setShouldReportPartialResults(false);
            Ok(recognize(&recognizer, &request, heard))
        }
    }

    /// Open a take: live through the audio engine where it starts, else recording to a file.
    /// Both permissions must be granted first — see [`ensure_permission`].
    pub fn start(heard: Sender<Heard>) -> Result<Dictation, String> {
        let recognizer = recognizer()?;
        match start_live(&recognizer, heard.clone()) {
            Some(take) => Ok(take),
            None => start_file(heard),
        }
    }

    /// The live take: an input tap on the engine streams buffers into the recognizer and
    /// meters them. `None` when the machine can't (no buffer request, no input, the engine
    /// won't start), so the file path takes over.
    fn start_live(recognizer: &SFSpeechRecognizer, heard: Sender<Heard>) -> Option<Dictation> {
        objc2::runtime::AnyClass::get(c"SFSpeechAudioBufferRecognitionRequest")?;
        objc2::runtime::AnyClass::get(c"AVAudioEngine")?;
        unsafe {
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setShouldReportPartialResults(true);
            if recognizer.supportsOnDeviceRecognition() {
                request.setRequiresOnDeviceRecognition(true);
            }
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let format = input.outputFormatForBus(0);
            if format.sampleRate() <= 0. || format.channelCount() == 0 {
                return None;
            }
            let level = Arc::new(AtomicU32::new(0));
            let tap = {
                let request = request.clone();
                let level = level.clone();
                RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
                    let buffer = buffer.as_ref();
                    request.appendAudioPCMBuffer(buffer);
                    let channels = buffer.floatChannelData();
                    if !channels.is_null() {
                        let frames = buffer.frameLength() as usize;
                        let stride = buffer.stride().max(1);
                        let first = (*channels).as_ptr();
                        // The first channel, in place: this runs on the audio thread.
                        let heard = if stride == 1 {
                            loudness(std::slice::from_raw_parts(first, frames))
                        } else {
                            let square = (0..frames).map(|i| *first.add(i * stride)).map(|s| s * s).sum::<f32>();
                            loudness_db(10. * (square / frames.max(1) as f32).max(1e-18).log10())
                        };
                        level.fetch_max(heard.to_bits(), Ordering::Relaxed);
                    }
                })
            };
            input.installTapOnBus_bufferSize_format_block(0, 1024, Some(&format), RcBlock::as_ptr(&tap));
            engine.prepare();
            if engine.startAndReturnError().is_err() {
                input.removeTapOnBus(0);
                return None;
            }
            let task = recognize(recognizer, &request, heard.clone());
            Some(Dictation { source: Source::Live { engine, request, level, open: true }, task: Some(task), heard })
        }
    }

    /// The fallback take: record to a temp `.caf`, metered for the waveform.
    fn start_file(heard: Sender<Heard>) -> Result<Dictation, String> {
        unsafe {
            let file = std::env::temp_dir().join(format!("trek-dictate-{}.caf", std::process::id()));
            let Some(format) = AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
                AVAudioFormat::alloc(),
                AVAudioCommonFormat::PCMFormatFloat32,
                44_100.,
                1,
                false,
            ) else {
                return Err("Couldn't set up recording.".into());
            };
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let recorder = AVAudioRecorder::initWithURL_format_error(AVAudioRecorder::alloc(), &url, &format)
                .map_err(|e| e.localizedDescription().to_string())?;
            recorder.setMeteringEnabled(true);
            if !recorder.record() {
                return Err("Couldn't start recording.".into());
            }
            Ok(Dictation { source: Source::File { recorder: Some(recorder), file, ours: true }, task: None, heard })
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// Dictation in Trek is macOS-only for now; Windows has its own (Win+H), which needs nothing
    /// from Trek.
    pub struct Dictation;

    /// What a dictation asked for says: that it isn't here (yet), and on Windows the way to its own.
    fn unavailable() -> String {
        if cfg!(windows) {
            format!("Dictation isn't available on Windows yet. {}.", crate::words::words().dictate_tip)
        } else {
            "Dictation is macOS-only for now.".into()
        }
    }

    impl Dictation {
        pub fn live(&self) -> bool {
            false
        }
        pub fn level(&self) -> f32 {
            0.
        }
        pub fn finish(&mut self) {}
    }

    #[cfg(feature = "shots")]
    pub fn transcribe_file(_: std::path::PathBuf, _: Sender<Heard>) -> Result<Dictation, String> {
        Err(unavailable())
    }

    pub fn available() -> bool {
        false
    }

    pub fn ensure_permission(_: Sender<Result<(), String>>) {}

    pub fn start(_: Sender<Heard>) -> Result<Dictation, String> {
        Err(unavailable())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn windows_says_dictation_isnt_here_yet() {
            let said = super::unavailable();
            if cfg!(windows) {
                assert_eq!(said, "Dictation isn't available on Windows yet. Press Win+H to dictate.");
            } else {
                assert_eq!(said, "Dictation is macOS-only for now.");
            }
            let (tx, _rx) = async_channel::unbounded();
            assert_eq!(super::start(tx).err(), Some(said));
            assert!(!super::available(), "no mic button here");
        }
    }
}

pub use imp::*;
