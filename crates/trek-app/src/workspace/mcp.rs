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

/// `s` as a session is given it, its tokens read back from the Keychain (one that's gone isn't
/// sent).
pub fn mcp_server_of(s: &McpServerConfig) -> McpServer {
    match &s.url {
        Some(url) => McpServer::http(s.name.clone(), url.clone(), s.resolved_headers()),
        None => McpServer::stdio(
            s.name.clone(),
            s.command.clone(),
            s.args.clone(),
            s.resolved_env(),
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
