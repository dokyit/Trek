//! Taking a conversation back (rewind, edit and resend, retry) or branching it (fork): what part
//! of a transcript a turn covers, how the agent's next session gets back to the right point, and
//! the recap a session gets when its agent can't.

use crate::store::{INTERRUPTED_BY_QUIT, Item, ResumePoint};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
            Reopen::Native { session, at, .. } => Some(ResumePoint {
                session: session.clone(),
                after: at.clone(),
            }),
            Reopen::Recap => None,
        }
    }
}

/// How the next session should pick up the conversation that's left when everything from a
/// message on goes. `kept`: the items before that message; `point`: where the agent's session
/// stood when it was sent; `current`: the session the thread uses now; `native`: the agent can
/// resume part of a session. `None`: nothing worth keeping, so a plain new session.
pub fn reopen_before(
    kept: &[Item],
    point: Option<&ResumePoint>,
    current: Option<&str>,
    native: bool,
) -> Option<Reopen> {
    if !kept.iter().any(|i| matches!(i, Item::User { .. })) {
        return None;
    }
    match point {
        Some(ResumePoint {
            session,
            after: Some(at),
        }) if native => {
            // Cut back in place only in the session the thread is on: the point may lie in one
            // this thread was forked from, which belongs to another thread.
            Some(Reopen::Native {
                session: session.clone(),
                at: Some(at.clone()),
                fork: current != Some(session.as_str()),
            })
        }
        _ => Some(Reopen::Recap),
    }
}

/// Where the turn ending at `end` (its `TurnEnd`, error or interruption) began: the first message
/// after the previous turn's end, so a message that steered the turn is part of it. Asides (a
/// command Trek answered, an answer to the agent's question) don't start turns.
pub fn turn_start(items: &[Item], end: usize) -> Option<usize> {
    let end = end.min(items.len());
    let boundary = items[..end].iter().rposition(ends_turn);
    let from = boundary.map_or(0, |b| b + 1);
    items[from..end]
        .iter()
        .position(|i| matches!(i, Item::User { aside: false, .. }))
        .map(|p| from + p)
}

/// Whether `item` ends a turn: its footer, a failure, a usage limit, or an interruption.
pub fn ends_turn(item: &Item) -> bool {
    match item {
        Item::TurnEnd { .. } | Item::Error { .. } | Item::Limit { .. } => true,
        Item::Notice { text } => text == "Interrupted" || text == INTERRUPTED_BY_QUIT,
        _ => false,
    }
}

/// Longest the first message runs in a recap.
const RECAP_FIRST: usize = 1_500;
/// Longest any other message runs in a recap.
const RECAP_MESSAGE: usize = 800;
/// How many of the latest exchanges a recap shows.
const RECAP_TURNS: usize = 3;
/// Longest a recap runs: past it, fewer of the latest exchanges are shown (the oldest go first).
const RECAP_MAX: usize = 9_000;

/// A short orientation for a session that starts afresh in a conversation already under way:
/// where the whole transcript is and how to search it (`log`: its path and line count), what was
/// asked first, the last few exchanges (long messages shortened, tool calls one line per turn),
/// and where things stand (how the last turn ended, a question left open, the files changed).
/// Everything else stays in the transcript, word for word, for the agent to search.
pub fn recap(items: &[Item], log: Option<(&Path, u64)>) -> String {
    // Turns, by the message that starts each.
    let starts: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i, Item::User { aside: false, .. }))
        .map(|(i, _)| i)
        .collect();
    if starts.is_empty() {
        return String::new();
    }
    let turn = |t: usize| {
        let end = starts.get(t + 1).copied().unwrap_or(items.len());
        &items[starts[t]..end]
    };
    let retrieval = log.map(|(path, lines)| crate::session_log::retrieval(path, lines, None));
    let first = user_line(&items[starts[0]], RECAP_FIRST);
    let tail = standing(items);
    // The latest exchanges, as many of `RECAP_TURNS` as fit (the first message is shown apart
    // when it isn't one of them).
    let mut from = starts.len().saturating_sub(RECAP_TURNS);
    let mut turns: Vec<String> = (from..starts.len()).map(|t| exchange(turn(t))).collect();
    let fixed = retrieval
        .iter()
        .chain(&tail)
        .map(|p| p.len() + 2)
        .sum::<usize>()
        + first.len()
        + 200;
    while turns.len() > 1 && fixed + turns.iter().map(|p| p.len() + 2).sum::<usize>() > RECAP_MAX {
        turns.remove(0);
        from += 1;
    }
    let mut out: Vec<String> = retrieval.into_iter().collect();
    if from > 0 {
        out.push("What was asked first:".to_string());
        out.push(first);
        let left_out = &items[starts[1].min(starts[from])..starts[from]];
        let messages = from - 1;
        let tools = left_out
            .iter()
            .filter(|i| matches!(i, Item::Tool { .. }))
            .count();
        if messages > 0 {
            out.push(format!(
                "({messages} later message{} and {tools} tool call{} before the latest exchanges are left out here{}.)",
                if messages == 1 { "" } else { "s" },
                if tools == 1 { "" } else { "s" },
                if log.is_some() {
                    ": they're in the transcript"
                } else {
                    ""
                }
            ));
        }
    }
    if starts.len() > 1 {
        out.push("The latest exchanges, oldest first:".to_string());
    }
    out.into_iter()
        .chain(turns)
        .chain(tail)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// "User: …", with how many images came with it.
fn user_line(item: &Item, max: usize) -> String {
    let Item::User { text, images, .. } = item else {
        return String::new();
    };
    let mut e = format!("User: {}", clip(text.trim(), max));
    if !images.is_empty() {
        e.push_str(&format!(
            " [{} image{} attached]",
            images.len(),
            if images.len() == 1 { "" } else { "s" }
        ));
    }
    e
}

/// One turn for a recap: its message (and any answers the user typed in it), one line for the
/// tools the agent used, and the agent's last words.
fn exchange(turn: &[Item]) -> String {
    let mut parts = vec![user_line(&turn[0], RECAP_MESSAGE)];
    let mut tools: Vec<String> = vec![];
    let mut calls = 0;
    for item in &turn[1..] {
        match item {
            Item::User { .. } => parts.push(user_line(item, RECAP_MESSAGE / 4)),
            Item::Tool { title, detail, .. } => {
                calls += 1;
                let line = format!(
                    "{title} {}",
                    clip(detail.lines().next().unwrap_or_default().trim(), 100)
                );
                let line = line.trim().to_string();
                if !tools.contains(&line) {
                    tools.push(line);
                }
            }
            _ => {}
        }
    }
    if calls > 0 {
        const LISTED: usize = 6;
        let more = tools.len().saturating_sub(LISTED);
        tools.truncate(LISTED);
        let mut line = format!(
            "(The assistant used {calls} tool call{}: {}",
            if calls == 1 { "" } else { "s" },
            tools.join("; ")
        );
        if more > 0 {
            line.push_str(&format!("; and {more} more"));
        }
        line.push(')');
        parts.push(line);
    }
    let last_said = turn.iter().rev().find_map(|i| match i {
        Item::Assistant { text } if !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    });
    if let Some(said) = last_said {
        parts.push(format!("Assistant: {}", clip(said, RECAP_MESSAGE)));
    }
    parts.join("\n\n")
}

/// Where the conversation stands at its end: how the last turn ended when it didn't finish, a
/// question the agent asked that's still open, an agent change, the files changed.
fn standing(items: &[Item]) -> Vec<String> {
    let mut notes = vec![];
    match items.iter().rev().find(|i| ends_turn(i)) {
        Some(Item::Error { text }) => {
            notes.push(format!("The last turn failed: {}", clip(text.trim(), 300)))
        }
        Some(Item::Limit { .. }) => notes.push("The last turn stopped at a usage limit.".into()),
        Some(Item::Notice { .. }) => {
            notes.push("The last turn was interrupted before it finished.".into())
        }
        _ => {}
    }
    let last_said = items
        .iter()
        .rposition(|i| matches!(i, Item::Assistant { text } if !text.trim().is_empty()));
    let last_asked = items.iter().rposition(|i| matches!(i, Item::User { .. }));
    if let Some(said) = last_said
        && last_asked.is_none_or(|a| a < said)
        && let Item::Assistant { text } = &items[said]
        && text.trim_end().ends_with('?')
    {
        notes.push(
            "Open: the assistant's last message asked the user something, still unanswered.".into(),
        );
    }
    if let Some(Item::Handoff {
        from,
        from_name,
        to,
        to_name,
        ..
    }) = items
        .iter()
        .rev()
        .find(|i| matches!(i, Item::Handoff { .. }))
    {
        notes.push(format!(
            "The conversation moved from {} to {} along the way.",
            from_name.as_deref().unwrap_or(from),
            to_name.as_deref().unwrap_or(to)
        ));
    }
    let mut files: Vec<&str> = vec![];
    for item in items {
        if let Item::Tool { title, detail, .. } = item {
            let t = title.to_ascii_lowercase();
            let path = detail.lines().next().unwrap_or_default().trim();
            let edits = ["edit", "write", "patch"].iter().any(|w| t.contains(w));
            if edits
                && !path.is_empty()
                && !path.contains(' ')
                && path.len() < 200
                && !files.contains(&path)
            {
                files.push(path);
            }
        }
    }
    if !files.is_empty() {
        const LISTED: usize = 15;
        let more = files.len().saturating_sub(LISTED);
        let mut line = format!(
            "Files changed so far: {}",
            files[..files.len().min(LISTED)].join(", ")
        );
        if more > 0 {
            line.push_str(&format!(" and {more} more"));
        }
        notes.push(line);
    }
    notes
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
        Item::User {
            text: t.into(),
            images: vec![],
            at: None,
            resume: None,
            aside: false,
        }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }
    fn end() -> Item {
        Item::TurnEnd {
            at: 1,
            took_secs: 1,
        }
    }
    fn point(session: &str, after: Option<&str>) -> ResumePoint {
        ResumePoint {
            session: session.into(),
            after: after.map(String::from),
        }
    }

    #[test]
    fn turns_start_at_their_first_message() {
        let items = [
            user("a"),
            said("1"),
            end(),
            user("b"),
            said("2"),
            user("steer"),
            said("3"),
            end(),
        ];
        assert_eq!(turn_start(&items, 2), Some(0));
        assert_eq!(
            turn_start(&items, 7),
            Some(3),
            "a steering message is part of the turn"
        );
        // An interrupted or failed turn ends there too.
        let items = [
            user("a"),
            Item::Notice {
                text: "Interrupted".into(),
            },
            user("b"),
            said("2"),
            end(),
        ];
        assert_eq!(turn_start(&items, 4), Some(2));
        let items = [
            user("a"),
            Item::Error {
                text: "boom".into(),
            },
            user("b"),
            said("2"),
            end(),
        ];
        assert_eq!(turn_start(&items, 4), Some(2));
        // An agent's own notice mid-turn doesn't end it.
        let items = [
            user("a"),
            Item::Notice {
                text: "Codex couldn't reopen".into(),
            },
            said("2"),
            end(),
        ];
        assert_eq!(turn_start(&items, 3), Some(0));
        assert_eq!(turn_start(&[said("woke up"), end()], 1), None);
    }

    #[test]
    fn asides_dont_start_turns() {
        let aside = |t: &str| Item::User {
            text: t.into(),
            images: vec![],
            at: None,
            resume: None,
            aside: true,
        };
        // A command Trek answered itself between two turns.
        let items = [
            user("remember APPLE"),
            said("ok"),
            end(),
            aside("/model"),
            Item::Notice {
                text: "Model: haiku".into(),
            },
            user("remember PEAR"),
            said("ok"),
            end(),
        ];
        assert_eq!(turn_start(&items, 7), Some(5));
        // An answer to the agent's question mid-turn is part of the turn its message started.
        let items = [
            user("set it up"),
            said("Which port?"),
            aside("8080"),
            said("done"),
            end(),
        ];
        assert_eq!(turn_start(&items, 4), Some(0));
        // A turn only an answer started has no message to take back.
        let items = [user("a"), said("1"), end(), aside("yes"), said("2"), end()];
        assert_eq!(turn_start(&items, 5), None);
        assert!(
            ends_turn(&end())
                && ends_turn(&Item::Error { text: "x".into() })
                && ends_turn(&Item::Notice {
                    text: "Interrupted".into()
                })
        );
        assert!(!ends_turn(&Item::Notice {
            text: "Model: haiku".into()
        }));
    }

    #[test]
    fn sessions_resume_natively_where_they_can() {
        let kept = [user("a"), said("1"), end()];
        let at = point("s1", Some("m7"));
        assert_eq!(
            reopen_before(&kept, Some(&at), Some("s1"), true),
            Some(Reopen::Native {
                session: "s1".into(),
                at: Some("m7".into()),
                fork: false
            })
        );
        // The point is in another session (this thread is a fork of it): a copy, never in place.
        assert_eq!(
            reopen_before(&kept, Some(&at), Some("s2"), true),
            Some(Reopen::Native {
                session: "s1".into(),
                at: Some("m7".into()),
                fork: true
            })
        );
        assert_eq!(
            reopen_before(&kept, Some(&at), None, true),
            Some(Reopen::Native {
                session: "s1".into(),
                at: Some("m7".into()),
                fork: true
            })
        );
        // Agents that can't, messages without a point, and messages that started their session.
        assert_eq!(
            reopen_before(&kept, Some(&at), Some("s1"), false),
            Some(Reopen::Recap)
        );
        assert_eq!(
            reopen_before(&kept, None, Some("s1"), true),
            Some(Reopen::Recap)
        );
        assert_eq!(
            reopen_before(&kept, Some(&point("s1", None)), Some("s1"), true),
            Some(Reopen::Recap)
        );
        // Nothing said before it: just a new session.
        assert_eq!(reopen_before(&[], Some(&at), Some("s1"), true), None);
        assert_eq!(
            reopen_before(&[Item::Notice { text: "hi".into() }], None, None, false),
            None
        );
        assert_eq!(
            Reopen::Native {
                session: "s".into(),
                at: Some("m".into()),
                fork: true
            }
            .point(),
            Some(point("s", Some("m")))
        );
    }

    #[test]
    fn reopen_round_trips_as_json() {
        for r in [
            Reopen::Recap,
            Reopen::Native {
                session: "s".into(),
                at: None,
                fork: true,
            },
            Reopen::Native {
                session: "s".into(),
                at: Some("m".into()),
                fork: false,
            },
        ] {
            assert_eq!(
                serde_json::from_str::<Reopen>(&serde_json::to_string(&r).unwrap()).unwrap(),
                r
            );
        }
    }

    #[test]
    fn recaps_keep_the_conversation_and_drop_the_noise() {
        let tool = |d: &str| Item::Tool {
            id: "t".into(),
            title: "Edit".into(),
            detail: d.into(),
            output: "lots of output".into(),
            status: ToolStatus::Done,
        };
        let items = [
            Item::User {
                text: "fix the parser".into(),
                images: vec!["/tmp/s.png".into()],
                at: None,
                resume: None,
                aside: false,
            },
            Item::Reasoning { text: "hmm".into() },
            tool("src/parser.rs"),
            tool("src/lib.rs"),
            said("Fixed it."),
            end(),
            Item::Notice {
                text: "Interrupted".into(),
            },
            Item::Error {
                text: "boom".into(),
            },
            user("thanks"),
        ];
        assert_eq!(
            recap(&items, None),
            "The latest exchanges, oldest first:\n\nUser: fix the parser [1 image attached]\n\n(The assistant used 2 tool calls: Edit src/parser.rs; Edit src/lib.rs)\n\nAssistant: Fixed it.\n\nUser: thanks\n\nThe last turn failed: boom\n\nFiles changed so far: src/parser.rs, src/lib.rs"
        );
        assert_eq!(recap(&[said("woke up")], None), "", "nothing was asked");
    }

    #[test]
    fn recaps_are_short_orientations_that_point_at_the_transcript() {
        let long = "x".repeat(5_000);
        let mut items: Vec<Item> = vec![];
        for i in 0..30 {
            items.push(user(&format!("{i} {long}")));
            items.push(Item::Tool {
                id: format!("t{i}"),
                title: "Bash".into(),
                detail: format!("cargo test {i}"),
                output: "error[E0308]: mismatched types".into(),
                status: ToolStatus::Done,
            });
            items.push(said(&format!("{i} {long}")));
            items.push(end());
        }
        items.push(said("Shall I push it?"));
        let path = Path::new("/data/transcripts/t.log");
        let r = recap(&items, Some((path, 4_321)));
        assert!(r.len() <= RECAP_MAX, "{}", r.len());
        assert!(
            r.starts_with("The whole conversation so far is in the transcript at \"/data/transcripts/t.log\" (4321 lines)"),
            "{r}"
        );
        assert!(r.contains("transcript_search"));
        assert!(
            r.contains("What was asked first:\n\nUser: 0 "),
            "the task stays"
        );
        assert!(r.contains("(26 later messages and 26 tool calls before the latest exchanges are left out here: they're in the transcript.)"), "{r}");
        assert!(
            r.contains("User: 29 ") && r.contains("Assistant: Shall I push it?"),
            "the latest exchange is in, with the agent's last words in it"
        );
        assert!(r.contains("Assistant: 28 ") && !r.contains("Assistant: 29 "));
        assert!(!r.contains("User: 15 "));
        assert!(r.contains(" …"), "each message is shortened");
        assert!(!r.contains("E0308"), "tool output stays in the transcript");
        assert!(r.contains("(The assistant used 1 tool call: Bash cargo test 29)"));
        assert!(
            r.ends_with(
                "Open: the assistant's last message asked the user something, still unanswered."
            ),
            "{r}"
        );
        // Each message the mock agent remembers stays its own paragraph.
        assert!(r.split("\n\n").filter(|p| p.starts_with("User: ")).count() == 4);
    }
}
