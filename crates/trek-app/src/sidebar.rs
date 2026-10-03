//! Inbox sidebar. Live work and anything waiting on you sit on top as quiet cards
//! (project · status, title, agent glyph); settled history folds into a footer grouped by project.

use crate::palette;
use crate::time;
use crate::ui;
use crate::workspace::{ItemRef, PanelTool, Route, SettingsPage, UpdateAction, UpdateStatus, Workspace, WorkspaceEvent};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use trek_core::store::{SearchHit, Section, Thread};
use trek_core::{RunState, ThreadSource};

pub struct Sidebar {
    workspace: Entity<Workspace>,
    search: Entity<InputState>,
    project_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    rename_input: Entity<InputState>,
    /// The thread being renamed while the dialog is open.
    renaming: Option<String>,
    /// The window is frontmost; spinners hold still when it isn't.
    active: bool,
    open_projects: HashSet<String>,
    filter_open: bool,
    usage_open: bool,
    updater_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let project_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects…"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Thread title"));
        let subscriptions = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    this.workspace.update(cx, |ws, cx| ws.set_search(q, cx));
                }
            }),
        ];
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.observe(&project_search, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe_in(&rename_input, window, |this: &mut Self, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::PressEnter { .. }) {
                return;
            }
            if let Some(id) = this.renaming.take() {
                let title = input.read(cx).value().to_string();
                this.workspace.update(cx, |ws, cx| ws.rename(&id, title, cx));
                window.close_dialog(cx);
            }
        }));
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            this.active = window.is_window_active();
            cx.notify();
        }));
        Self {
            workspace,
            search,
            project_search,
            clone_input,
            rename_input,
            renaming: None,
            active: window.is_window_active(),
            open_projects: Default::default(),
            filter_open: false,
            usage_open: false,
            updater_open: false,
            _subscriptions: subscriptions,
        }
    }

    fn top(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let filtering = self.workspace.read(cx).project_filter.is_some();
        let filter_open = self.filter_open;
        let this = cx.entity();
        let filter = Popover::new("project-filter")
            .anchor(Anchor::TopLeft)
            .appearance(false)
            .open(filter_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.filter_open = *open;
                cx.notify();
            }))
            .trigger(ui::icon_button("filter-projects", IconName::Folder, "Filter threads by project").selected(filtering))
            .content(move |_, _, cx| this.update(cx, |this, cx| this.project_menu(cx)));
        let add = ui::icon_button("add-project", crate::assets::Lucide::FolderPlus, "Add project").dropdown_menu_with_anchor(Anchor::TopLeft, {
            let ws = self.workspace.clone();
            let sidebar = cx.entity();
            move |menu, _, _| {
                let ws = ws.clone();
                let sb = sidebar.clone();
                menu.min_w(px(200.))
                    .item(PopupMenuItem::new("Open folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))))
                    .item(PopupMenuItem::new("Clone from GitHub…").icon(IconName::Github).on_click(move |_, window, cx| sb.update(cx, |s, cx| s.open_clone_dialog(window, cx))))
            }
        });
        h_flex()
            .px_3()
            .pt_1()
            .pb_2()
            .gap(px(2.))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                ),
            )
            .child(filter)
            .child(add)
            .child(ui::icon_button("new-thread", crate::assets::Lucide::SquarePen, "New thread (⌘N)").on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.new_thread(cx))
            })))
    }

    fn project_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let q = self.project_search.read(cx).value().to_lowercase();
        let current = ws.project_filter.clone();
        let projects: Vec<(String, String, Option<String>, std::path::PathBuf)> = ws
            .workspace_projects()
            .into_iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.remote.as_deref().is_some_and(|r| r.to_lowercase().contains(&q)))
            .map(|p| (p.id.clone(), p.name.clone(), p.remote.clone(), p.path.clone()))
            .collect();
        fn pick(id: Option<String>, cx: &mut Context<Sidebar>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
            cx.listener(move |this: &mut Sidebar, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.workspace.update(cx, |ws, cx| {
                    ws.project_filter = id;
                    cx.notify();
                });
                this.filter_open = false;
                cx.notify();
            })
        }
        ui::menu_surface(cx)
            .w(px(280.))
            .child(div().px(px(6.)).pb(px(4.)).child(Input::new(&self.project_search).small().prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground))))
            .child(
                ui::menu_row("pf-all", current.is_none(), cx)
                    .child(Icon::new(IconName::Folder).small().text_color(theme.muted_foreground))
                    .child("All projects")
                    .on_click(pick(None, cx)),
            )
            .child(
                v_flex().id("pf-list").max_h(px(320.)).overflow_y_scroll().children(projects.into_iter().map(|(id, name, remote, path)| {
                    let label = remote.clone().unwrap_or_else(|| name.clone());
                    let ws = self.workspace.clone();
                    let gear_id = id.clone();
                    ui::menu_row(SharedString::from(format!("pf-{id}")), current.as_ref() == Some(&id), cx)
                        .group("pf-row")
                        .child(ui::project_badge(&name, self.workspace.read(cx).project_icon(&path).as_deref(), cx))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            gpui_kit::component::button::Button::new(SharedString::from(format!("pf-gear-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Settings).text_color(theme.muted_foreground))
                                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                    let (p1, p2, ws2, ws3, id3) = (path.clone(), path.clone(), ws.clone(), ws.clone(), gear_id.clone());
                                    menu.item(PopupMenuItem::new("New thread here").on_click(move |_, _, cx| {
                                        let p = p2.clone();
                                        ws2.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(p) }, cx))
                                    }))
                                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&p1)))
                                    .item(PopupMenuItem::new("Project settings").on_click(move |_, _, cx| {
                                        let id = id3.clone();
                                        ws3.update(cx, |ws, cx| ws.open_project_settings(Some(id), cx))
                                    }))
                                }),
                        )
                        .on_click(pick(Some(id.clone()), cx))
                })),
            )
            .into_any_element()
    }

    fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.clone_input.clone();
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let input2 = input.clone();
            let ws = ws.clone();
            dialog
                .title("Clone from GitHub")
                .child(v_flex().gap_2().child("Uses your GitHub CLI login. Clones into ~/Developer.").child(Input::new(&input)))
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(gpui_kit::component::button::Button::new("cancel-clone").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("do-clone").primary().label("Clone").on_click(move |_, _, cx| {
                                let spec = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.clone_repo(spec, cx));
                            }),
                        )),
                )
        });
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
                    .child(Icon::new(crate::assets::Lucide::LoaderCircle).xsmall().text_color(palette::sky(cx)))
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

    /// While searching, the message that put a thread in the list when its title didn't match.
    fn content_hit(&self, t: &Thread, cx: &App) -> Option<SearchHit> {
        let ws = self.workspace.read(cx);
        let q = ws.search.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        ws.search_results.content_hit(&q, t).cloned()
    }

    /// Open a thread from the list; a thread found by its messages opens at the match.
    fn open(&mut self, id: String, hit: Option<SearchHit>, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| match hit.as_ref().and_then(ItemRef::of_hit) {
            Some(at) => ws.open_thread_at(&id, at, cx),
            None => ws.navigate(Route::Thread(id), cx),
        })
    }

    /// The matching words of a content hit, on one quiet line.
    fn hit_line(hit: &SearchHit, cx: &App) -> Div {
        let (text, ranges) = ui::lead_to_match(&hit.snippet, &hit.ranges, 12);
        div().min_w_0().truncate().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(ui::match_text(&text, &ranges, cx))
    }

    /// T3-style card: project · status on top, title below, agent glyph at the end.
    fn card(&self, t: &Thread, project: &str, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let quiet = t.run_state == RunState::Idle && !t.is_unseen() && !selected;
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        let row = v_flex()
            .id(SharedString::from(format!("card-{}", t.id)))
            .mx_2()
            .mb(px(2.))
            .px(px(12.))
            .py(px(10.))
            .gap(px(6.))
            .rounded(px(12.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::project_badge(project, self.workspace.read(cx).thread_project_icon(t).as_deref(), cx))
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
                            .text_size(px(14.))
                            .when(quiet, |el| el.text_color(theme.foreground.opacity(0.62)))
                            .when(t.is_unseen() && !selected, |el| el.font_medium())
                            .child(t.title.clone()),
                    )
                    .child(ui::agent_glyph(&t.agent, cx)),
            )
            .when_some(hit.clone(), |el, h| el.child(Self::hit_line(&h, cx).mt(px(-2.))))
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        self.with_menu(row, t, cx).into_any_element()
    }

    /// Codex-style compact row for settled history.
    fn line(&self, t: &Thread, selected: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let id = t.id.clone();
        let hit = self.content_hit(t, cx);
        let title = h_flex()
            .h(px(30.))
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if selected { theme.foreground } else { theme.foreground.opacity(0.78) })
                    .child(t.title.clone()),
            )
            .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.8)).child(time::relative(t.updated_at)));
        let row = v_flex()
            .id(SharedString::from(format!("line-{}", t.id)))
            .mx_2()
            .pl(px(30.))
            .pr_3()
            .rounded(px(8.))
            .cursor_pointer()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(title)
            .when_some(hit.clone(), |el, h| el.child(Self::hit_line(&h, cx).mt(px(-6.)).pb(px(6.))))
            .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), hit.clone(), cx)));
        self.with_menu(row, t, cx).into_any_element()
    }

    /// Right-click menu for a thread (T3's set): pin, settle, snooze, rename, copy, project, archive, delete.
    fn with_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(&self, row: E, t: &Thread, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.downgrade();
        let sidebar = cx.entity().downgrade();
        let tid = t.id.clone();
        let title = t.title.clone();
        let pinned = t.pinned_at.is_some();
        let settled = t.settled_at.is_some();
        let snoozed = t.snoozed_until.is_some_and(|u| u > trek_core::store::now_ms());
        let never_settle = t.never_settle;
        let imported = t.source != ThreadSource::Trek;
        let resume_cmd = match (&t.agent, &t.native_id) {
            (trek_core::AgentId::ClaudeCode, Some(n)) => Some(format!("claude --resume {n}")),
            (trek_core::AgentId::Codex, Some(n)) => Some(format!("codex resume {n}")),
            (trek_core::AgentId::OpenCode, Some(n)) => Some(format!("opencode -s {n}")),
            _ => None,
        };
        let cwd = t.cwd.clone();
        let project = t.project_id.clone().and_then(|pid| self.workspace.read(cx).project(&pid).map(|p| (p.id.clone(), p.name.clone())));
        row.context_menu(move |menu, window, cx| {
            let item = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                let ws = ws.clone();
                let tid = tid.clone();
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                })
            };
            let copy = |label: &'static str, text: String| PopupMenuItem::new(label).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
            let mut menu = menu
                .min_w(px(210.))
                .item(PopupMenuItem::new("Open in new window").icon(crate::assets::Lucide::SquareArrowOutUpRight).on_click({
                    let (ws, tid) = (ws.clone(), tid.clone());
                    move |_, _, cx| {
                        if let Some(ws) = ws.upgrade() {
                            crate::thread_window::open(ws, &tid, cx);
                        }
                    }
                }))
                .separator()
                .item(item(if pinned { "Unpin thread" } else { "Pin thread" }, |ws, id, cx| ws.toggle_pin(id, cx)))
                .item(if settled { item("Move to inbox", |ws, id, cx| ws.unsettle(id, cx)) } else { item("Settle thread", |ws, id, cx| ws.settle(id, cx)) });
            menu = menu.submenu("Snooze", window, cx, {
                let (ws, tid) = (ws.clone(), tid.clone());
                move |menu, _, _| {
                    let snooze = |label: &'static str, f: fn(&mut Workspace, &str, &mut Context<Workspace>)| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = ws.update(cx, |ws, cx| f(ws, &tid, cx));
                        })
                    };
                    let menu = menu
                        .item(snooze("For 1 hour", |ws, id, cx| ws.snooze(id, 1, cx)))
                        .item(snooze("For 3 hours", |ws, id, cx| ws.snooze(id, 3, cx)))
                        .item(snooze("Until tomorrow morning", |ws, id, cx| ws.snooze_until_morning(id, 1, cx)))
                        .item(snooze("Until next week", |ws, id, cx| ws.snooze_until_morning(id, 7, cx)));
                    if snoozed { menu.separator().item(snooze("Wake now", |ws, id, cx| ws.unsnooze(id, cx))) } else { menu }
                }
            });
            menu = menu.separator();
            menu = menu.item(PopupMenuItem::new("Rename thread").on_click({
                let (sidebar, tid, title) = (sidebar.clone(), tid.clone(), title.clone());
                move |_, window, cx| {
                    let _ = sidebar.update(cx, |s, cx| s.open_rename_dialog(tid.clone(), title.clone(), window, cx));
                }
            }));
            menu = menu.item(item("Regenerate title", |ws, id, cx| ws.regenerate_title(id, true, cx))).item(item("Mark unread", |ws, id, cx| ws.mark_unread(id, cx)));
            if let Some((pid, name)) = project.clone() {
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new(format!("Filter by {name}")).on_click(move |_, _, cx| {
                    let pid = pid.clone();
                    let _ = ws.update(cx, |ws, cx| {
                        ws.project_filter = Some(pid);
                        cx.notify();
                    });
                }));
            }
            menu = menu.submenu("Auto-settle behavior", window, cx, {
                let (ws, tid) = (ws.clone(), tid.clone());
                move |menu, _, _| {
                    let set = |label: &'static str, never: bool| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        PopupMenuItem::new(label).checked(never_settle == never).on_click(move |_, _, cx| {
                            let _ = ws.update(cx, |ws, cx| ws.set_never_settle(&tid, never, cx));
                        })
                    };
                    menu.item(set("Follow Trek's setting", false)).item(set("Never settle this thread", true))
                }
            });
            menu = menu.separator();
            menu = menu.submenu("Copy", window, cx, {
                let (ws, tid, title, resume_cmd, cwd) = (ws.clone(), tid.clone(), title.clone(), resume_cmd.clone(), cwd.clone());
                move |menu, _, _| {
                    let mut menu = menu.item(copy("Title", title.clone()));
                    menu = menu.item(PopupMenuItem::new("Conversation as Markdown").on_click({
                        let (ws, tid) = (ws.clone(), tid.clone());
                        move |_, _, cx| {
                            if let Ok(text) = ws.update(cx, |ws, _| ws.transcript_markdown(&tid)) {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                        }
                    }));
                    if let Some(cmd) = resume_cmd.clone() {
                        menu = menu.item(copy("Resume command", cmd));
                    }
                    if let Some(dir) = cwd.clone() {
                        menu = menu.item(copy("Folder path", dir.display().to_string()));
                    }
                    menu
                }
            });
            if let Some(dir) = cwd.clone() {
                menu = menu.item(PopupMenuItem::new("Show folder in Finder").on_click(move |_, _, cx| cx.reveal_path(&dir)));
            }
            if let Some((pid, _)) = project.clone() {
                let ws = ws.clone();
                menu = menu.item(PopupMenuItem::new("Project settings").on_click(move |_, _, cx| {
                    let pid = pid.clone();
                    let _ = ws.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
                }));
            }
            menu.separator().item(item("Archive thread", |ws, id, cx| ws.archive(id, cx))).item(PopupMenuItem::new("Delete…").icon(crate::assets::Lucide::Trash).on_click({
                let (ws, tid, title) = (ws.clone(), tid.clone(), title.clone());
                move |_, window, cx| {
                    let (ws, tid, title) = (ws.clone(), tid.clone(), title.clone());
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let (ws, tid) = (ws.clone(), tid.clone());
                        alert
                            .title(format!("Delete “{title}”?"))
                            .description(if imported {
                                "It leaves Trek for good. The original stays in the agent's own history."
                            } else {
                                "The thread and its transcript are deleted from Trek. This can't be undone."
                            })
                            .confirm()
                            .ok_text("Delete")
                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                            .on_ok(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.delete_thread(&tid, cx));
                                true
                            })
                    });
                }
            }))
        })
    }

    /// Right-click menu for a project (settled-history headers and the project filter).
    fn with_project_menu<E: InteractiveElement + ParentElement + Styled + IntoElement + 'static>(&self, row: E, id: String, name: String, path: std::path::PathBuf) -> impl IntoElement {
        let ws = self.workspace.downgrade();
        row.context_menu(move |menu, window, _| {
            let _ = &window;
            let act = |label: &'static str, f: fn(&mut Workspace, &str, &std::path::Path, &mut Context<Workspace>)| {
                let (ws, id, path) = (ws.clone(), id.clone(), path.clone());
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| f(ws, &id, &path, cx));
                })
            };
            let (p1, p2) = (path.clone(), path.clone());
            let (ws2, id2, name2) = (ws.clone(), id.clone(), name.clone());
            menu.min_w(px(200.))
                .item(act("New thread here", |ws, _, path, cx| ws.navigate(Route::Draft { project: Some(path.to_path_buf()) }, cx)))
                .item(act("Show only this project", |ws, id, _, cx| {
                    ws.project_filter = Some(id.to_string());
                    cx.notify();
                }))
                .separator()
                .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&p1)))
                .item(PopupMenuItem::new("Copy path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(p2.display().to_string()))))
                .item(act("Project settings", |ws, id, _, cx| ws.open_project_settings(Some(id.to_string()), cx)))
                .separator()
                .item(PopupMenuItem::new("Remove project…").icon(crate::assets::Lucide::Trash).on_click(move |_, window, cx| {
                    let (ws, id, name) = (ws2.clone(), id2.clone(), name2.clone());
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let (ws, id) = (ws.clone(), id.clone());
                        alert
                            .title(format!("Remove “{name}” from Trek?"))
                            .description("Its threads are archived and it leaves the sidebar. Files on disk and your agents' own history are not touched.")
                            .confirm()
                            .ok_text("Remove project")
                            .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                            .on_ok(move |_, _, cx| {
                                let _ = ws.update(cx, |ws, cx| ws.remove_project(&id, cx));
                                true
                            })
                    });
                }))
        })
    }

    fn open_rename_dialog(&mut self, id: String, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.rename_input.clone();
        input.update(cx, |s, cx| s.set_value(title, window, cx));
        self.renaming = Some(id.clone());
        // Put the cursor in the field once the dialog is up, with the old title selected.
        let focus = input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| {
                s.focus(window, cx);
                let len = s.value().len();
                s.set_selected_range(0..len, cx);
            })
        });
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let (input2, ws, id) = (input.clone(), ws.clone(), id.clone());
            dialog
                .title("Rename thread")
                .w(px(440.))
                .child(Input::new(&input))
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(gpui_kit::component::button::Button::new("cancel-rename").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("do-rename").primary().label("Rename").on_click(move |_, _, cx| {
                                let title = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.rename(&id, title, cx));
                            }),
                        )),
                )
        });
    }

    fn label(text: &str, cx: &App) -> impl IntoElement {
        div().px_5().pt_3().pb_1().text_xs().text_color(cx.theme().muted_foreground).child(text.to_string())
    }

    fn footer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let in_settings = matches!(ws.route, Route::Settings(_));
        let update = ws.updater.status.clone();
        let theme = cx.theme().clone();
        let importing = ws.importing;
        let (usage_open, updater_open) = (self.usage_open, self.updater_open);
        let this = cx.entity();
        let usage = Popover::new("usage-popover")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(usage_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.usage_open = *open;
                if *open {
                    this.workspace.update(cx, |ws, cx| ws.refresh_usage(cx));
                }
                cx.notify();
            }))
            .trigger({
                let peak = self.workspace.read(cx).agent_status.values().flat_map(|s| s.limits.iter().map(|l| l.percent)).fold(0.0f32, f32::max);
                let tint = if peak >= 95. { Some(palette::red(cx)) } else if peak >= 80. { Some(palette::amber(cx)) } else { None };
                let icon = Icon::new(crate::assets::Lucide::ChartNoAxesColumn);
                ui::icon_button("usage", match tint { Some(c) => icon.text_color(c), None => icon }, "Usage").selected(usage_open)
            })
            .content({
                let this = this.clone();
                move |_, _, cx| this.update(cx, |this, cx| this.usage_card(cx))
            });
        let busy = matches!(update, UpdateStatus::Checking | UpdateStatus::Downloading { .. });
        let ready = matches!(update, UpdateStatus::Ready { .. } | UpdateStatus::RestartPending { .. });
        // Found but not downloaded (automatic downloads off): say so, in neutral; ember is for
        // the one-click "restart into it".
        let available = matches!(update, UpdateStatus::Available { .. });
        let label_color = if ready { palette::ember(cx) } else { theme.foreground };
        let updater = Popover::new("updater-popover")
            .anchor(Anchor::BottomRight)
            .appearance(false)
            .open(updater_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.updater_open = *open;
                cx.notify();
            }))
            .trigger(
                ui::Pill::new("updater")
                    .ghost(!ready && !available)
                    .selected(updater_open)
                    .child(if busy {
                        Spinner::new().xsmall().color(theme.muted_foreground).into_any_element()
                    } else {
                        let color = if ready || available { label_color } else { theme.muted_foreground };
                        Icon::new(IconName::RefreshCw).small().text_color(color).into_any_element()
                    })
                    .when(ready || available, |el| el.child(div().text_xs().text_color(label_color).child("Update"))),
            )
            .content(move |_, _, cx| this.update(cx, |this, cx| this.updater_card(cx)));
        h_flex()
            .px_3()
            .py_2()
            .gap_1()
            .child(ui::icon_button("open-settings", IconName::Settings, "Settings (⌘,)").selected(in_settings).on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx))
            })))
            .child(ui::icon_button("open-git", crate::assets::Lucide::GitCompare, "Source control").on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(PanelTool::Git)))
            })))
            .child(usage)
            .child(div().flex_1())
            .when(importing, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground)))
            .child(updater)
    }

    /// Plan usage per agent: 5-hour, weekly and per-model windows with reset times.
    fn usage_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let loading = ws.usage_loading;
        let mut rows: Vec<(trek_core::AgentId, trek_agents::AgentStatus)> = ws
            .ready_agents()
            .into_iter()
            .filter_map(|a| ws.agent_status.get(&a.key()).cloned().map(|u| (a, u)))
            .collect();
        rows.sort_by_key(|(a, _)| a.key());
        let bar = |pct: f32, cx: &App| {
            let color = if pct >= 90. { palette::red(cx) } else if pct >= 70. { palette::amber(cx) } else { cx.theme().foreground.opacity(0.85) };
            div().h(px(5.)).w_full().rounded_full().bg(cx.theme().foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(color).w(relative((pct / 100.).clamp(0.0, 1.0))))
        };
        ui::menu_surface(cx)
            .w(px(320.))
            .p(px(14.))
            .gap(px(14.))
            .child(
                h_flex()
                    .child(div().flex_1().text_sm().font_semibold().child("Usage"))
                    .when(loading, |el| el.child(Spinner::new().xsmall().color(theme.muted_foreground))),
            )
            .when(rows.is_empty() && !loading, |el| el.child(div().text_sm().text_color(theme.muted_foreground).child("No plan usage reported by your agents.")))
            .children(rows.into_iter().map(|(agent, u)| {
                v_flex()
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(ui::agent_logo(&agent, px(16.), cx))
                            .child(div().text_sm().font_medium().child(agent.display_name()))
                            .child(div().flex_1())
                            .when_some(u.plan.clone(), |el, p| el.child(div().text_xs().text_color(theme.muted_foreground).child(p))),
                    )
                    .when(u.limits.is_empty() && u.error.is_none(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("No usage limits on this plan.")))
                    .when_some(u.error.clone(), |el, e| el.child(div().text_xs().text_color(palette::amber(cx)).child(e)))
                    .children(u.limits.iter().map(|l| {
                        let resets = l.resets_at.map(time::until).unwrap_or_default();
                        v_flex()
                            .gap(px(5.))
                            .child(
                                h_flex()
                                    .text_xs()
                                    .child(div().flex_1().child(l.label.clone()))
                                    .child(div().text_color(theme.muted_foreground).child(format!("{:.0}%", l.percent))),
                            )
                            .child(bar(l.percent, cx))
                            .when(!resets.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("Resets {resets}"))))
                    }))
            }))
            .into_any_element()
    }

    fn updater_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let view = ws.update_view();
        let notes = ws.update_notes().map(|u| (u.version.to_string(), u.notes.clone()));
        let channel = format!("{:?}", ws.settings.updates.channel);
        ui::menu_surface(cx)
            .w(px(320.))
            .p(px(14.))
            .gap(px(10.))
            .child(h_flex().gap_2().child(crate::brand::logo_mark(px(16.))).child(div().text_sm().font_semibold().child(format!("Trek {}", trek_core::VERSION))).child(div().flex_1()).child(div().text_xs().text_color(theme.muted_foreground).child(channel)))
            .child(div().text_sm().text_color(theme.muted_foreground).child(view.line))
            .when_some(view.progress, |el, p| {
                el.child(div().h(px(5.)).w_full().rounded_full().bg(theme.foreground.opacity(0.08)).child(div().h_full().rounded_full().bg(palette::ember(cx)).w(relative(p))))
            })
            .when_some(notes, |el, (version, notes)| {
                el.child(
                    v_flex()
                        .gap(px(6.))
                        .pt(px(10.))
                        .border_t_1()
                        .border_color(theme.foreground.opacity(0.07))
                        .child(div().text_xs().font_medium().child(format!("What's new in {version}")))
                        .child(ui::release_notes("updater-notes", notes, px(200.), cx)),
                )
            })
            .when_some(view.action, |el, action| {
                el.child(
                    gpui_kit::component::button::Button::new("updater-action")
                        .small()
                        .w_full()
                        .when(action == UpdateAction::Restart, |b| b.primary())
                        .when(action != UpdateAction::Restart, |b| b.outline())
                        .loading(view.busy)
                        .label(action.label())
                        .on_click(cx.listener(move |this, _, _, cx| this.workspace.update(cx, |ws, cx| ws.run_update_action(action, cx)))),
                )
            })
            .into_any_element()
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
        // Settled history can be hundreds of threads; while it's folded only the count is needed.
        let mut settled_count = 0;
        let sections: Vec<(Section, Vec<Thread>)> = ws
            .sections()
            .into_iter()
            .map(|(s, v)| {
                if s == Section::Settled {
                    settled_count = v.len();
                    if !settled_open {
                        return (s, vec![]);
                    }
                }
                (s, v.into_iter().cloned().collect())
            })
            .collect();
        let names: HashMap<String, String> = ws.projects.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
        let paths: HashMap<String, std::path::PathBuf> = ws.projects.iter().map(|p| (p.id.clone(), p.path.clone())).collect();
        let icons: HashMap<String, Option<String>> = ws.projects.iter().map(|p| (p.id.clone(), ws.project_icon(&p.path))).collect();
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
        if settled_count > 0 {
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
                    .child(format!("Settled ({settled_count})"))
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
                    let header = h_flex()
                        .id(SharedString::from(format!("proj-head-{pid}")))
                        .mx_2()
                        .px_3()
                        .h(px(30.))
                        .mt_1()
                        .gap_2()
                        .rounded(px(8.))
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(ui::project_badge(&name, icons.get(&pid).cloned().flatten().as_deref(), cx))
                        .child(div().truncate().child(name.clone()));
                    history = match paths.get(&pid) {
                        Some(path) => history.child(self.with_project_menu(header, pid.clone(), name.clone(), path.clone())),
                        None => history.child(header),
                    };
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
