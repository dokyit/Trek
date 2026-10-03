//! Usage limits: which window an agent's limit is, and a thread paused by one until it resets
//! (with what to send then). The pause is kept on the thread, so a scheduled resume survives
//! quitting Trek.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which of an agent's limits was reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "name", rename_all = "kebab-case")]
pub enum LimitScope {
    /// A rolling session window (Claude's and Codex's 5-hour limit).
    Session,
    Weekly,
    /// A limit on one model or model family ("Opus", "Fable", "gpt-reserve").
    Model(String),
    /// A provider's rate limit or quota with no named window.
    #[default]
    Other,
}

impl LimitScope {
    /// "5-hour limit", "Weekly limit", "Opus limit", "Usage limit".
    pub fn label(&self) -> String {
        match self {
            LimitScope::Session => "5-hour limit".into(),
            LimitScope::Weekly => "Weekly limit".into(),
            LimitScope::Model(name) => format!("{name} limit"),
            LimitScope::Other => "Usage limit".into(),
        }
    }
}

/// How long after a reset Trek waits before it resumes: the provider's clock and its own may
/// differ by a little, and a message sent a second early hits the limit again.
pub const RESUME_GRACE_MS: i64 = 60_000;

/// How long to wait before trying again when the agent says its limit resets at a time already
/// gone (its clock and the Mac's disagree, or the reset is running late): soon, but no loop.
pub const LATE_RESET_RETRY_MS: i64 = 5 * 60_000;

/// The longest wait between tries at a reset that keeps being late.
pub const MAX_LATE_RETRY_MS: i64 = 60 * 60_000;

/// How long to wait before trying again at a reset already gone, after `tries` resumes in a row
/// met the limit again: twice as long each time, up to an hour.
pub fn late_retry(tries: u32) -> i64 {
    (LATE_RESET_RETRY_MS << tries.min(8)).min(MAX_LATE_RETRY_MS)
}

/// When to count on a limit having reset, as of `now`: as reported, unless that's already past
/// (then in a while; longer after `tries` resumes that met the limit again).
pub fn reset_ahead(resets_at: Option<i64>, now: i64, tries: u32) -> Option<i64> {
    resets_at.map(|at| if at <= now { now + late_retry(tries) } else { at })
}

/// What a resume sends when the user queued nothing of their own.
pub const CONTINUE: &str = "Continue where you left off.";

/// A message waiting for a limit to reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PathBuf>,
}

/// A thread its agent's usage limit stopped, until the limit resets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pause {
    /// What the agent said, as it said it.
    pub message: String,
    /// When the limit resets (unix ms), when that's known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub scope: LimitScope,
    /// When the limit was hit.
    pub since: i64,
    /// Send `queued` (or `CONTINUE`) once the limit has reset.
    #[serde(default)]
    pub resume: bool,
    /// Messages the user sent while the thread was paused, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<Queued>,
    /// How many resumes in a row met the limit again: the first resume is news, the later ones
    /// aren't, and a reset that keeps being late is tried less often (`reset_ahead`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tries: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Pause {
    pub fn new(message: String, resets_at: Option<i64>, scope: LimitScope, since: i64, resume: bool) -> Pause {
        Pause { message, resets_at, scope, since, resume, queued: vec![], tries: 0 }
    }

    /// When the resume goes out: shortly after the reset. `None` without a reset time or a resume.
    pub fn resume_at(&self) -> Option<i64> {
        self.resets_at.filter(|_| self.resume).map(|r| r + RESUME_GRACE_MS)
    }

    /// When the pause is over: at its resume, or (nothing scheduled) once the limit has reset.
    pub fn ends_at(&self) -> Option<i64> {
        self.resume_at().or(self.resets_at)
    }

    /// The limit has reset by `now` (a pause with no reset time lasts until the user acts).
    pub fn is_over(&self, now: i64) -> bool {
        self.ends_at().is_some_and(|at| at <= now)
    }

    /// What goes out at the resume, in order: what the user queued, or a plain "continue".
    pub fn messages(&self) -> Vec<Queued> {
        if self.queued.is_empty() { vec![Queued { text: CONTINUE.into(), images: vec![] }] } else { self.queued.clone() }
    }

    /// The same limit hit again: the newer report wins, but the user's choices and messages stay.
    pub fn renewed(self, next: Pause) -> Pause {
        Pause { resume: self.resume || next.resume, queued: self.queued, ..next }
    }
}

/// The earliest moment any of `pauses` ends (see `Pause::ends_at`).
pub fn next_end<'a>(pauses: impl IntoIterator<Item = &'a Pause>) -> Option<i64> {
    pauses.into_iter().filter_map(Pause::ends_at).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pause(resets_at: Option<i64>, resume: bool) -> Pause {
        Pause::new("You've hit your session limit · resets 7:40pm".into(), resets_at, LimitScope::Session, 0, resume)
    }

    #[test]
    fn a_resume_goes_a_minute_after_the_reset() {
        let p = pause(Some(10_000_000), true);
        assert_eq!(p.resume_at(), Some(10_060_000));
        assert!(!p.is_over(10_059_999) && p.is_over(10_060_000));
        // Nothing scheduled: the pause is over at the reset itself.
        let ask = pause(Some(10_000_000), false);
        assert_eq!((ask.resume_at(), ask.ends_at()), (None, Some(10_000_000)));
        // No reset time: only the user ends it.
        let unknown = pause(None, true);
        assert_eq!(unknown.ends_at(), None);
        assert!(!unknown.is_over(i64::MAX));
        assert_eq!(next_end([&p, &ask, &unknown]), Some(10_000_000));
        assert_eq!(next_end([&unknown]), None);
    }

    #[test]
    fn a_reset_already_past_is_tried_again_in_a_while() {
        assert_eq!(reset_ahead(Some(10_000), 5_000, 3), Some(10_000));
        assert_eq!(reset_ahead(Some(5_000), 5_000, 0), Some(5_000 + LATE_RESET_RETRY_MS));
        assert_eq!(reset_ahead(None, 5_000, 2), None);
        // Late again and again: twice as long each time, up to an hour.
        let waits: Vec<i64> = (0..6).map(|tries| reset_ahead(Some(0), 0, tries).unwrap() / 60_000).collect();
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
        assert_eq!(late_retry(u32::MAX), MAX_LATE_RETRY_MS);
    }

    #[test]
    fn a_resume_sends_what_was_queued_in_order_or_continue() {
        let mut p = pause(Some(1), true);
        assert_eq!(p.messages(), vec![Queued { text: CONTINUE.into(), images: vec![] }]);
        p.queued.push(Queued { text: "first".into(), images: vec![] });
        p.queued.push(Queued { text: "second".into(), images: vec!["/tmp/a.png".into()] });
        let texts: Vec<String> = p.messages().into_iter().map(|q| q.text).collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn hitting_the_limit_again_keeps_the_users_choices() {
        let mut p = pause(Some(1_000), true);
        p.queued.push(Queued { text: "go on".into(), images: vec![] });
        let again = p.clone().renewed(Pause::new("still limited".into(), Some(9_000), LimitScope::Weekly, 5, false));
        assert_eq!((again.resets_at, again.resume, again.scope.clone(), again.message.as_str()), (Some(9_000), true, LimitScope::Weekly, "still limited"));
        assert_eq!(again.queued, p.queued);
    }

    #[test]
    fn pauses_round_trip_and_read_older_shapes() {
        let mut p = pause(Some(42), true);
        p.scope = LimitScope::Model("Opus".into());
        p.queued.push(Queued { text: "hi".into(), images: vec![] });
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Pause>(&json).unwrap(), p);
        let minimal: Pause = serde_json::from_str(r#"{"message":"limit","since":3}"#).unwrap();
        assert_eq!((minimal.resets_at, minimal.scope, minimal.resume, minimal.queued.len()), (None, LimitScope::Other, false, 0));
        assert_eq!(LimitScope::Model("Opus".into()).label(), "Opus limit");
    }
}
