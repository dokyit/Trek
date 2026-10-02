//! A thread in a window of its own: title bar, transcript and composer bound to that one thread,
//! whatever the main window is showing. Closing it leaves the session running; archiving or
//! deleting the thread closes it.

use crate::composer::Composer;
use crate::thread_view::ThreadView;
use crate::workspace::{Route, Scope, SettingsPage, Workspace, WorkspaceEvent};
use crate::*;
use gpui_kit::component::{ActiveTheme as _, IconName, StyledExt as _, TitleBar, v_flex, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Open `id` in its own window, or bring its window forward if it already has one.
pub fn open(workspace: Entity<Workspace>, id: &str, cx: &mut App) {
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
        ..TitleBar::window_options()
    };
    let thread_id = id.to_string();
    let ws = workspace.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| ThreadWindow::new(ws, thread_id, window, cx))) {
        Ok((handle, _)) => workspace.update(cx, |ws, cx| ws.thread_window_opened(id, handle, cx)),
        Err(e) => tracing::warn!("thread window: {e:#}"),
    }
}

pub struct ThreadWindow {
    workspace: Entity<Workspace>,
    id: String,
    thread_view: Entity<ThreadView>,
    composer: Entity<Composer>,
    /// The window title as last set (the thread's title).
    title: String,
    _subscriptions: Vec<Subscription>,
}

impl ThreadWindow {
    fn new(workspace: Entity<Workspace>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scope = Scope::Thread(id.clone());
        let thread_view = cx.new(|cx| ThreadView::new(workspace.clone(), scope.clone(), window, cx));
        let composer = cx.new(|cx| Composer::new(workspace.clone(), scope, window, cx));
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
                if let WorkspaceEvent::RestoreQueued { thread, text, images } = event {
                    if *thread == this.id {
                        this.composer.update(cx, |c, cx| c.restore(text, images, window, cx));
                    }
                }
            }),
            cx.on_release({
                let (ws, id) = (workspace.downgrade(), id.clone());
                move |_, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.thread_window_closed(&id, handle, cx));
                }
            }),
        ];
        let c = composer.clone();
        window.defer(cx, move |window, cx| c.update(cx, |c, cx| c.focus(window, cx)));
        Self { workspace, id, thread_view, composer, title, _subscriptions: subscriptions }
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let thread = ws.thread(&self.id).cloned();
        let folder = thread.as_ref().and_then(|t| t.cwd.clone());
        let root = folder.as_deref().map(trek_core::store::project_root);
        let project = root.as_ref().and_then(|r| ws.projects.iter().find(|p| &p.path == r));
        let name = project.map(|p| p.name.clone()).or_else(|| folder.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().to_string()));
        let icon = root.as_ref().and_then(|r| ws.project_icon(r));
        let settle_id = thread.as_ref().filter(|t| t.settled_at.is_none()).map(|t| t.id.clone());
        let title = thread.map(|t| t.title).unwrap_or_default();
        let transparent = ws.backdrop().is_some();
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
                        .when_some(name, |el, p| {
                            el.child(crate::ui::project_badge(&p, icon.as_deref(), cx))
                                .child(div().flex_none().text_color(theme.muted_foreground).child(p))
                                .child(div().text_color(theme.muted_foreground.opacity(0.6)).child("/"))
                        })
                        .child(div().truncate().font_medium().child(title)),
                )
                .when_some(folder, |el, dir| el.child(crate::root::open_in_button(dir)))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let backdrop = self.workspace.read(cx).backdrop();
        let theme = cx.theme().clone();
        v_flex()
            .id("thread-window")
            .key_context("ThreadWindow")
            .size_full()
            .bg(theme.sidebar)
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
                // A new thread in this thread's project, composed in the main window.
                let project = this.workspace.read(cx).thread(&this.id).and_then(|t| t.cwd.clone());
                this.show_in_main(Route::Draft { project }, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.show_in_main(Route::Settings(SettingsPage::General), cx)))
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
            .child(self.title_bar(cx))
            .child(
                div().flex_1().min_h_0().px_2().pb_2().child(
                    v_flex()
                        .size_full()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme.sidebar_border)
                        .bg(theme.background)
                        .overflow_hidden()
                        .child(div().flex_1().min_h_0().child(self.thread_view.clone()))
                        .child(self.composer.clone()),
                ),
            )
    }
}
