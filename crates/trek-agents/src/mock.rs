//! A scripted agent with no process and no network, for demos, UI tests and performance work.
//! Each prompt plays a realistic event stream chosen by a keyword in it:
//!
//! | keyword                      | plays                                                        |
//! | ---------------------------- | ------------------------------------------------------------ |
//! | (none)                       | an answer streamed token by token: markdown, code, paths     |
//! | `tools`                      | commands, reads, a search and an edit, with outputs          |
//! | `agents` / `subagents` [dur] | two background sub-agents that report back `dur` later        |
//! | `mock:task` [dur]            | a sub-agent the turn waits on (in the foreground) for `dur`   |
//! | `permission`                 | a command that needs approval                                |
//! | `question`                   | multiple-choice questions                                     |
//! | `plan` (or plan mode)        | a plan to approve before any change                          |
//! | `mock:long` [dur]            | a long build that runs `dur` (default 30s)                   |
//! | `mock:stream` [dur]          | one long answer streamed for `dur` (default 30s)             |
//! | `mock:prose`                 | a long answer using all of markdown: headings, bold, lists,  |
//! |                              | a table, code, paths and a link (for reviewing typography)   |
//! | `mock:explore` [dur]         | tools at a steady pace for `dur` (default 30s): reads, finds, |
//! |                              | commands, edits and web lookups, in groups between messages  |
//! | `error`                      | a turn that fails                                             |
//! | `mock:limit` [dur]           | a usage limit that resets `dur` from now (default 5s)        |
//! | `mock:write`                 | adds a line to `NOTES.md` in the session's folder (for real)  |
//! | `recall`                     | the messages it remembers from this conversation             |
//! | `mock:consult` [prompt]      | asks a mock sub-agent (Trek's `delegate_task`) and waits      |
//! | `mock:delegate` [prompt]     | starts a mock sub-agent and ends its turn; Trek wakes it      |
//! | `mock:pair` [prompt]         | starts two mock sub-agents on the same task and ends its turn |
//! | `mock:server` [dur]          | leaves a dev server running in the background (for `dur`, or  |
//! |                              | until stopped), printing a line now and then                  |
//! | `mock:watch` [dur]           | leaves a test watcher running; `dur` later (default 3s) it    |
//! |                              | reports a failure and the agent takes a turn of its own       |
//! | `mock:dev`                   | leaves a dev server and a quiet test watcher running          |
//! | `mock:cost` [`plan`]         | a turn on Claude Sonnet 5.5 at its API price, billed per      |
//! |                              | token (or, with `plan`, on a Claude Max plan)                 |
//! | `mock:verify`                | runs the project's verification CLI, as Trek told it          |
//! | `mock:design` / `mock:judge` | an arena's design package, and a judge's scores               |
//!
//! Messages Trek dresses up are played as asked, whatever their words: a request to restate
//! (`trek_core::restate`) gets a restatement, an arena (`Style::Arena`) is run with the
//! candidates and judge it names, and Trek's verification guides (`create-` and
//! `maintain-verification-skill`) set up, or touch up, a small verification skill in the
//! session's folder (for real).
//!
//! A sub-agent's prompt is whatever follows the keyword, so `mock:consult mock:long 2s` starts
//! one that works for two seconds (and `mock:consult mock:consult hi` one that consults in turn).
//! A message Trek sends to wake it with a sub-agent's answer gets a short reply.
//!
//! Keywords may be written bare or as `mock:<keyword>`; durations look like `500ms`, `30s`, `2m`.
//! Every turn also reports context and token usage (on the mock's own models, which have no
//! price). A prompt sent mid-turn steers it. Once a
//! session has hit its limit, every prompt hits it again until it resets (or `lift_limit`).
//!
//! Like Claude Code and Codex, it keeps each session's history (in memory, for the process) and
//! can resume one partway or fork it (`SessionConfig::resume_at`, `fork`); its `mock-recap`
//! model can't, as ACP agents can't, so rewinds give it a recap instead.
//!
//! Selected by `AgentId::Direct("mock")`. Trek offers it only when `TREK_MOCK_AGENT=1` (and in
//! its own tests).

use crate::{AgentEvent, BackgroundKind, BackgroundTask, Billing, Command, Decision, Prompt, Question, SessionConfig};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use trek_core::HandHolding;

pub use trek_core::catalog::MOCK_PROVIDER as PROVIDER;

/// The mock's model that can't resume a session partway (see `crate::resumes_partway`).
pub const RECAP_MODEL: &str = "mock-recap";

/// Every mock session's history, by session id: each message with the mark of the turn it
/// belongs to, oldest first. The mock's stand-in for an agent's session files.
static HISTORY: LazyLock<Mutex<HashMap<String, Vec<(String, String)>>>> = LazyLock::new(Default::default);

fn new_id(kind: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("mock-{kind}-{nanos:x}-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Sessions at their usage limit: session id → when it resets (unix ms).
static LIMITS: LazyLock<Mutex<HashMap<String, i64>>> = LazyLock::new(Default::default);

/// Lift session `id`'s usage limit now, as if it had reset (tests run on a clock of their own).
pub fn lift_limit(id: &str) {
    LIMITS.lock().unwrap().remove(id);
}

/// The messages the mock remembers in session `id`, oldest first.
pub fn remembered(id: &str) -> Vec<String> {
    HISTORY.lock().unwrap().get(id).map(|h| h.iter().map(|(_, m)| m.clone()).collect()).unwrap_or_default()
}

/// The mark of the last turn in session `id` (see `crate::session_tail`).
pub fn last_mark(id: &str) -> Option<String> {
    HISTORY.lock().unwrap().get(id)?.last().map(|(mark, _)| mark.clone())
}

/// A message that came with a recap of the conversation: the messages the recap holds, and the
/// message itself.
fn split_recap(text: &str) -> (Vec<String>, &str) {
    let Some((head, message)) = text.split_once("</recap>") else { return (vec![], text) };
    let recap = head.split_once("<recap>").map_or("", |(_, r)| r);
    let said = recap.split("\n\n").filter_map(|e| e.trim().strip_prefix("User: ")).map(|e| e.trim().to_string()).collect();
    (said, message.trim_start())
}

/// The mock shows up in pickers when `TREK_MOCK_AGENT=1`.
pub fn enabled() -> bool {
    std::env::var("TREK_MOCK_AGENT").is_ok_and(|v| v == "1")
}

/// Pace in thousandths: 1000 plays at demo speed, 0 plays the script without delays.
static PACE: AtomicU32 = AtomicU32::new(1000);

/// Scale the script's built-in delays (`1.0` = demo speed, `0.0` = none). Durations written in
/// the prompt (`mock:long 30s`) are kept as written.
pub fn set_pace(pace: f32) {
    PACE.store((pace.max(0.) * 1000.) as u32, Ordering::Relaxed);
}

fn paced(ms: u64) -> Duration {
    Duration::from_millis(ms * PACE.load(Ordering::Relaxed) as u64 / 1000)
}

const WINDOW: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Script {
    Answer,
    Tools,
    Agents(Option<Duration>),
    /// A sub-agent of its own that the turn waits on, working for a while.
    Task(Duration),
    Permission,
    Questions,
    Plan,
    Long(Duration),
    Stream(Duration),
    /// A long answer with every kind of block, for reviewing how answers read.
    Prose,
    Explore(Duration),
    Error,
    Write,
    Recall,
    Limit(Duration),
    /// Start a sub-agent through Trek's orchestration tools: waiting for its answer, or not.
    Delegate { wait: bool },
    /// Start two sub-agents on the same task, and end the turn.
    Pair,
    /// Leave a dev server running in the background, for a while or until stopped.
    Server(Option<Duration>),
    /// Leave a test watcher running that reports a failure after a while.
    Watch(Duration),
    /// Leave a dev server and a test watcher running (one that catches nothing for a day).
    Dev,
    /// A turn on a real model with a real price: billed per token, or on a plan.
    Cost { plan: bool },
    /// Trek woke it with what a sub-agent came back with.
    Wake,
    /// Asked to restate the request in its own words, and stop.
    Restate,
    /// A design arena: candidates, then a judge, through Trek's orchestration tools.
    Arena,
    /// A design package for an arena.
    Design,
    /// Scores for an arena's designs.
    Judge,
    /// Build the project's verification skill (Trek's `create-verification-skill` guide).
    SetupVerification,
    /// Bring it up to date (`maintain-verification-skill`).
    MaintainVerification,
    /// Run the verification CLI Trek named.
    Verify,
}

impl Script {
    /// The script a prompt asks for. `plan` is the session's plan mode: every turn ends in a plan.
    fn parse(text: &str, plan: bool) -> Script {
        // Before any keyword: a sub-agent's answer may mention "error" or "plan".
        if trek_core::orchestrate::is_wake(text) {
            return Script::Wake;
        }
        // What Trek added to a message decides, before the words in it.
        if trek_core::restate::split_restate(trek_core::orchestrate::split_consult(text).0).1 {
            return Script::Restate;
        }
        if trek_core::orchestrate::split_consult(text).1.is_some_and(|c| c.style == trek_core::orchestrate::Style::Arena) {
            return Script::Arena;
        }
        if text.contains(trek_core::skills::CREATE_VERIFICATION) {
            return Script::SetupVerification;
        }
        if text.contains(trek_core::skills::MAINTAIN_VERIFICATION) {
            return Script::MaintainVerification;
        }
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != ':' && c != '-').to_lowercase())
            .collect();
        let duration_after = |i: usize| words.get(i + 1).and_then(|w| parse_duration(w));
        let found = words.iter().enumerate().find_map(|(i, w)| {
            let key = w.strip_prefix("mock:").unwrap_or(w);
            Some(match key {
                // Bare "long" and "stream" are too common to start a 30-second turn.
                "long" if w.starts_with("mock:") => Script::Long(duration_after(i).unwrap_or(Duration::from_secs(30))),
                "stream" if w.starts_with("mock:") => Script::Stream(duration_after(i).unwrap_or(Duration::from_secs(30))),
                "prose" if w.starts_with("mock:") => Script::Prose,
                "explore" if w.starts_with("mock:") => Script::Explore(duration_after(i).unwrap_or(Duration::from_secs(30))),
                // The one script that changes files: only when asked for by its full name.
                "write" if w.starts_with("mock:") => Script::Write,
                "limit" if w.starts_with("mock:") => Script::Limit(duration_after(i).unwrap_or(Duration::from_secs(5))),
                "consult" if w.starts_with("mock:") => Script::Delegate { wait: true },
                "delegate" if w.starts_with("mock:") => Script::Delegate { wait: false },
                "pair" if w.starts_with("mock:") => Script::Pair,
                "task" if w.starts_with("mock:") => Script::Task(duration_after(i).unwrap_or(Duration::from_secs(6))),
                "server" if w.starts_with("mock:") => Script::Server(duration_after(i)),
                "watch" if w.starts_with("mock:") => Script::Watch(duration_after(i).unwrap_or(Duration::from_secs(3))),
                "dev" if w.starts_with("mock:") => Script::Dev,
                "cost" if w.starts_with("mock:") => Script::Cost { plan: words.get(i + 1).is_some_and(|w| w == "plan") },
                "verify" if w.starts_with("mock:") => Script::Verify,
                "design" if w.starts_with("mock:") => Script::Design,
                "judge" if w.starts_with("mock:") => Script::Judge,
                "error" => Script::Error,
                "permission" => Script::Permission,
                "question" | "questions" => Script::Questions,
                "plan" => Script::Plan,
                "agents" | "subagents" | "sub-agents" => Script::Agents(duration_after(i)),
                "tools" => Script::Tools,
                "recall" => Script::Recall,
                _ => return None,
            })
        });
        match found {
            Some(s) => s,
            None if plan => Script::Plan,
            None => Script::Answer,
        }
    }
}

/// The title a thread gets after its first turn: the mock names it after its script instead of
/// asking a model.
pub fn title(request: &str) -> String {
    match Script::parse(request, false) {
        Script::Answer => "How the app starts",
        Script::Tools => "Add a verbose flag",
        Script::Agents(_) => "Scout the routes and error handling",
        Script::Task(_) => "Survey the test suite",
        Script::Permission => "Apply the schema migrations",
        Script::Questions => "Choose a database",
        Script::Plan => "Require a session on every route",
        Script::Long(_) => "Run the full test suite",
        Script::Stream(_) => "Walk through the codebase",
        Script::Prose => "Tour the startup code",
        Script::Explore(_) => "Animate the title as it appears",
        Script::Error => "Fix the failing build",
        Script::Write => "Add a note",
        Script::Recall => "What was said",
        Script::Limit(_) => "Refactor the parser",
        Script::Delegate { .. } => "Get a second opinion",
        Script::Pair => "Get two opinions",
        Script::Server(_) => "Start the dev server",
        Script::Watch(_) => "Watch the tests",
        Script::Dev => "Run the app while we work",
        Script::Cost { .. } => "Price the API calls",
        Script::Wake => "A sub-agent reported back",
        Script::Restate => "Say it back first",
        Script::Arena | Script::Design | Script::Judge => "Design the rate limiter",
        Script::SetupVerification => "Set up verification",
        Script::MaintainVerification => "Maintain verification",
        Script::Verify => "Verify the change",
    }
    .into()
}

/// The longest duration a prompt can ask for.
const MAX_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// `500ms`, `30s`, `2m`, at most a day.
fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.find(|c: char| !c.is_ascii_digit()).map(|i| s.split_at(i))?;
    let n: u64 = num.parse().ok()?;
    let d = match unit {
        "ms" => Duration::from_millis(n),
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n.saturating_mul(60)),
        _ => return None,
    };
    Some(d.min(MAX_DURATION))
}

/// Why a turn stopped early.
enum Stop {
    Interrupted,
    /// Shut down, or Trek stopped listening.
    Closed,
}

type Step<T = ()> = std::result::Result<T, Stop>;

/// What the user said to a pending request.
enum Reply {
    Decision(Decision),
    Answers(Vec<(String, String)>),
}

struct Session {
    /// The session's folder (`mock:write` writes there).
    cwd: std::path::PathBuf,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
    hand_holding: HandHolding,
    plan: bool,
    /// "Always allow" was chosen for commands this session.
    commands_allowed: bool,
    context: u64,
    next_id: u64,
    /// Prompts sent while a turn ran; the turn acknowledges them at its next step.
    steer: Vec<String>,
    /// Work left running between turns: background sub-agents and shells. Shared with the tasks
    /// that play the shells, which run on while the session waits for its next message.
    background: Jobs,
    /// What background work reported that makes the agent take a turn of its own.
    notes: async_channel::Receiver<Note>,
    note: async_channel::Sender<Note>,
    /// The session's id, under which its history is kept.
    native_id: String,
    /// The mark of the turn under way: messages sent during it share it.
    mark: String,
    /// The model it plays, named in its token reports.
    model: String,
    /// The way to Trek's orchestration tools, when Trek gave the session them.
    orchestrate: Option<trek_ipc::Client>,
    /// What Trek told it about the project (`SessionConfig::instructions`).
    instructions: Option<String>,
}

/// Background work's news, for a turn the agent takes on its own (as Claude Code does on a
/// task's notification).
enum Note {
    /// The test watcher caught a failure.
    Watcher(String),
    /// The scouts (`agents`) are back.
    Scouts,
    /// A sub-agent of its own was stopped on request (Claude Code takes a turn to say so).
    Stopped(String),
}

/// Work running in the background of a session.
#[derive(Clone, Default)]
struct Jobs(Arc<Mutex<Vec<Job>>>);

struct Job {
    task: BackgroundTask,
    /// What it has printed so far.
    output: String,
}

impl Jobs {
    fn add(&self, task: BackgroundTask) {
        self.0.lock().unwrap().push(Job { task, output: String::new() });
    }

    /// Take it off the list, with what it was (`None`: stopped already).
    fn take(&self, id: &str) -> Option<BackgroundTask> {
        let mut jobs = self.0.lock().unwrap();
        let ix = jobs.iter().position(|j| j.task.id == id)?;
        Some(jobs.remove(ix).task)
    }

    /// Take it off the list; `false` when it wasn't on it (stopped already).
    fn remove(&self, id: &str) -> bool {
        let mut jobs = self.0.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.task.id != id);
        jobs.len() != before
    }

    fn running(&self, id: &str) -> bool {
        self.0.lock().unwrap().iter().any(|j| j.task.id == id)
    }

    fn print(&self, id: &str, line: &str) {
        if let Some(j) = self.0.lock().unwrap().iter_mut().find(|j| j.task.id == id) {
            j.output.push_str(line);
            j.output.push('\n');
        }
    }

    fn output(&self, id: &str) -> Option<String> {
        self.0.lock().unwrap().iter().find(|j| j.task.id == id && j.task.readable).map(|j| j.output.clone())
    }

    fn event(&self) -> AgentEvent {
        AgentEvent::Background(self.0.lock().unwrap().iter().map(|j| j.task.clone()).collect())
    }
}

/// Ends a session's background work when the session ends.
struct EndJobs(Jobs);

impl Drop for EndJobs {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.0.0.lock() {
            jobs.clear();
        }
    }
}

/// The name Trek gives its orchestration MCP server in a session.
pub const ORCHESTRATE_SERVER: &str = trek_core::orchestrate::SERVER;

pub async fn run(
    config: SessionConfig,
    commands: async_channel::Receiver<Command>,
    events: async_channel::Sender<AgentEvent>,
) -> Result<()> {
    let (native_id, resumed_at, recap) = open_session(&config);
    let commands = match recap {
        Some(r) => crate::recap_first(commands, r),
        None => commands,
    };
    let (note, notes) = async_channel::unbounded();
    let mut s = Session {
        cwd: config.cwd.clone(),
        commands,
        events,
        hand_holding: config.hand_holding,
        plan: config.plan,
        commands_allowed: false,
        context: 9_400,
        next_id: 0,
        steer: vec![],
        background: Jobs::default(),
        notes,
        note,
        native_id: native_id.clone(),
        mark: String::new(),
        model: config.model.clone().unwrap_or_else(|| "mock-swift".into()),
        orchestrate: config.mcp_servers.iter().find(|m| m.name == ORCHESTRATE_SERVER).and_then(|m| trek_ipc::Client::from_pairs(&m.env)),
        instructions: config.instructions.clone(),
    };
    // What it left running ends with it, however it ends: the loops printing for its shells
    // stop once they're off the list.
    let _jobs = EndJobs(s.background.clone());
    if s.emit(AgentEvent::Started { native_id, model: Some(s.model.clone()) }).await.is_err() {
        return Ok(());
    }
    if let Some(at) = resumed_at {
        let _ = s.emit(AgentEvent::Mark(at)).await;
    }
    // Nothing leaves the Mac, so nothing is billed.
    let _ = s.emit(AgentEvent::Billing(Billing::Local)).await;
    let _ = s.emit(AgentEvent::Context { used: s.context, window: WINDOW }).await;
    loop {
        // Between turns: the user's next message, or a background shell that has something to say.
        let cmd = tokio::select! {
            cmd = s.commands.recv() => cmd,
            Ok(note) = s.notes.recv() => {
                if s.woken(&note).await.is_err() {
                    break;
                }
                continue;
            }
        };
        let Ok(cmd) = cmd else { break };
        match cmd {
            Command::Prompt { text, .. } => {
                if s.turn(&text).await.is_err() {
                    break;
                }
            }
            Command::SetHandHolding(h) => s.hand_holding = h,
            Command::SetModel { model, .. } => s.model = model,
            Command::ReadTask { id } => s.read_task(&id),
            Command::StopTask { id } => s.stop_task(&id),
            Command::Shutdown => break,
            // No turn is running and nothing is pending between turns.
            Command::Interrupt | Command::Respond { .. } | Command::Answer { .. } => {}
        }
    }
    Ok(())
}

/// The session `config` asks for: its id, the point it was resumed at (if partway), and the
/// recap to use because it couldn't be (the history is gone, or the point isn't in it).
fn open_session(config: &SessionConfig) -> (String, Option<String>, Option<String>) {
    let mut sessions = HISTORY.lock().unwrap();
    let Some(from) = &config.resume else {
        let id = new_id("session");
        sessions.insert(id.clone(), vec![]);
        // A new session: the recap reaches it with the first message (`crate::start`).
        return (id, None, None);
    };
    let mut history = sessions.get(from).cloned().unwrap_or_default();
    let mut recap = None;
    if let Some(at) = &config.resume_at {
        match history.iter().rposition(|(mark, _)| mark == at) {
            Some(ix) => history.truncate(ix + 1),
            None => {
                history.clear();
                recap = config.recap.clone();
            }
        }
    }
    let id = if config.fork || recap.is_some() { new_id("session") } else { from.clone() };
    sessions.insert(id.clone(), history);
    let at = config.resume_at.clone().filter(|_| recap.is_none());
    (id, at, recap)
}

impl Session {
    async fn emit(&self, ev: AgentEvent) -> Step {
        self.events.send(ev).await.map_err(|_| Stop::Closed)
    }

    fn id(&mut self, kind: &str) -> String {
        self.next_id += 1;
        format!("mock-{kind}-{}", self.next_id)
    }

    /// Keep a message in the session's history, with the turn under way.
    fn remember(&self, text: &str) {
        let (recapped, message) = split_recap(text);
        let mut sessions = HISTORY.lock().unwrap();
        let history = sessions.entry(self.native_id.clone()).or_default();
        history.extend(recapped.into_iter().map(|m| ("recap".to_string(), m)));
        history.push((self.mark.clone(), message.to_string()));
    }

    /// One prompt, start to finish (including an early stop). Errs only when the session is over.
    async fn turn(&mut self, text: &str) -> Step {
        self.mark = new_id("turn");
        self.remember(text);
        // The turn is in the history now: cutting the session back to its mark keeps it whole.
        self.emit(AgentEvent::Mark(self.mark.clone())).await?;
        let text = split_recap(text).1.to_string();
        let text = text.as_str();
        self.context += 1_200 + text.len() as u64 / 3;
        self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await?;
        let mut script = Script::parse(text, self.plan);
        // Until the limit resets, every prompt meets it.
        if let Some(until) = LIMITS.lock().unwrap().get(&self.native_id).copied().filter(|u| *u > trek_core::store::now_ms()) {
            script = Script::Limit(Duration::from_millis((until - trek_core::store::now_ms()) as u64));
        }
        match self.play(script, text).await {
            Ok(()) => {}
            Err(Stop::Interrupted) => {
                self.steer.clear();
                self.emit(AgentEvent::TurnComplete { error: Some("Interrupted".into()) }).await?;
            }
            Err(Stop::Closed) => return Err(Stop::Closed),
        }
        self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await
    }

    async fn play(&mut self, script: Script, text: &str) -> Step {
        match script {
            Script::Delegate { wait } => self.delegate(text, wait).await?,
            Script::Pair => self.pair(text).await?,
            Script::Server(until) => self.server(until).await?,
            Script::Watch(after) => self.watch(after).await?,
            Script::Dev => {
                self.server(None).await?;
                self.watch(MAX_DURATION).await?
            }
            Script::Wake => {
                let body = text.split_once(":\n\n").map_or(text, |(_, b)| b);
                let gist = trek_core::orchestrate::preview(body.split("\n\n---\n\n").next().unwrap_or(body), 160);
                self.say(&format!("The sub-agent reported back: {gist}")).await?;
            }
            Script::Answer => {
                self.think("The user wants an overview. I'll keep it short and point at the files that matter.").await?;
                self.say(ANSWER).await?;
            }
            Script::Tools => self.tools().await?,
            Script::Agents(after) => return self.agents(after).await,
            Script::Task(total) => self.task(total).await?,
            Script::Permission => self.permission().await?,
            Script::Questions => self.questions().await?,
            Script::Plan => self.plan_turn().await?,
            Script::Long(total) => self.long(total).await?,
            Script::Stream(total) => self.stream(total).await?,
            Script::Prose => {
                self.think(PROSE_THOUGHT).await?;
                // At reading pace, so the answer can be watched (and captured) as it grows.
                self.say_at(PROSE, 45).await?;
            }
            Script::Explore(total) => self.explore(total).await?,
            Script::Write => self.write().await?,
            Script::Recall => {
                let mut said = remembered(&self.native_id);
                said.pop();
                let text = if said.is_empty() { "I don't remember anything from before this message.".to_string() } else { format!("I remember: {}", said.join(" | ")) };
                self.say(&text).await?;
            }
            Script::Limit(after) => return self.limit(after).await,
            Script::Cost { plan } => return self.priced_turn(plan).await,
            Script::Restate => self.restate(text).await?,
            Script::Arena => self.arena(text).await?,
            Script::Design => self.design().await?,
            Script::Judge => self.judge(text).await?,
            Script::SetupVerification => self.setup_verification().await?,
            Script::MaintainVerification => self.maintain_verification().await?,
            Script::Verify => self.verify().await?,
            Script::Error => {
                self.think("Let me check the build first.").await?;
                let id = self.id("tool");
                self.tool_start(&id, "Run command", "cargo build").await?;
                self.pause(paced(400)).await?;
                self.emit(AgentEvent::ToolFinished { id, output: "error[E0425]: cannot find value `cfg` in this scope\n --> src/main.rs:14:9".into(), ok: false }).await?;
                self.report_usage().await?;
                return self.emit(AgentEvent::TurnComplete { error: Some("The mock agent hit an error: the build failed and the session ended (exit code 101).".into()) }).await;
            }
        }
        self.finish().await
    }

    async fn finish(&mut self) -> Step {
        self.acknowledge_steer().await?;
        self.report_usage().await?;
        self.emit(AgentEvent::TurnComplete { error: None }).await
    }

    /// A usage limit hit partway through, resetting `after` from now. Like Claude Code: the
    /// limit's own message instead of an answer, and the turn fails with it.
    async fn limit(&mut self, after: Duration) -> Step {
        let resets_at = {
            let mut limits = LIMITS.lock().unwrap();
            *limits.entry(self.native_id.clone()).or_insert_with(|| trek_core::store::now_ms() + after.as_millis() as i64)
        };
        self.think("Picking up the parser refactor where it stood.").await?;
        self.tool("Read", "src/parser.rs", "pub fn parse(input: &str) -> Result<Ast> { … }", 150).await?;
        let at = chrono::DateTime::from_timestamp_millis(resets_at).map(|d| d.with_timezone(&chrono::Local).format("%-I:%M%P").to_string()).unwrap_or_default();
        let message = format!("You've hit your session limit · resets {at}");
        self.emit(AgentEvent::LimitReached { message: message.clone(), resets_at: Some(resets_at), scope: crate::LimitScope::Session }).await?;
        self.emit(AgentEvent::TurnComplete { error: Some(message) }).await
    }

    /// The tokens a turn used, as agents report them as it ends: the conversation so far read
    /// from the cache, the new message fresh, and the answer.
    async fn report_usage(&self) -> Step {
        let tokens = trek_core::TokenUsage { input: 1_200, output: 420, cache_read: self.context, cache_write: 0 };
        self.emit(AgentEvent::Usage { model: Some(self.model.clone()), tokens, cost: None }).await
    }

    /// `mock:cost`: a turn as Claude Code reports one on Claude Sonnet 5.5, priced at its API
    /// price: billed per token, or covered by a Claude Max plan.
    async fn priced_turn(&mut self, plan: bool) -> Step {
        self.emit(AgentEvent::Billing(if plan { Billing::Plan(Some("Claude Max".into())) } else { Billing::Metered })).await?;
        self.think("Reading what the calls cost.").await?;
        self.say("Each call is priced at the model's API rates: fresh input, cache writes and reads, and output.").await?;
        let model = "claude-sonnet-5-5";
        let tokens = trek_core::TokenUsage { input: 2_400, output: 1_850, cache_read: 182_000, cache_write: 12_600 };
        let cost = trek_core::pricing::request(model, &trek_core::AgentId::ClaudeCode, &tokens, tokens.cache_write, false);
        self.emit(AgentEvent::Usage { model: Some(model.into()), tokens, cost }).await?;
        self.emit(AgentEvent::TurnComplete { error: None }).await
    }

    /// Wait `d`, handling whatever the user sends meanwhile.
    async fn pause(&mut self, d: Duration) -> Step {
        if d.is_zero() {
            loop {
                match self.commands.try_recv() {
                    Ok(cmd) => self.handle_midturn(Ok(cmd))?,
                    Err(async_channel::TryRecvError::Empty) => break,
                    Err(async_channel::TryRecvError::Closed) => return Err(Stop::Closed),
                }
            }
            tokio::task::yield_now().await;
            return Ok(());
        }
        let sleep = tokio::time::sleep(d);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return Ok(()),
                cmd = self.commands.recv() => self.handle_midturn(cmd)?,
            }
        }
    }

    /// Commands that arrive while a turn runs and no request is pending.
    fn handle_midturn(&mut self, cmd: std::result::Result<Command, async_channel::RecvError>) -> Step {
        match cmd {
            Ok(Command::Prompt { text, .. }) => {
                self.remember(&text);
                self.steer.push(text);
            }
            Ok(Command::Interrupt) => return Err(Stop::Interrupted),
            Ok(Command::SetHandHolding(h)) => self.hand_holding = h,
            Ok(Command::ReadTask { id }) => self.read_task(&id),
            Ok(Command::StopTask { id }) => self.stop_task(&id),
            Ok(Command::Shutdown) | Err(_) => return Err(Stop::Closed),
            Ok(Command::SetModel { .. } | Command::Respond { .. } | Command::Answer { .. }) => {}
        }
        Ok(())
    }

    /// What a background task has printed so far, without a turn (as Claude Code's
    /// `get_task_output` answers).
    fn read_task(&self, id: &str) {
        if let Some(output) = self.background.output(id) {
            let _ = self.events.try_send(AgentEvent::TaskOutput { id: id.to_string(), output });
        }
    }

    /// Stop a background task: it's off the list at once. A sub-agent's call ends, failed, and
    /// the agent takes a turn of its own to say it was stopped, as Claude Code does.
    fn stop_task(&self, id: &str) {
        let Some(task) = self.background.take(id) else { return };
        let _ = self.events.try_send(self.background.event());
        if task.kind == BackgroundKind::Agent {
            let call = task.call.clone().unwrap_or_else(|| task.id.clone());
            let _ = self.events.try_send(AgentEvent::Task { id: call.clone(), description: None, activity: None, tool_uses: None, done: Some(false) });
            let _ = self.events.try_send(AgentEvent::ToolFinished { id: call, output: task.title.clone(), ok: false });
            let _ = self.note.try_send(Note::Stopped(task.title));
        }
    }

    /// Background work reported something: the agent takes a turn on its own, with no message
    /// from the user (as Claude Code does on a task's notification).
    async fn woken(&mut self, note: &Note) -> Step {
        let run = async {
            match note {
                Note::Watcher(what) => {
                    self.think("The watcher has something.").await?;
                    self.tool("Read", "tests/parser.rs", "#[test]\nfn rejects_a_truncated_body() { … }", 120).await?;
                    self.say(&format!("{what} The parser accepts a body that ends early; I'll make it return `ParseError::Truncated`.")).await?;
                }
                Note::Stopped(what) => {
                    self.say(&format!("The sub-agent \"{what}\" was stopped. Ready when you are.")).await?;
                }
                Note::Scouts => {
                    self.say("Both scouts reported back:\n\n1. **Routes** — 12 handlers in `src/routes.rs`, two of them unauthenticated.\n2. **Errors** — 7 `unwrap()` calls on request input; I'd turn those into `400`s.").await?;
                }
            }
            self.finish().await
        };
        match run.await {
            Err(Stop::Interrupted) => self.emit(AgentEvent::TurnComplete { error: Some("Interrupted".into()) }).await,
            other => other,
        }
    }

    /// Block until `request_id` is answered.
    async fn wait_reply(&mut self, request_id: &str) -> Step<Reply> {
        loop {
            match self.commands.recv().await {
                Ok(Command::Respond { request_id: r, decision }) if r == request_id => return Ok(Reply::Decision(decision)),
                Ok(Command::Answer { request_id: r, answers }) if r == request_id => return Ok(Reply::Answers(answers)),
                other => self.handle_midturn(other)?,
            }
        }
    }

    /// A prompt sent mid-turn is folded into the running turn, as Claude Code does.
    async fn acknowledge_steer(&mut self) -> Step {
        for text in std::mem::take(&mut self.steer) {
            self.say(&format!("Noted — {}. I've taken that into account.", text.trim().trim_end_matches('.'))).await?;
        }
        Ok(())
    }

    async fn think(&mut self, text: &str) -> Step {
        for chunk in tokens(text) {
            self.emit(AgentEvent::ReasoningDelta(chunk.into())).await?;
            self.pause(paced(10)).await?;
        }
        Ok(())
    }

    /// Stream `text` a token at a time, then send it whole (as agents do at the end of a message).
    async fn say(&mut self, text: &str) -> Step {
        self.say_at(text, 14).await
    }

    /// `say`, `ms` between tokens.
    async fn say_at(&mut self, text: &str, ms: u64) -> Step {
        for chunk in tokens(text) {
            self.emit(AgentEvent::TextDelta(chunk.into())).await?;
            self.pause(paced(ms)).await?;
        }
        self.emit(AgentEvent::TextDone(text.into())).await
    }

    async fn tool_start(&mut self, id: &str, title: &str, detail: &str) -> Step {
        self.emit(AgentEvent::ToolStarted { id: id.into(), title: title.into(), detail: detail.into() }).await
    }

    async fn tool(&mut self, title: &str, detail: &str, output: &str, ms: u64) -> Step {
        let id = self.id("tool");
        self.tool_start(&id, title, detail).await?;
        self.pause(paced(ms)).await?;
        self.emit(AgentEvent::ToolFinished { id, output: output.into(), ok: true }).await
    }

    /// An edit that adds `added` lines and removes `removed`.
    async fn edit(&mut self, path: &str, output: &str, ms: u64, (added, removed): (u32, u32)) -> Step {
        let id = self.id("tool");
        self.tool_start(&id, "Edit", path).await?;
        self.emit(AgentEvent::ToolLines { id: id.clone(), added, removed }).await?;
        self.pause(paced(ms)).await?;
        self.emit(AgentEvent::ToolFinished { id, output: output.into(), ok: true }).await
    }

    async fn tools(&mut self) -> Step {
        self.think("I should look around before changing anything.").await?;
        self.tool("Run command", "ls -la", LS_OUTPUT, 250).await?;
        self.tool("Read", "src/main.rs", MAIN_RS, 180).await?;
        self.tool("Search", "parse_args", "src/main.rs:3:    let cfg = parse_args();\nsrc/cli.rs:12:pub fn parse_args() -> Config {", 150).await?;
        self.acknowledge_steer().await?;
        self.say("The flag parsing lives in `src/cli.rs`; I'll add the `--verbose` flag there and wire it through `src/main.rs`.").await?;
        self.edit("src/cli.rs", "Applied 1 edit to src/cli.rs", 220, (9, 1)).await?;
        self.edit("src/main.rs", "Applied 2 edits to src/main.rs", 200, (5, 2)).await?;
        self.emit(AgentEvent::DiffStat { additions: 14, deletions: 3 }).await?;
        self.tool("Run command", "cargo test", TEST_OUTPUT, 600).await?;
        self.say("Added a `--verbose` flag:\n\n- `src/cli.rs` parses it into `Config::verbose`\n- `src/main.rs` raises the log level when it's set\n\nAll 14 tests pass.").await
    }

    /// Ask a mock sub-agent, through Trek, for a second opinion on whatever follows the keyword.
    async fn delegate(&mut self, text: &str, wait: bool) -> Step {
        let lower = text.to_lowercase();
        let task = ["mock:consult", "mock:delegate"]
            .iter()
            .find_map(|k| lower.find(k).map(|at| text[at + k.len()..].trim()))
            .filter(|t| !t.is_empty())
            .unwrap_or("Review how the app starts and say what you'd change.")
            .to_string();
        self.say("I'll get a second opinion from another model.").await?;
        let id = self.id("tool");
        let title = "Second opinion";
        // Named as Claude names MCP tools.
        self.tool_start(&id, &format!("mcp__{ORCHESTRATE_SERVER}__delegate_task"), title).await?;
        let params = serde_json::json!({ "title": title, "prompt": task, "agent": format!("direct:{PROVIDER}"), "model": "mock-swift", "effort": "low", "mode": "advise", "wait": wait });
        match self.ipc("delegate_task", params).await? {
            Ok(answer) => {
                let output = serde_json::to_string_pretty(&answer).unwrap_or_default();
                self.emit(AgentEvent::ToolFinished { id, output, ok: true }).await?;
                match answer["result"].as_str() {
                    Some(r) if wait => self.say(&format!("The second opinion is in: {}", trek_core::orchestrate::preview(r, 160))).await,
                    _ => self.say("It's on it. I'll pick its answer up when it reports back.").await,
                }
            }
            Err(e) => {
                self.emit(AgentEvent::ToolFinished { id, output: e.clone(), ok: false }).await?;
                self.say(&format!("I couldn't get a second opinion: {e}")).await
            }
        }
    }

    /// One call to Trek's orchestration tools, as `trek-mcp orchestrate` would make it. The user
    /// can stop the turn while it waits: the call's connection is dropped.
    async fn ipc(&mut self, method: &str, params: serde_json::Value) -> Step<std::result::Result<serde_json::Value, String>> {
        let Some(client) = self.orchestrate.clone() else { return Ok(Err("Trek's sub-agent tools aren't in this session.".into())) };
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let method = method.to_string();
        let mut call = tokio::task::spawn_blocking(move || {
            let mut conn = client.connect()?;
            if let Ok(h) = conn.handle() {
                let _ = handle_tx.send(h);
            }
            conn.call(&method, &params)
        });
        loop {
            tokio::select! {
                done = &mut call => return Ok(done.unwrap_or_else(|e| Err(e.to_string()))),
                cmd = self.commands.recv() => {
                    if let Err(stop) = self.handle_midturn(cmd) {
                        if let Ok(h) = handle_rx.try_recv() {
                            let _ = h.shutdown(std::net::Shutdown::Both);
                        }
                        return Err(stop);
                    }
                }
            }
        }
    }

    /// Several calls to Trek's orchestration tools at once, answered in order. As with `ipc`, the
    /// user can stop the turn while they wait: their connections are dropped.
    async fn ipc_all(&mut self, method: &str, calls: Vec<serde_json::Value>) -> Step<Vec<std::result::Result<serde_json::Value, String>>> {
        let Some(client) = self.orchestrate.clone() else { return Ok(calls.iter().map(|_| Err("Trek's sub-agent tools aren't in this session.".into())).collect()) };
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let pending: Vec<_> = calls
            .into_iter()
            .map(|params| {
                let (client, method, handle_tx) = (client.clone(), method.to_string(), handle_tx.clone());
                tokio::task::spawn_blocking(move || {
                    let mut conn = client.connect()?;
                    if let Ok(h) = conn.handle() {
                        let _ = handle_tx.send(h);
                    }
                    conn.call(&method, &params)
                })
            })
            .collect();
        let mut all = Box::pin(futures::future::join_all(pending));
        loop {
            tokio::select! {
                done = &mut all => return Ok(done.into_iter().map(|r| r.unwrap_or_else(|e| Err(e.to_string()))).collect()),
                cmd = self.commands.recv() => {
                    if let Err(stop) = self.handle_midturn(cmd) {
                        while let Ok(h) = handle_rx.try_recv() {
                            let _ = h.shutdown(std::net::Shutdown::Both);
                        }
                        return Err(stop);
                    }
                }
            }
        }
    }

    /// Say back what it was asked, in its own words, and stop there.
    async fn restate(&mut self, text: &str) -> Step {
        let said = trek_core::restate::as_written(text).trim().trim_end_matches(['.', '?', '!']);
        let ask = match said.chars().next() {
            Some(c) => format!("{}{}", c.to_lowercase(), &said[c.len_utf8()..]),
            None => "look at what we have so far".into(),
        };
        self.think("Before anything else: say back what I've been asked, and nothing more.").await?;
        self.say(&format!(
            "Here's how I understand it, in my own words:\n\n- **What you want:** you'd like me to {ask}.\n- **Why it matters:** the current behaviour gets in your way, and you want it fixed at the cause rather than patched over.\n- **What I'd leave alone:** anything that isn't part of that.\n\nI haven't changed anything. Tell me if that's right and I'll go ahead."
        ))
        .await
    }

    /// An arena as Trek's instructions lay it out: ground the problem, have each candidate draft
    /// a design at once, have the judge score them blind, and synthesise.
    async fn arena(&mut self, text: &str) -> Step {
        let (said, consult) = trek_core::orchestrate::split_consult(text);
        let Some(consult) = consult else { return self.say("There's no arena in that message.").await };
        let said = trek_core::restate::as_written(said).trim().to_string();
        self.think("Ground the problem before anyone designs: what's there now, and how callers will use it.").await?;
        self.tool("Read", "src/webhooks.rs", "pub fn deliver(endpoint: &Endpoint, event: Event) -> Result<()> { … }", 120).await?;
        self.tool("Search", "deliver(", "src/webhooks.rs:41\nsrc/jobs/retry.rs:18\nsrc/api/events.rs:77", 100).await?;
        let brief = format!(
            "{said}\n\nGround: webhooks go out one event at a time through `deliver` in src/webhooks.rs, called from the API, the retry job and the event fan-out. Endpoints belong to customers; a slow one mustn't hold up the rest. Callers want to know whether an event went, waits, or was dropped."
        );
        let letters: Vec<char> = (0..consult.consultants.len()).map(|i| (b'A' + i as u8) as char).collect();
        self.say(&format!("Here's the ground: `deliver` in `src/webhooks.rs` has three callers, and one slow endpoint can hold up the rest. {} candidates will draft designs on their own.", consult.consultants.len())).await?;
        // Every candidate at once, each with a row of its own.
        let mut rows = vec![];
        let mut calls = vec![];
        for (c, letter) in consult.consultants.iter().zip(&letters) {
            let id = self.id("tool");
            let title = format!("Design {letter}");
            self.tool_start(&id, &format!("mcp__{ORCHESTRATE_SERVER}__delegate_task"), &title).await?;
            rows.push(id);
            calls.push(serde_json::json!({ "title": title, "prompt": format!("mock:design {brief}"), "agent": c.agent.key(), "model": c.model, "effort": c.effort.as_str(), "mode": "advise", "wait": true }));
        }
        let answers = self.ipc_all("delegate_task", calls).await?;
        let mut packages = vec![];
        for ((id, letter), answer) in rows.into_iter().zip(&letters).zip(answers) {
            match answer {
                Ok(v) => {
                    self.emit(AgentEvent::ToolFinished { id, output: serde_json::to_string_pretty(&v).unwrap_or_default(), ok: true }).await?;
                    packages.push(format!("Design {letter}:\n{}", v["result"].as_str().unwrap_or("(no design)")));
                }
                Err(e) => {
                    self.emit(AgentEvent::ToolFinished { id, output: e.clone(), ok: false }).await?;
                    return self.say(&format!("The arena couldn't run: {e}")).await;
                }
            }
        }
        // The judge sees the designs by letter only.
        let Some(judge) = consult.judge.clone().or_else(|| consult.consultants.first().cloned()) else { return self.say("The arena has no candidates.").await };
        let id = self.id("tool");
        self.tool_start(&id, &format!("mcp__{ORCHESTRATE_SERVER}__delegate_task"), "Judge the designs").await?;
        let call = serde_json::json!({ "title": "Judge the designs", "prompt": format!("mock:judge Score these designs.\n\n{brief}\n\n{}", packages.join("\n\n")), "agent": judge.agent.key(), "model": judge.model, "effort": judge.effort.as_str(), "mode": "advise", "wait": true });
        let verdict = match self.ipc("delegate_task", call).await? {
            Ok(v) => {
                self.emit(AgentEvent::ToolFinished { id, output: serde_json::to_string_pretty(&v).unwrap_or_default(), ok: true }).await?;
                v["result"].as_str().unwrap_or_default().to_string()
            }
            Err(e) => {
                self.emit(AgentEvent::ToolFinished { id, output: e.clone(), ok: false }).await?;
                return self.say(&format!("The judge couldn't run: {e}")).await;
            }
        };
        let winner = verdict.lines().find_map(|l| l.strip_prefix("Strongest: ")).unwrap_or("Design A").to_string();
        let then = if consult.implement { "Implementing against this sketch now." } else { "I haven't changed any files." };
        self.say(&format!(
            "The judge scored the designs blind; **{winner} won**, and I've folded in the others' retry queue.\n\n```rust\n// Call site\nmatch limiter.admit(&endpoint) {{\n    Admit::Now => deliver(&endpoint, event)?,\n    Admit::After(wait) => queue.retry_in(wait, event),\n}}\n\npub struct Limiter {{ /* per-endpoint buckets */ }}\npub enum Admit {{ Now, After(Duration) }}\nimpl Limiter {{\n    pub fn per_endpoint(rate: u32, per: Duration) -> Self;\n    pub fn admit(&self, endpoint: &Endpoint) -> Admit;\n}}\n```\n\n{then}"
        ))
        .await?;
        if consult.implement {
            self.edit("src/webhooks/limiter.rs", "Created src/webhooks/limiter.rs", 160, (64, 0)).await?;
            self.edit("src/webhooks.rs", "Applied 2 edits to src/webhooks.rs", 140, (9, 3)).await?;
        }
        Ok(())
    }

    /// A design package, its approach after the model playing it.
    async fn design(&mut self) -> Step {
        let (name, shape) = match self.model.as_str() {
            "mock-deep" => ("Leaky bucket with a retry queue", "Queue::push(endpoint, event) -> Ticket"),
            m if m.starts_with("relay") => ("Sliding window per endpoint", "Window::check(endpoint) -> Result<(), RetryAt>"),
            _ => ("Token bucket per endpoint", "Limiter::admit(endpoint) -> Admit"),
        };
        self.think("Sketch the call sites first; the types follow from them.").await?;
        self.say(&format!(
            "## {name}\n\n**Call sites**\n\n```rust\nlet admit = limiter.admit(&endpoint);\n```\n\n**Core types and signatures**\n\n```rust\n{shape}\n```\n\n**Rationale:** one decision per event, made where `deliver` is called, so no caller needs to know how the limit is kept. It fails closed: an endpoint it can't place waits rather than floods."
        ))
        .await
    }

    /// Scores for the designs in `text`, by letter.
    async fn judge(&mut self, text: &str) -> Step {
        let letters: Vec<char> = text.lines().filter_map(|l| l.strip_prefix("Design ").and_then(|r| r.strip_suffix(':')).and_then(|r| r.chars().next())).collect();
        let strongest = if letters.len() > 1 { letters[1] } else { letters.first().copied().unwrap_or('A') };
        let rows: Vec<String> = letters
            .iter()
            .map(|l| {
                let total = if *l == strongest { "4.6" } else { "3.8" };
                format!("| {l} | {} | {} | 4 | {} | 4 | {total} |", if *l == strongest { 5 } else { 4 }, if *l == strongest { 5 } else { 3 }, if *l == strongest { 5 } else { 4 })
            })
            .collect();
        self.think("Score each against the rubric, without knowing who wrote it.").await?;
        self.say(&format!(
            "Strongest: Design {strongest}\n\nIts interface is one call deep and fails closed. The others' retry queue is worth keeping.\n\n| Design | Call sites | Depth | Simplicity | Failure | Fit | Score |\n| --- | --- | --- | --- | --- | --- | --- |\n{}",
            rows.join("\n")
        ))
        .await
    }

    /// Where the mock keeps the verification skill it sets up, in the session's folder.
    const VERIFY_SKILL: &'static str = ".agents/skills/verify-app";

    /// Build a small verification skill in the session's folder (for real): SKILL.md marked for
    /// Trek, a CLI, and a Feature Map. Then run its check.
    async fn setup_verification(&mut self) -> Step {
        let dir = self.cwd.join(Self::VERIFY_SKILL);
        let cli = format!("./{}/scripts/app", Self::VERIFY_SKILL);
        self.think("Learn how the app runs, then build the lever: a CLI first, the notes after.").await?;
        self.tool("Read", "README.md", "# The app\n\nRun it with `cargo run`.", 120).await?;
        let files: [(&str, String, bool); 4] = [
            ("SKILL.md", format!("---\nname: verify-app\ndescription: Drive, debug and verify the app. Use it to check any change works before calling it done.\nmetadata:\n  trek: verification\n  cli: {cli}\n---\n\n# Verify the app\n\nRun `{cli} check` before calling a change done. `{cli} --help` lists every command.\n\nThe Feature Map is in references/features/README.md.\n"), false),
            ("scripts/app", "#!/bin/sh\n# The app's verification CLI.\ncase \"$1\" in\n  check) echo '{\"ok\":true,\"checks\":[\"build\",\"launch\",\"smoke\"]}' ;;\n  --help|\"\") echo 'app <check|open|screenshot> [--json] [--dry-run]' ;;\n  *) echo \"{\\\"ok\\\":false,\\\"error\\\":\\\"No command $1: see app --help\\\"}\"; exit 2 ;;\nesac\n".to_string(), true),
            ("references/features/README.md", "# Feature Map\n\n- [Notes](notes.md): write and find notes.\n".to_string(), false),
            ("references/features/notes.md", "# Notes\n\nWrite and find notes. Reach it from the sidebar's Notes item, or `app open notes`.\n".to_string(), false),
        ];
        for (path, body, exec) in files {
            let id = self.id("tool");
            let full = dir.join(path);
            self.tool_start(&id, "Write", &full.display().to_string()).await?;
            let wrote = full.parent().map(|p| std::fs::create_dir_all(p)).transpose().and_then(|_| std::fs::write(&full, &body));
            if exec && wrote.is_ok() {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755));
            }
            self.emit(AgentEvent::ToolLines { id: id.clone(), added: body.lines().count() as u32, removed: 0 }).await?;
            let (output, ok) = match wrote {
                Ok(()) => (format!("Wrote {path}"), true),
                Err(e) => (format!("Couldn't write {path}: {e}"), false),
            };
            self.emit(AgentEvent::ToolFinished { id, output, ok }).await?;
            self.pause(paced(60)).await?;
        }
        self.tool("Run command", &format!("{cli} check --json"), "{\"ok\":true,\"checks\":[\"build\",\"launch\",\"smoke\"]}", 200).await?;
        self.say(&format!("The verification skill is in `{}`: a CLI (`{cli}`) with `check`, `open` and `screenshot`, and a Feature Map. `check` passes.", Self::VERIFY_SKILL)).await
    }

    /// Touch the verification skill up: run it, and add what the Feature Map lacks.
    async fn maintain_verification(&mut self) -> Step {
        let map = self.cwd.join(Self::VERIFY_SKILL).join("references/features/README.md");
        let Ok(before) = std::fs::read_to_string(&map) else { return self.say("There's no verification skill here to maintain yet.").await };
        let cli = format!("./{}/scripts/app", Self::VERIFY_SKILL);
        self.tool("Run command", &format!("{cli} check --json"), "{\"ok\":true,\"checks\":[\"build\",\"launch\",\"smoke\"]}", 150).await?;
        let line = "- [Settings](settings.md): the app's preferences.\n";
        if !before.contains(line) {
            let id = self.id("tool");
            self.tool_start(&id, "Edit", &map.display().to_string()).await?;
            let _ = std::fs::write(&map, format!("{before}{line}"));
            let _ = std::fs::write(map.with_file_name("settings.md"), "# Settings\n\nThe app's preferences. Reach them with ⌘, or `app open settings`.\n");
            self.emit(AgentEvent::ToolLines { id: id.clone(), added: 1, removed: 0 }).await?;
            self.emit(AgentEvent::ToolFinished { id, output: "Applied 1 edit".into(), ok: true }).await?;
        }
        self.say("The verification skill is up to date: `check` passes, and the Feature Map has the Settings screen now.").await
    }

    /// Run the verification CLI Trek named for the project (the mock doesn't run anything for
    /// real here: the output is what a passing check prints).
    async fn verify(&mut self) -> Step {
        let cli = self.instructions.as_deref().and_then(|i| i.split_once("(`")).and_then(|(_, rest)| rest.split_once('`')).map(|(cli, _)| cli.to_string());
        let Some(cli) = cli else { return self.say("This project has no verification skill yet, so there's nothing to check the change with.").await };
        self.edit("src/notes.rs", "Applied 1 edit to src/notes.rs", 120, (6, 2)).await?;
        self.tool("Run command", &format!("{cli} check --json"), "{\"ok\":true,\"checks\":[\"build\",\"launch\",\"smoke\"]}", 200).await?;
        self.say(&format!("Changed `src/notes.rs` and verified it with `{cli} check`: build, launch and the smoke run pass.")).await
    }

    /// Add a line to `NOTES.md` in the session's folder: a real change, for worktree reviews.
    async fn write(&mut self) -> Step {
        let path = self.cwd.join("NOTES.md");
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let n = before.lines().filter(|l| l.starts_with("- ")).count() + 1;
        let text = if before.is_empty() { format!("# Notes\n\n- Note {n}\n") } else { format!("{before}- Note {n}\n") };
        let id = self.id("tool");
        self.tool_start(&id, "Edit", "NOTES.md").await?;
        self.pause(paced(150)).await?;
        let (output, ok) = match std::fs::write(&path, &text) {
            Ok(()) => (format!("Wrote note {n} to NOTES.md"), true),
            Err(e) => (format!("Couldn't write NOTES.md: {e}"), false),
        };
        let added = (text.lines().count() - before.lines().count()) as i64;
        if ok {
            self.emit(AgentEvent::ToolLines { id: id.clone(), added: added as u32, removed: 0 }).await?;
        }
        self.emit(AgentEvent::ToolFinished { id, output, ok }).await?;
        self.emit(AgentEvent::DiffStat { additions: added, deletions: 0 }).await?;
        self.say(&format!("Added note {n} to `NOTES.md`.")).await
    }

    async fn agents(&mut self, after: Option<Duration>) -> Step {
        let after = after.unwrap_or_else(|| paced(6_000));
        self.say("I'll send two scouts ahead: one maps the HTTP routes, the other audits error handling.").await?;
        let scouts = [
            ("Map the HTTP routes", "Reading src/routes.rs", ("Read", "src/routes.rs")),
            ("Audit error handling", "Searching for unwrap()", ("Search", "unwrap()")),
        ];
        let mut ids = vec![];
        for (description, ..) in scouts {
            let id = self.id("agent");
            self.tool_start(&id, "Subagent", description).await?;
            self.emit(AgentEvent::Task { id: id.clone(), description: Some(description.into()), activity: None, tool_uses: None, done: None }).await?;
            self.background.add(BackgroundTask { id: id.clone(), kind: BackgroundKind::Agent, title: description.into(), call: Some(id.clone()), readable: false, stoppable: true });
            ids.push(id);
        }
        self.emit(self.background.event()).await?;
        for id in &ids {
            self.emit(AgentEvent::ToolFinished { id: id.clone(), output: "Async agent launched successfully.".into(), ok: true }).await?;
        }
        self.say("Both scouts are out. I'll pull their findings together when they report back.").await?;
        self.finish().await?;
        // The turn is over; the scouts work on, and the agent takes a turn of its own once they're
        // back (none if they were all stopped). Meanwhile the session takes messages as usual.
        let (jobs, events, note) = (self.background.clone(), self.events.clone(), self.note.clone());
        tokio::spawn(async move {
            let send = |ev: AgentEvent| events.try_send(ev).is_ok();
            for (i, id) in ids.iter().enumerate() {
                tokio::time::sleep(after / 3).await;
                let (_, activity, (title, detail)) = scouts[i];
                if jobs.running(id) {
                    send(AgentEvent::TaskStep { task: id.clone(), title: title.into(), detail: detail.into() });
                    send(AgentEvent::Task { id: id.clone(), description: None, activity: Some(activity.into()), tool_uses: Some(2 + i as u64 * 3), done: None });
                }
            }
            let mut back = 0;
            for id in ids {
                tokio::time::sleep(after / 3).await;
                // Stopped on request: it's off the list already, and says nothing more.
                let done = jobs.remove(&id);
                send(AgentEvent::Task { id: id.clone(), description: None, activity: None, tool_uses: None, done: Some(done) });
                if done {
                    back += 1;
                    send(jobs.event());
                    send(AgentEvent::ToolFinished { id, output: "Found what it was sent for.".into(), ok: true });
                }
            }
            if back > 0 {
                let _ = note.send(Note::Scouts).await;
            }
        });
        Ok(())
    }

    /// `mock:task`: a sub-agent of its own (Claude's `Task`, in the foreground) that the turn
    /// waits on while it works, reporting its steps as it goes.
    async fn task(&mut self, total: Duration) -> Step {
        self.say("I'll have a sub-agent survey the tests while I wait for it.").await?;
        let id = self.id("agent");
        let description = "Survey the test suite";
        self.tool_start(&id, "Subagent", description).await?;
        self.emit(AgentEvent::Task { id: id.clone(), description: Some(description.into()), activity: None, tool_uses: None, done: None }).await?;
        let steps = [("Read", "tests/routes.rs", "Reading tests/routes.rs"), ("Search", "#[test]", "Searching for #[test]"), ("Run command", "cargo test --no-run", "Running cargo test --no-run")];
        for (i, (title, detail, activity)) in steps.into_iter().enumerate() {
            self.pause(total / steps.len() as u32).await?;
            self.emit(AgentEvent::TaskStep { task: id.clone(), title: title.into(), detail: detail.into() }).await?;
            self.emit(AgentEvent::Task { id: id.clone(), description: None, activity: Some(activity.into()), tool_uses: Some(i as u64 + 1), done: None }).await?;
        }
        self.emit(AgentEvent::Task { id: id.clone(), description: None, activity: None, tool_uses: None, done: Some(true) }).await?;
        self.emit(AgentEvent::ToolFinished { id, output: "212 tests across 18 files; 3 are ignored.".into(), ok: true }).await?;
        self.say("The sub-agent counted 212 tests across 18 files, 3 of them ignored.").await
    }

    /// `mock:pair`: two sub-agents on the same task, through Trek; the turn ends while they work.
    async fn pair(&mut self, text: &str) -> Step {
        let lower = text.to_lowercase();
        let task = lower.find("mock:pair").map(|at| text[at + "mock:pair".len()..].trim()).filter(|t| !t.is_empty()).unwrap_or("Review how the app starts.").to_string();
        self.say("I'll ask two models at once.").await?;
        for title in ["First opinion", "Second opinion"] {
            let id = self.id("tool");
            self.tool_start(&id, &format!("mcp__{ORCHESTRATE_SERVER}__delegate_task"), title).await?;
            let params = serde_json::json!({ "title": title, "prompt": task, "agent": format!("direct:{PROVIDER}"), "model": "mock-swift", "effort": "low", "mode": "advise" });
            let (output, ok) = match self.ipc("delegate_task", params).await? {
                Ok(answer) => (serde_json::to_string_pretty(&answer).unwrap_or_default(), true),
                Err(e) => (e, false),
            };
            self.emit(AgentEvent::ToolFinished { id, output, ok }).await?;
        }
        self.say("Both are on it. I'll compare what they say when they report back.").await
    }

    /// `mock:server`: a dev server left running in the background, printing as it goes. It runs
    /// for `until` (or until stopped, or the session ends), and says nothing to the agent.
    async fn server(&mut self, until: Option<Duration>) -> Step {
        let id = self.id("shell");
        self.tool_start(&id, "Run command", "npm run dev").await?;
        self.background.add(BackgroundTask { id: id.clone(), kind: BackgroundKind::Shell, title: "npm run dev".into(), call: Some(id.clone()), readable: true, stoppable: true });
        for line in ["> trail-app@0.4.0 dev", "> vite", "", "  VITE v5.4.2  ready in 312 ms", "", "  ➜  Local:   http://localhost:5173/"] {
            self.background.print(&id, line);
        }
        self.emit(self.background.event()).await?;
        self.emit(AgentEvent::ToolFinished { id: id.clone(), output: format!("Command running in background with ID: {id}."), ok: true }).await?;
        let (jobs, events) = (self.background.clone(), self.events.clone());
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let lines = ["[vite] page reload src/App.tsx", "[vite] hmr update /src/routes/Home.tsx", "GET / 200 in 14ms", "[vite] page reload src/styles.css"];
            let mut n = 0;
            while jobs.running(&id) && until.is_none_or(|u| started.elapsed() < u) {
                // Real time, whatever the pace: a server prints when it prints.
                tokio::time::sleep(Duration::from_millis(400).min(until.unwrap_or(Duration::MAX))).await;
                if n % 3 == 2 {
                    jobs.print(&id, lines[(n / 3) % lines.len()]);
                }
                n += 1;
            }
            // It exited on its own: off the list (one stopped is off it already).
            if jobs.remove(&id) {
                let _ = events.send(jobs.event()).await;
            }
        });
        self.say("The dev server is running at http://localhost:5173 — it stays up in the background while you try it.").await
    }

    /// `mock:watch`: a test watcher left running; `after` later it catches a failure, ends, and
    /// the agent takes a turn of its own about it.
    async fn watch(&mut self, after: Duration) -> Step {
        let id = self.id("watch");
        self.tool_start(&id, "Monitor", "cargo watch -x test").await?;
        self.background.add(BackgroundTask { id: id.clone(), kind: BackgroundKind::Monitor, title: "cargo watch -x test".into(), call: Some(id.clone()), readable: true, stoppable: true });
        self.background.print(&id, "[Running 'cargo test']");
        self.background.print(&id, "test result: ok. 14 passed; 0 failed");
        self.emit(self.background.event()).await?;
        self.emit(AgentEvent::ToolFinished { id: id.clone(), output: format!("Monitor started (task {id})."), ok: true }).await?;
        let (jobs, events, note) = (self.background.clone(), self.events.clone(), self.note.clone());
        tokio::spawn(async move {
            // A watcher stopped (or whose session ended) catches nothing more.
            let started = tokio::time::Instant::now();
            while jobs.running(&id) && started.elapsed() < after {
                tokio::time::sleep((after - started.elapsed()).min(Duration::from_millis(200))).await;
            }
            jobs.print(&id, "test parser::rejects_a_truncated_body ... FAILED");
            if jobs.remove(&id) {
                let _ = events.send(jobs.event()).await;
                let _ = note.send(Note::Watcher("The test watcher caught a failure: `parser::rejects_a_truncated_body`.".into())).await;
            }
        });
        self.say("The test watcher is running in the background; I'll pick up anything it catches.").await
    }

    async fn permission(&mut self) -> Step {
        self.say("The schema change needs the migration script to run.").await?;
        let tool = self.id("tool");
        let command = "./scripts/migrate.sh --apply";
        self.tool_start(&tool, "Run command", command).await?;
        let ask = !self.commands_allowed && self.hand_holding != HandHolding::FullAccess;
        let decision = if ask {
            let request_id = self.id("permission");
            self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "Run command".into(), detail: command.into(), prompt: None }).await?;
            match self.wait_reply(&request_id).await? {
                Reply::Decision(d) => d,
                Reply::Answers(_) => Decision::Allow,
            }
        } else {
            Decision::Allow
        };
        if decision == Decision::AllowForSession {
            self.commands_allowed = true;
        }
        if decision == Decision::Deny {
            self.emit(AgentEvent::ToolFinished { id: tool, output: "The user declined this action.".into(), ok: false }).await?;
            return self.say("Okay, I won't run it. The migration is in `scripts/migrate.sh` if you want to apply it yourself.").await;
        }
        self.pause(paced(500)).await?;
        self.emit(AgentEvent::ToolFinished { id: tool, output: "Applying 3 migrations…\n✓ 0007_add_users_email\n✓ 0008_index_sessions\n✓ 0009_drop_legacy_tokens".into(), ok: true }).await?;
        self.say("Migrations applied: the `users` table has an `email` column and sessions are indexed.").await
    }

    async fn questions(&mut self) -> Step {
        self.think("Two choices only the user can make.").await?;
        let request_id = self.id("question");
        let questions = vec![
            Question {
                question: "Which database should the service use?".into(),
                header: "Database".into(),
                options: vec![
                    ("SQLite".into(), "One file next to the binary, zero setup.".into()),
                    ("Postgres".into(), "A server; better for many concurrent writers.".into()),
                ],
                multi: false,
                secret: false,
            },
            Question {
                question: "What should ship with it?".into(),
                header: "Extras".into(),
                options: vec![("Migrations".into(), String::new()), ("Seed data".into(), String::new()), ("Backups".into(), "Nightly, kept for a week.".into())],
                multi: true,
                secret: false,
            },
        ];
        self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "AskUserQuestion".into(), detail: String::new(), prompt: Some(Prompt::Questions(questions)) })
            .await?;
        match self.wait_reply(&request_id).await? {
            Reply::Answers(answers) => {
                let summary = answers.iter().map(|(q, a)| format!("- {q} **{a}**")).collect::<Vec<_>>().join("\n");
                self.say(&format!("Got it:\n\n{summary}\n\nSetting that up now.")).await
            }
            Reply::Decision(_) => self.say("No problem — I'll go with SQLite and migrations, the simplest setup.").await,
        }
    }

    async fn plan_turn(&mut self) -> Step {
        self.think("Read-only first: understand the layout, then propose a plan.").await?;
        self.tool("Read", "src/routes.rs", "pub fn routes() -> Router { … }", 200).await?;
        self.tool("Search", "fn handler", "src/routes.rs: 12 matches", 150).await?;
        let request_id = self.id("plan");
        self.emit(AgentEvent::PermissionRequest { request_id: request_id.clone(), title: "ExitPlanMode".into(), detail: String::new(), prompt: Some(Prompt::Plan(PLAN.into())) }).await?;
        match self.wait_reply(&request_id).await? {
            Reply::Decision(Decision::Deny) => self.say("Okay — I'll keep refining the plan. What should change?").await,
            _ => {
                self.plan = false;
                self.edit("src/auth.rs", "Applied 1 edit to src/auth.rs", 250, (24, 2)).await?;
                self.edit("src/routes.rs", "Applied 2 edits to src/routes.rs", 250, (7, 4)).await?;
                self.emit(AgentEvent::DiffStat { additions: 31, deletions: 6 }).await?;
                self.tool("Run command", "cargo test", TEST_OUTPUT, 500).await?;
                self.say("Implemented the plan: every route now goes through `require_session`, and the tests pass.").await
            }
        }
    }

    /// A long build: one command that runs for most of `total`, with a context update every ten
    /// seconds at demo pace (none at pace zero: tests would otherwise see them land depending on
    /// how fast the machine runs them). Steering is acknowledged within a quarter second.
    async fn long(&mut self, total: Duration) -> Step {
        let started = tokio::time::Instant::now();
        self.think("This needs the full test suite; it takes a while.").await?;
        let id = self.id("tool");
        self.tool_start(&id, "Run command", "cargo test --workspace").await?;
        let end = started + total;
        let every = paced(10_000);
        let mut next_context = if every.is_zero() { end } else { started + every };
        loop {
            let now = tokio::time::Instant::now();
            if now >= end {
                break;
            }
            self.pause((end - now).min(Duration::from_millis(250))).await?;
            self.acknowledge_steer().await?;
            if tokio::time::Instant::now() >= next_context && next_context < end {
                next_context += every;
                self.context += 800;
                self.emit(AgentEvent::Context { used: self.context, window: WINDOW }).await?;
            }
        }
        self.emit(AgentEvent::ToolFinished { id, output: TEST_OUTPUT.into(), ok: true }).await?;
        self.say(&format!("The full suite passed after {} — nothing to fix.", humanize(total))).await
    }

    /// One long answer, streamed a token at a time (as `say` does) for `total`: section after
    /// section of markdown with lists, code and paths.
    async fn stream(&mut self, total: Duration) -> Step {
        let end = tokio::time::Instant::now() + total;
        let mut text = String::new();
        let mut part = 0;
        loop {
            part += 1;
            let section = format!("{}## Part {part}\n\n{}\n\n", if part == 1 { "" } else { "\n" }, if part % 2 == 1 { ANSWER } else { PLAN });
            for chunk in tokens(&section) {
                self.emit(AgentEvent::TextDelta(chunk.into())).await?;
                text.push_str(chunk);
                if tokio::time::Instant::now() >= end {
                    return self.emit(AgentEvent::TextDone(text)).await;
                }
                // Real time, not the pace: the prompt asked for this long.
                self.pause(Duration::from_millis(14)).await?;
            }
        }
    }
}

impl Session {
    /// Tools at a steady pace for `total`, the way an agent finds its way around a project:
    /// groups of reads, finds and commands, a thought or a message between them, then edits and
    /// a test run. Real time, not the pace: the prompt asked for this long. A step takes a
    /// fourteenth of `total` (at most 900 ms), so even a short run plays several of them.
    async fn explore(&mut self, total: Duration) -> Step {
        let end = tokio::time::Instant::now() + total;
        let step = (total / 14).clamp(Duration::from_millis(5), Duration::from_millis(900));
        let mut i = 0;
        while tokio::time::Instant::now() < end {
            match EXPLORE[i % EXPLORE.len()] {
                Explore::Think(text) => self.think(text).await?,
                Explore::Say(text) => self.say(text).await?,
                Explore::Tool(title, detail) | Explore::Edit(title, detail, ..) => {
                    let id = self.id("tool");
                    self.tool_start(&id, title, detail).await?;
                    if let Explore::Edit(.., added, removed) = EXPLORE[i % EXPLORE.len()] {
                        self.emit(AgentEvent::ToolLines { id: id.clone(), added, removed }).await?;
                    }
                    self.pause(step).await?;
                    self.emit(AgentEvent::ToolFinished { id, output: format!("{title} {detail}: done"), ok: true }).await?;
                    self.pause(step / 4).await?;
                }
            }
            self.acknowledge_steer().await?;
            i += 1;
        }
        self.say("The title now fades in from the left as it arrives, and the tests pass.").await
    }
}

/// One step of `mock:explore`.
#[derive(Clone, Copy)]
enum Explore {
    Think(&'static str),
    Say(&'static str),
    /// A tool call: its title and detail, as agents report them.
    Tool(&'static str, &'static str),
    /// A call that changes a file, with the lines it adds and removes.
    Edit(&'static str, &'static str, u32, u32),
}

const EXPLORE: &[Explore] = &[
    Explore::Think("**Exploring the project**\n\nFirst, where titles come from and where the sidebar draws them."),
    Explore::Tool("Run command", "cd /Users/me/code/trail-app && git status --short"),
    Explore::Tool("Run command", "sed -n 165,260p src/shared/ui/ParticleText.tsx"),
    Explore::Tool("Search", "generateTitle|titleGen|generatedTitle"),
    Explore::Tool("Read", "src/features/sessions/model/session.ts"),
    Explore::Tool("List files", "src/features/sessions/ui/**/*.tsx"),
    Explore::Tool("Read", "src/features/sessions/ui/AgentTitle.tsx"),
    Explore::Tool("Read", "Cargo.toml"),
    Explore::Say("Titles come from `generateTitle` in `src/features/sessions/model/session.ts`; the sidebar draws them in `AgentTitle.tsx`."),
    Explore::Tool("Fetch", "https://developer.mozilla.org/en-US/docs/Web/CSS/mask-image"),
    Explore::Tool("Search the web", "css mask-image gradient text reveal"),
    Explore::Think("**Planning the change**\n\nA mask that sweeps left to right, keyed on the title so it only plays when it changes."),
    Explore::Edit("Edit", "src/features/sessions/ui/AgentTitle.tsx", 18, 4),
    Explore::Edit("Edit", "src/shared/ui/ParticleText.tsx", 42, 0),
    Explore::Edit("Write", "src/shared/ui/reveal.css", 27, 0),
    Explore::Tool("Run command", "npm run typecheck"),
    Explore::Tool("Run command", "npx vitest run src/features/sessions"),
    Explore::Say("The reveal is in. Checking the native side next."),
    Explore::Tool("Read", "src-tauri/src/main.rs"),
    Explore::Tool("Run command", "rg -n \"set_title\" src-tauri/src"),
    Explore::Tool("Read", "README.md"),
    Explore::Edit("Edit", "src-tauri/src/window.rs", 6, 2),
    Explore::Tool("Run command", "cargo test --manifest-path src-tauri/Cargo.toml"),
    Explore::Say("Native titles follow the same rule now."),
];

fn humanize(d: Duration) -> String {
    match d.as_secs() {
        0 => format!("{} ms", d.as_millis()),
        s @ 1..=59 => format!("{s} s"),
        s => format!("{} min {} s", s / 60, s % 60),
    }
}

/// Split text the way a model streams it: short runs of a few characters, ending at word breaks.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        if (c.is_whitespace() && end - start >= 3) || end - start >= 12 {
            out.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

const ANSWER: &str = "## How the app starts\n\nStartup happens in `src/main.rs`: it parses the flags, loads the config and opens the window.\n\n- **Flags** are parsed in `src/cli.rs` into a `Config`.\n- **Settings** come from `config.toml`; anything missing falls back to the defaults.\n- The **window** is created last, so a bad config fails fast.\n\n```rust\nfn main() -> anyhow::Result<()> {\n    let cfg = cli::parse_args();\n    let settings = Settings::load(&cfg.config_path)?;\n    app::run(settings)\n}\n```\n\nIf you want to change the startup order, `app::run` in `src/app.rs` is the place.";

const PROSE_THOUGHT: &str = "**Mapping the startup path**\n\nThe flags are parsed in `src/cli.rs` before anything else, so a *bad flag* never opens a window. I'll walk through it in order and show the config table.";

const PROSE: &str = "## How the app starts\n\nStartup lives in `src/main.rs`. It does three things, in order: it **parses the flags**, it **loads the settings**, and only then does it **open the window**, so a bad config fails before anything is drawn.\n\n### Flags and settings\n\n- **Flags** are parsed in `src/cli.rs` into a `Config`. Unknown flags are an error, not a warning.\n- **Settings** come from `config.toml`; anything missing falls back to the defaults in `src/settings.rs`.\n  - Paths in it may start with `~/`.\n  - A setting that fails to parse names its line.\n- The **window** is created last, from `src/app.rs`.\n\n### What each setting does\n\n| Setting | Default | What it does |\n| --- | --- | --- |\n| `theme` | `night` | Night or Paper |\n| `font_size` | `14.5` | Transcript text, in points |\n| `telemetry` | `false` | Never sent unless you turn it on |\n\n## The entry point\n\n```rust\nfn main() -> anyhow::Result<()> {\n    let cfg = cli::parse_args();\n    let settings = Settings::load(&cfg.config_path)?;\n    app::run(settings)\n}\n```\n\n> The order matters: a window opened before the settings load would flash the default theme.\n\n1. Read `src/cli.rs` first: it is short.\n2. Then `src/settings.rs`, which is where most changes land.\n3. Finally `src/app.rs`, for the window itself.\n\nThe folder `src/` holds all of it. For the config format itself, see [the TOML spec](https://toml.io).";

const PLAN: &str = "## Require a session on every route\n\n1. Add `require_session` middleware in `src/auth.rs`.\n2. Wrap the router in `src/routes.rs` with it, keeping `/health` public.\n3. Return `401` with a JSON body when the session is missing or expired.\n4. Add tests for an authenticated and an anonymous request.\n\nNo database changes.";

const LS_OUTPUT: &str = "total 48\ndrwxr-xr-x  8 me  staff   256 Oct  2 09:14 .\n-rw-r--r--  1 me  staff  1184 Oct  2 09:14 Cargo.toml\n-rw-r--r--  1 me  staff   912 Oct  2 09:14 README.md\ndrwxr-xr-x  5 me  staff   160 Oct  2 09:14 src\ndrwxr-xr-x  3 me  staff    96 Oct  2 09:14 tests";

const MAIN_RS: &str = "mod cli;\n\nfn main() {\n    let cfg = cli::parse_args();\n    app::run(cfg);\n}";

const TEST_OUTPUT: &str = "running 14 tests\n..............\ntest result: ok. 14 passed; 0 failed; 0 ignored; finished in 0.42s";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scripts_follow_keywords() {
        assert_eq!(Script::parse("explain the startup", false), Script::Answer);
        assert_eq!(Script::parse("Run the tools please", false), Script::Tools);
        assert_eq!(Script::parse("mock:long 2s", false), Script::Long(Duration::from_secs(2)));
        assert_eq!(Script::parse("mock:long", false), Script::Long(Duration::from_secs(30)));
        assert_eq!(Script::parse("how long is it?", false), Script::Answer, "bare `long` is just a word");
        assert_eq!(Script::parse("mock:stream 2m", false), Script::Stream(Duration::from_secs(120)));
        assert_eq!(Script::parse("stream the logs", false), Script::Answer);
        assert_eq!(Script::parse("mock:explore 5s", false), Script::Explore(Duration::from_secs(5)));
        assert_eq!(Script::parse("mock:task 2s", false), Script::Task(Duration::from_secs(2)));
        assert_eq!(Script::parse("mock:dev", false), Script::Dev);
        assert_eq!(Script::parse("explore the repo", false), Script::Answer, "bare `explore` is just a word");
        assert_eq!(Script::parse("mock:prose", false), Script::Prose);
        assert_eq!(Script::parse("purple prose", false), Script::Answer, "bare `prose` is just a word");
        assert_eq!(Script::parse("send subagents 300ms", false), Script::Agents(Some(Duration::from_millis(300))));
        assert_eq!(Script::parse("ask a question", false), Script::Questions);
        assert_eq!(Script::parse("mock:permission", false), Script::Permission);
        assert_eq!(Script::parse("anything", true), Script::Plan);
        assert_eq!(Script::parse("Error!", false), Script::Error);
        assert_eq!(Script::parse("mock:limit 5s", false), Script::Limit(Duration::from_secs(5)));
        assert_eq!(Script::parse("mock:limit", false), Script::Limit(Duration::from_secs(5)));
        assert_eq!(Script::parse("the rate limit", false), Script::Answer, "bare `limit` is just a word");
        assert_eq!(Script::parse("mock:consult mock:long 2s", false), Script::Delegate { wait: true }, "the first keyword is the parent's");
        assert_eq!(Script::parse("mock:delegate", false), Script::Delegate { wait: false });
        assert_eq!(Script::parse("consult a friend", false), Script::Answer);
        assert_eq!(Script::parse("[Trek] Sub-agent “x” failed: error", false), Script::Wake);
        assert_eq!(Script::parse("mock:verify the notes", false), Script::Verify);
        assert_eq!(Script::parse("mock:design an error budget", false), Script::Design, "the first keyword decides");
        assert_eq!(Script::parse("mock:judge these: Design A has an error path", false), Script::Judge);
    }

    #[test]
    fn what_trek_adds_to_a_message_decides_the_script() {
        use trek_core::orchestrate::{Consult, Consultant, Style, consult_prompt};
        // Whatever the words say ("plan", "error"), a request to restate is played as one.
        let restate = trek_core::restate::with_restate("plan the error pages");
        assert_eq!(Script::parse(&restate, false), Script::Restate);
        assert_eq!(Script::parse(&restate, true), Script::Restate, "in plan mode too");
        let c = Consult { consultants: vec![Consultant { agent: trek_core::AgentId::Direct(PROVIDER.into()), model: "mock-deep".into(), effort: trek_core::Effort::High }], style: Style::Arena, implement: false, judge: None };
        assert_eq!(Script::parse(&consult_prompt("rate-limit the webhooks", &c, |_| "Deep".into()), false), Script::Arena);
        let advise = Consult { style: Style::Advise, ..c };
        assert_eq!(Script::parse(&consult_prompt("explain it", &advise, |_| "Deep".into()), false), Script::Answer, "an ordinary consult is the words' to decide");
        assert_eq!(Script::parse("Set up verification following `/x/skills/create-verification-skill/SKILL.md`", false), Script::SetupVerification);
        assert_eq!(Script::parse("Maintain it following `/x/maintain-verification-skill/SKILL.md`", false), Script::MaintainVerification);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("10"), None);
        // Huge values are capped rather than overflowing.
        assert_eq!(parse_duration("999999999999999999m"), Some(MAX_DURATION));
        assert_eq!(parse_duration("99999999999s"), Some(MAX_DURATION));
        assert_eq!(parse_duration("99999999999999999999999s"), None, "not a u64");
    }

    #[test]
    fn titles_follow_the_script() {
        assert_eq!(title("explain the startup"), "How the app starts");
        assert_eq!(title("mock:long 5s"), "Run the full test suite");
        assert_eq!(title("ask a question"), "Choose a database");
        assert_eq!(title("mock:write"), "Add a note");
        assert_eq!(title("write it down"), "How the app starts", "only `mock:write` writes");
    }

    #[test]
    fn tokens_rebuild_the_text() {
        for text in [ANSWER, PLAN, PROSE, PROSE_THOUGHT, "short", "ünïcödé words stream fine"] {
            assert_eq!(tokens(text).concat(), text);
            assert!(tokens(text).iter().all(|t| !t.is_empty()));
        }
    }

    struct Live {
        commands: async_channel::Sender<Command>,
        events: async_channel::Receiver<AgentEvent>,
    }

    impl Live {
        fn start(hand_holding: HandHolding, plan: bool) -> Live {
            set_pace(0.);
            let config = SessionConfig {
                agent: trek_core::AgentId::Direct(PROVIDER.into()),
                cwd: PathBuf::from("/tmp"),
                model: None,
                effort: trek_core::Effort::Low,
                hand_holding,
                plan,
                read_only: false,
                resume: None,
                resume_at: None,
                fork: false,
                recap: None,
                fast: None,
                mcp_servers: vec![],
                instructions: None,
                read_dirs: vec![],
            };
            Live::of(config)
        }

        fn of(config: SessionConfig) -> Live {
            set_pace(0.);
            let h = crate::start(config);
            Live { commands: h.commands, events: h.events }
        }

        async fn send(&self, cmd: Command) {
            self.commands.send(cmd).await.unwrap();
        }

        async fn prompt(&self, text: &str) {
            self.send(Command::Prompt { text: text.into(), images: vec![] }).await;
        }

        /// Events up to and including the first one `stop` accepts.
        async fn until(&self, stop: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
            let mut out = vec![];
            loop {
                let ev = tokio::time::timeout(Duration::from_secs(10), self.events.recv()).await.expect("mock stalled").expect("mock exited");
                let done = stop(&ev);
                out.push(ev);
                if done {
                    return out;
                }
            }
        }

        async fn turn(&self) -> Vec<AgentEvent> {
            self.until(|e| matches!(e, AgentEvent::TurnComplete { .. })).await
        }
    }

    fn text(events: &[AgentEvent]) -> String {
        events.iter().filter_map(|e| if let AgentEvent::TextDone(t) = e { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    fn config(resume: Option<&str>, at: Option<&str>, fork: bool, recap: Option<&str>) -> SessionConfig {
        SessionConfig {
            agent: trek_core::AgentId::Direct(PROVIDER.into()),
            cwd: PathBuf::from("/tmp"),
            model: None,
            effort: trek_core::Effort::Low,
            hand_holding: HandHolding::Auto,
            plan: false,
            read_only: false,
            resume: resume.map(String::from),
            resume_at: at.map(String::from),
            fork,
            recap: recap.map(String::from),
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
        }
    }

    #[test]
    fn sessions_resume_partway_fork_or_fall_back_to_a_recap() {
        let said = |m: &str, t: &str| (m.to_string(), t.to_string());
        HISTORY.lock().unwrap().insert("s".into(), vec![said("m1", "one"), said("m2", "two"), said("m2", "steered"), said("m3", "three")]);
        // A fork through a turn copies the history up to it; the session is left as it was.
        let (fork, at, recap) = open_session(&config(Some("s"), Some("m2"), true, Some("recap")));
        assert!(fork != "s" && at.as_deref() == Some("m2") && recap.is_none());
        assert_eq!(remembered(&fork), ["one", "two", "steered"]);
        assert_eq!(remembered("s").len(), 4);
        // In place: the same session, cut back.
        let (id, at, _) = open_session(&config(Some("s"), Some("m1"), false, None));
        assert_eq!((id.as_str(), at.as_deref()), ("s", Some("m1")));
        assert_eq!(remembered("s"), ["one"]);
        // A point it doesn't have: a new session, with the recap to go with the first message.
        let (id, at, recap) = open_session(&config(Some("s"), Some("gone"), false, Some("User: one")));
        assert!(id != "s" && at.is_none() && recap.as_deref() == Some("User: one"));
        assert!(remembered(&id).is_empty());
        // Resumed whole, and new.
        assert_eq!(open_session(&config(Some("s"), None, false, None)).0, "s");
        assert!(open_session(&config(None, None, false, None)).0.starts_with("mock-session-"));
    }

    #[test]
    fn a_session_taken_back_remembers_only_what_was_kept() {
        trek_core::runtime().block_on(async {
            set_pace(0.);
            let open = |c: SessionConfig| {
                let h = crate::start(c);
                Live { commands: h.commands, events: h.events }
            };
            let m = open(config(None, None, false, None));
            let started = m.until(|e| matches!(e, AgentEvent::Started { .. })).await;
            let Some(AgentEvent::Started { native_id, .. }) = started.last() else { panic!() };
            let mark = |events: &[AgentEvent]| events.iter().find_map(|e| if let AgentEvent::Mark(m) = e { Some(m.clone()) } else { None }).unwrap();
            m.prompt("apple").await;
            let first = mark(&m.turn().await);
            m.prompt("banana").await;
            m.turn().await;
            m.prompt("recall").await;
            assert_eq!(text(&m.turn().await), "I remember: apple | banana");
            m.send(Command::Shutdown).await;

            // Cut back to the first turn: the second is forgotten.
            let back = open(config(Some(native_id), Some(&first), false, None));
            let started = back.until(|e| matches!(e, AgentEvent::Mark(_))).await;
            assert!(started.contains(&AgentEvent::Started { native_id: native_id.clone(), model: Some("mock-swift".into()) }));
            back.prompt("recall").await;
            assert_eq!(text(&back.turn().await), "I remember: apple");

            // A new session primed with a recap remembers what the recap says.
            let recapped = open(config(None, None, false, Some("User: apple\n\nAssistant: OK\n\nUser: cherry")));
            recapped.prompt("recall").await;
            assert_eq!(text(&recapped.turn().await), "I remember: apple | cherry");
        });
    }

    #[test]
    fn plays_an_answer_with_streamed_text() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Supervised, false);
            let start = m.until(|e| matches!(e, AgentEvent::Context { .. })).await;
            assert!(matches!(&start[0], AgentEvent::Started { native_id, .. } if native_id.starts_with("mock-")));
            m.prompt("explain").await;
            let events = m.turn().await;
            let streamed: String = events.iter().filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
            assert_eq!(streamed, ANSWER);
            assert!(events.iter().filter(|e| matches!(e, AgentEvent::TextDelta(_))).count() > 20, "streams token by token");
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ReasoningDelta(_))));
            // The turn's tokens, on the session's model, just before it ends.
            let n = events.len();
            assert!(matches!(&events[n - 2], AgentEvent::Usage { model: Some(m), tokens, .. } if m == "mock-swift" && tokens.output > 0), "{:?}", &events[n - 2]);
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
        });
    }

    #[test]
    fn restating_says_it_back_and_does_nothing_else() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::FullAccess, false);
            m.prompt(&trek_core::restate::with_restate("Fix the flaky upload test.")).await;
            let events = m.turn().await;
            let said = text(&events);
            assert!(said.contains("in my own words") && said.contains("you'd like me to fix the flaky upload test.") && said.contains("I haven't changed anything"), "{said}");
            assert!(!events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { .. })), "nothing done yet");
        });
    }

    #[test]
    fn it_sets_up_a_verification_skill_then_verifies_with_its_cli() {
        let dir = std::env::temp_dir().join(format!("trek-mock-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        trek_core::runtime().block_on(async {
            let m = Live::of(SessionConfig { cwd: dir.clone(), ..config(None, None, false, None) });
            m.prompt("Set up a verification skill, following Trek's guide in `/x/skills/create-verification-skill/SKILL.md`.").await;
            let events = m.turn().await;
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: None })), "{events:?}");
        });
        // Trek finds what it made, and its CLI really runs.
        let found = trek_core::verification::find(&dir).expect("a verification skill");
        assert_eq!((found.name.as_str(), found.cli.as_deref()), ("verify-app", Some("./.agents/skills/verify-app/scripts/app")));
        assert!(found.dir.join("references/features/README.md").exists());
        let out = std::process::Command::new(found.dir.join("scripts/app")).arg("check").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("\"ok\":true"));
        // Told about it, a session checks its change with the CLI.
        let notes = trek_core::verification::instructions(&trek_core::verification::record(&found, None, None), &dir, &dir);
        trek_core::runtime().block_on(async {
            let m = Live::of(SessionConfig { cwd: dir.clone(), instructions: Some(notes), ..config(None, None, false, None) });
            m.prompt("mock:verify the notes change").await;
            let events = m.turn().await;
            assert!(
                events.iter().any(|e| matches!(e, AgentEvent::ToolStarted { title, detail, .. } if title == "Run command" && detail == "./.agents/skills/verify-app/scripts/app check --json")),
                "{events:?}"
            );
            // Without a skill, it says so.
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:verify").await;
            assert!(text(&m.turn().await).contains("no verification skill yet"));
            // Maintaining it adds what the Feature Map lacked.
            let m = Live::of(SessionConfig { cwd: dir.clone(), ..config(None, None, false, None) });
            m.prompt("Maintain it, following `/x/maintain-verification-skill/SKILL.md`.").await;
            m.turn().await;
        });
        assert!(std::fs::read_to_string(found.dir.join("references/features/README.md")).unwrap().contains("settings.md"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn its_own_models_cost_nothing_and_mock_cost_has_a_real_price() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let start = m.until(|e| matches!(e, AgentEvent::Context { .. })).await;
            assert!(start.contains(&AgentEvent::Billing(crate::Billing::Local)), "{start:?}");
            m.prompt("explain").await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::Usage { cost: None, .. })), "the mock's models have no price");
            m.prompt("mock:cost").await;
            let events = m.turn().await;
            assert!(events.contains(&AgentEvent::Billing(crate::Billing::Metered)));
            let Some(AgentEvent::Usage { model: Some(model), cost: Some(cost), .. }) = events.iter().find(|e| matches!(e, AgentEvent::Usage { .. })) else { panic!("{events:?}") };
            // 2,400 × $2 + 1,850 × $10 + 182,000 × $0.20 + 12,600 × $4 (1-hour writes), per million.
            assert_eq!(model, "claude-sonnet-5-5");
            assert!((cost.usd - (0.0048 + 0.0185 + 0.0364 + 0.0504)).abs() < 1e-9, "{cost:?}");
            m.prompt("mock:cost plan").await;
            assert!(m.turn().await.contains(&AgentEvent::Billing(crate::Billing::Plan(Some("Claude Max".into())))));
        });
    }

    #[test]
    fn permission_waits_for_the_user() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Supervised, false);
            m.prompt("mock:permission").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: None, .. }) = events.last() else { panic!("{events:?}") };
            m.send(Command::Respond { request_id: request_id.clone(), decision: Decision::Deny }).await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: false, .. })));
            assert!(text(&events).contains("won't run it"));
            // Full access: no prompt at all.
            m.send(Command::SetHandHolding(HandHolding::FullAccess)).await;
            m.prompt("permission again").await;
            let events = m.turn().await;
            assert!(!events.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })));
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: true, .. })));
        });
    }

    #[test]
    fn questions_take_answers() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("a question for you").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Questions(q)), .. }) = events.last() else { panic!("{events:?}") };
            assert_eq!(q.len(), 2);
            assert!(q[1].multi);
            m.send(Command::Answer { request_id: request_id.clone(), answers: vec![(q[0].question.clone(), "Postgres".into())] }).await;
            assert!(text(&m.turn().await).contains("**Postgres**"));
        });
    }

    #[test]
    fn plan_mode_ends_in_a_plan() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, true);
            m.prompt("add auth").await;
            let events = m.until(|e| matches!(e, AgentEvent::PermissionRequest { .. })).await;
            let Some(AgentEvent::PermissionRequest { request_id, prompt: Some(Prompt::Plan(plan)), .. }) = events.last() else { panic!("{events:?}") };
            assert!(plan.contains("require_session"));
            m.send(Command::Respond { request_id: request_id.clone(), decision: Decision::Allow }).await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::DiffStat { .. })));
            // Approving leaves plan mode: the next prompt is answered normally.
            m.prompt("thanks").await;
            assert!(!m.turn().await.iter().any(|e| matches!(e, AgentEvent::PermissionRequest { .. })));
        });
    }

    #[test]
    fn background_agents_finish_after_the_turn() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("subagents 30ms").await;
            let first = m.turn().await;
            let out = first.iter().find_map(|e| if let AgentEvent::Background(b) = e { Some(b.clone()) } else { None }).expect("a background set");
            assert_eq!(out.iter().map(|t| (t.kind, t.title.as_str())).collect::<Vec<_>>(), [(BackgroundKind::Agent, "Map the HTTP routes"), (BackgroundKind::Agent, "Audit error handling")]);
            let tasks = first.iter().filter(|e| matches!(e, AgentEvent::Task { description: Some(_), .. })).count();
            assert_eq!(tasks, 2);
            let rest = m.turn().await;
            assert!(rest.iter().any(|e| matches!(e, AgentEvent::TaskStep { title, .. } if title == "Read")), "{rest:?}");
            assert!(rest.contains(&AgentEvent::Background(vec![])));
            assert_eq!(rest.iter().filter(|e| matches!(e, AgentEvent::Task { done: Some(true), .. })).count(), 2);
            assert!(text(&rest).contains("reported back"));
        });
    }

    #[test]
    fn a_stopped_background_agent_gets_a_turn_saying_so() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("subagents 30s").await;
            let first = m.turn().await;
            let out = first.iter().find_map(|e| if let AgentEvent::Background(b) = e { Some(b.clone()) } else { None }).expect("a background set");
            m.send(Command::StopTask { id: out[0].id.clone() }).await;
            // As Claude Code does (recorded live): the call fails, and a turn of its own says so.
            let echo = m.turn().await;
            assert!(echo.contains(&AgentEvent::Task { id: out[0].id.clone(), description: None, activity: None, tool_uses: None, done: Some(false) }), "{echo:?}");
            assert!(text(&echo).contains("was stopped"), "{echo:?}");
        });
    }

    #[test]
    fn a_dev_server_runs_on_after_the_turn_and_stops_when_asked() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:server").await;
            let turn = m.turn().await;
            let Some(AgentEvent::Background(b)) = turn.iter().find(|e| matches!(e, AgentEvent::Background(_))) else { panic!("{turn:?}") };
            assert_eq!((b[0].kind, b[0].title.as_str(), b[0].readable, b[0].stoppable), (BackgroundKind::Shell, "npm run dev", true, true));
            let id = b[0].id.clone();
            assert!(text(&turn).contains("http://localhost:5173"));
            // Read between turns, with no turn of its own.
            m.send(Command::ReadTask { id: id.clone() }).await;
            let read = m.until(|e| matches!(e, AgentEvent::TaskOutput { .. })).await;
            assert!(matches!(read.last(), Some(AgentEvent::TaskOutput { id: i, output }) if *i == id && output.contains("ready in")), "{read:?}");
            assert!(!read.iter().any(|e| matches!(e, AgentEvent::TurnComplete { .. })));
            m.send(Command::StopTask { id: id.clone() }).await;
            assert_eq!(m.until(|e| matches!(e, AgentEvent::Background(_))).await.last(), Some(&AgentEvent::Background(vec![])));
            // Gone: nothing left to read.
            m.send(Command::ReadTask { id }).await;
            m.prompt("hello").await;
            assert!(!m.turn().await.iter().any(|e| matches!(e, AgentEvent::TaskOutput { .. })));
        });
    }

    #[test]
    fn a_session_s_background_work_ends_with_it() {
        let jobs = Jobs::default();
        jobs.add(BackgroundTask { id: "shell-1".into(), kind: BackgroundKind::Shell, title: "npm run dev".into(), call: None, readable: true, stoppable: true });
        drop(EndJobs(jobs.clone()));
        assert!(!jobs.running("shell-1"), "the loop printing for it stops");
    }

    #[test]
    fn a_turn_waits_on_its_own_sub_agent_as_it_works() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:task 30ms").await;
            let turn = m.turn().await;
            let started = turn.iter().position(|e| matches!(e, AgentEvent::ToolStarted { title, .. } if title == "Subagent")).expect("its row");
            let steps = turn.iter().filter(|e| matches!(e, AgentEvent::TaskStep { .. })).count();
            let done = turn.iter().position(|e| matches!(e, AgentEvent::Task { done: Some(true), .. })).expect("it finishes");
            let finished = turn.iter().position(|e| matches!(e, AgentEvent::ToolFinished { .. })).expect("its call returns");
            assert!(started < done && done < finished && steps == 3, "{turn:?}");
            assert!(!turn.iter().any(|e| matches!(e, AgentEvent::Background(_))), "in the foreground: nothing left running");
            assert!(text(&turn).contains("212 tests"));
        });
    }

    #[test]
    fn a_watcher_that_catches_something_starts_a_turn_of_its_own() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:watch 50ms").await;
            let turn = m.turn().await;
            assert!(turn.iter().any(|e| matches!(e, AgentEvent::Background(b) if b.len() == 1 && b[0].kind == BackgroundKind::Monitor)));
            // No message from anyone: the watcher's report starts the next turn.
            let woken = m.turn().await;
            let first = woken.iter().position(|e| matches!(e, AgentEvent::ReasoningDelta(_) | AgentEvent::TextDelta(_))).expect("output");
            assert!(woken[..first].contains(&AgentEvent::Background(vec![])), "the watcher ended before the turn: {woken:?}");
            assert!(text(&woken).contains("caught a failure"));
            assert!(matches!(woken.last(), Some(AgentEvent::TurnComplete { error: None })));
        });
    }

    #[test]
    fn long_turns_stop_on_interrupt_and_take_steering() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:long 300ms").await;
            m.prompt("use tabs").await;
            let events = m.turn().await;
            assert!(text(&events).contains("Noted — use tabs"));
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
            // As long as a prompt can ask for: the turn runs (no overflow) until stopped.
            m.prompt("mock:long 99999999999s").await;
            m.until(|e| matches!(e, AgentEvent::ToolStarted { .. })).await;
            m.send(Command::Interrupt).await;
            let events = m.turn().await;
            assert_eq!(events.last(), Some(&AgentEvent::TurnComplete { error: Some("Interrupted".into()) }));
        });
    }

    #[test]
    fn streams_one_answer_for_as_long_as_asked() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let start = std::time::Instant::now();
            m.prompt("mock:stream 300ms").await;
            let events = m.turn().await;
            assert!(start.elapsed() >= Duration::from_millis(300));
            let streamed: String = events.iter().filter_map(|e| if let AgentEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
            assert!(streamed.starts_with("## Part 1\n\n## How the app starts"), "{streamed}");
            assert_eq!(text(&events), streamed, "one message, sent whole at the end");
            assert!(events.iter().filter(|e| matches!(e, AgentEvent::TextDelta(_))).count() > 10);
        });
    }

    #[test]
    fn explores_with_tools_for_as_long_as_asked() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let start = std::time::Instant::now();
            m.prompt("mock:explore 300ms").await;
            let events = m.turn().await;
            assert!(start.elapsed() >= Duration::from_millis(300));
            let tools: Vec<&str> = events.iter().filter_map(|e| if let AgentEvent::ToolStarted { title, .. } = e { Some(title.as_str()) } else { None }).collect();
            assert!(tools.len() >= 8, "{tools:?}");
            assert_eq!(tools.len(), events.iter().filter(|e| matches!(e, AgentEvent::ToolFinished { ok: true, .. })).count());
            // Messages between the groups, and the last word at the end.
            assert!(text(&events).contains("Titles come from"));
            assert!(text(&events).ends_with("the tests pass."));
            // Edits say how many lines they changed.
            if tools.len() > 13 {
                assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolLines { added: 18, removed: 4, .. })));
            }
        });
    }

    #[test]
    fn consults_a_sub_agent_through_trek() {
        use std::io::{BufReader, Write as _};
        let dir = std::env::temp_dir().join(format!("trek-mock-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sock");
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // Trek's side: one delegate_task, answered as a finished sub-agent.
        let trek = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut w = stream;
            let hello: serde_json::Value = serde_json::from_str(&trek_ipc::read_frame(&mut r, trek_ipc::MAX_FRAME).unwrap().unwrap()).unwrap();
            assert_eq!(trek_ipc::parse_hello(&hello).map(|h| (h.1.to_string(), h.2.to_string())), Some(("tok".into(), "ses".into())));
            w.write_all(trek_ipc::encode(&serde_json::json!({"ok": true})).as_bytes()).unwrap();
            let req: serde_json::Value = serde_json::from_str(&trek_ipc::read_frame(&mut r, trek_ipc::MAX_FRAME).unwrap().unwrap()).unwrap();
            let answer = serde_json::json!({"id": "child-1", "status": "done", "result": "Use a cache."});
            w.write_all(trek_ipc::encode(&trek_ipc::reply(&req["id"], Ok(answer))).as_bytes()).unwrap();
            req
        });
        trek_core::runtime().block_on(async {
            set_pace(0.);
            let env = vec![(trek_ipc::ENV_SOCKET.to_string(), path.display().to_string()), (trek_ipc::ENV_TOKEN.into(), "tok".into()), (trek_ipc::ENV_SESSION.into(), "ses".into())];
            let server = crate::McpServer { name: ORCHESTRATE_SERVER.into(), command: "trek-mcp".into(), args: vec!["orchestrate".into()], env, tool_timeout_secs: None };
            let h = crate::start(SessionConfig { mcp_servers: vec![server], ..config(None, None, false, None) });
            let m = Live { commands: h.commands, events: h.events };
            m.prompt("mock:consult check the cache").await;
            let events = m.turn().await;
            assert!(events.contains(&AgentEvent::ToolStarted { id: "mock-tool-1".into(), title: "mcp__trek-orchestrate__delegate_task".into(), detail: "Second opinion".into() }));
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: true, output, .. } if output.contains("child-1"))));
            assert!(text(&events).contains("The second opinion is in: Use a cache."), "{}", text(&events));
            // Woken with a result, it answers in a line.
            m.prompt(&trek_core::orchestrate::wake_text(&[trek_core::orchestrate::Report {
                id: "child-1".into(),
                title: "Second opinion".into(),
                model: "Mock Swift".into(),
                outcome: trek_core::orchestrate::Outcome::Failed("The mock agent hit an error".into()),
            }]))
            .await;
            assert!(text(&m.turn().await).starts_with("The sub-agent reported back"), "not read as `mock:error`");
        });
        let req = trek.join().unwrap();
        assert_eq!(req["method"], "delegate_task");
        assert_eq!(req["params"]["prompt"], "check the cache");
        assert_eq!(req["params"]["wait"], true);
        assert_eq!(req["params"]["mode"], "advise");
        let _ = std::fs::remove_dir_all(dir);

        // Without Trek's tools the call fails, and the turn says so.
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:delegate").await;
            let events = m.turn().await;
            assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolFinished { ok: false, .. })));
            assert!(text(&events).contains("couldn't get a second opinion"));
        });
    }

    #[test]
    fn errors_fail_the_turn() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            m.prompt("mock:error").await;
            let events = m.turn().await;
            assert!(matches!(events.last(), Some(AgentEvent::TurnComplete { error: Some(_), .. })));
        });
    }

    #[test]
    fn a_limit_holds_until_it_resets() {
        trek_core::runtime().block_on(async {
            let m = Live::start(HandHolding::Auto, false);
            let started = m.until(|e| matches!(e, AgentEvent::Started { .. })).await;
            let Some(AgentEvent::Started { native_id, .. }) = started.last() else { panic!() };
            let before = trek_core::store::now_ms();
            m.prompt("mock:limit 5s").await;
            let events = m.turn().await;
            let Some(AgentEvent::LimitReached { message, resets_at: Some(at), scope }) = events.iter().find(|e| matches!(e, AgentEvent::LimitReached { .. })) else { panic!("{events:?}") };
            assert!(*at >= before + 5_000 && *at < before + 7_000);
            assert_eq!(*scope, crate::LimitScope::Session);
            assert!(message.starts_with("You've hit your session limit · resets "));
            assert_eq!(events.last(), Some(&AgentEvent::TurnComplete { error: Some(message.clone()) }));
            // Anything else meets the same limit until it resets.
            m.prompt("explain the startup").await;
            let again = m.turn().await;
            assert!(again.contains(&AgentEvent::LimitReached { message: message.clone(), resets_at: Some(*at), scope: crate::LimitScope::Session }));
            lift_limit(native_id);
            m.prompt("explain the startup").await;
            assert!(matches!(m.turn().await.last(), Some(AgentEvent::TurnComplete { error: None, .. })));
        });
    }
}
