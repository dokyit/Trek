//! Dictation on Windows, by one of two engines (`Engine`):
//!
//! - **On this PC** (`local`): Whisper over the microphone, as WASAPI records it, its speech
//!   model downloaded the first time the mic is clicked. It's what a Trek without package
//!   identity dictates with (Trek as unpacked, or installed by its setup): Windows' own
//!   recognizer listens for such a process but never answers it (seen live on Windows 11: the
//!   microphone's level moving for half a minute with no word back and no end to the session,
//!   while Windows' own voice typing, Win+H, typed into Trek's composer).
//! - **Windows' own** (`winrt`): `Windows.Media.SpeechRecognition`, the online dictation voice
//!   typing uses too, for a Trek with package identity, where Windows does answer.
//!
//! Where neither can run (a processor without AVX2, and no package identity) the mic offers
//! Windows' voice typing instead (`open_system_dictation`), and says why Trek's own isn't there.
//!
//! Windows asks desktop apps for nothing: the microphone and online speech recognition are
//! switches in Settings › Privacy & security. So asking (`ensure_permission`) reads them first
//! (`hints`: the microphone's for both engines, online speech recognition's for Windows' own),
//! and the first time one that's off is in the way its page opens, as macOS shows a prompt once.
//! Then the engine gets ready and opens the mic while the composer's mic spins (or shows the
//! model's download), so every way of failing to start is said before the waveform shows;
//! `start` takes the open recording.
//!
//! What's decided from what Windows and Whisper say (`logic`, `hints`, `audio`, `transcript`,
//! `model`'s decisions) is plain code every platform's tests run.

pub mod audio;
#[cfg(windows)]
pub mod capture;
pub mod hints;
#[cfg(windows)]
pub mod local;
pub mod logic;
pub mod model;
pub mod transcript;
#[cfg(windows)]
pub mod whisper;
#[cfg(windows)]
pub mod winrt;

#[cfg(windows)]
pub use self::live::*;

#[cfg(windows)]
mod live {
    use super::logic::{MICROPHONE_PAGE, Problem, SPEECH_PAGE};
    use super::{hints, local, winrt};
    use crate::{Access, Heard, Progress};
    use async_channel::Sender;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    /// Why Windows' own recognizer can't dictate on this PC, once something lasting said so
    /// (`Problem::lasting`): it's set aside for the rest of the session.
    static GONE: Mutex<Option<String>> = Mutex::new(None);

    /// What takes dictation in this process.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Engine {
        /// Windows' own recognizer: Trek has package identity, the recognizer is registered,
        /// and nothing has ruled it out this session.
        Windows,
        /// Whisper on this PC: wherever its processor can run it.
        Local,
        /// Neither: the mic offers Windows' voice typing.
        None,
    }

    /// The engine dictation uses now.
    pub fn engine() -> Engine {
        static REGISTERED: OnceLock<bool> = OnceLock::new();
        static PACKAGED: OnceLock<bool> = OnceLock::new();
        static LOCAL: OnceLock<bool> = OnceLock::new();
        if *REGISTERED.get_or_init(hints::recognizer_registered)
            && *PACKAGED.get_or_init(packaged)
            && GONE.lock().is_ok_and(|gone| gone.is_none())
        {
            Engine::Windows
        } else if *LOCAL.get_or_init(local::can_run) {
            Engine::Local
        } else {
            Engine::None
        }
    }

    /// Whether the mic button shows: Trek can take dictation here.
    pub fn available() -> bool {
        engine() != Engine::None
    }

    /// This process runs with package identity (an MSIX, or a sparse package for its exe).
    pub(crate) fn packaged() -> bool {
        use ::windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
        use ::windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
        let mut len = 0u32;
        // SAFETY: asks only for the length (no buffer): an unpackaged process answers
        // APPMODEL_ERROR_NO_PACKAGE, a packaged one ERROR_INSUFFICIENT_BUFFER.
        let answer = unsafe { GetCurrentPackageFullName(&mut len, None) };
        answer != APPMODEL_ERROR_NO_PACKAGE
    }

    /// The mic's tip: Windows' own recognizer is online, so it says so.
    pub fn tip() -> &'static str {
        match engine() {
            Engine::Windows => "Dictate with Windows' online speech recognition",
            Engine::Local | Engine::None => crate::DICTATE,
        }
    }

    /// Open Windows' own voice typing (Win+H) into the text field that has the focus: the last
    /// resort, where neither of Trek's engines can run (`available`). False when the keys
    /// couldn't be sent.
    pub fn open_system_dictation() -> bool {
        use ::windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
            SendInput, VIRTUAL_KEY, VK_H, VK_LWIN,
        };
        if available() {
            return false;
        }
        let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let keys = [
            key(VK_LWIN, false),
            key(VK_H, false),
            key(VK_H, true),
            key(VK_LWIN, true),
        ];
        // SAFETY: four keyboard inputs, each the size SendInput is told.
        let sent = unsafe { SendInput(&keys, std::mem::size_of::<INPUT>() as i32) };
        sent as usize == keys.len()
    }

    /// Where Trek can't dictate, Windows' own dictation still can: the dimmed mic's tip.
    pub fn fallback() -> Option<&'static str> {
        if available() {
            None
        } else {
            trek_words::words().dictate_fallback
        }
    }

    /// Why Trek can't dictate here: neither engine can run, and the on-device one is the one
    /// that could have.
    fn why_not() -> String {
        Problem::CantRun.sentence()
    }

    /// What a problem calls for beyond its sentence: one that lasts sets Windows' recognizer
    /// aside for the rest of the session, and a privacy switch that's off opens its page in
    /// Settings, the first time in a session it's in the way.
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

    /// `reply` hears `Ok` once the engine is ready and the mic open, or why not: a switch
    /// that's off (read first, so nothing is loaded for nothing), then whatever the engine says
    /// as it gets ready. The on-device engine downloads its model first, the first time.
    pub fn ensure_permission(reply: Sender<Result<(), String>>) {
        let engine = engine();
        if engine == Engine::None {
            let _ = reply.try_send(Err(why_not()));
            return;
        }
        let hints = hints::Hints::read();
        let precheck = match engine {
            Engine::Windows => hints.precheck(),
            _ => hints.microphone(),
        };
        if let Err(problem) = precheck {
            settle(&problem);
            let _ = reply.try_send(Err(problem.sentence()));
            return;
        }
        match engine {
            Engine::Windows => winrt::open(reply),
            _ => local::open(reply),
        }
    }

    /// The take: it takes the recording `ensure_permission` opened (or opens one, failing as
    /// `Heard::Failed`), so it never waits here.
    pub fn start(heard: Sender<Heard>) -> Result<Dictation, String> {
        match engine() {
            Engine::None => Err(why_not()),
            Engine::Local => local::claim(heard).map(Dictation::Local),
            Engine::Windows => {
                let level = Arc::new(AtomicU32::new(0));
                let take = winrt::claim(heard, level.clone())?;
                Ok(Dictation::Windows {
                    take,
                    level,
                    open: true,
                })
            }
        }
    }

    /// What `ensure_permission` started for a take given up before it started lets go: the
    /// download it's waiting on stops, or the mic it opened closes, now.
    pub fn abandon() {
        match engine() {
            Engine::Windows => winrt::abandon(),
            Engine::Local => local::abandon(),
            Engine::None => {}
        }
    }

    /// The on-device engine's model downloading, the first time.
    pub fn getting_ready() -> Option<Progress> {
        match engine() {
            Engine::Local => local::getting_ready(),
            _ => None,
        }
    }

    /// The on-device engine's speech model, where it's the engine.
    pub fn model() -> Option<crate::Model> {
        (engine() == Engine::Local).then(local::model)
    }

    pub fn remove_model() -> Result<(), String> {
        local::remove_model()
    }

    /// One take: listening until ✓ (`finish`) or the take is dropped.
    pub enum Dictation {
        /// Windows' own recognizer's session for take `take`; `level` is the loudest meter
        /// reading since the last `level` (see `winrt::Worker::tick`).
        Windows {
            take: u64,
            level: Arc<AtomicU32>,
            open: bool,
        },
        Local(local::Take),
    }

    impl Dictation {
        /// Partial text comes while the user speaks (always, but for a file decoded on this PC).
        pub fn live(&self) -> bool {
            match self {
                Dictation::Windows { .. } => true,
                Dictation::Local(take) => take.live(),
            }
        }

        /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
        pub fn level(&self) -> f32 {
            match self {
                Dictation::Windows {
                    level, open: true, ..
                } => f32::from_bits(level.swap(0, Ordering::Relaxed)),
                Dictation::Windows { .. } => 0.,
                Dictation::Local(take) => take.level(),
            }
        }

        /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
        pub fn finish(&mut self) {
            match self {
                Dictation::Windows { take, open, .. } => {
                    if std::mem::take(open) {
                        winrt::finish(*take);
                    }
                }
                Dictation::Local(take) => take.finish(),
            }
        }

        /// How long the take's text may take after ✓.
        pub fn grace(&self) -> Duration {
            match self {
                Dictation::Windows { .. } => crate::GRACE,
                Dictation::Local(take) => take.grace(),
            }
        }
    }

    /// A take abandoned (✕, the composer closed, the app quitting), or done with, ends at the
    /// mic: Windows' session is called off and its recognizer closed (the on-device take's own
    /// `Drop` stops it).
    impl Drop for Dictation {
        fn drop(&mut self) {
            if let Dictation::Windows { take, .. } = self {
                winrt::cancel(*take);
            }
        }
    }

    /// The microphone switches, without asking: Windows asks desktop apps nothing.
    pub fn mic_access() -> Access {
        hints::Hints::read().mic_access()
    }

    /// Online speech recognition, without asking; on this PC there's nothing to allow.
    pub fn speech_access() -> Access {
        match engine() {
            Engine::Windows => hints::Hints::read().speech_access(),
            Engine::Local => Access::Granted,
            Engine::None => Access::Unavailable,
        }
    }

    /// Windows has no prompt for desktop apps: its microphone page in Settings instead. `done`
    /// hears whether it's allowed now.
    pub fn ask_mic(done: impl Fn(bool) + Send + Sync + 'static) {
        winrt::open_settings(MICROPHONE_PAGE);
        done(mic_access() == Access::Granted)
    }

    /// The speech page in Settings, where online speech recognition is turned on (nothing to
    /// turn on for dictation on this PC).
    pub fn ask_speech(done: impl Fn(bool) + Send + Sync + 'static) {
        if engine() == Engine::Windows {
            winrt::open_settings(SPEECH_PAGE);
        }
        done(speech_access() == Access::Granted)
    }

    /// A recording decoded on this PC as a take (Windows' own recognizer listens to the
    /// microphone only).
    #[cfg(feature = "shots")]
    pub fn transcribe_file(
        file: std::path::PathBuf,
        heard: Sender<Heard>,
    ) -> Result<Dictation, String> {
        if !local::can_run() {
            return Err(why_not());
        }
        local::transcribe_file(file, heard).map(Dictation::Local)
    }
}

#[cfg(windows)]
use self::live::settle;
