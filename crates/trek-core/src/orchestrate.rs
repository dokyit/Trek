//! Sub-agents: what Trek tells an agent it starts on another's behalf (`delegate_task`), what it
//! takes back as the answer, how it wakes the agent that asked, and the "consult" instructions
//! the composer adds to a message. The live part (threads, sessions, the socket) is the app's.

use crate::catalog::ModelInfo;
use crate::store::Item;
use crate::types::{AgentId, Effort};

/// How many levels of sub-agents may stack under a thread the user started: its sub-agents may
/// start their own, those may not.
pub const MAX_DEPTH: usize = 2;
/// Sub-agents one thread may have running at once.
pub const MAX_RUNNING: usize = 4;
/// Sub-agents one thread may start between two messages from the user: enough for four
/// consultants over every round of a discussion, and a stop to an agent that keeps on starting
/// them.
pub const MAX_PER_REQUEST: usize = MAX_RUNNING * DISCUSS_ROUNDS;
/// What a sub-agent's row says when Trek quit while it ran.
pub const CUT_OFF: &str = "Trek quit before it finished.";
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
        // A table's rule row says nothing; its cells read as a list.
        .filter(|l| !(l.contains("---") && l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))))
        .map(|l| match l.trim().strip_prefix('|').and_then(|r| r.strip_suffix('|')) {
            Some(cells) => cells.split('|').map(str::trim).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" · "),
            None => l.to_string(),
        })
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub id: String,
    pub title: String,
    /// "Sol", "Opus 5.5".
    pub model: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Trek's orchestration tools reach agents as the MCP server of this name.
pub const SERVER: &str = "trek-orchestrate";

/// Whether tool row `title` is Trek's own `tool`, named however the agent names MCP tools:
/// `mcp__trek-orchestrate__delegate_task` (Claude, and Codex as Trek reports it),
/// `trek-orchestrate_delegate_task` (OpenCode). Another server's tool of the same name isn't.
fn is_ours(title: &str, tool: &str) -> bool {
    title.strip_suffix(tool).is_some_and(|server| server.trim_end_matches(['_', '.', '/', ':', '-']).ends_with(SERVER))
}

/// Whether an agent's tool row is a call to Trek's `delegate_task`.
pub fn is_delegate_call(title: &str) -> bool {
    is_ours(title, "delegate_task")
}

/// A readable title for a row of one of Trek's other orchestration tools.
pub fn tool_label(title: &str) -> Option<&'static str> {
    [
        ("list_models", "Listed the models"),
        ("task_status", "Checked on a sub-agent"),
        ("task_result", "Read a sub-agent's answer"),
        ("cancel_task", "Stopped a sub-agent"),
    ]
    .into_iter()
    .find(|(tool, _)| is_ours(title, tool))
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
    /// A design arena (pstack's "measure a hundred times, cut once"): the agent grounds the
    /// problem, each consultant drafts a design of its own, a judge on another model (of another
    /// family when there's one) scores them blind, and the agent synthesises one.
    Arena,
}

impl Style {
    fn key(self) -> &'static str {
        match self {
            Style::Advise => "advise",
            Style::Discuss => "discuss",
            Style::Arena => "arena",
        }
    }
}

/// Consultants picked in the composer, and what happens once they've had their say.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Consult {
    pub consultants: Vec<Consultant>,
    pub style: Style,
    /// Then implement (else just report).
    pub implement: bool,
    /// Who judges an arena's designs (`None`: Trek picks, see `pick_judge`).
    pub judge: Option<Consultant>,
}

/// The family a model comes from ("anthropic", "openai", …), so an arena can draw designs from
/// different ones and have them judged by another than the agent's own. Models Trek can't place
/// count as their agent's own family.
pub fn family(agent: &AgentId, model: &str) -> String {
    known_family(agent, model).map(String::from).unwrap_or_else(|| agent.key())
}

/// The family Trek can tell `model` comes from: by its name, or as the only kind its agent runs.
fn known_family(agent: &AgentId, model: &str) -> Option<&'static str> {
    let m = model.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| m.contains(w));
    let named = if has(&["claude", "opus", "sonnet", "haiku"]) {
        Some("anthropic")
    } else if has(&["gpt", "codex", "openai/"]) || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        Some("openai")
    } else if has(&["gemini", "gemma"]) {
        Some("google")
    } else if has(&["grok"]) {
        Some("xai")
    } else if has(&["qwen"]) {
        Some("qwen")
    } else if has(&["kimi", "moonshot"]) {
        Some("moonshot")
    } else if has(&["deepseek"]) {
        Some("deepseek")
    } else if has(&["glm", "zhipu", "z-ai"]) {
        Some("zhipu")
    } else if has(&["llama"]) {
        Some("meta")
    } else if has(&["mistral", "devstral", "codestral"]) {
        Some("mistral")
    } else {
        None
    };
    named.or(match agent {
        AgentId::ClaudeCode => Some("anthropic"),
        AgentId::Codex => Some("openai"),
        _ => None,
    })
}

/// Whether `model` can design or judge in an arena: one that works on code itself, not a router
/// that picks a model of any family as it goes ("Auto") or one made for something else (deep
/// research, images, speech).
pub fn designs(agent: &AgentId, model: &ModelInfo) -> bool {
    let words = format!("{} {}", model.id, model.name).to_lowercase();
    let router = known_family(agent, &model.id).is_none() && ["auto", "default", "router"].iter().any(|r| model.id.eq_ignore_ascii_case(r) || model.name.eq_ignore_ascii_case(r));
    let other = ["research", "image", "embed", "audio", "realtime", "tts", "transcribe", "search"].iter().any(|w| words.contains(w));
    !(router || other)
}

/// A consultant on `model` of `agent` at High, or as near as the model goes.
fn at_high(agent: &AgentId, model: &ModelInfo) -> Consultant {
    let effort = if model.efforts.is_empty() { Effort::High } else { Effort::High.clamp_to(&model.efforts) };
    Consultant { agent: agent.clone(), model: model.id.clone(), effort }
}

/// Whether `model` is a small one, by its name: a few billion parameters ("llama-3.2-1b",
/// "gpt-oss-20b") or a nano/tiny model. Fine for quick jobs; an arena only takes one when there's
/// nothing else to draft a design or judge.
pub fn small(model: &ModelInfo) -> bool {
    let words = format!("{} {}", model.id, model.name).to_lowercase();
    let billions = words.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.')).filter_map(|w| w.strip_suffix('b')?.parse::<f32>().ok()).reduce(f32::max);
    billions.is_some_and(|b| b < 30.0) || words.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| matches!(w, "nano" | "tiny"))
}

/// An arena's candidates when none are picked: one per model family, from every model on offer
/// (`options`: each agent's default first, then its others, smartest first), as many as can run
/// at once. Families Trek knows come first (a model it can't place may well be one of them).
/// Models called straight through an API (often local or general-purpose) only make up an arena
/// that coding agents can't: two designs at least. With fewer than two families on offer, other
/// models of one make up the two; small models only when there's nothing else.
pub fn arena_defaults(options: &[(AgentId, ModelInfo)]) -> Vec<Consultant> {
    let mut seen: Vec<String> = vec![];
    let mut out: Vec<Consultant> = vec![];
    for (known, direct) in [(true, false), (true, true), (false, false), (false, true)] {
        let pass = |(a, m): &&(AgentId, ModelInfo)| designs(a, m) && !small(m) && known_family(a, &m.id).is_some() == known && matches!(a, AgentId::Direct(_)) == direct;
        for (agent, model) in options.iter().filter(pass) {
            let f = family(agent, &model.id);
            let room = if direct { 2 } else { MAX_RUNNING };
            if !seen.contains(&f) && out.len() < room {
                seen.push(f);
                out.push(at_high(agent, model));
            }
        }
    }
    let (large, tiny): (Vec<_>, Vec<_>) = options.iter().filter(|(a, m)| designs(a, m)).partition(|(_, m)| !small(m));
    for (agent, model) in large.into_iter().chain(tiny) {
        if out.len() >= 2 {
            break;
        }
        if !out.iter().any(|c| c.agent == *agent && c.model == model.id) {
            out.push(at_high(agent, model));
        }
    }
    out
}

/// Who can judge an arena run by an agent on `main` (agent, model): any model on offer that
/// works on code but that one, those of another family first.
pub fn judges(main: (&AgentId, &str), options: &[(AgentId, ModelInfo)]) -> Vec<(AgentId, ModelInfo)> {
    let own = family(main.0, main.1);
    let mut fit: Vec<(AgentId, ModelInfo)> = options.iter().filter(|(a, m)| designs(a, m) && !(a == main.0 && m.id == main.1)).cloned().collect();
    fit.sort_by_key(|(a, m)| family(a, &m.id) == own);
    fit
}

/// The judge of an arena run by an agent on `main` (agent, model), from `options` (every model on
/// offer, preferred first): a model of another family if there's one, else another model of the
/// same; one that isn't small, isn't a candidate and whose family Trek knows, as far as there's
/// a choice. `None` when `main` is the only model on offer.
pub fn pick_judge(main: (&AgentId, &str), candidates: &[Consultant], options: &[(AgentId, ModelInfo)]) -> Option<Consultant> {
    let own = family(main.0, main.1);
    let fresh = |a: &AgentId, m: &ModelInfo| !candidates.iter().any(|c| c.agent == *a && c.model == m.id);
    let best = judges(main, options).into_iter().min_by_key(|(a, m)| (family(a, &m.id) == own, small(m), !fresh(a, m), known_family(a, &m.id).is_none()));
    best.map(|(a, m)| at_high(&a, &m))
}

/// Whether an arena can run as picked: two designs at least, no more than run at once, and a
/// judge. Else what's missing, to tell the user.
pub fn arena_problem(consult: &Consult) -> Option<String> {
    if consult.style != Style::Arena {
        return None;
    }
    let n = consult.consultants.len();
    if n < 2 {
        Some("An arena needs two designs at least: pick another model to draft one".into())
    } else if n > MAX_RUNNING {
        Some(format!("An arena drafts {MAX_RUNNING} designs at most, all at once: drop {}", n - MAX_RUNNING))
    } else if consult.judge.is_none() {
        Some("An arena needs a judge on another model than the thread's, and there's none on offer".into())
    } else {
        None
    }
}

/// Rounds a discussion may take.
pub const DISCUSS_ROUNDS: usize = 3;

const CONSULT_OPEN: &str = "<trek-consult";
const CONSULT_CLOSE: &str = "</trek-consult>";

/// `text` with the instructions for consulting: which models to ask (named for the agent with
/// `name`, e.g. "Sol · High"), how, and what to do after. An arena without a judge has none
/// named: the agent is told to pick one of another family.
pub fn consult_prompt(text: &str, consult: &Consult, name: impl Fn(&Consultant) -> String) -> String {
    let with: Vec<String> = consult.consultants.iter().map(Consultant::key).collect();
    let then = if consult.implement { "implement" } else { "report" };
    let line = |c: &Consultant| format!("   - {}: agent \"{}\", model \"{}\", effort \"{}\"", name(c), c.agent.key(), c.model, c.effort.as_str());
    let list: Vec<String> = consult.consultants.iter().map(line).collect();
    let finish = if consult.implement { "then implement it." } else { "then stop there: don't change any files." };
    let mut steps = vec![];
    match consult.style {
        Style::Advise | Style::Discuss => steps.push(format!(
            "1. Call `delegate_task` once for each consultant below, all at once if you can, with mode \"advise\" and wait true:\n{}\n   They can't see this conversation: give each a self-contained brief (the request above, the files that matter, what you've found so far) and ask for their review and recommendations, not for edits.",
            list.join("\n")
        )),
        Style::Arena => {}
    }
    match consult.style {
        Style::Advise => {
            steps.push("2. Weigh their advice against your own judgment: the decision is yours.".into());
            steps.push(format!("3. Tell me in a few lines what each consultant recommended and what you decided, {finish}"));
        }
        Style::Discuss => {
            steps.push(format!(
                "2. Where you disagree with a consultant, or they disagree with each other, run another round: call `delegate_task` for them again with the brief, every position so far and the open objections. Stop once you agree, or after {DISCUSS_ROUNDS} rounds in all."
            ));
            steps.push(format!("3. Tell me in a few lines where you landed: what you agreed on (or what's still disputed, and your call), {finish}"));
        }
        Style::Arena => {
            let letters: Vec<String> = (0..consult.consultants.len()).map(|i| format!("\"Design {}\"", (b'A' + i as u8) as char)).collect();
            steps.push("1. Ground the problem before anyone designs: read the code it touches and write a short brief of what exists now, who owns what, the constraints, and how callers will use what's built. Don't design it yourself yet.".into());
            steps.push(format!(
                "2. Run the arena: call `delegate_task` once for each candidate below, all at once, with mode \"advise\", wait true, and titles {}:\n{}\n   Give each the same self-contained brief and ask for a design package: a sketch of the call sites (how callers will use it), the core types, the public function signatures, and a short rationale, written as code with placeholder bodies. Each designs on its own: tell none of them about the others or your own ideas. Ask each to weigh how deep the interface is (a simple interface over real functionality), how it fails, and what a weaker model working with it would get wrong.",
                letters.join(", "),
                list.join("\n")
            ));
            let judge = match &consult.judge {
                Some(j) => format!("the judge below, with mode \"advise\", wait true and the title \"Judge the designs\":\n{}", line(j)),
                None => "a judge on another model than yours, of another family if there's one (`list_models` has them), with mode \"advise\", wait true and the title \"Judge the designs\"".into(),
            };
            steps.push(format!(
                "3. Cross-judge: call `delegate_task` for {judge}\n   Give it the brief and every design package, labelled by letter without saying which model wrote which, and ask it to score each from 1 to 5 on fit with the call sites, depth of the interface, simplicity, how it fails, and fit with the codebase as it is; then to name the strongest and what each of the others does better."
            ));
            let then = if consult.implement {
                "then implement against the sketch. If the code shows the sketch is wrong (the same workaround at unrelated call sites, or types that need escape hatches), stop and tell me rather than forcing it."
            } else {
                "then stop there: don't change any files."
            };
            steps.push(format!(
                "4. Synthesise: start from the strongest design and fold in what the others do better. Tell me in a few lines how the designs scored, which won and why, and show the final sketch (call sites, types, signatures), {then}"
            ));
        }
    }
    let judge = consult.judge.as_ref().filter(|_| consult.style == Style::Arena).map(|j| format!(" judge=\"{}\"", j.key())).unwrap_or_default();
    let lead = match consult.style {
        Style::Arena => "Before you build this, run a design arena through Trek's `delegate_task` tool:",
        _ => "Before you act on this, consult other models through Trek's `delegate_task` tool:",
    };
    format!(
        "{}\n\n{CONSULT_OPEN} style=\"{}\" then=\"{then}\" with=\"{}\"{judge}>\n{lead}\n\n{}\n\nIf `delegate_task` isn't available to you, say so and stop.\n{CONSULT_CLOSE}",
        text.trim_end(),
        consult.style.key(),
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
    let style = match attr("style") {
        Some("discuss") => Style::Discuss,
        Some("arena") => Style::Arena,
        _ => Style::Advise,
    };
    let consult = Consult { consultants, style, implement: attr("then") != Some("report"), judge: attr("judge").and_then(Consultant::from_key).filter(|_| style == Style::Arena) };
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
        assert_eq!(plain_preview("| Design | Score |\n| --- | :---: |\n| A | 4.6 |\n\nA wins.", 100), "Design · Score A · 4.6 A wins.");
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
        assert!(is_delegate_call("trek-orchestrate_delegate_task"), "OpenCode's naming");
        assert!(!is_delegate_call("delegate_task"), "a bare name could be any server's");
        assert!(!is_delegate_call("mcp__t3-code__delegate_task"), "another server's tool of the same name");
        assert!(!is_delegate_call("Subagent"));
        assert_eq!(tool_label("mcp__trek-orchestrate__task_status"), Some("Checked on a sub-agent"));
        assert_eq!(tool_label("trek-orchestrate_cancel_task"), Some("Stopped a sub-agent"));
        assert_eq!(tool_label("cancel_task"), None);
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
            judge: None,
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

    fn model(id: &str) -> ModelInfo {
        ModelInfo::new(id, id, 0, &[Effort::Low, Effort::Medium, Effort::High])
    }

    #[test]
    fn models_fall_into_families() {
        assert_eq!(family(&AgentId::ClaudeCode, "claude-opus-5-5"), "anthropic");
        assert_eq!(family(&AgentId::ClaudeCode, "default"), "anthropic", "an agent's own models");
        assert_eq!(family(&AgentId::Codex, "gpt-5.6-sol"), "openai");
        assert_eq!(family(&AgentId::OpenCode, "opencode/grok-code-fast"), "xai");
        assert_eq!(family(&AgentId::Acp("github-copilot".into()), "anthropic/claude-sonnet-5"), "anthropic");
        assert_eq!(family(&AgentId::Acp("github-copilot".into()), "gemini-3-pro"), "google");
        assert_eq!(family(&AgentId::Direct("mock".into()), "mock-swift"), "direct:mock", "unknown: the agent's");
    }

    #[test]
    fn an_arena_draws_one_design_per_family_and_a_judge_from_another() {
        let defaults = vec![
            (AgentId::ClaudeCode, model("claude-opus-5-5")),
            (AgentId::Codex, model("gpt-5.6-sol")),
            (AgentId::Acp("github-copilot".into()), model("claude-sonnet-5")),
            (AgentId::OpenCode, model("grok-code-fast")),
        ];
        let picked = arena_defaults(&defaults);
        let keys: Vec<String> = picked.iter().map(Consultant::key).collect();
        assert_eq!(keys, ["claude-code/claude-opus-5-5/high", "codex/gpt-5.6-sol/high", "opencode/grok-code-fast/high"], "Copilot's Claude is the same family");
        // Every model on offer counts, not only defaults; a router (Copilot's Auto) and a model
        // made for research don't design, and models Trek can't place come last.
        let copilot = AgentId::Acp("github-copilot".into());
        let options = vec![
            (AgentId::ClaudeCode, model("claude-opus-5-5")),
            (AgentId::Codex, model("gpt-6-astra")),
            (AgentId::Acp("google".into()), ModelInfo::new("deep-research-max-preview", "Google/Deep Research Max Preview", 0, &[])),
            (copilot.clone(), ModelInfo::new("auto", "Auto", 0, &[])),
            (AgentId::Direct("local".into()), model("house-coder")),
            (AgentId::Direct("openrouter".into()), model("meta-llama/llama-3.2-1b-instruct")),
            (copilot.clone(), model("claude-sonnet-5")),
            (copilot.clone(), model("gemini-3-pro")),
            (AgentId::OpenCode, model("opencode/grok-4.6")),
        ];
        let keys: Vec<String> = arena_defaults(&options).iter().map(Consultant::key).collect();
        assert_eq!(keys, ["claude-code/claude-opus-5-5/high", "codex/gpt-6-astra/high", "acp:github-copilot/gemini-3-pro/high", "opencode/opencode/grok-4.6/high"]);
        // An API model of a family of its own only makes up an arena of fewer than two.
        let keys: Vec<String> = arena_defaults(&options[..6]).iter().map(Consultant::key).collect();
        assert_eq!(keys, ["claude-code/claude-opus-5-5/high", "codex/gpt-6-astra/high"]);
        // A small model is no designer: an API model of a family of its own makes up two.
        let keys: Vec<String> = arena_defaults(&options[1..6]).iter().map(Consultant::key).collect();
        assert_eq!(keys, ["codex/gpt-6-astra/high", "direct:local/house-coder/high"]);
        let opencode_llama = (AgentId::OpenCode, ModelInfo::new("openrouter/meta-llama/llama-3.2-1b-instruct", "OpenRouter/Llama 3.2 1B Instruct", 0, &[]));
        let mut with_llama = options.clone();
        with_llama.insert(2, opencode_llama.clone());
        let keys: Vec<String> = arena_defaults(&with_llama).iter().map(Consultant::key).collect();
        assert_eq!(keys, ["claude-code/claude-opus-5-5/high", "codex/gpt-6-astra/high", "acp:github-copilot/gemini-3-pro/high", "opencode/opencode/grok-4.6/high"], "not the 1B model");
        assert!(small(&opencode_llama.1) && small(&model("gpt-oss-20b")) && small(&model("gpt-5-nano")));
        assert!(!small(&model("qwen3-coder-480b-a35b")) && !small(&model("qwen3-30b-a3b")) && !small(&model("gpt-5.6-luna")) && !small(&model("claude-opus-5-5")));
        let opus = (&AgentId::ClaudeCode, "claude-opus-5-5");
        assert_eq!(pick_judge(opus, &[], &options[3..5]).map(|j| j.key()).as_deref(), Some("direct:local/house-coder/high"), "never the router");
        // The judge: another family than the main agent's, not a candidate when there's a choice.
        let options = vec![(AgentId::ClaudeCode, model("claude-opus-5-5")), (AgentId::Codex, model("gpt-5.6-sol")), (AgentId::Codex, model("gpt-5.6-luna"))];
        let judge = pick_judge(opus, &picked, &options).unwrap();
        assert_eq!(judge.key(), "codex/gpt-5.6-luna/high");
        let judge = pick_judge(opus, &picked, &options[..2]).unwrap();
        assert_eq!(judge.key(), "codex/gpt-5.6-sol/high", "a candidate, when nothing else is of another family");
        assert_eq!(pick_judge(opus, &picked, &options[..1]), None, "the thread's own model is all there is");
        // One family on offer: two of its models design, and a third (else the other) judges.
        let claude = vec![(AgentId::ClaudeCode, model("claude-opus-5-5")), (AgentId::ClaudeCode, model("claude-sonnet-5-5")), (AgentId::ClaudeCode, model("claude-haiku-4-5"))];
        let picked = arena_defaults(&claude);
        let keys: Vec<String> = picked.iter().map(Consultant::key).collect();
        assert_eq!(keys, ["claude-code/claude-opus-5-5/high", "claude-code/claude-sonnet-5-5/high"]);
        assert_eq!(pick_judge(opus, &picked, &claude).map(|j| j.key()).as_deref(), Some("claude-code/claude-haiku-4-5/high"));
        assert_eq!(pick_judge(opus, &picked, &claude[..2]).map(|j| j.key()).as_deref(), Some("claude-code/claude-sonnet-5-5/high"));
        let judges: Vec<String> = judges(opus, &[claude[1].clone(), (AgentId::Codex, model("gpt-5.6-sol")), claude[0].clone()]).into_iter().map(|(_, m)| m.id).collect();
        assert_eq!(judges, ["gpt-5.6-sol", "claude-sonnet-5-5"], "another family first; never the thread's own model");
        // Efforts a model doesn't take are clamped.
        let low_only = ModelInfo::new("glm-5", "GLM 5", 0, &[Effort::Low]);
        assert_eq!(arena_defaults(&[(AgentId::OpenCode, low_only)])[0].effort, Effort::Low);
    }

    #[test]
    fn an_arena_needs_two_designs_no_more_than_run_at_once_and_a_judge() {
        let c = |m: &str| Consultant { agent: AgentId::Codex, model: m.into(), effort: Effort::High };
        let mut arena = Consult { consultants: vec![c("a")], style: Style::Arena, implement: true, judge: Some(c("j")) };
        assert!(arena_problem(&arena).is_some_and(|p| p.contains("two designs")));
        assert_eq!(arena_problem(&Consult { style: Style::Advise, ..arena.clone() }), None, "only an arena");
        arena.consultants = ["a", "b", "c", "d", "e"].map(c).to_vec();
        assert!(arena_problem(&arena).is_some_and(|p| p.contains("drop 1")));
        arena.consultants.truncate(2);
        assert_eq!(arena_problem(&arena), None);
        arena.judge = None;
        assert!(arena_problem(&arena).is_some_and(|p| p.contains("judge")));
    }

    #[test]
    fn arena_instructions_ground_sketch_judge_and_synthesise() {
        let candidates = vec![
            Consultant { agent: AgentId::Codex, model: "gpt-5.6-sol".into(), effort: Effort::High },
            Consultant { agent: AgentId::ClaudeCode, model: "claude-opus-5-5".into(), effort: Effort::High },
        ];
        let judge = Consultant { agent: AgentId::OpenCode, model: "grok-code-fast".into(), effort: Effort::High };
        let consult = Consult { consultants: candidates, style: Style::Arena, implement: true, judge: Some(judge.clone()) };
        let text = consult_prompt("Add rate limiting to webhooks", &consult, |c| c.model.clone());
        for step in ["1. Ground the problem", "2. Run the arena", "titles \"Design A\", \"Design B\"", "3. Cross-judge", "\"Judge the designs\"", "4. Synthesise", "implement against the sketch"] {
            assert!(text.contains(step), "{step}: {text}");
        }
        assert!(text.contains("grok-code-fast: agent \"opencode\", model \"grok-code-fast\""), "the judge is named");
        let (said, back) = split_consult(&text);
        assert_eq!(said, "Add rate limiting to webhooks");
        assert_eq!(back, Some(consult.clone()));
        // No judge named: the agent picks one of another family; report only.
        let open = Consult { judge: None, implement: false, ..consult };
        let text = consult_prompt("x", &open, |c| c.model.clone());
        assert!(text.contains("another model than yours, of another family if there's one") && text.contains("don't change any files"), "{text}");
        assert_eq!(split_consult(&text).1, Some(open));
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
