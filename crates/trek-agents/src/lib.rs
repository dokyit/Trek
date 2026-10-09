//! Live agent sessions. Every backend (vendor CLI, ACP agent, direct API) is driven through the
//! same channel pair: the UI sends [`Command`]s and receives normalized [`AgentEvent`]s.
//! Sessions run on `trek_core::runtime()`; channels are executor-agnostic.

mod title;
pub use title::{generate_commit_message, generate_title};
mod acp;
mod claude;
mod codex;
mod direct;
pub mod limits;
pub mod mcp_check;
pub mod mock;
mod opencode;
mod status;

pub use acp::{AcpInfo, acp_probe};
pub use codex::list_models as codex_models;
pub use limits::{Limit, LimitScope};
pub use opencode::share_history as share_opencode_history;
pub use status::{AgentStatus, CommandKind, ResetCredit, SlashCommand, UsageLimit, claude_commands, claude_status, codex_commands, codex_consume_reset, codex_status, devin_status};

use std::collections::HashMap;
use std::path::PathBuf;
use trek_core::{AgentId, Effort, HandHolding, TokenUsage, UsageCost};

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentId,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Effort,
    pub hand_holding: HandHolding,
    pub plan: bool,
    /// Nothing may change files, whatever `hand_holding` lets through (a sub-agent that only
    /// advises): Claude loses its editing tools, ACP agents work in plan mode and can't write
    /// through Trek, Codex keeps the read-only sandbox `Supervised` gives it.
    pub read_only: bool,
    /// The agent's own session id to resume.
    pub resume: Option<String>,
    /// Resume only the conversation up to this point of `resume` (an id from `AgentEvent::Mark`):
    /// everything after it is dropped.
    pub resume_at: Option<String>,
    /// Open `resume` as a copy (a fork), leaving the session itself as it is.
    pub fork: bool,
    /// What was said so far, for a session that starts afresh in a conversation already under
    /// way: the first message carries it. With `resume` set it's the fallback, used if the agent
    /// can't take its session back to `resume_at`.
    pub recap: Option<String>,
    /// Fast mode: Claude `fastMode`, or the Codex service tier to use.
    pub fast: Option<String>,
    /// Extra MCP servers to attach to the session, on top of the agent's own config.
    pub mcp_servers: Vec<McpServer>,
    /// What Trek tells the agent about the project (its verification skill): Claude Code gets it
    /// as part of its system prompt (`notes_in_system_prompt`), other agents with the session's
    /// first message, so Trek gives a session that resumes only notes it hasn't told the thread.
    pub instructions: Option<String>,
    /// Folders outside `cwd` the agent may read without asking (the guides Trek ships): Claude
    /// Code gets them with `--add-dir`; Codex reads anywhere already.
    pub read_dirs: Vec<PathBuf>,
}

/// An MCP server Trek adds to a session.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    /// How long one of its tool calls may take, where the agent caps that itself (Codex: a
    /// minute unless told). Trek's sub-agent tools can wait on a sub-agent for many minutes.
    pub tool_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum McpTransport {
    /// A command the agent starts and talks to over its stdio.
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
    /// A server already running at `url` (streamable HTTP), sent `headers` with each request.
    Http { url: String, headers: Vec<(String, String)> },
}

impl McpServer {
    pub fn stdio(name: impl Into<String>, command: impl Into<String>, args: Vec<String>, env: Vec<(String, String)>) -> Self {
        Self { name: name.into(), transport: McpTransport::Stdio { command: command.into(), args, env }, tool_timeout_secs: None }
    }

    pub fn http(name: impl Into<String>, url: impl Into<String>, headers: Vec<(String, String)>) -> Self {
        Self { name: name.into(), transport: McpTransport::Http { url: url.into(), headers }, tool_timeout_secs: None }
    }

    /// The environment a stdio server is started with (none for a remote one).
    pub fn env(&self) -> &[(String, String)] {
        match &self.transport {
            McpTransport::Stdio { env, .. } => env,
            McpTransport::Http { .. } => &[],
        }
    }

    pub fn is_http(&self) -> bool {
        matches!(self.transport, McpTransport::Http { .. })
    }

    /// `{command, args, env}` or `{type: "http", url, headers}`: the shape Claude's `mcpServers`
    /// takes (Codex's differs for HTTP, see `codex::codex_mcp_servers`).
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let map = |pairs: &[(String, String)]| -> serde_json::Map<String, serde_json::Value> {
            pairs.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect()
        };
        match &self.transport {
            McpTransport::Stdio { command, args, env } => serde_json::json!({ "command": command, "args": args, "env": map(env) }),
            McpTransport::Http { url, headers } => serde_json::json!({ "type": "http", "url": url, "headers": map(headers) }),
        }
    }
}

/// `{name: {…}, ...}` for a set of servers.
pub(crate) fn mcp_servers_json(servers: &[McpServer]) -> serde_json::Value {
    serde_json::Value::Object(servers.iter().map(|s| (s.name.clone(), s.to_json())).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    AllowForSession,
    Deny,
}

/// Something richer than allow/deny that the agent put to the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    /// Multiple-choice questions (Claude's AskUserQuestion).
    Questions(Vec<Question>),
    /// A plan to approve before the agent starts changing things (Claude's ExitPlanMode), as Markdown.
    Plan(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub question: String,
    pub header: String,
    /// `(label, description)`
    pub options: Vec<(String, String)>,
    pub multi: bool,
    /// The answer is a secret (a token, a password): it must not be shown or kept.
    pub secret: bool,
}

/// Who pays for a session's tokens. Cost figures only mean money for `Metered` sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Billing {
    /// A subscription login ("Claude Max", "ChatGPT Plus"; `None` when the plan isn't named):
    /// usage counts against the plan's limits, and per-token costs are API-price estimates.
    Plan(Option<String>),
    /// Charged per token: an API key, or a cloud account (Bedrock, Vertex).
    Metered,
    /// A model running on this Mac.
    Local,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Answers to a `Prompt::Questions` request: `(question, chosen labels or free text)`.
    Answer { request_id: String, answers: Vec<(String, String)> },
    /// A user message; `images` are local image files attached before the text.
    Prompt { text: String, images: Vec<PathBuf> },
    Interrupt,
    Respond { request_id: String, decision: Decision },
    SetHandHolding(HandHolding),
    SetModel { model: String, effort: Effort },
    /// Settings a session is started with (`SessionConfig::plan`, `fast`, `effort`), for one that
    /// can't be restarted to take them (it has work running in the background). Agents that read
    /// them per turn, or can be told mid-session, take them now; the others with their next session.
    SetModes { plan: bool, fast: Option<String>, effort: Effort },
    /// Read the end of a background task's output (`BackgroundTask::readable`); the answer
    /// comes back as `AgentEvent::TaskOutput`. Starts no turn.
    ReadTask { id: String },
    /// Stop a background task (`BackgroundTask::stoppable`).
    StopTask { id: String },
    Shutdown,
}

/// Work an agent left running between turns: a shell command (a dev server, a test watcher, a
/// browser session), a `Monitor` watching one, or a sub-agent of its own. It outlives the turn
/// that started it, and when it reports, the agent may take a turn by itself.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundTask {
    /// The agent's id for it (what `Command::ReadTask` and `Command::StopTask` take).
    pub id: String,
    pub kind: BackgroundKind,
    /// What it is, in the agent's words: the command, or the task's description.
    pub title: String,
    /// The tool call that started it, when the agent says.
    pub call: Option<String>,
    /// Its output can be read while it runs (`Command::ReadTask`).
    pub readable: bool,
    /// The agent can stop it on request (`Command::StopTask`).
    pub stoppable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundKind {
    /// A shell command left running.
    Shell,
    /// A command whose output the agent watches, an event at a time (Claude's `Monitor`).
    Monitor,
    /// One of the agent's own sub-agents, working on while the agent waits for its report.
    Agent,
    /// Anything else (a workflow, a long MCP call).
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// The agent is up; `native_id` resumes it later.
    Started { native_id: String, model: Option<String> },
    /// Streaming assistant text, appended to the current message.
    TextDelta(String),
    /// The final text of the current assistant message (replaces streamed text).
    TextDone(String),
    ReasoningDelta(String),
    ToolStarted { id: String, title: String, detail: String },
    ToolFinished { id: String, output: String, ok: bool },
    /// Lines a tool call that changed a file added and removed (sent once known; a later report
    /// for the same call replaces it).
    ToolLines { id: String, added: u32, removed: u32 },
    /// The agent needs the user: a yes/no approval, or (`prompt`) a question or a plan to review.
    PermissionRequest { request_id: String, title: String, detail: String, prompt: Option<Prompt> },
    /// The agent settled a request on its own (it timed out, or the turn moved on): its card goes.
    PermissionResolved { request_id: String },
    /// A diff stat for the turn, when the agent reports one.
    DiffStat { additions: i64, deletions: i64 },
    /// A turn ended (`error`: why it failed, or "Interrupted"). What it cost came before it, in
    /// its `Usage` reports.
    TurnComplete { error: Option<String> },
    /// Tokens currently in the context window, and the window size.
    Context { used: u64, window: u64 },
    /// Tokens the running turn used on `model` (`None`: the session's own), sent as the agent
    /// reports them, before the turn's `TurnComplete`. A turn may send several: one per model
    /// it used, or one per request. `cost`: what they cost at API prices, as the agent said or
    /// as priced per request (`trek_core::pricing`); `None` when no price is known here.
    Usage { model: Option<String>, tokens: TokenUsage, cost: Option<UsageCost> },
    /// How the session is billed, once the agent has said which login it uses.
    Billing(Billing),
    /// A sub-agent (keyed by the tool call that launched it) started, moved on, or finished.
    Task { id: String, description: Option<String>, activity: Option<String>, tool_uses: Option<u64>, done: Option<bool> },
    /// A tool call a sub-agent made (keyed by the call that launched the sub-agent), for its
    /// row's live activity.
    TaskStep { task: String, title: String, detail: String },
    /// The work the agent has running in the background now, all of it: it replaces the last
    /// set. A turn can end with some still running.
    Background(Vec<BackgroundTask>),
    /// The end of background task `id`'s output, as it stands (the answer to `Command::ReadTask`).
    TaskOutput { id: String, output: String },
    /// The slash commands the agent offers in this session (replaces any earlier list).
    Commands(Vec<SlashCommand>),
    /// The effort levels the session's `model` offers (they can change with the model: OpenCode
    /// 2's do), and the one it's on. Sent as the session starts and after a model switch.
    Efforts { model: String, efforts: Vec<Effort>, effort: Option<Effort> },
    /// Something the user should know that isn't an error (the transcript shows it as a note).
    Notice(String),
    /// The latest point the session can be taken back to, for `SessionConfig::resume_at`: the
    /// last message's id (Claude Code) or the last finished turn's (Codex).
    Mark(String),
    /// The agent stopped at a usage limit (5-hour, weekly, a model's, or a provider's rate limit).
    /// `resets_at`: unix ms, when the agent said or it could be worked out. The turn then ends
    /// with an error saying the same.
    LimitReached { message: String, resets_at: Option<i64>, scope: LimitScope },
    /// How much of one of the plan's usage windows is used (0–100) and when it resets (unix ms),
    /// as the agent reports it while it works. Not a limit reached: the turn goes on.
    LimitUsed { scope: LimitScope, percent: f32, resets_at: Option<i64> },
    /// Something went wrong. It ends no turn by itself (an unreadable image, a refused model
    /// change): a turn that fails says so as it completes, or its session ends.
    Error(String),
    Exited,
}

pub struct SessionHandle {
    pub commands: async_channel::Sender<Command>,
    pub events: async_channel::Receiver<AgentEvent>,
    /// What the session's agent processes write to stderr (the IDE's Output panel shows it).
    pub log: SessionLog,
}

/// The last lines a session's agent processes wrote to stderr, oldest first.
#[derive(Clone, Default, Debug)]
pub struct SessionLog(std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>);

/// Lines a `SessionLog` keeps.
const SESSION_LOG_LINES: usize = 2000;

impl SessionLog {
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push(&self, line: String) {
        let mut l = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        l.push_back(line);
        while l.len() > SESSION_LOG_LINES {
            l.pop_front();
        }
    }
}

tokio::task_local! {
    /// The log of the session whose task is running: the processes it starts write there.
    static SESSION_LOG: SessionLog;
}

/// Whether `agent` can resume its session partway (`SessionConfig::resume_at`) and fork it, so a
/// rewind leaves the agent knowing exactly what the transcript keeps. Others start a new session
/// with a recap. (The mock's `mock-recap` model plays an agent that can't.)
pub fn resumes_partway(agent: &AgentId, model: Option<&str>) -> bool {
    match agent {
        AgentId::ClaudeCode | AgentId::Codex => true,
        AgentId::Direct(p) if trek_core::catalog::is_mock(p) => model != Some(mock::RECAP_MODEL),
        _ => false,
    }
}

/// Where session `session` of `agent` stands now (`ResumePoint::after` for a message sent next),
/// read from the agent's own files: for threads whose point Trek doesn't know yet (imported, or
/// kept by an older Trek). Reads the session from the start: run it off the main thread.
pub fn session_tail(agent: &AgentId, session: &str) -> Option<String> {
    match agent {
        AgentId::ClaudeCode => trek_core::import::claude::last_message(session),
        AgentId::Codex => trek_core::import::codex::last_turn(session),
        AgentId::Direct(p) if trek_core::catalog::is_mock(p) => mock::last_mark(session),
        _ => None,
    }
}

/// The first message of a session that starts afresh in a conversation already under way.
pub(crate) fn recap_prompt(recap: &str, text: &str) -> String {
    format!(
        "This conversation began in an earlier session that you can't see. Here is a recap of it, oldest first, so you can carry on where it left off:\n\n<recap>\n{recap}\n</recap>\n\n{text}"
    )
}

/// `commands`, with `recap` put in front of the first message.
pub(crate) fn recap_first(commands: async_channel::Receiver<Command>, recap: String) -> async_channel::Receiver<Command> {
    first_prompt(commands, move |text| recap_prompt(&recap, text))
}

/// Whether `agent` takes the project notes (`SessionConfig::instructions`) in its system prompt,
/// every session (Claude Code; the mock reads them from the config). Others get them in a
/// message.
pub fn notes_in_system_prompt(agent: &AgentId) -> bool {
    matches!(agent, AgentId::ClaudeCode) || matches!(agent, AgentId::Direct(p) if trek_core::catalog::is_mock(p))
}

/// A session's first message, with what Trek tells the agent about the project.
pub(crate) fn instructions_prompt(instructions: &str, text: &str) -> String {
    format!("<trek-project-notes>\n{}\n</trek-project-notes>\n\n{text}", instructions.trim())
}

/// `commands`, with the first message rewritten by `f`.
fn first_prompt(commands: async_channel::Receiver<Command>, f: impl FnOnce(&str) -> String + Send + 'static) -> async_channel::Receiver<Command> {
    let (tx, rx) = async_channel::unbounded();
    trek_core::runtime().spawn(async move {
        let mut f = Some(f);
        while let Ok(mut cmd) = commands.recv().await {
            if let Command::Prompt { text, .. } = &mut cmd {
                if let Some(f) = f.take() {
                    *text = f(text);
                }
            }
            if tx.send(cmd).await.is_err() {
                break;
            }
        }
    });
    rx
}

/// Start a session for `config.agent`.
pub fn start(config: SessionConfig) -> SessionHandle {
    let (cmd_tx, cmd_rx) = async_channel::unbounded();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    let events = ev_tx.clone();
    // A new session gets the recap with its first message; one that resumes has it as a fallback.
    let cmd_rx = match (&config.recap, &config.resume) {
        (Some(recap), None) => recap_first(cmd_rx, recap.clone()),
        _ => cmd_rx,
    };
    // The project notes go with the first message, unless the agent has them in its system
    // prompt. A session that resumes gets only notes its thread wasn't told (Trek leaves out
    // the rest).
    let cmd_rx = match &config.instructions {
        Some(notes) if !notes_in_system_prompt(&config.agent) => {
            let notes = notes.clone();
            first_prompt(cmd_rx, move |text| instructions_prompt(&notes, text))
        }
        _ => cmd_rx,
    };
    let log = SessionLog::default();
    supervise(
        SESSION_LOG.scope(log.clone(), async move {
            match &config.agent {
                AgentId::ClaudeCode => claude::run(config, cmd_rx, events).await,
                AgentId::Codex => codex::run(config, cmd_rx, events).await,
                AgentId::Direct(p) if trek_core::catalog::is_mock(p) => mock::run(config, cmd_rx, events).await,
                AgentId::Direct(_) => direct::run(config, cmd_rx, events).await,
                AgentId::Acp(_) | AgentId::OpenCode | AgentId::Droid => acp::run(config, cmd_rx, events).await,
            }
        }),
        ev_tx,
    );
    SessionHandle { commands: cmd_tx, events: ev_rx, log }
}

/// Run a session, then report how it ended: its error, if any, and `Exited`. A session that
/// panics ends the same way, so its thread stops working instead of waiting on it forever.
fn supervise(session: impl Future<Output = anyhow::Result<()>> + Send + 'static, events: async_channel::Sender<AgentEvent>) {
    trek_core::runtime().spawn(async move {
        let result = tokio::spawn(session).await.unwrap_or_else(|e| Err(anyhow::anyhow!("the agent session crashed: {e}")));
        if let Err(e) = result {
            let _ = events.send(AgentEvent::Error(format!("{e:#}"))).await;
        }
        let _ = events.send(AgentEvent::Exited).await;
    });
}

/// Count `+`/`-` lines in a unified diff.
pub fn diff_stat(diff: &str) -> (i64, i64) {
    let mut add = 0;
    let mut del = 0;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            add += 1;
        } else if line.starts_with('-') {
            del += 1;
        }
    }
    (add, del)
}

/// Where a step of an agent's running plan (to-do list) stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Pending,
    Active,
    Done,
}

/// A plan update as a tool row: `(detail, output)`. The detail names the step in progress; the
/// output is the whole checklist.
pub(crate) fn plan_row(steps: &[(String, Step)]) -> (String, String) {
    let done = steps.iter().filter(|(_, s)| *s == Step::Done).count();
    let detail = match steps.iter().find(|(_, s)| *s == Step::Active) {
        Some((text, _)) => text.clone(),
        None if done == steps.len() => format!("All {} steps done", steps.len()),
        None => format!("{done} of {} steps done", steps.len()),
    };
    let output = steps
        .iter()
        .map(|(text, s)| {
            let mark = match s {
                Step::Done => "✓",
                Step::Active => "→",
                Step::Pending => "○",
            };
            format!("{mark} {text}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    (clip(&detail, 200), output)
}

/// Said when an agent can't reopen a saved conversation and starts over.
pub(crate) fn lost_session(agent: &str) -> AgentEvent {
    AgentEvent::Notice(format!("{agent} couldn't reopen this conversation, so it continues in a new session without the earlier context."))
}

/// A plan's title for its row: its first non-empty line, without the Markdown heading marks.
pub(crate) fn plan_title(plan: &str) -> String {
    clip(plan.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim_start_matches('#').trim(), 200)
}

/// The longest line of an agent's protocol stream that's read: far more than any message a
/// CLI sends (a tool's whole output in one JSON line runs to a few megabytes).
const MAX_LINE: usize = 64 << 20;

/// An agent's protocol stream, a line at a time. Unlike `tokio::io::Lines`, a line has a longest
/// length (`MAX_LINE`): one that never ends, or runs to gigabytes, is skipped rather than read
/// into memory until there's none left. And a line that isn't UTF-8 is read as well as it can
/// be, where `Lines` ends the stream (and so the session) with an error.
pub(crate) struct ProtocolLines<R> {
    reader: R,
    line: Vec<u8>,
    /// The line being read is over the limit: the rest of it is passed over.
    skipping: bool,
}

impl<R: tokio::io::AsyncBufRead + Unpin> ProtocolLines<R> {
    pub(crate) fn new(reader: R) -> Self {
        ProtocolLines { reader, line: Vec::new(), skipping: false }
    }

    /// The next line, without its line ending; `None` at the end of the stream. What's read of a
    /// line stays read if this is dropped partway (it's used in `select!`).
    pub(crate) async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        use tokio::io::AsyncBufReadExt as _;
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                let rest = std::mem::take(&mut self.line);
                return Ok((!rest.is_empty() && !std::mem::take(&mut self.skipping)).then(|| String::from_utf8_lossy(&rest).into_owned()));
            }
            let end = chunk.iter().position(|b| *b == b'\n');
            let taken = end.map_or(chunk.len(), |e| e + 1);
            if !self.skipping {
                if self.line.len() + taken > MAX_LINE {
                    tracing::warn!("a protocol line over {} MB was skipped", MAX_LINE >> 20);
                    self.skipping = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(&chunk[..end.unwrap_or(chunk.len())]);
                }
            }
            self.reader.consume(taken);
            if end.is_some() {
                if std::mem::take(&mut self.skipping) {
                    continue;
                }
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
        }
    }
}

/// A child and everything it starts, in a process group of its own (on Windows, a job object).
pub(crate) struct GroupChild {
    child: Option<tokio::process::Child>,
    group: i32,
    /// The job holding the child and all it starts; closing it ends them all.
    #[cfg(windows)]
    job: Option<job::Job>,
}

/// Start `command` in a process group of its own, tracked by `trek_core::procs` until it's ended.
#[cfg(unix)]
pub(crate) fn spawn_group(command: &mut tokio::process::Command) -> std::io::Result<GroupChild> {
    command.process_group(0);
    let child = command.spawn()?;
    let group = child.id().unwrap_or_default() as i32;
    trek_core::procs::register(group);
    Ok(GroupChild { child: Some(child), group })
}

/// Start `command` in a job object of its own, with no console window, tracked by
/// `trek_core::procs` (by the child's pid) until it's ended. The job ends everything in it when
/// its last handle closes, so a Trek that crashes or is killed takes its agents along.
#[cfg(windows)]
pub(crate) fn spawn_group(command: &mut tokio::process::Command) -> std::io::Result<GroupChild> {
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
    let job = job::Job::new()?;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    let child = command.spawn()?;
    // The child runs from here until it's in the job, a few microseconds: anything it starts in
    // that time (it would have to load and get going first) isn't in the job. Starting it
    // suspended would close that gap, but the handle of its first thread, needed to resume it, is
    // one std doesn't hand out. A child that's in a job already (Trek run under a CI runner or a
    // terminal that uses jobs) can be put in this one too: jobs nest since Windows 8.
    if let Some(handle) = child.raw_handle()
        && let Err(e) = job.assign(handle)
    {
        tracing::warn!("couldn't put process {:?} in a job; what it starts may outlive it: {e}", child.id());
    }
    let group = child.id().unwrap_or_default() as i32;
    trek_core::procs::register(group);
    Ok(GroupChild { child: Some(child), group, job: Some(job) })
}

/// A Windows job object that ends the processes in it when its last handle closes.
#[cfg(windows)]
mod job {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub(crate) struct Job(OwnedHandle);

    impl Job {
        /// A new, unnamed job whose handle no child inherits (a child holding it would keep the
        /// job, and itself, alive). Without JOB_OBJECT_LIMIT_BREAKAWAY_OK nothing in it can
        /// leave: what the child starts stays in.
        pub(crate) fn new() -> std::io::Result<Job> {
            // SAFETY: no security attributes and no name: both may be null.
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `h` was just created, and is owned from here on.
            let job = Job(unsafe { OwnedHandle::from_raw_handle(h) });
            // SAFETY: all zeroes is a valid limit information (integers only): no limits.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of_val(&info) as u32;
            // SAFETY: `info` is the structure the class names, `size` long.
            if unsafe { SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(crate) fn assign(&self, process: RawHandle) -> std::io::Result<()> {
            // SAFETY: both handles are open: the job is this one's, the process the caller's child's.
            match unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } {
                0 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            }
        }

        /// End every process in the job, the way KILL to a process group does.
        pub(crate) fn terminate(&self) {
            // SAFETY: a job handle this owns, with all access.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }
}

/// Run `command` in a group of its own, with `input` (if any) on its stdin, and collect its
/// output. The group, and whatever it started, is ended once the command exits or `limit` passes.
pub(crate) async fn output_group(command: &mut tokio::process::Command, input: Option<Vec<u8>>, limit: std::time::Duration) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    command.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_group(command)?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
    }
    fn drain(pipe: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> tokio::task::JoinHandle<Vec<u8>> {
        tokio::spawn(async move {
            let mut out = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut out).await;
            }
            out
        })
    }
    let (stdout, stderr) = (drain(child.stdout.take()), drain(child.stderr.take()));
    let status = tokio::time::timeout(limit, child.wait()).await;
    child.terminate().await;
    let status = status.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))??;
    // The group is gone, so the pipes are closed (or soon will be).
    let collect = |task: tokio::task::JoinHandle<Vec<u8>>| async move { tokio::time::timeout(std::time::Duration::from_secs(2), task).await.ok().and_then(Result::ok).unwrap_or_default() };
    Ok(std::process::Output { status, stdout: collect(stdout).await, stderr: collect(stderr).await })
}

impl GroupChild {
    pub(crate) async fn terminate(&mut self) {
        let Some(child) = self.child.take() else { return };
        #[cfg(unix)]
        finish_group(child, self.group).await;
        #[cfg(windows)]
        finish_group(child, self.group, self.job.take()).await;
    }
}

impl std::ops::Deref for GroupChild {
    type Target = tokio::process::Child;

    fn deref(&self) -> &Self::Target {
        self.child.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for GroupChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child.as_mut().unwrap()
    }
}

#[cfg(unix)]
fn signal_group(group: i32, signal: i32) {
    if group > 0 {
        // SAFETY: a negative pid asks kill(2) to signal that process group.
        unsafe { libc::kill(-group, signal) };
    }
}

#[cfg(unix)]
async fn finish_group(mut child: tokio::process::Child, group: i32) {
    signal_group(group, libc::SIGTERM);
    let waited = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
    // The leader can exit while a descendant ignores TERM. KILL the group even when the
    // leader was already reaped, so nothing it started is left behind.
    signal_group(group, libc::SIGKILL);
    if waited.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(REAP_AFTER_KILL, child.wait()).await;
    }
    trek_core::procs::unregister(group);
}

/// Windows has no TERM. The gentle stop is the end of the child's stdin, which agents take as the
/// cue to exit, when it's still the group's to close: callers that write to the child take its
/// stdin, and the stop is theirs, by dropping it. A console CTRL_BREAK would be the nearest thing
/// to TERM, but it only reaches processes on the sender's console, and a child started with
/// CREATE_NO_WINDOW has one of its own (Trek, a GUI app, has none to share); attaching to the
/// child's console to send it would change the console of the whole of Trek while other
/// threads run. So past the grace period it's the job's KILL.
#[cfg(windows)]
fn soft_stop(child: &mut tokio::process::Child) {
    drop(child.stdin.take());
}

#[cfg(windows)]
async fn finish_group(mut child: tokio::process::Child, group: i32, job: Option<job::Job>) {
    soft_stop(&mut child);
    let waited = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
    // As with KILL to a process group: end the whole job even when the child has exited, so
    // nothing it started is left behind.
    if let Some(job) = &job {
        job.terminate();
    }
    if waited.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(REAP_AFTER_KILL, child.wait()).await;
    }
    trek_core::procs::unregister(group);
}

/// How long to wait for a KILLed leader to be reaped: one stuck in uninterruptible I/O can
/// outlive KILL for a while, and isn't waited on forever.
const REAP_AFTER_KILL: std::time::Duration = std::time::Duration::from_secs(5);

/// Wait up to `limit` for `child` to exit. True once it has, or can't be waited on (ECHILD).
fn reaped(child: &mut tokio::process::Child, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) if std::time::Instant::now() >= deadline => return false,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(windows)]
impl Drop for GroupChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        let (group, job) = (self.group, self.job.take());
        soft_stop(&mut child);
        // As on Unix, a thread rather than a runtime task. It holds the job: were the handle
        // closed here, the job would end its processes at once, with no grace.
        std::thread::spawn(move || {
            let exited = reaped(&mut child, std::time::Duration::from_secs(2));
            if let Some(job) = &job {
                job.terminate();
            }
            if !exited {
                let _ = child.start_kill();
                reaped(&mut child, REAP_AFTER_KILL);
            }
            trek_core::procs::unregister(group);
        });
    }
}

#[cfg(unix)]
impl Drop for GroupChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        let group = self.group;
        signal_group(group, libc::SIGTERM);
        // A runtime task can be cancelled as its runtime shuts down. A short-lived OS thread
        // makes dropped handles keep the same cleanup guarantee as explicit termination.
        std::thread::spawn(move || {
            let exited = reaped(&mut child, std::time::Duration::from_secs(2));
            signal_group(group, libc::SIGKILL);
            if !exited {
                let _ = child.start_kill();
                reaped(&mut child, REAP_AFTER_KILL);
            }
            trek_core::procs::unregister(group);
        });
    }
}

/// The last lines a child process wrote to stderr, for a readable error when it dies.
#[derive(Clone)]
pub(crate) struct StderrTail(std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>);

impl StderrTail {
    pub(crate) fn capture(stderr: tokio::process::ChildStderr, tag: &'static str) -> Self {
        use tokio::io::AsyncBufReadExt as _;
        let tail = Self(Default::default());
        let lines = tail.0.clone();
        // The session that started the process keeps all it says, for the Output panel.
        let session = SESSION_LOG.try_with(SessionLog::clone).ok();
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr);
            let mut split = StderrLines::default();
            let keep = |l: String| {
                tracing::debug!("{tag} stderr: {l}");
                if let Some(s) = &session {
                    s.push(l.clone());
                }
                let mut t = lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                t.push_back(l);
                if t.len() > 20 {
                    t.pop_front();
                }
            };
            loop {
                let chunk = match reader.fill_buf().await {
                    Ok(c) if !c.is_empty() => c,
                    _ => break,
                };
                let n = chunk.len();
                split.push(chunk, keep);
                reader.consume(n);
            }
            if let Some(l) = split.finish() {
                keep(l);
            }
        });
        tail
    }

    /// The lines kept, oldest first.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect()
    }

    /// "`name` exited: <last non-empty stderr line>".
    pub(crate) fn exited(&self, name: &str) -> anyhow::Error {
        let tail = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match tail.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()) {
            Some(l) => anyhow::anyhow!("{name} exited: {l}"),
            None => anyhow::anyhow!("{name} exited unexpectedly"),
        }
    }
}

/// The most of one stderr line kept; the rest of it is read and dropped.
const STDERR_LINE: usize = 8 * 1024;

/// Stderr split into lines as a terminal shows them: a `\r` not before `\n` starts the line over
/// (progress output), and a line keeps at most `STDERR_LINE` bytes.
#[derive(Default)]
struct StderrLines {
    line: Vec<u8>,
    cr: bool,
}

impl StderrLines {
    fn push(&mut self, bytes: &[u8], mut line: impl FnMut(String)) {
        for &b in bytes {
            match b {
                b'\n' => {
                    self.cr = false;
                    line(String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned());
                }
                b'\r' => self.cr = true,
                _ => {
                    if std::mem::take(&mut self.cr) {
                        self.line.clear();
                    }
                    if self.line.len() < STDERR_LINE {
                        self.line.push(b);
                    }
                }
            }
        }
    }

    /// The last line, when it didn't end in `\n`.
    fn finish(self) -> Option<String> {
        (!self.line.is_empty()).then(|| String::from_utf8_lossy(&self.line).into_owned())
    }
}

/// How much of a background task's or command's output Trek keeps while it runs: the end of it.
pub(crate) const OUTPUT_TAIL: usize = 8 * 1024;

/// `text` with `more` added, cut from the front to about `OUTPUT_TAIL` bytes (at a line break
/// when there's one near the cut).
pub(crate) fn keep_tail(text: &mut String, more: &str) {
    text.push_str(more);
    if text.len() <= OUTPUT_TAIL {
        return;
    }
    let mut cut = text.len() - OUTPUT_TAIL;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    let cut = text[cut..].find('\n').map(|n| cut + n + 1).filter(|c| *c - cut < 512).unwrap_or(cut);
    text.drain(..cut);
}

pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Lines added and removed going from `old` to `new`. Lines both share at the start and the end
/// are skipped; in between, a line counts as kept when the other side has it too (the lines of a
/// typical edit are distinct, so this matches a line diff without running one).
pub fn line_changes(old: &str, new: &str) -> (u32, u32) {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[head..old.len() - tail], &new[head..new.len() - tail]);
    let mut left: HashMap<&str, usize> = HashMap::new();
    for l in old {
        *left.entry(l).or_default() += 1;
    }
    let mut kept = 0;
    for l in new {
        if let Some(n) = left.get_mut(l).filter(|n| **n > 0) {
            *n -= 1;
            kept += 1;
        }
    }
    ((new.len() - kept) as u32, (old.len() - kept) as u32)
}

/// Read a local image as `(media_type, base64 data)`.
pub(crate) fn load_image(path: &std::path::Path) -> anyhow::Result<(&'static str, String)> {
    use base64::Engine as _;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        other => anyhow::bail!("unsupported image type .{other} ({})", path.display()),
    };
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("can't read image {}: {e}", path.display()))?;
    Ok((media_type, base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn protocol_lines_are_read_whole_and_overlong_ones_skipped() {
        let lines = |text: &'static [u8]| ProtocolLines::new(tokio::io::BufReader::with_capacity(8, text));
        // Lines longer than the reader's buffer, either line ending, a last one without any.
        let mut l = lines(b"{\"a\":\"0123456789abcdef\"}\r\nsecond\n\nlast");
        let mut got = vec![];
        while let Some(line) = l.next_line().await.unwrap() {
            got.push(line);
        }
        assert_eq!(got, ["{\"a\":\"0123456789abcdef\"}", "second", "", "last"]);
        // Not UTF-8: read as well as it can be, and the stream goes on.
        let mut l = lines(b"caf\xff\nnext\n");
        assert_eq!(l.next_line().await.unwrap().as_deref(), Some("caf\u{fffd}"));
        assert_eq!(l.next_line().await.unwrap().as_deref(), Some("next"));
        assert_eq!(l.next_line().await.unwrap(), None);
        // A line over the limit is passed over, and the one after it read.
        let long = vec![b'x'; MAX_LINE + 10];
        let text: Vec<u8> = [b"first\n".as_slice(), &long, b"\nafter\n"].concat();
        let mut l = ProtocolLines::new(tokio::io::BufReader::new(std::io::Cursor::new(text)));
        assert_eq!(l.next_line().await.unwrap().as_deref(), Some("first"));
        assert_eq!(l.next_line().await.unwrap().as_deref(), Some("after"));
        assert_eq!(l.next_line().await.unwrap(), None);
    }

    /// A shell that runs `unix` under `sh -c`, or `windows` under `cmd /c`.
    fn echo_to_stderr(unix: &str, windows: &str) -> tokio::process::Command {
        if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd.exe");
            c.args(["/d", "/c", windows]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", unix]);
            c
        }
    }

    #[tokio::test]
    async fn stderr_tail_keeps_draining_after_invalid_utf8() {
        use std::process::Stdio;
        let mut command = if cfg!(windows) {
            // cmd has no printf: `type` a file holding the same bytes.
            let file = std::env::temp_dir().join(format!("trek-stderr-bytes-{}.bin", std::process::id()));
            std::fs::write(&file, b"\xff\nvalid\n").unwrap();
            let mut c = tokio::process::Command::new("cmd.exe");
            c.args(["/d", "/c", "type"]).arg(file).arg("1>&2");
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "printf '\\377\\nvalid\\n' >&2"]);
            c
        };
        let mut child = command.stderr(Stdio::piped()).spawn().unwrap();
        let tail = StderrTail::capture(child.stderr.take().unwrap(), "test");
        child.wait().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(tail.exited("test").to_string().ends_with("valid"));
    }

    #[tokio::test]
    async fn a_sessions_processes_write_their_stderr_to_its_log() {
        use std::process::Stdio;
        let log = SessionLog::default();
        SESSION_LOG
            .scope(log.clone(), async {
                let mut child = echo_to_stderr("echo 'warning: slow' >&2; echo done >&2", "(echo warning: slow)>&2 & (echo done)>&2").stderr(Stdio::piped()).spawn().unwrap();
                let _tail = StderrTail::capture(child.stderr.take().unwrap(), "test");
                child.wait().await.unwrap();
            })
            .await;
        for _ in 0..100 {
            if log.len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(log.lines(), ["warning: slow", "done"]);
        // Outside a session nothing is kept but the tail.
        let mut child = echo_to_stderr("echo stray >&2", "(echo stray)>&2").stderr(Stdio::piped()).spawn().unwrap();
        let _tail = StderrTail::capture(child.stderr.take().unwrap(), "test");
        child.wait().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(log.len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ending_or_dropping_a_group_kills_its_grandchild() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt as _, BufReader};
        async fn tree() -> (GroupChild, i32) {
            let mut command = tokio::process::Command::new("sh");
            command.args(["-c", "sh -c 'trap \"\" TERM; exec sleep 300' & echo $!; wait"]).stdout(Stdio::piped());
            let mut child = spawn_group(&mut command).unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).await.unwrap();
            (child, line.trim().parse().unwrap())
        }
        async fn assert_gone(pid: i32) {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                // SAFETY: signal 0 only checks whether this test's child still exists.
                let gone = unsafe { libc::kill(pid, 0) } < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
                if gone || tokio::time::Instant::now() >= deadline {
                    assert!(gone, "grandchild {pid} survived its process group");
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }

        let (mut child, grandchild) = tree().await;
        child.terminate().await;
        assert_gone(grandchild).await;

        let (child, grandchild) = tree().await;
        drop(child);
        assert_gone(grandchild).await;
    }

    #[test]
    fn stderr_lines_are_capped_and_progress_starts_over() {
        let mut split = StderrLines::default();
        let mut got = vec![];
        let long = vec![b'x'; STDERR_LINE * 3];
        split.push(b"one\r\ntwo\n10%\r50%\r", |l| got.push(l));
        split.push(b"100%\n", |l| got.push(l));
        split.push(&long, |l| got.push(l));
        split.push(b"\nlast", |l| got.push(l));
        assert_eq!(&got[..3], ["one", "two", "100%"]);
        assert_eq!(got[3].len(), STDERR_LINE);
        assert_eq!(split.finish().as_deref(), Some("last"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn groups_are_tracked_until_ended() {
        let sleeper = || {
            let mut command = tokio::process::Command::new("sleep");
            command.arg("30");
            spawn_group(&mut command).unwrap()
        };
        let mut child = sleeper();
        let group = child.group;
        assert!(trek_core::procs::live().contains(&group));
        child.terminate().await;
        assert!(!trek_core::procs::live().contains(&group));

        let child = sleeper();
        let group = child.group;
        drop(child);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while trek_core::procs::live().contains(&group) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!trek_core::procs::live().contains(&group), "a dropped group is untracked once reaped");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_s_output_is_collected_and_a_slow_one_ended() {
        let mut cat = tokio::process::Command::new("cat");
        let out = output_group(&mut cat, Some(b"hi".to_vec()), std::time::Duration::from_secs(5)).await.unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hi");
        let mut slow = tokio::process::Command::new("sh");
        slow.args(["-c", "sleep 30 & wait"]);
        let started = std::time::Instant::now();
        let err = output_group(&mut slow, None, std::time::Duration::from_millis(200)).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "the group was ended, not waited out");
    }

    /// A process held open from before it's ended, so its id can't be another's when it's checked.
    #[cfg(windows)]
    struct Held(std::os::windows::io::OwnedHandle);

    #[cfg(windows)]
    impl Held {
        fn open(pid: u32) -> Held {
            use std::os::windows::io::FromRawHandle as _;
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
            // SAFETY: plain call; the handle is checked before it's owned.
            let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!h.is_null(), "process {pid} runs");
            Held(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(h) })
        }

        fn exits_within(&self, limit: std::time::Duration) -> bool {
            use std::os::windows::io::AsRawHandle as _;
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            // SAFETY: a process handle with SYNCHRONIZE.
            let waited = unsafe { WaitForSingleObject(self.0.as_raw_handle(), limit.as_millis() as u32) };
            waited == WAIT_OBJECT_0
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn ending_or_dropping_a_job_kills_its_grandchild() {
        use std::process::Stdio;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt as _, BufReader};
        // PowerShell starts a ping, says its pid, then runs `then`. The ping doesn't read stdin, so
        // closing it doesn't stop it: only the job's end does.
        async fn tree(then: &str) -> (GroupChild, Held) {
            let script = format!(
                "$i = New-Object Diagnostics.ProcessStartInfo 'ping.exe', '-n 300 127.0.0.1'; $i.UseShellExecute = $false; $i.RedirectStandardOutput = $true; \
                 $p = [Diagnostics.Process]::Start($i); [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); {then}"
            );
            let mut command = tokio::process::Command::new("powershell.exe");
            command.args(["-NoProfile", "-NonInteractive", "-Command", &script]).stdin(Stdio::piped()).stdout(Stdio::piped());
            let mut child = spawn_group(&mut command).unwrap();
            let mut line = String::new();
            let mut stdout = BufReader::new(child.stdout.take().unwrap());
            tokio::time::timeout(Duration::from_secs(60), stdout.read_line(&mut line)).await.expect("PowerShell said the ping's pid").unwrap();
            (child, Held::open(line.trim().parse().unwrap()))
        }

        let (mut child, grandchild) = tree("Start-Sleep 300").await;
        child.terminate().await;
        assert!(grandchild.exits_within(Duration::from_secs(10)), "the grandchild survived its job");

        let (child, grandchild) = tree("Start-Sleep 300").await;
        drop(child);
        assert!(grandchild.exits_within(Duration::from_secs(10)), "the grandchild survived its dropped job");

        // Its parent gone, the grandchild is still the job's: no parent id leads to it any more.
        let (mut child, grandchild) = tree("exit").await;
        assert!(tokio::time::timeout(Duration::from_secs(30), child.wait()).await.unwrap().unwrap().success());
        assert!(!grandchild.exits_within(Duration::from_millis(500)), "the ping runs on");
        child.terminate().await;
        assert!(grandchild.exits_within(Duration::from_secs(10)), "the orphaned grandchild survived its job");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn groups_are_tracked_until_ended_on_windows() {
        let pinger = || {
            let mut command = tokio::process::Command::new("ping.exe");
            command.args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null());
            spawn_group(&mut command).unwrap()
        };
        let mut child = pinger();
        let group = child.group;
        assert!(trek_core::procs::live().contains(&group));
        child.terminate().await;
        assert!(!trek_core::procs::live().contains(&group));

        let child = pinger();
        let group = child.group;
        drop(child);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while trek_core::procs::live().contains(&group) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!trek_core::procs::live().contains(&group), "a dropped group is untracked once reaped");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_finished_child_is_gone_and_untracked() {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/d", "/c", "exit 3"]);
        let mut child = spawn_group(&mut command).unwrap();
        let group = child.group;
        let status = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await.unwrap().unwrap();
        assert_eq!(status.code(), Some(3));
        let started = std::time::Instant::now();
        child.terminate().await;
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "nothing to wait for once it's exited");
        assert!(!trek_core::procs::live().contains(&group));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_command_s_output_is_collected_and_a_slow_one_ended_on_windows() {
        // System32's own sort, not one from Git or MSYS that may come first on PATH.
        let sort = std::path::Path::new(&std::env::var_os("SystemRoot").unwrap()).join(r"System32\sort.exe");
        let out = output_group(&mut tokio::process::Command::new(sort), Some(b"b\r\na\r\n".to_vec()), std::time::Duration::from_secs(30)).await.unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"a\r\nb\r\n");
        // cmd waits for the ping under it; both go when the time is up.
        let mut slow = tokio::process::Command::new("cmd.exe");
        slow.args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"]);
        let started = std::time::Instant::now();
        let err = output_group(&mut slow, None, std::time::Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "the group was ended, not waited out");
    }

    #[test]
    fn a_command_s_output_keeps_its_end() {
        let mut out = String::new();
        keep_tail(&mut out, &"line\n".repeat(OUTPUT_TAIL));
        assert!(out.len() <= OUTPUT_TAIL && out.starts_with("line\n") && out.ends_with("line\n"));
        let mut wide = String::new();
        keep_tail(&mut wide, &"é".repeat(OUTPUT_TAIL));
        assert!(wide.len() <= OUTPUT_TAIL + 1 && wide.chars().all(|c| c == 'é'));
    }

    /// Every test in this binary is isolated before any runs: none may reach the user's data
    /// folder or Keychain, whichever runs first.
    #[ctor::ctor(unsafe)]
    fn isolate_process() {
        trek_core::paths::isolate(std::env::temp_dir().join("trek-agents-tests"));
    }

    #[test]
    fn diff_stat_ignores_headers() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-old\n+new\n+more\n";
        assert_eq!(diff_stat(d), (2, 1));
    }

    #[test]
    fn line_changes_count_what_an_edit_did() {
        assert_eq!(line_changes("a\nb\nc\n", "a\nB\nc\n"), (1, 1));
        assert_eq!(line_changes("", "one\ntwo\n"), (2, 0), "a new file");
        assert_eq!(line_changes("fn a() {\n}\n", "fn a() {\n    x();\n    y();\n}\n"), (2, 0));
        assert_eq!(line_changes("x\ny\nz", ""), (0, 3));
        // Moved lines aren't changes; repeated ones are counted as often as they're added.
        assert_eq!(line_changes("1\n2\n3", "3\n1\n2"), (0, 0));
        assert_eq!(line_changes("}\n", "}\n}\n}\n"), (2, 0));
        assert_eq!(line_changes("same", "same"), (0, 0));
    }

    #[test]
    fn plan_title_is_the_first_line() {
        assert_eq!(plan_title("\n# Add `hello.txt`\n\n1. Write it"), "Add `hello.txt`");
        assert_eq!(plan_title(""), "");
    }

    #[test]
    fn plan_row_names_the_active_step() {
        let steps = vec![("Read code".to_string(), Step::Done), ("Fix bug".into(), Step::Active), ("Test".into(), Step::Pending)];
        assert_eq!(plan_row(&steps), ("Fix bug".into(), "✓ Read code\n→ Fix bug\n○ Test".into()));
        let done = vec![("A".to_string(), Step::Done), ("B".into(), Step::Done)];
        assert_eq!(plan_row(&done).0, "All 2 steps done");
        let waiting = vec![("A".to_string(), Step::Done), ("B".into(), Step::Pending)];
        assert_eq!(plan_row(&waiting).0, "1 of 2 steps done");
    }

    #[test]
    fn only_a_session_s_first_message_carries_the_project_notes() {
        let (tx, rx) = async_channel::unbounded();
        let out = first_prompt(rx, |text| instructions_prompt("Verify with ./app check.", text));
        trek_core::runtime().block_on(async {
            tx.send(Command::Prompt { text: "first".into(), images: vec![] }).await.unwrap();
            tx.send(Command::Prompt { text: "second".into(), images: vec![] }).await.unwrap();
            let texts: Vec<String> = [out.recv().await.unwrap(), out.recv().await.unwrap()]
                .into_iter()
                .map(|c| if let Command::Prompt { text, .. } = c { text } else { String::new() })
                .collect();
            assert_eq!(texts, ["<trek-project-notes>\nVerify with ./app check.\n</trek-project-notes>\n\nfirst", "second"]);
        });
    }

    #[test]
    fn only_the_first_message_carries_the_recap() {
        let (tx, rx) = async_channel::unbounded();
        let out = recap_first(rx, "User: hi".into());
        let prompt = |t: &str| Command::Prompt { text: t.into(), images: vec![] };
        let got = trek_core::runtime().block_on(async {
            tx.send(Command::SetHandHolding(trek_core::HandHolding::Auto)).await.unwrap();
            tx.send(prompt("first")).await.unwrap();
            tx.send(prompt("second")).await.unwrap();
            drop(tx);
            let mut got = vec![];
            while let Ok(c) = out.recv().await {
                got.push(c);
            }
            got
        });
        let texts: Vec<String> = got.iter().filter_map(|c| if let Command::Prompt { text, .. } = c { Some(text.clone()) } else { None }).collect();
        assert_eq!(got.len(), 3, "everything passes through, in order");
        assert!(texts[0].contains("<recap>\nUser: hi\n</recap>") && texts[0].ends_with("\n\nfirst"), "{}", texts[0]);
        assert_eq!(texts[1], "second");
    }

    #[test]
    fn which_agents_resume_partway() {
        assert!(resumes_partway(&AgentId::ClaudeCode, None) && resumes_partway(&AgentId::Codex, Some("gpt-5.6-luna")));
        assert!(!resumes_partway(&AgentId::OpenCode, None) && !resumes_partway(&AgentId::Direct("anthropic".into()), None));
        let mock = AgentId::Direct(mock::PROVIDER.into());
        assert!(resumes_partway(&mock, None) && !resumes_partway(&mock, Some(mock::RECAP_MODEL)));
    }

    #[test]
    fn a_session_that_panics_still_exits() {
        use super::AgentEvent;
        let (tx, rx) = async_channel::unbounded();
        super::supervise(async { panic!("overflow when adding duration to instant") }, tx);
        // The channel closes once the supervisor is done with it.
        let events = trek_core::runtime().block_on(async {
            let mut events = vec![];
            while let Ok(e) = rx.recv().await {
                events.push(e);
            }
            events
        });
        assert!(matches!(&events[..], [AgentEvent::Error(e), AgentEvent::Exited] if e.contains("crashed")), "{events:?}");
    }
}

#[cfg(test)]
mod live_usage {
    use super::*;
    use std::time::Duration;

    /// One tiny turn with a real agent: the events it sent.
    fn one_turn(agent: AgentId, model: &str) -> Vec<AgentEvent> {
        turn_in(agent, model, None, "Reply with just the word: ok", HandHolding::Supervised)
    }

    /// The session a turn ran in.
    fn session_of(events: &[AgentEvent]) -> String {
        events.iter().find_map(|e| if let AgentEvent::Started { native_id, .. } = e { Some(native_id.clone()) } else { None }).expect("started")
    }

    fn turn_in(agent: AgentId, model: &str, resume: Option<String>, prompt: &str, hand_holding: HandHolding) -> Vec<AgentEvent> {
        let cwd = std::path::PathBuf::from("/tmp/trek-basecamp-e2e");
        std::fs::create_dir_all(&cwd).unwrap();
        let session = start(SessionConfig {
            agent,
            cwd,
            model: Some(model.into()),
            effort: Effort::Low,
            hand_holding,
            plan: false,
            resume,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
            read_only: false,
        });
        trek_core::runtime().block_on(async {
            session.commands.send(Command::Prompt { text: prompt.into(), images: vec![] }).await.unwrap();
            let mut seen = vec![];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                let ev = tokio::time::timeout_at(deadline, session.events.recv()).await.expect("in time").expect("open");
                println!("{ev:?}");
                let done = matches!(ev, AgentEvent::TurnComplete { .. } | AgentEvent::Exited);
                seen.push(ev);
                if done {
                    break;
                }
            }
            let _ = session.commands.send(Command::Shutdown).await;
            seen
        })
    }

    fn reported(events: &[AgentEvent]) -> Vec<(Option<String>, TokenUsage)> {
        events.iter().filter_map(|e| if let AgentEvent::Usage { model, tokens, .. } = e { Some((model.clone(), *tokens)) } else { None }).collect()
    }

    #[test]
    #[ignore = "talks to the real Claude Code"]
    fn claude_live_turn_reports_its_tokens() {
        let ev = one_turn(AgentId::ClaudeCode, "claude-haiku-4-5");
        let used = reported(&ev);
        assert!(used.iter().any(|(m, t)| m.as_deref().is_some_and(|m| m.starts_with("claude-haiku")) && t.output > 0), "{used:?}");
    }

    #[test]
    #[ignore = "talks to the real Claude Code"]
    fn claude_live_resumed_turn_counts_only_itself() {
        let first = turn_in(AgentId::ClaudeCode, "claude-haiku-4-5", None, "Reply with just the word: ok", HandHolding::Supervised);
        let before: u64 = reported(&first).iter().map(|(_, t)| t.total()).sum();
        // A new process resuming the session: its totals carry the first turn's.
        let second = turn_in(AgentId::ClaudeCode, "claude-haiku-4-5", Some(session_of(&first)), "Reply with just the word: yes", HandHolding::Supervised);
        let used = reported(&second);
        println!("first {before}, resumed {used:?}");
        assert!(matches!(&used[..], [(Some(m), t)] if m.starts_with("claude-haiku") && t.output > 0 && t.output < 200), "{used:?}");
        // The first process left its totals in the ledger: the resumed turn is Claude Code's
        // own figure, not one priced here.
        let costs: Vec<UsageCost> = second.iter().filter_map(|e| if let AgentEvent::Usage { cost, .. } = e { *cost } else { None }).collect();
        println!("resumed costs {costs:?}");
        assert!(matches!(&costs[..], [c] if c.reported && c.usd > 0.0 && c.usd < 0.05), "{costs:?}");
    }

    #[test]
    #[ignore = "talks to the real Claude Code"]
    fn claude_live_background_shell_outlives_the_turn_and_wakes_it() {
        let cwd = std::path::PathBuf::from("/tmp/trek-background-e2e");
        std::fs::create_dir_all(&cwd).unwrap();
        let session = start(SessionConfig {
            agent: AgentId::ClaudeCode,
            cwd,
            model: Some("claude-haiku-4-5".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::FullAccess,
            plan: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
            read_only: false,
        });
        let prompt = "Use the Bash tool with run_in_background=true to run: echo started-bg; sleep 8; echo done-bg. Then immediately reply with just the word: started. When it completes, reply with just: finished.";
        let seen = trek_core::runtime().block_on(async {
            session.commands.send(Command::Prompt { text: prompt.into(), images: vec![] }).await.unwrap();
            let mut seen = vec![];
            let mut turns = 0;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            while turns < 2 {
                let ev = tokio::time::timeout_at(deadline, session.events.recv()).await.expect("in time").expect("open");
                println!("{ev:?}");
                if let AgentEvent::TurnComplete { .. } = ev {
                    turns += 1;
                    // Between turns, the shell's output can be read.
                    if let Some(id) = seen.iter().rev().find_map(|e| if let AgentEvent::Background(b) = e { b.first().map(|t: &BackgroundTask| t.id.clone()) } else { None }) {
                        session.commands.send(Command::ReadTask { id }).await.unwrap();
                    }
                }
                seen.push(ev);
            }
            let _ = session.commands.send(Command::Shutdown).await;
            seen
        });
        let first_end = seen.iter().position(|e| matches!(e, AgentEvent::TurnComplete { .. })).unwrap();
        assert!(seen[..first_end].iter().any(|e| matches!(e, AgentEvent::Background(b) if b.iter().any(|t| t.kind == BackgroundKind::Shell && t.readable && t.stoppable))), "{seen:?}");
        assert!(seen.iter().any(|e| matches!(e, AgentEvent::TaskOutput { output, .. } if output.contains("started-bg"))), "{seen:?}");
        let rest = &seen[first_end + 1..];
        let emptied = rest.iter().position(|e| *e == AgentEvent::Background(vec![])).expect("the shell ends");
        assert!(rest[emptied..].iter().any(|e| matches!(e, AgentEvent::TextDelta(_))), "the agent takes a turn of its own: {rest:?}");
    }

    #[test]
    #[ignore = "talks to the real Claude Code"]
    fn claude_live_stopped_background_agent_and_what_follows() {
        let cwd = std::path::PathBuf::from("/tmp/trek-background-e2e");
        std::fs::create_dir_all(&cwd).unwrap();
        let session = start(SessionConfig {
            agent: AgentId::ClaudeCode,
            cwd,
            model: Some("claude-haiku-4-5".into()),
            effort: Effort::Low,
            hand_holding: HandHolding::FullAccess,
            plan: false,
            resume: None,
            resume_at: None,
            fork: false,
            recap: None,
            fast: None,
            mcp_servers: vec![],
            instructions: None,
            read_dirs: vec![],
            read_only: false,
        });
        let prompt = "Use the Agent tool with run_in_background=true and subagent_type general-purpose, description \"Wait a while\", prompt: \"Run the Bash command `sleep 45` and then reply with just: waited.\" Then immediately reply with just the word: started.";
        let seen = trek_core::runtime().block_on(async {
            session.commands.send(Command::Prompt { text: prompt.into(), images: vec![] }).await.unwrap();
            let mut seen = vec![];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            // The turn that sends it out.
            loop {
                let ev = tokio::time::timeout_at(deadline, session.events.recv()).await.expect("in time").expect("open");
                println!("{ev:?}");
                let done = matches!(ev, AgentEvent::TurnComplete { .. });
                seen.push(ev);
                if done {
                    break;
                }
            }
            let agent = seen.iter().rev().find_map(|e| if let AgentEvent::Background(b) = e { b.iter().find(|t| t.kind == BackgroundKind::Agent).map(|t| t.id.clone()) } else { None }).expect("an agent out");
            println!("--- stopping {agent}");
            session.commands.send(Command::StopTask { id: agent }).await.unwrap();
            // Whatever follows within 30 s.
            let quiet = tokio::time::Instant::now() + Duration::from_secs(30);
            while let Ok(Ok(ev)) = tokio::time::timeout_at(quiet, session.events.recv()).await {
                println!("{ev:?}");
                seen.push(ev);
            }
            let _ = session.commands.send(Command::Shutdown).await;
            seen
        });
        let first_end = seen.iter().position(|e| matches!(e, AgentEvent::TurnComplete { .. })).unwrap();
        let rest = &seen[first_end + 1..];
        assert!(rest.contains(&AgentEvent::Background(vec![])), "it stops: {rest:?}");
        // Recorded (Claude Code 2.1.289): it takes a turn of its own to say so, which Trek keeps
        // quiet (`Workspace::interrupt`, `quiet_turn`).
        assert!(rest.iter().any(|e| matches!(e, AgentEvent::TextDelta(_))), "{rest:?}");
    }

    #[test]
    #[ignore = "talks to the real OpenCode"]
    fn opencode_live_turn_with_tools_counts_every_step() {
        let model = std::env::var("TREK_LIVE_OPENCODE_MODEL").unwrap_or_else(|_| "opencode/ling-3.1-flash-free".into());
        let ev = turn_in(AgentId::OpenCode, &model, None, "Use your shell tool to run `ls -a` here, then tell me how many entries it printed.", HandHolding::FullAccess);
        let steps = trek_core::import::opencode::usage(&session_of(&ev), 0, i64::MAX);
        let used = reported(&ev);
        println!("steps {steps:?}\nreported {used:?}");
        assert!(steps.len() > 1, "a turn with a tool call takes more than one step: {steps:?}");
        let total = |t: &[TokenUsage]| t.iter().map(|t| t.total()).sum::<u64>();
        assert_eq!(total(&used.iter().map(|(_, t)| *t).collect::<Vec<_>>()), total(&steps.iter().map(|(_, _, t)| *t).collect::<Vec<_>>()));
    }

    #[test]
    #[ignore = "talks to the real Codex"]
    fn codex_live_turn_reports_its_tokens() {
        let ev = one_turn(AgentId::Codex, "gpt-5.6-luna");
        let used = reported(&ev);
        assert!(matches!(&used[..], [(Some(m), t)] if m == "gpt-5.6-luna" && t.total() > 0), "{used:?}");
    }

    #[test]
    #[ignore = "talks to the real OpenCode"]
    fn opencode_live_turn_reports_what_it_reports() {
        // ACP agents report a turn's tokens in the prompt response when they do at all.
        let ev = one_turn(AgentId::OpenCode, &std::env::var("TREK_LIVE_OPENCODE_MODEL").unwrap_or_else(|_| "opencode/ling-3.1-flash-free".into()));
        assert!(matches!(ev.last(), Some(AgentEvent::TurnComplete { error: None, .. })), "{ev:?}");
        println!("reported: {:?}", reported(&ev));
    }
}
