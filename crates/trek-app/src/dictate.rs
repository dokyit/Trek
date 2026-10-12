//! Speak instead of type: the composer's mic button records a take to a temp `.caf`, and
//! stopping it has the Speech framework transcribe the file into the text. macOS asks for the
//! microphone and Speech permissions the first time each is needed; a build without the usage
//! descriptions in its Info.plist — a bare `cargo run` binary — hides the button instead of
//! asking (TCC ends processes that ask without declaring).

use async_channel::Sender;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use block2::RcBlock;
    use objc2::AllocAnyThread as _;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioCommonFormat, AVAudioFormat, AVAudioRecorder};
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL, ns_string};
    use objc2_speech::{SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest};
    use std::path::PathBuf;

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

    /// One take: recording while it lives, transcribing after `finish`.
    pub struct Dictation {
        recorder: Option<Retained<AVAudioRecorder>>,
        file: PathBuf,
        /// `start` made the temp file and Drop removes it; `transcribe_file` borrows the
        /// caller's and leaves it.
        ours: bool,
        /// The Speech task while it transcribes; keeping it alive keeps the result coming.
        task: Option<Retained<SFSpeechRecognitionTask>>,
    }

    impl Dictation {
        /// The mic is still open.
        pub fn recording(&self) -> bool {
            self.recorder.is_some()
        }

        /// Stop recording and transcribe the take; `reply` gets the text or why it failed.
        pub fn finish(&mut self, reply: Sender<Result<String, String>>) {
            if let Some(recorder) = self.recorder.take() {
                unsafe { recorder.stop() };
            }
            match transcribe_task(&self.file, reply.clone()) {
                Ok(task) => self.task = Some(task),
                Err(e) => {
                    let _ = reply.try_send(Err(e));
                }
            }
        }
    }

    /// A take abandoned (the composer closed, the app is quitting) ends at the mic and leaves
    /// nothing in tmp.
    impl Drop for Dictation {
        fn drop(&mut self) {
            if let Some(recorder) = self.recorder.take() {
                unsafe { recorder.stop() };
            }
            if let Some(task) = self.task.take() {
                unsafe { task.cancel() };
            }
            if self.ours {
                let _ = std::fs::remove_file(&self.file);
            }
        }
    }

    /// A take over an existing audio file — no mic, but Speech permission still applies. The
    /// shots harness drives this (`dictate-file`) to prove the transcription path without a
    /// clickable permission dialog.
    #[cfg(feature = "shots")]
    pub fn transcribe_file(file: PathBuf, reply: Sender<Result<String, String>>) -> Result<Dictation, String> {
        let task = transcribe_task(&file, reply)?;
        Ok(Dictation { recorder: None, file, ours: false, task: Some(task) })
    }

    /// Speech over `file`: `reply` gets the transcription, or why it failed.
    fn transcribe_task(file: &std::path::Path, reply: Sender<Result<String, String>>) -> Result<Retained<SFSpeechRecognitionTask>, String> {
        unsafe {
            // `init` is failable (no supported locale); `new` would hand nil to Retained.
            let Some(recognizer) = SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) else {
                return Err(format!("Speech recognition isn't available on {}.", crate::words::words().this_computer));
            };
            if !recognizer.isAvailable() {
                return Err(format!("Speech recognition isn't available on {}.", crate::words::words().this_computer));
            }
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let request = SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url);
            request.setShouldReportPartialResults(false);
            request.setAddsPunctuation(true);
            let block = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                if !error.is_null() {
                    let _ = reply.try_send(Err((*error).localizedDescription().to_string()));
                } else if !result.is_null() && (*result).isFinal() {
                    let _ = reply.try_send(Ok((*result).bestTranscription().formattedString().to_string()));
                }
            });
            Ok(recognizer.recognitionTaskWithRequest_resultHandler(&request, &block))
        }
    }

    /// Start recording. Both permissions must be granted first — see [`ensure_permission`].
    pub fn start() -> Result<Dictation, String> {
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
            if !recorder.record() {
                return Err("Couldn't start recording.".into());
            }
            Ok(Dictation { recorder: Some(recorder), file, ours: true, task: None })
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// Dictation in Trek is macOS-only for now; Windows has its own (Win+H), which needs nothing
    /// from Trek.
    pub struct Dictation;

    /// What a dictation asked for says: on Windows the way to its own, else that it isn't here.
    fn unavailable() -> String {
        if cfg!(windows) { format!("{}.", crate::words::words().dictate_tip) } else { "Dictation is macOS-only for now.".into() }
    }

    impl Dictation {
        pub fn recording(&self) -> bool {
            false
        }
        pub fn finish(&mut self, _: Sender<Result<String, String>>) {}
    }

    #[cfg(feature = "shots")]
    pub fn transcribe_file(_: std::path::PathBuf, _: Sender<Result<String, String>>) -> Result<Dictation, String> {
        Err(unavailable())
    }

    pub fn available() -> bool {
        false
    }

    pub fn ensure_permission(_: Sender<Result<(), String>>) {}

    pub fn start() -> Result<Dictation, String> {
        Err(unavailable())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn windows_points_to_its_own_dictation() {
            let said = super::unavailable();
            if cfg!(windows) {
                assert_eq!(said, "Press Win+H to dictate.");
            } else {
                assert_eq!(said, "Dictation is macOS-only for now.");
            }
            assert_eq!(super::start().err(), Some(said));
            assert!(!super::available(), "no mic button here");
        }
    }
}

pub use imp::*;
