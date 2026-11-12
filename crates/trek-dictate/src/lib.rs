//! Speak instead of type: the composer's mic opens a take that streams the microphone into the
//! system's speech recognizer, so partial text arrives while the user speaks.
//!
//! On macOS the take streams through an `AVAudioEngine` input tap into an
//! `SFSpeechAudioBufferRecognitionRequest` (on-device where the recognizer supports it). Where
//! that can't start, the take records to a temp `.caf` with `AVAudioRecorder` and the Speech
//! framework transcribes the file once it stops. macOS asks for the microphone and Speech
//! permissions the first time each is needed; a build without the usage descriptions in its
//! Info.plist — a bare `cargo run` binary — hides the button instead of asking (TCC ends
//! processes that ask without declaring).
//!
//! On Windows it's Whisper on the PC itself (whisper.cpp over the microphone as WASAPI records
//! it), its speech model downloaded the first time dictation is used; a Trek with package
//! identity uses Windows' own online speech recognition instead, which Windows answers only
//! there (`windows`). Where neither can run, the mic offers Windows' voice typing (Win+H).
//! Elsewhere there's no dictation, and no mic.
//!
//! Under `test-support` a fake stands in for all of it (`fake`), so no test reaches a real
//! microphone; with none installed there's no dictation.

use async_channel::Sender;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(feature = "test-support")]
pub mod fake;
#[cfg(any(windows, test))]
#[doc(hidden)]
pub mod windows;
#[cfg(all(windows, not(feature = "test-support")))]
use self::windows as imp;

/// Under `test-support` each call goes to the fake, otherwise to the platform's backend.
macro_rules! backend {
    ($($call:tt)*) => {{
        #[cfg(feature = "test-support")]
        let answer = fake::$($call)*;
        #[cfg(not(feature = "test-support"))]
        let answer = imp::$($call)*;
        answer
    }};
}

/// Whether the mic button shows: Trek can take dictation here.
pub fn available() -> bool {
    backend!(available())
}

/// The mic button's tip: "Dictate", except where what's said goes online (a packaged Trek on
/// Windows, whose dictation is Windows' online recognizer), which says so.
pub fn tip() -> &'static str {
    backend!(tip())
}

/// The tip where dictation runs on the computer itself.
const DICTATE: &str = "Dictate";

/// Where Trek keeps what it can download again (the app passes `trek_core::paths::local_data_dir`
/// at launch: `%LOCALAPPDATA%`'s data folder on Windows, not the roaming one). Dictation that
/// needs a speech model of its own (Windows' on-device engine) keeps it in `dictation/` there.
pub fn keep_data_in(dir: fn() -> PathBuf) {
    let _ = DATA.set(dir);
}

static DATA: OnceLock<fn() -> PathBuf> = OnceLock::new();

/// Trek's local data folder, once the app has said where it is.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn data_dir() -> Option<PathBuf> {
    DATA.get().map(|dir| dir())
}

/// Dictation's one-time setup while it's under way (Windows' on-device engine downloading its
/// speech model, which the first ask waits for): how far it has got. `None` when there's nothing
/// to wait for.
pub fn getting_ready() -> Option<Progress> {
    backend!(getting_ready())
}

/// How far a download has got, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

impl Progress {
    /// 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.
        } else {
            (self.done.min(self.total) as f64 / self.total as f64) as f32
        }
    }

    /// "31 of 141 MB", in Windows Explorer's megabytes (1,048,576 bytes), the part done rounded
    /// down so it reads the total only once it's all there.
    pub fn said(&self) -> String {
        format!(
            "{} of {} MB",
            megabytes(self.done.min(self.total), true),
            megabytes(self.total, false)
        )
    }
}

/// Whole megabytes (1,048,576 bytes): rounded down for a part `done`, to the nearest otherwise.
pub fn megabytes(bytes: u64, done: bool) -> u64 {
    const MB: u64 = 1 << 20;
    if done {
        bytes / MB
    } else {
        (bytes + MB / 2) / MB
    }
}

/// The speech model dictation runs on this PC, where it needs one of its own (Windows'
/// on-device engine; `None` on a Mac, or where Windows' own recognizer listens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// What it's called: "Whisper base (multilingual)".
    pub name: &'static str,
    pub bytes: u64,
    /// Downloaded and in place.
    pub on_disk: bool,
    /// The folder it's kept in.
    pub folder: Option<PathBuf>,
}

/// The speech model, as it stands now.
pub fn model() -> Option<Model> {
    backend!(model())
}

/// Delete the downloaded speech model (and any half-finished download of it); the next take
/// downloads it again. A take under way keeps the copy it loaded.
pub fn remove_model() -> Result<(), String> {
    backend!(remove_model())
}

/// How long ✓ waits for a take's text before what was shown goes in, unless the take says
/// otherwise (`Dictation::grace`).
pub const GRACE: Duration = Duration::from_millis(1500);

/// Where Trek can't take dictation but the system's own dictation can (Windows' Win+H): what a
/// dimmed mic says instead. `None` where Trek can, or where there's nothing to point to.
pub fn fallback() -> Option<&'static str> {
    backend!(fallback())
}

/// Open the system's own dictation into the text field with the focus, where Trek's can't
/// take it ([`fallback`]): Windows' voice typing (Win+H). False where there's none to open, or
/// it couldn't be.
pub fn open_system_dictation() -> bool {
    backend!(open_system_dictation())
}

/// `reply` hears `Ok` once dictation may start (the system's permissions are in, and on Windows
/// the mic is open and listening), or why it may not. It can answer a while after it's called.
pub fn ensure_permission(reply: Sender<Result<(), String>>) {
    backend!(ensure_permission(reply))
}

/// Open a take, once [`ensure_permission`] said yes: what it hears goes to `heard`.
pub fn start(heard: Sender<Heard>) -> Result<Dictation, String> {
    backend!(start(heard)).map(Dictation)
}

/// A take [`ensure_permission`] said yes to but the composer gave up before it started (✕ or
/// another draft while the mic was starting): whatever was opened for it lets go of the mic.
/// macOS opens nothing until [`start`].
pub fn abandon() {
    backend!(abandon())
}

/// The microphone permission, without asking.
pub fn mic_access() -> Access {
    backend!(mic_access())
}

/// Speech recognition's permission (online speech recognition, on Windows), without asking.
pub fn speech_access() -> Access {
    backend!(speech_access())
}

/// The system's way to allow the microphone: its prompt on macOS (`done` hears the answer, on
/// another thread), its Settings page on Windows (`done` hears where it stands now).
pub fn ask_mic(done: impl Fn(bool) + Send + Sync + 'static) {
    #[cfg(feature = "test-support")]
    fake::ask(done);
    #[cfg(not(feature = "test-support"))]
    imp::ask_mic(done);
}

/// The same for speech recognition.
pub fn ask_speech(done: impl Fn(bool) + Send + Sync + 'static) {
    #[cfg(feature = "test-support")]
    fake::ask(done);
    #[cfg(not(feature = "test-support"))]
    imp::ask_speech(done);
}

/// A take over an existing audio file — no mic, but Speech permission still applies. The shots
/// harness drives this (`dictate-file`) to prove the transcription path without a clickable
/// permission dialog.
#[cfg(feature = "shots")]
pub fn transcribe_file(
    file: std::path::PathBuf,
    heard: Sender<Heard>,
) -> Result<Dictation, String> {
    #[cfg(feature = "test-support")]
    let take = {
        let _ = (file, heard);
        Err("No recordings in a test.".to_string())
    };
    #[cfg(not(feature = "test-support"))]
    let take = imp::transcribe_file(file, heard);
    take.map(Dictation)
}

/// One take: the mic open while it lives, and the recognizer turning it into text. Dropping it
/// ends the take at the mic.
pub struct Dictation(
    #[cfg(not(feature = "test-support"))] imp::Dictation,
    #[cfg(feature = "test-support")] fake::Take,
);

impl Dictation {
    /// Partial text comes while the user speaks (else the text arrives once, after ✓).
    pub fn live(&self) -> bool {
        self.0.live()
    }

    /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
    pub fn level(&self) -> f32 {
        self.0.level()
    }

    /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
    pub fn finish(&mut self) {
        self.0.finish()
    }

    /// How long, after [`finish`](Self::finish), the take's text may take before what was shown
    /// goes in instead: [`GRACE`], or longer for a recognizer that decodes the take once more on
    /// this PC (Windows' on-device engine, as long as its passes have been taking).
    pub fn grace(&self) -> Duration {
        self.0.grace()
    }
}

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

/// Where one of dictation's privacy permissions stands (onboarding's permissions step).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Granted,
    /// Never asked: asking shows the system's prompt.
    NotAsked,
    /// Turned down (or restricted): only the system's settings can change it now.
    Denied,
    /// This build can't ask: no usage descriptions in its Info.plist (a bare `cargo run`), or a
    /// system without the API.
    Unavailable,
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
// Under `test-support` the fake answers every call instead.
#[cfg_attr(feature = "test-support", allow(dead_code))]
mod imp {
    use super::*;
    use block2::RcBlock;
    use objc2::AllocAnyThread as _;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_avf_audio::{
        AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioCommonFormat, AVAudioEngine,
        AVAudioFormat, AVAudioPCMBuffer, AVAudioRecorder, AVAudioTime,
    };
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL, ns_string};
    use objc2_speech::{
        SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionRequest,
        SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognizer,
        SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest,
    };
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

    /// A build that can't ask hides the mic rather than pointing to macOS's own dictation.
    pub fn fallback() -> Option<&'static str> {
        None
    }

    pub fn tip() -> &'static str {
        crate::DICTATE
    }

    pub fn open_system_dictation() -> bool {
        false
    }

    /// The Speech framework has its models already: nothing to download, nothing to keep.
    pub fn getting_ready() -> Option<Progress> {
        None
    }

    pub fn model() -> Option<Model> {
        None
    }

    pub fn remove_model() -> Result<(), String> {
        Ok(())
    }

    /// Nothing is opened before `start`, so a take given up while asking holds nothing.
    pub fn abandon() {}

    /// The bundle declares microphone and speech use — and AVAudioApplication asks for them,
    /// so macOS 13 and older never show the button.
    fn packaged() -> bool {
        if objc2::runtime::AnyClass::get(c"AVAudioApplication").is_none() {
            return false;
        }
        let info = NSBundle::mainBundle();
        info.objectForInfoDictionaryKey(ns_string!("NSMicrophoneUsageDescription"))
            .is_some()
            && info
                .objectForInfoDictionaryKey(ns_string!("NSSpeechRecognitionUsageDescription"))
                .is_some()
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
                    let _ = reply.try_send(Err(
                        "Trek has no microphone access — allow it in System Settings › Privacy."
                            .into(),
                    ));
                }
                _ => {
                    let again = reply.clone();
                    let block = RcBlock::new(move |granted: Bool| {
                        if granted.as_bool() {
                            speech_permission(again.clone());
                        } else {
                            let _ =
                                again.try_send(Err("Trek needs the microphone to dictate.".into()));
                        }
                    });
                    AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block);
                }
            }
        }
    }

    /// The microphone permission, without asking.
    pub fn mic_access() -> Access {
        if !packaged() {
            return Access::Unavailable;
        }
        match unsafe { AVAudioApplication::sharedInstance().recordPermission() } {
            p if p == AVAudioApplicationRecordPermission::Granted => Access::Granted,
            p if p == AVAudioApplicationRecordPermission::Denied => Access::Denied,
            _ => Access::NotAsked,
        }
    }

    /// The Speech Recognition permission, without asking.
    pub fn speech_access() -> Access {
        if !packaged() {
            return Access::Unavailable;
        }
        match unsafe { SFSpeechRecognizer::authorizationStatus() } {
            s if s == SFSpeechRecognizerAuthorizationStatus::Authorized => Access::Granted,
            s if s == SFSpeechRecognizerAuthorizationStatus::NotDetermined => Access::NotAsked,
            _ => Access::Denied,
        }
    }

    /// Show macOS's microphone prompt (only a build that declares the mic can: see `packaged`).
    /// `done` hears whether it was allowed, on another thread.
    pub fn ask_mic(done: impl Fn(bool) + Send + Sync + 'static) {
        if !packaged() {
            return done(false);
        }
        let block = RcBlock::new(move |granted: Bool| done(granted.as_bool()));
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&block) };
    }

    /// Show macOS's Speech Recognition prompt; `done` hears whether it was allowed.
    pub fn ask_speech(done: impl Fn(bool) + Send + Sync + 'static) {
        if !packaged() {
            return done(false);
        }
        let block = RcBlock::new(move |status| {
            done(status == SFSpeechRecognizerAuthorizationStatus::Authorized)
        });
        unsafe { SFSpeechRecognizer::requestAuthorization(&block) };
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
        Live {
            engine: Retained<AVAudioEngine>,
            request: Retained<SFSpeechAudioBufferRecognitionRequest>,
            level: Arc<AtomicU32>,
            open: bool,
        },
        /// The fallback: a recording to `file`, transcribed once it stops. `ours`: `start`
        /// made the file and Drop removes it (`transcribe_file` borrows the caller's).
        File {
            recorder: Option<Retained<AVAudioRecorder>>,
            file: PathBuf,
            ours: bool,
        },
    }

    impl Dictation {
        /// Partial text comes while the user speaks (the audio-buffer request).
        pub fn live(&self) -> bool {
            matches!(self.source, Source::Live { .. })
        }

        /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
        pub fn level(&self) -> f32 {
            match &self.source {
                Source::Live {
                    level, open: true, ..
                } => f32::from_bits(level.swap(0, Ordering::Relaxed)),
                Source::File {
                    recorder: Some(r), ..
                } => unsafe {
                    r.updateMeters();
                    loudness_db(r.averagePowerForChannel(0))
                },
                _ => 0.,
            }
        }

        /// Speech settles a take's text quickly once its audio ends.
        pub fn grace(&self) -> std::time::Duration {
            GRACE
        }

        /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
        pub fn finish(&mut self) {
            match &mut self.source {
                Source::Live {
                    engine,
                    request,
                    open,
                    ..
                } => {
                    if std::mem::take(open) {
                        unsafe {
                            engine.inputNode().removeTapOnBus(0);
                            engine.stop();
                            request.endAudio();
                        }
                    }
                }
                Source::File { recorder, file, .. } => {
                    let Some(recorder) = recorder.take() else {
                        return;
                    };
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
                Source::Live {
                    engine,
                    request,
                    open,
                    ..
                } => {
                    if std::mem::take(open) {
                        unsafe {
                            engine.inputNode().removeTapOnBus(0);
                            engine.stop();
                            request.endAudio();
                        }
                    }
                }
                Source::File {
                    recorder,
                    file,
                    ours,
                } => {
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
        Ok(Dictation {
            source: Source::File {
                recorder: None,
                file,
                ours: false,
            },
            task: Some(task),
            heard,
        })
    }

    /// The recognizer for the user's language, if Speech has one ready.
    fn recognizer() -> Result<Retained<SFSpeechRecognizer>, String> {
        let missing = || {
            format!(
                "Speech recognition isn't available on {}.",
                trek_words::words().this_computer
            )
        };
        unsafe {
            // `init` is failable (no supported locale); `new` would hand nil to Retained.
            let recognizer =
                SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()).ok_or_else(missing)?;
            if !recognizer.isAvailable() {
                return Err(missing());
            }
            Ok(recognizer)
        }
    }

    /// Start `request` on `recognizer`; every result and the failure go to `heard`.
    fn recognize(
        recognizer: &SFSpeechRecognizer,
        request: &SFSpeechRecognitionRequest,
        heard: Sender<Heard>,
    ) -> Retained<SFSpeechRecognitionTask> {
        unsafe {
            request.setAddsPunctuation(true);
            let block = RcBlock::new(
                move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                    if !result.is_null() {
                        let text = (*result).bestTranscription().formattedString().to_string();
                        let _ = heard.try_send(if (*result).isFinal() {
                            Heard::Final(text)
                        } else {
                            Heard::Partial(text)
                        });
                    } else if !error.is_null() {
                        let _ = heard
                            .try_send(Heard::Failed((*error).localizedDescription().to_string()));
                    }
                },
            );
            recognizer.recognitionTaskWithRequest_resultHandler(request, &block)
        }
    }

    /// Speech over `file`: `heard` gets the transcription, or why it failed.
    fn transcribe_task(
        file: &std::path::Path,
        heard: Sender<Heard>,
    ) -> Result<Retained<SFSpeechRecognitionTask>, String> {
        let recognizer = recognizer()?;
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let request = SFSpeechURLRecognitionRequest::initWithURL(
                SFSpeechURLRecognitionRequest::alloc(),
                &url,
            );
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
                RcBlock::new(
                    move |buffer: NonNull<AVAudioPCMBuffer>, _: NonNull<AVAudioTime>| {
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
                                let square = (0..frames)
                                    .map(|i| *first.add(i * stride))
                                    .map(|s| s * s)
                                    .sum::<f32>();
                                loudness_db(
                                    10. * (square / frames.max(1) as f32).max(1e-18).log10(),
                                )
                            };
                            level.fetch_max(heard.to_bits(), Ordering::Relaxed);
                        }
                    },
                )
            };
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&tap),
            );
            engine.prepare();
            if engine.startAndReturnError().is_err() {
                input.removeTapOnBus(0);
                return None;
            }
            let task = recognize(recognizer, &request, heard.clone());
            Some(Dictation {
                source: Source::Live {
                    engine,
                    request,
                    level,
                    open: true,
                },
                task: Some(task),
                heard,
            })
        }
    }

    /// The fallback take: record to a temp `.caf`, metered for the waveform.
    fn start_file(heard: Sender<Heard>) -> Result<Dictation, String> {
        unsafe {
            let file =
                std::env::temp_dir().join(format!("trek-dictate-{}.caf", std::process::id()));
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
            let recorder =
                AVAudioRecorder::initWithURL_format_error(AVAudioRecorder::alloc(), &url, &format)
                    .map_err(|e| e.localizedDescription().to_string())?;
            recorder.setMeteringEnabled(true);
            if !recorder.record() {
                return Err("Couldn't start recording.".into());
            }
            Ok(Dictation {
                source: Source::File {
                    recorder: Some(recorder),
                    file,
                    ours: true,
                },
                task: None,
                heard,
            })
        }
    }
}

/// Neither macOS nor Windows: no recognizer, so no mic.
#[cfg(not(any(target_os = "macos", windows)))]
// Under `test-support` the fake answers every call instead.
#[cfg_attr(feature = "test-support", allow(dead_code))]
mod imp {
    use super::*;

    pub struct Dictation;

    /// What a dictation asked for says.
    fn unavailable() -> String {
        "Dictation is on macOS and Windows only for now.".into()
    }

    impl Dictation {
        pub fn live(&self) -> bool {
            false
        }
        pub fn level(&self) -> f32 {
            0.
        }
        pub fn finish(&mut self) {}
        pub fn grace(&self) -> std::time::Duration {
            GRACE
        }
    }

    #[cfg(feature = "shots")]
    pub fn transcribe_file(_: std::path::PathBuf, _: Sender<Heard>) -> Result<Dictation, String> {
        Err(unavailable())
    }

    pub fn available() -> bool {
        false
    }

    pub fn fallback() -> Option<&'static str> {
        None
    }

    pub fn tip() -> &'static str {
        crate::DICTATE
    }

    pub fn getting_ready() -> Option<Progress> {
        None
    }

    pub fn model() -> Option<Model> {
        None
    }

    pub fn remove_model() -> Result<(), String> {
        Ok(())
    }

    pub fn open_system_dictation() -> bool {
        false
    }

    pub fn ensure_permission(reply: Sender<Result<(), String>>) {
        let _ = reply.try_send(Err(unavailable()));
    }

    pub fn abandon() {}

    pub fn mic_access() -> Access {
        Access::Unavailable
    }

    pub fn speech_access() -> Access {
        Access::Unavailable
    }

    pub fn ask_mic(done: impl Fn(bool) + Send + Sync + 'static) {
        done(false)
    }

    pub fn ask_speech(done: impl Fn(bool) + Send + Sync + 'static) {
        done(false)
    }

    pub fn start(_: Sender<Heard>) -> Result<Dictation, String> {
        Err(unavailable())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn there_is_no_dictation_here() {
            let said = super::unavailable();
            assert_eq!(said, "Dictation is on macOS and Windows only for now.");
            let (tx, _rx) = async_channel::unbounded();
            assert_eq!(super::start(tx).err(), Some(said.clone()));
            let (reply, answer) = async_channel::bounded(1);
            super::ensure_permission(reply);
            assert_eq!(answer.try_recv().ok(), Some(Err(said)));
            assert!(!super::available(), "no mic button here");
            assert_eq!(super::fallback(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Windows' backend, as Trek calls it, without opening anything: a test runs without package
    /// identity, which Windows' own recognizer never answers, so Trek dictates on this PC
    /// wherever its processor runs Whisper; Windows' voice typing is offered only where it
    /// can't, and never opens while Trek's own mic is there. Nothing online to allow, either.
    #[cfg(windows)]
    #[test]
    fn windows_dictates_on_this_pc_without_package_identity() {
        use crate::Access;
        use crate::windows as backend;
        assert!(!backend::packaged(), "a test runs without package identity");
        let ours = backend::local::can_run();
        let engine = if ours {
            backend::Engine::Local
        } else {
            backend::Engine::None
        };
        assert_eq!(backend::engine(), engine);
        assert_eq!(backend::available(), ours);
        assert_eq!(
            backend::fallback(),
            (!ours).then_some("Press Win+H to dictate")
        );
        assert_eq!(backend::tip(), "Dictate");
        if ours {
            assert!(
                !backend::open_system_dictation(),
                "Win+H never opens by itself"
            );
            assert_eq!(backend::speech_access(), Access::Granted);
            let model = backend::model().expect("the engine's model");
            assert_eq!(model.bytes, backend::model::BASE.bytes);
        }
    }

    #[test]
    fn levels_map_from_dbfs_and_samples() {
        assert_eq!(crate::loudness_db(-160.), 0.);
        assert_eq!(crate::loudness_db(0.), 1.);
        assert_eq!(crate::loudness_db(f32::NAN), 0.);
        assert_eq!(crate::loudness(&[]), 0.);
        assert!(crate::loudness(&[0.5, -0.5]) > 0.9);
    }
}
