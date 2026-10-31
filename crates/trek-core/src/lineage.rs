//! Lineage: threads an agent starts as threads of the user's own (`start_thread`), and the
//! thread its work moves to when it hands off (`handoff`, for a fresh context window). Unlike a
//! sub-agent (`orchestrate`), such a thread is in the sidebar like any other; it remembers which
//! thread started it (`Thread::started_by`) or which it continues (`Thread::continued_from`).
//!
//! Each link shows in both transcripts as a row (an `Item::Tool` whose id says which link and
//! which thread, `row`), so older builds and the phone show it too, as a plain tool row. The live
//! part (making threads, following them) is the app's.

use crate::store::{Item, ToolStatus};

/// How many levels of agent-started threads may stack under one the user started: theirs may
/// start threads, and those theirs, three levels down and no further.
pub const MAX_DEPTH: usize = 3;
/// Threads one thread may have at work at once that its agent started.
pub const MAX_RUNNING: usize = 8;
/// Threads one thread's agent may start between two messages from the user: a stop to one that
/// keeps on starting them.
pub const MAX_PER_REQUEST: usize = 16;
/// Handoffs one after another with no word from the user in between: a stop to agents handing
/// the work round in a loop.
pub const MAX_HANDOFFS_IN_A_ROW: usize = 3;

/// Transcript rows for links carry ids of this form: `trek-lineage:<link>:<thread id>`.
pub const ROW: &str = "trek-lineage:";

/// How a row's thread is related to the thread whose transcript it's in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Link {
    /// This thread's agent started it (`start_thread`).
    Started,
    /// This thread's work went on there (`handoff`).
    ContinuedIn,
    /// It started this thread.
    StartedBy,
    /// This thread took its work over.
    ContinuedFrom,
}

impl Link {
    pub const ALL: [Link; 4] = [
        Link::Started,
        Link::ContinuedIn,
        Link::StartedBy,
        Link::ContinuedFrom,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Link::Started => "started",
            Link::ContinuedIn => "continued-in",
            Link::StartedBy => "started-by",
            Link::ContinuedFrom => "continued-from",
        }
    }

    pub fn from_key(key: &str) -> Option<Link> {
        Link::ALL.into_iter().find(|l| l.key() == key)
    }

    /// What the row says before the other thread's title.
    pub fn label(self) -> &'static str {
        match self {
            Link::Started => "Started thread",
            Link::ContinuedIn => "Continued in",
            Link::StartedBy => "Started by",
            Link::ContinuedFrom => "Continued from",
        }
    }

    /// The row stands for this thread's agent's own call (`start_thread`, `handoff`), which it's
    /// placed after; the other two open a thread's transcript.
    pub fn answers_a_call(self) -> bool {
        matches!(self, Link::Started | Link::ContinuedIn)
    }
}

pub fn row_id(link: Link, thread: &str) -> String {
    format!("{ROW}{}:{thread}", link.key())
}

/// The link and thread a transcript row stands for.
pub fn of_row(id: &str) -> Option<(Link, &str)> {
    let (link, thread) = id.strip_prefix(ROW)?.split_once(':')?;
    Some((Link::from_key(link)?, thread)).filter(|(_, t)| !t.is_empty())
}

/// The row for `link` to `thread`, titled `title` as it was then.
pub fn row(link: Link, thread: &str, title: &str) -> Item {
    Item::Tool {
        id: row_id(link, thread),
        title: link.label().into(),
        detail: title.into(),
        output: String::new(),
        status: ToolStatus::Done,
    }
}

/// Whether an agent's tool row is a call to Trek's `start_thread` or `handoff`.
pub fn is_thread_call(title: &str) -> bool {
    crate::orchestrate::is_ours(title, "start_thread")
        || crate::orchestrate::is_ours(title, "handoff")
}

/// Whether an agent's tool row is a call to any of Trek's thread tools (`start_thread`,
/// `handoff`, `list_threads`, `thread_status`, `read_thread`, `send_to_thread`).
pub fn is_thread_tool(title: &str) -> bool {
    [
        "start_thread",
        "handoff",
        "list_threads",
        "thread_status",
        "read_thread",
        "send_to_thread",
    ]
    .iter()
    .any(|t| crate::orchestrate::is_ours(title, t))
}

/// The first message of a thread that takes over another's work: the brief its agent wrote,
/// framed so the new agent knows where it stands, and where that thread's whole transcript is
/// (`from`: its id, the log's path and line count) for anything exact the brief left out. (Not
/// the other thread's title: that's often the user's first message cut short, which the new
/// agent could take for its task.)
pub fn handoff_prompt(brief: &str, from: Option<(&str, &std::path::Path, u64)>) -> String {
    let transcript = match from {
        Some((id, path, lines)) => {
            format!(" {}", crate::session_log::retrieval(path, lines, Some(id)))
        }
        None => " Nothing else of that conversation comes with it.".to_string(),
    };
    format!(
        "You're taking over from another thread in Trek whose context was getting full. Its agent left you this brief. Check the state it describes before changing anything, then carry on with the next steps.{transcript}\n\n<brief>\n{}\n</brief>",
        brief.trim()
    )
}

/// How many levels of agent-started threads `id` sits under, given a way to look up the thread
/// that started a thread (or the sub-agent's parent).
pub fn depth(id: &str, started_by: impl Fn(&str) -> Option<String>) -> usize {
    crate::orchestrate::depth(id, started_by)
}

/// The brief inside a [`handoff_prompt`], when `text` is one: what the new thread is asked,
/// without Trek's frame around it.
pub fn handoff_brief(text: &str) -> Option<&str> {
    let text = text.strip_prefix("You're taking over from another thread in Trek")?;
    let (_, rest) = text.split_once("\n\n<brief>\n")?;
    rest.strip_suffix("\n</brief>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_name_their_link_and_thread() {
        for link in Link::ALL {
            assert_eq!(of_row(&row_id(link, "abc")), Some((link, "abc")));
            let Item::Tool {
                id, title, status, ..
            } = row(link, "abc", "Fix it")
            else {
                panic!("a tool row")
            };
            assert_eq!(
                (of_row(&id), title.as_str(), status),
                (Some((link, "abc")), link.label(), ToolStatus::Done)
            );
        }
        assert_eq!(of_row("trek-lineage:started:"), None);
        assert_eq!(of_row("trek-lineage:adopted:abc"), None);
        assert_eq!(of_row("trek-task:abc"), None);
        assert!(Link::Started.answers_a_call() && !Link::StartedBy.answers_a_call());
    }

    #[test]
    fn thread_calls_are_trek_s_own() {
        assert!(is_thread_call("mcp__trek-orchestrate__start_thread"));
        assert!(
            is_thread_call("trek-orchestrate_handoff"),
            "OpenCode's naming"
        );
        assert!(
            !is_thread_call("handoff"),
            "a bare name could be any server's"
        );
        assert!(!is_thread_call("mcp__trek-orchestrate__delegate_task"));
        assert!(
            is_thread_tool("mcp__trek-orchestrate__read_thread")
                && is_thread_tool("mcp__trek-orchestrate__handoff")
        );
        assert!(!is_thread_tool("mcp__trek-orchestrate__task_status"));
    }

    #[test]
    fn a_handoff_s_prompt_carries_the_brief() {
        let p = handoff_prompt("  Goal: fix it.\nNext: tests.  ", None);
        assert!(
            p.starts_with("You're taking over")
                && p.ends_with("<brief>\nGoal: fix it.\nNext: tests.\n</brief>"),
            "{p}"
        );
        let p = handoff_prompt(
            "Goal: fix it.",
            Some(("t-1", std::path::Path::new("/d/transcripts/t-1.log"), 88)),
        );
        assert!(
            p.contains("That thread's whole conversation is in its transcript at \"/d/transcripts/t-1.log\" (88 lines)")
                && p.contains("transcript_search with thread \"t-1\"")
                && p.ends_with("<brief>\nGoal: fix it.\n</brief>"),
            "{p}"
        );
        assert_eq!(handoff_brief(&p), Some("Goal: fix it."));
        assert_eq!(handoff_brief("Goal: fix it."), None);
    }
}
