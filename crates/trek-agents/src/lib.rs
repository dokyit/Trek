//! Live agent sessions. Every backend (vendor CLI, ACP agent, direct API) is driven through the
//! same channel pair: the UI sends [`Command`]s and receives normalized [`AgentEvent`]s.
//! Sessions run on `trek_core::runtime()`; channels are executor-agnostic.

mod claude;
mod codex;
mod direct;
mod status;

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

#[derive(Debug, Clone)]
pub enum Command {
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
    PermissionRequest { request_id: String, title: String, detail: String },
    /// A diff stat for the turn, when the agent reports one.
    DiffStat { additions: i64, deletions: i64 },
    TurnComplete { cost_usd: Option<f64>, error: Option<String> },
    /// Tokens currently in the context window, and the window size.
    Context { used: u64, window: u64 },
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
            other => Err(anyhow::anyhow!("{} isn't wired up yet — coming with ACP support", other.display_name())),
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
    #[test]
    fn diff_stat_ignores_headers() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-old\n+new\n+more\n";
        assert_eq!(super::diff_stat(d), (2, 1));
    }
}
