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
pub mod mock;
mod opencode;
mod status;

pub use acp::{AcpInfo, acp_probe};
pub use codex::list_models as codex_models;
pub use limits::{Limit, LimitScope};
pub use status::{AgentStatus, CommandKind, SlashCommand, UsageLimit, claude_status, codex_status};

use std::path::PathBuf;
use trek_core::{AgentId, Effort, HandHolding};

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentId,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Effort,
    pub hand_holding: HandHolding,
    pub plan: bool,
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
    /// Extra MCP servers (stdio) to attach to the session, on top of the agent's own config.
    pub mcp_servers: Vec<McpServer>,
}

/// A stdio MCP server Trek adds to a session.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl McpServer {
    /// `{command, args, env}` — the shape both Claude's `mcpServers` and Codex's `mcp_servers` use.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let env: serde_json::Map<String, serde_json::Value> =
            self.env.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
        serde_json::json!({ "command": self.command, "args": self.args, "env": env })
    }
}

/// `{name: {command, args, env}, ...}` for a set of servers.
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
    Shutdown,
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
    /// The agent needs the user: a yes/no approval, or (`prompt`) a question or a plan to review.
    PermissionRequest { request_id: String, title: String, detail: String, prompt: Option<Prompt> },
    /// The agent settled a request on its own (it timed out, or the turn moved on): its card goes.
    PermissionResolved { request_id: String },
    /// A diff stat for the turn, when the agent reports one.
    DiffStat { additions: i64, deletions: i64 },
    /// A turn ended. `cost_usd`: what the session has cost so far, as a running total (a resumed
    /// session may carry on from its saved total; it starts again from zero after a `/clear`).
    TurnComplete { cost_usd: Option<f64>, error: Option<String> },
    /// Tokens currently in the context window, and the window size.
    Context { used: u64, window: u64 },
    /// How the session is billed, once the agent has said which login it uses.
    Billing(Billing),
    /// A sub-agent (keyed by the tool call that launched it) started, moved on, or finished.
    Task { id: String, description: Option<String>, activity: Option<String>, tool_uses: Option<u64>, done: Option<bool> },
    /// How many background sub-agents are still running; the turn isn't really over until zero.
    Background(usize),
    /// The slash commands the agent offers in this session (replaces any earlier list).
    Commands(Vec<SlashCommand>),
    /// Something the user should know that isn't an error (the transcript shows it as a note).
    Notice(String),
    /// The latest point the session can be taken back to, for `SessionConfig::resume_at`: the
    /// last message's id (Claude Code) or the last finished turn's (Codex).
    Mark(String),
    /// The agent stopped at a usage limit (5-hour, weekly, a model's, or a provider's rate limit).
    /// `resets_at`: unix ms, when the agent said or it could be worked out. The turn then ends
    /// with an error saying the same.
    LimitReached { message: String, resets_at: Option<i64>, scope: LimitScope },
    /// Something went wrong. It ends no turn by itself (an unreadable image, a refused model
    /// change): a turn that fails says so as it completes, or its session ends.
    Error(String),
    Exited,
}

pub struct SessionHandle {
    pub commands: async_channel::Sender<Command>,
    pub events: async_channel::Receiver<AgentEvent>,
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
    let (tx, rx) = async_channel::unbounded();
    trek_core::runtime().spawn(async move {
        let mut recap = Some(recap);
        while let Ok(mut cmd) = commands.recv().await {
            if let Command::Prompt { text, .. } = &mut cmd {
                if let Some(r) = recap.take() {
                    *text = recap_prompt(&r, text);
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
    supervise(
        async move {
            match &config.agent {
                AgentId::ClaudeCode => claude::run(config, cmd_rx, events).await,
                AgentId::Codex => codex::run(config, cmd_rx, events).await,
                AgentId::Direct(p) if trek_core::catalog::is_mock(p) => mock::run(config, cmd_rx, events).await,
                AgentId::Direct(_) => direct::run(config, cmd_rx, events).await,
                AgentId::Acp(_) | AgentId::OpenCode | AgentId::Droid => acp::run(config, cmd_rx, events).await,
            }
        },
        ev_tx,
    );
    SessionHandle { commands: cmd_tx, events: ev_rx }
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

/// The last lines a child process wrote to stderr, for a readable error when it dies.
#[derive(Clone)]
pub(crate) struct StderrTail(std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>);

impl StderrTail {
    pub(crate) fn capture(stderr: tokio::process::ChildStderr, tag: &'static str) -> Self {
        use tokio::io::AsyncBufReadExt as _;
        let tail = Self(Default::default());
        let lines = tail.0.clone();
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(l)) = reader.next_line().await {
                tracing::debug!("{tag} stderr: {l}");
                let mut t = lines.lock().unwrap();
                t.push_back(l);
                if t.len() > 20 {
                    t.pop_front();
                }
            }
        });
        tail
    }

    /// "`name` exited: <last non-empty stderr line>".
    pub(crate) fn exited(&self, name: &str) -> anyhow::Error {
        let tail = self.0.lock().unwrap();
        match tail.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()) {
            Some(l) => anyhow::anyhow!("{name} exited: {l}"),
            None => anyhow::anyhow!("{name} exited unexpectedly"),
        }
    }
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
