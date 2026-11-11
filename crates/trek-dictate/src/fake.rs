//! Dictation without a microphone, for the UI tests. Under `test-support` every call lands here
//! instead of on the platform's recognizer: with no fake installed there's no dictation at all
//! (no mic, and asking says so), so no test ever opens a real microphone, and the layouts tests
//! check are the ones without a mic. `install` puts a fake on this thread (each GPUI test runs
//! on its own), and the test says what it allows and what it hears.
//!
//! The fake opens the mic as Windows does: on an ask that's allowed (`ensure_permission`), so a
//! take given up before it starts must be abandoned (`abandon`), and `mic_open` tells whether
//! anything still holds it.

use crate::{Access, Heard};
use async_channel::Sender;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static FAKE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
}

struct State {
    available: bool,
    /// With `available` off: the system's own dictation is there (`fallback`).
    fallback: bool,
    /// What asking answers instead of `Ok`.
    refusal: Option<String>,
    /// Asks wait for `answer` instead of being answered at once.
    holding: bool,
    pending: Vec<Sender<Result<(), String>>>,
    /// What `start` fails with.
    start_error: Option<String>,
    /// Asks allowed whose mic no take has claimed (or abandoned) yet.
    readied: usize,
    asked: usize,
    takes: usize,
    abandoned: usize,
    /// Times the system's own dictation was asked to open.
    system_opened: usize,
    /// The take alive now, and whether ✓ stopped its mic.
    alive: bool,
    finished: bool,
    heard: Option<Sender<Heard>>,
    level: f32,
}

/// The fake for this thread, while it lives.
pub struct Fake(Rc<RefCell<State>>);

/// Put a fake in for this thread's dictation: the mic shows, asking allows it, and a take opens
/// live, hearing what the test says (`hear`).
pub fn install() -> Fake {
    let state = Rc::new(RefCell::new(State {
        available: true,
        fallback: false,
        refusal: None,
        holding: false,
        pending: Vec::new(),
        start_error: None,
        readied: 0,
        asked: 0,
        takes: 0,
        abandoned: 0,
        system_opened: 0,
        alive: false,
        finished: false,
        heard: None,
        level: 0.,
    }));
    FAKE.with(|f| *f.borrow_mut() = Some(state.clone()));
    Fake(state)
}

impl Drop for Fake {
    fn drop(&mut self) {
        FAKE.with(|f| {
            let mut f = f.borrow_mut();
            if f.as_ref().is_some_and(|s| Rc::ptr_eq(s, &self.0)) {
                *f = None;
            }
        });
    }
}

impl Fake {
    /// Asking is turned down: `why` is what the composer says.
    pub fn refuse(&self, why: &str) {
        self.0.borrow_mut().refusal = Some(why.into());
    }

    /// Trek can't dictate here (no recognizer): no mic, except a dimmed one pointing to the
    /// system's own dictation where the platform the test pretends to be has one. Asking says
    /// `why`.
    pub fn unavailable(&self, why: &str) {
        let mut s = self.0.borrow_mut();
        s.available = false;
        s.fallback = true;
        s.refusal = Some(why.into());
    }

    /// Asks wait, as the system's prompt (or Windows readying its recognizer) does, until `answer`.
    pub fn hold(&self) {
        self.0.borrow_mut().holding = true;
    }

    /// Answer the asks waiting, and those to come at once.
    pub fn answer(&self) {
        let pending = {
            let mut s = self.0.borrow_mut();
            s.holding = false;
            std::mem::take(&mut s.pending)
        };
        for reply in pending {
            answer(&self.0, reply);
        }
    }

    /// The take fails to start, saying `why`.
    pub fn fail_start(&self, why: &str) {
        self.0.borrow_mut().start_error = Some(why.into());
    }

    /// What the take hears, as the recognizer would say it.
    pub fn hear(&self, heard: Heard) {
        if let Some(tx) = self.0.borrow().heard.as_ref() {
            let _ = tx.try_send(heard);
        }
    }

    /// How loud the mic is, for the waveform.
    pub fn set_level(&self, level: f32) {
        self.0.borrow_mut().level = level;
    }

    /// A take is open and its mic hasn't been stopped by ✓.
    pub fn listening(&self) -> bool {
        let s = self.0.borrow();
        s.alive && !s.finished
    }

    /// Anything holds the mic: a take listening, or an allowed ask no take has claimed.
    pub fn mic_open(&self) -> bool {
        self.listening() || self.0.borrow().readied > 0
    }

    /// ✓ stopped the take's mic.
    pub fn finished(&self) -> bool {
        self.0.borrow().finished
    }

    /// Asks so far.
    pub fn asked(&self) -> usize {
        self.0.borrow().asked
    }

    /// Takes started so far.
    pub fn takes(&self) -> usize {
        self.0.borrow().takes
    }

    /// Asks given up before their take started.
    pub fn abandoned(&self) -> usize {
        self.0.borrow().abandoned
    }

    /// Times the mic asked for the system's own dictation (Windows' voice typing) to open.
    pub fn system_opened(&self) -> usize {
        self.0.borrow().system_opened
    }
}

fn installed() -> Option<Rc<RefCell<State>>> {
    FAKE.with(|f| f.borrow().clone())
}

fn answer(state: &Rc<RefCell<State>>, reply: Sender<Result<(), String>>) {
    let refusal = state.borrow().refusal.clone();
    let answer = match refusal {
        Some(why) => Err(why),
        None => {
            state.borrow_mut().readied += 1;
            Ok(())
        }
    };
    let _ = reply.try_send(answer);
}

/// What asking says in a test that installed no fake.
const NONE_HERE: &str = "No dictation in this test.";

pub(crate) fn available() -> bool {
    installed().is_some_and(|s| s.borrow().available)
}

pub(crate) fn fallback() -> Option<&'static str> {
    let s = installed()?;
    let s = s.borrow();
    if !s.available && s.fallback {
        trek_words::words().dictate_fallback
    } else {
        None
    }
}

pub(crate) fn ensure_permission(reply: Sender<Result<(), String>>) {
    let Some(state) = installed() else {
        let _ = reply.try_send(Err(NONE_HERE.into()));
        return;
    };
    let holding = {
        let mut s = state.borrow_mut();
        s.asked += 1;
        s.holding
    };
    if holding {
        state.borrow_mut().pending.push(reply);
    } else {
        answer(&state, reply);
    }
}

pub(crate) fn start(heard: Sender<Heard>) -> Result<Take, String> {
    let state = installed().ok_or_else(|| NONE_HERE.to_string())?;
    let mut s = state.borrow_mut();
    if let Some(why) = s.start_error.clone() {
        return Err(why);
    }
    s.readied = s.readied.saturating_sub(1);
    s.takes += 1;
    s.alive = true;
    s.finished = false;
    s.heard = Some(heard);
    drop(s);
    Ok(Take(state))
}

pub(crate) fn open_system_dictation() -> bool {
    if let Some(state) = installed() {
        state.borrow_mut().system_opened += 1;
    }
    false
}

pub(crate) fn abandon() {
    if let Some(state) = installed() {
        let mut s = state.borrow_mut();
        s.abandoned += 1;
        s.readied = s.readied.saturating_sub(1);
    }
}

pub(crate) fn mic_access() -> Access {
    if installed().is_some() {
        Access::Granted
    } else {
        Access::Unavailable
    }
}

pub(crate) fn speech_access() -> Access {
    mic_access()
}

pub(crate) fn ask(done: impl Fn(bool)) {
    done(installed().is_some())
}

/// A fake take: alive until it drops, as a real one holds the mic.
pub struct Take(Rc<RefCell<State>>);

impl Take {
    pub fn live(&self) -> bool {
        true
    }

    pub fn level(&self) -> f32 {
        let s = self.0.borrow();
        if s.finished { 0. } else { s.level }
    }

    pub fn finish(&mut self) {
        self.0.borrow_mut().finished = true;
    }
}

impl Drop for Take {
    fn drop(&mut self) {
        let mut s = self.0.borrow_mut();
        s.alive = false;
        s.heard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_fake_there_is_no_dictation() {
        assert!(!crate::available());
        assert_eq!(crate::fallback(), None);
        let (reply, answer) = async_channel::bounded(1);
        crate::ensure_permission(reply);
        assert_eq!(answer.try_recv().unwrap(), Err(NONE_HERE.to_string()));
        let (heard, _) = async_channel::unbounded();
        assert!(crate::start(heard).is_err());
        assert_eq!(crate::mic_access(), Access::Unavailable);
    }

    #[test]
    fn a_take_hears_what_the_test_says_and_lets_go_when_dropped() {
        let fake = install();
        assert!(crate::available());
        let (reply, answer) = async_channel::bounded(1);
        crate::ensure_permission(reply);
        assert_eq!(answer.try_recv().unwrap(), Ok(()));
        assert!(fake.mic_open(), "allowed: the mic is open for the take");
        let (heard, results) = async_channel::unbounded();
        let mut take = crate::start(heard).unwrap();
        assert!(take.live() && fake.listening());
        fake.set_level(0.5);
        assert_eq!(take.level(), 0.5);
        fake.hear(Heard::Partial("hello".into()));
        assert_eq!(results.try_recv().unwrap(), Heard::Partial("hello".into()));
        take.finish();
        assert!(fake.finished() && !fake.listening());
        assert_eq!(take.level(), 0., "the mic is off");
        drop(take);
        assert!(!fake.mic_open());
        assert_eq!((fake.asked(), fake.takes()), (1, 1));
    }

    #[test]
    fn held_asks_wait_and_abandoning_one_lets_go_of_its_mic() {
        let fake = install();
        fake.hold();
        let (reply, answer) = async_channel::bounded(1);
        crate::ensure_permission(reply);
        assert!(answer.try_recv().is_err(), "still asking");
        fake.answer();
        assert_eq!(answer.try_recv().unwrap(), Ok(()));
        assert!(fake.mic_open());
        crate::abandon();
        assert!(!fake.mic_open());
        assert_eq!(fake.abandoned(), 1);
    }

    #[test]
    fn a_refusal_or_a_failed_start_says_why() {
        let fake = install();
        fake.refuse("No.");
        let (reply, answer) = async_channel::bounded(1);
        crate::ensure_permission(reply);
        assert_eq!(answer.try_recv().unwrap(), Err("No.".into()));
        assert!(!fake.mic_open());
        fake.fail_start("Busy.");
        let (heard, _) = async_channel::unbounded();
        assert_eq!(crate::start(heard).err(), Some("Busy.".into()));
        fake.unavailable("Not here.");
        assert!(!crate::available());
        // Dropping the fake puts things back as they were: no dictation.
        drop(fake);
        assert!(!crate::available());
        assert_eq!(crate::fallback(), None);
    }
}
