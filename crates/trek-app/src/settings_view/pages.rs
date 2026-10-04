//! The hand-built settings pages: General, Appearance, Notifications, Shortcuts, API keys,
//! Permissions, Updates and About. The rest live in settings_view.rs.

use super::SettingsView;
use crate::palette;
use crate::ui;
use crate::workspace::{Route, UpdateAction};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::catalog::DIRECT_PROVIDERS;
use trek_core::settings::{Channel, FollowUp, NotifyMode, OnUsageLimit, RunIn, Settings, ThemeChoice, secrets};
use trek_core::{AgentId, Effort, HandHolding};

/// A compact dropdown button that shows the current choice.
fn picker<T: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    leading: Option<AnyElement>,
    current_label: impl Into<SharedString>,
    options: Vec<(T, String)>,
    current: Option<T>,
    on_pick: impl Fn(T, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    Button::new(id)
        .small()
        .outline()
        .child(
            h_flex()
                .gap(px(7.))
                .max_w(px(220.))
                .children(leading)
                .child(div().min_w_0().truncate().text_size(px(12.5)).child(current_label.into()))
                .child(Icon::new(IconName::ChevronDown).size(px(12.)).text_color(theme.muted_foreground)),
        )
        .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.min_w(px(200.)).max_h(px(340.)).scrollable(true);
            for (value, label) in options.clone() {
                let on_pick = on_pick.clone();
                let checked = current.as_ref() == Some(&value);
                menu = menu.item(PopupMenuItem::new(label).checked(checked).on_click(move |_, _, cx| on_pick(value.clone(), cx)));
            }
            menu
        })
        .into_any_element()
}

/// A key cap, e.g. ⌘ or N.
fn key(label: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .min_w(px(22.))
        .h(px(22.))
        .px(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(theme.foreground.opacity(0.1))
        .bg(theme.foreground.opacity(0.04))
        .text_size(px(12.))
        .text_color(theme.foreground.opacity(0.85))
        .child(label.to_string())
        .into_any_element()
}

fn effort_label(e: Effort) -> &'static str {
    e.label()
}

impl SettingsView {
    pub(super) fn general_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ws = self.workspace.read(cx);
        let agents = ws.ready_agents();
        let current = AgentId::from_key(&s.general.default_agent);
        let models = ws.models_for(&current);
        let model_id = s.general.default_model.clone().filter(|m| models.iter().any(|i| crate::composer::same_model(m, &i.id)));
        let model_label = model_id
            .as_ref()
            .and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id)))
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "Agent's default".into());
        let efforts: Vec<Effort> = {
            let info = model_id.as_ref().and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id))).or(models.first());
            match info.map(|i| i.efforts.clone()).filter(|e| !e.is_empty()) {
                Some(e) => e,
                None => vec![Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max],
            }
        };
        let w = self.workspace.clone();
        let agent_picker = picker(
            "def-agent",
            Some(ui::agent_logo(&current, px(14.), cx)),
            current.display_name(),
            agents.iter().map(|a| (a.clone(), a.display_name())).collect(),
            Some(current.clone()),
            move |a: AgentId, cx| {
                w.update(cx, |ws, cx| {
                    ws.settings.general.default_agent = a.key();
                    ws.settings.general.default_model = None;
                    if matches!(ws.route, Route::Draft { .. }) {
                        ws.draft_prefs.agent = a;
                        ws.draft_prefs.model = None;
                    }
                    ws.save_settings(cx);
                })
            },
            cx,
        );
        let w = self.workspace.clone();
        let mut model_options: Vec<(Option<String>, String)> = vec![(None, "Agent's default".into())];
        model_options.extend(models.iter().map(|m| (Some(m.id.clone()), m.name.clone())));
        let model_picker = picker(
            "def-model",
            None,
            model_label,
            model_options,
            Some(model_id.clone()),
            move |m: Option<String>, cx| {
                w.update(cx, |ws, cx| {
                    ws.settings.general.default_model = m.clone();
                    if matches!(ws.route, Route::Draft { .. }) {
                        ws.draft_prefs.model = m;
                    }
                    ws.save_settings(cx);
                })
            },
            cx,
        );
        let w = self.workspace.clone();
        let effort_picker = picker(
            "def-effort",
            None,
            effort_label(s.general.default_effort),
            efforts.iter().map(|e| (*e, effort_label(*e).to_string())).collect(),
            Some(s.general.default_effort),
            move |e: Effort, cx| {
                w.update(cx, |ws, cx| {
                    ws.settings.general.default_effort = e;
                    if matches!(ws.route, Route::Draft { .. }) {
                        ws.draft_prefs.effort = e;
                    }
                    ws.save_settings(cx);
                })
            },
            cx,
        );

        vec![
            ui::group(
                vec![
                    Self::row("Agent", "New threads start with this agent. Any installed agent is one click away in the composer.", agent_picker, cx),
                    Self::row("Model", "Leave on the agent's default to follow whatever it recommends.", model_picker, cx),
                    Self::row("Reasoning effort", "Higher effort thinks longer before answering and uses more of your plan.", effort_picker, cx),
                ],
                cx,
            ),
            Self::heading("Composer", cx),
            ui::group(
                vec![
                    Self::row(
                        "Send with",
                        if s.general.send_with_cmd_enter { "↩ adds a new line; ⌘↩ sends." } else { "↩ sends; ⇧↩ adds a new line." },
                        ui::segmented(
                            "send-key",
                            vec![(false, "↩ Return"), (true, "⌘↩ Command-Return")],
                            s.general.send_with_cmd_enter,
                            self.setter(|s, v| s.general.send_with_cmd_enter = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "Messages sent while an agent works",
                        "Steer slips your message in at the agent's next step. Queue holds it until the turn ends.",
                        ui::segmented(
                            "follow-up",
                            vec![(FollowUp::Steer, "Steer"), (FollowUp::Queue, "Queue")],
                            s.general.follow_up,
                            self.setter(|s, v| s.general.follow_up = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "When a usage limit is reached",
                        match s.general.on_usage_limit {
                            OnUsageLimit::Ask => "The thread pauses, and a bar above the prompt offers to resume it when the limit resets.",
                            OnUsageLimit::Resume => "The thread pauses, then carries on by itself a minute after the limit resets.",
                        },
                        ui::segmented(
                            "on-usage-limit",
                            vec![(OnUsageLimit::Ask, "Ask"), (OnUsageLimit::Resume, "Resume automatically at reset")],
                            s.general.on_usage_limit,
                            self.setter(|s, v| s.general.on_usage_limit = v),
                            cx,
                        ),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Inbox", cx),
            ui::group(
                vec![Self::row(
                    "Name threads automatically",
                    "After the first answer, a small fast model writes a short title through your Claude Code login. Off, the title is the start of your first message.",
                    self.switch("auto-title", s.general.auto_title, |s, v| s.general.auto_title = v),
                    cx,
                ), Self::row(
                    "Settle finished threads",
                    "Read threads leave the inbox after this long. Threads waiting on you never settle on their own.",
                    ui::segmented(
                        "auto-settle",
                        vec![(0, "Never"), (1, "1 day"), (3, "3 days"), (7, "1 week")],
                        s.inbox.auto_settle_days,
                        self.setter(|s, v| s.inbox.auto_settle_days = v),
                        cx,
                    ),
                    cx,
                ), Self::row(
                    "Settle threads when their work is merged",
                    "A thread that committed on a branch settles once that branch is merged into the default branch, here or on origin, or when you merge its worktree from the Git tool.",
                    self.switch("settle-on-merge", s.inbox.auto_settle_on_merge, |s, v| s.inbox.auto_settle_on_merge = v),
                    cx,
                )],
                cx,
            ),
            Self::heading("System", cx),
            ui::group(
                vec![Self::row(
                    "Keep the Mac awake while agents work",
                    "Stops idle sleep until every running turn has finished. The display can still sleep.",
                    self.switch("prevent-sleep", s.general.prevent_sleep_while_running, |s, v| s.general.prevent_sleep_while_running = v),
                    cx,
                )],
                cx,
            ),
        ]
    }

    /// Three small window previews instead of a word list.
    fn theme_tiles(&self, current: ThemeChoice, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mock = |bg: u32, side: u32, line: u32, accent: u32| {
            h_flex()
                .size_full()
                .bg(rgb(bg))
                .child(v_flex().w(px(30.)).h_full().bg(rgb(side)).p(px(6.)).gap(px(4.)).children((0..3).map(move |_| div().h(px(3.)).w_full().rounded_full().bg(rgb(line)))))
                .child(
                    v_flex()
                        .flex_1()
                        .p(px(8.))
                        .gap(px(5.))
                        .child(div().h(px(3.)).w(px(46.)).rounded_full().bg(rgb(line)))
                        .child(div().h(px(3.)).w(px(34.)).rounded_full().bg(rgb(line)))
                        .child(div().mt_auto().h(px(10.)).w_full().rounded(px(3.)).border_1().border_color(rgb(line)).child(div().ml_auto().mr(px(2.)).mt(px(2.)).size(px(4.)).rounded_full().bg(rgb(accent)))),
                )
                .into_any_element()
        };
        let options = [
            (ThemeChoice::System, "Match macOS"),
            (ThemeChoice::Night, "Night"),
            (ThemeChoice::Paper, "Paper"),
        ];
        let set = self.setter(|s, v| s.appearance.theme = v);
        h_flex()
            .gap(px(14.))
            .children(options.into_iter().map(|(choice, label)| {
                let selected = choice == current;
                let set = set.clone();
                let preview = match choice {
                    ThemeChoice::Night => mock(0x0A0A0B, 0x111113, 0x2A2A2F, 0xFF7A3D),
                    ThemeChoice::Paper => mock(0xFBFAF8, 0xF2F0EC, 0xD9D5CE, 0xE85D1F),
                    ThemeChoice::System => h_flex()
                        .size_full()
                        .child(div().w(relative(0.5)).h_full().overflow_hidden().child(mock(0xFBFAF8, 0xF2F0EC, 0xD9D5CE, 0xE85D1F)))
                        .child(div().w(relative(0.5)).h_full().overflow_hidden().child(mock(0x0A0A0B, 0x111113, 0x2A2A2F, 0xFF7A3D)))
                        .into_any_element(),
                };
                v_flex()
                    .id(SharedString::from(format!("theme-{label}")))
                    .gap(px(8.))
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(132.))
                            .h(px(84.))
                            .rounded(px(9.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(if selected { theme.foreground.opacity(0.9) } else { theme.foreground.opacity(0.1) })
                            .when(selected, |el| el.shadow(vec![BoxShadow { color: theme.foreground.opacity(0.9), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false }]))
                            .child(preview),
                    )
                    .child(div().text_size(px(12.5)).text_color(if selected { theme.foreground } else { theme.muted_foreground }).child(label))
                    .on_click(move |_, window, cx| {
                        set(choice, window, cx);
                        crate::set_theme(choice, window, cx);
                    })
            }))
            .into_any_element()
    }

    pub(super) fn appearance_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let text_size = match s.appearance.transcript_font_size() {
            v if v < 14.0 => 0u8,
            v if v < 15.25 => 1,
            v if v < 16.75 => 2,
            _ => 3,
        };
        vec![
            self.theme_tiles(s.appearance.theme, cx),
            Self::heading("Text size", cx),
            ui::group(
                vec![
                    Self::row(
                        "Conversation text",
                        "Messages from you and your agents.",
                        ui::segmented(
                            "text-size",
                            vec![(0u8, "Small"), (1, "Default"), (2, "Large"), (3, "Larger")],
                            text_size,
                            self.setter(|s, v: u8| s.appearance.transcript_font_size = [13.5, 14.5, 16.0, 17.5][v as usize]),
                            cx,
                        ),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Background", cx),
            Self::note("Art behind the composer on a new thread, or tinted behind the whole window.", cx),
            self.background_gallery(s, cx),
            div().h(px(16.)).into_any_element(),
            ui::group(
                vec![
                    Self::row(
                        "Show on",
                        "",
                        ui::segmented(
                            "bg-place",
                            vec![(trek_core::settings::BackgroundPlacement::NewThread, "New thread"), (trek_core::settings::BackgroundPlacement::Everywhere, "Everywhere")],
                            s.appearance.background_placement,
                            self.setter(|s, v| s.appearance.background_placement = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "Dim",
                        "Darkens the art so text stays readable.",
                        ui::segmented(
                            "bg-dim",
                            vec![(0u8, "None"), (1, "Light"), (2, "Medium"), (3, "Strong")],
                            match s.appearance.background_dim {
                                d if d < 0.1 => 0u8,
                                d if d < 0.3 => 1,
                                d if d < 0.5 => 2,
                                _ => 3,
                            },
                            self.setter(|s, v: u8| s.appearance.background_dim = [0.0, 0.2, 0.4, 0.6][v as usize]),
                            cx,
                        ),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Motion", cx),
            ui::group(
                vec![Self::row(
                    "Reduce motion",
                    "Fades instead of slides, and no trail drawing.",
                    self.switch("reduce-motion", s.appearance.reduce_motion, |s, v| s.appearance.reduce_motion = v),
                    cx,
                )],
                cx,
            ),
        ]
    }

    pub(super) fn notifications_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let n = &s.notifications;
        vec![
            ui::group(
                vec![
                    Self::row(
                        "Alerts",
                        "When a turn finishes or an agent is waiting for your approval.",
                        ui::segmented(
                            "notify-mode",
                            vec![(NotifyMode::BannerAndSound, "Banner & sound"), (NotifyMode::Banner, "Banner"), (NotifyMode::Sound, "Sound"), (NotifyMode::Off, "Off")],
                            n.mode,
                            self.setter(|s, v| s.notifications.mode = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "Only when Trek is in the background",
                        "While you're in Trek, other threads show a note in the window instead of a banner or sound.",
                        // With alerts off there's no banner or sound for it to hold back.
                        self.switch("notify-unfocused", n.only_when_unfocused, |s, v| s.notifications.only_when_unfocused = v).disabled(n.mode == NotifyMode::Off),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Badges", cx),
            ui::group(
                vec![
                    Self::row(
                        "Dock badge",
                        "Count of threads waiting on you, on Trek's Dock icon.",
                        self.switch("dock-badge", n.dock_badge, |s, v| s.notifications.dock_badge = v),
                        cx,
                    ),
                    Self::row(
                        "Menu bar icon",
                        "The trail fills in while agents work and shows a dot when one needs you.",
                        self.switch("menu-bar-icon", n.menu_bar_icon, |s, v| s.notifications.menu_bar_icon = v),
                        cx,
                    ),
                ],
                cx,
            ),
        ]
    }

    pub(super) fn shortcuts_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        const GROUPS: &[(&str, &[(&str, &[&str])])] = &[
            (
                "Threads",
                &[
                    ("Search threads and commands", &["⌘", "K"]),
                    ("New thread", &["⌘", "N"]),
                    ("Open a folder", &["⌘", "O"]),
                    ("Open the thread in a new window", &["⌘", "⇧", "↩"]),
                    ("Settle the current thread", &["⌘", "E"]),
                    ("Stop the agent", &["⌘", "."]),
                ],
            ),
            (
                "Composer",
                &[("Send", &["↩"]), ("New line", &["⇧", "↩"]), ("Plan mode", &["⇧", "⇥"]), ("Cycle hand-holding", &["⌘", "⇧", "A"]), ("Commands", &["/"]), ("Mention a file", &["@"]), ("Use a skill", &["$"]), ("Attach a copied image", &["⌘", "V"]), ("Take a snapshot", &["⌘", "⇧", "S"])],
            ),
            ("Window", &[("Basecamp", &["⌘", "⇧", "H"]), ("Leave Basecamp", &["esc"]), ("Toggle the sidebar", &["⌘", "B"]), ("Toggle the tools panel", &["⌘", "J"]), ("Settings", &["⌘", ","]), ("Close a thread window", &["⌘", "W"]), ("Hide Trek", &["⌘", "H"]), ("Minimize", &["⌘", "M"]), ("Quit", &["⌘", "Q"])]),
        ];
        let send_cmd = self.workspace.read(cx).settings.general.send_with_cmd_enter;
        let mut out = vec![];
        for (gi, (group, items)) in GROUPS.iter().enumerate() {
            if gi > 0 {
                out.push(Self::heading(group, cx));
            } else {
                out.push(div().pb(px(10.)).text_size(px(13.)).font_semibold().child(group.to_string()).into_any_element());
            }
            let rows = items
                .iter()
                .map(|(label, keys)| {
                    let keys: Vec<&str> = match (*label, send_cmd) {
                        ("Send", true) => vec!["⌘", "↩"],
                        ("New line", true) => vec!["↩"],
                        _ => keys.to_vec(),
                    };
                    h_flex()
                        .h(px(42.))
                        .child(div().flex_1().text_size(px(13.)).child(label.to_string()))
                        .child(h_flex().gap(px(4.)).children(keys.iter().map(|k| key(k, cx))))
                        .into_any_element()
                })
                .collect();
            out.push(ui::group(rows, cx));
        }
        out
    }

    pub(super) fn api_keys_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let mut rows = vec![];
        for p in DIRECT_PROVIDERS.iter().filter(|p| !p.local) {
            let Some(input) = self.key_inputs.get(p.id).cloned() else { continue };
            let saved = self.saved_keys.get(p.id).copied().unwrap_or(false);
            let from_env = p.env_key.filter(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
            let id = p.id;
            let status: SharedString = match (saved, from_env) {
                (true, _) => "Saved in your Keychain".into(),
                (false, Some(k)) => format!("Using ${k} from your shell").into(),
                (false, None) => "Not set".into(),
            };
            let title = h_flex()
                .gap(px(10.))
                .child(ui::agent_logo(&AgentId::Direct(p.id.to_string()), px(18.), cx))
                .child(v_flex().child(div().text_size(px(13.5)).font_medium().child(p.name)).child(div().text_size(px(12.)).text_color(muted).child(status)));
            let control = if saved {
                h_flex()
                    .gap(px(6.))
                    .child(div().text_size(px(12.5)).text_color(muted).child("••••••••"))
                    .child(Button::new(SharedString::from(format!("del-{id}"))).small().ghost().label("Remove").on_click(cx.listener(move |this, _, _, cx| {
                        let _ = secrets::delete_api_key(id);
                        this.saved_keys.insert(id, false);
                        this.workspace.update(cx, |ws, cx| {
                            ws.settings.api_providers.retain(|p| p != id);
                            ws.save_settings(cx);
                        });
                    })))
                    .into_any_element()
            } else {
                h_flex()
                    .gap(px(6.))
                    .w(px(300.))
                    .child(div().flex_1().child(Input::new(&input).small()))
                    .child(Button::new(SharedString::from(format!("save-{id}"))).small().outline().label("Save").on_click(cx.listener(move |this, _, window, cx| {
                        let Some(input) = this.key_inputs.get(id).cloned() else { return };
                        let key = input.read(cx).value().trim().to_string();
                        if key.is_empty() {
                            return;
                        }
                        match secrets::set_api_key(id, &key) {
                            Ok(()) => {
                                this.saved_keys.insert(id, true);
                                input.update(cx, |s, cx| s.set_value("", window, cx));
                                this.workspace.update(cx, |ws, cx| {
                                    if !ws.settings.api_providers.iter().any(|p| p == id) {
                                        ws.settings.api_providers.push(id.to_string());
                                    }
                                    ws.save_settings(cx);
                                });
                                window.push_notification("Saved to your Keychain", cx);
                            }
                            Err(e) => window.push_notification(format!("Couldn't save the key: {e}"), cx),
                        }
                    })))
                    .into_any_element()
            };
            rows.push(Self::row(title, "", control, cx));
        }
        vec![ui::group(rows, cx)]
    }

    pub(super) fn permissions_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let unlocked = s.permissions.full_access_unlocked;
        let current = s.general.hand_holding;
        let levels = HandHolding::ALL
            .into_iter()
            .map(|h| {
                let locked = h == HandHolding::FullAccess && !unlocked;
                let selected = h == current;
                let ws = self.workspace.clone();
                h_flex()
                    .id(SharedString::from(format!("level-{h:?}")))
                    .py(px(13.))
                    .gap(px(12.))
                    .items_start()
                    .when(!locked, |el| el.cursor_pointer())
                    .when(locked, |el| el.opacity(0.5))
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(16.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .border_color(if selected { palette::ember(cx) } else { theme.foreground.opacity(0.25) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |el| el.child(div().size(px(8.)).rounded_full().bg(palette::ember(cx)))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(px(3.))
                            .child(div().text_size(px(13.5)).font_medium().child(h.label()))
                            .child(div().text_size(px(12.5)).line_height(relative(1.5)).text_color(theme.muted_foreground).child(if locked {
                                "Turn on “Allow Full access” below to choose it.".to_string()
                            } else {
                                h.description().to_string()
                            })),
                    )
                    .on_click(move |_, _, cx| {
                        if !locked {
                            ws.update(cx, |ws, cx| {
                                ws.settings.general.hand_holding = h;
                                ws.save_settings(cx);
                            })
                        }
                    })
                    .into_any_element()
            })
            .collect();

        let view = cx.entity().downgrade();
        let full_switch = Switch::new("full-access").checked(unlocked).on_click(move |v: &bool, window, cx| {
            let view = view.clone();
            if !*v {
                let _ = view.update(cx, |this, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.settings.permissions.full_access_unlocked = false;
                        if ws.settings.general.hand_holding == HandHolding::FullAccess {
                            ws.settings.general.hand_holding = HandHolding::Auto;
                        }
                        ws.save_settings(cx);
                    })
                });
                return;
            }
            window.open_alert_dialog(cx, move |alert, _, _| {
                let view = view.clone();
                alert
                    .title("Allow Full access?")
                    .description("Agents will run commands and edit files anywhere on this Mac without asking. Use it only for work you'd trust to run unattended.")
                    .confirm()
                    .ok_text("Allow Full access")
                    .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.settings.permissions.full_access_unlocked = true;
                                ws.save_settings(cx);
                            })
                        });
                        true
                    })
            });
        });

        // What each level means to each agent, as a table rather than run-on sentences.
        let mono = theme.mono_font_family.clone();
        let cell = |t: String, head: bool| {
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .when(head, |el| el.text_color(theme.muted_foreground).font_medium())
                .when(!head, |el| el.font_family(mono.clone()).text_color(theme.foreground.opacity(0.85)))
                .child(t)
        };
        let mut table = vec![h_flex()
            .h(px(36.))
            .child(div().w(px(150.)).text_size(px(12.)).font_medium().text_color(theme.muted_foreground).child("Level"))
            .child(cell("Claude Code".into(), true))
            .child(cell("Codex".into(), true))
            .into_any_element()];
        for h in HandHolding::ALL {
            let (sb, ap, _) = h.codex_policy();
            table.push(
                h_flex()
                    .h(px(38.))
                    .child(div().w(px(150.)).text_size(px(13.)).child(h.label()))
                    .child(cell(h.claude_mode().to_string(), false))
                    .child(cell(format!("{sb} · {ap}"), false))
                    .into_any_element(),
            );
        }

        vec![
            div().pb(px(10.)).text_size(px(13.)).font_semibold().child("Default for new threads").into_any_element(),
            ui::group(levels, cx),
            div().pt(px(10.)).text_size(px(12.5)).text_color(theme.muted_foreground).child("Change it for any thread from the composer.").into_any_element(),
            Self::heading("Full access", cx),
            ui::group(vec![Self::row("Allow Full access", "Adds the no-prompts, no-sandbox level. Off by default.", full_switch, cx)], cx),
            Self::heading("What each level means", cx),
            Self::note("Trek translates your choice into each agent's own permission settings.", cx),
            ui::group(table, cx),
        ]
    }

    pub(super) fn updates_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let view = ws.update_view();
        let pending = ws.pending_changes();
        // The release history: what this channel has published up to the update on offer.
        let ceiling = ws.updater.offer.as_ref().map(|o| o.version.clone()).filter(|_| pending.is_some()).unwrap_or_else(trek_core::update::current_version);
        let history: Vec<trek_core::changelog::Release> = ws
            .updater
            .changelog
            .iter()
            .filter(|r| r.version <= ceiling && trek_core::changelog::on_channel(r, s.updates.channel))
            .filter(|r| pending.as_ref().is_none_or(|p| !p.releases.iter().any(|q| q.version == r.version)))
            .take(8)
            .cloned()
            .collect();
        let repo = trek_core::changelog::github_repo(&s.updates);
        let action = view.action.map(|action| {
            Button::new("update-action")
                .small()
                .when(action == UpdateAction::Restart, |b| b.primary())
                .when(action != UpdateAction::Restart, |b| b.outline())
                .loading(view.busy)
                .label(action.label())
                .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.run_update_action(action, cx))))
        });
        let header = h_flex()
            .gap(px(14.))
            .pb(px(24.))
            .child(div().flex_none().child(crate::brand::logo_mark(px(36.))))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(15.)).font_semibold().child(format!("Trek {}", trek_core::VERSION)))
                    .child(div().text_size(px(12.5)).line_height(relative(1.5)).text_color(theme.muted_foreground).child(view.line))
                    .when_some(view.progress, |el, p| {
                        el.child(div().mt(px(6.)).h(px(4.)).max_w(px(280.)).rounded_full().bg(theme.foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(palette::ember(cx)).w(relative(p))))
                    }),
            )
            .children(action.map(|b| div().flex_none().child(b)))
            .into_any_element();
        let mut page = vec![header];
        if let Some(changes) = pending {
            let title = match changes.releases.as_slice() {
                [one] => format!("What's new in Trek {}", one.version),
                many => format!("What's changed · {} releases", many.len()),
            };
            page.push(
                h_flex()
                    .pb(px(10.))
                    .gap(px(8.))
                    .child(div().text_size(px(13.)).font_semibold().child(title))
                    .child(div().flex_1())
                    .when_some(changes.compare, |el, url| el.child(ui::web_link("compare-link", "Compare on GitHub", url, cx)))
                    .into_any_element(),
            );
            page.push(div().pb(px(28.)).max_w(px(560.)).child(ui::releases_notes("update-notes", &changes.releases, px(360.), cx)).into_any_element());
        }
        page.push(ui::group(
            vec![
                Self::row(
                    "Channel",
                    "Stable gets finished releases. Beta and Nightly get new things first, with the occasional rough edge, and every stable release too.",
                    ui::segmented(
                        "channel",
                        vec![(Channel::Stable, "Stable"), (Channel::Beta, "Beta"), (Channel::Nightly, "Nightly")],
                        s.updates.channel,
                        self.setter(|s, v| s.updates.channel = v),
                        cx,
                    ),
                    cx,
                ),
                Self::row("Check automatically", "Once a day, in the background.", self.switch("auto-check", s.updates.auto_check, |s, v| s.updates.auto_check = v), cx),
                Self::row(
                    "Download automatically",
                    "Gets the update ready in the background. It installs when you restart or quit Trek, never while an agent is working.",
                    self.switch("auto-dl", s.updates.auto_download, |s, v| s.updates.auto_download = v),
                    cx,
                ),
            ],
            cx,
        ));
        if !history.is_empty() {
            page.push(
                h_flex()
                    .pt(px(28.))
                    .pb(px(10.))
                    .gap(px(8.))
                    .child(div().text_size(px(13.)).font_semibold().child("Release history"))
                    .child(div().flex_1())
                    .when_some(repo, |el, repo| el.child(ui::web_link("all-releases", "All releases", format!("https://github.com/{repo}/releases"), cx)))
                    .into_any_element(),
            );
            page.push(div().pb(px(28.)).max_w(px(560.)).child(ui::releases_notes("release-history", &history, px(420.), cx)).into_any_element());
        }
        page
    }

    pub(super) fn about_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let data_dir = trek_core::paths::data_dir();
        let settings_file = trek_core::paths::settings_file();
        let channel = format!("{:?}", s.updates.channel);
        let header = h_flex()
            .gap(px(16.))
            .pb(px(28.))
            .child(img("brand/icon.png").size(px(64.)).flex_none())
            .child(
                v_flex()
                    .gap(px(3.))
                    .child(div().text_size(px(18.)).font_semibold().child("Trek"))
                    .child(div().text_size(px(13.)).text_color(theme.muted_foreground).child(format!("Version {} · {channel}", trek_core::VERSION)))
                    .child(div().text_size(px(13.)).text_color(theme.muted_foreground).child("Every agent, one trail.")),
            )
            .into_any_element();
        let reveal = {
            let d = data_dir.clone();
            Button::new("reveal-data").small().outline().label("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&d))
        };
        let open_settings = {
            let f = settings_file.clone();
            Button::new("open-settings-file").small().outline().label("Open").on_click(move |_, _, cx| cx.open_with_system(&f))
        };
        let copy = Button::new("copy-diag").small().outline().label("Copy").on_click(cx.listener(|this, _, window, cx| {
            let ws = this.workspace.read(cx);
            let mut lines = vec![format!("Trek {}", trek_core::VERSION)];
            if let Ok(o) = std::process::Command::new("sw_vers").arg("-productVersion").output() {
                lines.push(format!("macOS {}", String::from_utf8_lossy(&o.stdout).trim()));
            }
            for a in &ws.agents {
                lines.push(format!("{}: {:?}{}", a.name, a.availability, a.version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default()));
            }
            lines.push(format!("Settings: {}", trek_core::paths::settings_file().display()));
            cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
            window.push_notification("Diagnostics copied", cx);
        }));
        vec![
            header,
            ui::group(
                vec![
                    Self::row("Data folder", trek_core::paths::tildify(&data_dir), reveal, cx),
                    Self::row("Settings file", trek_core::paths::tildify(&settings_file), open_settings, cx),
                    Self::row("Diagnostics", "Version, macOS and agent details, for a bug report.", copy, cx),
                ],
                cx,
            ),
        ]
    }
}

// ---------------------------------------------------------------------------------------------
// App Snapshots and Skills

impl SettingsView {
    pub(super) fn snapshots_page(&mut self, s: &Settings, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use trek_core::settings::{SnapshotFormat, SnapshotMode};
        let theme = cx.theme().clone();
        let p = &s.snapshots;
        let (count, bytes) = crate::mentions::snapshot_usage();
        let size = match bytes {
            b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
            b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
            b => format!("{} KB", b / 1024),
        };
        let folder = trek_core::paths::data_dir().join("snapshots");
        let sr = crate::integrations::screen_recording_allowed();
        let reveal = {
            let f = folder.clone();
            Button::new("snap-reveal").small().outline().label("Show in Finder").on_click(move |_, _, cx| {
                let _ = std::fs::create_dir_all(&f);
                cx.open_with_system(&f)
            })
        };
        let clear = Button::new("snap-clear").small().ghost().label("Clear").disabled(count == 0).on_click(cx.listener(|_, _, window, cx| {
            if let Ok(entries) = std::fs::read_dir(trek_core::paths::data_dir().join("snapshots")) {
                for e in entries.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            window.push_notification("Snapshots cleared", cx);
            cx.notify();
        }));
        let permission: AnyElement = if sr {
            Self::status_dot(palette::emerald(cx), "Allowed")
        } else {
            Button::new("snap-perm").small().outline().label("Allow Screen Recording").on_click(|_, _, cx| cx.open_url(crate::integrations::SCREEN_RECORDING_PANE)).into_any_element()
        };
        vec![
            ui::group(
                vec![
                    Self::row(
                        "⌘⇧S takes",
                        "The + menu in the composer offers all three.",
                        ui::segmented(
                            "snap-mode",
                            vec![(SnapshotMode::Window, "A window"), (SnapshotMode::Area, "An area"), (SnapshotMode::Screen, "The screen")],
                            p.default_mode,
                            self.setter(|s, v| s.snapshots.default_mode = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row("Hide Trek while capturing", "Trek steps aside so you can pick the window behind it.", self.switch("snap-hide", p.hide_trek, |s, v| s.snapshots.hide_trek = v), cx),
                    Self::row("Window shadow", "Keep macOS's drop shadow around window snapshots.", self.switch("snap-shadow", p.window_shadow, |s, v| s.snapshots.window_shadow = v), cx),
                    Self::row("Shutter sound", "", self.switch("snap-sound", p.sound, |s, v| s.snapshots.sound = v), cx),
                    Self::row(
                        "Format",
                        "PNG is sharp for UI; JPEG is smaller for photos and busy screens.",
                        ui::segmented("snap-format", vec![(SnapshotFormat::Png, "PNG"), (SnapshotFormat::Jpg, "JPEG")], p.format, self.setter(|s, v| s.snapshots.format = v), cx),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Storage", cx),
            ui::group(
                vec![
                    Self::row(
                        "Keep snapshots",
                        "Older ones are deleted when Trek starts. Messages that used them keep their text.",
                        ui::segmented(
                            "snap-keep",
                            vec![(1u32, "1 day"), (7, "1 week"), (14, "2 weeks"), (30, "1 month"), (0, "Forever")],
                            p.keep_days,
                            self.setter(|s, v| s.snapshots.keep_days = v),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "Snapshot folder",
                        if count == 0 { "Empty".to_string() } else { format!("{count} snapshot{} · {size}", if count == 1 { "" } else { "s" }) },
                        h_flex().gap(px(6.)).child(clear).child(reveal),
                        cx,
                    ),
                ],
                cx,
            ),
            Self::heading("Permission", cx),
            ui::group(vec![Self::row("Screen Recording", "macOS asks once; snapshots of other apps need it.", permission, cx)], cx),
            div().pt(px(12.)).text_size(px(12.5)).text_color(theme.muted_foreground).child("Snapshots are saved inside Trek's data folder and attached as images to your next message.").into_any_element(),
        ]
    }

    pub(super) fn skills_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use trek_core::skills::{self, SkillHome, SkillSource};
        let theme = cx.theme().clone();
        if self.skills.is_none() {
            let project = self.workspace.read(cx).current_cwd();
            self.skills = Some(skills::discover(project.as_deref()));
        }
        let all = self.skills.clone().unwrap_or_default();
        let q = self.skill_filter.read(cx).value().to_lowercase();
        let shown: Vec<_> = all.iter().filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q)).cloned().collect();
        let off = all.iter().filter(|s| !s.enabled).count();

        // Toolbar: filter, add from a folder, new skill.
        let view = cx.entity();
        let add = Button::new("skill-add").small().outline().icon(IconName::FolderOpen).label("Add from folder").dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let mut menu = menu.min_w(px(220.)).label("Copy the skill into…");
            for (home, label) in [(SkillHome::ClaudeCode, "Claude Code"), (SkillHome::Codex, "Codex"), (SkillHome::Shared, "All agents (~/.agents)")] {
                let view = view.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| view.update(cx, |this, cx| this.add_skill_from_folder(home, cx))));
            }
            menu
        });
        let new = Button::new("skill-new").small().primary().icon(IconName::Plus).label("New skill").on_click(cx.listener(|this, _, window, cx| this.open_new_skill(window, cx)));
        let toolbar = h_flex()
            .gap(px(8.))
            .pb(px(20.))
            .child(div().flex_1().child(Input::new(&self.skill_filter).small().prefix(Icon::new(IconName::Search).size(px(14.)).text_color(theme.muted_foreground))))
            .child(add)
            .child(new)
            .into_any_element();

        let mut out = vec![toolbar];
        out.push(
            div()
                .pb(px(4.))
                .text_size(px(12.5))
                .text_color(theme.muted_foreground)
                .child(format!("{} skills{}", all.len(), if off > 0 { format!(" · {off} off") } else { String::new() }))
                .into_any_element(),
        );
        // Group by source, editable homes first.
        let mut order: Vec<SkillSource> = vec![SkillSource::ClaudeCode, SkillSource::Codex, SkillSource::Shared, SkillSource::Project];
        for s in &shown {
            if !order.contains(&s.source) {
                order.push(s.source.clone());
            }
        }
        for source in order {
            let group: Vec<_> = shown.iter().filter(|s| s.source == source).cloned().collect();
            if group.is_empty() {
                continue;
            }
            out.push(Self::heading(&source.label(), cx));
            let rows = group.into_iter().enumerate().map(|(i, skill)| self.skill_row(i, skill, cx)).collect();
            out.push(ui::group(rows, cx));
        }
        if shown.is_empty() {
            out.push(div().py(px(24.)).text_size(px(13.)).text_color(theme.muted_foreground).child(if all.is_empty() { "No skills yet. Create one, or add a folder that contains a SKILL.md." } else { "No skills match." }).into_any_element());
        }
        out
    }

    fn skill_row(&mut self, i: usize, skill: trek_core::skills::Skill, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let key = format!("{}-{}", skill.source.label(), i);
        let editable = skill.source.editable();
        let title = h_flex()
            .gap(px(8.))
            .child(div().text_size(px(13.5)).font_medium().when(!skill.enabled, |el| el.text_color(theme.muted_foreground)).child(skill.name.clone()));
        let desc: String = skill.description.chars().take(120).collect::<String>() + if skill.description.chars().count() > 120 { "…" } else { "" };
        let md = skill.skill_md();
        let dir = skill.dir.clone();
        let view = cx.entity();
        let s2 = skill.clone();
        let more = Button::new(SharedString::from(format!("skill-more-{key}")))
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis).text_color(theme.muted_foreground))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let (md, dir, view, s2) = (md.clone(), dir.clone(), view.clone(), s2.clone());
                let mut menu = menu
                    .min_w(px(190.))
                    .item(PopupMenuItem::new("Open SKILL.md").on_click(move |_, _, cx| cx.open_with_system(&md)))
                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&dir)));
                if s2.source.editable() {
                    menu = menu.separator().item(PopupMenuItem::new("Move to Trash").icon(IconName::Delete).on_click(move |_, window, cx| {
                        let s3 = s2.clone();
                        let view = view.clone();
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            let (s4, view) = (s3.clone(), view.clone());
                            alert
                                .title(format!("Move “{}” to the Trash?", s3.name))
                                .description("Every agent stops loading it. You can restore it from the Trash.")
                                .confirm()
                                .ok_text("Move to Trash")
                                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                                .on_ok(move |_, window, cx| {
                                    match trek_core::skills::trash(&s4) {
                                        Ok(()) => window.push_notification(format!("Moved {} to the Trash", s4.name), cx),
                                        Err(e) => window.push_notification(format!("{e}"), cx),
                                    }
                                    view.update(cx, |this, cx| this.skills_changed(cx));
                                    true
                                })
                        });
                    }));
                }
                menu
            });
        let trailing: AnyElement = if editable {
            let s2 = skill.clone();
            Switch::new(SharedString::from(format!("skill-on-{key}")))
                .checked(skill.enabled)
                .on_click(cx.listener(move |this, v: &bool, window, cx| {
                    if let Err(e) = trek_core::skills::set_enabled(&s2, *v) {
                        window.push_notification(format!("{e}"), cx);
                    }
                    this.skills_changed(cx);
                }))
                .into_any_element()
        } else {
            div().text_size(px(12.)).text_color(theme.muted_foreground).child("Read-only").into_any_element()
        };
        Self::row(title, desc, h_flex().gap(px(6.)).child(more).child(div().pl(px(4.)).child(trailing)), cx)
    }

    /// Rescan, and have the agents' command lists (the `$` picker) pick the change up.
    fn skills_changed(&mut self, cx: &mut Context<Self>) {
        self.skills = None;
        self.workspace.update(cx, |ws, cx| {
            ws.status_fetched_at = 0;
            ws.refresh_usage(cx);
        });
        cx.notify();
    }

    fn add_skill_from_folder(&mut self, home: trek_core::skills::SkillHome, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Add Skill".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let errors: Vec<String> = paths.iter().filter_map(|p| trek_core::skills::install_from(p, home).err().map(|e| e.to_string())).collect();
            let _ = this.update(cx, |this, cx| {
                if let Some(e) = errors.first() {
                    this.workspace.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: e.clone(), undo: None }));
                }
                this.skills_changed(cx);
            });
        })
        .detach();
    }

    fn open_new_skill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use trek_core::skills::SkillHome;
        let (name, desc) = (self.skill_name.clone(), self.skill_desc.clone());
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let home = view.read(cx).skill_home;
            let (name2, desc2, view2) = (name.clone(), desc.clone(), view.clone());
            let pick = {
                let view = view.clone();
                move |h: SkillHome, _: &mut Window, cx: &mut App| view.update(cx, |this, cx| {
                    this.skill_home = h;
                    cx.notify();
                })
            };
            dialog
                .title("New skill")
                .w(px(480.))
                .child(
                    v_flex()
                        .gap(px(12.))
                        .child(Input::new(&name))
                        .child(Input::new(&desc))
                        .child(
                            h_flex()
                                .gap(px(10.))
                                .child(div().text_size(px(13.)).text_color(cx.theme().muted_foreground).child("Save to"))
                                .child(ui::segmented("skill-home", vec![(SkillHome::ClaudeCode, "Claude Code"), (SkillHome::Codex, "Codex"), (SkillHome::Shared, "All agents")], home, pick, cx)),
                        ),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(Button::new("skill-cancel").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(Button::new("skill-create").primary().label("Create and open").on_click(move |_, window, cx| {
                            let n = name2.read(cx).value().to_string();
                            let d = desc2.read(cx).value().to_string();
                            let home = view2.read(cx).skill_home;
                            match trek_core::skills::create(&n, &d, home) {
                                Ok(md) => {
                                    cx.open_with_system(&md);
                                    name2.update(cx, |s, cx| s.set_value("", window, cx));
                                    desc2.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                                Err(e) => window.push_notification(format!("{e}"), cx),
                            }
                            view2.update(cx, |this, cx| this.skills_changed(cx));
                        }))),
                )
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Project

impl SettingsView {
    pub(super) fn project_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use trek_core::settings::ProjectAction;
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let projects: Vec<trek_core::store::Project> = ws.workspace_projects().into_iter().cloned().collect();
        let selected = ws.settings_project.clone().and_then(|id| projects.iter().find(|p| p.id == id).cloned()).or_else(|| projects.first().cloned());
        let Some(project) = selected else {
            return vec![div().py(px(24.)).text_size(px(13.)).text_color(theme.muted_foreground).child("No projects yet. Add one from the sidebar's folder button.").into_any_element()];
        };
        let prefs = ws.project_prefs(&project.path);
        let general = ws.settings.general.clone();
        let agents = ws.ready_agents();
        let unlocked = ws.settings.permissions.full_access_unlocked;
        let agent = prefs.agent.as_deref().map(AgentId::from_key).filter(|a| agents.contains(a));
        let effective_agent = agent.clone().unwrap_or_else(|| AgentId::from_key(&general.default_agent));
        let models = ws.models_for(&effective_agent);
        let threads = ws.threads.iter().filter(|t| t.project_id.as_deref() == Some(project.id.as_str())).count();
        let path = project.path.clone();
        let pid = project.id.clone();

        // Which project these settings apply to.
        let w = self.workspace.clone();
        let chooser = h_flex()
            .gap(px(8.))
            .pb(px(24.))
            .text_size(px(13.))
            .text_color(theme.muted_foreground)
            .child("Settings for")
            .child(picker(
                "project-chooser",
                Some(ui::project_badge(&project.name, &ui::ProjectLook::of(&prefs), cx)),
                project.remote.clone().unwrap_or_else(|| project.name.clone()),
                projects.iter().map(|p| (p.id.clone(), p.remote.clone().unwrap_or_else(|| p.name.clone()))).collect(),
                Some(project.id.clone()),
                move |id: String, cx| {
                    w.update(cx, |ws, cx| {
                        ws.settings_project = Some(id);
                        cx.notify();
                    })
                },
                cx,
            ))
            .into_any_element();

        // Icon: monogram, one of Trek's icons, or an image file.
        let w = self.workspace.clone();
        let p2 = path.clone();
        let icon_menu = Button::new("project-icon").small().outline().label("Choose icon").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            menu = menu.min_w(px(190.)).max_h(px(320.)).scrollable(true);
            for (key, icon) in ui::PROJECT_ICONS {
                let (w, p) = (w.clone(), p2.clone());
                let mut label: String = key.to_string();
                if let Some(f) = label.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                menu = menu.item(PopupMenuItem::new(label).icon(Icon::new(*icon)).on_click(move |_, _, cx| {
                    let spec = format!("lucide:{key}");
                    w.update(cx, |ws, cx| ws.update_project_prefs(&p, |pr| pr.icon = Some(spec), cx))
                }));
            }
            menu
        });
        let view = cx.entity();
        let p2 = path.clone();
        let icon_file = Button::new("project-icon-file").small().outline().label("Choose image…").on_click(move |_, _, cx| {
            let p = p2.clone();
            view.update(cx, |this, cx| this.pick_project_image(p, cx))
        });
        let w = self.workspace.clone();
        let p2 = path.clone();
        let icon_reset = prefs.icon.is_some().then(|| {
            Button::new("project-icon-reset").small().ghost().label("Reset").on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p2, |pr| pr.icon = None, cx)))
        });
        // Colour: from the name, or one of a few that stay apart.
        let colors = {
            let dark = cx.theme().mode.is_dark();
            let ring = cx.theme().foreground.opacity(0.75);
            let options = std::iter::once((None, "Automatic: from the name".to_string())).chain(ui::PROJECT_COLORS.iter().map(|(name, hue)| (Some(*hue), name.to_string())));
            h_flex().gap(px(4.)).children(options.map(|(hue, name)| {
                let (w, p) = (self.workspace.clone(), path.clone());
                let id = SharedString::from(format!("project-color-{}", hue.map_or("auto".to_string(), |h| h.to_string())));
                let picked = prefs.color == hue;
                div()
                    .id(id)
                    .test_support()
                    .size(px(22.))
                    .p(px(2.))
                    .rounded_full()
                    .border_2()
                    .border_color(if picked { ring } else { gpui_kit::transparent_black() })
                    .cursor_pointer()
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx))
                    .when(hue.is_none(), |el| el.mr(px(4.)))
                    .child({
                        let ink = ui::project_ink(ui::project_hue(&project.name, hue), dark);
                        // Automatic is a ring in the name's colour, so it doesn't read as one of the picks.
                        let dot = div().size_full().rounded_full();
                        if hue.is_none() { dot.border_2().border_color(ink) } else { dot.bg(ink) }
                    })
                    .on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p, |pr| pr.color = hue, cx)))
            }))
        };
        let reveal = {
            let p = path.clone();
            Button::new("project-reveal").small().outline().label("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&p))
        };

        // New-thread defaults; "Trek default" clears the override.
        let w = self.workspace.clone();
        let p2 = path.clone();
        let mut agent_opts: Vec<(Option<AgentId>, String)> = vec![(None, format!("Trek default ({})", AgentId::from_key(&general.default_agent).display_name()))];
        agent_opts.extend(agents.iter().map(|a| (Some(a.clone()), a.display_name())));
        let agent_picker = picker(
            "proj-agent",
            Some(ui::agent_logo(&effective_agent, px(14.), cx)),
            agent.as_ref().map(|a| a.display_name()).unwrap_or_else(|| "Trek default".into()),
            agent_opts,
            Some(agent.clone()),
            move |a: Option<AgentId>, cx| {
                w.update(cx, |ws, cx| {
                    ws.update_project_prefs(
                        &p2,
                        |pr| {
                            pr.agent = a.map(|a| a.key());
                            pr.model = None;
                        },
                        cx,
                    )
                })
            },
            cx,
        );
        let w = self.workspace.clone();
        let p2 = path.clone();
        let model_id = prefs.model.clone().filter(|m| models.iter().any(|i| crate::composer::same_model(m, &i.id)));
        let mut model_opts: Vec<(Option<String>, String)> = vec![(None, "Agent's default".into())];
        model_opts.extend(models.iter().map(|m| (Some(m.id.clone()), m.name.clone())));
        let model_picker = picker(
            "proj-model",
            None,
            model_id.as_ref().and_then(|m| models.iter().find(|i| crate::composer::same_model(m, &i.id))).map(|m| m.name.clone()).unwrap_or_else(|| "Agent's default".into()),
            model_opts,
            Some(model_id.clone()),
            move |m: Option<String>, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p2, |pr| pr.model = m, cx)),
            cx,
        );
        let w = self.workspace.clone();
        let p2 = path.clone();
        let mut effort_opts: Vec<(Option<Effort>, String)> = vec![(None, format!("Trek default ({})", general.default_effort.label()))];
        effort_opts.extend([Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max].into_iter().map(|e| (Some(e), e.label().to_string())));
        let effort_picker = picker(
            "proj-effort",
            None,
            prefs.effort.map(|e| e.label().to_string()).unwrap_or_else(|| "Trek default".into()),
            effort_opts,
            Some(prefs.effort),
            move |e: Option<Effort>, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p2, |pr| pr.effort = e, cx)),
            cx,
        );
        let w = self.workspace.clone();
        let p2 = path.clone();
        let mut hh_opts: Vec<(Option<HandHolding>, String)> = vec![(None, format!("Trek default ({})", general.hand_holding.label()))];
        hh_opts.extend(HandHolding::ALL.into_iter().filter(|h| unlocked || *h != HandHolding::FullAccess).map(|h| (Some(h), h.label().to_string())));
        let hh_picker = picker(
            "proj-hh",
            None,
            prefs.hand_holding.map(|h| h.label().to_string()).unwrap_or_else(|| "Trek default".into()),
            hh_opts,
            Some(prefs.hand_holding),
            move |h: Option<HandHolding>, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p2, |pr| pr.hand_holding = h, cx)),
            cx,
        );

        // Where new threads run; worktrees need git.
        let w = self.workspace.clone();
        let p2 = path.clone();
        let run_in = ui::segmented(
            "proj-run-in",
            vec![(RunIn::Local, "Local"), (RunIn::Worktree, "New worktree")],
            prefs.run_in,
            move |r, _, cx| w.update(cx, |ws, cx| ws.update_project_prefs(&p2, |pr| pr.run_in = r, cx)),
            cx,
        );
        let mut new_thread_rows = vec![
            Self::row("Agent", "", agent_picker, cx),
            Self::row("Model", "", model_picker, cx),
            Self::row("Reasoning effort", "", effort_picker, cx),
            Self::row("Hand-holding", "", hh_picker, cx),
        ];
        if project.is_repo {
            new_thread_rows.push(Self::row(
                "Runs in",
                "The project folder, or a worktree of its own on a new branch, so threads don't edit each other's files. The composer can change it per thread.",
                run_in,
                cx,
            ));
            new_thread_rows.push(Self::row(
                "Copy into new worktrees",
                "Files a fresh checkout lacks because git ignores them, separated by commas. Ones git doesn't ignore aren't copied.",
                div().w(px(240.)).child(Input::new(&self.worktree_copy).small()),
                cx,
            ));
        }

        // Actions: named commands, run in a terminal tab from the title bar's Run menu.
        let mut action_rows: Vec<AnyElement> = prefs
            .actions
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (w, p2) = (self.workspace.clone(), path.clone());
                let (w2, cmd, root) = (self.workspace.clone(), a.command.clone(), path.clone());
                let controls = h_flex()
                    .gap(px(6.))
                    .child(Button::new(SharedString::from(format!("action-run-{i}"))).small().outline().icon(crate::assets::Lucide::Play).label("Run").on_click(move |_, _, cx| {
                        let (cmd, root) = (cmd.clone(), root.clone());
                        w2.update(cx, |ws, cx| ws.run_project_action(root, cmd, cx))
                    }))
                    .child(Button::new(SharedString::from(format!("action-del-{i}"))).small().ghost().label("Remove").on_click(move |_, _, cx| {
                        w.update(cx, |ws, cx| {
                            ws.update_project_prefs(
                                &p2,
                                |pr| {
                                    if i < pr.actions.len() {
                                        pr.actions.remove(i);
                                    }
                                },
                                cx,
                            )
                        })
                    }));
                Self::row(a.name.clone(), a.command.clone(), controls, cx)
            })
            .collect();
        let p2 = path.clone();
        action_rows.push(
            h_flex()
                .w_full()
                .py(px(12.))
                .gap(px(8.))
                .child(div().w(px(160.)).child(Input::new(&self.action_name).small()))
                .child(div().flex_1().child(Input::new(&self.action_command).small()))
                .child(Button::new("action-add").small().outline().icon(IconName::Plus).label("Add").on_click(cx.listener(move |this, _, window, cx| {
                    let name = this.action_name.read(cx).value().trim().to_string();
                    let command = this.action_command.read(cx).value().trim().to_string();
                    if name.is_empty() || command.is_empty() {
                        window.push_notification("Give the action a name and a command.", cx);
                        return;
                    }
                    let p = p2.clone();
                    this.workspace.update(cx, |ws, cx| ws.update_project_prefs(&p, |pr| pr.actions.push(ProjectAction { name, command }), cx));
                    this.action_name.update(cx, |s, cx| s.set_value("", window, cx));
                    this.action_command.update(cx, |s, cx| s.set_value("", window, cx));
                })))
                .into_any_element(),
        );

        // Danger.
        let view = cx.entity().downgrade();
        let name = project.name.clone();
        let remove = Button::new("project-remove").small().outline().icon(crate::assets::Lucide::Trash).label("Remove project").on_click(move |_, window, cx| {
            let (view, pid, name) = (view.clone(), pid.clone(), name.clone());
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (view, pid) = (view.clone(), pid.clone());
                alert
                    .title(format!("Remove “{name}” from Trek?"))
                    .description("Its threads are archived and the project leaves the sidebar. Files on disk and your agents' own history are not touched. Open the folder again to bring it back.")
                    .confirm()
                    .ok_text("Remove project")
                    .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        let _ = view.update(cx, |this, cx| this.workspace.update(cx, |ws, cx| ws.remove_project(&pid, cx)));
                        true
                    })
            });
        });

        vec![
            chooser,
            ui::group(
                vec![
                    Self::row("Name", "Shown in the sidebar and thread lists. The folder isn't renamed.", div().w(px(240.)).child(Input::new(&self.project_name).small()), cx),
                    Self::row(
                        "Icon",
                        match prefs.icon.as_deref() {
                            None => "Automatic: two letters from the name.",
                            Some(s) if s.strip_prefix("file:").is_some_and(|f| std::path::Path::new(f).exists()) => "Your image.",
                            Some(s) if s.starts_with("file:") => "Your image is gone, so the two letters stand in. Choose another.",
                            Some(_) => "One of Trek's icons.",
                        },
                        h_flex().gap(px(6.)).child(ui::project_badge(&project.name, &ui::ProjectLook::of(&prefs), cx)).child(div().w(px(4.))).children(icon_reset).child(icon_menu).child(icon_file),
                        cx,
                    ),
                    Self::row("Color", "Its badge, and its folders in answers, the Explorer and the composer.", colors, cx),
                    Self::row("Folder", trek_core::paths::tildify(&path), reveal, cx),
                ],
                cx,
            ),
            Self::heading("Verification", cx),
            Self::note("A skill that lets agents check their own work here: a small CLI to drive and debug the app, notes on setting up its dev environment, and a Feature Map of what it does and how to reach each part. Every agent working in this project is told to use it.", cx),
            ui::group(self.verification_rows(&project, cx), cx),
            Self::heading("New threads", cx),
            Self::note("What a new thread in this project starts with. Anything left on “Trek default” follows Settings → General and Permissions.", cx),
            ui::group(new_thread_rows, cx),
            Self::heading("Actions", cx),
            Self::note("Commands you run often in this project: tests, a build, a dev server. They appear in the Run menu in the title bar and open in a terminal tab.", cx),
            ui::group(action_rows, cx),
            Self::heading("Remove", cx),
            ui::group(
                vec![Self::row(
                    "Remove project",
                    format!("Archives {} and takes the project out of Trek. Nothing on disk is deleted.", if threads == 1 { "its 1 thread".to_string() } else { format!("its {threads} threads") }),
                    remove,
                    cx,
                )],
                cx,
            ),
        ]
    }

    /// The project's verification skill: set it up, or how it stands, with Maintain and the
    /// weekly reminder.
    fn verification_rows(&mut self, project: &trek_core::store::Project, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let ws = self.workspace.read(cx);
        let path = project.path.clone();
        let now = trek_core::store::now_ms();
        let run = ws.verification_run(&path).map(|(id, maintain)| (ws.thread(&id).map(|t| t.title.clone()).unwrap_or_default(), id, maintain));
        let open_run = |id: String, w: Entity<crate::workspace::Workspace>| {
            Button::new("verify-open-run").small().outline().label("Open thread").on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx)))
        };
        let w = self.workspace.clone();
        let Some(v) = ws.verification(&path) else {
            let (desc, control) = match run {
                Some((title, id, _)) => (format!("Being set up in “{title}”."), open_run(id, w).into_any_element()),
                None => (
                    "Not set up yet. Trek starts a thread with your default agent that builds one, following Trek's guide.".to_string(),
                    Button::new("verify-setup")
                        .small()
                        .outline()
                        .icon(crate::assets::Lucide::BadgeCheck)
                        .label("Set up verification")
                        .on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.start_verification(path.clone(), false, cx)))
                        .into_any_element(),
                ),
            };
            return vec![Self::row("Verification skill", desc, control, cx)];
        };
        let due = trek_core::verification::due(&v, now);
        let age = v.maintained_at.map(|at| format!("last maintained {}", crate::workspace::ago(at, now))).unwrap_or_else(|| "not maintained yet".into());
        let status = h_flex()
            .gap(px(7.))
            .child(div().size(px(6.)).rounded_full().bg(palette::emerald(cx)))
            .child("Ready")
            .child(div().font_normal().text_color(if due { palette::amber(cx) } else { muted }).child(format!("· {age}")));
        let skill_dir = std::path::PathBuf::from(&v.skill);
        let shown = skill_dir.strip_prefix(&path).map(|p| p.display().to_string()).unwrap_or_else(|_| trek_core::paths::tildify(&skill_dir));
        let maintain = match run {
            Some((_, id, _)) => open_run(id, w.clone()).label("Maintaining…").into_any_element(),
            None => {
                let (w, p) = (w.clone(), path.clone());
                Button::new("verify-maintain").small().outline().icon(IconName::Redo).label("Maintain").on_click(move |_, _, cx| w.update(cx, |ws, cx| ws.start_verification(p.clone(), true, cx))).into_any_element()
            }
        };
        let reveal = Button::new("verify-reveal").small().ghost().label("Show").on_click(move |_, _, cx| cx.reveal_path(&skill_dir.join("SKILL.md")));
        let mut rows = vec![Self::row(status, format!("“{}” in {shown}", v.name), h_flex().gap(px(6.)).child(reveal).child(maintain), cx)];
        if let Some(cli) = v.cli.clone() {
            // One in the skill's folder reads from there (`scripts/app`): the row above says where
            // that is. The whole command is in the tooltip, and copied.
            let short = cli.strip_prefix("./").and_then(|c| c.strip_prefix(&format!("{shown}/"))).unwrap_or(&cli).to_string();
            let (tip, copied) = (SharedString::from(cli.clone()), cli.clone());
            let chip = div()
                .id("verify-cli")
                .test_support()
                .min_w_0()
                .max_w(px(220.))
                .truncate()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(6.))
                .bg(theme.foreground.opacity(0.06))
                .font_family(theme.mono_font_family.clone())
                .text_size(px(12.))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .child(short);
            let copy = Button::new("verify-cli-copy").ghost().xsmall().icon(Icon::new(IconName::Copy).text_color(muted)).tooltip("Copy the command").on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                window.push_notification("Command copied", cx);
            });
            rows.push(Self::row(
                "Its CLI",
                "Agents run it from their working folder to drive and check the app. A turn that ran it is marked Verified.",
                h_flex().min_w_0().gap(px(2.)).child(chip).child(copy),
                cx,
            ));
        } else {
            // Without one, no turn can be marked Verified: say why, and what fixes it.
            rows.push(Self::row(
                "Its CLI",
                "None named, so Trek can't tell when a turn checked its work. Maintain adds one, named as `cli:` in the skill's metadata.",
                div().id("verify-no-cli").test_support().text_size(px(12.5)).text_color(muted).child("None"),
                cx,
            ));
        }
        let (w, p) = (self.workspace.clone(), path.clone());
        rows.push(Self::row(
            "Remind me weekly",
            "Once it's a week since it was last maintained, Trek reminds you, with a button that runs Maintain.",
            Switch::new("verify-remind").checked(v.remind_weekly).on_click(move |on: &bool, _, cx| {
                let on = *on;
                w.update(cx, |ws, cx| ws.set_verification_reminder(&p, on, cx))
            }),
            cx,
        ));
        rows
    }

    /// The name field follows the selected project (needs the window, so it runs from `render`).
    pub(super) fn sync_project_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        let projects = ws.workspace_projects();
        let selected = ws.settings_project.as_ref().and_then(|id| projects.iter().find(|p| &p.id == id)).or(projects.first()).map(|p| (p.id.clone(), p.name.clone(), p.path.clone()));
        let Some((id, name, path)) = selected else { return };
        if self.project_name_for.as_deref() != Some(id.as_str()) {
            let copy = self.workspace.read(cx).project_prefs(&path).worktree_copy.join(", ");
            self.project_name_for = Some(id);
            self.project_name.update(cx, |s, cx| s.set_value(name, window, cx));
            self.worktree_copy.update(cx, |s, cx| s.set_value(copy, window, cx));
        }
    }

    fn pick_project_image(&mut self, project: std::path::PathBuf, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use as Icon".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(src) = paths.into_iter().next() else { return };
            // Keep a copy in Trek's data folder so the icon survives the original moving.
            let dir = trek_core::paths::data_dir().join("project-icons");
            let _ = std::fs::create_dir_all(&dir);
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_string();
            let dest = dir.join(format!("{}.{ext}", uuid_like()));
            if std::fs::copy(&src, &dest).is_err() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let spec = format!("file:{}", dest.display());
                this.workspace.update(cx, |ws, cx| ws.update_project_prefs(&project, |pr| pr.icon = Some(spec), cx));
            });
        })
        .detach();
    }
}

fn uuid_like() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}
