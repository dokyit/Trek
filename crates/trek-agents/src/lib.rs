//! Live agent sessions. Every backend (vendor CLI, ACP agent, direct API) is driven through the
//! same channel pair: the UI sends [`Command`]s and receives normalized [`AgentEvent`]s.
//! Sessions run on `trek_core::runtime()`; channels are executor-agnostic.

mod title;
pub use title::generate_title;
mod acp;
mod claude;
mod codex;
mod direct;
mod opencode;
mod status;

pub use acp::{AcpInfo, acp_probe};
pub use codex::list_models as codex_models;
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
    Error(String),
    Exited,
}

pub struct SessionHandle {
    pub commands: async_channel::Sender<Command>,
    pub events: async_channel::Receiver<AgentEvent>,
}

/// Start a session for `config.agent`.
pub fn start(config: SessionConfig) -> SessionHandle {
    let (cmd_tx, cmd_rx) = async_channel::unbounded();
    let (ev_tx, ev_rx) = async_channel::unbounded();
    trek_core::runtime().spawn(async move {
        let result = match &config.agent {
            AgentId::ClaudeCode => claude::run(config, cmd_rx, ev_tx.clone()).await,
            AgentId::Codex => codex::run(config, cmd_rx, ev_tx.clone()).await,
            AgentId::Direct(_) => direct::run(config, cmd_rx, ev_tx.clone()).await,
            AgentId::Acp(_) | AgentId::OpenCode | AgentId::Droid => acp::run(config, cmd_rx, ev_tx.clone()).await,
        };
        if let Err(e) = result {
            let _ = ev_tx.send(AgentEvent::Error(format!("{e:#}"))).await;
        }
        let _ = ev_tx.send(AgentEvent::Exited).await;
    });
    SessionHandle { commands: cmd_tx, events: ev_rx }
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
}
