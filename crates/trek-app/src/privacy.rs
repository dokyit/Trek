//! Telemetry in the app (`trek_telemetry`): started at launch, the card that asks once
//! (after onboarding, never in a test or the verify profile, never with `DO_NOT_TRACK`), and
//! the names agents and tools are counted under. Settings › Privacy changes the answer
//! (`settings_view::privacy`); Help › Send Feedback… is `feedback`.

use crate::scale::text;
use crate::workspace::{Route, SettingsPage, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use trek_core::AgentId;

const CARD_WIDTH: f32 = 400.;

/// What's shared, as the card and Settings › Privacy say it.
pub const SHARED: &str = "Anonymous usage counts (how often Trek opens, which agents and tools you use), crash and error reports, the app version, OS and architecture.";
/// What never is.
pub const NEVER: &str = "Prompts, code, file names, paths, transcripts, keys or account emails.";

/// Start telemetry for this run, once the workspace has read the settings. A send that
/// finishes redraws the windows (Settings › Privacy shows what went).
pub fn start(ws: &Entity<Workspace>, cx: &mut App) {
    let (channel, set_up_before) = {
        let s = &ws.read(cx).settings;
        (
            format!("{:?}", s.updates.channel).to_lowercase(),
            s.onboarding.completed,
        )
    };
    trek_telemetry::start(trek_telemetry::Start {
        data_dir: trek_core::paths::data_dir(),
        channel,
        set_up_before,
        os_version,
        scrubber: scrubber(ws.read(cx)),
    });
    if let Some(changes) = trek_telemetry::changes() {
        cx.spawn(async move |cx| {
            while changes.recv().await.is_ok() {
                cx.update(|cx| cx.refresh_windows());
            }
        })
        .detach();
    }
    ask_when_ready(ws.clone(), cx);
}

/// What takes paths, emails and secrets out of a report, knowing this user's projects.
pub fn scrubber(ws: &Workspace) -> trek_telemetry::Scrubber {
    let home = trek_core::paths::home();
    let data = trek_core::paths::data_dir();
    let logs = crate::logging::log_dir();
    let mut folders: Vec<std::path::PathBuf> = ws
        .settings
        .user_projects
        .iter()
        .chain(ws.settings.projects.keys())
        .map(std::path::PathBuf::from)
        .chain(ws.projects.iter().map(|p| p.path.clone()))
        .chain(ws.threads.iter().filter_map(|t| t.cwd.clone()))
        // Never the home folder itself, or a folder Trek's own are in: they'd hide everything.
        .filter(|p| *p != home && !data.starts_with(p) && !logs.starts_with(p))
        .collect();
    folders.sort();
    folders.dedup();
    let folders: Vec<String> = folders.iter().map(|p| p.display().to_string()).collect();
    trek_telemetry::Scrubber::here(&folders, crate::share::render::redact_secrets)
}

/// The OS version alone (`26.1`; on Windows `11 (26100)`), off the UI thread.
fn os_version() -> Option<String> {
    #[cfg(windows)]
    {
        let v = crate::system::windows_version();
        Some(v.strip_prefix("Windows ").map(str::to_string).unwrap_or(v))
    }
    #[cfg(target_os = "macos")]
    {
        trek_core::procs::command("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Holds the watch on the workspace until the card has been put up.
struct Asking {
    _watch: Option<Subscription>,
}

impl Global for Asking {}

/// Put the card up once, as soon as the main window is past onboarding.
fn ask_when_ready(ws: Entity<Workspace>, cx: &mut App) {
    if !trek_telemetry::should_ask() || try_ask(&ws, cx) {
        return;
    }
    let watch = cx.observe(&ws, |ws, cx| {
        if try_ask(&ws, cx) {
            cx.defer(|cx| cx.set_global(Asking { _watch: None }));
        }
    });
    cx.set_global(Asking {
        _watch: Some(watch),
    });
}

/// Whether there's nothing left to wait for: asked now, or no longer to be asked.
fn try_ask(ws: &Entity<Workspace>, cx: &mut App) -> bool {
    if !trek_telemetry::should_ask() {
        return true;
    }
    let w = ws.read(cx);
    if w.route == Route::Onboarding || w.settings.onboarding.due() {
        return false;
    }
    let Some(main) = w.main_window else {
        return false;
    };
    main.update(cx, |_, window, _| {
        window.on_next_frame(|window, cx| open_card(window, cx))
    })
    .is_ok()
}

/// The card: what's shared and what never is, Yes or No. It takes an answer (no ×, no click
/// beside it), and either is changed in Settings › Privacy.
pub fn open_card(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let line = |label: &'static str, text: &'static str| {
            v_flex()
                .gap(px(2.))
                .child(div().text_size(px(text::BODY)).font_medium().child(label))
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .line_height(relative(1.5))
                        .text_color(muted)
                        .child(text),
                )
        };
        dialog
            .w(px(CARD_WIDTH))
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .child(
                v_flex()
                    .id("consent-card")
                    .test_support()
                    .gap(px(14.))
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(text::HEADING))
                                    .font_semibold()
                                    .child("Share anonymous usage data?"),
                            )
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .line_height(relative(1.5))
                                    .text_color(muted)
                                    .child("It shows what breaks and how many people use Trek."),
                            ),
                    )
                    .child(line("Shared", SHARED))
                    .child(line("Never shared", NEVER))
                    .child(
                        div()
                            .text_size(px(text::LABEL))
                            .text_color(muted)
                            .child("You can change this in Settings › Privacy."),
                    )
                    .child(
                        h_flex()
                            .pt(px(4.))
                            .gap(px(8.))
                            .justify_end()
                            .child(
                                Button::new("consent-no")
                                    .outline()
                                    .label("No")
                                    .on_click(|_, window, cx| answer(false, window, cx)),
                            )
                            .child(
                                Button::new("consent-yes")
                                    .primary()
                                    .label("Yes")
                                    .on_click(|_, window, cx| answer(true, window, cx)),
                            ),
                    ),
            )
    });
}

fn answer(yes: bool, window: &mut Window, cx: &mut App) {
    trek_telemetry::set_consent(yes);
    window.close_dialog(cx);
    cx.refresh_windows();
}

/// Go to Settings › Privacy in the main window.
pub fn open_settings(ws: &Entity<Workspace>, cx: &mut App) {
    ws.update(cx, |ws, cx| {
        ws.navigate(Route::Settings(SettingsPage::Privacy), cx)
    });
}

/// The name an agent is counted under: the built-in agents' and the registry's own ids, and a
/// provider Trek knows. One the user named (their own command, a custom endpoint) is `other`.
pub fn agent_name(agent: &AgentId, ws: &Workspace) -> String {
    match agent {
        AgentId::Acp(id) => {
            let known =
                trek_core::catalog::ACP_AGENTS
                    .iter()
                    .any(|a| a.id == id.as_str())
                    || ws.settings.added_agents.iter().any(|a| {
                        a.id == *id && a.source == trek_core::registry::AgentSource::Registry
                    });
            if known {
                agent.key()
            } else {
                "acp:other".into()
            }
        }
        AgentId::Direct(id) => {
            let known = id == "mock"
                || trek_core::catalog::DIRECT_PROVIDERS
                    .iter()
                    .any(|p| p.id == id.as_str());
            if known {
                agent.key()
            } else {
                "direct:other".into()
            }
        }
        _ => agent.key(),
    }
}

/// One more message to `thread`'s agent today (with the user's yes).
pub fn count_agent(ws: &Workspace, thread: &str) {
    if let Some(t) = ws.thread(thread) {
        trek_telemetry::count(trek_telemetry::Feature::Agent, &agent_name(&t.agent, ws));
    }
}

/// One more opening of `tool` today (with the user's yes).
pub fn count_tool(tool: crate::workspace::PanelTool) {
    use crate::workspace::PanelTool;
    let name = match tool {
        PanelTool::Git => "git",
        PanelTool::Explorer => "explorer",
        PanelTool::Terminal => "terminal",
        PanelTool::Browser => "browser",
        PanelTool::SideChat => "side-chat",
        PanelTool::Simulator => "simulator",
        PanelTool::Phone => "iphone",
    };
    trek_telemetry::count(trek_telemetry::Feature::Tool, name);
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would come with it.
    use super::{NEVER, SHARED};

    #[test]
    fn the_card_names_what_never_leaves() {
        for never in [
            "Prompts",
            "code",
            "file names",
            "paths",
            "transcripts",
            "keys",
            "account emails",
        ] {
            assert!(NEVER.contains(never), "{never}");
        }
        for shared in [
            "usage counts",
            "crash",
            "error reports",
            "version",
            "OS",
            "architecture",
        ] {
            assert!(SHARED.contains(shared), "{shared}");
        }
    }
}
