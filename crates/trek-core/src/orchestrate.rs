//! Sub-agents: what Trek tells an agent it starts on another's behalf (`delegate_task`), what it
//! takes back as the answer, how it wakes the agent that asked, and the "consult" instructions
//! the composer adds to a message. The live part (threads, sessions, the socket) is the app's.

use crate::store::Item;
use crate::types::{AgentId, Effort};

/// How many levels of sub-agents may stack under a thread the user started: its sub-agents may
/// start their own, those may not.
pub const MAX_DEPTH: usize = 2;
/// Sub-agents one thread may have running at once.
pub const MAX_RUNNING: usize = 4;
/// The longest answer `task_result` returns (and `delegate_task` with `wait`).
pub const RESULT_CAP: usize = 16_000;
/// The longest answer in a wake-up message; `task_result` has the rest.
pub const WAKE_CAP: usize = 2_000;
/// The longest preview `task_status` gives.
pub const PREVIEW_CAP: usize = 600;

/// Transcript rows for sub-agents carry ids of this form, with the sub-agent's thread id after it.
pub const TASK_ROW: &str = "trek-task:";

pub fn task_row(child: &str) -> String {
    format!("{TASK_ROW}{child}")
}

/// The sub-agent a transcript row stands for.
pub fn task_of_row(id: &str) -> Option<&str> {
    id.strip_prefix(TASK_ROW).filter(|c| !c.is_empty())
}

/// What a sub-agent may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Read-only: review, research, recommend. Nothing it asks to do is approved.
    #[default]
    Advise,
    /// Change files, with its parent's access level.
    Implement,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "advise" | "advice" | "review" | "read-only" | "readonly" => Some(Mode::Advise),
            "implement" | "edit" | "write" => Some(Mode::Implement),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Advise => "advise",
            Mode::Implement => "implement",
        }
    }
}

/// The first message of a sub-agent: its task, framed so it knows it answers another agent (who
/// gets only its last message) and, advising, that it mustn't change anything.
pub fn child_prompt(mode: Mode, prompt: &str) -> String {
    let frame = match mode {
        Mode::Advise => "You're a consultant brought in by another coding agent, through Trek. Work read-only: read the code, search and reason, but don't edit files or run anything that changes them (any such request is declined). Your final message goes back to that agent as your answer, so make it complete on its own: findings first, then concrete recommendations, with file paths.",
        Mode::Implement => "You're a sub-agent started by another coding agent, through Trek, to do one task in this folder. Do it, check it works, and keep to the task. Your final message goes back to that agent as your report, so make it complete on its own: what you changed (with file paths), how you checked it, and anything left open.",
    };
    format!("{frame}\n\n<task>\n{}\n</task>", prompt.trim())
}

/// A sub-agent's answer: its last message in its last turn, after the plan it offered (advising
/// in plan mode, an agent's plan is its advice). `None` when it said nothing.
pub fn final_answer(items: &[Item], plan: Option<&str>) -> Option<String> {
    let start = items.iter().rposition(|i| matches!(i, Item::User { aside: false, .. })).map_or(0, |i| i + 1);
    let last = items[start..].iter().rev().find_map(|i| match i {
        Item::Assistant { text } if !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    });
    match (plan.map(str::trim).filter(|p| !p.is_empty()), last) {
        (Some(p), Some(l)) if !p.contains(l) => Some(format!("{p}\n\n{l}")),
        (Some(p), _) => Some(p.to_string()),
        (None, l) => l.map(str::to_string),
    }
}

/// The error a sub-agent's last turn ended with.
pub fn last_error(items: &[Item]) -> Option<String> {
    let start = items.iter().rposition(|i| matches!(i, Item::User { aside: false, .. })).map_or(0, |i| i + 1);
    items[start..].iter().rev().find_map(|i| if let Item::Error { text } = i { Some(text.trim().to_string()) } else { None })
}

/// `text`, cut to about `max` bytes (at a line or word break near the end) with a note saying so.
pub fn cap(text: &str, max: usize, more: &str) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let cut = head.rfind('\n').filter(|i| *i > max * 3 / 4).or_else(|| head.rfind(' ').filter(|i| *i > max * 3 / 4)).unwrap_or(end);
    format!("{}\n\n[… cut here: {} of {} characters. {more}]", text[..cut].trim_end(), cut, text.len())
}

/// The first `max` bytes of `text` on one line, for a preview.
pub fn preview(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() <= max {
        return flat;
    }
    let mut end = max;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", flat[..end].trim_end())
}

/// `preview`, without the markdown marks a person would see as noise in one line: headings,
/// list bullets, quotes, emphasis and code ticks.
pub fn plain_preview(text: &str, max: usize) -> String {
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .map(|l| {
            let l = l.trim_start().trim_start_matches('#').trim_start_matches('>').trim_start();
            let l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(l);
            let l = match l.split_once(". ") {
                Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
                _ => l,
            };
            l.replace("**", "").replace("__", "").replace('`', "")
        })
        .filter(|l| !l.trim().is_empty())
        .collect();
    preview(&lines.join(" "), max)
}

/// How a sub-agent ended, for the agent that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: String,
    pub title: String,
    /// "Sol", "Opus 5.5".
    pub model: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Failed(String),
    Cancelled,
}

/// Wake-up messages start with this. The transcript draws them as a note, not as something the
/// user typed.
pub const WAKE_PREFIX: &str = "[Trek] ";

pub fn is_wake(text: &str) -> bool {
    text.starts_with(WAKE_PREFIX)
}

/// The message that wakes an agent with what its sub-agents came back with.
pub fn wake_text(reports: &[Report]) -> String {
    let blocks: Vec<String> = reports
        .iter()
        .map(|r| {
            let who = format!("“{}” ({}, task {})", r.title, r.model, r.id);
            match &r.outcome {
                Outcome::Done(text) => format!("{WAKE_PREFIX}Sub-agent {who} finished:\n\n{}", cap(text, WAKE_CAP, "Call task_result for the whole answer.")),
                Outcome::Failed(why) => format!("{WAKE_PREFIX}Sub-agent {who} failed: {}", cap(why, WAKE_CAP, "")),
                Outcome::Cancelled => format!("{WAKE_PREFIX}Sub-agent {who} was stopped before it finished."),
            }
        })
        .collect();
    blocks.join("\n\n---\n\n")
}

/// The wake-up note as the transcript shows it: "Sol reported back on “Review the cache”".
pub fn wake_summary(text: &str) -> String {
    let n = text.matches(WAKE_PREFIX).count();
    let first = text.strip_prefix(WAKE_PREFIX).and_then(|t| t.strip_prefix("Sub-agent ")).unwrap_or_default();
    let title = first.split_once('”').map(|(t, _)| t.trim_start_matches('“')).unwrap_or("a task");
    let model = first.split_once("” (").and_then(|(_, rest)| rest.split_once(',')).map(|(m, _)| m).unwrap_or("A sub-agent");
    let verb = if first.contains(" failed: ") {
        "failed on"
    } else if first.contains(" was stopped ") {
        "was stopped on"
    } else {
        "reported back on"
    };
    match n {
        0 | 1 => format!("{model} {verb} “{title}”"),
        n => format!("{n} sub-agents reported back"),
    }
}

/// Whether an agent's tool row is a call to Trek's `delegate_task` (named however the agent
/// names MCP tools: `mcp__trek-orchestrate__delegate_task`, `delegate_task`, …).
pub fn is_delegate_call(title: &str) -> bool {
    title.ends_with("delegate_task")
}

/// A readable title for a row of one of Trek's other orchestration tools.
pub fn tool_label(title: &str) -> Option<&'static str> {
    let ours = |tool: &str| title == tool || (title.ends_with(tool) && title.contains("trek-orchestrate"));
    [
        ("list_models", "Listed the models"),
        ("task_status", "Checked on a sub-agent"),
        ("task_result", "Read a sub-agent's answer"),
        ("cancel_task", "Stopped a sub-agent"),
    ]
    .into_iter()
    .find(|(tool, _)| ours(tool))
    .map(|(_, label)| label)
}

/// One model asked for advice: the agent that runs it, the model and its effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consultant {
    pub agent: AgentId,
    pub model: String,
    pub effort: Effort,
}

impl Consultant {
    /// `codex/gpt-5.6-sol/high`, as the consult block records it.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.agent.key(), self.model, self.effort.as_str())
    }

    pub fn from_key(key: &str) -> Option<Consultant> {
        // Agent keys have no slash; model ids may.
        let (agent, rest) = key.trim().split_once('/')?;
        let (model, effort) = rest.rsplit_once('/')?;
        let effort = Effort::parse(effort)?;
        (!model.is_empty() && !agent.is_empty()).then(|| Consultant { agent: AgentId::from_key(agent), model: model.to_string(), effort })
    }
}

/// How the agent works with its consultants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// They review and suggest; the agent decides.
    #[default]
    Advise,
    /// The agent and its consultants go back and forth until they agree (a few rounds at most).
    Discuss,
}

/// Consultants picked in the composer, and what happens once they've had their say.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Consult {
    pub consultants: Vec<Consultant>,
    pub style: Style,
    /// Then implement (else just report).
    pub implement: bool,
}

/// Rounds a discussion may take.
pub const DISCUSS_ROUNDS: usize = 3;

const CONSULT_OPEN: &str = "<trek-consult";
const CONSULT_CLOSE: &str = "</trek-consult>";

/// `text` with the instructions for consulting: which models to ask (named for the agent with
/// `name`, e.g. "Sol · High"), how, and what to do after.
pub fn consult_prompt(text: &str, consult: &Consult, name: impl Fn(&Consultant) -> String) -> String {
    let with: Vec<String> = consult.consultants.iter().map(Consultant::key).collect();
    let style = match consult.style {
        Style::Advise => "advise",
        Style::Discuss => "discuss",
    };
    let then = if consult.implement { "implement" } else { "report" };
    let list: Vec<String> = consult
        .consultants
        .iter()
        .map(|c| format!("   - {}: agent \"{}\", model \"{}\", effort \"{}\"", name(c), c.agent.key(), c.model, c.effort.as_str()))
        .collect();
    let mut steps = vec![format!(
        "1. Call `delegate_task` once for each consultant below, all at once if you can, with mode \"advise\" and wait true:\n{}\n   They can't see this conversation: give each a self-contained brief (the request above, the files that matter, what you've found so far) and ask for their review and recommendations, not for edits.",
        list.join("\n")
    )];
    match consult.style {
        Style::Advise => {
            steps.push("2. Weigh their advice against your own judgment: the decision is yours.".into());
            steps.push(format!(
                "3. Tell me in a few lines what each consultant recommended and what you decided, {}",
                if consult.implement { "then implement it." } else { "then stop there: don't change any files." }
            ));
        }
        Style::Discuss => {
            steps.push(format!(
                "2. Where you disagree with a consultant, or they disagree with each other, run another round: call `delegate_task` for them again with the brief, every position so far and the open objections. Stop once you agree, or after {DISCUSS_ROUNDS} rounds in all."
            ));
            steps.push(format!(
                "3. Tell me in a few lines where you landed: what you agreed on (or what's still disputed, and your call), {}",
                if consult.implement { "then implement it." } else { "then stop there: don't change any files." }
            ));
        }
    }
    format!(
        "{}\n\n{CONSULT_OPEN} style=\"{style}\" then=\"{then}\" with=\"{}\">\nBefore you act on this, consult other models through Trek's `delegate_task` tool:\n\n{}\n\nIf `delegate_task` isn't available to you, say so and stop.\n{CONSULT_CLOSE}",
        text.trim_end(),
        with.join(", "),
        steps.join("\n")
    )
}

/// A message split into what the user wrote and the consult it carries, if any.
pub fn split_consult(text: &str) -> (&str, Option<Consult>) {
    let Some(at) = text.rfind(&format!("\n\n{CONSULT_OPEN}")) else { return (text, None) };
    let block = &text[at + 2..];
    if !block.trim_end().ends_with(CONSULT_CLOSE) {
        return (text, None);
    }
    let head = block.lines().next().unwrap_or_default();
    let attr = |name: &str| head.split_once(&format!("{name}=\"")).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v);
    let consultants: Vec<Consultant> = attr("with").unwrap_or_default().split(',').filter_map(Consultant::from_key).collect();
    if consultants.is_empty() {
        return (text, None);
    }
    let consult = Consult {
        consultants,
        style: if attr("style") == Some("discuss") { Style::Discuss } else { Style::Advise },
        implement: attr("then") != Some("report"),
    };
    (text[..at].trim_end(), Some(consult))
}

/// How many levels of parents `id` has, given a way to look a thread's parent up.
pub fn depth(id: &str, parent_of: impl Fn(&str) -> Option<String>) -> usize {
    let mut depth = 0;
    let mut at = id.to_string();
    while let Some(p) = parent_of(&at) {
        depth += 1;
        // A loop can't be made through Trek, but a damaged database mustn't hang it.
        if depth > 16 {
            break;
        }
        at = p;
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> Item {
        Item::User { text: t.into(), images: vec![], at: None, resume: None, aside: false }
    }
    fn said(t: &str) -> Item {
        Item::Assistant { text: t.into() }
    }

    #[test]
    fn the_answer_is_the_last_thing_said_in_the_last_turn() {
        let items = vec![user("first"), said("old answer"), user("review it"), said("Let me look."), Item::Reasoning { text: "hm".into() }, said("Use a cache."), said(" ")];
        assert_eq!(final_answer(&items, None).as_deref(), Some("Use a cache."));
        assert_eq!(final_answer(&[user("go")], None), None);
        // Advising in plan mode, the plan it offered is the advice.
        assert_eq!(final_answer(&items, Some("## Plan\n1. Cache")).as_deref(), Some("## Plan\n1. Cache\n\nUse a cache."));
        assert_eq!(final_answer(&[user("go")], Some("plan")).as_deref(), Some("plan"));
        let failed = vec![user("go"), Item::Error { text: "Usage limit reached".into() }];
        assert_eq!(last_error(&failed).as_deref(), Some("Usage limit reached"));
        assert_eq!(last_error(&items), None);
    }

    #[test]
    fn long_answers_are_cut_with_a_note() {
        assert_eq!(cap("short", 10, "x"), "short");
        let long = "word ".repeat(1000);
        let out = cap(&long, 100, "Call task_result.");
        assert!(out.len() < 200 && out.contains("of 5000 characters. Call task_result.]"), "{out}");
        let unicode = "é".repeat(100);
        assert!(cap(&unicode, 51, "").starts_with("éé"), "cuts on a character boundary");
        assert_eq!(preview("a\n\n  b   c", 100), "a b c");
        assert_eq!(preview("abcdef", 3), "abc…");
        assert_eq!(plain_preview("## Findings\n\n- `sum2` is **vague**\n1. Rename it\n> ok\n```rust\n", 100), "Findings sum2 is vague Rename it ok");
    }

    #[test]
    fn rows_name_their_sub_agent() {
        assert_eq!(task_of_row(&task_row("abc")), Some("abc"));
        assert_eq!(task_of_row("toolu_1"), None);
        assert_eq!(task_of_row(TASK_ROW), None);
    }

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse("Advise"), Some(Mode::Advise));
        assert_eq!(Mode::parse("implement"), Some(Mode::Implement));
        assert_eq!(Mode::parse("delete everything"), None);
        assert!(child_prompt(Mode::Advise, " look ").contains("read-only") && child_prompt(Mode::Advise, " look ").ends_with("<task>\nlook\n</task>"));
        assert!(child_prompt(Mode::Implement, "fix").contains("what you changed"));
    }

    #[test]
    fn wake_ups_carry_each_result_and_read_as_a_note() {
        let reports = vec![
            Report { id: "a1".into(), title: "Review the cache".into(), model: "Sol".into(), outcome: Outcome::Done("Looks good.".into()) },
            Report { id: "b2".into(), title: "Check the tests".into(), model: "Opus 5.5".into(), outcome: Outcome::Failed("Usage limit reached".into()) },
        ];
        let one = wake_text(&reports[..1]);
        assert!(is_wake(&one));
        assert_eq!(one, "[Trek] Sub-agent “Review the cache” (Sol, task a1) finished:\n\nLooks good.");
        assert_eq!(wake_summary(&one), "Sol reported back on “Review the cache”");
        let failed = wake_text(&reports[1..]);
        assert_eq!(wake_summary(&failed), "Opus 5.5 failed on “Check the tests”");
        let both = wake_text(&reports);
        assert!(both.contains("Usage limit reached") && both.contains("Looks good."));
        assert_eq!(wake_summary(&both), "2 sub-agents reported back");
        let stopped = wake_text(&[Report { outcome: Outcome::Cancelled, ..reports[0].clone() }]);
        assert_eq!(wake_summary(&stopped), "Sol was stopped on “Review the cache”");
        let big = wake_text(&[Report { outcome: Outcome::Done("x ".repeat(5000)), ..reports[0].clone() }]);
        assert!(big.len() < WAKE_CAP + 300 && big.contains("Call task_result"), "capped");
        assert!(!is_wake("hello [Trek] "));
    }

    #[test]
    fn orchestration_rows_are_recognised() {
        assert!(is_delegate_call("mcp__trek-orchestrate__delegate_task"));
        assert!(is_delegate_call("delegate_task"));
        assert!(!is_delegate_call("Subagent"));
        assert_eq!(tool_label("mcp__trek-orchestrate__task_status"), Some("Checked on a sub-agent"));
        assert_eq!(tool_label("cancel_task"), Some("Stopped a sub-agent"));
        assert_eq!(tool_label("mcp__other__list_models"), None, "another server's tool keeps its name");
    }

    #[test]
    fn consultants_round_trip() {
        let c = Consultant { agent: AgentId::Codex, model: "gpt-5.6-sol".into(), effort: Effort::High };
        assert_eq!(c.key(), "codex/gpt-5.6-sol/high");
        assert_eq!(Consultant::from_key(&c.key()), Some(c.clone()));
        let acp = Consultant { agent: AgentId::Acp("github-copilot".into()), model: "openai/gpt-5".into(), effort: Effort::Low };
        assert_eq!(Consultant::from_key(&acp.key()), Some(acp), "a model id with a slash");
        assert_eq!(Consultant::from_key("codex//high"), None);
        assert_eq!(Consultant::from_key("codex/x/loud"), None);
    }

    #[test]
    fn consult_instructions_name_each_model_and_come_back_off() {
        let consult = Consult {
            consultants: vec![
                Consultant { agent: AgentId::Codex, model: "gpt-5.6-sol".into(), effort: Effort::High },
                Consultant { agent: AgentId::ClaudeCode, model: "claude-opus-5-5".into(), effort: Effort::Max },
            ],
            style: Style::Discuss,
            implement: false,
        };
        let text = consult_prompt("Should the diff panel default to unified?\n", &consult, |c| format!("{} model", c.model));
        assert!(text.starts_with("Should the diff panel default to unified?\n\n<trek-consult"));
        assert!(text.contains("gpt-5.6-sol model: agent \"codex\", model \"gpt-5.6-sol\", effort \"high\""));
        assert!(text.contains("agent \"claude-code\", model \"claude-opus-5-5\", effort \"max\""));
        assert!(text.contains("another round") && text.contains("don't change any files"));
        let (said, back) = split_consult(&text);
        assert_eq!(said, "Should the diff panel default to unified?");
        assert_eq!(back, Some(consult.clone()));
        let advise = Consult { style: Style::Advise, implement: true, ..consult };
        let text = consult_prompt("go", &advise, |_| "m".into());
        assert!(text.contains("then implement it.") && !text.contains("another round"));
        assert_eq!(split_consult(&text).1, Some(advise));
        assert_eq!(split_consult("plain text"), ("plain text", None));
        assert_eq!(split_consult("a\n\n<trek-consult with=\"\">\n</trek-consult>").1, None);
    }

    #[test]
    fn depth_counts_parents() {
        let parents = std::collections::HashMap::from([("c", "b"), ("b", "a")]);
        let of = |id: &str| parents.get(id).map(|p| p.to_string());
        assert_eq!(depth("a", of), 0);
        assert_eq!(depth("b", of), 1);
        assert_eq!(depth("c", of), 2);
        let looped = std::collections::HashMap::from([("x", "y"), ("y", "x")]);
        assert!(depth("x", |id: &str| looped.get(id).map(|p| p.to_string())) <= 17);
    }
}
