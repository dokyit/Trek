//! What each onboarding step shows. The pieces (cards of hairline rows, a header, a state label)
//! follow the composer's look (`composer::look`): the card's fill, hairline and soft shadow.

use super::*;
use crate::composer::look;
use crate::palette;
use crate::ui;
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, IconName, StyledExt as _};
use std::cell::Cell;
use trek_core::HandHolding;
use trek_core::settings::{AppIcon, ThemeChoice};

/// The intro's length: the hiker walks up to the cairn while it stacks, then the words come.
const INTRO: Duration = Duration::from_millis(2200);

thread_local! {
    /// A press on a drag tile that hasn't become a drag yet. One pointer, so one for all tiles;
    /// kept outside the views, whose closures are made anew each frame.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Trek's icon, to drag into a list in System Settings as from Finder: a press, then a move with
/// the button down, hands the app to macOS's drag (`permissions::drag_app`).
pub(super) fn drag_tile(id: impl Into<ElementId>, size: f32, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    div()
        .id(id)
        .test_support()
        .flex_none()
        .size(px(size))
        .rounded(px(size * 0.22))
        .cursor(CursorStyle::OpenHand)
        .hover(|s| s.bg(theme.foreground.opacity(0.05)))
        .child(
            img(crate::system::app_icon_image(AppIcon::Ember))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            ARMED.with(|a| a.set(true));
            cx.stop_propagation();
        })
        .on_mouse_move(|e: &MouseMoveEvent, window, _| {
            if e.pressed_button == Some(MouseButton::Left) && ARMED.with(|a| a.replace(false)) {
                crate::permissions::drag_app(window);
            }
        })
        .on_mouse_up(MouseButton::Left, |_, _, _| ARMED.with(|a| a.set(false)))
        .into_any_element()
}

/// A step's title and what it's for.
fn header(title: &str, body: impl Into<SharedString>, cx: &App) -> AnyElement {
    v_flex()
        .gap(px(8.))
        .pb(px(24.))
        .child(
            div()
                .text_size(px(24.))
                .font_semibold()
                .line_height(relative(1.2))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(14.))
                .line_height(relative(1.55))
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .into_any_element()
}

/// Rows on a card: the composer card's fill, hairline and shadow, a hairline between rows.
fn card(rows: impl IntoIterator<Item = AnyElement>, cx: &App) -> AnyElement {
    let line = look::hairline(cx);
    v_flex()
        .w_full()
        .rounded(px(14.))
        .bg(look::card_fill(cx))
        .border_1()
        .border_color(line)
        .shadow(look::card_shadow(cx))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(move |(i, r)| {
            div()
                .when(i > 0, |el| el.border_t_1().border_color(line))
                .child(r)
        }))
        .into_any_element()
}

/// A row on a card: a leading mark, title over detail, and what's on the right.
fn row(
    lead: Option<AnyElement>,
    title: impl IntoElement,
    detail: impl Into<SharedString>,
    trail: impl IntoElement,
    cx: &App,
) -> Div {
    let detail: SharedString = detail.into();
    h_flex()
        .w_full()
        .min_h(px(56.))
        .px(px(16.))
        .py(px(11.))
        .gap(px(12.))
        .children(lead.map(|lead| {
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .justify_center()
                .child(lead)
        }))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(div().text_size(px(13.5)).font_medium().child(title))
                .when(!detail.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(12.5))
                            .line_height(relative(1.45))
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                }),
        )
        .child(div().flex_none().child(trail))
}

/// A muted heading over a card.
fn section(text: &str, cx: &App) -> AnyElement {
    div()
        .pt(px(26.))
        .pb(px(10.))
        .text_size(px(13.))
        .font_semibold()
        .text_color(cx.theme().foreground)
        .child(text.to_string())
        .into_any_element()
}

fn note(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .pt(px(12.))
        .text_size(px(12.5))
        .line_height(relative(1.5))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// "Allowed ✓", "Connected ✓": a state in its colour.
fn state_label(id: impl Into<ElementId>, text: &str, color: Hsla, check: bool) -> AnyElement {
    h_flex()
        .id(id)
        .test_support()
        .gap(px(5.))
        .text_size(px(12.5))
        .text_color(color)
        .when(check, |el| {
            el.child(Icon::new(IconName::Check).size(px(13.)).text_color(color))
        })
        .child(text.to_string())
        .into_any_element()
}

/// A small pill (an agent's plan, a model).
fn pill(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .bg(look::chip_fill(cx))
        .text_size(px(11.5))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// A command to paste into a terminal, with a button that copies it.
fn command_chip(id: &str, command: &str, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let copied = command.to_string();
    h_flex()
        .mt(px(4.))
        .min_w_0()
        .h(px(26.))
        .pl(px(8.))
        .pr(px(2.))
        .gap(px(4.))
        .rounded(px(6.))
        .bg(theme.foreground.opacity(0.05))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_family(theme.mono_font_family.clone())
                .text_size(px(11.5))
                .child(command.to_string()),
        )
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
        )
        .into_any_element()
}

/// Run `command` in the terminal under onboarding (installs and sign-ins); agents are looked for
/// again when it exits.
fn run_in_terminal(ws: &Entity<Workspace>, command: &str, cx: &mut App) {
    let command = command.to_string();
    ws.update(cx, |_, cx| {
        cx.emit(crate::workspace::WorkspaceEvent::RunInTerminal { command, cwd: None })
    });
}

/// `catalog::agent_setup` is keyed by the ACP id alone.
fn setup_for(agent: &AgentId) -> Option<trek_core::catalog::AgentSetup> {
    match agent {
        AgentId::Acp(id) => trek_core::catalog::agent_setup(id),
        other => trek_core::catalog::agent_setup(&other.key()),
    }
}

/// How far `t` (0 to 1) is through the part of the intro from `from` to `to`.
fn span(t: f32, from: f32, to: f32) -> f32 {
    ((t - from) / (to - from)).clamp(0., 1.)
}

impl Onboarding {
    /// The intro: the hiker walks the trail up to the cairn as it stacks itself, then the
    /// welcome fades up. With reduce motion, all of it is simply there.
    pub(super) fn welcome(
        &mut self,
        reduce: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let t = if reduce {
            1.
        } else {
            (self.intro_at.elapsed().as_secs_f32() / INTRO.as_secs_f32()).min(1.)
        };
        if t < 1. {
            window.request_animation_frame();
        }
        let walk = span(t, 0., 0.7);
        // Half a lap: from the trail's start to its end, slowing to a stop there.
        let hiker =
            crate::mascot::trail(0.5 * walk, self.intro_at.elapsed().as_secs_f32(), false, cx);
        let words = span(t, 0.45, 0.8);
        let rise = |el: Div, k: f32| el.opacity(k).mt(px(8. * (1. - k)));
        let point = |icon: Icon, title: &str, body: &str| {
            v_flex()
                .w(px((COLUMN - 20.) / 3.))
                .flex_none()
                .gap(px(6.))
                .p(px(14.))
                .rounded(px(12.))
                .bg(look::chip_fill(cx))
                .child(icon.size(px(16.)).text_color(theme.muted_foreground))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_medium()
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(relative(1.45))
                        .text_color(theme.muted_foreground)
                        .child(body.to_string()),
                )
        };
        let this = crate::words::words().this_computer;
        v_flex()
            .id("ob-welcome")
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                h_flex()
                    .items_end()
                    .gap(px(6.))
                    .child(div().w(px(220.)).child(hiker))
                    .child(crate::brand::cairn_draw(
                        SharedString::from(format!("ob-cairn-{}", self.run)),
                        px(72.),
                        reduce,
                    )),
            )
            .child(
                rise(v_flex(), words)
                    .pt(px(28.))
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(30.))
                            .font_semibold()
                            .child("Welcome to Trek"),
                    )
                    .child(
                        div()
                            .max_w(px(460.))
                            .text_center()
                            .text_size(px(14.5))
                            .line_height(relative(1.55))
                            .text_color(theme.muted_foreground)
                            .child("One calm place for every coding agent you use. A couple of minutes to set up, and every step can wait."),
                    ),
            )
            .child(
                rise(h_flex(), span(t, 0.6, 0.95))
                    .items_stretch()
                    .w(px(COLUMN))
                    .pt(px(30.))
                    .gap(px(10.))
                    .child(point(Icon::new(IconName::Bot), "Your agents, your plans", "Claude Code, Codex and more, with the logins you already pay for."))
                    .child(point(Icon::new(crate::assets::Lucide::Laptop), &format!("Kept on {this}"), "Threads, keys and settings stay here. Nothing goes through Trek's servers."))
                    .child(point(Icon::new(crate::assets::Lucide::ShieldCheck), "You decide", "How much an agent may do on its own, per thread.")),
            )
            .child(
                rise(v_flex(), span(t, 0.7, 1.))
                    .pt(px(32.))
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Button::new("ob-start")
                            .primary()
                            .large()
                            .label("Get started")
                            .on_click(cx.listener(|this, _, _, cx| this.advance(cx))),
                    )
                    .child(
                        Button::new("ob-skip-all")
                            .ghost()
                            .small()
                            .label("Skip setup")
                            .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn appearance_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.workspace.read(cx).settings.clone();
        let theme = cx.theme().clone();
        let glass = s.appearance.glass;
        let text_size = match s.appearance.transcript_font_size() {
            v if v < 14.0 => 0u8,
            v if v < 15.25 => 1,
            v if v < 16.75 => 2,
            _ => 3,
        };
        let set_theme = self.setter(|s, v: ThemeChoice| s.appearance.theme = v);
        let switch = |id: &'static str, on: bool, f: fn(&mut Settings, bool)| {
            let set = self.setter(f);
            Switch::new(id)
                .checked(on)
                .on_click(move |v: &bool, window, cx| set(*v, window, cx))
        };
        let frost = h_flex()
            .id("ob-frost")
            .test_support()
            .w(px(240.))
            .gap(px(10.))
            .when(!glass, |el| el.opacity(0.45))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Clear"),
            )
            .child(div().flex_1().child(
                gpui_kit::component::slider::Slider::new(&self.glass_slider).disabled(!glass),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Frosted"),
            );
        let sample = div()
            .mt(px(10.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(look::chip_fill(cx))
            .text_size(px(s.appearance.transcript_font_size()))
            .line_height(relative(1.5))
            .child("Conversations read like this. Agents answer in the same size.");
        v_flex()
            .child(header(
                "Make it yours",
                "Pick a theme and how the window looks. It changes as you choose, and it's all in Settings › Appearance later.",
                cx,
            ))
            .child(crate::settings_view::theme_tiles(s.appearance.theme, set_theme, cx))
            .child(section("Window", cx))
            .child(card(
                [
                    row(None, "Liquid glass", crate::words::words().glass_note, switch("ob-glass", glass, |s, v| s.appearance.glass = v), cx).into_any_element(),
                    row(None, "Frost", "Clear lets more of your desktop through; frosted keeps more of the theme's colour.", frost, cx).into_any_element(),
                    row(
                        None,
                        "Reduce motion",
                        "Fades instead of slides, and the working animations hold still. Trek also follows the system's setting.",
                        switch("ob-reduce-motion", s.appearance.reduce_motion, |s, v| s.appearance.reduce_motion = v),
                        cx,
                    )
                    .into_any_element(),
                ],
                cx,
            ))
            .child(section("Text size", cx))
            .child(card(
                [v_flex()
                    .px(px(16.))
                    .py(px(14.))
                    .child(
                        h_flex()
                            .child(div().flex_1().text_size(px(13.5)).font_medium().child("Conversation text"))
                            .child(ui::segmented(
                                "ob-text-size",
                                vec![(0u8, "Small"), (1, "Default"), (2, "Large"), (3, "Larger")],
                                text_size,
                                self.setter(|s, v: u8| {
                                    s.appearance.transcript_font_size = [13.5, 14.5, 16.0, 17.5][v as usize]
                                }),
                                cx,
                            )),
                    )
                    .child(sample)
                    .into_any_element()],
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn permissions_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let required: Vec<Permission> = Permission::ALL
            .into_iter()
            .filter(|p| !p.optional())
            .collect();
        let granted = required
            .iter()
            .filter(|p| self.perms.get(p) == Some(&Status::Granted))
            .count();
        let icon = |p: Permission| -> Icon {
            match p {
                Permission::Accessibility => Icon::new(crate::assets::Lucide::Hand),
                Permission::ScreenRecording => Icon::new(crate::assets::Lucide::Monitor),
                Permission::Microphone => Icon::new(crate::assets::Lucide::Mic),
                Permission::SpeechRecognition => {
                    Icon::new(crate::assets::Lucide::MessageSquareQuote)
                }
                Permission::Notifications => Icon::new(IconName::Bell),
                Permission::FullDiskAccess => Icon::new(IconName::Folder),
                Permission::LocalNetwork => Icon::new(crate::assets::Lucide::Wifi),
            }
        };
        let rows = Permission::ALL.into_iter().map(|p| {
            let status = self.perms.get(&p).copied().unwrap_or(Status::NotYet);
            let id = p.key();
            let trail: AnyElement = match status {
                Status::Granted => state_label(
                    SharedString::from(format!("ob-perm-{id}-on")),
                    "Allowed",
                    palette::emerald(cx),
                    true,
                ),
                Status::NeedsApp => state_label(
                    SharedString::from(format!("ob-perm-{id}-app")),
                    "In the installed app",
                    theme.muted_foreground,
                    false,
                ),
                _ => {
                    let prompt =
                        p.grant(status, self.asked.contains(&p)) == permissions::Grant::Prompt;
                    let label = if prompt {
                        "Allow…"
                    } else if status == Status::Unknown {
                        "Settings…"
                    } else {
                        "Open Settings…"
                    };
                    let button = Button::new(SharedString::from(format!("ob-perm-{id}")))
                        .small()
                        .label(label)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.grant(p, window, cx)),
                        );
                    if status == Status::Unknown {
                        button.ghost()
                    } else {
                        button.outline()
                    }
                    .into_any_element()
                }
            };
            let detail = match status {
                Status::Denied => {
                    format!("{} Turned off: turn Trek on in System Settings.", p.why())
                }
                _ => p.why().to_string(),
            };
            let drop_hint = (p.takes_a_drop()
                && !matches!(status, Status::Granted | Status::NeedsApp))
            .then(|| {
                h_flex()
                    .pt(px(6.))
                    .gap(px(8.))
                    .child(drag_tile(
                        SharedString::from(format!("ob-drag-{id}")),
                        26.,
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("Not in the list? Drag Trek into it, then turn it on."),
                    )
            });
            h_flex()
                .id(SharedString::from(format!("ob-perm-row-{id}")))
                .test_support()
                .w_full()
                .min_h(px(56.))
                .px(px(16.))
                .py(px(11.))
                .gap(px(12.))
                .items_start()
                .child(
                    div()
                        .pt(px(2.))
                        .w(px(24.))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(icon(p).size(px(16.)).text_color(theme.muted_foreground)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(div().text_size(px(13.5)).font_medium().child(p.title()))
                                .when(p.optional(), |el| el.child(pill("Optional", cx))),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .line_height(relative(1.45))
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        )
                        .children(drop_hint),
                )
                .child(div().flex_none().pt(px(2.)).child(trail))
                .into_any_element()
        });
        v_flex()
            .child(header(
                "Allow what Trek uses",
                format!(
                    "macOS asks for each one once. All of them can wait: Trek works without them, minus the feature each is for. {granted} of {} allowed.",
                    required.len()
                ),
                cx,
            ))
            .child(card(rows.collect::<Vec<_>>(), cx))
            .child(note(
                "Allow… shows macOS's own prompt. Open Settings… goes straight to the right list in System Settings, with Trek's icon beside it to drag in. This page updates the moment a permission changes. Screen Recording can ask you to reopen Trek once it's on.",
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn agents_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let detecting = ws.detecting;
        let this = crate::words::words().this_computer;
        let scan = scan_agents(&ws.agents, &ws.agent_status, &ws.acp_info, detecting);
        let authenticating = ws.authenticating_agents.clone();
        let (found, missing): (Vec<_>, Vec<_>) = scan
            .into_iter()
            .partition(|a| a.state != AgentState::NotInstalled);
        let connected = found
            .iter()
            .filter(|a| a.state == AgentState::Connected)
            .count();
        let body = if detecting {
            format!("Looking on {this} for the agents you already use…")
        } else if found.is_empty() {
            format!(
                "None of the agents Trek runs is on {this} yet. Install one below, sign in to it, and it's ready: Trek uses each vendor's own CLI with your login."
            )
        } else {
            format!(
                "{connected} of {} found on {this} {} ready. Trek runs each vendor's own CLI with the login you have, so subscriptions just work; your credentials never pass through Trek.",
                found.len(),
                if connected == 1 { "is" } else { "are" }
            )
        };
        let ws_entity = self.workspace.clone();
        let found_rows: Vec<AnyElement> = found
            .into_iter()
            .map(|a| {
                let key = a.agent.key();
                let id = key.replace(':', "-");
                let title = h_flex()
                    .gap(px(8.))
                    .child(a.name.clone())
                    .children(a.login.badge().map(|b| pill(b, cx)));
                let trail: AnyElement = match a.state {
                    AgentState::Connected => state_label(
                        SharedString::from(format!("ob-agent-{id}-ok")),
                        "Connected",
                        palette::emerald(cx),
                        true,
                    ),
                    _ => {
                        let signing = authenticating.contains(&key);
                        let agent = a.agent.clone();
                        let ws = ws_entity.clone();
                        Button::new(SharedString::from(format!("ob-login-{id}")))
                            .small()
                            .outline()
                            .loading(signing)
                            .label(if signing { "Signing in…" } else { "Sign in" })
                            .on_click(move |_, _, cx| match &agent {
                                AgentId::Acp(_) => {
                                    let k = agent.key();
                                    ws.update(cx, |ws, cx| ws.authenticate_acp_agent(&k, None, cx));
                                }
                                other => {
                                    if let Some(s) = setup_for(other) {
                                        run_in_terminal(&ws, s.login, cx);
                                    }
                                }
                            })
                            .into_any_element()
                    }
                };
                let detail = if a.state == AgentState::NotSignedIn && a.detail.is_empty() {
                    "Not signed in".to_string()
                } else if a.state == AgentState::NotSignedIn {
                    format!("Not signed in · {}", a.detail)
                } else {
                    a.detail.clone()
                };
                let login_cmd = (a.state == AgentState::NotSignedIn
                    && !matches!(a.agent, AgentId::Acp(_)))
                .then(|| setup_for(&a.agent))
                .flatten()
                .map(|s| command_chip(&id, s.login, cx));
                h_flex()
                    .id(SharedString::from(format!("ob-agent-{id}")))
                    .test_support()
                    .w_full()
                    .min_h(px(56.))
                    .px(px(16.))
                    .py(px(11.))
                    .gap(px(12.))
                    .when(login_cmd.is_some(), |el| el.items_start())
                    .child(
                        div()
                            .w(px(24.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(ui::agent_logo(&a.agent, px(20.), cx)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(13.5)).font_medium().child(title))
                            .when(!detail.is_empty(), |el| {
                                el.child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .text_color(if a.state == AgentState::NotSignedIn {
                                            palette::amber(cx)
                                        } else {
                                            theme.muted_foreground
                                        })
                                        .child(detail),
                                )
                            })
                            .children(login_cmd),
                    )
                    .child(div().flex_none().child(trail))
                    .into_any_element()
            })
            .collect();
        let more = missing.len().saturating_sub(MISSING_SHOWN);
        let shown = if self.all_missing {
            missing.len()
        } else {
            missing.len().min(MISSING_SHOWN)
        };
        let missing_rows: Vec<AnyElement> = missing
            .iter()
            .take(shown)
            .map(|a| {
                let id = a.agent.key().replace(':', "-");
                let install = setup_for(&a.agent).map(|s| s.install);
                let ws = ws_entity.clone();
                let cmd = install.map(str::to_string);
                h_flex()
                    .id(SharedString::from(format!("ob-agent-{id}")))
                    .test_support()
                    .w_full()
                    .px(px(16.))
                    .py(px(11.))
                    .gap(px(12.))
                    .items_start()
                    .child(
                        div()
                            .pt(px(2.))
                            .w(px(24.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(ui::agent_logo(&a.agent, px(20.), cx)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_medium()
                                    .child(a.name.clone()),
                            )
                            .children(install.map(|i| command_chip(&id, i, cx))),
                    )
                    .children(cmd.map(|cmd| {
                        Button::new(SharedString::from(format!("ob-install-{id}")))
                            .small()
                            .outline()
                            .icon(IconName::ArrowDown)
                            .label("Install")
                            .on_click(move |_, _, cx| run_in_terminal(&ws, &cmd, cx))
                    }))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .child(header("Your agents and subscriptions", body, cx))
            .when(detecting, |el| {
                el.child(
                    h_flex()
                        .gap(px(10.))
                        .pb(px(14.))
                        .text_size(px(13.))
                        .text_color(theme.muted_foreground)
                        .child(Spinner::new().small())
                        .child("Scanning for agent CLIs and their logins…"),
                )
            })
            .when(!found_rows.is_empty(), |el| el.child(card(found_rows, cx)))
            .when(!missing_rows.is_empty(), |el| {
                el.child(section("Not installed", cx))
                    .child(card(missing_rows, cx))
            })
            .child(
                h_flex()
                    .pt(px(14.))
                    .gap(px(8.))
                    .when(more > 0 && !self.all_missing, |el| {
                        el.child(
                            Button::new("ob-agents-more")
                                .small()
                                .ghost()
                                .label(format!("Show {more} more"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.all_missing = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("ob-rescan")
                            .small()
                            .outline()
                            .loading(detecting)
                            .icon(IconName::RefreshCw)
                            .label("Scan again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.update(cx, |ws, cx| ws.detect_agents(cx))
                            })),
                    )
                    // An agent Trek doesn't list: from the ACP Registry, or its own command.
                    .child(
                        Button::new("ob-add-agent")
                            .small()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("Add another agent…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                crate::add_agent::open(
                                    this.workspace.clone(),
                                    crate::add_agent::Tab::Registry,
                                    window,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(note(
                "Install and Sign in run in the terminal below; Trek looks again when they finish.",
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn providers_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let settings = ws.settings.clone();
        let store = crate::words::words().your_credential_store;
        let providers: Vec<_> = DIRECT_PROVIDERS
            .iter()
            .filter(|p| !p.local)
            .filter(|p| self.all_providers || FEATURED_PROVIDERS.contains(&p.id))
            .collect();
        let hidden = DIRECT_PROVIDERS.iter().filter(|p| !p.local).count() - providers.len();
        let rows: Vec<AnyElement> = providers
            .into_iter()
            .map(|p| {
                let id = p.id;
                let state = key_state(id, &settings, |k| {
                    std::env::var(k).is_ok_and(|v| !v.is_empty())
                });
                let status: SharedString = match state {
                    KeyState::Saved => format!("Saved in {store}").into(),
                    KeyState::FromEnv(k) => format!("Using ${k} from your shell").into(),
                    KeyState::NotSet => "Not set".into(),
                };
                let title = h_flex().gap(px(8.)).child(p.name).children(
                    key_page(id)
                        .filter(|_| state != KeyState::Saved)
                        .map(|url| {
                            div()
                                .id(SharedString::from(format!("ob-key-page-{id}")))
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child("Get a key ↗")
                                .on_click(move |_, _, cx| cx.open_url(url))
                        }),
                );
                let trail: AnyElement = if state == KeyState::Saved {
                    h_flex()
                        .gap(px(6.))
                        .child(state_label(
                            SharedString::from(format!("ob-key-{id}-saved")),
                            "Saved",
                            palette::emerald(cx),
                            true,
                        ))
                        .child(
                            Button::new(SharedString::from(format!("ob-key-del-{id}")))
                                .small()
                                .ghost()
                                .label("Remove")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Err(e) = this
                                        .workspace
                                        .update(cx, |ws, cx| ws.remove_api_key(id, cx))
                                    {
                                        crate::toast::push(
                                            window,
                                            format!("Couldn't remove the key: {e}"),
                                            cx,
                                        );
                                    }
                                })),
                        )
                        .into_any_element()
                } else {
                    let input = self.key_inputs.get(id).cloned();
                    h_flex()
                        .gap(px(6.))
                        .w(px(260.))
                        .children(input.map(|i| div().flex_1().child(Input::new(&i).small())))
                        .child(
                            Button::new(SharedString::from(format!("ob-key-save-{id}")))
                                .small()
                                .outline()
                                .label("Save")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.save_key(id, window, cx)
                                })),
                        )
                        .into_any_element()
                };
                row(
                    Some(ui::agent_logo(
                        &AgentId::Direct(id.to_string()),
                        px(20.),
                        cx,
                    )),
                    title,
                    status,
                    trail,
                    cx,
                )
                .id(SharedString::from(format!("ob-key-{id}")))
                .test_support()
                .into_any_element()
            })
            .collect();
        v_flex()
            .child(header(
                "Bring your own API keys",
                format!("Optional. Use models straight from a provider, billed to your key. Keys are kept in {store}, never in Trek's files. A subscription through an agent above needs none."),
                cx,
            ))
            .child(card(rows, cx))
            .when(hidden > 0, |el| {
                el.child(
                    h_flex().pt(px(12.)).child(
                        Button::new("ob-keys-more")
                            .small()
                            .ghost()
                            .label(format!("{hidden} more providers"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.all_providers = true;
                                cx.notify();
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    /// Save the key typed for `id` (in the platform's secret store, as Settings › API Keys does).
    fn save_key(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.key_inputs.get(id).cloned() else {
            return;
        };
        let key = input.read(cx).value().trim().to_string();
        if key.is_empty() {
            return;
        }
        match self
            .workspace
            .update(cx, |ws, cx| ws.save_api_key(id, &key, cx))
        {
            Ok(()) => {
                input.update(cx, |s, cx| s.set_value("", window, cx));
                crate::toast::push(
                    window,
                    format!("Saved to {}", crate::words::words().your_credential_store),
                    cx,
                );
            }
            Err(e) => crate::toast::push(window, format!("Couldn't save the key: {e}"), cx),
        }
        cx.notify();
    }

    pub(super) fn local_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let detecting = ws.detecting;
        let this = crate::words::words().this_computer;
        let clis = self.local_clis.clone().unwrap_or_default();
        let servers = scan_local(&ws.agents, &clis, &ws.settings.disabled_agents);
        let models: usize = servers
            .iter()
            .filter(|s| s.running)
            .map(|s| s.models.len())
            .sum();
        let ws_entity = self.workspace.clone();
        let rows: Vec<AnyElement> = servers
            .into_iter()
            .map(|s| {
                let id = s.id;
                let agent = AgentId::Direct(id.into());
                let key = agent.key();
                let (detail, trail): (String, AnyElement) = if s.running {
                    let detail = match s.models.len() {
                        0 => "Running, with no models yet. Pull one, e.g. ollama pull qwen3-coder"
                            .to_string(),
                        1 => "Running · 1 model".to_string(),
                        n => format!("Running · {n} models"),
                    };
                    let ws = ws_entity.clone();
                    let toggle = Switch::new(SharedString::from(format!("ob-local-{id}-on")))
                        .checked(s.enabled)
                        .on_click(move |v: &bool, _, cx| {
                            let (k, v) = (key.clone(), *v);
                            ws.update(cx, |ws, cx| {
                                ws.settings.disabled_agents.retain(|d| *d != k);
                                if !v {
                                    ws.settings.disabled_agents.push(k);
                                }
                                ws.save_settings(cx);
                            })
                        });
                    (
                        detail,
                        h_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(theme.muted_foreground)
                                    .child("Use in Trek"),
                            )
                            .child(toggle)
                            .into_any_element(),
                    )
                } else if s.installed {
                    let start = s.start_command().map(|cmd| {
                        let ws = ws_entity.clone();
                        Button::new(SharedString::from(format!("ob-local-{id}-start")))
                            .small()
                            .outline()
                            .icon(Icon::new(crate::assets::Lucide::Play))
                            .label("Start")
                            .on_click(move |_, _, cx| run_in_terminal(&ws, cmd, cx))
                            .into_any_element()
                    });
                    (
                        "Installed, not running. Start it and Trek finds it on its usual port."
                            .to_string(),
                        start.unwrap_or_else(|| div().into_any_element()),
                    )
                } else {
                    let url = s.download();
                    (
                        "Not installed.".to_string(),
                        Button::new(SharedString::from(format!("ob-local-{id}-get")))
                            .small()
                            .ghost()
                            .icon(IconName::ExternalLink)
                            .label(format!(
                                "Get {}",
                                s.name.split(" /").next().unwrap_or(s.name)
                            ))
                            .on_click(move |_, _, cx| cx.open_url(url))
                            .into_any_element(),
                    )
                };
                let chips = (s.running && !s.models.is_empty()).then(|| {
                    let extra = s.models.len().saturating_sub(6);
                    h_flex()
                        .pt(px(6.))
                        .flex_wrap()
                        .gap(px(4.))
                        .children(s.models.iter().take(6).map(|m| pill(m.clone(), cx)))
                        .when(extra > 0, |el| el.child(pill(format!("+{extra}"), cx)))
                });
                h_flex()
                    .id(SharedString::from(format!("ob-local-{id}")))
                    .test_support()
                    .w_full()
                    .px(px(16.))
                    .py(px(11.))
                    .gap(px(12.))
                    .items_start()
                    .child(
                        div()
                            .pt(px(2.))
                            .w(px(24.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(ui::agent_logo(&agent, px(20.), cx)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .child(div().text_size(px(13.5)).font_medium().child(s.name))
                                    .when(s.running, |el| {
                                        el.child(
                                            div()
                                                .size(px(7.))
                                                .rounded_full()
                                                .bg(palette::emerald(cx)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            )
                            .children(chips),
                    )
                    .child(div().flex_none().child(trail))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .child(header(
                "Models on this machine",
                if models > 0 {
                    format!("Free, private, and offline. {models} models found on {this}: pick them in the composer like any other.")
                } else {
                    format!("Free, private, and offline. Trek finds Ollama, LM Studio, and llama.cpp or MLX on {this} on their usual ports; there's nothing to set up.")
                },
                cx,
            ))
            .child(card(rows, cx))
            .child(
                h_flex().pt(px(14.)).child(
                    Button::new("ob-local-rescan")
                        .small()
                        .outline()
                        .loading(detecting)
                        .icon(IconName::RefreshCw)
                        .label("Scan again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.local_clis = None;
                            this.find_local_clis(cx);
                            this.workspace.update(cx, |ws, cx| ws.detect_agents(cx))
                        })),
                ),
            )
            .into_any_element()
    }

    pub(super) fn approvals_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.workspace.read(cx).draft_prefs.hand_holding;
        let ember = palette::ember(cx);
        let rows: Vec<AnyElement> = HandHolding::ALL
            .into_iter()
            .map(|h| {
                let locked = h == HandHolding::FullAccess;
                let selected = h == current;
                let radio = div()
                    .size(px(16.))
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        ember
                    } else {
                        theme.foreground.opacity(0.25)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |el| {
                        el.child(div().size(px(8.)).rounded_full().bg(ember))
                    });
                let detail = if locked {
                    "No prompts and no sandbox. Unlock it later in Settings › Permissions."
                } else {
                    h.description()
                };
                row(Some(radio.into_any_element()), h.label(), detail, div(), cx)
                    .id(SharedString::from(format!("ob-hh-{h:?}")))
                    .test_support()
                    .when(locked, |el| el.opacity(0.5))
                    .when(!locked, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(theme.foreground.opacity(0.03)))
                            .on_click(cx.listener(move |this, _, _, cx| {
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
            .child(header(
                "How much should agents ask first?",
                "The default for new threads. Change it for any thread from the composer.",
                cx,
            ))
            .child(card(rows, cx))
            .into_any_element()
    }

    pub(super) fn phone_step(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let this = crate::words::words().this_computer;
        let enabled = ws.settings.mobile.enabled;
        let running = ws
            .remote
            .as_ref()
            .map(|r| (r.offer.clone(), r.devices.len()));
        let machines = ws.machines.list.len();
        let ws_entity = self.workspace.clone();
        let phone_on = {
            let set = self.setter(|s, v: bool| s.mobile.enabled = v);
            Switch::new("ob-phone-on")
                .checked(enabled)
                .on_click(move |v: &bool, window, cx| set(*v, window, cx))
        };
        let pairing: Option<AnyElement> = match (enabled, running) {
            (false, _) => None,
            (true, None) => Some(note("Starting the phone server…", cx)),
            (true, Some((Some(offer), _))) => {
                let ws = ws_entity.clone();
                Some(
                    h_flex()
                        .id("ob-pairing")
                        .test_support()
                        .px(px(16.))
                        .py(px(14.))
                        .gap(px(20.))
                        .items_start()
                        .child(crate::settings_view::qr(&offer.url, 3.))
                        .child(
                            v_flex()
                                .gap(px(8.))
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(13.5)).font_medium().child("Scan this with Trek on your iPhone"))
                                .child(div().text_size(px(12.5)).text_color(theme.muted_foreground).child("No camera? Choose “Enter address and code” on the phone and type:"))
                                .child(div().text_size(px(20.)).font_semibold().font_family(theme.mono_font_family.clone()).child(offer.code.clone()))
                                .child(
                                    Button::new("ob-pair-cancel")
                                        .small()
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.cancel_pairing(cx))),
                                ),
                        )
                        .into_any_element(),
                )
            }
            (true, Some((None, devices))) => {
                let ws = ws_entity.clone();
                Some(
                    row(
                        None,
                        if devices > 0 {
                            format!("{devices} paired")
                        } else {
                            "Pair your iPhone".to_string()
                        },
                        "A code to scan with Trek on iPhone; it works once, for a few minutes.",
                        Button::new("ob-pair")
                            .small()
                            .outline()
                            .icon(Icon::new(crate::assets::Lucide::Smartphone))
                            .label("Show pairing code")
                            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.offer_pairing(cx))),
                        cx,
                    )
                    .into_any_element(),
                )
            }
        };
        let mut rows = vec![
            row(
                Some(Icon::new(crate::assets::Lucide::Smartphone).size(px(16.)).text_color(theme.muted_foreground).into_any_element()),
                "Let your iPhone connect",
                "Follow every thread, answer approvals and questions, and start work from your phone. Off, nothing listens.",
                phone_on,
                cx,
            )
            .into_any_element(),
        ];
        rows.extend(pairing);
        let ws = ws_entity.clone();
        rows.push(
            row(
                Some(Icon::new(crate::assets::Lucide::Laptop).size(px(16.)).text_color(theme.muted_foreground).into_any_element()),
                if machines > 0 { format!("Other machines · {machines} paired") } else { "Another machine".to_string() },
                format!("Start threads on another computer running Trek from {this}: they run there and show here."),
                Button::new("ob-add-machine")
                    .small()
                    .outline()
                    .label("Add a machine…")
                    .on_click(move |_, window, cx| crate::machines::sheets::open_pair(ws.clone(), window, cx)),
                cx,
            )
            .into_any_element(),
        );
        v_flex()
            .child(header(
                "Your phone and other machines",
                format!("Optional. The agents keep running on {this}; these just reach them from elsewhere. Connections are encrypted, and each device checks it's talking to {this}."),
                cx,
            ))
            .child(card(rows, cx))
            .child(note("Trek on iPhone isn't on the App Store yet: build it from ios/ in Trek's repository. All of this is in Settings › Phone & Machines.", cx))
            .into_any_element()
    }

    pub(super) fn finish_step(&self, reduce: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let s = &ws.settings;
        let theme_name = match s.appearance.theme {
            ThemeChoice::System => "Match the system",
            ThemeChoice::Night => "Night",
            ThemeChoice::Paper => "Paper",
        };
        let appearance = format!(
            "{theme_name}{}",
            if s.appearance.glass {
                " · Liquid glass"
            } else {
                ""
            }
        );
        let scan = scan_agents(&ws.agents, &ws.agent_status, &ws.acp_info, ws.detecting);
        let ready: Vec<String> = scan
            .iter()
            .filter(|a| a.state == AgentState::Connected)
            .map(|a| a.name.clone())
            .collect();
        let agents = if ready.is_empty() {
            "None yet".to_string()
        } else {
            ready.join(", ")
        };
        let keys: Vec<&str> = DIRECT_PROVIDERS
            .iter()
            .filter(|p| !p.local && s.api_providers.iter().any(|k| k == p.id))
            .map(|p| p.name)
            .collect();
        let keys = if keys.is_empty() {
            "None".to_string()
        } else {
            keys.join(", ")
        };
        let local = scan_local(&ws.agents, &HashSet::new(), &s.disabled_agents);
        let local: Vec<String> = local
            .iter()
            .filter(|l| l.running && l.enabled)
            .map(|l| format!("{} · {} models", l.name, l.models.len()))
            .collect();
        let local = if local.is_empty() {
            "None running".to_string()
        } else {
            local.join(", ")
        };
        let phone = match (&ws.remote, s.mobile.enabled) {
            (Some(r), _) if !r.devices.is_empty() => format!("{} paired", r.devices.len()),
            (_, true) => "On".to_string(),
            _ => "Off".to_string(),
        };
        let imported = ws
            .import_summary
            .as_ref()
            .map(|i| i.claude_code + i.codex + i.opencode)
            .filter(|n| *n > 0)
            .map(|n| format!("{n} threads from your other agents are in the sidebar."));
        let mut summary: Vec<(Step, &str, String)> =
            vec![(Step::Appearance, "Appearance", appearance)];
        if self.here.contains(&Step::Permissions) {
            let required: Vec<Permission> = Permission::ALL
                .into_iter()
                .filter(|p| !p.optional())
                .collect();
            let on = required
                .iter()
                .filter(|p| permissions::status(**p) == Status::Granted)
                .count();
            summary.push((
                Step::Permissions,
                "Permissions",
                format!("{on} of {} allowed", required.len()),
            ));
        }
        summary.extend([
            (Step::Agents, "Agents", agents),
            (Step::Providers, "API keys", keys),
            (Step::LocalModels, "Local models", local),
            (
                Step::Approvals,
                "Approvals",
                ws.draft_prefs.hand_holding.label().to_string(),
            ),
            (Step::Phone, "Phone", phone),
        ]);
        let summary_rows: Vec<AnyElement> = summary
            .into_iter()
            .map(|(step, k, v)| {
                h_flex()
                    .id(SharedString::from(format!("ob-summary-{}", step.key())))
                    .test_support()
                    .px(px(16.))
                    .h(px(46.))
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(130.))
                            .flex_none()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(k),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.5))
                            .child(v),
                    )
                    .child(
                        Button::new(SharedString::from(format!("ob-change-{}", step.key())))
                            .xsmall()
                            .ghost()
                            .label("Change")
                            .on_click(cx.listener(move |this, _, _, cx| this.go(step, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let selected = self.project.clone();
        let projects: Vec<_> = ws
            .workspace_projects()
            .into_iter()
            .take(5)
            .cloned()
            .collect();
        let project_rows: Vec<AnyElement> = projects
            .into_iter()
            .map(|p| {
                let path = p.path.clone();
                let is_sel = selected.as_ref() == Some(&p.path);
                let detail = p
                    .remote
                    .clone()
                    .unwrap_or_else(|| trek_core::paths::tildify(&p.path));
                row(
                    Some(ui::project_badge(&p.name, &ws.project_look(&p.path), cx)),
                    p.name.clone(),
                    detail,
                    if is_sel {
                        Icon::new(IconName::Check)
                            .size(px(15.))
                            .text_color(palette::ember(cx))
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    },
                    cx,
                )
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
        v_flex()
            .child(div().pb(px(20.)).child(crate::brand::cairn_draw(
                SharedString::from(format!("ob-ready-cairn-{}", self.run)),
                px(56.),
                reduce,
            )))
            .child(header(
                "You're set",
                imported.unwrap_or_else(|| {
                    "Here's what you chose. Everything can be changed in Settings.".to_string()
                }),
                cx,
            ))
            .child(card(summary_rows, cx))
            .child(section("Start your first thread in", cx))
            .when(!project_rows.is_empty(), |el| {
                el.child(card(project_rows, cx))
            })
            .child(
                h_flex().pt(px(12.)).child(
                    Button::new("ob-open-folder")
                        .small()
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label("Open folder…")
                        .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                ),
            )
            .into_any_element()
    }

    /// Add a folder as a project without leaving onboarding (`Workspace::open_folder` navigates).
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Project".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
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
}
