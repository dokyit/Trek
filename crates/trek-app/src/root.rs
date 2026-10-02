//! The main window: title bar, sidebar, and the routed content area.

use crate::composer::Composer;
use crate::onboarding::Onboarding;
use crate::panels::RightPanel;
use crate::settings_view::{SettingsNav, SettingsView};
use crate::sidebar::Sidebar;
use crate::thread_view::ThreadView;
use crate::workspace::{Route, Scope, SettingsPage, UndoAction, Workspace, WorkspaceEvent};
use crate::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 272.;

pub struct TrekWindow {
    workspace: Entity<Workspace>,
    sidebar: Entity<Sidebar>,
    thread_view: Entity<ThreadView>,
    composer: Entity<Composer>,
    settings: Entity<SettingsView>,
    settings_nav: Entity<SettingsNav>,
    right_panel: Entity<RightPanel>,
    onboarding: Entity<Onboarding>,
    /// Right-panel resize in progress: (pointer x at grab, width at grab).
    panel_drag: Option<(Pixels, f32)>,
    _subscriptions: Vec<Subscription>,
}

/// The chat column never gets narrower than this while the right panel is open.
const MIN_CHAT: f32 = 440.;

impl TrekWindow {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), Scope::Main, window, cx));
        let thread_view = cx.new(|cx| ThreadView::new(workspace.clone(), Scope::Main, window, cx));
        let handle = window.window_handle();
        workspace.update(cx, |ws, _| ws.main_window = Some(handle));
        let settings = cx.new(|cx| SettingsView::new(workspace.clone(), window, cx));
        let onboarding = cx.new(|cx| Onboarding::new(workspace.clone(), window, cx));
        let settings_nav = cx.new(|cx| SettingsNav::new(workspace.clone(), cx));
        let saved_width = workspace.read(cx).settings.layout.right_panel_width;
        let right_panel = cx.new(|_| {
            let mut p = RightPanel::new(workspace.clone());
            p.width = saved_width.max(crate::panels::MIN_PANEL);
            p
        });
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| {
                this.right_panel.update(cx, |p, cx| p.sync_native(cx));
                cx.notify();
            }),
            cx.subscribe_in(&workspace, window, |this, _, event: &WorkspaceEvent, window, cx| match event {
                WorkspaceEvent::Toast { message, undo } => this.toast(message.clone(), undo.clone(), window, cx),
                WorkspaceEvent::Attention { message, thread } => this.attention(message.clone(), thread, window, cx),
                WorkspaceEvent::FocusComposer => this.composer.update(cx, |c, cx| c.focus(window, cx)),
                WorkspaceEvent::OpenTool(tool) => {
                    let tool = *tool;
                    this.right_panel.update(cx, |p, cx| p.open_tool(tool, window, cx));
                }
                WorkspaceEvent::RunInTerminal(command) => {
                    let command = command.clone();
                    this.right_panel.update(cx, |p, cx| p.run_command(command, window, cx));
                }
                WorkspaceEvent::InsertIntoComposer(text) => this.composer.update(cx, |c, cx| c.insert_text(text, window, cx)),
                WorkspaceEvent::AttachImage(path) => {
                    let path = path.clone();
                    this.composer.update(cx, |c, cx| c.attach_image(path, cx));
                }
                WorkspaceEvent::RestoreQueued { thread, text, images } => {
                    // A thread window showing the thread takes them instead.
                    let ws = this.workspace.read(cx);
                    if ws.route == Route::Thread(thread.clone()) && !ws.thread_windows.contains_key(thread) {
                        this.composer.update(cx, |c, cx| c.restore(text, images, window, cx));
                    }
                }
                WorkspaceEvent::ActivateMain => window.activate_window(),
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.workspace.read(cx).settings.appearance.theme == trek_core::settings::ThemeChoice::System {
                    crate::set_theme(trek_core::settings::ThemeChoice::System, window, cx);
                }
            }),
        ];
        // TREK_OPEN_TOOL=browser (or terminal, explorer, git, side-chat) opens that tool at launch.
        if let Ok(name) = std::env::var("TREK_OPEN_TOOL") {
            let name = name.trim().to_lowercase();
            if let Some(tool) = crate::workspace::PanelTool::ALL.into_iter().find(|t| t.label().to_lowercase().replace(' ', "-") == name) {
                let panel = right_panel.clone();
                window.defer(cx, move |window, cx| panel.update(cx, |p, cx| p.open_tool(tool, window, cx)));
            }
        }
        Self { workspace, sidebar, thread_view, composer, settings, settings_nav, right_panel, onboarding, panel_drag: None, _subscriptions: subscriptions }
    }

    /// A thread needs the user or finished: toast, banner and sound per the notification settings.
    /// The main window decides for every window, so a sound or banner never doubles up.
    fn attention(&mut self, message: String, thread: &str, window: &mut Window, cx: &mut Context<Self>) {
        let active = cx.active_window();
        let ws = self.workspace.read(cx);
        let settings = ws.settings.notifications.clone();
        let alert = crate::system::alert_for(&settings, active.is_some(), ws.viewing(thread, active));
        if alert.sound {
            crate::system::play_alert_sound();
        }
        let front = active.filter(|a| *a != window.window_handle());
        if alert.banner && !(alert.toast && front.is_none()) {
            window.push_notification(Notification::new().message(message.clone()).system(), cx);
        }
        if alert.toast {
            let note = Notification::new().message(message);
            push_in_front(if alert.banner && front.is_none() { note.in_app_and_system() } else { note }, window, cx);
        }
    }

    fn toast(&mut self, message: String, undo: Option<UndoAction>, window: &mut Window, cx: &mut Context<Self>) {
        let mut note = Notification::new().message(message);
        if let Some(action) = undo {
            let ws = self.workspace.downgrade();
            note = note.action(move |_, _, _| {
                let ws = ws.clone();
                let action = action.clone();
                gpui_kit::component::button::Button::new("undo").label("Undo").small().on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.undo(action.clone(), cx));
                })
            })
            .autohide(true);
        }
        push_in_front(note, window, cx);
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let collapsed = ws.sidebar_collapsed;
        let theme = cx.theme().clone();
        let thread = ws.current_thread().cloned();
        let project_name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (project, title, folder): (Option<String>, String, Option<std::path::PathBuf>) = match &ws.route {
            Route::Thread(_) => match &thread {
                Some(t) => (t.cwd.as_deref().map(project_name), t.title.clone(), t.cwd.clone()),
                None => (None, "Trek".into(), None),
            },
            Route::Draft { project } => (project.as_deref().map(project_name), "New thread".into(), project.clone()),
            Route::Settings(_) => (None, "Settings".into(), None),
            Route::Onboarding => (None, String::new(), None),
        };
        let settle_id = thread.as_ref().filter(|t| t.settled_at.is_none()).map(|t| t.id.clone());
        // The project's icon and its actions (Settings → Project).
        let root = folder.as_deref().map(trek_core::store::project_root);
        let project_entry = root.as_ref().and_then(|r| ws.projects.iter().find(|p| &p.path == r));
        let project = project_entry.map(|p| p.name.clone()).or(project);
        let icon = root.as_ref().and_then(|r| ws.project_icon(r));
        let actions = root.as_ref().map(|r| ws.project_prefs(r).actions).unwrap_or_default();
        let project_id = project_entry.map(|p| p.id.clone());
        let transparent = self.workspace.read(cx).backdrop().is_some();
        TitleBar::new().when(transparent, |t| t.bg(gpui_kit::transparent_black())).child(
            h_flex()
                .w_full()
                .h_full()
                .items_center()
                .gap_2()
                .pr_2()
                .child(
                    h_flex()
                        .gap_2()
                        .when(!collapsed, |el| el.w(px(SIDEBAR_WIDTH - 80.)))
                        .flex_none()
                        .child(
                            crate::ui::icon_button("toggle-sidebar", IconName::PanelLeft, "Toggle sidebar (⌘B)").on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.sidebar_collapsed = !ws.sidebar_collapsed;
                                    cx.notify();
                                })
                            })),
                        )
                        .child(crate::brand::logo_mark(px(15.)))
                        .child(div().text_sm().font_semibold().text_color(theme.foreground.opacity(0.9)).child("Trek")),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .pl_2()
                        .gap_2()
                        .text_sm()
                        .when_some(project.clone(), |el, p| {
                            el.child(crate::ui::project_badge(&p, icon.as_deref(), cx))
                                .child(div().text_color(theme.muted_foreground).child(p))
                                .child(div().text_color(theme.muted_foreground.opacity(0.6)).child("/"))
                        })
                        .child(div().truncate().font_medium().child(title)),
                )
                .when(folder.is_some(), |el| el.child(run_button(actions, project_id, self.workspace.clone())))
                .when_some(folder, |el, dir| el.child(open_in_button(dir)))
                .when(thread.is_some(), |el| {
                    el.child(
                        crate::ui::icon_button("pop-out", crate::assets::Lucide::SquareArrowOutUpRight, "Open in new window (⇧⌘↩)")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenInNewWindow), cx)),
                    )
                })
                .child(
                    crate::ui::icon_button("toggle-tools", IconName::PanelRight, "Tools panel (⌘J)")
                        .on_click(cx.listener(|this, _, _, cx| this.right_panel.update(cx, |p, cx| p.toggle(cx)))),
                )
                .when_some(settle_id, |el, id| {
                    el.child(crate::ui::icon_button("settle", IconName::Check, "Settle (⌘E)").on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.workspace.update(cx, |ws, cx| ws.settle(&id, cx))
                    })))
                }),
        )
    }
}

/// Show an in-app notification in the frontmost Trek window (`window`, the main one, when Trek
/// isn't in front or it already is).
fn push_in_front(note: Notification, window: &mut Window, cx: &mut App) {
    let mut note = Some(note);
    if let Some(front) = cx.active_window().filter(|a| *a != window.window_handle()) {
        let _ = front.update(cx, |_, w, cx| {
            if let Some(n) = note.take() {
                w.push_notification(n, cx);
            }
        });
    }
    if let Some(n) = note {
        window.push_notification(n, cx);
    }
}

/// "Run" menu: the project's actions, each opening in a terminal tab.
fn run_button(actions: Vec<trek_core::settings::ProjectAction>, project_id: Option<String>, ws: Entity<Workspace>) -> impl IntoElement {
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    use gpui_kit::component::Sizable as _;
    Button::new("run-actions").ghost().small().icon(crate::assets::Lucide::Play).tooltip("Run a project action").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
        menu = menu.min_w(px(220.));
        if actions.is_empty() {
            menu = menu.label("No actions for this project yet");
        }
        for a in actions.clone() {
            let ws = ws.clone();
            menu = menu.item(PopupMenuItem::new(a.name.clone()).icon(crate::assets::Lucide::Play).on_click(move |_, _, cx| {
                let cmd = a.command.clone();
                ws.update(cx, |ws, cx| ws.run_project_action(cmd, cx))
            }));
        }
        let (ws, pid) = (ws.clone(), project_id.clone());
        menu.separator().item(PopupMenuItem::new(if actions.is_empty() { "Add an action…" } else { "Edit actions…" }).on_click(move |_, _, cx| {
            let pid = pid.clone();
            ws.update(cx, |ws, cx| ws.open_project_settings(pid, cx))
        }))
    })
}

/// "Open in" menu for the project folder: Finder, Terminal, and editors that are installed.
pub(crate) fn open_in_button(dir: std::path::PathBuf) -> impl IntoElement {
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let apps: Vec<(&'static str, &'static str)> = [
        ("Finder", "Finder"),
        ("Terminal", "Terminal"),
        ("Ghostty", "Ghostty"),
        ("Zed", "Zed"),
        ("Cursor", "Cursor"),
        ("VS Code", "Visual Studio Code"),
        ("Xcode", "Xcode"),
    ]
    .into_iter()
    .filter(|(_, app)| *app == "Finder" || std::path::Path::new(&format!("/Applications/{app}.app")).exists() || *app == "Terminal")
    .collect();
    gpui_kit::component::button::Button::new("open-in")
        .outline()
        .small()
        .icon(IconName::FolderOpen)
        .label("Open")
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let mut menu = menu.min_w(px(180.));
            for (label, app) in apps.clone() {
                let dir = dir.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, _| {
                    let _ = std::process::Command::new("/usr/bin/open").arg("-a").arg(app).arg(&dir).spawn();
                }));
            }
            menu
        })
}

impl TrekWindow {
    fn end_panel_drag(&mut self, cx: &mut Context<Self>) {
        if self.panel_drag.take().is_none() {
            return;
        }
        let w = self.right_panel.read(cx).width;
        self.right_panel.update(cx, |p, cx| p.set_width(w, cx));
        self.workspace.update(cx, |ws, cx| {
            ws.overlay_open = false;
            cx.notify();
        });
        cx.notify();
    }
}

impl Render for TrekWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let route = self.workspace.read(cx).route.clone();
        let collapsed = self.workspace.read(cx).sidebar_collapsed;
        if route == Route::Onboarding {
            return v_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(TitleBar::new())
                .child(self.onboarding.clone())
                .into_any_element();
        }
        let in_settings = matches!(route, Route::Settings(_));
        let backdrop = self.workspace.read(cx).backdrop();
        let (right_open, wanted_width) = {
            let p = self.right_panel.read(cx);
            (p.open, p.width)
        };
        // Whatever the stored width, leave the chat column at least MIN_CHAT wide.
        let side = if collapsed || in_settings { 8. } else { SIDEBAR_WIDTH };
        let max_width = (window.viewport_size().width.as_f32() - side - MIN_CHAT - 16.).max(crate::panels::MIN_PANEL);
        let right_width = wanted_width.clamp(crate::panels::MIN_PANEL, max_width);
        let dragging = self.panel_drag.is_some();
        let content = match route {
            Route::Settings(_) => self.settings.clone().into_any_element(),
            Route::Draft { .. } => div()
                .relative()
                .size_full()
                .child(div().absolute().top_0().left_0().size_full().child(self.thread_view.clone()))
                .child(
                    v_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .justify_center()
                        .pb(px(40.))
                        .child(self.composer.clone()),
                )
                .into_any_element(),
            _ => v_flex()
                .size_full()
                .min_w_0()
                .child(div().flex_1().min_h_0().child(self.thread_view.clone()))
                .child(self.composer.clone())
                .into_any_element(),
        };
        v_flex()
            .id("trek-window")
            // Panel resizing: follow the pointer anywhere in the window until the button is released.
            .when(dragging, |el| {
                el.cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                        let Some((x0, w0)) = this.panel_drag else { return };
                        let w = (w0 + (x0 - e.position.x).as_f32()).clamp(crate::panels::MIN_PANEL, max_width);
                        this.right_panel.update(cx, |p, cx| {
                            p.width = w;
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_panel_drag(cx)))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_panel_drag(cx)))
            })
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &NewThread, _, cx| this.workspace.update(cx, |ws, cx| ws.new_thread(cx))))
            .on_action(cx.listener(|this, _: &crate::TakeSnapshot, _, cx| this.composer.update(cx, |c, cx| c.snapshot_default(cx))))
            .on_action(cx.listener(|this, _: &OpenFolder, _, cx| this.workspace.update(cx, |ws, cx| ws.open_folder(cx))))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx))
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::About), cx))
            }))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    ws.check_for_updates(true, cx);
                    ws.navigate(Route::Settings(SettingsPage::Updates), cx);
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    ws.sidebar_collapsed = !ws.sidebar_collapsed;
                    cx.notify();
                })
            }))
            .on_action(cx.listener(|this, _: &SettleThread, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    if let Route::Thread(id) = ws.route.clone() {
                        ws.settle(&id, cx);
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &Interrupt, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    if let Route::Thread(id) = ws.route.clone() {
                        ws.interrupt(&id, cx);
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &CycleHandHolding, _, cx| this.workspace.update(cx, |ws, cx| ws.cycle_hand_holding(&Scope::Main, cx))))
            .on_action(cx.listener(|this, _: &OpenInNewWindow, _, cx| {
                if let Route::Thread(id) = this.workspace.read(cx).route.clone() {
                    crate::thread_window::open(this.workspace.clone(), &id, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &ToggleRightPanel, _, cx| this.right_panel.update(cx, |p, cx| p.toggle(cx))))
            .bg(cx.theme().sidebar)
            .when_some(backdrop.clone(), |el, (spec, dim)| {
                let side = cx.theme().sidebar;
                el.relative()
                    .child(img(crate::ui::background_source(&spec)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover))
                    .child(div().absolute().top_0().left_0().size_full().bg(side.opacity((0.5 + dim * 0.6).min(0.92))))
            })
            .child(self.title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(!collapsed && !in_settings, |el| el.child(self.sidebar.clone()))
                    .when(in_settings, |el| el.child(self.settings_nav.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .pr_2()
                            .pb_2()
                            .when(collapsed, |el| el.pl_2())
                            .child(
                                div()
                                    .size_full()
                                    .rounded(px(12.))
                                    .border_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .bg(cx.theme().background)
                                    .overflow_hidden()
                                    .child(content),
                            ),
                    )
                    .when(right_open && !in_settings, |el| {
                        let theme = cx.theme().clone();
                        // Grab strip on the panel's left edge: drag to resize, double-click to reset.
                        let handle = div()
                            .id("panel-resize")
                            .absolute()
                            .top_0()
                            .bottom(px(8.))
                            .left(px(-9.))
                            .w(px(13.))
                            .flex()
                            .justify_center()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .group("panel-resize")
                            .child(
                                div()
                                    .w(px(2.))
                                    .h_full()
                                    .rounded_full()
                                    .when(dragging, |el| el.bg(theme.foreground.opacity(0.35)))
                                    .when(!dragging, |el| el.group_hover("panel-resize", |s| s.bg(theme.foreground.opacity(0.2)))),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                    if e.click_count >= 2 {
                                        this.right_panel.update(cx, |p, cx| p.set_width(trek_core::settings::DEFAULT_RIGHT_PANEL_WIDTH, cx));
                                        return;
                                    }
                                    this.panel_drag = Some((e.position.x, right_width));
                                    // Native views (browser, simulator) would swallow the drag.
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.overlay_open = true;
                                        cx.notify();
                                    });
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            );
                        el.child(
                            div().relative().w(px(right_width)).flex_none().h_full().pr_2().pb_2().child(handle).child(
                                div()
                                    .size_full()
                                    .rounded(px(12.))
                                    .border_1()
                                    .border_color(cx.theme().sidebar_border)
                                    .bg(cx.theme().background)
                                    .overflow_hidden()
                                    .child(self.right_panel.clone()),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
