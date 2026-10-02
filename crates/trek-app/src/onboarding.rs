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
    _subscriptions: Vec<Subscription>,
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
        Self { workspace, step: 0, imported: false, project: None, last_step_change: std::time::Instant::now(), _subscriptions: subs }
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
            .child(div().pb(px(24.)).child(brand::trail_draw("welcome-trail", px(64.), reduce)))
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

    fn agents_step(&self, cx: &App) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let (found, missing): (Vec<_>, Vec<_>) =
            ws.agents.iter().filter(|a| !matches!(a.agent, AgentId::Direct(_))).cloned().partition(|a| a.availability != Availability::NotInstalled);
        let detecting = ws.detecting && found.is_empty();
        let rows: Vec<AnyElement> = found
            .iter()
            .map(|a| {
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
                let trail = match a.availability {
                    _ if needs_login => Self::state_label("Sign in later", palette::amber(cx), None),
                    Availability::Ready => Self::state_label("Ready", palette::emerald(cx), Some(Icon::new(IconName::Check))),
                    _ => Self::state_label("Not running", theme.muted_foreground, None),
                };
                Self::list_row(ui::agent_logo(&a.agent, px(18.), cx), a.name.clone(), detail, trail, cx).into_any_element()
            })
            .collect();
        v_flex()
            .child(Self::header("Your agents", "Trek drives each vendor's own agent with the login you already have. Your credentials never pass through Trek.", cx))
            .when(detecting, |el| el.child(h_flex().gap(px(10.)).py(px(16.)).text_size(px(13.)).text_color(theme.muted_foreground).child(Spinner::new().small()).child("Looking for agents on this Mac…")))
            .when(!rows.is_empty(), |el| el.child(Self::list(rows, cx)))
            .when(!missing.is_empty(), |el| {
                el.child(div().pt(px(14.)).text_size(px(12.5)).text_color(theme.muted_foreground).child(format!(
                    "{} more, including {}, can be installed later from Settings → Agents.",
                    missing.len(),
                    missing.iter().take(2).map(|a| a.name.clone()).collect::<Vec<_>>().join(" and ")
                )))
            })
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
                Self::list_row(ui::monogram(&p.name, cx).into_any_element(), p.name.clone(), detail, trail, cx)
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
            .child(div().pb(px(24.)).child(brand::trail_draw("ready-trail", px(64.), reduce)))
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
        let reduce = self.workspace.read(cx).settings.appearance.reduce_motion;
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
