//! Settings › Tools › Connections: Figma. Its desktop app's MCP server (local, no login) can go
//! to every agent; its remote server signs in only clients in Figma's MCP catalog, which Trek
//! isn't (and must never pose as), so Trek sets it up through Claude Code and Codex, which are.

use super::SettingsView;
use crate::palette;
use crate::ui;
use crate::workspace::WorkspaceEvent;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;
use trek_core::settings::{FIGMA_DESKTOP_SERVER, FIGMA_DESKTOP_URL};

/// Figma's remote MCP server.
pub(super) const FIGMA_REMOTE_URL: &str = "https://mcp.figma.com/mcp";

/// Whether an agent has Figma's remote server set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FigmaLink {
    Connected,
    NotConnected,
    NotInstalled,
}

/// Claude Code's and Codex's link to Figma's remote server, from the MCP servers each loads
/// (`integrations::agent_mcp_servers`) and Claude Code's plugins (Figma's plugin brings the
/// server with it). A server counts if it's named `figma` (what Connect adds) or `figma-…`,
/// except a desktop one. `installed` says whether an agent's CLI is on this Mac.
pub(super) fn figma_links(agent_mcp: &[(&'static str, Vec<String>)], plugins: &[String], installed: impl Fn(&AgentId) -> bool) -> Vec<(AgentId, FigmaLink)> {
    let remote = |n: &String| {
        let n = n.to_ascii_lowercase();
        (n == "figma" || n.starts_with("figma-") || n.starts_with("figma_")) && !n.contains("desktop") && !n.contains("dev-mode")
    };
    [(AgentId::ClaudeCode, "Claude Code"), (AgentId::Codex, "Codex")]
        .into_iter()
        .map(|(agent, label)| {
            let listed = agent_mcp.iter().any(|(a, names)| *a == label && names.iter().any(remote));
            let plugin = agent == AgentId::ClaudeCode && plugins.iter().any(|p| p.split('@').next().is_some_and(|n| n.eq_ignore_ascii_case("figma")));
            let link = if listed || plugin {
                FigmaLink::Connected
            } else if installed(&agent) {
                FigmaLink::NotConnected
            } else {
                FigmaLink::NotInstalled
            };
            (agent, link)
        })
        .collect()
}

/// What Connect runs in Trek's terminal for `agent`: the agent's own command to add Figma's
/// remote server (the sign-in is the agent's, in its own way).
pub(super) fn connect_command(agent: &AgentId) -> Option<String> {
    match agent {
        AgentId::ClaudeCode => Some(format!("claude mcp add --scope user --transport http figma {FIGMA_REMOTE_URL}")),
        AgentId::Codex => Some(format!("codex mcp add figma --url {FIGMA_REMOTE_URL} && codex mcp login figma")),
        _ => None,
    }
}

/// Figma's own plugin for Claude Code: the remote server plus Figma's skills.
const CLAUDE_PLUGIN: &str = "claude plugin install figma@claude-plugins-official";

/// Whether the Figma desktop app's MCP server answers, asked as an agent would open (an MCP
/// `initialize`): any HTTP answer means it's up. Runs on Trek's tokio runtime; the answer
/// comes back over the channel.
fn check_figma_desktop() -> async_channel::Receiver<bool> {
    let (tx, rx) = async_channel::bounded(1);
    trek_core::runtime().spawn(async move {
        let hello = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "trek", "version": env!("CARGO_PKG_VERSION") } },
        });
        let running = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)).no_proxy().build() {
            Ok(client) => client.post(FIGMA_DESKTOP_URL).header("Accept", "application/json, text/event-stream").json(&hello).send().await.is_ok(),
            Err(_) => false,
        };
        let _ = tx.send(running).await;
    });
    rx
}

impl SettingsView {
    /// Ask whether Figma's desktop server is up, unless that's known or being asked.
    pub(super) fn probe_figma_desktop(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.figma_desktop;
        if p.fresh || p.task.is_some() {
            return;
        }
        let answer = check_figma_desktop();
        p.task = Some(cx.spawn(async move |this, cx| {
            let running = answer.recv().await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                let p = &mut this.figma_desktop;
                p.value = Some(running);
                p.fresh = true;
                p.task = None;
                cx.notify();
            });
        }));
    }

    /// The Connections group at the top of the Tools page.
    pub(super) fn connections(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.workspace.read(cx).settings.clone();
        let muted = cx.theme().muted_foreground;
        let theme = if cx.theme().mode.is_dark() { "dark" } else { "light" };
        let figma_logo = img(SharedString::from(format!("logos/{theme}/figma.png"))).size(px(18.)).flex_none();
        let titled = |logo: AnyElement, text: &'static str| h_flex().gap(px(10.)).child(logo).child(div().text_size(px(13.5)).font_medium().child(text));

        let running = self.figma_desktop.value;
        let checking = self.figma_desktop.loading();
        let (desktop_text, desktop_status): (&str, AnyElement) = match running {
            None => ("Looking for the Figma desktop app…", div().text_size(px(12.5)).text_color(muted).child("Checking…").into_any_element()),
            Some(true) => (
                "Agents given its server can read the design open in Figma: frames, variables and components.",
                Self::status_dot(palette::emerald(cx), "Running"),
            ),
            Some(false) => (
                "Not running — open a design file in Figma, press Shift-D, and turn on Enable desktop MCP server in the Inspect panel.",
                Button::new("figma-desktop-check")
                    .small()
                    .outline()
                    .loading(checking)
                    .icon(IconName::RefreshCw)
                    .label("Check again")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.figma_desktop.invalidate();
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
        };
        let mut rows = vec![
            Self::row(titled(figma_logo.into_any_element(), "Figma desktop app"), desktop_text, div().id("figma-desktop-status").test_support().child(desktop_status), cx),
            Self::row(
                "Give agents the Figma desktop server",
                format!("Every new session gets it as {FIGMA_DESKTOP_SERVER}, whichever agent runs it. Needs a Dev or Full seat."),
                self.switch("figma-desktop", s.tools.figma_desktop, |s, v| s.tools.figma_desktop = v),
                cx,
            ),
        ];

        let links = self.tools.value.as_ref().map(|p| p.figma.clone());
        for agent in [AgentId::ClaudeCode, AgentId::Codex] {
            let link = links.as_ref().and_then(|l| l.iter().find(|(a, _)| *a == agent).map(|(_, l)| *l));
            let claude = agent == AgentId::ClaudeCode;
            let (name, title) = if claude { ("Claude Code", "Figma through Claude Code") } else { ("Codex", "Figma through Codex") };
            let description = match link {
                None => "Reading its setup…".to_string(),
                Some(FigmaLink::Connected) => format!("Figma's remote server, signed in with your Figma account in {name}."),
                Some(FigmaLink::NotConnected) if claude => "Adds Figma's remote server to Claude Code. Then, in Claude Code, type /mcp, choose figma, and Authenticate.".into(),
                Some(FigmaLink::NotConnected) => "Adds Figma's remote server to Codex and signs you in to Figma in your browser.".into(),
                Some(FigmaLink::NotInstalled) => format!("{name} isn't installed."),
            };
            let key = agent.key();
            let control: AnyElement = match link {
                None => div().into_any_element(),
                Some(FigmaLink::Connected) => Self::status_dot(palette::emerald(cx), "Connected"),
                Some(FigmaLink::NotInstalled) => div().text_size(px(12.5)).text_color(muted).child("Not installed").into_any_element(),
                Some(FigmaLink::NotConnected) => h_flex()
                    .gap_2()
                    .when(claude, |el| {
                        el.child(Button::new("figma-claude-plugin").small().ghost().label("Use the plugin").tooltip("Figma's plugin for Claude Code: the remote server plus Figma's skills").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command: CLAUDE_PLUGIN.into(), cwd: None }));
                                this.tools.invalidate();
                                window.push_notification("Then, in Claude Code, type /mcp, choose figma, and Authenticate.", cx);
                            },
                        )))
                    })
                    .child(Button::new(SharedString::from(format!("figma-connect-{key}"))).small().outline().icon(IconName::Plus).label("Connect").on_click(cx.listener({
                        let agent = agent.clone();
                        move |this, _, window, cx| {
                            let Some(command) = connect_command(&agent) else { return };
                            this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::RunInTerminal { command, cwd: None }));
                            // It's set up once the command has run: the page reads it again when shown.
                            this.tools.invalidate();
                            if agent == AgentId::ClaudeCode {
                                window.push_notification("Then, in Claude Code, type /mcp, choose figma, and Authenticate.", cx);
                            }
                        }
                    })))
                    .into_any_element(),
            };
            rows.push(Self::row(titled(ui::agent_logo(&agent, px(18.), cx), title), description, div().id(SharedString::from(format!("figma-{key}"))).test_support().child(control), cx));
        }
        vec![
            // The page's first section: no room above it for one before.
            div().pb(px(10.)).text_size(px(13.)).font_semibold().child("Connections").into_any_element(),
            ui::group(rows, cx),
            div()
                .pt_3()
                .child(Self::note(
                    "Figma lets only apps it has approved sign in to its remote server, so Trek connects it through Claude Code and Codex. On a View or Collab seat, Figma allows 20 calls a month.",
                    cx,
                ))
                .into_any_element(),
        ]
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{FigmaLink, connect_command, figma_links};
    use trek_core::AgentId;

    #[test]
    fn figma_counts_as_connected_by_its_server_or_claudes_plugin() {
        let installed = |_: &AgentId| true;
        let mcp = vec![("Claude Code", vec!["github".to_string(), "Figma".to_string()]), ("Codex", vec!["figma-desktop".to_string()])];
        assert_eq!(figma_links(&mcp, &[], installed), [(AgentId::ClaudeCode, FigmaLink::Connected), (AgentId::Codex, FigmaLink::NotConnected)]);
        let none = vec![("Claude Code", vec![]), ("Codex", vec![])];
        let plugin = ["figma@claude-plugins-official".to_string()];
        assert_eq!(figma_links(&none, &plugin, installed)[0].1, FigmaLink::Connected, "Figma's plugin brings the server");
        assert_eq!(figma_links(&none, &[], |a| *a == AgentId::Codex), [(AgentId::ClaudeCode, FigmaLink::NotInstalled), (AgentId::Codex, FigmaLink::NotConnected)]);
    }

    #[test]
    fn connect_uses_each_agents_own_command() {
        assert_eq!(connect_command(&AgentId::ClaudeCode).as_deref(), Some("claude mcp add --scope user --transport http figma https://mcp.figma.com/mcp"));
        assert_eq!(connect_command(&AgentId::Codex).as_deref(), Some("codex mcp add figma --url https://mcp.figma.com/mcp && codex mcp login figma"));
        assert_eq!(connect_command(&AgentId::OpenCode), None);
    }
}
