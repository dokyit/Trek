//! First run: Welcome → Your agents → Bring your threads → Hand-holding → Open a project → Done.

use crate::brand;
use crate::palette;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;
use trek_core::detect::Availability;
use trek_core::{AgentId, HandHolding};

const STEPS: usize = 6;

pub struct Onboarding {
    workspace: Entity<Workspace>,
    step: usize,
    imported: bool,
    last_step_change: std::time::Instant,
    _subscriptions: Vec<Subscription>,
}

impl Onboarding {
    pub fn new(workspace: Entity<Workspace>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        Self { workspace, step: 0, imported: false, last_step_change: std::time::Instant::now(), _subscriptions: subs }
    }

    fn go(&mut self, step: usize, cx: &mut Context<Self>) {
        // Ignore click bursts while a step transition is still animating in.
        if self.last_step_change.elapsed() < std::time::Duration::from_millis(250) {
            return;
        }
        self.last_step_change = std::time::Instant::now();
        self.step = step.min(STEPS - 1);
        if self.step == 2 && !self.imported {
            self.imported = true;
            self.workspace.update(cx, |ws, cx| ws.import_threads(cx));
        }
        cx.notify();
    }

    fn title(text: &str) -> impl IntoElement {
        div().text_size(px(28.)).font_semibold().child(text.to_string())
    }

    fn subtitle(text: &str, cx: &App) -> impl IntoElement {
        div().text_color(cx.theme().muted_foreground).max_w(px(520.)).text_center().child(text.to_string())
    }

    /// Staggered entrance for list rows.
    fn stagger(el: impl IntoElement + 'static, id: impl Into<ElementId>, index: usize) -> impl IntoElement {
        div().child(el).with_animation(
            id,
            Animation::new(Duration::from_millis(320 + index as u64 * 60)).with_easing(ease_out_quint()),
            move |el, t| {
                // Hold each row back by its index, then ease in.
                let start = (index as f32 * 60.) / (320. + index as f32 * 60.);
                let p = ((t - start) / (1. - start)).clamp(0., 1.);
                el.opacity(p).mt(px(10. * (1. - p)))
            },
        )
    }

    fn step_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let reduce = ws.settings.appearance.reduce_motion;
        match self.step {
            0 => v_flex()
                .items_center()
                .gap_4()
                .child(brand::trail_draw("welcome-trail", px(180.), reduce))
                .child(div().text_size(px(40.)).font_semibold().child("Trek"))
                .child(Self::subtitle("Every agent. One trail. Run Claude Code, Codex, and any model you like from one fast, native app.", cx))
                .into_any_element(),
            1 => {
                let detecting = ws.detecting;
                let agents: Vec<_> = ws
                    .agents
                    .iter()
                    .filter(|a| a.availability != Availability::NotInstalled || matches!(a.agent, AgentId::ClaudeCode | AgentId::Codex))
                    .cloned()
                    .collect();
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(Self::title("Your agents"))
                    .child(Self::subtitle(
                        "Trek uses the agents you already pay for. It drives each vendor's own app with your login, so your subscriptions just work.",
                        cx,
                    ))
                    .child(
                        v_flex()
                            .w(px(440.))
                            .gap_1()
                            .when(detecting && agents.is_empty(), |el| el.child(h_flex().justify_center().p_4().child(Spinner::new())))
                            .children(agents.into_iter().enumerate().map(|(i, a)| {
                                let (icon, color, status) = match a.availability {
                                    Availability::Ready => (Icon::new(IconName::CircleCheck), palette::emerald(cx), a.version.clone().unwrap_or("Ready".into())),
                                    Availability::NeedsLogin => (Icon::new(IconName::CircleAlert), palette::amber(cx), "Sign in from your terminal".into()),
                                    Availability::Offline => (Icon::new(crate::assets::Lucide::CircleDashed), theme.muted_foreground, "Not running".into()),
                                    Availability::NotInstalled => {
                                        (Icon::new(crate::assets::Lucide::CircleDashed), theme.muted_foreground, a.install_hint.clone().unwrap_or_default())
                                    }
                                };
                                Self::stagger(
                                    h_flex()
                                        .gap_3()
                                        .px_3()
                                        .py_2()
                                        .rounded(theme.radius)
                                        .bg(theme.secondary)
                                        .child(icon.text_color(color))
                                        .child(div().font_medium().w(px(130.)).child(a.name.clone()))
                                        .child(div().flex_1().truncate().text_xs().text_color(theme.muted_foreground).child(status)),
                                    SharedString::from(format!("agent-row-{}", a.agent.key())),
                                    i,
                                )
                            })),
                    )
                    .into_any_element()
            }
            2 => {
                let summary = ws.import_summary.clone();
                let projects = ws.projects.len();
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(Self::title("Bring your threads"))
                    .child(Self::subtitle(
                        "Trek reads the conversations other agents keep on this Mac, read-only, so you can pick up where you left off.",
                        cx,
                    ))
                    .child(match summary {
                        None => h_flex().gap_2().child(Spinner::new()).child("Looking for threads…").into_any_element(),
                        Some(s) => v_flex()
                            .w(px(440.))
                            .gap_1()
                            .children(
                                [("Claude Code", s.claude_code), ("Codex", s.codex), ("OpenCode", s.opencode)].into_iter().enumerate().map(
                                    |(i, (name, n))| {
                                        Self::stagger(
                                            h_flex()
                                                .gap_3()
                                                .px_3()
                                                .py_2()
                                                .rounded(theme.radius)
                                                .bg(theme.secondary)
                                                .child(Icon::new(crate::assets::Lucide::MessageSquare).text_color(theme.muted_foreground))
                                                .child(div().flex_1().font_medium().child(name))
                                                .child(div().text_sm().child(format!("{n} threads"))),
                                            ("import-row", i),
                                            i,
                                        )
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .pt_2()
                                    .text_sm()
                                    .text_center()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("Across {projects} projects. They'll wait under Settled in your sidebar.")),
                            )
                            .into_any_element(),
                    })
                    .into_any_element()
            }
            3 => {
                let current = ws.draft_prefs.hand_holding;
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(Self::title("How much hand-holding?"))
                    .child(Self::subtitle("Pick a default. You can change it for any thread from the composer.", cx))
                    .child(h_flex().gap_3().children(HandHolding::ALL.into_iter().filter(|h| *h != HandHolding::FullAccess).map(|h| {
                        let selected = h == current;
                        let ring = palette::hand_holding(h, cx);
                        v_flex()
                            .id(SharedString::from(format!("hh-{h:?}")))
                            .w(px(190.))
                            .h(px(150.))
                            .p_4()
                            .gap_2()
                            .rounded(theme.radius_lg)
                            .border_2()
                            .cursor_pointer()
                            .border_color(if selected { palette::ember(cx) } else { theme.border })
                            .bg(if selected { palette::ember(cx).opacity(0.06) } else { theme.secondary })
                            .hover(|s| s.border_color(palette::ember(cx).opacity(0.6)))
                            .child(div().size(px(10.)).rounded_full().bg(ring))
                            .child(div().font_semibold().child(h.label()))
                            .child(div().text_sm().text_color(theme.muted_foreground).child(h.description()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.draft_prefs.hand_holding = h;
                                    cx.notify();
                                })
                            }))
                    })))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Full access (no prompts, no sandbox) can be unlocked later in Settings → Permissions."),
                    )
                    .into_any_element()
            }
            4 => {
                let projects: Vec<_> = ws.projects.iter().take(6).cloned().collect();
                let selected = match &ws.route {
                    Route::Draft { project } => project.clone(),
                    _ => None,
                };
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(Self::title("Open a project"))
                    .child(Self::subtitle("Start from a folder on this Mac or clone a repository from GitHub.", cx))
                    .child(
                        v_flex().w(px(460.)).gap_1().children(projects.into_iter().enumerate().map(|(i, p)| {
                            let path = p.path.clone();
                            let is_sel = selected.as_ref() == Some(&p.path);
                            Self::stagger(
                                h_flex()
                                    .id(SharedString::from(format!("proj-pick-{}", p.id)))
                                    .gap_3()
                                    .px_3()
                                    .py_2()
                                    .rounded(theme.radius)
                                    .cursor_pointer()
                                    .bg(if is_sel { palette::ember(cx).opacity(0.1) } else { theme.secondary })
                                    .hover(|s| s.bg(theme.list_hover))
                                    .child(Icon::new(IconName::Folder).text_color(theme.muted_foreground))
                                    .child(div().font_medium().child(p.name.clone()))
                                    .child(div().flex_1().truncate().text_xs().text_color(theme.muted_foreground).child(trek_core::paths::tildify(&p.path)))
                                    .when(is_sel, |el| el.child(Icon::new(IconName::Check).text_color(palette::ember(cx))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let path = path.clone();
                                        this.workspace.update(cx, |ws, cx| {
                                            ws.route = Route::Draft { project: Some(path) };
                                            cx.notify();
                                        })
                                    })),
                                SharedString::from(format!("proj-anim-{}", p.id)),
                                i,
                            )
                        })),
                    )
                    .child(
                        h_flex().gap_2().child(
                            Button::new("ob-open-folder")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Open folder…")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.open_folder(cx);
                                    })
                                })),
                        ),
                    )
                    .into_any_element()
            }
            _ => v_flex()
                .items_center()
                .gap_4()
                .child(brand::trail_draw("done-trail", px(120.), reduce))
                .child(Self::title("You're all set"))
                .child(Self::subtitle("Your threads are in the sidebar. Agents that need you will light up the Inbox.", cx))
                .into_any_element(),
        }
    }
}

impl Render for Onboarding {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let theme = cx.theme().clone();
        let content = self.step_content(cx);
        let (primary, back) = match step {
            0 => ("Get started", false),
            5 => ("Start your trek", true),
            _ => ("Continue", true),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_8()
            .child(
                div().min_h(px(420.)).flex().items_center().child(content).with_animation(
                    ("onboarding-step", step),
                    Animation::new(Duration::from_millis(420)).with_easing(ease_out_quint()),
                    |el, t| el.opacity(t).ml(px(24. * (1. - t))),
                ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .when(back, |el| {
                        el.child(Button::new("ob-back").ghost().label("Back").on_click(cx.listener(|this, _, _, cx| {
                            let s = this.step.saturating_sub(1);
                            this.go(s, cx)
                        })))
                    })
                    .child(Button::new("ob-next").primary().large().label(primary).on_click(cx.listener(move |this, _, _, cx| {
                        if step == STEPS - 1 {
                            this.workspace.update(cx, |ws, cx| ws.finish_onboarding(cx));
                        } else {
                            this.go(step + 1, cx);
                        }
                    }))),
            )
            .child(h_flex().gap_2().children((0..STEPS).map(|i| {
                div()
                    .h(px(6.))
                    .w(px(if i == step { 22. } else { 6. }))
                    .rounded_full()
                    .bg(if i <= step { palette::ember(cx) } else { theme.border })
            })))
    }
}
