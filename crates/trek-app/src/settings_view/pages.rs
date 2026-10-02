//! The hand-built settings pages: General, Appearance, Notifications, Shortcuts, API keys,
//! Permissions, Updates and About. The rest live in settings_view.rs.

use super::SettingsView;
use crate::palette;
use crate::ui;
use crate::workspace::{Route, UpdateStatus};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::catalog::DIRECT_PROVIDERS;
use trek_core::settings::{Channel, FollowUp, NotifyMode, Settings, ThemeChoice, secrets};
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
                ],
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
        let ui_size = match s.appearance.ui_font_size {
            v if v < 12.75 => 0u8,
            v if v < 13.75 => 1,
            _ => 2,
        };
        let text_size = match s.appearance.transcript_font_size {
            v if v < 13.5 => 0u8,
            v if v < 14.75 => 1,
            v if v < 16.25 => 2,
            _ => 3,
        };
        vec![
            self.theme_tiles(s.appearance.theme, cx),
            Self::heading("Text", cx),
            ui::group(
                vec![
                    Self::row(
                        "Interface",
                        "Sidebar, menus and settings.",
                        ui::segmented(
                            "ui-size",
                            vec![(0u8, "Small"), (1, "Default"), (2, "Large")],
                            ui_size,
                            self.setter(|s, v: u8| s.appearance.ui_font_size = [12.0, 13.0, 14.0][v as usize]),
                            cx,
                        ),
                        cx,
                    ),
                    Self::row(
                        "Conversation",
                        "Messages from you and your agents.",
                        ui::segmented(
                            "text-size",
                            vec![(0u8, "Small"), (1, "Default"), (2, "Large"), (3, "Larger")],
                            text_size,
                            self.setter(|s, v: u8| s.appearance.transcript_font_size = [13.0, 14.0, 15.5, 17.0][v as usize]),
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
                        "Skip alerts for the thread you're already looking at.",
                        self.switch("notify-unfocused", n.only_when_unfocused, |s, v| s.notifications.only_when_unfocused = v),
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
            ("Threads", &[("New thread", &["⌘", "N"]), ("Open a folder", &["⌘", "O"]), ("Settle the current thread", &["⌘", "E"]), ("Stop the agent", &["⌘", "."])]),
            (
                "Composer",
                &[("Send", &["↩"]), ("New line", &["⇧", "↩"]), ("Plan mode", &["⇧", "⇥"]), ("Cycle hand-holding", &["⌘", "⇧", "A"]), ("Commands", &["/"]), ("Mention a file", &["@"]), ("Use a skill", &["$"])],
            ),
            ("Window", &[("Toggle the sidebar", &["⌘", "B"]), ("Toggle the tools panel", &["⌘", "J"]), ("Settings", &["⌘", ","]), ("Hide Trek", &["⌘", "H"]), ("Minimize", &["⌘", "M"]), ("Quit", &["⌘", "Q"])]),
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
        let status = self.workspace.read(cx).update.clone();
        let (line, busy) = match &status {
            UpdateStatus::Idle => ("You're on the latest version you've checked for.".to_string(), false),
            UpdateStatus::Checking => ("Checking for updates…".into(), true),
            UpdateStatus::UpToDate => ("Trek is up to date.".into(), false),
            UpdateStatus::Available { version, .. } => (format!("Trek {version} is available."), false),
            UpdateStatus::Downloading { version, progress } => (format!("Downloading Trek {version} · {:.0}%", progress * 100.), true),
            UpdateStatus::Ready { version, .. } => (format!("Trek {version} is ready. Restart to finish."), false),
            UpdateStatus::RestartPending { .. } => ("Restarting as soon as your agents finish.".into(), true),
            UpdateStatus::Failed(e) => (format!("Couldn't check: {e}"), false),
        };
        let ready = matches!(status, UpdateStatus::Ready { .. });
        let action = if ready {
            Button::new("restart-update").small().primary().label("Restart to update").on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.restart_to_update(cx))))
        } else {
            Button::new("check-now")
                .small()
                .outline()
                .loading(busy)
                .label("Check now")
                .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.check_for_updates(true, cx))))
        };
        let header = h_flex()
            .gap(px(14.))
            .pb(px(24.))
            .child(crate::brand::logo_mark(px(36.)))
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(div().text_size(px(15.)).font_semibold().child(format!("Trek {}", trek_core::VERSION)))
                    .child(div().text_size(px(12.5)).text_color(theme.muted_foreground).child(line)),
            )
            .child(action)
            .into_any_element();
        vec![
            header,
            ui::group(
                vec![
                    Self::row(
                        "Channel",
                        "Beta and Nightly get new things first, and occasionally rough edges.",
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
                        "Installs when you next restart, never while an agent is working.",
                        self.switch("auto-dl", s.updates.auto_download, |s, v| s.updates.auto_download = v),
                        cx,
                    ),
                ],
                cx,
            ),
        ]
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
