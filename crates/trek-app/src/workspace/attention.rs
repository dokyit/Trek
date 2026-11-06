//! What needs the user, and why, in one place: the list behind the sidebar's "Needs you", the keys
//! that jump through it, and the words a waiting row says, which are the words its alert says.

use super::{PendingPermission, Scope, Workspace};
use std::cmp::Reverse;
use std::collections::HashSet;
use trek_agents::Prompt;
use trek_core::RunState;
use trek_core::store::{Item, Section, Thread, now_ms};

/// What the jump key says when there's nowhere to go.
const NOTHING_NEEDS_YOU: &str = "Nothing needs you.";
const ONLY_THIS_ONE: &str = "Only this thread needs you.";
/// What the key back to the last prompt says when it can't go anywhere.
const NO_PROMPT_YET: &str = "No prompt sent since Trek opened.";
const PROMPT_THREAD_GONE: &str = "The thread you last sent a prompt to is gone.";
const ALREADY_THERE: &str = "You last sent a prompt here.";

/// Long enough for any row and most banners; the rest of a long command is on the transcript's card.
const MOST_WORDS: usize = 120;

/// Why a thread is in "Needs you".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// An approval, in words ("run cargo test", "create an empty file").
    Approval(String),
    /// A question, as the agent put it.
    Question(String),
    Plan,
    /// A sub-agent of its own waits on an approval (sub-agents have no rows of their own).
    SubAgent,
    /// It waits on the user, as a machine's row put it; none: what for isn't known here.
    Waiting(Option<String>),
    /// Its turn failed; the error, when its transcript is at hand.
    Failed(Option<String>),
    /// Trek closed in the middle of its turn, or while it waited on a card.
    CutShort,
    /// Its pull request has failing checks, in words (`pr_checks`).
    Checks(String),
    /// Its turn is over and the user hasn't looked.
    Finished,
}

impl Why {
    /// What a thread that's asking for something is asking for.
    pub fn asking(title: &str, detail: &str, prompt: Option<&Prompt>) -> Why {
        match prompt {
            Some(Prompt::Questions(questions)) => Why::Question(
                questions
                    .first()
                    .map(|q| clip(one_line(&q.question)))
                    .unwrap_or_default(),
            ),
            Some(Prompt::Plan(_)) => Why::Plan,
            // The name alone, after "Waiting:": this can reach a banner and the phone, and a row
            // has room for little more.
            Some(Prompt::Secret { name, .. }) => Why::Question(clip(name)),
            None => Why::Approval(approval_words(title, detail)),
        }
    }

    fn of_request(request: &PendingPermission) -> Why {
        Why::asking(&request.title, &request.detail, request.prompt.as_ref())
    }

    /// What a row says, and (with the thread's name, `alert`) what the banner said.
    pub fn words(&self) -> String {
        match self {
            Why::Approval(what) => format!("Waiting: {}?", what.trim_end_matches('?')),
            Why::Question(question) if question.is_empty() => "Waiting: a question for you".into(),
            Why::Question(question) => format!("Waiting: {question}"),
            Why::Plan => "Waiting: a plan to review".into(),
            Why::SubAgent => "Sub-agent needs you".into(),
            Why::Waiting(Some(what)) => format!("Waiting: {what}"),
            Why::Waiting(None) => "Needs you".into(),
            Why::Failed(Some(error)) => format!("Failed: {error}"),
            Why::Failed(None) => "Failed".into(),
            Why::CutShort => "Cut short when Trek closed".into(),
            Why::Checks(words) => words.clone(),
            Why::Finished => "Finished".into(),
        }
    }

    /// The banner for a thread that just started waiting: the row's words, then which thread.
    pub fn alert(&self, title: &str) -> String {
        format!("{} · {title}", self.words())
    }

    /// It can't go on without the user, or it failed. What the Dock badge counts; what's merely
    /// unread isn't called for.
    pub fn calls_for_you(&self) -> bool {
        !matches!(self, Why::CutShort | Why::Finished)
    }

    /// Asked first, then failures, then what's only news.
    fn rank(&self) -> u8 {
        match self {
            Why::Approval(_) | Why::Question(_) | Why::Plan | Why::SubAgent | Why::Waiting(_) => 0,
            Why::Failed(_) | Why::Checks(_) => 1,
            Why::CutShort | Why::Finished => 2,
        }
    }
}

/// An approval in a few plain words, to follow "Waiting:": "run cargo test", "edit main.rs". The
/// agents give a tool's name and what it would act on (`tool_title` and its like), or their own
/// words for the call when they have them (Claude Code does for a command), which read best.
pub fn approval_words(title: &str, detail: &str) -> String {
    let title = title.trim();
    let detail = one_line(detail);
    if detail.is_empty() {
        return clip(tool_words(title));
    }
    let said = is_sentence(detail);
    clip(match title {
        "Run command" if said => lower_first(detail),
        "Run command" => format!("run {}", without_cd(detail)),
        "Read" | "Edit" | "Write" if said && !detail.contains(['/', '\\']) => lower_first(detail),
        "Read" | "Edit" | "Write" => format!("{} {}", title.to_lowercase(), files_in(detail)),
        "Search" | "Fetch" | "List files" => format!("{} {detail}", title.to_lowercase()),
        "Search the web" => format!("search the web for {detail}"),
        "Network access" => format!("network access to {detail}"),
        "" => detail.to_string(),
        // Anything else names itself; its detail is whatever the tool took (JSON, often).
        _ => tool_words(title),
    })
}

/// A tool's name as words: an MCP tool without its server's prefix, anything else as it reads.
fn tool_words(title: &str) -> String {
    match title.rsplit_once("__") {
        Some((_, tool)) if title.starts_with("mcp__") => format!("use {tool}"),
        _ => lower_first(title),
    }
}

/// `text` to go mid-sentence. Left as it is when it opens with an acronym ("URL…").
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if !second.is_uppercase() => first
            .to_lowercase()
            .chain(text[first.len_utf8()..].chars())
            .collect(),
        _ => text.to_string(),
    }
}

/// The agent's own words for a call ("Create an empty file"), not a command or a path: a
/// capitalized word, and more after it.
fn is_sentence(detail: &str) -> bool {
    let Some((first, _)) = detail.split_once(' ') else {
        return false;
    };
    let mut letters = first.chars();
    letters.next().is_some_and(char::is_uppercase) && letters.all(char::is_lowercase)
}

/// `cd /some/where && cargo test` is about `cargo test`: the folder would fill the row.
fn without_cd(command: &str) -> &str {
    command
        .strip_prefix("cd ")
        .and_then(|rest| rest.split_once(" && "))
        .map_or(command, |(_, rest)| rest.trim_start())
}

/// The file an edit is about, by name ("main.rs"); several as the first and how many more.
fn files_in(detail: &str) -> String {
    let mut files = detail.split(", ");
    let first = files.next().unwrap_or(detail);
    let name = first
        .rsplit(['/', '\\'])
        .find(|p| !p.is_empty())
        .unwrap_or(first);
    match files.count() {
        0 => name.to_string(),
        more => format!("{name} and {more} more"),
    }
}

fn one_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

fn clip(text: impl Into<String>) -> String {
    let text = text.into();
    match text.char_indices().nth(MOST_WORDS) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text,
    }
}

impl Workspace {
    /// Everything that needs the user, in the order the sidebar lists it and the jump key goes
    /// through it: what's asking first, then failures, then what finished unread; newest first
    /// within each. Snoozed threads count once they raise their hand; settled, archived, side and
    /// editor chats don't (nothing in the sidebar could clear them), nor do sub-agents, which
    /// count as their parent.
    pub fn needing_you(&self) -> Vec<(&Thread, Why)> {
        let now = now_ms();
        let sub_agents = self.waiting_on_sub_agents();
        let waiting = self.waiting_threads();
        let mut out: Vec<(&Thread, Why)> = self
            .threads
            .iter()
            .filter_map(|t| {
                let section = t.section(now)?;
                let why = self.why_needed(t, section, &sub_agents, &waiting)?;
                Some((t, why))
            })
            .collect();
        out.extend(self.machines_needing_you());
        out.sort_by_key(|(t, why)| (why.rank(), Reverse(t.updated_at)));
        out
    }

    fn why_needed(
        &self,
        t: &Thread,
        section: Section,
        sub_agents: &HashSet<String>,
        waiting: &HashSet<String>,
    ) -> Option<Why> {
        if t.run_state == RunState::NeedsYou {
            return Some(match self.pending_request(&t.id) {
                Some(request) => Why::of_request(request),
                None => Why::Waiting(None),
            });
        }
        if sub_agents.contains(&t.id) {
            return Some(Why::SubAgent);
        }
        if t.needs_you() {
            return Some(Why::Failed(self.last_error(&t.id)));
        }
        // A pull request it opened is failing, unless it's at work on that or something else now.
        if t.run_state == RunState::Idle {
            if let Some(offer) = self.pr_checks_offer(&t.id) {
                return Some(Why::Checks(offer.row_words()));
            }
        }
        // Idle and unread: unless it's only between turns (its sub-agents are out), or paused at a
        // usage limit with nothing to do yet.
        let finished = t.run_state == RunState::Idle
            && t.is_unseen()
            && t.paused.is_none()
            && matches!(section, Section::Inbox | Section::Pinned)
            && !waiting.contains(&t.id);
        finished.then(|| match self.cut_short.get(&t.id) {
            // Cut off while it waited on the user: it still does (its offer only runs it again
            // when they say so), and that's what calls for them, not news.
            Some(_) if self.cut_off_waiting(t) => Why::Waiting(
                self.interrupted_offer(&t.id)
                    .and_then(|o| o.turn.asked.as_deref())
                    .map(one_line)
                    .map(clip),
            ),
            Some(at) if *at == t.updated_at => Why::CutShort,
            _ => Why::Finished,
        })
    }

    /// `t` was cut off by Trek closing while it waited on the user, and waits on them still: its
    /// offer is open and it hasn't been looked at. It calls for them (the Dock badge counts it).
    pub fn cut_off_waiting(&self, t: &Thread) -> bool {
        t.run_state == RunState::Idle
            && t.is_unseen()
            && self.cut_short.get(&t.id) == Some(&t.updated_at)
            && self
                .interrupted_offer(&t.id)
                .is_some_and(|o| o.turn.waiting)
    }

    fn last_error(&self, id: &str) -> Option<String> {
        self.live.get(id)?.items.iter().rev().find_map(|i| match i {
            Item::Error { text } => Some(clip(one_line(text))).filter(|e| !e.is_empty()),
            _ => None,
        })
    }

    /// What waits on the user on the machines that can be reached, by what their rows say. One
    /// that can't be reached can't say, and nothing there could be answered from here. Only what
    /// calls for the user: opening a thread there isn't sure to mark it read at once, and an
    /// unread row would stay on the list it was just opened from.
    pub(super) fn machines_needing_you(&self) -> Vec<(&Thread, Why)> {
        let sub_agents = self.machines.sub_agents_need_you();
        self.machines
            .list
            .iter()
            .filter(|m| m.reachable())
            .flat_map(|m| m.listed())
            .filter_map(|t| {
                let said = || self.machines.needs_text(&t.id).map(one_line).map(clip);
                let why = match t.run_state {
                    RunState::NeedsYou => Why::Waiting(said()),
                    _ if sub_agents.contains(&t.id) => Why::SubAgent,
                    _ if t.needs_you() => Why::Failed(said()),
                    _ => return None,
                };
                Some((t, why))
            })
            .collect()
    }

    /// What a waiting thread's row and alert say, for one thread.
    pub fn why_needs_you(&self, id: &str) -> Option<Why> {
        self.needing_you()
            .into_iter()
            .find(|(t, _)| t.id == id)
            .map(|(_, why)| why)
    }

    /// The thread the jump key goes to from `from`: the one after it in `needing_you`, round to the
    /// first. A finished thread leaves the list as it's opened, so the next press carries on from
    /// where it was (`jumped_to`) rather than from the top. `Err`: what to say instead.
    pub fn next_needing_you(&mut self, from: Option<&str>) -> Result<String, &'static str> {
        let list: Vec<String> = self
            .needing_you()
            .into_iter()
            .map(|(t, _)| t.id.clone())
            .collect();
        if list.is_empty() {
            return Err(NOTHING_NEEDS_YOU);
        }
        let at = match from.and_then(|id| list.iter().position(|t| t == id)) {
            Some(_) if list.len() == 1 => return Err(ONLY_THIS_ONE),
            Some(at) => (at + 1) % list.len(),
            None => self
                .jumped_to
                .as_ref()
                .filter(|(id, _)| from == Some(id.as_str()))
                .map_or(0, |(_, at)| at % list.len()),
        };
        self.jumped_to = Some((list[at].clone(), at));
        Ok(list[at].clone())
    }

    /// The user sent a prompt from `scope`'s composer: the key back to it goes there. A side chat
    /// answers beside its thread, which is what's on screen anyway.
    pub fn note_prompt_sent(&mut self, scope: &Scope) {
        if matches!(scope, Scope::Side) {
            return;
        }
        if let Some(id) = self.thread_id_in(scope).map(str::to_string) {
            self.last_prompt = Some(id);
        }
    }

    /// The thread the user last sent a prompt to, unless it's gone or `from` is it. `Err`: what to
    /// say instead.
    pub fn last_prompt_thread(&self, from: Option<&str>) -> Result<String, &'static str> {
        let id = self.last_prompt.as_deref().ok_or(NO_PROMPT_YET)?;
        if self.thread(id).is_none_or(|t| t.archived_at.is_some()) {
            return Err(PROMPT_THREAD_GONE);
        }
        if from == Some(id) {
            return Err(ALREADY_THERE);
        }
        Ok(id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_agents::Question;

    #[test]
    fn approvals_read_as_a_few_plain_words() {
        for (title, detail, words) in [
            ("Run command", "cargo test", "run cargo test"),
            (
                "Run command",
                "cd /Users/me/project && cargo test -p app\necho done",
                "run cargo test -p app",
            ),
            // Claude Code's own words for a command, when it has them.
            (
                "Run command",
                "Create an empty file named cancel.txt",
                "create an empty file named cancel.txt",
            ),
            ("Run command", "Run the test suite", "run the test suite"),
            ("Run command", "FOO=1 make test", "run FOO=1 make test"),
            ("Run command", "Makefile", "run Makefile"),
            ("Edit", "/a/b/main.rs", "edit main.rs"),
            ("Edit", "/My Work/main.rs", "edit main.rs"),
            ("Edit", "Update the config", "update the config"),
            ("Edit", "src/a.rs, src/b.rs, c.rs", "edit a.rs and 2 more"),
            ("Write", "C:\\work\\notes.md", "write notes.md"),
            ("Fetch", "https://example.com", "fetch https://example.com"),
            ("Search the web", "ratatui", "search the web for ratatui"),
            (
                "Network access",
                "github.com",
                "network access to github.com",
            ),
            ("Grant permissions", "write: /etc", "grant permissions"),
            ("mcp__github__create_issue", "{\"a\":1}", "use create_issue"),
            ("Run the tests", "Runs them", "run the tests"),
            ("URL check", "", "URL check"),
            ("Run command", "", "run command"),
            ("", "something odd", "something odd"),
        ] {
            assert_eq!(approval_words(title, detail), words);
        }
    }

    #[test]
    fn a_long_command_is_clipped_for_the_row() {
        let words = approval_words("Run command", &"x".repeat(500));
        assert_eq!(words.chars().count(), MOST_WORDS + 1);
        assert!(words.ends_with('…'));
    }

    #[test]
    fn what_a_thread_waits_for_is_said_the_same_way_on_the_row_and_in_the_alert() {
        let approval = Why::asking("Run command", "cargo test", None);
        assert_eq!(approval.words(), "Waiting: run cargo test?");
        assert_eq!(
            Why::asking("Allow this?", "", None).words(),
            "Waiting: allow this?",
            "one question mark"
        );
        assert_eq!(
            approval.alert("Fix the login bug"),
            "Waiting: run cargo test? · Fix the login bug"
        );
        let question = Prompt::Questions(vec![Question {
            question: "Which region first?".into(),
            header: String::new(),
            options: vec![],
            multi: false,
            secret: false,
        }]);
        assert_eq!(
            Why::asking("Question", "", Some(&question)).words(),
            "Waiting: Which region first?"
        );
        let plan = Prompt::Plan("Do it".into());
        assert_eq!(
            Why::asking("Plan", "", Some(&plan)).words(),
            "Waiting: a plan to review"
        );
        let secret = Prompt::Secret {
            name: "GITHUB_TOKEN".into(),
            why: "To open the pull request.".into(),
        };
        assert_eq!(
            Why::asking("Secret", "", Some(&secret)).words(),
            "Waiting: GITHUB_TOKEN",
            "the name, never why it's wanted"
        );
    }

    #[test]
    fn asking_comes_first_then_failures_then_news() {
        let order = [
            Why::Finished,
            Why::Failed(None),
            Why::Approval("run x".into()),
            Why::CutShort,
            Why::SubAgent,
        ];
        let mut ranked = order.clone();
        ranked.sort_by_key(Why::rank);
        assert_eq!(ranked[0], Why::Approval("run x".into()));
        assert_eq!(ranked[1], Why::SubAgent);
        assert_eq!(ranked[2], Why::Failed(None));
        assert!(!Why::Finished.calls_for_you() && !Why::CutShort.calls_for_you());
        assert!(Why::Failed(None).calls_for_you() && Why::SubAgent.calls_for_you());
    }
}
