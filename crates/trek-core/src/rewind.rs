//! Taking a conversation back (rewind, edit and resend, retry) or branching it (fork): what part
//! of a transcript a turn covers, how the agent's next session gets back to the right point, and
//! the recap a session gets when its agent can't.

use crate::store::{INTERRUPTED_BY_QUIT, Item, ResumePoint};
use serde::{Deserialize, Serialize};

/// How a thread's next session picks up a conversation that was cut back or copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reopen {
    /// Resume the agent's own `session`, cut back to just after `at` (`ResumePoint::after`), or
    /// whole when `at` is absent. `fork`: in a copy, leaving `session` as it is (another thread
    /// may be using it).
    Native {
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<String>,
        #[serde(default)]
        fork: bool,
    },
    /// A new session, primed with a recap of the conversation kept.
    Recap,
}

impl Reopen {
    /// Where a message sent before this session starts goes in it.
    pub fn point(&self) -> Option<ResumePoint> {
        match self {
            Reopen::Native { session, at, .. } => Some(ResumePoint { session: session.clone(), after: at.clone() }),
            Reopen::Recap => None,
        }
    }
}

/// How the next session should pick up the conversation that's left when everything from a
/// message on goes. `kept`: the items before that message; `point`: where the agent's session
/// stood when it was sent; `current`: the session the thread uses now; `native`: the agent can
/// resume part of a session. `None`: nothing worth keeping, so a plain new session.
pub fn reopen_before(kept: &[Item], point: Option<&ResumePoint>, current: Option<&str>, native: bool) -> Option<Reopen> {
    if !kept.iter().any(|i| matches!(i, Item::User { .. })) {
        return None;
    }
    match point {
        Some(ResumePoint { session, after: Some(at) }) if native => {
            // Cut back in place only in the session the thread is on: the point may lie in one
            // this thread was forked from, which belongs to another thread.
            Some(Reopen::Native { session: session.clone(), at: Some(at.clone()), fork: current != Some(session.as_str()) })
        }
        _ => Some(Reopen::Recap),
    }
}

/// Where the turn ending at `end` (its `TurnEnd`) began: the first message after the previous
/// turn's end, so a message that steered the turn is part of it.
pub fn turn_start(items: &[Item], end: usize) -> Option<usize> {
    let end = end.min(items.len());
    let boundary = items[..end].iter().rposition(|i| match i {
        Item::TurnEnd { .. } | Item::Error { .. } => true,
        Item::Notice { text } => text == "Interrupted" || text == INTERRUPTED_BY_QUIT,
        _ => false,
    });
    let from = boundary.map_or(0, |b| b + 1);
    items[from..end].iter().position(|i| matches!(i, Item::User { .. })).map(|p| from + p)
}

/// Longest a single message runs in a recap.
const RECAP_MESSAGE: usize = 2_000;
/// Longest a recap runs; the oldest messages are left out past it.
const RECAP_MAX: usize = 24_000;

/// What was said so far, compactly, for a session that starts afresh in a conversation already
/// under way: the user's messages and the agent's answers (long ones shortened), and one line per
/// thing the agent did. Notices and errors are left out.
pub fn recap(items: &[Item]) -> String {
    let mut entries: Vec<String> = vec![];
    for item in items {
        match item {
            Item::User { text, images, .. } => {
                let mut e = format!("User: {}", clip(text.trim(), RECAP_MESSAGE));
                if !images.is_empty() {
                    e.push_str(&format!(" [{} image{} attached]", images.len(), if images.len() == 1 { "" } else { "s" }));
                }
                entries.push(e);
            }
            Item::Assistant { text } if !text.trim().is_empty() => entries.push(format!("Assistant: {}", clip(text.trim(), RECAP_MESSAGE))),
            Item::Tool { title, detail, .. } => {
                let line = format!("(The assistant used a tool: {title} {})", clip(detail.lines().next().unwrap_or_default(), 160));
                // Runs of tool calls stay one line each, without blank lines between them.
                match entries.last_mut() {
                    Some(last) if last.starts_with("(The assistant used a tool") => {
                        last.push('\n');
                        last.push_str(&line);
                    }
                    _ => entries.push(line),
                }
            }
            _ => {}
        }
    }
    let mut total = 0;
    let keep = entries.iter().rev().take_while(|e| {
        total += e.len() + 2;
        total <= RECAP_MAX
    });
    let kept = keep.count().max(1).min(entries.len());
    let mut out = entries[entries.len() - kept..].join("\n\n");
    if kept < entries.len() {
        out = format!("(Earlier messages left out.)\n\n{out}");
    }
    out
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ToolStatus;

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }
    fn end() -> Item {
        Item::TurnEnd { at: 1, took_secs: 1 }
    }
    fn point(session: &str, after: Option<&str>) -> ResumePoint {
        ResumePoint { session: session.into(), after: after.map(String::from) }
    }

    #[test]
    fn turns_start_at_their_first_message() {
        let items = [user("a"), said("1"), end(), user("b"), said("2"), user("steer"), said("3"), end()];
        assert_eq!(turn_start(&items, 2), Some(0));
        assert_eq!(turn_start(&items, 7), Some(3), "a steering message is part of the turn");
        // An interrupted or failed turn ends there too.
        let items = [user("a"), Item::Notice { text: "Interrupted".into() }, user("b"), said("2"), end()];
        assert_eq!(turn_start(&items, 4), Some(2));
        let items = [user("a"), Item::Error { text: "boom".into() }, user("b"), said("2"), end()];
        assert_eq!(turn_start(&items, 4), Some(2));
        // An agent's own notice mid-turn doesn't end it.
        let items = [user("a"), Item::Notice { text: "Codex couldn't reopen".into() }, said("2"), end()];
        assert_eq!(turn_start(&items, 3), Some(0));
        assert_eq!(turn_start(&[said("woke up"), end()], 1), None);
    }

    #[test]
    fn sessions_resume_natively_where_they_can() {
        let kept = [user("a"), said("1"), end()];
        let at = point("s1", Some("m7"));
        assert_eq!(reopen_before(&kept, Some(&at), Some("s1"), true), Some(Reopen::Native { session: "s1".into(), at: Some("m7".into()), fork: false }));
        // The point is in another session (this thread is a fork of it): a copy, never in place.
        assert_eq!(reopen_before(&kept, Some(&at), Some("s2"), true), Some(Reopen::Native { session: "s1".into(), at: Some("m7".into()), fork: true }));
        assert_eq!(reopen_before(&kept, Some(&at), None, true), Some(Reopen::Native { session: "s1".into(), at: Some("m7".into()), fork: true }));
        // Agents that can't, messages without a point, and messages that started their session.
        assert_eq!(reopen_before(&kept, Some(&at), Some("s1"), false), Some(Reopen::Recap));
        assert_eq!(reopen_before(&kept, None, Some("s1"), true), Some(Reopen::Recap));
        assert_eq!(reopen_before(&kept, Some(&point("s1", None)), Some("s1"), true), Some(Reopen::Recap));
        // Nothing said before it: just a new session.
        assert_eq!(reopen_before(&[], Some(&at), Some("s1"), true), None);
        assert_eq!(reopen_before(&[Item::Notice { text: "hi".into() }], None, None, false), None);
        assert_eq!(Reopen::Native { session: "s".into(), at: Some("m".into()), fork: true }.point(), Some(point("s", Some("m"))));
    }

    #[test]
    fn reopen_round_trips_as_json() {
        for r in [Reopen::Recap, Reopen::Native { session: "s".into(), at: None, fork: true }, Reopen::Native { session: "s".into(), at: Some("m".into()), fork: false }] {
            assert_eq!(serde_json::from_str::<Reopen>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
        }
    }

    #[test]
    fn recaps_keep_the_conversation_and_drop_the_noise() {
        let tool = |d: &str| Item::Tool { id: "t".into(), title: "Edit".into(), detail: d.into(), output: "lots of output".into(), status: ToolStatus::Done };
        let items = [
            Item::User { text: "fix the parser".into(), images: vec!["/tmp/s.png".into()], at: None, resume: None },
            Item::Reasoning { text: "hmm".into() },
            tool("src/parser.rs"),
            tool("src/lib.rs"),
            said("Fixed it."),
            end(),
            Item::Notice { text: "Interrupted".into() },
            Item::Error { text: "boom".into() },
            user("thanks"),
        ];
        assert_eq!(
            recap(&items),
            "User: fix the parser [1 image attached]\n\n(The assistant used a tool: Edit src/parser.rs)\n(The assistant used a tool: Edit src/lib.rs)\n\nAssistant: Fixed it.\n\nUser: thanks"
        );
    }

    #[test]
    fn long_recaps_keep_the_latest_messages() {
        let long = "x".repeat(5_000);
        let items: Vec<Item> = (0..30).map(|i| if i % 2 == 0 { user(&format!("{i} {long}")) } else { said(&format!("{i} {long}")) }).collect();
        let r = recap(&items);
        assert!(r.len() <= RECAP_MAX + 100, "{}", r.len());
        assert!(r.starts_with("(Earlier messages left out.)"));
        assert!(r.ends_with("29 ") || r.contains("Assistant: 29 "), "the latest message is in");
        assert!(!r.contains("User: 0 "));
        assert!(r.contains(" …"), "each message is shortened");
    }
}
