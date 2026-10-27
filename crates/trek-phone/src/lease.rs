//! One agent drives the phone at a time. The first thread to use it holds it until it lets go
//! (`phone_release`), its turn ends, or it sits idle past `IDLE`; another thread meanwhile is
//! told who has it. The user can take the phone back (Stop): the turn that held it gets no more,
//! and its next turn may claim it again.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a holder may go without using the phone before another thread may take it.
pub const IDLE: Duration = Duration::from_secs(10 * 60);

/// What a call gets once the user has taken the phone back.
pub const TOOK_OVER: &str = "The user took over the iPhone: stop using it and wait for them. (They can hand it back, or carry on with you in their next message.)";

#[derive(Debug, Clone, PartialEq)]
pub struct Holder {
    pub thread: String,
    /// The thread's title, for the strip and for other threads.
    pub title: String,
    /// The turn it's in (a new turn after Stop may claim again).
    pub turn: Option<Instant>,
    pub since: Instant,
    pub last: Instant,
}

#[derive(Debug, Default)]
pub struct Lease {
    holder: Option<Holder>,
    /// Threads the user took the phone back from, with the turn each was in then.
    taken: HashMap<String, Option<Instant>>,
}

impl Lease {
    /// `thread` (in `turn`) wants the phone now: Ok when it holds it (now or already), else why
    /// not.
    pub fn claim(
        &mut self,
        thread: &str,
        title: &str,
        turn: Option<Instant>,
        now: Instant,
    ) -> Result<(), String> {
        match self.taken.get(thread) {
            Some(t) if *t == turn => return Err(TOOK_OVER.into()),
            // A new turn: the user carried on with the agent, which may use it again.
            Some(_) => {
                self.taken.remove(thread);
            }
            None => {}
        }
        if let Some(h) = &mut self.holder {
            let free = now.saturating_duration_since(h.last) > IDLE;
            if h.thread == thread {
                h.last = now;
                h.turn = turn;
                return Ok(());
            }
            if !free {
                let ago = now.saturating_duration_since(h.last).as_secs();
                return Err(format!(
                    "The iPhone is in use by another agent (thread \"{}\", last action {ago} s ago). Only one agent drives it at a time: wait and try again later, or ask the user.",
                    h.title
                ));
            }
        }
        self.holder = Some(Holder {
            thread: thread.to_string(),
            title: title.to_string(),
            turn,
            since: now,
            last: now,
        });
        Ok(())
    }

    /// `thread` lets go (`phone_release`, or its turn ended): whether it held the phone.
    pub fn release(&mut self, thread: &str) -> bool {
        if self.holder.as_ref().is_some_and(|h| h.thread == thread) {
            self.holder = None;
            return true;
        }
        false
    }

    /// The user takes the phone back from whoever holds it (Stop).
    pub fn take_back(&mut self) -> Option<Holder> {
        let h = self.holder.take()?;
        self.taken.insert(h.thread.clone(), h.turn);
        Some(h)
    }

    /// The user hands the phone back to the agents they took it from.
    pub fn hand_back(&mut self) {
        self.taken.clear();
    }

    pub fn taken_over(&self) -> bool {
        !self.taken.is_empty()
    }

    /// Who holds the phone now (an idle holder's lease has run out).
    pub fn holder(&self, now: Instant) -> Option<&Holder> {
        self.holder
            .as_ref()
            .filter(|h| now.saturating_duration_since(h.last) <= IDLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_thread_at_a_time_until_it_lets_go() {
        let now = Instant::now();
        let mut lease = Lease::default();
        assert!(lease.claim("a", "Fix login", None, now).is_ok());
        assert!(lease.claim("a", "Fix login", None, now).is_ok(), "again");
        let err = lease.claim("b", "Other", None, now).unwrap_err();
        assert!(err.contains("Fix login"), "{err}");
        assert!(!lease.release("b"));
        assert!(lease.release("a"));
        assert!(lease.claim("b", "Other", None, now).is_ok());
        assert_eq!(lease.holder(now).unwrap().thread, "b");
    }

    #[test]
    fn an_idle_holder_loses_it() {
        let start = Instant::now();
        let mut lease = Lease::default();
        lease.claim("a", "A", None, start).unwrap();
        let later = start + IDLE + Duration::from_secs(1);
        assert!(lease.holder(later).is_none());
        assert!(lease.claim("b", "B", None, later).is_ok());
    }

    #[test]
    fn stop_takes_it_back_for_that_turn_only() {
        let now = Instant::now();
        let turn = Some(now);
        let mut lease = Lease::default();
        lease.claim("a", "A", turn, now).unwrap();
        assert_eq!(lease.take_back().unwrap().thread, "a");
        assert!(lease.taken_over());
        assert_eq!(lease.claim("a", "A", turn, now).unwrap_err(), TOOK_OVER);
        // Another thread may use it meanwhile; a's next turn may too.
        assert!(lease.claim("b", "B", None, now).is_ok());
        lease.release("b");
        let next = Some(now + Duration::from_secs(5));
        assert!(lease.claim("a", "A", next, now).is_ok());
        assert!(!lease.taken_over());
        // Hand back clears the rest.
        lease.take_back();
        lease.hand_back();
        assert!(lease.claim("a", "A", next, now).is_ok());
    }
}
