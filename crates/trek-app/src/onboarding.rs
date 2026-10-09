//! First run: Welcome → Your agents → Your threads → Hand-holding → Project → Ready.
//!
//! One fixed column, left-aligned like the rest of Trek; the footer never moves between steps,
//! so Continue stays under the pointer. Lists are hairline rows with real logos, choices are
//! radio rows, and the accent colour is kept for the primary button and the selected choice.

use crate::brand;
use crate::palette;
use crate::ui;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;
use trek_core::detect::Availability;
use trek_core::{AgentId, HandHolding};

const STEPS: usize = 6;
const COLUMN: f32 = 560.;

pub struct Onboarding {
    workspace: Entity<Workspace>,
    step: usize,
    imported: bool,
    /// The project picked on the Project step; opened when onboarding finishes.
    project: Option<std::path::PathBuf>,
    last_step_change: std::time::Instant,
    /// The agents step lists every agent that isn't installed, not just the first few.
    all_missing: bool,
    _subscriptions: Vec<Subscription>,
}

/// Agents the agents step always lists before the rest that aren't installed.
const MISSING_SHOWN: usize = 4;

/// Where an agent stands on this Mac, as the agents step shows it.
#[derive(Debug, Clone, PartialEq)]
enum AgentState {
    Connected,
    NotSignedIn,
    NotInstalled,
}

/// Every coding agent Trek can run, in the order the agents step lists them: what detection
/// found, else what Trek knows of (detection doesn't run in an isolated process).
fn known_agents(detected: &[trek_core::detect::DetectedAgent]) -> Vec<(AgentId, String, Option<trek_core::detect::DetectedAgent>)> {
    let mut ids = vec![AgentId::ClaudeCode, AgentId::Codex, AgentId::OpenCode, AgentId::Droid];
    ids.extend(trek_core::catalog::ACP_AGENTS.iter().map(|a| AgentId::Acp(a.id.into())));
    for a in detected.iter().filter(|a| !matches!(a.agent, AgentId::Direct(_))) {
        if !ids.contains(&a.agent) {
            ids.push(a.agent.clone());
        }
    }
    ids.into_iter()
        .map(|id| {
            let found = detected.iter().find(|a| a.agent == id).cloned();
            let name = found.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| id.display_name());
            (id, name, found)
        })
        .collect()
}

/// `catalog::agent_setup` is keyed by the ACP id alone.
fn setup_for(agent: &AgentId) -> Option<trek_core::catalog::AgentSetup> {
    match agent {
        AgentId::Acp(id) => trek_core::catalog::agent_setup(id),
        other => trek_core::catalog::agent_setup(&other.key()),
    }
}

/// "2.1.287 (Claude Code)" / "grok 1.0.46 (…) [stable]" → "2.1.287" / "1.0.46".
fn clean_version(v: &str) -> Option<String> {
    v.split(|c: char| c.is_whitespace() || c == '(' || c == ',')
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
        .map(|t| t.trim_end_matches('.').to_string())
}

impl Onboarding {
    pub fn new(workspace: Entity<Workspace>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        Self { workspace, step: 0, imported: false, project: None, last_step_change: std::time::Instant::now(), all_missing: false, _subscriptions: subs }
    }

    fn go(&mut self, step: usize, cx: &mut Context<Self>) {
        // Ignore click bursts while a step is still animating in.
        if self.last_step_change.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_step_change = std::time::Instant::now();
        self.step = step.min(STEPS - 1);
        if self.step == 4 && self.project.is_none() {
            self.project = self.workspace.read(cx).workspace_projects().first().map(|p| p.path.clone());
        }
        if self.step == 2 && !self.imported {
            self.imported = true;
            self.workspace.update(cx, |ws, cx| ws.import_threads(cx));
        }
        cx.notify();
    }

    /// Add a folder as a project without leaving onboarding (Workspace::open_folder navigates).
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Choose Project".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                let added = this.workspace.update(cx, |ws, cx| {
                    let p = ws.store.ensure_project(&path).ok()?;
                    let key = p.path.display().to_string();
                    if !ws.settings.user_projects.contains(&key) {
                        ws.settings.user_projects.push(key);
                        ws.save_settings(cx);
                    }
                    ws.reload(cx);
                    Some(p.path)
                });
                if let Some(p) = added {
                    this.project = Some(p);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn header(title: &str, body: &str, cx: &App) -> AnyElement {
        v_flex()
            .gap(px(8.))
            .pb(px(28.))
            .child(div().text_size(px(26.)).font_semibold().line_height(relative(1.2)).child(title.to_string()))
            .child(div().text_size(px(14.)).line_height(relative(1.55)).text_color(cx.theme().muted_foreground).child(body.to_string()))
            .into_any_element()
    }

    /// A hairline list row: leading mark, title over detail, trailing element.
    fn list_row(lead: AnyElement, title: impl Into<SharedString>, detail: impl Into<SharedString>, trail: AnyElement, cx: &App) -> Div {
        let theme = cx.theme();
        let detail: SharedString = detail.into();
        h_flex()
            .min_h(px(52.))
            .py(px(10.))
            .gap(px(12.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(div().w(px(22.)).flex_none().flex().justify_center().child(lead))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(13.5)).font_medium().child(title.into()))
                    .when(!detail.is_empty(), |el| el.child(div().truncate().text_size(px(12.5)).text_color(theme.muted_foreground).child(detail))),
            )
            .child(trail)
    }

    fn list(rows: impl IntoIterator<Item = AnyElement>, cx: &App) -> AnyElement {
        v_flex().border_t_1().border_color(cx.theme().foreground.opacity(0.07)).children(rows).into_any_element()
    }

    fn state_label(text: &str, color: Hsla, icon: Option<Icon>) -> AnyElement {
        h_flex()
            .gap(px(5.))
            .text_size(px(12.5))
            .text_color(color)
            .children(icon.map(|i| i.size(px(13.)).text_color(color)))
            .child(text.to_string())
            .into_any_element()
    }

    fn welcome(&self, reduce: bool, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let point = |icon: Icon, title: &str, body: &str| {
            h_flex()
                .gap(px(14.))
                .items_start()
                .child(div().mt(px(1.)).child(icon.size(px(16.)).text_color(theme.muted_foreground)))
                .child(
                    v_flex()
                        .gap(px(2.))
                        .child(div().text_size(px(13.5)).font_medium().child(title.to_string()))
                        .child(div().text_size(px(13.)).line_height(relative(1.5)).text_color(theme.muted_foreground).child(body.to_string())),
                )
        };
        v_flex()
            .child(div().pb(px(24.)).child(brand::cairn_draw("welcome-cairn", px(64.), reduce)))
            .child(Self::header("Welcome to Trek", "One native app for every coding agent you use. A minute of setup and you're in.", cx))
            .child(
                v_flex()
                    .gap(px(20.))
                    .child(point(Icon::new(IconName::Bot), "Bring the agents you already pay for", "Claude Code, Codex, OpenCode and more run with your own logins and plans."))
                    .child(point(Icon::new(IconName::Inbox), "Pick up where you left off", "Conversations from other agents on this Mac appear in your sidebar, read-only."))
                    .child(point(Icon::new(crate::assets::Lucide::ShieldCheck), "Stay in control", "Choose how much an agent may do on its own, per thread.")),
            )
            .into_any_element()
    }

    /// A command to paste into a terminal, with a button that copies it.
    fn command_chip(id: &str, label: &str, command: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let copied = command.to_string();
        h_flex()
            .pt(px(6.))
            .gap(px(8.))
            .min_w_0()
            .child(div().flex_none().text_size(px(12.)).text_color(theme.muted_foreground).child(label.to_string()))
            .child(
                h_flex()
                    .min_w_0()
                    .h(px(26.))
                    .pl(px(8.))
                    .pr(px(2.))
                    .gap(px(4.))
                    .rounded(px(6.))
                    .bg(theme.foreground.opacity(0.05))
                    .child(div().min_w_0().truncate().font_family(theme.mono_font_family.clone()).text_size(px(12.)).child(command.to_string()))
                    .child(
                        Button::new(SharedString::from(format!("ob-copy-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Copy).text_color(theme.muted_foreground))
                            .tooltip("Copy the command")
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                crate::toast::push(window, "Command copied", cx);
                            }),
                    ),
            )
            .into_any_element()
    }

    /// One agent: logo, name and what's known of it, its state, and the command that installs it
    /// or signs in to it when it needs one.
    fn agent_row(&self, agent: &AgentId, name: &str, detail: String, state: AgentState, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = agent.key().replace(':', "-");
        let setup = setup_for(agent);
        let (label, color, icon) = match state {
            AgentState::Connected => ("Connected", palette::emerald(cx), Some(Icon::new(IconName::Check))),
            AgentState::NotSignedIn => ("Not signed in", palette::amber(cx), None),
            AgentState::NotInstalled => ("Not installed", theme.muted_foreground, None),
        };
        let fix = match state {
            AgentState::Connected => None,
            AgentState::NotSignedIn => setup.map(|s| Self::command_chip(&id, "Sign in", s.login, cx)),
            AgentState::NotInstalled => setup.map(|s| Self::command_chip(&id, "Install", s.install, cx)),
        };
        h_flex()
            .id(SharedString::from(format!("ob-agent-{id}")))
            .test_support()
            .min_h(px(52.))
            .py(px(10.))
            .gap(px(12.))
            .items_start()
            .border_b_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(div().pt(px(1.)).w(px(22.)).flex_none().flex().justify_center().child(ui::agent_logo(agent, px(18.), cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(13.5)).font_medium().child(name.to_string()))
                            .child(Self::state_label(label, color, icon)),
                    )
                    .when(!detail.is_empty(), |el| el.child(div().truncate().text_size(px(12.5)).text_color(theme.muted_foreground).child(detail)))
                    .children(fix),
            )
            .into_any_element()
    }

    /// Every agent Trek can run and where it stands here: connected, not signed in, or not
    /// installed, with the command that fixes the last two.
    fn agents_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let detecting = ws.detecting;
        let known = known_agents(&ws.agents);
        let mut found: Vec<(AgentId, String, String, AgentState)> = vec![];
        let mut missing: Vec<(AgentId, String)> = vec![];
        for (agent, name, detected) in known {
            let Some(a) = detected.filter(|a| a.availability != Availability::NotInstalled) else {
                // Not found yet while a scan runs: it may still turn up.
                if !detecting {
                    missing.push((agent, name));
                }
                continue;
            };
            let status = ws.agent_status.get(&a.agent.key());
            let acp = ws.acp_info.get(&a.agent.key());
            let needs_login = a.availability == Availability::NeedsLogin
                || status.is_some_and(|st| !st.logged_in && st.account.is_none())
                || acp.is_some_and(|i| i.as_ref().map_or(true, |i| i.needs_auth));
            let version = a.version.as_deref().and_then(clean_version).map(|v| format!("Version {v}"));
            let detail = match (status, acp) {
                (Some(st), _) if st.account.is_some() || st.plan.is_some() => [st.account.clone(), st.plan.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · "),
                (_, Some(Ok(i))) if !i.needs_auth && !i.models.is_empty() => {
                    [Some(format!("{} models", i.models.len())), version.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ")
                }
                _ => version.unwrap_or_default(),
            };
            found.push((agent, name, detail, if needs_login { AgentState::NotSignedIn } else { AgentState::Connected }));
        }
        let none = found.is_empty() && !detecting;
        let body = if none {
            "Trek drives each vendor's own agent with the login you already have. None is installed on this Mac yet: install one below, sign in to it, then scan again."
        } else {
            "Trek drives each vendor's own agent with the login you already have. Your credentials never pass through Trek."
        };
        let found_rows: Vec<AnyElement> = found.into_iter().map(|(agent, name, detail, state)| self.agent_row(&agent, &name, detail, state, cx)).collect();
        let more = missing.len().saturating_sub(MISSING_SHOWN);
        let shown = if self.all_missing { missing.len() } else { missing.len().min(MISSING_SHOWN) };
        let missing_rows: Vec<AnyElement> = missing.iter().take(shown).map(|(agent, name)| self.agent_row(agent, name, String::new(), AgentState::NotInstalled, cx)).collect();
        let section = |text: &str| div().pt(px(22.)).pb(px(8.)).text_size(px(12.5)).font_medium().text_color(theme.muted_foreground).child(text.to_string());
        v_flex()
            .child(Self::header("Your agents", body, cx))
            .when(detecting && found_rows.is_empty(), |el| {
                el.child(h_flex().gap(px(10.)).py(px(16.)).text_size(px(13.)).text_color(theme.muted_foreground).child(Spinner::new().small()).child("Looking for agents on this Mac…"))
            })
            .when(!found_rows.is_empty(), |el| el.child(Self::list(found_rows, cx)))
            .when(!missing_rows.is_empty(), |el| {
                el.when(!none, |el| el.child(section("Not installed"))).child(Self::list(missing_rows, cx))
            })
            .child(
                h_flex()
                    .pt(px(14.))
                    .gap(px(8.))
                    .when(more > 0 && !self.all_missing, |el| {
                        el.child(Button::new("ob-agents-more").small().ghost().label(format!("Show {more} more")).on_click(cx.listener(|this, _, _, cx| {
                            this.all_missing = true;
                            cx.notify();
                        })))
                    })
                    .when(!missing.is_empty() && !detecting, |el| {
                        el.child(Button::new("ob-rescan").small().outline().icon(IconName::RefreshCw).label("Scan again").on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| ws.detect_agents(cx))
                        })))
                    })
                    // An agent Trek doesn't list: from the ACP Registry, or its own command.
                    .child(Button::new("ob-add-agent").small().ghost().icon(IconName::Plus).label("Add another agent…").on_click(cx.listener(|this, _, window, cx| {
                        crate::add_agent::open(this.workspace.clone(), crate::add_agent::Tab::Registry, window, cx)
                    }))),
            )
            .into_any_element()
    }

    fn threads_step(&self, cx: &App) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let body = "Trek reads, and never changes, the conversations other agents keep on this Mac.";
        let content = match ws.import_summary.clone() {
            None => h_flex().gap(px(10.)).py(px(16.)).text_size(px(13.)).text_color(theme.muted_foreground).child(Spinner::new().small()).child("Reading your history…").into_any_element(),
            Some(s) => {
                let total = s.claude_code + s.codex + s.opencode;
                let rows = [(AgentId::ClaudeCode, "Claude Code", s.claude_code), (AgentId::Codex, "Codex", s.codex), (AgentId::OpenCode, "OpenCode", s.opencode)]
                    .into_iter()
                    .map(|(id, name, n)| {
                        let trail = div().text_size(px(13.)).text_color(if n > 0 { theme.foreground } else { theme.muted_foreground }).child(if n == 1 { "1 thread".to_string() } else { format!("{n} threads") });
                        Self::list_row(ui::agent_logo(&id, px(18.), cx), name, "", trail.into_any_element(), cx).into_any_element()
                    });
                v_flex()
                    .child(Self::list(rows, cx))
                    .child(div().pt(px(14.)).text_size(px(12.5)).text_color(theme.muted_foreground).child(if total == 0 {
                        "Nothing to bring over yet. New threads will collect in your sidebar.".to_string()
                    } else {
                        format!("{total} threads across {} projects. Finished ones wait under Settled.", ws.projects.len())
                    }))
                    .into_any_element()
            }
        };
        v_flex().child(Self::header("Your threads", body, cx)).child(content).into_any_element()
    }

    fn hand_holding_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.workspace.read(cx).draft_prefs.hand_holding;
        let rows: Vec<AnyElement> = HandHolding::ALL
            .into_iter()
            .map(|h| {
                let locked = h == HandHolding::FullAccess;
                let selected = h == current;
                let radio = div()
                    .size(px(16.))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected { palette::ember(cx) } else { theme.foreground.opacity(0.25) })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |el| el.child(div().size(px(8.)).rounded_full().bg(palette::ember(cx))));
                let detail = if locked { "No prompts and no sandbox. Unlock it later in Settings → Permissions." } else { h.description() };
                Self::list_row(radio.into_any_element(), h.label(), detail, div().into_any_element(), cx)
                    .id(SharedString::from(format!("ob-hh-{h:?}")))
                    .test_support()
                    .when(locked, |el| el.opacity(0.5))
                    .when(!locked, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.draft_prefs.hand_holding = h;
                                cx.notify();
                            })
                        }))
                    })
                    .into_any_element()
            })
            .collect();
        v_flex()
            .child(Self::header("How much should agents ask first?", "This is the default for new threads. You can change it for any thread from the composer.", cx))
            .child(Self::list(rows, cx))
            .into_any_element()
    }

    fn project_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let selected = self.project.clone();
        let projects: Vec<_> = ws.workspace_projects().into_iter().take(6).cloned().collect();
        let rows: Vec<AnyElement> = projects
            .into_iter()
            .map(|p| {
                let path = p.path.clone();
                let is_sel = selected.as_ref() == Some(&p.path);
                let detail = p.remote.clone().unwrap_or_else(|| trek_core::paths::tildify(&p.path));
                let trail = if is_sel { Icon::new(IconName::Check).size(px(15.)).text_color(palette::ember(cx)).into_any_element() } else { div().into_any_element() };
                Self::list_row(ui::project_badge(&p.name, &ws.project_look(&p.path), cx), p.name.clone(), detail, trail, cx)
                    .id(SharedString::from(format!("ob-proj-{}", p.id)))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.foreground.opacity(0.03)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.project = Some(path.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        v_flex()
            .child(Self::header("Where are you working?", "Pick a project for your first thread. You can add more from the sidebar any time.", cx))
            .when(!empty, |el| el.child(Self::list(rows, cx)))
            .child(
                h_flex()
                    .pt(px(16.))
                    .gap(px(8.))
                    .child(Button::new("ob-open-folder").small().outline().icon(IconName::FolderOpen).label("Open folder…").on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))),
            )
            .into_any_element()
    }

    fn ready_step(&self, reduce: bool, cx: &App) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let agent = ws.draft_prefs.agent.clone();
        let project = self.project.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "None yet".into());
        let summary = [
            ("Agent", agent.display_name()),
            ("Hand-holding", ws.draft_prefs.hand_holding.label().to_string()),
            ("First project", project),
        ];
        v_flex()
            .child(div().pb(px(24.)).child(brand::cairn_draw("ready-cairn", px(64.), reduce)))
            .child(Self::header("You're ready", "Threads that need a decision rise to the top of your inbox. Everything here can be changed in Settings.", cx))
            .child(Self::list(
                summary.into_iter().map(|(k, v)| {
                    h_flex()
                        .h(px(44.))
                        .border_b_1()
                        .border_color(theme.foreground.opacity(0.07))
                        .child(div().w(px(160.)).text_size(px(13.)).text_color(theme.muted_foreground).child(k))
                        .child(div().text_size(px(13.5)).child(v))
                        .into_any_element()
                }),
                cx,
            ))
            .into_any_element()
    }
}

impl Render for Onboarding {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let theme = cx.theme().clone();
        let reduce = !self.workspace.read(cx).motion(cx);
        let content = match step {
            0 => self.welcome(reduce, cx),
            1 => self.agents_step(cx),
            2 => self.threads_step(cx),
            3 => self.hand_holding_step(cx),
            4 => self.project_step(cx),
            _ => self.ready_step(reduce, cx),
        };
        let primary = match step {
            0 => "Get started",
            s if s == STEPS - 1 => "Open Trek",
            _ => "Continue",
        };
        let animated = div().w_full().child(content).with_animation(
            ("onboarding-step", step),
            Animation::new(Duration::from_millis(if reduce { 160 } else { 300 })).with_easing(ease_out_quint()),
            move |el, t| if reduce { el.opacity(t) } else { el.opacity(t).mt(px(8. * (1. - t))) },
        );
        let footer = h_flex()
            .w(px(COLUMN))
            .h(px(64.))
            .border_t_1()
            .border_color(theme.foreground.opacity(0.07))
            .child(div().flex_1().text_size(px(12.5)).text_color(theme.muted_foreground).child(format!("Step {} of {STEPS}", step + 1)))
            .when(step > 0, |el| {
                el.child(Button::new("ob-back").ghost().label("Back").on_click(cx.listener(|this, _, _, cx| {
                    let s = this.step.saturating_sub(1);
                    this.go(s, cx)
                })))
            })
            .child(div().w(px(8.)))
            .child(Button::new("ob-next").primary().label(primary).on_click(cx.listener(move |this, _, _, cx| {
                if step == STEPS - 1 {
                    let project = this.project.clone();
                    this.workspace.update(cx, |ws, cx| {
                        ws.finish_onboarding(cx);
                        if project.is_some() {
                            ws.navigate(Route::Draft { project }, cx);
                        }
                    });
                } else {
                    this.go(step + 1, cx);
                }
            })));
        // The footer is pinned to the window, so Continue never moves between steps.
        div()
            .relative()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .id("onboarding-body")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom(px(88.))
                    .overflow_y_scroll()
                    .child(h_flex().w_full().justify_center().child(v_flex().w(px(COLUMN)).pt(px(88.)).pb(px(32.)).child(animated))),
            )
            .child(h_flex().absolute().left_0().right_0().bottom(px(24.)).justify_center().child(footer))
    }
}
