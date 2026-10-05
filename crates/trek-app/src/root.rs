//! The main window: title bar, sidebar, and the routed content area.

use crate::basecamp::Basecamp;
use crate::command_palette::CommandPalette;
use crate::composer::Composer;
use crate::onboarding::Onboarding;
use crate::panels::RightPanel;
use crate::settings_view::{SettingsNav, SettingsView};
use crate::sidebar::Sidebar;
use crate::thread_view::ThreadView;
use crate::working_bar::WorkingBar;
use crate::workspace::{Route, Scope, SettingsPage, UndoAction, Workspace, WorkspaceEvent};
use crate::*;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 272.;

pub struct TrekWindow {
    workspace: Entity<Workspace>,
    pub(crate) sidebar: Entity<Sidebar>,
    pub(crate) thread_view: Entity<ThreadView>,
    pub(crate) composer: Entity<Composer>,
    settings: Entity<SettingsView>,
    settings_nav: Entity<SettingsNav>,
    pub(crate) basecamp: Entity<Basecamp>,
    notes: Entity<crate::notes::NotesView>,
    pub(crate) right_panel: Entity<RightPanel>,
    pub(crate) working_bar: Entity<WorkingBar>,
    /// What the thread's agent runs in the background, above the composer.
    pub(crate) background_strip: Entity<crate::background_strip::BackgroundStrip>,
    title: Entity<WindowTitle>,
    onboarding: Entity<Onboarding>,
    pub(crate) palette: Entity<CommandPalette>,
    /// Takes keyboard focus when whatever had it leaves the screen (the composer, once a settings
    /// page opens): with nothing focused, keys reach none of the window's shortcuts, ⌘K included.
    focus: FocusHandle,
    /// The composer changed since the last frame: lay it out from its content again rather than
    /// reusing its cached frame (see `Composer::element`).
    composer_changed: bool,
    /// Right-panel resize in progress: (pointer x at grab, width at grab).
    panel_drag: Option<(Pixels, f32)>,
    /// Whether the window was last told to blur what's behind it (liquid glass).
    glass_applied: Option<bool>,
    /// With `TREK_FORCE_ACTIVE`, frames for the window while it's hidden (see `mascot::force_active`).
    _hidden_frames: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The chat column never gets narrower than this while the right panel is open.
const MIN_CHAT: f32 = 440.;

impl TrekWindow {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), Scope::Main, window, cx));
        let thread_view = cx.new(|cx| ThreadView::new(workspace.clone(), Scope::Main, window, cx));
        let working_bar = cx.new(|cx| WorkingBar::new(workspace.clone(), Scope::Main, window, cx));
        let background_strip = cx.new(|cx| crate::background_strip::BackgroundStrip::new(workspace.clone(), Scope::Main, window, cx));
        let handle = window.window_handle();
        workspace.update(cx, |ws, _| ws.main_window = Some(handle));
        let settings = cx.new(|cx| SettingsView::new(workspace.clone(), window, cx));
        let onboarding = cx.new(|cx| Onboarding::new(workspace.clone(), window, cx));
        let settings_nav = cx.new(|cx| SettingsNav::new(workspace.clone(), cx));
        let basecamp = cx.new(|cx| Basecamp::new(workspace.clone(), cx));
        let notes = cx.new(|cx| crate::notes::NotesView::new(workspace.clone(), window, cx));
        let saved_width = workspace.read(cx).settings.layout.right_panel_width;
        let right_panel = cx.new(|_| {
            let mut p = RightPanel::new(workspace.clone());
            p.width = saved_width.max(crate::panels::MIN_PANEL);
            p
        });
        let palette = cx.new(|cx| CommandPalette::new(workspace.clone(), right_panel.clone(), basecamp.clone(), window, cx));
        let title = cx.new(|cx| WindowTitle::new(workspace.clone(), right_panel.clone(), cx));
        let subscriptions = vec![
            cx.observe(&workspace, |this, _, cx| {
                this.right_panel.update(cx, |p, cx| p.sync_native(cx));
                cx.notify();
            }),
            // Toasts, alerts, the palette from elsewhere and bringing this window forward are
            // handled app-wide (`init`), so they keep working while this window is closed.
            cx.subscribe_in(&workspace, window, |this, _, event: &WorkspaceEvent, window, cx| match event {
                WorkspaceEvent::Toast { .. } | WorkspaceEvent::Attention { .. } | WorkspaceEvent::ActivateMain | WorkspaceEvent::OpenPalette => {}
                // Basecamp has no composer: it takes the keys itself (Esc goes back).
                WorkspaceEvent::FocusComposer if this.workspace.read(cx).route == Route::Basecamp => this.basecamp.read(cx).focus_handle().focus(window, cx),
                // Nor have notes: the note's text takes them.
                WorkspaceEvent::FocusComposer if this.workspace.read(cx).route == Route::Notes => this.notes.read(cx).focus_handle(cx).focus(window, cx),
                WorkspaceEvent::FocusComposer => this.composer.update(cx, |c, cx| c.focus(window, cx)),
                WorkspaceEvent::OpenTool(tool) => {
                    let tool = *tool;
                    this.right_panel.update(cx, |p, cx| p.open_tool(tool, window, cx));
                }
                WorkspaceEvent::RunInTerminal { command, cwd } => {
                    let (command, cwd) = (command.clone(), cwd.clone());
                    this.right_panel.update(cx, |p, cx| p.run_command(command, cwd, window, cx));
                }
                WorkspaceEvent::InsertIntoComposer(text) => this.composer.update(cx, |c, cx| c.insert_text(text, window, cx)),
                WorkspaceEvent::AttachImage(path) => {
                    let path = path.clone();
                    this.composer.update(cx, |c, cx| c.attach_image(path, cx));
                }
                WorkspaceEvent::RestoreQueued { thread, text, images } => {
                    // A thread window showing the thread takes them instead.
                    if this.workspace.read(cx).shown_in(thread) == Some(Scope::Main) {
                        this.composer.update(cx, |c, cx| c.restore(text, images, window, cx));
                    }
                }
                WorkspaceEvent::ComposeIn { scope: Scope::Main, thread, text, images, edit } => {
                    if this.workspace.read(cx).thread_id_in(&Scope::Main) == Some(thread.as_str()) {
                        this.composer.update(cx, |c, cx| c.compose(thread, text, images, edit.clone(), window, cx));
                    }
                }
                WorkspaceEvent::ComposeIn { .. } => {}
                WorkspaceEvent::CorrectRestatement { scope: Scope::Main, thread } => {
                    if this.workspace.read(cx).thread_id_in(&Scope::Main) == Some(thread.as_str()) {
                        this.composer.update(cx, |c, cx| c.correct(window, cx));
                    }
                }
                WorkspaceEvent::CorrectRestatement { .. } => {}
                // The transcript views and the background strip redraw themselves.
                WorkspaceEvent::Transcript { .. } | WorkspaceEvent::Background { .. } => {}
            }),
            cx.on_focus_lost(window, |this, window, cx| this.focus.focus(window, cx)),
            cx.observe(&composer, |this, _, _| this.composer_changed = true),
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
        // TREK_OPEN_SETTINGS=updates (or any page label, dashes for spaces) opens that settings
        // page at launch, for design review of states that are hard to reach by hand.
        if let Some(page) = std::env::var("TREK_OPEN_SETTINGS").ok().and_then(|n| crate::settings_view::page_named(&n)) {
            // The project page opens on the project on screen.
            workspace.update(cx, |ws, cx| if page == SettingsPage::Project { ws.open_project_settings(None, cx) } else { ws.navigate(Route::Settings(page), cx) });
        }
        // TREK_OPEN_THREAD=<thread id> opens that thread here at launch, the same way; with
        // `@<n>`, scrolled to its nth item, as a search hit would (the transcript scrolled up).
        if let Ok(spec) = std::env::var("TREK_OPEN_THREAD") {
            let spec = spec.trim().to_string();
            let (id, at) = match spec.split_once('@') {
                Some((id, n)) => (id.to_string(), n.parse::<usize>().ok()),
                None => (spec, None),
            };
            workspace.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
            // Once the transcript has loaded and settled at its end.
            if let Some(n) = at {
                let ws = workspace.clone();
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                    let _ = ws.update(cx, |ws, cx| ws.open_thread_at(&id, crate::workspace::ItemRef::Position(n), cx));
                })
                .detach();
            }
        }
        // TREK_OPEN_BASECAMP=1 (or =week, =all) opens Basecamp at launch, the same way.
        if let Ok(which) = std::env::var("TREK_OPEN_BASECAMP") {
            match which.trim() {
                "week" => basecamp.update(cx, |b, cx| b.set_range(trek_core::basecamp::Range::Week, cx)),
                "all" => basecamp.update(cx, |b, cx| b.set_range(trek_core::basecamp::Range::All, cx)),
                _ => {}
            }
            workspace.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx));
        }
        // Once this window hears the workspace (it hears nothing until later; see `show_palette`):
        // the composer, or the focus anchor on screens without one, takes the keys, as after
        // a navigation. A window reopened from a thread window may also have a message to show
        // (a fork's) or follow-ups to hand back.
        cx.defer_in(window, |this, window, cx| {
            let ws = this.workspace.clone();
            match ws.read(cx).route.clone() {
                Route::Thread(_) | Route::Draft { .. } => this.composer.update(cx, |c, cx| c.focus(window, cx)),
                Route::Basecamp => this.basecamp.read(cx).focus_handle().focus(window, cx),
                Route::Notes => this.notes.read(cx).focus_handle(cx).focus(window, cx),
                Route::Settings(_) | Route::Onboarding => this.focus.focus(window, cx),
            }
            let pending = ws.update(cx, |ws, _| ws.pending_compose.take());
            if let Some((thread, text, images)) = pending.filter(|(t, ..)| ws.read(cx).thread_id_in(&Scope::Main) == Some(t.as_str())) {
                this.composer.update(cx, |c, cx| c.compose(&thread, &text, &images, None, window, cx));
            }
            if let Some(id) = ws.read(cx).thread_id_in(&Scope::Main).map(str::to_string) {
                ws.update(cx, |ws, cx| ws.hand_back_queued(&id, cx));
            }
        });
        let hidden_frames = crate::system::hidden_frames(window, cx);
        Self {
            workspace,
            sidebar,
            thread_view,
            composer,
            settings,
            settings_nav,
            basecamp,
            notes,
            right_panel,
            working_bar,
            background_strip,
            title,
            onboarding,
            palette,
            focus: cx.focus_handle(),
            composer_changed: true,
            panel_drag: None,
            glass_applied: None,
            _hidden_frames: hidden_frames,
            _subscriptions: subscriptions,
        }
    }

    /// Where `focus` lives: inside the window's action handlers, and too small to take clicks
    /// (clicking the transcript leaves the composer focused).
    fn focus_anchor(&self) -> Div {
        div().absolute().size_0().track_focus(&self.focus)
    }
}

/// The title bar: sidebar toggle, the project and thread on screen, the project's actions, "Open
/// in", the tools panel and Settle. A view of its own so the working animation's frames reuse it.
pub struct WindowTitle {
    workspace: Entity<Workspace>,
    right_panel: Entity<RightPanel>,
    _subscription: Subscription,
}

impl WindowTitle {
    fn new(workspace: Entity<Workspace>, right_panel: Entity<RightPanel>, cx: &mut Context<Self>) -> Self {
        let _subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
        Self { workspace, right_panel, _subscription }
    }
}

impl Render for WindowTitle {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::tests::rendered("WindowTitle");
        let ws = self.workspace.read(cx);
        let collapsed = ws.sidebar_collapsed;
        let theme = cx.theme().clone();
        let thread = ws.current_thread().cloned();
        let project_name = |p: &std::path::Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (project, title, folder): (Option<String>, String, Option<std::path::PathBuf>) = match &ws.route {
            Route::Thread(_) => match &thread {
                // A thread without a project has a folder of its own, but no project to name.
                Some(t) => (t.cwd.as_deref().filter(|c| !trek_core::paths::is_chat_dir(c)).map(project_name), t.title.clone(), t.cwd.clone()),
                None => (None, "Trek".into(), None),
            },
            Route::Draft { project } => (project.as_deref().map(project_name), "New thread".into(), project.clone()),
            Route::Settings(_) => (None, "Settings".into(), None),
            Route::Basecamp => (None, "Basecamp".into(), None),
            Route::Notes => (None, "Notes".into(), None),
            Route::Onboarding => (None, String::new(), None),
        };
        let settle_id = thread.as_ref().filter(|t| t.settled_at.is_none()).map(|t| t.id.clone());
        let worktree = thread.as_ref().and_then(|t| t.worktree.clone());
        let reveal = match (&ws.route, &thread) {
            (Route::Thread(_), Some(t)) => ws.title_reveal(&t.id).map(|(p, old)| (p, old.to_string())),
            _ => None,
        };
        if reveal.is_some() {
            window.request_animation_frame();
        }
        // The project's icon and its actions (Settings → Project).
        // A thread knows its project (a worktree's folder isn't the project's).
        let root = thread
            .as_ref()
            .and_then(|t| ws.project_dir(t))
            .or_else(|| folder.as_deref().filter(|f| !trek_core::paths::is_chat_dir(f)).map(trek_core::store::project_root));
        let project_entry = root.as_ref().and_then(|r| ws.projects.iter().find(|p| &p.path == r));
        let project = project_entry.map(|p| p.name.clone()).or(project);
        let look = root.as_ref().map(|r| ws.project_look(r)).unwrap_or_default();
        let actions = root.as_ref().map(|r| ws.project_prefs(r).actions).unwrap_or_default();
        let project_id = project_entry.map(|p| p.id.clone());
        // A worktree thread's actions run on its own copy of the code, where its changes are.
        let run_dir = worktree.as_ref().filter(|w| !w.is_missing()).map(|w| w.path.clone()).or_else(|| root.clone());
        let transparent = self.workspace.read(cx).see_through();
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
                            el.child(crate::ui::project_badge(&p, &look, cx))
                                .child(div().text_color(theme.muted_foreground).child(p))
                                .child(div().text_color(theme.muted_foreground.opacity(0.6)).child("/"))
                        })
                        .child(div().min_w_0().font_medium().child(crate::ui::title_text(
                            "window-title",
                            &title,
                            reveal.as_ref().map(|(p, old)| (*p, old.as_str())),
                            theme.foreground,
                            cx,
                        )))
                        .when_some(worktree, |el, wt| el.child(crate::worktree_ui::branch_chip("title-branch", &wt, cx))),
                )
                .when_some(run_dir, |el, dir| el.child(run_button(actions, dir, project_id, self.workspace.clone())))
                .when_some(folder, |el, dir| el.child(open_in_button(dir)))
                .when(thread.is_some(), |el| {
                    el.child(
                        crate::ui::icon_button("pop-out", crate::assets::Lucide::SquareArrowOutUpRight, "Open in new window (⌘⇧↩)")
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

/// TREK_WINDOW_SIZE=<width>x<height> (points) sizes the main window at launch, for design review
/// at sizes a screenshot can't be dragged to.
fn launch_size() -> Option<Size<Pixels>> {
    let v = std::env::var("TREK_WINDOW_SIZE").ok()?;
    let (w, h) = v.trim().split_once('x')?;
    Some(size(px(w.parse().ok()?), px(h.parse().ok()?)))
}

/// Open the main window: at launch, and again when something needs it after it was closed.
/// `focus: false` opens it behind other apps' windows and leaves keyboard focus where it is (a
/// launch in the background).
pub fn open_main(workspace: Entity<Workspace>, focus: bool, cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(launch_size().unwrap_or(size(px(1280.), px(820.))), cx)),
        window_min_size: Some(size(px(760.), px(520.))),
        app_id: Some("dev.trek.Trek".into()),
        focus,
        show: focus,
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        if !focus {
            crate::system::order_back(window);
        }
        if let Some(error) = workspace.update(cx, |ws, _| ws.store_error.take()) {
            window.on_next_frame(move |window, cx| database_error(&error, window, cx));
        }
        cx.new(|cx| TrekWindow::new(workspace, window, cx))
    })?;
    Ok(())
}

/// Trek's database couldn't be opened (another Trek holding it, a damaged file): this session
/// saves nothing, so ask before the user starts working in it.
fn database_error(error: &str, window: &mut Window, cx: &mut App) {
    let detail = format!("{error}\n\nAnything you do now won't be saved. Quit, close any other copy of Trek, and open it again.");
    let answer = window.prompt(gpui_kit::PromptLevel::Critical, "Trek couldn't open its database", Some(&detail), &["Quit", "Continue Without Saving"], cx);
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}

/// Bring the main window forward, reopening it if it was closed.
pub fn show_main(workspace: Entity<Workspace>, cx: &mut App) {
    let main = workspace.read(cx).main_window;
    if main.is_some_and(|m| m.update(cx, |_, window, _| window.activate_window()).is_ok()) {
        return;
    }
    if let Err(e) = open_main(workspace, true, cx) {
        tracing::warn!("main window: {e:#}");
    }
}

/// ⌘K from a thread window or with no window open: the palette opens in the main window (its
/// commands act there), which comes forward, reopened if it was closed. Opened here rather than
/// through an event to the window, as a window opened just now hears none until later.
pub fn show_palette(workspace: Entity<Workspace>, cx: &mut App) {
    show_main(workspace.clone(), cx);
    let Some(main) = workspace.read(cx).main_window else { return };
    let _ = main.update(cx, |root, window, cx| {
        let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
        if let Some(trek) = view.and_then(|v| v.downcast::<TrekWindow>().ok()) {
            let palette = trek.read(cx).palette.clone();
            palette.update(cx, |p, cx| p.open(window, cx));
        }
    });
}

/// Workspace events that belong to no single window: toasts and alerts go to whichever Trek
/// window is in front (else the main one, else any), and "show the main window" reopens it.
/// Handled here rather than by the main window so they keep working after it's closed.
pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    // A click on one of Trek's banners opens its thread. GPUI keeps one handler for the app,
    // which replaces GPUI Kit's own; that one only serves toasts delivered to the system as
    // well, and Trek posts none (see `attention`).
    let ws = workspace.downgrade();
    cx.on_system_notification_response(move |response, cx| {
        let Some(thread) = attention_thread(&response.tag) else { return };
        let Some(ws) = ws.upgrade() else { return };
        cx.activate(true);
        // Not deferred: no window is busy here, and nothing would run a deferred call until
        // the next event.
        reveal_now(&ws, thread, cx);
    });
    cx.subscribe(&workspace, |ws, event: &WorkspaceEvent, cx| match event {
        WorkspaceEvent::Toast { message, undo } => toast(&ws, message.clone(), undo.clone(), cx),
        WorkspaceEvent::Attention { message, thread } => attention(&ws, message.clone(), thread, cx),
        WorkspaceEvent::ActivateMain => show_main(ws, cx),
        WorkspaceEvent::OpenPalette => show_palette(ws, cx),
        _ => {}
    })
    .detach();
    let ws = workspace.downgrade();
    cx.on_window_closed(move |cx, id| {
        let _ = ws.update(cx, |ws, cx| {
            if let Some(main) = ws.main_window.filter(|m| m.window_id() == id) {
                ws.main_window_closed(main, cx);
            }
        });
    })
    .detach();
}

/// The Trek window with keyboard focus; `None` when another app is in front.
fn key_window(cx: &mut App) -> Option<AnyWindowHandle> {
    cx.windows().into_iter().find(|w| w.update(cx, |_, window, _| window.is_window_active()).unwrap_or(false))
}

/// Where an in-app notification goes: the window in front, else the main window, else any.
fn notice_window(workspace: &Entity<Workspace>, front: Option<AnyWindowHandle>, cx: &App) -> Option<AnyWindowHandle> {
    front.or(workspace.read(cx).main_window).or_else(|| cx.windows().into_iter().next())
}

/// A thread needs the user or finished: toast, banner and sound per the notification settings.
/// Decided once for all windows, so a sound or banner never doubles up. Clicking either opens
/// the thread.
fn attention(workspace: &Entity<Workspace>, message: String, thread: &str, cx: &mut App) {
    let front = key_window(cx);
    let ws = workspace.read(cx);
    let alert = crate::system::alert_for(&ws.settings.notifications, front.is_some(), ws.viewing(thread, front));
    if alert.sound {
        crate::system::play_alert_sound();
    }
    if alert.banner {
        // Posted by Trek rather than through a toast's system delivery, so it goes out with no
        // window open too, and its tag says which thread to open when it's clicked.
        cx.show_system_notification(SystemNotification { tag: attention_tag(thread), title: message.clone().into(), body: SharedString::default(), actions: Vec::new() });
    }
    if alert.toast {
        let Some(target) = notice_window(workspace, front, cx) else { return };
        let (ws, thread) = (workspace.clone(), thread.to_string());
        let note = Notification::new().message(message).on_click(move |_, _, cx| reveal_thread(&ws, &thread, cx));
        let _ = target.update(cx, |_, window, cx| window.push_notification(note, cx));
    }
}

const ATTENTION_TAG: &str = "trek-attention-";

/// The system notification tag for an alert about `thread`; a newer alert replaces an older one.
fn attention_tag(thread: &str) -> SharedString {
    format!("{ATTENTION_TAG}{thread}").into()
}

/// The thread a banner Trek posted is about.
fn attention_thread(tag: &str) -> Option<&str> {
    tag.strip_prefix(ATTENTION_TAG).filter(|id| !id.is_empty())
}

/// Bring `thread` on screen: its own window if it has one, else the main window (reopened if
/// it was closed) showing it. Deferred: called from a toast, the window it's in is busy, and
/// reaching it then would look like it had closed (and open a second main window).
pub fn reveal_thread(workspace: &Entity<Workspace>, thread: &str, cx: &mut App) {
    let (workspace, thread) = (workspace.clone(), thread.to_string());
    cx.defer(move |cx| reveal_now(&workspace, &thread, cx));
}

fn reveal_now(workspace: &Entity<Workspace>, thread: &str, cx: &mut App) {
    if workspace.read(cx).thread(thread).is_none() {
        return show_main(workspace.clone(), cx);
    }
    let own = workspace.read(cx).thread_windows.get(thread).copied();
    if own.is_some_and(|w| w.update(cx, |_, window, _| window.activate_window()).is_ok()) {
        return;
    }
    workspace.update(cx, |ws, cx| ws.navigate(Route::Thread(thread.to_string()), cx));
    show_main(workspace.clone(), cx);
}

fn toast(workspace: &Entity<Workspace>, message: String, undo: Option<UndoAction>, cx: &mut App) {
    let front = key_window(cx);
    let Some(target) = notice_window(workspace, front, cx) else { return };
    let mut note = Notification::new().message(message);
    if let Some(action) = undo {
        let ws = workspace.downgrade();
        let label = match action {
            UndoAction::CancelRestart => "Not now",
            UndoAction::MaintainVerification(_) => "Maintain",
            _ => "Undo",
        };
        note = note
            .action(move |_, _, _| {
                let ws = ws.clone();
                let action = action.clone();
                gpui_kit::component::button::Button::new("undo").label(label).small().on_click(move |_, _, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.undo(action.clone(), cx));
                })
            })
            .autohide(true);
    }
    let _ = target.update(cx, |_, window, cx| window.push_notification(note, cx));
}

/// "Run" menu: the project's actions, each opening in a terminal tab in `dir`.
fn run_button(actions: Vec<trek_core::settings::ProjectAction>, dir: std::path::PathBuf, project_id: Option<String>, ws: Entity<Workspace>) -> impl IntoElement {
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    use gpui_kit::component::Sizable as _;
    Button::new("run-actions").ghost().small().icon(crate::assets::Lucide::Play).tooltip("Run a project action").dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
        menu = menu.min_w(px(220.));
        if actions.is_empty() {
            menu = menu.label("No actions for this project yet");
        }
        for a in actions.clone() {
            let (ws, dir) = (ws.clone(), dir.clone());
            menu = menu.item(PopupMenuItem::new(a.name.clone()).icon(crate::assets::Lucide::Play).on_click(move |_, _, cx| {
                let (cmd, dir) = (a.command.clone(), dir.clone());
                ws.update(cx, |ws, cx| ws.run_project_action(dir, cmd, cx))
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
    gpui_kit::component::button::Button::new("open-in")
        .outline()
        .small()
        .icon(IconName::FolderOpen)
        .label("Open")
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            let mut menu = menu.min_w(px(180.));
            // Checked when the menu opens, not on every frame of the title bar.
            let apps = [
                ("Finder", "Finder"),
                ("Terminal", "Terminal"),
                ("Ghostty", "Ghostty"),
                ("Zed", "Zed"),
                ("Cursor", "Cursor"),
                ("VS Code", "Visual Studio Code"),
                ("Xcode", "Xcode"),
            ]
            .into_iter()
            .filter(|(_, app)| matches!(*app, "Finder" | "Terminal") || std::path::Path::new(&format!("/Applications/{app}.app")).exists());
            for (label, app) in apps {
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
        #[cfg(test)]
        crate::tests::rendered("TrekWindow");
        let route = self.workspace.read(cx).route.clone();
        let collapsed = self.workspace.read(cx).sidebar_collapsed;
        if route == Route::Onboarding {
            return v_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(TitleBar::new())
                .child(self.onboarding.clone())
                .child(self.focus_anchor())
                .into_any_element();
        }
        let in_settings = matches!(route, Route::Settings(_));
        let backdrop = self.workspace.read(cx).backdrop();
        let glass = self.workspace.read(cx).glass();
        crate::ui::apply_glass(window, glass.is_some(), &mut self.glass_applied, cx);
        let (right_open, wanted_width) = {
            let p = self.right_panel.read(cx);
            (p.open, p.width)
        };
        // Whatever the stored width, leave the chat column at least MIN_CHAT wide.
        let side = if collapsed || in_settings { 8. } else { SIDEBAR_WIDTH };
        let max_width = (window.viewport_size().width.as_f32() - side - MIN_CHAT - 16.).max(crate::panels::MIN_PANEL);
        let right_width = wanted_width.clamp(crate::panels::MIN_PANEL, max_width);
        let dragging = self.panel_drag.is_some();
        // The heavy views are cached: a frame that only moves the working bar reuses them as
        // drawn. Each re-renders when it's notified.
        let fill = || StyleRefinement::default().size_full();
        let tabs = crate::tabs::strip(&self.workspace, glass.is_some(), cx);
        let content = match route {
            Route::Settings(_) => self.settings.clone().into_any_element(),
            Route::Basecamp => self.basecamp.clone().cached(fill()).into_any_element(),
            Route::Notes => self.notes.clone().into_any_element(),
            Route::Draft { .. } => v_flex()
                .size_full()
                .min_w_0()
                .children(tabs)
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(div().absolute().top_0().left_0().size_full().child(self.thread_view.clone().cached(fill())))
                        .child(
                            v_flex()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .justify_center()
                                .pb(px(40.))
                                .child(Composer::element(&self.composer, &mut self.composer_changed, cx)),
                        ),
                )
                .into_any_element(),
            _ => v_flex()
                .size_full()
                .min_w_0()
                .children(tabs)
                .child(div().flex_1().min_h_0().child(self.thread_view.clone().cached(fill())))
                .child(crate::working_bar::cached(&self.working_bar, self.thread_view.read(cx).tail.clone(), cx))
                .child(crate::background_strip::cached(&self.background_strip, cx))
                .child(Composer::element(&self.composer, &mut self.composer_changed, cx))
                .into_any_element(),
        };
        v_flex()
            .id("trek-window")
            .key_context("TrekWindow")
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
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.palette.update(cx, |p, cx| p.toggle(window, cx))))
            .on_action(cx.listener(|this, _: &OpenBasecamp, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Basecamp, cx))))
            .on_action(cx.listener(|this, _: &crate::OpenNotes, _, cx| this.workspace.update(cx, |ws, cx| ws.navigate(Route::Notes, cx))))
            .on_action(cx.listener(|this, _: &crate::CloseTab, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    if let Route::Thread(id) = ws.route.clone() {
                        ws.close_tab(&id, cx);
                    }
                })
            }))
            .on_action(cx.listener(|this, _: &crate::NextTab, _, cx| this.workspace.update(cx, |ws, cx| ws.cycle_tab(1, cx))))
            .on_action(cx.listener(|this, _: &crate::PreviousTab, _, cx| this.workspace.update(cx, |ws, cx| ws.cycle_tab(-1, cx))))
            .bg(crate::ui::chrome_bg(glass, cx))
            .when_some(backdrop.clone(), |el, (spec, dim)| {
                let side = cx.theme().sidebar;
                el.relative()
                    .child(img(crate::ui::background_source(&spec)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover))
                    .child(div().absolute().top_0().left_0().size_full().bg(side.opacity((0.5 + dim * 0.6).min(0.92))))
            })
            .child(self.focus_anchor())
            .child(self.title.clone().cached(StyleRefinement::default().w_full().flex_none().h(gpui_kit::component::TITLE_BAR_HEIGHT)))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(!collapsed && !in_settings, |el| {
                        el.child(self.sidebar.clone().cached(StyleRefinement::default().w(px(SIDEBAR_WIDTH)).h_full().flex_none()))
                    })
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
                                    .border_color(crate::ui::panel_border(glass, cx))
                                    .bg(crate::ui::panel_bg(glass, cx))
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
                                    .border_color(crate::ui::panel_border(glass, cx))
                                    .bg(crate::ui::panel_bg(glass, cx))
                                    .overflow_hidden()
                                    .child(self.right_panel.clone().cached(fill())),
                            ),
                        )
                    }),
            )
            // ⌘K, drawn over everything else in the window.
            .child(self.palette.clone())
            .into_any_element()
    }
}
