//! The thread dictation's WinRT and COM objects live on, in the multithreaded apartment. An ask
//! (`open`) readies Windows' recognizer for dictation and opens the mic; the take that starts
//! (`claim`) takes that session, and ✓ (`finish`) or the take's end (`cancel`) stops it. Windows'
//! events arrive on its own threads and are posted here as messages, so everything about a
//! session happens on this thread, in order, through `logic::Phrases`.
//!
//! Waiting on Windows is bounded (`within`): a recognizer that never answers fails the ask instead
//! of holding the thread. A session nobody claims (the composer went away while the mic opened)
//! lets go of the mic after `CLAIM`.

use super::logic::{self, Phrases, Problem};
use crate::Heard;
use ::windows::Foundation::{TimeSpan, TypedEventHandler};
use ::windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionConfidence, SpeechRecognitionHypothesisGeneratedEventArgs,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
    SpeechRecognizer,
};
use ::windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use ::windows::Win32::Media::Audio::{
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole,
};
use ::windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED,
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use ::windows::core::{HSTRING, PCWSTR, Ref, w};
use async_channel::Sender;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// How long Windows may take to ready the recognizer (the first one of a session loads the speech
/// engine), and to open the mic.
const COMPILE: Duration = Duration::from_secs(10);
const START: Duration = Duration::from_secs(10);
/// How long calling a session off may take before the recognizer is closed anyway.
const CANCEL: Duration = Duration::from_secs(2);
/// How long an open session waits for its take before it lets go of the mic.
const CLAIM: Duration = Duration::from_secs(3);
/// How long a stopped session waits for Windows to complete it (`Completed`) before its take ends
/// with what was heard, and the session is called off: Windows doesn't always say (seen with
/// online recognition still busy with the last words).
const SETTLE: Duration = Duration::from_secs(5);
/// How often the meter is read while a take listens: faster than the waveform's 20 fps, which
/// takes the loudest reading since its last.
const TICK: Duration = Duration::from_millis(25);
/// A minute of silence ends a take (what was heard goes in), not Windows' default of 20 s: a
/// pause to think shouldn't end it.
const SILENCE: TimeSpan = TimeSpan {
    Duration: 60 * 10_000_000,
};

enum Msg {
    /// An ask: ready the recognizer and open the mic, and answer once it's listening.
    Open(Sender<Result<(), String>>),
    /// `start`: take `take` takes an open session (or opens one), hearing through `heard`.
    Claim {
        take: u64,
        heard: Sender<Heard>,
        level: Arc<AtomicU32>,
    },
    /// The ask was given up before its take started: let go of the session it opened.
    Abandon,
    /// ✓: stop the mic; Windows settles the last phrase, then completes the session.
    Finish(u64),
    /// The take is over: call its session off and close the recognizer.
    Cancel(u64),
    /// What Windows says about session `n`, from its own threads.
    Hypothesis(u64, String),
    Phrase {
        session: u64,
        text: String,
        rejected: bool,
    },
    Completed(u64, i32),
}

/// Ready Windows' recognizer and open the mic; `reply` hears `Ok` once it's listening, or why
/// it couldn't (and `super::settle` has done what that calls for).
pub fn open(reply: Sender<Result<(), String>>) {
    if !post(Msg::Open(reply.clone())) {
        let _ = reply.try_send(Err(Problem::NoAnswer.sentence()));
    }
}

/// The take that starts: what the open session hears goes to `heard`, and its loudness to
/// `level`. The take's id names it for `finish` and `cancel`.
pub fn claim(heard: Sender<Heard>, level: Arc<AtomicU32>) -> Result<u64, String> {
    static TAKES: AtomicU64 = AtomicU64::new(1);
    let take = TAKES.fetch_add(1, Ordering::Relaxed);
    if post(Msg::Claim { take, heard, level }) {
        Ok(take)
    } else {
        Err(Problem::NoAnswer.sentence())
    }
}

pub fn abandon() {
    post(Msg::Abandon);
}

pub fn finish(take: u64) {
    post(Msg::Finish(take));
}

pub fn cancel(take: u64) {
    post(Msg::Cancel(take));
}

/// Hand `msg` to the thread, starting it the first time. `false` if it isn't there to take it.
fn post(msg: Msg) -> bool {
    static INBOX: OnceLock<mpsc::Sender<Msg>> = OnceLock::new();
    INBOX
        .get_or_init(|| {
            let (inbox, messages) = mpsc::channel();
            let posts = inbox.clone();
            let _ = std::thread::Builder::new()
                .name("trek-dictate".into())
                .spawn(move || Worker::new(posts).run(messages));
            inbox
        })
        .send(msg)
        .is_ok()
}

/// The thread's state: the sessions open now, and the meter while any is.
struct Worker {
    /// For Windows' event handlers, which post what they hear back here.
    posts: mpsc::Sender<Msg>,
    sessions: Vec<Session>,
    meter: Option<IAudioMeterInformation>,
    next: u64,
}

/// Windows listening through the mic: a recognizer readied for dictation and its continuous
/// session, for a take once one claims it.
struct Session {
    n: u64,
    /// The take it's for, once one took it. Until then what it hears is only kept, and goes to
    /// the take as it claims the session.
    take: Option<Take>,
    opened: Instant,
    /// When its take was finished (✓): Windows completes it soon after, or `SETTLE` ends it.
    stopped: Option<Instant>,
    recognizer: SpeechRecognizer,
    session: SpeechContinuousRecognitionSession,
    /// The handlers' registrations: HypothesisGenerated, ResultGenerated, Completed.
    tokens: [i64; 3],
    phrases: Phrases,
    /// The speech language's name, for a failure that's about it.
    language: String,
}

struct Take {
    id: u64,
    heard: Sender<Heard>,
    level: Arc<AtomicU32>,
}

impl Worker {
    fn new(posts: mpsc::Sender<Msg>) -> Worker {
        Worker {
            posts,
            sessions: Vec::new(),
            meter: None,
            next: 1,
        }
    }

    fn run(mut self, messages: mpsc::Receiver<Msg>) {
        // SAFETY: the first COM call on this thread, which is Trek's own; it stays in the
        // multithreaded apartment for as long as the process runs, and every object below is
        // made and released on it.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        loop {
            let msg = if self.sessions.is_empty() {
                match messages.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                }
            } else {
                match messages.recv_timeout(TICK) {
                    Ok(msg) => Some(msg),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            };
            if let Some(msg) = msg {
                self.handle(msg);
            }
            self.tick();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Open(reply) => {
                let answer = self.open().map_err(|problem| {
                    super::settle(&problem);
                    problem.sentence()
                });
                let _ = reply.try_send(answer);
            }
            Msg::Claim { take, heard, level } => {
                let take = Take {
                    id: take,
                    heard,
                    level,
                };
                if self.sessions.iter().all(|s| s.take.is_some())
                    && let Err(problem) = self.open()
                {
                    // Started without an ask (or after its session ended): it opens now, and a
                    // failure comes as the take's.
                    super::settle(&problem);
                    let _ = take.heard.try_send(Heard::Failed(problem.sentence()));
                    return;
                }
                let Some(s) = self.sessions.iter_mut().find(|s| s.take.is_none()) else {
                    return;
                };
                // Words heard between the ask's answer and now.
                let so_far = s.phrases.text();
                if !so_far.is_empty() {
                    let _ = take.heard.try_send(Heard::Partial(so_far));
                }
                s.take = Some(take);
            }
            Msg::Abandon => {
                if let Some(i) = self.sessions.iter().position(|s| s.take.is_none()) {
                    self.sessions.remove(i).cancel();
                }
            }
            Msg::Finish(id) => {
                let Some(i) = self.position_of(id) else {
                    return;
                };
                let s = &mut self.sessions[i];
                s.phrases.finish();
                s.stopped = Some(Instant::now());
                // Windows settles what it heard last, then completes the session (`Completed`).
                if s.session.StopAsync().is_err() {
                    // It can't be stopped (it ended already): what was heard is the take's text.
                    let mut s = self.sessions.remove(i);
                    let said = s.phrases.completed(logic::status::SUCCESS, &s.language);
                    s.tell(said);
                }
            }
            Msg::Cancel(id) => {
                if let Some(i) = self.position_of(id) {
                    self.sessions.remove(i).cancel();
                }
            }
            Msg::Hypothesis(n, text) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.n == n) {
                    let said = s.phrases.hypothesis(&text);
                    s.tell(said);
                }
            }
            Msg::Phrase {
                session,
                text,
                rejected,
            } => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.n == session) {
                    let said = s.phrases.result(&text, rejected);
                    s.tell(said);
                }
            }
            Msg::Completed(n, status) => {
                if let Some(i) = self.sessions.iter().position(|s| s.n == n) {
                    let mut s = self.sessions.remove(i);
                    let said = s.phrases.completed(status, &s.language);
                    s.tell(said);
                }
            }
        }
    }

    fn position_of(&self, take: u64) -> Option<usize> {
        self.sessions
            .iter()
            .position(|s| s.take.as_ref().is_some_and(|t| t.id == take))
    }

    /// Between messages: an open session nobody claimed in time lets go of the mic, and while a
    /// take listens the meter's reading goes to it.
    fn tick(&mut self) {
        while let Some(i) = self
            .sessions
            .iter()
            .position(|s| s.take.is_none() && s.opened.elapsed() > CLAIM)
        {
            self.sessions.remove(i).cancel();
        }
        // Finished, and Windows never completed it: what was heard is the take's text.
        while let Some(i) = self
            .sessions
            .iter()
            .position(|s| s.stopped.is_some_and(|at| at.elapsed() > SETTLE))
        {
            let mut s = self.sessions.remove(i);
            let said = s.phrases.completed(logic::status::SUCCESS, &s.language);
            s.tell(said);
            s.cancel();
        }
        if self.sessions.is_empty() {
            self.meter = None;
            return;
        }
        let Some(meter) = &self.meter else {
            return;
        };
        // SAFETY: the meter was activated on this thread, which is the only one that uses it.
        let Ok(peak) = (unsafe { meter.GetPeakValue() }) else {
            return;
        };
        let level = logic::loudness_peak(peak).to_bits();
        for s in &self.sessions {
            if let Some(take) = s.take.as_ref().filter(|_| !s.phrases.finishing()) {
                // Non-negative floats order as their bits do, so the loudest reading wins.
                take.level.fetch_max(level, Ordering::Relaxed);
            }
        }
    }

    /// Ready a recognizer for dictation in the system's speech language and start its continuous
    /// session, which opens the mic.
    fn open(&mut self) -> Result<(), Problem> {
        let problem = |e: ::windows::core::Error| Problem::from_code(e.code().0);
        // No capture endpoint, no point loading the engine.
        microphone().map_err(problem)?;
        // Nor in listening to a muted one: it hears silence, and the take ends with nothing.
        if microphone_muted() {
            return Err(Problem::MicrophoneMuted);
        }
        let recognizer = SpeechRecognizer::new().map_err(problem)?;
        let language = recognizer
            .CurrentLanguage()
            .and_then(|l| l.DisplayName())
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let n = self.next;
        self.next += 1;
        match self.listen(n, &recognizer, &language) {
            Ok((session, tokens)) => {
                self.sessions.push(Session {
                    n,
                    take: None,
                    opened: Instant::now(),
                    stopped: None,
                    recognizer,
                    session,
                    tokens,
                    phrases: Phrases::default(),
                    language,
                });
                if self.meter.is_none() {
                    self.meter = meter().ok();
                }
                Ok(())
            }
            Err(problem) => {
                let _ = recognizer.Close();
                Err(problem)
            }
        }
    }

    /// The dictation constraint compiled on `recognizer`, its handlers posting what they hear as
    /// session `n`, and the session started.
    fn listen(
        &self,
        n: u64,
        recognizer: &SpeechRecognizer,
        language: &str,
    ) -> Result<(SpeechContinuousRecognitionSession, [i64; 3]), Problem> {
        let problem = |e: ::windows::core::Error| Problem::from_code(e.code().0);
        let dictation = SpeechRecognitionTopicConstraint::Create(
            SpeechRecognitionScenario::Dictation,
            &HSTRING::from("dictation"),
        )
        .map_err(problem)?;
        recognizer
            .Constraints()
            .and_then(|c| c.Append(&dictation))
            .map_err(problem)?;
        let compiled = within(COMPILE, |done| {
            recognizer.CompileConstraintsAsync()?.when(move |r| {
                let _ = done.send(r.and_then(|c| c.Status()));
            })
        })?;
        if let Some(problem) = Problem::compiling(compiled.0, language) {
            return Err(problem);
        }
        let session = recognizer.ContinuousRecognitionSession().map_err(problem)?;
        let _ = session.SetAutoStopSilenceTimeout(SILENCE);

        let posts = self.posts.clone();
        let hypothesis = recognizer
            .HypothesisGenerated(&TypedEventHandler::new(
                move |_: Ref<SpeechRecognizer>,
                      args: Ref<SpeechRecognitionHypothesisGeneratedEventArgs>| {
                    if let Some(args) = args.as_ref() {
                        let text = args.Hypothesis()?.Text()?.to_string_lossy();
                        let _ = posts.send(Msg::Hypothesis(n, text));
                    }
                    Ok(())
                },
            ))
            .map_err(problem)?;
        let posts = self.posts.clone();
        let result = session.ResultGenerated(&TypedEventHandler::new(
            move |_: Ref<SpeechContinuousRecognitionSession>,
                  args: Ref<SpeechContinuousRecognitionResultGeneratedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    let result = args.Result()?;
                    let rejected = result.Status()? != SpeechRecognitionResultStatus::Success
                        || result.Confidence()? == SpeechRecognitionConfidence::Rejected;
                    let text = result.Text()?.to_string_lossy();
                    let _ = posts.send(Msg::Phrase {
                        session: n,
                        text,
                        rejected,
                    });
                }
                Ok(())
            },
        ));
        let posts = self.posts.clone();
        let completed = session.Completed(&TypedEventHandler::new(
            move |_: Ref<SpeechContinuousRecognitionSession>,
                  args: Ref<SpeechContinuousRecognitionCompletedEventArgs>| {
                if let Some(args) = args.as_ref() {
                    let _ = posts.send(Msg::Completed(n, args.Status()?.0));
                }
                Ok(())
            },
        ));
        let (result, completed) = match (result, completed) {
            (Ok(result), Ok(completed)) => (result, completed),
            (result, completed) => {
                let _ = recognizer.RemoveHypothesisGenerated(hypothesis);
                if let Ok(token) = result {
                    let _ = session.RemoveResultGenerated(token);
                }
                if let Ok(token) = completed {
                    let _ = session.RemoveCompleted(token);
                }
                return Err(Problem::Stopped);
            }
        };
        let tokens = [hypothesis, result, completed];
        if let Err(problem) = within(START, |done| {
            session.StartAsync()?.when(move |r| {
                let _ = done.send(r);
            })
        }) {
            // Opening may still be under way: call it off, so the mic isn't left open.
            let _ = session.CancelAsync();
            let _ = recognizer.RemoveHypothesisGenerated(tokens[0]);
            let _ = session.RemoveResultGenerated(tokens[1]);
            let _ = session.RemoveCompleted(tokens[2]);
            return Err(problem);
        }
        Ok((session, tokens))
    }
}

impl Session {
    /// What Phrases made of an event goes to the take, if one has claimed the session.
    fn tell(&self, said: Option<Heard>) {
        if let (Some(heard), Some(take)) = (said, &self.take) {
            let _ = take.heard.try_send(heard);
        }
    }

    /// Call the session off and let go of the mic; nothing it says from now on goes anywhere.
    fn cancel(mut self) {
        self.phrases.cancel();
        let _ = within(CANCEL, |done| {
            self.session.CancelAsync()?.when(move |r| {
                let _ = done.send(r);
            })
        });
        // Dropping closes the recognizer.
    }
}

/// A session over, however it ended: its handlers go, and closing the recognizer frees the mic
/// and the speech engine.
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.recognizer.RemoveHypothesisGenerated(self.tokens[0]);
        let _ = self.session.RemoveResultGenerated(self.tokens[1]);
        let _ = self.session.RemoveCompleted(self.tokens[2]);
        let _ = self.recognizer.Close();
    }
}

/// Wait up to `limit` for what `arm` asks of Windows, which hands its answer to `done` once
/// Windows has it.
fn within<T: Send + 'static>(
    limit: Duration,
    arm: impl FnOnce(mpsc::Sender<::windows::core::Result<T>>) -> ::windows::core::Result<()>,
) -> Result<T, Problem> {
    let (done, answer) = mpsc::channel();
    arm(done).map_err(|e| Problem::from_code(e.code().0))?;
    match answer.recv_timeout(limit) {
        Ok(answer) => answer.map_err(|e| Problem::from_code(e.code().0)),
        Err(_) => Err(Problem::NoAnswer),
    }
}

/// The default capture endpoint: the microphone Windows' recognizer listens with. Getting it
/// doesn't open it.
fn microphone() -> ::windows::core::Result<IMMDevice> {
    // SAFETY: COM is initialized on the calling thread (this module's thread, or the probe's
    // main); the enumerator is released when it drops.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        devices.GetDefaultAudioEndpoint(eCapture, eConsole)
    }
}

/// The default microphone is muted in Windows. One whose state can't be read counts as not.
fn microphone_muted() -> bool {
    microphone()
        // SAFETY: on the thread that made `device`; `Activate` makes the interface its type
        // names, with no activation parameters, and `GetMute` only reads.
        .and_then(|device| unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) })
        .and_then(|volume| unsafe { volume.GetMute() })
        .is_ok_and(|muted| muted.as_bool())
}

/// The default microphone's endpoint id, for the probe: whether Windows has one at all.
pub fn default_microphone() -> ::windows::core::Result<String> {
    let device = microphone()?;
    // SAFETY: `GetId` hands back a NUL-terminated string the caller owns, read once and freed
    // with the allocator it came from.
    unsafe {
        let id = device.GetId()?;
        let text = id.to_string().unwrap_or_default();
        CoTaskMemFree(Some(id.0 as *const _));
        Ok(text)
    }
}

/// The capture endpoint's peak meter, for the waveform. It reads what's already being captured
/// (the recognizer's session); it captures nothing itself.
fn meter() -> ::windows::core::Result<IAudioMeterInformation> {
    let device = microphone()?;
    // SAFETY: on the thread that made `device`; `Activate` makes the interface its type names,
    // with no activation parameters.
    unsafe { device.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) }
}

/// Open `page` (an `ms-settings:` link) in Settings. ShellExecute may hand a link to shell
/// extensions that want a single-threaded apartment, so it runs on a short thread of its own
/// rather than on the caller's or the worker's.
pub fn open_settings(page: &'static str) {
    let _ = std::thread::Builder::new()
        .name("trek-dictate-settings".into())
        .spawn(move || {
            // SAFETY: the first COM call on this new thread, undone below once it's done.
            let joined =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            let page = HSTRING::from(page);
            // SAFETY: both strings are NUL-terminated and outlive the call; no window owns it.
            unsafe {
                ::windows::Win32::UI::Shell::ShellExecuteW(
                    None,
                    w!("open"),
                    &page,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    ::windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
                );
            }
            if joined.is_ok() {
                // SAFETY: pairs the successful CoInitializeEx above, on the same thread.
                unsafe { CoUninitialize() };
            }
        });
}
