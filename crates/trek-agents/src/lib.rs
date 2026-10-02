//! Live agent sessions. Every backend (vendor CLI, ACP agent, direct API) is driven through the
//! same channel pair: the UI sends [`Command`]s and receives normalized [`AgentEvent`]s.
//! Sessions run on `trek_core::runtime()`; channels are executor-agnostic.

mod acp;
mod claude;
mod codex;
mod direct;

pub use acp::{AcpInfo, acp_probe};
pub use codex::list_models as codex_models;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    AllowForSession,
    Deny,
}

#[derive(Debug, Clone)]
pub enum Command {
    Prompt(String),
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

#[cfg(test)]
mod tests {
    #[test]
    fn diff_stat_ignores_headers() {
        let d = "--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n-old\n+new\n+more\n";
        assert_eq!(super::diff_stat(d), (2, 1));
    }
}
