//! Dictation on Windows: Windows' own speech recognition (`Windows.Media.SpeechRecognition`, the
//! online dictation that voice typing, Win+H, uses too) in a continuous session. Hypotheses come
//! while the user speaks and phrases as they settle, each reported as the whole take so far, and
//! the take's text once ✓ stops it. The capture endpoint's meter draws the waveform.
//!
//! Windows asks desktop apps for nothing: the microphone and online speech recognition are
//! switches in Settings › Privacy & security. So asking (`ensure_permission`) reads them first
//! (`hints`), and the first time one that's off is in the way its page opens, as macOS shows a
//! prompt once. Then it readies the recognizer and opens the mic while the composer's mic spins,
//! so every way of failing to start is said before the waveform shows; `start` takes the open
//! session. Microsoft documents these APIs for packaged apps and Trek isn't one: where Windows
//! refuses it outright (no recognizer, or an organization turned online speech off), the mic
//! dims for the session and points to Win+H instead.
//!
//! Everything WinRT and COM lives on one thread (`winrt`); what's decided from what Windows says
//! (`logic`, `hints`) is plain code every platform's tests run.

pub mod hints;
pub mod logic;
#[cfg(windows)]
pub mod winrt;

#[cfg(windows)]
pub use self::live::*;

#[cfg(windows)]
mod live {
    use super::logic::{MICROPHONE_PAGE, Problem, SPEECH_PAGE};
    use super::{hints, winrt};
    use crate::{Access, Heard};
    use async_channel::Sender;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Why Trek can't dictate on this PC, once something lasting said so (`Problem::lasting`):
    /// the mic stays dimmed for the rest of the session.
    static GONE: Mutex<Option<String>> = Mutex::new(None);

    /// Whether the mic button shows: Windows' recognizer is registered, and nothing has said it
    /// can't work here since.
    pub fn available() -> bool {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        *REGISTERED.get_or_init(hints::recognizer_registered)
            && GONE.lock().is_ok_and(|gone| gone.is_none())
    }

    /// Where Trek can't dictate, Windows' own dictation still can: the dimmed mic's tip.
    pub fn fallback() -> Option<&'static str> {
        if available() {
            None
        } else {
            trek_words::words().dictate_fallback
        }
    }

    fn why_not() -> String {
        GONE.lock()
            .ok()
            .and_then(|gone| gone.clone())
            .unwrap_or_else(|| Problem::Unavailable.sentence())
    }

    /// What a problem calls for beyond its sentence: one that lasts dims the mic for the rest of
    /// the session, and a privacy switch that's off opens its page in Settings, the first time
    /// in a session it's in the way.
    pub(super) fn settle(problem: &Problem) {
        if problem.lasting()
            && let Ok(mut gone) = GONE.lock()
        {
            gone.get_or_insert_with(|| problem.sentence());
        }
        if let Some(page) = problem.switch() {
            static OPENED: Mutex<Vec<&str>> = Mutex::new(Vec::new());
            let first = OPENED.lock().is_ok_and(|mut opened| {
                let first = !opened.contains(&page);
                if first {
                    opened.push(page);
                }
                first
            });
            if first {
                winrt::open_settings(page);
            }
        }
    }

    /// `reply` hears `Ok` once the recognizer is ready and the mic open, or why not: a switch
    /// that's off (read first, so the engine isn't loaded for nothing), then whatever Windows
    /// says as the recognizer is readied and its session started.
    pub fn ensure_permission(reply: Sender<Result<(), String>>) {
        if !available() {
            let _ = reply.try_send(Err(why_not()));
            return;
        }
        if let Err(problem) = hints::Hints::read().precheck() {
            settle(&problem);
            let _ = reply.try_send(Err(problem.sentence()));
            return;
        }
        winrt::open(reply);
    }

    /// The take: it takes the session `ensure_permission` opened (or opens one, failing as
    /// `Heard::Failed`), so it never waits on Windows here.
    pub fn start(heard: Sender<Heard>) -> Result<Dictation, String> {
        if !available() {
            return Err(why_not());
        }
        let level = Arc::new(AtomicU32::new(0));
        let take = winrt::claim(heard, level.clone())?;
        Ok(Dictation {
            take,
            level,
            open: true,
        })
    }

    /// The session `ensure_permission` opened for a take given up before it started: the mic
    /// goes now, not when the open session gives up waiting.
    pub fn abandon() {
        winrt::abandon();
    }

    /// One take: Windows listening until ✓ (`finish`) or the take is dropped.
    pub struct Dictation {
        take: u64,
        /// The loudest reading since the last `level`, as f32 bits (see `winrt::Worker::tick`).
        level: Arc<AtomicU32>,
        open: bool,
    }

    impl Dictation {
        /// Always: Windows sends hypotheses while the user speaks.
        pub fn live(&self) -> bool {
            true
        }

        /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
        pub fn level(&self) -> f32 {
            if self.open {
                f32::from_bits(self.level.swap(0, Ordering::Relaxed))
            } else {
                0.
            }
        }

        /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
        pub fn finish(&mut self) {
            if std::mem::take(&mut self.open) {
                winrt::finish(self.take);
            }
        }
    }

    /// A take abandoned (✕, the composer closed, the app quitting), or done with, ends at the
    /// mic: its session is called off and the recognizer closed.
    impl Drop for Dictation {
        fn drop(&mut self) {
            winrt::cancel(self.take);
        }
    }

    /// The microphone switches, without asking: Windows asks desktop apps nothing.
    pub fn mic_access() -> Access {
        hints::Hints::read().mic_access()
    }

    /// Online speech recognition, without asking.
    pub fn speech_access() -> Access {
        hints::Hints::read().speech_access()
    }

    /// Windows has no prompt for desktop apps: its microphone page in Settings instead. `done`
    /// hears whether it's allowed now.
    pub fn ask_mic(done: impl Fn(bool) + Send + Sync + 'static) {
        winrt::open_settings(MICROPHONE_PAGE);
        done(mic_access() == Access::Granted)
    }

    /// The speech page in Settings, where online speech recognition is turned on.
    pub fn ask_speech(done: impl Fn(bool) + Send + Sync + 'static) {
        winrt::open_settings(SPEECH_PAGE);
        done(speech_access() == Access::Granted)
    }

    /// Windows' recognizer listens to the microphone only: there's no file to give it.
    #[cfg(feature = "shots")]
    pub fn transcribe_file(_: std::path::PathBuf, _: Sender<Heard>) -> Result<Dictation, String> {
        Err("Windows speech recognition can't transcribe a recording.".into())
    }
}

#[cfg(windows)]
use self::live::settle;
