//! A thread in a window of its own: title bar, transcript and composer bound to that one thread,
//! whatever the main window is showing. Closing it leaves the session running; archiving or
//! deleting the thread closes it.

use crate::composer::Composer;
use crate::thread_view::ThreadView;
use crate::working_bar::WorkingBar;
use crate::workspace::{Route, Scope, SettingsPage, Workspace, WorkspaceEvent};
use crate::*;
use gpui_kit::component::{ActiveTheme as _, IconName, StyledExt as _, TitleBar, v_flex, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Open `id` in its own window, or bring its window forward if it already has one.
pub fn open(workspace: Entity<Workspace>, id: &str, cx: &mut App) {
    open_with_focus(workspace, id, true, cx);
}

/// `open`; with `focus: false` the window opens behind other apps' windows and keyboard focus
/// stays where it is (a launch in the background).
pub fn open_with_focus(workspace: Entity<Workspace>, id: &str, focus: bool, cx: &mut App) {
    let (existing, exists, open_count) = {
        let ws = workspace.read(cx);
        (ws.thread_windows.get(id).copied(), ws.thread(id).is_some(), ws.thread_windows.len())
    };
    if let Some(handle) = existing {
        if handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    if !exists {
        return;
    }
    // Each new window steps down and right from the last so they don't stack exactly.
    let step = px(28. * (open_count % 6) as f32);
    let mut bounds = Bounds::centered(None, size(px(880.), px(780.)), cx);
    bounds.origin = bounds.origin + point(step, step);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(520.), px(440.))),
        app_id: Some("dev.trek.Trek".into()),
        focus,
        show: focus || crate::system::SHOW_BEHIND,
        ..TitleBar::window_options()
    };
    let thread_id = id.to_string();
    let ws = workspace.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        if !focus {
            crate::system::order_back(window);
        }
        cx.new(|cx| ThreadWindow::new(ws, thread_id, window, cx))
    }) {
        Ok((handle, _)) => workspace.update(cx, |ws, cx| ws.thread_window_opened(id, handle, cx)),
        Err(e) => tracing::warn!("thread window: {e:#}"),
    }
}

pub struct ThreadWindow {
    workspace: Entity<Workspace>,
    id: String,
    thread_view: Entity<ThreadView>,
    composer: Entity<Composer>,
    working_bar: Entity<WorkingBar>,
    background_strip: Entity<crate::background_strip::BackgroundStrip>,
    /// Attachments up close, over everything else in the window.
    preview: Entity<crate::image_preview::ImagePreview>,
    /// The composer changed since the last frame (see `Composer::element`).
    composer_changed: bool,
    /// The window title as last set (the thread's title).
    title: String,
    /// Whether the window was last told to blur what's behind it (liquid glass).
    glass_applied: Option<bool>,
    /// With `TREK_FORCE_ACTIVE`, frames for the window while it's hidden (see `system::hidden_frames`).
    _hidden_frames: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ThreadWindow {
    fn new(workspace: Entity<Workspace>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scope = Scope::Thread(id.clone());
        let thread_view = cx.new(|cx| ThreadView::new(workspace.clone(), scope.clone(), window, cx));
        let working_bar = cx.new(|cx| WorkingBar::new(workspace.clone(), scope.clone(), window, cx));
        let background_strip = cx.new(|cx| crate::background_strip::BackgroundStrip::new(workspace.clone(), scope.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), scope, window, cx));
        let preview = cx.new(|cx| crate::image_preview::ImagePreview::new(window, cx));
        let title = workspace.read(cx).thread(&id).map(|t| t.title.clone()).unwrap_or_default();
        window.set_window_title(&title);
        let handle = window.window_handle();
        let subscriptions = vec![
            cx.observe_in(&workspace, window, |this, ws, window, cx| {
                match ws.read(cx).thread(&this.id).filter(|t| t.archived_at.is_none()).map(|t| t.title.clone()) {
                    None => window.remove_window(),
                    Some(title) if title != this.title => {
                        window.set_window_title(&title);
                        this.title = title;
                    }
                    Some(_) => {}
                }
                cx.notify();
            }),
            cx.subscribe_in(&workspace, window, |this, _, event: &WorkspaceEvent, window, cx| {
                match event {
                    WorkspaceEvent::RestoreQueued { thread, text, images } if *thread == this.id => {
                        this.composer.update(cx, |c, cx| c.restore(text, images, window, cx));
                    }
                    WorkspaceEvent::ComposeIn { scope: Scope::Thread(t), thread, text, images, edit } if *t == this.id => {
                        this.composer.update(cx, |c, cx| c.compose(thread, text, images, edit.clone(), window, cx));
                    }
                    WorkspaceEvent::CorrectRestatement { scope: Scope::Thread(t), .. } if *t == this.id => {
                        this.composer.update(cx, |c, cx| c.correct(window, cx));
                    }
                    _ => {}
                }
            }),
            cx.observe(&composer, |this, _, _| this.composer_changed = true),
            // With nothing focused, keys reach none of the window's shortcuts (⌘W included): the
            // composer takes focus back when what had it leaves (an answered question's text field).
            cx.on_focus_lost(window, |this, window, cx| this.composer.update(cx, |c, cx| c.focus(window, cx))),
            cx.on_release({
                let (ws, id) = (workspace.downgrade(), id.clone());
                move |_, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.thread_window_closed(&id, handle, cx));
                }
            }),
        ];
        let c = composer.clone();
        window.defer(cx, move |window, cx| c.update(cx, |c, cx| c.focus(window, cx)));
        let _hidden_frames = crate::system::hidden_frames(window, cx);
        Self { workspace, id, thread_view, composer, working_bar, background_strip, preview, composer_changed: true, title, glass_applied: None, _hidden_frames, _subscriptions: subscriptions }
    }

    /// `narrow`: the window is too narrow for the project's name and the Open button's label.
    fn title_bar(&self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let thread = ws.thread(&self.id).cloned();
        let folder = thread.as_ref().and_then(|t| t.cwd.clone());
        let chat = folder.as_deref().is_some_and(trek_core::paths::is_chat_dir);
        let root = thread.as_ref().and_then(|t| ws.project_dir(t)).or_else(|| folder.as_deref().filter(|_| !chat).map(trek_core::store::project_root));
        let project = root.as_ref().and_then(|r| ws.projects.iter().find(|p| &p.path == r));
        let name = project
            .map(|p| p.name.clone())
            .or_else(|| folder.as_ref().filter(|_| !chat).and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()));
        let look = root.as_ref().map(|r| ws.project_look(r)).unwrap_or_default();
        let settle_id = thread.as_ref().filter(|t| t.settled_at.is_none()).map(|t| t.id.clone());
        let worktree = thread.as_ref().and_then(|t| t.worktree.clone());
        let title = thread.map(|t| t.title).unwrap_or_default();
        let reveal = ws.title_reveal(&self.id).map(|(p, old)| (p, old.to_string()));
        let transparent = ws.see_through();
        TitleBar::new().when(transparent, |t| t.bg(gpui_kit::transparent_black())).child(
            h_flex()
                .w_full()
                .h_full()
                .items_center()
                .gap_2()
                .pr_2()
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .pl_1()
                        .gap_2()
                        .text_sm()
                        .overflow_hidden()
                        .when_some(name, |el, p| {
                            el.child(div().flex_none().child(crate::ui::project_badge(&p, &look, cx))).when(!narrow, |el| {
                                el.child(div().flex_shrink_1().min_w(px(24.)).truncate().text_color(theme.muted_foreground).child(p))
                                    .child(div().flex_none().text_color(theme.muted_foreground.opacity(0.6)).child("/"))
                            })
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
                .when_some(folder, |el, dir| el.child(crate::root::open_in_button(dir, narrow)))
                .when_some(settle_id, |el, id| {
                    el.child(crate::ui::icon_button("settle", IconName::Check, "Settle (⌘E)").on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.workspace.update(cx, |ws, cx| ws.settle(&id, cx))
                    })))
                }),
        )
    }

    /// Open `route` in the main window and bring it forward (settings, a new thread).
    fn show_in_main(&self, route: Route, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.show_in_main(route, cx));
    }
}

impl Render for ThreadWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let backdrop = self.workspace.read(cx).backdrop();
        let glass = self.workspace.read(cx).glass();
        crate::ui::apply_glass(window, glass.is_some(), &mut self.glass_applied, cx);
        crate::root::place_toasts(self.composer.read(cx).height().max(px(120.)) + px(16.), window, cx);
        if self.workspace.read(cx).title_reveal(&self.id).is_some() {
            window.request_animation_frame();
        }
        let theme = cx.theme().clone();
        v_flex()
            .id("thread-window")
            .key_context("ThreadWindow")
            .size_full()
            .bg(crate::ui::chrome_bg(glass, cx))
            .text_color(theme.foreground)
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &SettleThread, _, cx| {
                let id = this.id.clone();
                this.workspace.update(cx, |ws, cx| ws.settle(&id, cx))
            }))
            .on_action(cx.listener(|this, _: &Interrupt, _, cx| {
                let id = this.id.clone();
                this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx))
            }))
            .on_action(cx.listener(|this, _: &CycleHandHolding, _, cx| {
                let scope = Scope::Thread(this.id.clone());
                this.workspace.update(cx, |ws, cx| ws.cycle_hand_holding(&scope, cx))
            }))
            .on_action(cx.listener(|this, _: &TakeSnapshot, _, cx| this.composer.update(cx, |c, cx| c.snapshot_default(cx))))
            .on_action(cx.listener(|this, _: &NewThread, _, cx| {
                // A new thread, in no project as ⌘N is in the main window, composed there.
                this.show_in_main(Route::Draft { project: None }, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.show_in_main(Route::Settings(SettingsPage::General), cx)))
            .on_action(cx.listener(|this, _: &OpenPalette, _, cx| this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenPalette))))
            .on_action(cx.listener(|this, _: &About, _, cx| this.show_in_main(Route::Settings(SettingsPage::About), cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.workspace.update(cx, |ws, cx| ws.check_for_updates(true, cx));
                this.show_in_main(Route::Settings(SettingsPage::Updates), cx)
            }))
            .on_action(cx.listener(|this, _: &OpenFolder, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    cx.emit(WorkspaceEvent::ActivateMain);
                    ws.open_folder(cx)
                })
            }))
            .when_some(backdrop, |el, (spec, dim)| {
                let side = theme.sidebar;
                el.relative()
                    .child(img(crate::ui::background_source(&spec)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover))
                    .child(div().absolute().top_0().left_0().size_full().bg(side.opacity((0.5 + dim * 0.6).min(0.92))))
            })
            .child(self.title_bar(window.viewport_size().width < px(640.), cx))
            .child(
                div().flex_1().min_h_0().px_2().pb_2().child(
                    v_flex()
                        .size_full()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(crate::ui::panel_border(glass, cx))
                        .bg(crate::ui::panel_bg(glass, cx))
                        .overflow_hidden()
                        .relative()
                        .children(crate::ui::glass_sheen(glass, cx))
                        // Cached as in the main window: the working bar's frames redraw only the bar.
                        .child(div().flex_1().min_h_0().child(self.thread_view.clone().cached(StyleRefinement::default().size_full())))
                        .child(crate::working_bar::cached(&self.working_bar, self.thread_view.read(cx).tail.clone(), cx))
                        .child(crate::background_strip::cached(&self.background_strip, cx))
                        .child(Composer::element(&self.composer, &mut self.composer_changed, cx)),
                ),
            )
            .child(self.preview.clone())
            .children(self.preview.read(cx).is_open().then(|| crate::chrome::caption_over_overlays(window, cx)).flatten())
    }
}
