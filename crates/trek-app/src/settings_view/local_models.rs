//! Settings › Local Models: the model servers on this computer, as onboarding's step shows them:
//! running (with the models it serves, and a switch for using it), installed and not running
//! (Start), or not installed (Get).

use super::{Probe, SettingsView};
use crate::onboarding::{LocalServer, scan_local};
use crate::scale::text;
use crate::ui;
use crate::workspace::WorkspaceEvent;
use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::*;
use std::collections::HashSet;
use trek_core::AgentId;

/// What the page reads off the main thread: which servers' command-line tools are installed, and
/// where Ollama answers (`OLLAMA_HOST`).
#[derive(Clone, Default)]
pub(super) struct LocalProbe {
    clis: HashSet<String>,
    ollama: String,
}

impl LocalProbe {
    pub(super) fn read() -> Self {
        Self {
            clis: crate::onboarding::installed_local_clis(),
            ollama: trek_core::detect::ollama_base(),
        }
    }
}

/// Ollama's address as the page says it, when it isn't the usual one.
fn unusual_address(base: &str) -> Option<String> {
    let usual = trek_core::detect::ollama_base_from(None);
    (!base.is_empty() && base != usual).then(|| {
        base.trim_end_matches("/v1")
            .trim_start_matches("http://")
            .to_string()
    })
}

impl SettingsView {
    pub(super) fn local_models_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let ws = self.workspace.read(cx);
        let detecting = ws.detecting;
        let probe: &Probe<LocalProbe> = &self.local;
        let read = probe.value.clone().unwrap_or_default();
        let servers = scan_local(&ws.agents, &read.clis, &ws.settings.disabled_agents);
        let up = servers.iter().any(|s| s.running);
        let this = crate::words::words().this_computer;
        let mut out = vec![];
        if !up {
            out.push(Self::note(
                &if detecting {
                    "Looking for model servers…".to_string()
                } else {
                    format!(
                        "No model server is running on {this}. Start one and it shows up here on its own; Ollama's address is read from OLLAMA_HOST when that is set."
                    )
                },
                cx,
            ));
        }
        let address = unusual_address(&read.ollama);
        let rows = servers
            .into_iter()
            .map(|s| self.local_row(s, address.as_deref(), muted, cx))
            .collect();
        out.push(ui::group(rows, cx));
        out.push(
            h_flex()
                .pt_3()
                .child(
                    Button::new("rescan-local")
                        .small()
                        .outline()
                        .loading(detecting)
                        .icon(IconName::RefreshCw)
                        .label("Scan again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.local.invalidate();
                            this.workspace.update(cx, |ws, cx| ws.detect_agents(cx));
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        out
    }

    fn local_row(
        &self,
        s: LocalServer,
        address: Option<&str>,
        muted: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = s.id;
        let agent = AgentId::Direct(id.into());
        let key = agent.key();
        let at = address.filter(|_| id == "ollama");
        let title = h_flex()
            .gap(px(10.))
            .child(ui::agent_logo(&agent, px(18.), cx))
            .child(div().text_size(px(text::TITLE)).font_medium().child(s.name));
        let detail = if s.running {
            let used = if s.models.is_empty() {
                "Running. Pull a model, e.g. ollama pull qwen3-coder".to_string()
            } else {
                s.models.join(", ")
            };
            match at {
                Some(at) => format!("At {at}. {used}"),
                None => used,
            }
        } else if !s.enabled {
            "Not running, and turned off in Trek.".to_string()
        } else if s.installed {
            "Installed, not running. Start it and Trek finds it on its usual port.".to_string()
        } else {
            "Not installed.".to_string()
        };
        // Using it is the user's choice for as long as it's up, and stays theirs while it's off.
        let control = if s.running || !s.enabled {
            let ws = self.workspace.clone();
            let toggle = Switch::new(SharedString::from(format!("local-{id}-on")))
                .checked(s.enabled)
                .on_click(move |on: &bool, _, cx| {
                    let (key, on) = (key.clone(), *on);
                    ws.update(cx, |ws, cx| {
                        ws.settings.disabled_agents.retain(|d| *d != key);
                        if !on {
                            ws.settings.disabled_agents.push(key);
                        }
                        ws.save_settings(cx);
                    })
                });
            let state = if !s.running {
                Self::status_dot(muted.opacity(0.5), "Not running")
            } else if s.models.is_empty() {
                Self::status_dot(crate::palette::amber(cx), "No models")
            } else {
                Self::status_dot(crate::palette::emerald(cx), "Ready")
            };
            h_flex()
                .gap(px(12.))
                .child(state)
                .child(
                    div()
                        .id(SharedString::from(format!("local-{id}-use")))
                        .test_support()
                        .child(toggle),
                )
                .into_any_element()
        } else if s.installed {
            match s.start_command() {
                Some(command) => Button::new(SharedString::from(format!("local-{id}-start")))
                    .small()
                    .outline()
                    .icon(Icon::new(crate::assets::Lucide::Play))
                    .label("Start")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.update(cx, |_, cx| {
                            cx.emit(WorkspaceEvent::RunInTerminal {
                                command: command.to_string(),
                                cwd: None,
                            })
                        })
                    }))
                    .into_any_element(),
                None => Self::status_dot(muted.opacity(0.5), "Not running"),
            }
        } else {
            let url = s.download();
            Button::new(SharedString::from(format!("local-{id}-get")))
                .small()
                .outline()
                .icon(IconName::ExternalLink)
                .label(format!(
                    "Get {}",
                    s.name.split(" /").next().unwrap_or(s.name)
                ))
                .on_click(move |_, _, cx| cx.open_url(url))
                .into_any_element()
        };
        Self::row(title, detail, control, cx)
    }
}
