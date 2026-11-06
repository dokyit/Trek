//! Turns a quit or a crash cut off, and what Trek may do about them. Nothing here runs anything:
//! the workspace asks, and only runs a thread when the user says so or has turned on resuming at
//! launch. No tool call is ever sent again: the agent gets a message, and a note that what was
//! running may or may not have finished, and decides from what it finds.

use crate::store::{Section, Thread};

/// What a resumed thread is sent, unless the user edits it.
pub const RESUME_MESSAGE: &str = "Resume working";

/// How many times in a row a thread is resumed at launch without the user asking. A thread that
/// takes Trek down with it would otherwise bring it down again at every launch; past this it
/// waits to be asked, like any other.
pub const MAX_AUTO_RESUMES: u32 = 2;

/// A turn the last run left open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted {
    pub thread: String,
    /// It was waiting on the user (an approval, a question, a plan), not working: it comes back
    /// waiting, and only the user's say-so runs it.
    pub waiting: bool,
    /// What it was asking, when the transcript shows that (the tool call the card was about).
    pub asked: Option<String>,
    /// When Trek found it cut off (unix ms).
    pub at: i64,
    /// How many times it has been resumed since it was cut off.
    pub tries: u32,
}

impl Interrupted {
    /// Whether turning on "resume at launch" covers it.
    pub fn may_auto_resume(&self) -> bool {
        !self.waiting && self.tries < MAX_AUTO_RESUMES
    }
}

/// Whether the launch offer lists `t`: the threads the user sees in Agents mode, and not ones they
/// put away. Sub-agents report to their parents; side chats and editor chats have no place in the
/// sidebar, and their own composers offer them. A thread archived, settled or snoozed was closed
/// on purpose.
pub fn listed(t: &Thread, now: i64) -> bool {
    matches!(t.section(now), Some(Section::Inbox | Section::Pinned))
}

/// What the sidebar's notice says, given how many listed threads were cut off working and how
/// many were waiting on the user. `None` when there are none.
pub fn headline(working: usize, waiting: usize) -> Option<String> {
    match (working, waiting) {
        (0, 0) => None,
        (0, 1) => Some("1 thread is waiting for you".into()),
        (0, n) => Some(format!("{n} threads are waiting for you")),
        (1, _) => Some("1 thread was interrupted".into()),
        (n, _) => Some(format!("{n} threads were interrupted")),
    }
}

/// The notice's second line, when chats were cut off both ways: Resume all leaves these.
pub fn waiting_too(working: usize, waiting: usize) -> Option<String> {
    match (working, waiting) {
        (0, _) | (_, 0) => None,
        (_, 1) => Some("1 is waiting for you".into()),
        (_, n) => Some(format!("{n} are waiting for you")),
    }
}

/// What the agent is told, under the message the user sees: the transcript keeps the message as
/// typed. Tool calls are never replayed: Trek can't know how far one got.
fn note(waiting: bool) -> &'static str {
    if waiting {
        "(Trek closed while this turn waited for the user's answer, and that request has gone. Nothing was answered. Ask again if you still need it.)"
    } else {
        "(Trek closed before your last turn finished. Tool calls still running then may or may not have taken effect, and none were repeated: check how things stand before running any of them again.)"
    }
}

/// The prompt for the first message after a cut-off turn.
pub fn prompt(text: &str, waiting: bool) -> String {
    format!("{text}\n\n{}", note(waiting))
}

/// What a card that was up says about what it asked, from the transcript: the tool call it was
/// about, on one line.
pub fn asked_line(title: &str, detail: &str) -> String {
    let detail = detail.lines().next().unwrap_or_default().trim();
    let line = if detail.is_empty() {
        title.trim().to_string()
    } else {
        format!("{} {detail}", title.trim())
    };
    const MOST: usize = 120;
    match line.char_indices().nth(MOST) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(waiting: bool, tries: u32) -> Interrupted {
        Interrupted {
            thread: "t".into(),
            waiting,
            asked: None,
            at: 1,
            tries,
        }
    }

    #[test]
    fn only_work_cut_off_resumes_by_itself_and_not_for_ever() {
        assert!(turn(false, 0).may_auto_resume());
        assert!(turn(false, MAX_AUTO_RESUMES - 1).may_auto_resume());
        assert!(
            !turn(false, MAX_AUTO_RESUMES).may_auto_resume(),
            "a thread that keeps going down with Trek waits to be asked"
        );
        assert!(
            !turn(true, 0).may_auto_resume(),
            "a thread waiting on an approval stays waiting"
        );
    }

    #[test]
    fn the_notice_counts_what_was_cut_off_and_what_waits() {
        assert_eq!(headline(0, 0), None);
        assert_eq!(headline(1, 0).unwrap(), "1 thread was interrupted");
        assert_eq!(headline(3, 0).unwrap(), "3 threads were interrupted");
        assert_eq!(headline(0, 1).unwrap(), "1 thread is waiting for you");
        assert_eq!(headline(0, 2).unwrap(), "2 threads are waiting for you");
        assert_eq!(headline(2, 1).unwrap(), "2 threads were interrupted");
        assert_eq!(waiting_too(2, 1).unwrap(), "1 is waiting for you");
        assert_eq!(waiting_too(1, 3).unwrap(), "3 are waiting for you");
        assert_eq!(waiting_too(2, 0), None);
        assert_eq!(waiting_too(0, 2), None, "the headline says it");
    }

    #[test]
    fn the_agent_is_told_not_to_trust_what_was_running() {
        let working = prompt("Resume working", false);
        assert!(working.starts_with("Resume working\n\n"), "{working}");
        assert!(
            working.contains("may or may not have taken effect"),
            "{working}"
        );
        let waiting = prompt("Go on", true);
        assert!(waiting.starts_with("Go on\n\n"), "{waiting}");
        assert!(waiting.contains("Nothing was answered"), "{waiting}");
    }

    #[test]
    fn what_a_card_asked_stays_on_one_short_line() {
        assert_eq!(
            asked_line("Bash", "rm -rf build\nand more"),
            "Bash rm -rf build"
        );
        assert_eq!(asked_line("Plan", ""), "Plan");
        let long = asked_line("Bash", &"x".repeat(400));
        assert!(long.chars().count() <= 121 && long.ends_with('…'), "{long}");
    }
}
