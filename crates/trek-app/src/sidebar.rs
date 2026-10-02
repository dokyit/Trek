//! Inbox sidebar. Live work and anything waiting on you sit on top as quiet cards
//! (project · status, title, agent glyph); settled history folds into a footer grouped by project.

use crate::palette;
use crate::time;
use crate::ui;
use crate::workspace::{Route, SettingsPage, UpdateStatus, Workspace};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_core::store::{Section, Thread};
use trek_core::{RunState, ThreadSource};

pub struct Sidebar {
    workspace: Entity<Workspace>,
    search: Entity<InputState>,
    open_projects: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let subscriptions = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    this.workspace.update(cx, |ws, cx| {
                        ws.search = q;
                        cx.notify();
                    });
                }
            }),
        ];
        Self { workspace, search, open_projects: Default::default(), _subscriptions: subscriptions }
    }

    fn top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let draft = matches!(self.workspace.read(cx).route, Route::Draft { .. });
        v_flex()
            .px_2()
            .pt_1()
            .gap(px(2.))
            .child(
                h_flex()
                    .px_1()
                    .gap_1()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.search)
                                .small()
                                .appearance(false)
                                .prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                        ),
                    )
                    .child(ui::icon_button("open-folder", IconName::FolderOpen, "Open folder (⌘O)").on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| ws.open_folder(cx))
                    }))),
            )
            .child(
                ui::nav_row("new-thread", Icon::new(crate::assets::Lucide::SquarePen), "New thread", Some("⌘N"), draft, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx)))),
            )
    }

    fn status(&self, t: &Thread, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let ws = self.workspace.read(cx);
        match t.run_state {
            RunState::Working => {
                let elapsed = ws.live.get(&t.id).and_then(|l| l.turn_started).map(|s| time::elapsed(s.elapsed())).unwrap_or_default();
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(palette::sky(cx))
                    .child(Spinner::new().xsmall().color(palette::sky(cx)))
                    .child(format!("Working {elapsed}"))
                    .into_any_element()
            }
            RunState::NeedsYou => ui::status_text("Needs you", palette::amber(cx)),
            RunState::Failed => ui::status_text("Failed", palette::red(cx)),
            RunState::Idle if t.is_unseen() => h_flex()
                .gap_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(div().size(px(6.)).rounded_full().bg(palette::emerald(cx)))
                .child(time::relative(t.updated_at))
                .into_any_element(),
            RunState::Idle => div().text_xs().text_color(theme.muted_foreground).child(time::relative(t.updated_at)).into_any_element(),
        }
    }

    /// T3-style card: project · status on top, title below, agent glyph at the end.
    fn card(&self, t: &Thread, project: &str, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let quiet = t.run_state == RunState::Idle && !t.is_unseen() && !selected;
        let id = t.id.clone();
        let row = v_flex()
            .id(SharedString::from(format!("card-{}", t.id)))
            .mx_2()
            .px_3()
            .py(px(9.))
            .gap(px(5.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::monogram(project, cx))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(project.to_string()))
                    .child(self.status(t, cx)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(quiet, |el| el.text_color(theme.foreground.opacity(0.62)))
                            .when(t.is_unseen() && !selected, |el| el.font_medium())
                            .child(t.title.clone()),
                    )
                    .child(ui::agent_glyph(&t.agent, cx)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
            }));
        self.with_menu(row, t).into_any_element()
    }

    /// Codex-style compact row for settled history.
    fn line(&self, t: &Thread, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = t.id.clone();
        let row = h_flex()
            .id(SharedString::from(format!("line-{}", t.id)))
            .mx_2()
            .pl(px(30.))
            .pr_3()
            .h(px(30.))
            .gap_2()
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if selected { theme.foreground } else { theme.foreground.opacity(0.78) })
                    .child(t.title.clone()),
            )
            .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.8)).child(time::relative(t.updated_at)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let id = id.clone();
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx))
            }));
        self.with_menu(row, t).into_any_element()
    }

    fn with_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(&self, row: E, t: &Thread) -> impl IntoElement {
        let ws = self.workspace.downgrade();
        let tid = t.id.clone();
        let pinned = t.pinned_at.is_some();
        let settled = t.settled_at.is_some();
        let resume_cmd = match (t.source, &t.native_id) {
            (ThreadSource::ClaudeCode, Some(n)) => Some(format!("claude --resume {n}")),
            (ThreadSource::Codex, Some(n)) => Some(format!("codex resume {n}")),
            (ThreadSource::OpenCode, Some(n)) => Some(format!("opencode -s {n}")),
            _ => None,
        };
        let cwd = t.cwd.clone();
        row.context_menu(move |menu, _, _| {
            let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                let ws = ws.clone();
                let tid = tid.clone();
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                })
            };
            let mut menu = menu
                .item(item(if pinned { "Unpin" } else { "Pin" }, |ws, id, cx| ws.toggle_pin(id, cx)))
                .item(if settled { item("Move to Inbox", |ws, id, cx| ws.unsettle(id, cx)) } else { item("Settle", |ws, id, cx| ws.settle(id, cx)) })
                .item(item("Snooze 1 hour", |ws, id, cx| ws.snooze(id, 1, cx)))
                .item(item("Snooze until tomorrow", |ws, id, cx| ws.snooze(id, 16, cx)))
                .item(item("Mark as unread", |ws, id, cx| ws.mark_unread(id, cx)))
                .separator();
            if let Some(cmd) = resume_cmd.clone() {
                menu = menu.item(PopupMenuItem::new("Copy resume command").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(cmd.clone()));
                }));
            }
            if let Some(dir) = cwd.clone() {
                menu = menu.item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| cx.reveal_path(&dir)));
            }
            menu.separator().item(item("Archive", |ws, id, cx| ws.archive(id, cx)))
        })
    }

    fn label(text: &str, cx: &App) -> impl IntoElement {
        div().px_5().pt_3().pb_1().text_xs().text_color(cx.theme().muted_foreground).child(text.to_string())
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let in_settings = matches!(ws.route, Route::Settings(_));
        let pill = match &ws.update {
            UpdateStatus::Downloading { version, progress } => Some((format!("Downloading {version} · {:.0}%", progress * 100.), false)),
            UpdateStatus::Ready { version, .. } => Some((format!("Restart to update to {version}"), true)),
            UpdateStatus::RestartPending { .. } => Some(("Restarting when agents finish".to_string(), false)),
            _ => None,
        };
        let importing = ws.importing;
        let theme = cx.theme().clone();
        v_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .when_some(pill, |el, (label, ready)| {
                el.child(
                    h_flex()
                        .id("update-pill")
                        .mx_1()
                        .px_3()
                        .h(px(30.))
                        .gap_2()
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_xs()
                        .bg(if ready { palette::ember(cx).opacity(0.14) } else { theme.secondary })
                        .text_color(if ready { palette::ember(cx) } else { theme.muted_foreground })
                        .child(Icon::new(IconName::ArrowUp).xsmall())
                        .child(label)
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.restart_to_update(cx))))
                        .with_animation(
                            "update-pill-in",
                            Animation::new(std::time::Duration::from_millis(260)).with_easing(ease_out_quint()),
                            |el, t| el.opacity(t).mt(px(6. * (1. - t))),
                        ),
                )
            })
            .child(
                h_flex()
                    .px_1()
                    .gap_1()
                    .child(ui::icon_button("open-settings", IconName::Settings, "Settings (⌘,)").selected(in_settings).on_click(cx.listener(
                        |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx)),
                    )))
                    .child(ui::icon_button("rescan", IconName::RefreshCw, "Find threads from other agents").on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| {
                            ws.import_threads(cx);
                            ws.detect_agents(cx);
                        })
                    })))
                    .child(div().flex_1())
                    .when(importing, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground))),
            )
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let selected = match &ws.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        let searching = !ws.search.is_empty();
        let settled_open = ws.settled_open || searching;
        let importing = ws.importing;
        let sections: Vec<(Section, Vec<Thread>)> = ws.sections().into_iter().map(|(s, v)| (s, v.into_iter().cloned().collect())).collect();
        let names: HashMap<String, String> = ws.projects.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let project_of = |t: &Thread| t.project_id.as_ref().and_then(|p| names.get(p).cloned()).unwrap_or_else(|| "No project".into());
        let theme = cx.theme().clone();

        let mut list = v_flex().pt_1().pb_3();
        let mut settled: Vec<Thread> = vec![];
        let mut live_count = 0;
        for (section, threads) in sections {
            match section {
                Section::Settled => settled = threads,
                Section::Pinned | Section::Snoozed => {
                    list = list.child(Self::label(section.label(), cx));
                    for t in &threads {
                        list = list.child(self.card(t, &project_of(t), selected.as_deref() == Some(&t.id), cx));
                    }
                }
                Section::Inbox | Section::Working => {
                    live_count += threads.len();
                    for t in &threads {
                        list = list.child(self.card(t, &project_of(t), selected.as_deref() == Some(&t.id), cx));
                    }
                }
            }
        }
        if live_count == 0 && !searching {
            list = list.child(
                v_flex()
                    .mx_4()
                    .my_2()
                    .px_3()
                    .py_3()
                    .gap_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().text_sm().text_color(theme.foreground.opacity(0.8)).child(if importing { "Finding your threads…" } else { "Inbox zero" }))
                    .child("Running agents and anything that needs you will show up here."),
            );
        }

        // Settled history, grouped by project (Codex style).
        let mut history = v_flex();
        if !settled.is_empty() {
            let mut groups: Vec<(String, String, Vec<Thread>)> = Vec::new();
            for t in settled.iter().cloned() {
                let pid = t.project_id.clone().unwrap_or_default();
                match groups.iter_mut().find(|g| g.0 == pid) {
                    Some(g) => g.2.push(t),
                    None => {
                        let name = project_of(&t);
                        groups.push((pid, name, vec![t]));
                    }
                }
            }
            history = history.child(
                h_flex()
                    .id("settled-toggle")
                    .mx_2()
                    .px_3()
                    .h(px(30.))
                    .gap_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("Settled ({})", settled.len()))
                    .child(div().flex_1().h(px(1.)).bg(theme.border))
                    .child(Icon::new(if settled_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| {
                            ws.settled_open = !ws.settled_open;
                            cx.notify();
                        })
                    })),
            );
            if settled_open {
                for (pid, name, items) in groups {
                    let open = self.open_projects.contains(&pid);
                    let shown = if open || searching { items.len() } else { items.len().min(5) };
                    let pid2 = pid.clone();
                    history = history.child(
                        h_flex()
                            .mx_2()
                            .px_3()
                            .h(px(30.))
                            .mt_1()
                            .gap_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(IconName::Folder).small())
                            .child(div().truncate().child(name)),
                    );
                    for t in items.iter().take(shown) {
                        history = history.child(self.line(t, selected.as_deref() == Some(&t.id), cx));
                    }
                    if items.len() > 5 && !searching {
                        history = history.child(
                            div()
                                .id(SharedString::from(format!("more-{pid}")))
                                .mx_2()
                                .pl(px(30.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.foreground))
                                .child(if open { "Show less" } else { "Show more" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.open_projects.remove(&pid2) {
                                        this.open_projects.insert(pid2.clone());
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
            }
        }

        v_flex()
            .w(px(crate::root::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .when(self.workspace.read(cx).backdrop().is_none(), |el| el.bg(theme.sidebar))
            .child(self.top(cx))
            .child(div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().child(list).child(history))
            .child(self.footer(cx))
    }
}
