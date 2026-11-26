//! The user's own MCP servers: what a session is handed for each, and checking that one works.

use super::Workspace;
use gpui_kit::Context;
use std::sync::atomic::{AtomicU64, Ordering};
use trek_agents::McpServer;
use trek_core::settings::McpServerConfig;

/// What checking a server found.
#[derive(Debug, Clone, PartialEq)]
pub enum McpCheck {
    /// Under way. The number tells this check from one started after it (an edit, a click).
    Checking(u64),
    /// It answered; the names of its tools.
    Works(Vec<String>),
    /// Why it didn't, in words for Settings.
    Failed(String),
}

/// Whether `name` is one of Trek's own channel variables. A server the user configured must
/// never be handed them: the token lets a process act as that session (`request_secret`, sub-
/// agents), so a pasted or agent-written config could otherwise read it from its own env.
/// In any case: Windows' variable names have none.
fn is_trek_channel(name: &str) -> bool {
    trek_core::procs::is_channel_env(name)
}

/// The names Trek's own tool servers go by. Agents call their tools by these names
/// (`mcp__trek-computer__click`) and Trek hands some of them a session's channel, so a server of
/// the user's (pasted, or written into the settings by an agent) can't take one: it's left out of
/// every session.
pub const RESERVED_SERVERS: [&str; 6] = [
    super::orchestrate::COMPUTER_SERVER,
    "trek-simulator",
    "trek-video",
    trek_ipc::browser::SERVER,
    trek_ipc::phone::SERVER,
    trek_core::orchestrate::SERVER,
];

/// Whether `name` is one of Trek's own servers' (in any case, with space around it or not).
pub fn is_reserved_server(name: &str) -> bool {
    RESERVED_SERVERS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(name.trim()))
}

/// `s` as a session is given it, its tokens read back from the Keychain (one that's gone isn't
/// sent).
pub fn mcp_server_of(s: &McpServerConfig) -> McpServer {
    match &s.url {
        Some(url) => McpServer::http(s.name.clone(), url.clone(), s.resolved_headers()),
        None => McpServer::stdio(
            s.name.clone(),
            s.command.clone(),
            s.args.clone(),
            s.resolved_env()
                .into_iter()
                .filter(|(name, _)| !is_trek_channel(name))
                .collect(),
        ),
    }
}

impl Workspace {
    /// Start server `name` (or call it, for a remote one) the way an agent would and list its
    /// tools, off the main thread; `mcp_checks` has the outcome.
    pub fn check_mcp_server(&mut self, name: &str, cx: &mut Context<Self>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let Some(config) = self
            .settings
            .tools
            .mcp_servers
            .iter()
            .find(|s| s.name == name)
            .cloned()
        else {
            return;
        };
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.mcp_checks
            .insert(name.to_string(), McpCheck::Checking(id));
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            // The Keychain is read here, off the main thread, quietly: a token an earlier copy of
            // Trek saved is locked and left out, and the check says so rather than failing on it.
            let server = tokio::task::spawn_blocking(move || {
                let server = mcp_server_of(&config);
                (server, config.locked_secrets())
            })
            .await;
            let found = match server {
                Ok((_, locked)) if !locked.is_empty() => Err(format!(
                    "Its saved {} is locked: an earlier copy of Trek saved it. Allow access above, or edit the server and paste it again.",
                    locked.join(", ")
                )),
                Ok((server, _)) => trek_agents::mcp_check::list_tools(&server).await,
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(found).await;
        });
        let name = name.to_string();
        let task = cx.spawn(async move |this, cx| {
            let Ok(found) = rx.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                // Only the latest check of a server that's still there counts.
                if this.mcp_checks.get(&name) == Some(&McpCheck::Checking(id)) {
                    this.mcp_checks
                        .insert(name, found.map_or_else(McpCheck::Failed, McpCheck::Works));
                    cx.notify();
                }
            });
        });
        self.keep(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trek_core::settings::McpEnvVar;

    fn env(name: &str, value: &str) -> McpEnvVar {
        McpEnvVar {
            name: name.into(),
            value: value.into(),
            secret: false,
        }
    }

    #[test]
    fn a_users_mcp_server_is_never_handed_treks_channel_token() {
        let mut server = McpServerConfig::stdio("pasted", "/bin/true", vec![]);
        server.env = vec![
            env(trek_ipc::ENV_TOKEN, "stolen-token"),
            env(trek_ipc::ENV_SOCKET, "/tmp/stolen"),
            env(trek_ipc::ENV_SESSION, "stolen-session"),
            env("KEEP", "yes"),
        ];
        assert_eq!(
            mcp_server_of(&server).env(),
            [("KEEP".to_string(), "yes".to_string())]
        );
    }
}
