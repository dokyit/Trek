//! Settings. A nav column replaces the thread sidebar (as in Codex); pages are grouped cards.

use crate::brand;
use crate::palette;
use crate::ui;
use crate::workspace::{Route, SettingsPage, UpdateStatus, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use trek_core::catalog::DIRECT_PROVIDERS;
use trek_core::detect::Availability;
use trek_core::settings::{BackgroundPlacement, Channel, FollowUp, Settings, ThemeChoice, secrets};
use trek_core::{AgentId, HandHolding};

fn page_icon(p: SettingsPage) -> Icon {
    match p {
        SettingsPage::General => Icon::new(IconName::Settings2),
        SettingsPage::Appearance => Icon::new(IconName::Palette),
        SettingsPage::Agents => Icon::new(IconName::Bot),
        SettingsPage::ApiKeys => Icon::new(crate::assets::Lucide::Lock),
        SettingsPage::LocalModels => Icon::new(IconName::Cpu),
        SettingsPage::Permissions => Icon::new(crate::assets::Lucide::ShieldCheck),
        SettingsPage::Inbox => Icon::new(IconName::Inbox),
        SettingsPage::Import => Icon::new(IconName::ArrowDown),
        SettingsPage::Updates => Icon::new(IconName::RefreshCw),
        SettingsPage::About => Icon::new(IconName::Info),
    }
}

const NAV_GROUPS: &[(&str, &[SettingsPage])] = &[
    ("App", &[SettingsPage::General, SettingsPage::Appearance]),
    ("Models", &[SettingsPage::Agents, SettingsPage::ApiKeys, SettingsPage::LocalModels]),
    ("Workflow", &[SettingsPage::Permissions, SettingsPage::Inbox, SettingsPage::Import]),
    ("Trek", &[SettingsPage::Updates, SettingsPage::About]),
];

fn page_blurb(p: SettingsPage) -> &'static str {
    match p {
        SettingsPage::General => "Defaults for new threads and how Trek behaves while agents work.",
        SettingsPage::Appearance => "Theme, background art and motion.",
        SettingsPage::Agents => "Coding agents found on this Mac. Trek uses your existing logins.",
        SettingsPage::ApiKeys => "Use a provider directly with your own API key.",
        SettingsPage::LocalModels => "Models running on this Mac.",
        SettingsPage::Permissions => "How much each agent may do without asking.",
        SettingsPage::Inbox => "When finished threads leave the inbox.",
        SettingsPage::Import => "Bring in threads from other agents on this Mac.",
        SettingsPage::Updates => "Trek updates itself in the background.",
        SettingsPage::About => "",
    }
}

/// The settings navigation column, shown in place of the thread sidebar.
pub struct SettingsNav {
    workspace: Entity<Workspace>,
    _subscription: Subscription,
}

impl SettingsNav {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self { workspace, _subscription: sub }
    }
}

impl Render for SettingsNav {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = match self.workspace.read(cx).route {
            Route::Settings(p) => p,
            _ => SettingsPage::General,
        };
        let theme = cx.theme().clone();
        v_flex()
            .w(px(crate::root::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .px_2()
            .pt_1()
            .gap(px(2.))
            .when(self.workspace.read(cx).backdrop().is_none(), |el| el.bg(theme.sidebar))
            .child(
                h_flex()
                    .id("settings-back")
                    .mx_1()
                    .px_2()
                    .h(px(32.))
                    .gap_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.list_hover).text_color(theme.foreground))
                    .child(Icon::new(IconName::ArrowLeft).small())
                    .child("Back to app")
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx)))),
            )
            .children(NAV_GROUPS.iter().map(|(group, pages)| {
                v_flex()
                    .pt(px(16.))
                    .gap(px(2.))
                    .child(div().px(px(12.)).pb(px(4.)).text_xs().text_color(theme.muted_foreground).child(*group))
                    .children(pages.iter().map(|&p| {
                        let label: &'static str = p.label();
                        ui::nav_row(label, page_icon(p), label, None, p == current, cx)
                            .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(p), cx))))
                    }))
            }))
    }
}

pub struct SettingsView {
    workspace: Entity<Workspace>,
    key_inputs: HashMap<&'static str, Entity<InputState>>,
    saved_keys: HashMap<&'static str, bool>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut key_inputs = HashMap::new();
        let mut saved_keys = HashMap::new();
        for p in DIRECT_PROVIDERS.iter().filter(|p| !p.local) {
            key_inputs.insert(p.id, cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Paste key")));
            saved_keys.insert(p.id, secrets::api_key(p.id).is_some());
        }
        let subs = vec![cx.observe(&workspace, |_, _, cx| cx.notify())];
        Self { workspace, key_inputs, saved_keys, _subscriptions: subs }
    }

    fn page(&self, cx: &App) -> SettingsPage {
        match self.workspace.read(cx).route {
            Route::Settings(p) => p,
            _ => SettingsPage::General,
        }
    }

    /// Closure that edits settings and saves.
    fn setter<T: 'static>(&self, f: fn(&mut Settings, T)) -> impl Fn(T, &mut Window, &mut App) + Clone + 'static {
        let ws = self.workspace.clone();
        move |v: T, _: &mut Window, cx: &mut App| {
            ws.update(cx, |ws, cx| {
                f(&mut ws.settings, v);
                ws.save_settings(cx);
            })
        }
    }

    fn row(title: impl IntoElement, description: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> AnyElement {
        let description: SharedString = description.into();
        h_flex()
            .w_full()
            .min_h(px(56.))
            .gap(px(24.))
            .py(px(12.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(px(14.)).font_medium().child(title))
                    .when(!description.is_empty(), |el| {
                        el.child(div().text_size(px(12.5)).line_height(relative(1.45)).text_color(cx.theme().muted_foreground).child(description))
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    fn switch(&self, id: &'static str, on: bool, f: fn(&mut Settings, bool)) -> Switch {
        let set = self.setter(f);
        Switch::new(id).checked(on).on_click(move |v: &bool, window, cx| set(*v, window, cx))
    }

    /// Section heading: more space above than below (rhythm), real weight instead of an eyebrow.
    fn heading(text: &str, cx: &App) -> AnyElement {
        div().pt(px(32.)).pb(px(8.)).px(px(4.)).text_size(px(14.)).font_semibold().text_color(cx.theme().foreground).child(text.to_string()).into_any_element()
    }

    fn note(text: &str, cx: &App) -> AnyElement {
        div().pb(px(12.)).px(px(4.)).text_size(px(13.)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground).child(text.to_string()).into_any_element()
    }

    fn status_dot(color: Hsla, label: &'static str) -> AnyElement {
        h_flex().gap_2().text_xs().child(div().size(px(7.)).rounded_full().bg(color)).child(label).into_any_element()
    }

    /// Thumbnails: None, the built-in art, the user's own image, and "Choose image…".
    fn background_gallery(&self, s: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = s.appearance.background.clone();
        let mut options: Vec<(Option<String>, &'static str)> =
            vec![(None, "None"), (Some("builtin:dawn".into()), "Dawn"), (Some("builtin:night".into()), "Night"), (Some("builtin:paper".into()), "Paper")];
        if let Some(c) = current.clone().filter(|c| !c.starts_with("builtin:")) {
            options.push((Some(c), "Yours"));
        }
        let tile = |id: SharedString, selected: bool| {
            v_flex().id(id).gap(px(6.)).cursor_pointer().child(
                div()
                    .w(px(100.))
                    .h(px(66.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .border_2()
                    .border_color(if selected { theme.foreground } else { theme.border })
                    .border_dashed()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Plus).text_color(theme.muted_foreground)),
            )
        };
        h_flex()
            .flex_wrap()
            .gap(px(12.))
            .px(px(4.))
            .children(options.into_iter().map(|(spec, label)| {
                let selected = spec == current;
                let preview = spec.clone();
                let ws = self.workspace.clone();
                v_flex()
                    .id(SharedString::from(format!("bg-{label}")))
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(
                        div()
                            .relative()
                            .w(px(100.))
                            .h(px(66.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_2()
                            .border_color(if selected { theme.foreground } else { theme.border })
                            .bg(theme.background)
                            .when_some(preview, |el, spec| el.child(img(ui::background_source(&spec)).size_full().object_fit(ObjectFit::Cover))),
                    )
                    .child(div().text_xs().text_color(if selected { theme.foreground } else { theme.muted_foreground }).child(label))
                    .on_click(move |_, _, cx| {
                        let spec = spec.clone();
                        ws.update(cx, |ws, cx| {
                            ws.settings.appearance.background = spec;
                            ws.save_settings(cx);
                        })
                    })
            }))
            .child(
                tile("bg-choose".into(), false)
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Choose image…"))
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.pick_background_image(cx)))),
            )
            .into_any_element()
    }

    fn content(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let page = self.page(cx);
        let s = self.workspace.read(cx).settings.clone();
        let muted = cx.theme().muted_foreground;
        let mut out: Vec<AnyElement> = vec![];
        match page {
            SettingsPage::General => {
                let agents = self.workspace.read(cx).ready_agents();
                let current = AgentId::from_key(&s.general.default_agent);
                let ws = self.workspace.clone();
                let options: Vec<(usize, String)> = agents.iter().enumerate().map(|(i, a)| (i, a.display_name())).collect();
                let cur_ix = agents.iter().position(|a| *a == current).unwrap_or(0);
                let agents2 = agents.clone();
                let pick_agent = move |i: usize, _: &mut Window, cx: &mut App| {
                    if let Some(a) = agents2.get(i).cloned() {
                        ws.update(cx, |ws, cx| {
                            ws.settings.general.default_agent = a.key();
                            ws.draft_prefs.agent = a;
                            ws.draft_prefs.model = None;
                            ws.save_settings(cx);
                        });
                    }
                };
                out.push(ui::group(
                    vec![
                        Self::row(
                            "Default agent",
                            "Used for new threads. Every installed agent stays one click away in the composer.",
                            ui::segmented("def-agent", options, cur_ix, pick_agent, cx),
                            cx,
                        ),
                        Self::row(
                            "While an agent is working, ↩",
                            "Steer injects your message at the next step. Queue sends it when the turn ends.",
                            ui::segmented(
                                "follow-up",
                                vec![(FollowUp::Steer, "Steers"), (FollowUp::Queue, "Queues")],
                                s.general.follow_up,
                                self.setter(|s, v| s.general.follow_up = v),
                                cx,
                            ),
                            cx,
                        ),
                        Self::row(
                            "Keep the Mac awake while agents run",
                            "",
                            self.switch("prevent-sleep", s.general.prevent_sleep_while_running, |s, v| s.general.prevent_sleep_while_running = v),
                            cx,
                        ),
                    ],
                    cx,
                ));
            }
            SettingsPage::Appearance => {
                let set = self.setter(|s, v| s.appearance.theme = v);
                let pick = move |v: ThemeChoice, window: &mut Window, cx: &mut App| {
                    set(v, window, cx);
                    crate::set_theme(v, window, cx);
                };
                out.push(ui::group(
                    vec![Self::row(
                        "Theme",
                        "System follows macOS light and dark.",
                        ui::segmented("theme", vec![(ThemeChoice::System, "System"), (ThemeChoice::Night, "Night"), (ThemeChoice::Paper, "Paper")], s.appearance.theme, pick, cx),
                        cx,
                    )],
                    cx,
                ));
                out.push(Self::heading("Background", cx));
                out.push(self.background_gallery(&s, cx));
                out.push(div().h(px(12.)).into_any_element());
                out.push(ui::group(
                    vec![
                        Self::row(
                            "Show on",
                            "New thread puts the art behind the composer. Everywhere tints the whole window.",
                            ui::segmented(
                                "bg-place",
                                vec![(BackgroundPlacement::NewThread, "New thread"), (BackgroundPlacement::Everywhere, "Everywhere")],
                                s.appearance.background_placement,
                                self.setter(|s, v| s.appearance.background_placement = v),
                                cx,
                            ),
                            cx,
                        ),
                        Self::row(
                            "Dim",
                            "Darkens the image so text stays readable.",
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
                ));
                out.push(Self::heading("Motion", cx));
                out.push(ui::group(
                    vec![Self::row(
                        "Reduce motion",
                        "Use simple fades instead of movement.",
                        self.switch("reduce-motion", s.appearance.reduce_motion, |s, v| s.appearance.reduce_motion = v),
                        cx,
                    )],
                    cx,
                ));
            }
            SettingsPage::Agents => {
                let ws = self.workspace.read(cx);
                let detecting = ws.detecting;
                let agents: Vec<_> = ws.agents.iter().filter(|a| !matches!(a.agent, AgentId::Direct(_))).cloned().collect();
                out.push(Self::note(
                    "Trek runs each vendor's own agent with the login you already have, so your subscriptions just work. Trek never reads or stores those credentials.",
                    cx,
                ));
                let mut rows = vec![];
                for a in agents {
                    let status = match a.availability {
                        Availability::Ready => Self::status_dot(palette::emerald(cx), "Ready"),
                        Availability::NeedsLogin => Self::status_dot(palette::amber(cx), "Sign in needed"),
                        Availability::NotInstalled => Self::status_dot(muted.opacity(0.5), "Not installed"),
                        Availability::Offline => Self::status_dot(muted.opacity(0.5), "Offline"),
                    };
                    let wired = matches!(a.agent, AgentId::ClaudeCode | AgentId::Codex);
                    let detail = match (&a.version, &a.install_hint) {
                        (Some(v), _) if wired => v.clone(),
                        (Some(v), _) => format!("{v} · coming with ACP support"),
                        (None, Some(h)) => h.clone(),
                        _ => String::new(),
                    };
                    let title = h_flex().gap_2().child(ui::agent_glyph(&a.agent, cx)).child(a.name.clone());
                    rows.push(Self::row(title, detail, status, cx));
                }
                out.push(ui::group(rows, cx));
                out.push(
                    h_flex()
                        .pt_3()
                        .child(
                            Button::new("rescan")
                                .small()
                                .outline()
                                .loading(detecting)
                                .icon(IconName::RefreshCw)
                                .label("Scan again")
                                .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.detect_agents(cx)))),
                        )
                        .into_any_element(),
                );
            }
            SettingsPage::ApiKeys => {
                out.push(Self::note("Keys are stored in your macOS Keychain. Keys already in your shell environment are picked up automatically.", cx));
                let mut rows = vec![];
                for p in DIRECT_PROVIDERS.iter().filter(|p| !p.local) {
                    let Some(input) = self.key_inputs.get(p.id).cloned() else { continue };
                    let saved = self.saved_keys.get(p.id).copied().unwrap_or(false);
                    let id = p.id;
                    let control = h_flex()
                        .gap_1()
                        .w(px(320.))
                        .child(div().flex_1().child(Input::new(&input).small()))
                        .child(Button::new(SharedString::from(format!("save-{id}"))).small().outline().label("Save").on_click(cx.listener(
                            move |this, _, window, cx| {
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
                                        window.push_notification("Saved to Keychain", cx);
                                    }
                                    Err(e) => window.push_notification(format!("Couldn't save key: {e}"), cx),
                                }
                            },
                        )))
                        .when(saved, |el| {
                            el.child(ui::icon_button(SharedString::from(format!("del-{id}")), IconName::Delete, "Remove key").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let _ = secrets::delete_api_key(id);
                                    this.saved_keys.insert(id, false);
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.settings.api_providers.retain(|p| p != id);
                                        ws.save_settings(cx);
                                    });
                                },
                            )))
                        });
                    rows.push(Self::row(p.name, if saved { "Saved in Keychain" } else { "" }, control, cx));
                }
                out.push(ui::group(rows, cx));
            }
            SettingsPage::LocalModels => {
                let ws = self.workspace.read(cx);
                let locals: Vec<_> = ws.agents.iter().filter(|a| matches!(a.agent, AgentId::Direct(_))).cloned().collect();
                out.push(Self::note("Trek finds model servers on this Mac automatically: Ollama, LM Studio, and llama.cpp or MLX servers.", cx));
                let rows = locals
                    .into_iter()
                    .map(|a| {
                        let (status, detail) = match a.availability {
                            Availability::Ready if a.models.is_empty() => {
                                (Self::status_dot(palette::amber(cx), "No models"), "Running. Pull a model, e.g. ollama pull qwen3-coder".to_string())
                            }
                            Availability::Ready => (Self::status_dot(palette::emerald(cx), "Ready"), a.models.join(", ")),
                            _ => (Self::status_dot(muted.opacity(0.5), "Not running"), String::new()),
                        };
                        Self::row(a.name.clone(), detail, status, cx)
                    })
                    .collect();
                out.push(ui::group(rows, cx));
            }
            SettingsPage::Permissions => {
                let view = cx.entity().downgrade();
                let unlocked = s.permissions.full_access_unlocked;
                let full_switch = Switch::new("full-access").checked(unlocked).on_click(move |v: &bool, window, cx| {
                    let view = view.clone();
                    if !*v {
                        let _ = view.update(cx, |this, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.settings.permissions.full_access_unlocked = false;
                                ws.save_settings(cx);
                            })
                        });
                        return;
                    }
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let view = view.clone();
                        alert
                            .title("Allow Full access?")
                            .description("Agents will run commands and edit files anywhere without asking. Only use it for work you'd trust to run unattended.")
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
                out.push(ui::group(
                    vec![
                        Self::row(
                            "Default hand-holding",
                            "How much the agent checks with you. Change it per thread from the composer.",
                            ui::segmented(
                                "default-hh",
                                HandHolding::ALL.iter().filter(|h| unlocked || **h != HandHolding::FullAccess).map(|h| (*h, h.label())).collect(),
                                s.general.hand_holding,
                                self.setter(|s, v| s.general.hand_holding = v),
                                cx,
                            ),
                            cx,
                        ),
                        Self::row("Allow Full access", "Adds the no-prompts, no-sandbox level to the composer.", full_switch, cx),
                    ],
                    cx,
                ));
                out.push(Self::heading("How each level maps to each agent", cx));
                out.push(ui::group(
                    HandHolding::ALL
                        .into_iter()
                        .map(|h| {
                            let (sb, ap, rv) = h.codex_policy();
                            Self::row(
                                h.label(),
                                format!("{} Claude Code: {} · Codex: {sb}, {ap}, reviewer {rv}", h.description(), h.claude_mode()),
                                div().size(px(8.)).rounded_full().bg(palette::hand_holding(h, cx)),
                                cx,
                            )
                        })
                        .collect(),
                    cx,
                ));
            }
            SettingsPage::Inbox => {
                out.push(ui::group(
                    vec![Self::row(
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
                    )],
                    cx,
                ));
            }
            SettingsPage::Import => {
                let ws = self.workspace.read(cx);
                let importing = ws.importing;
                let summary = ws.import_summary.clone();
                out.push(Self::note("Trek reads, and never changes, the threads other agents keep on this Mac, so you can browse and continue them here.", cx));
                out.push(ui::group(
                    vec![
                        Self::row("Claude Code", "~/.claude/projects", self.switch("imp-claude", s.import.claude_code, |s, v| s.import.claude_code = v), cx),
                        Self::row("Codex", "~/.codex", self.switch("imp-codex", s.import.codex, |s, v| s.import.codex = v), cx),
                        Self::row("OpenCode", "~/.local/share/opencode", self.switch("imp-opencode", s.import.opencode, |s, v| s.import.opencode = v), cx),
                        Self::row(
                            "How far back",
                            "",
                            ui::segmented(
                                "max-age",
                                vec![(30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "All")],
                                s.import.max_age_days,
                                self.setter(|s, v| s.import.max_age_days = v),
                                cx,
                            ),
                            cx,
                        ),
                    ],
                    cx,
                ));
                out.push(
                    h_flex()
                        .pt_3()
                        .gap_3()
                        .child(
                            Button::new("import-now")
                                .small()
                                .outline()
                                .loading(importing)
                                .label("Import now")
                                .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.import_threads(cx)))),
                        )
                        .when_some(summary, |el, s| {
                            el.child(div().text_sm().text_color(muted).child(format!(
                                "{} Claude Code · {} Codex · {} OpenCode",
                                s.claude_code, s.codex, s.opencode
                            )))
                        })
                        .into_any_element(),
                );
            }
            SettingsPage::Updates => {
                let status = self.workspace.read(cx).update.clone();
                let status_text = match &status {
                    UpdateStatus::Idle => format!("Version {}", trek_core::VERSION),
                    UpdateStatus::Checking => "Checking…".into(),
                    UpdateStatus::UpToDate => format!("Version {} is the latest", trek_core::VERSION),
                    UpdateStatus::Available { version, .. } => format!("Version {version} is available"),
                    UpdateStatus::Downloading { version, progress } => format!("Downloading {version} · {:.0}%", progress * 100.),
                    UpdateStatus::Ready { version, .. } => format!("Version {version} is ready"),
                    UpdateStatus::RestartPending { .. } => "Restarting when your agents finish".into(),
                    UpdateStatus::Failed(e) => format!("Couldn't check: {e}"),
                };
                let ready = matches!(status, UpdateStatus::Ready { .. });
                let action = if ready {
                    Button::new("restart-update").small().primary().label("Restart to update").on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| ws.restart_to_update(cx))
                    }))
                } else {
                    Button::new("check-now")
                        .small()
                        .outline()
                        .loading(matches!(status, UpdateStatus::Checking))
                        .label("Check now")
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.check_for_updates(true, cx))))
                };
                out.push(ui::group(
                    vec![
                        Self::row("Trek", status_text, action, cx),
                        Self::row(
                            "Channel",
                            "Beta and Nightly get new things first.",
                            ui::segmented(
                                "channel",
                                vec![(Channel::Stable, "Stable"), (Channel::Beta, "Beta"), (Channel::Nightly, "Nightly")],
                                s.updates.channel,
                                self.setter(|s, v| s.updates.channel = v),
                                cx,
                            ),
                            cx,
                        ),
                        Self::row("Check automatically", "", self.switch("auto-check", s.updates.auto_check, |s, v| s.updates.auto_check = v), cx),
                        Self::row(
                            "Download in the background",
                            "Updates install when you restart, never while an agent is working.",
                            self.switch("auto-dl", s.updates.auto_download, |s, v| s.updates.auto_download = v),
                            cx,
                        ),
                    ],
                    cx,
                ));
            }
            SettingsPage::About => {
                out.push(
                    v_flex()
                        .items_center()
                        .gap_3()
                        .py_10()
                        .child(brand::trail_draw("about-trail", px(120.), s.appearance.reduce_motion))
                        .child(div().text_size(px(26.)).font_semibold().child("Trek"))
                        .child(div().text_color(muted).child("Every agent. One trail."))
                        .child(div().text_sm().text_color(muted).child(format!("Version {}", trek_core::VERSION)))
                        .into_any_element(),
                );
            }
        }
        out
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page(cx);
        let body = self.content(cx);
        div().id("settings-content").size_full().overflow_y_scroll().child(
            h_flex().w_full().justify_center().child(
                v_flex()
                    .w_full()
                    .max_w(px(680.))
                    .px(px(40.))
                    .pt(px(40.))
                    .pb(px(64.))
                    .child(div().px(px(4.)).text_size(px(22.)).font_semibold().child(page.label()))
                    .when(!page_blurb(page).is_empty(), |el| {
                        el.child(div().px(px(4.)).pt(px(4.)).text_size(px(13.)).text_color(cx.theme().muted_foreground).child(page_blurb(page)))
                    })
                    .child(div().h(px(24.)))
                    .children(body),
            ),
        )
    }
}
