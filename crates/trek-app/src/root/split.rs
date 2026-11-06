//! A split in the main window, as drawn: the two panes, each a whole thread (transcript, working
//! bar, background strip, composer), the divider between them and the drop zones for a thread
//! dragged from the sidebar. What it means is `workspace::split`. The left pane is the window's
//! own views on `Scope::Main`; the right pane's views are made the first time they're needed and
//! go when the split does, so a window that never opens one has none of this.

use super::TrekWindow;
use crate::background_strip::BackgroundStrip;
use crate::composer::Composer;
use crate::scale::{radius, text, to_rems};
use crate::thread_view::ThreadView;
use crate::working_bar::WorkingBar;
use crate::workspace::split::{DIVIDER, Pane, left_width, ratio_at};
use crate::workspace::{Route, Scope, WorkspaceEvent};
use gpui_kit::base::TextSelection;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// A pane's header: what's in it, the way to close it, and which has the focus.
const HEADER: f32 = 30.;

/// The views of the right pane.
pub struct BesidePane {
    pub thread_view: Entity<ThreadView>,
    pub composer: Entity<Composer>,
    pub working_bar: Entity<WorkingBar>,
    pub background_strip: Entity<BackgroundStrip>,
    /// The composer changed since the last frame (see `Composer::element`).
    composer_changed: bool,
    _watch: Subscription,
    /// The keyboard landing in a composer by any way (Tab, a menu or the palette giving it back)
    /// takes the focus there: typing and the focus are never in different panes.
    _focus_in: [Subscription; 2],
}

/// The divider being dragged.
#[derive(Clone, Copy)]
pub struct SplitDrag {
    /// Pointer x at grab, and the left pane's width then.
    from: Pixels,
    width: f32,
    /// The room the panes share.
    room: f32,
    /// Where the drag has it: the workspace hears when it comes to rest.
    pub ratio: f32,
}

/// A thread being dragged from the sidebar, to open beside the one on screen.
#[derive(Clone)]
pub struct DraggedThread {
    pub id: String,
    pub title: SharedString,
}

impl Render for DraggedThread {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(10.))
            .h(px(28.))
            .max_w(px(260.))
            .flex()
            .items_center()
            .rounded(px(crate::fluent::control(radius::BUTTON)))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(px(text::BODY))
            .text_color(theme.foreground)
            .child(div().min_w_0().truncate().child(self.title.clone()))
    }
}

impl TrekWindow {
    /// The right pane's views, made the first time they're asked for.
    pub(super) fn beside_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut BesidePane {
        if self.beside.is_none() {
            let ws = self.workspace.clone();
            let thread_view = cx.new(|cx| ThreadView::new(ws.clone(), Scope::Beside, window, cx));
            let working_bar = cx.new(|cx| WorkingBar::new(ws.clone(), Scope::Beside, window, cx));
            let background_strip =
                cx.new(|cx| BackgroundStrip::new(ws.clone(), Scope::Beside, window, cx));
            let composer = cx.new(|cx| Composer::new(ws.clone(), Scope::Beside, window, cx));
            let main = self.composer.clone();
            composer.update(cx, |c, cx| c.share_drafts_with(&main, window, cx));
            let _watch = cx.observe(&composer, |this, _, _| {
                if let Some(pane) = this.beside.as_mut() {
                    pane.composer_changed = true;
                }
            });
            let (left, right) = (
                main.read(cx).focus_handle(cx),
                composer.read(cx).focus_handle(cx),
            );
            let _focus_in = [
                cx.on_focus_in(&left, window, |this, _, cx| {
                    this.workspace
                        .update(cx, |ws, cx| ws.focus_pane(Pane::Left, cx))
                }),
                cx.on_focus_in(&right, window, |this, _, cx| {
                    this.workspace
                        .update(cx, |ws, cx| ws.focus_pane(Pane::Right, cx))
                }),
            ];
            self.beside = Some(BesidePane {
                thread_view,
                composer,
                working_bar,
                background_strip,
                composer_changed: true,
                _watch,
                _focus_in,
            });
        }
        self.beside.as_mut().expect("just made")
    }

    /// The composer of the pane with the focus: what "the composer" means to the window's keys and
    /// to what's meant for it (a browser pick, an attachment).
    pub(super) fn focused_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Composer> {
        match self.workspace.read(cx).harness_scope() {
            Scope::Beside => self.beside_pane(window, cx).composer.clone(),
            _ => self.composer.clone(),
        }
    }

    /// The composer showing `thread`, if a pane or the main window has it on screen.
    pub(super) fn composer_showing(
        &mut self,
        thread: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Composer>> {
        let ws = self.workspace.read(cx);
        if ws.thread_id_in(&Scope::Main) == Some(thread) {
            Some(self.composer.clone())
        } else if ws.thread_id_in(&Scope::Beside) == Some(thread) {
            Some(self.beside_pane(window, cx).composer.clone())
        } else {
            None
        }
    }

    /// A split that came, went or changed threads since the workspace last spoke. A thread gone
    /// from its pane takes the pane with it. The composers and transcripts have each followed the
    /// workspace by the time the deferred part runs: a message put away just as its thread came
    /// to the other pane is picked up there, and closing the left pane moves the right one's
    /// thread to the window's own views with its place in the transcript.
    pub(super) fn sync_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.split_prune(cx));
        let now = self
            .workspace
            .read(cx)
            .split
            .as_ref()
            .map(|s| (s.left.clone(), s.right.clone()));
        if now == self.split_shown {
            return;
        }
        let was = std::mem::replace(&mut self.split_shown, now.clone());
        if now.is_some() {
            self.beside_pane(window, cx);
            cx.defer_in(window, |this, window, cx| {
                this.composer
                    .update(cx, |c, cx| c.take_up_parked(window, cx));
                if let Some(pane) = this.beside.as_ref() {
                    pane.composer
                        .clone()
                        .update(cx, |c, cx| c.take_up_parked(window, cx));
                }
            });
            return;
        }
        self.split_drag = None;
        cx.defer_in(window, move |this, window, cx| {
            this.composer
                .update(cx, |c, cx| c.take_up_parked(window, cx));
            let Some(pane) = this.beside.take() else {
                return;
            };
            let Some((_, right)) = was else {
                return;
            };
            if this.workspace.read(cx).thread_id_in(&Scope::Main) == Some(right.as_str()) {
                let to = this.thread_view.clone();
                pane.thread_view
                    .update(cx, |v, cx| v.hand_over(&right, &to, cx));
            }
        });
    }

    /// A press in the pane without the focus moves the focus there, and goes on to do what it was
    /// for. Another button's (a menu's) takes the keyboard along now, for the menu to give it back
    /// to; the left button's settles when it's released (`pane_press_ended`).
    fn pane_pressed(
        &mut self,
        pane: Pane,
        button: MouseButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was = self.focused_composer(window, cx);
        self.workspace.update(cx, |ws, cx| ws.focus_pane(pane, cx));
        if button == MouseButton::Left {
            self.pane_press = Some(was);
        } else {
            let now = self.focused_composer(window, cx);
            now.update(cx, |c, cx| c.focus(window, cx));
        }
    }

    /// The press that moved the focus is over: typing goes to the pane it moved to. The keyboard
    /// stays where the press put it if that's a text field of the pane's own or a selection to
    /// copy, and where it was if that's nothing to do with the panes (the terminal dock). A press
    /// on an answer's text, which takes the keyboard to select with, gives it up if it selected
    /// nothing.
    pub(super) fn pane_press_ended(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(was) = self.pane_press.take() else {
            return;
        };
        cx.defer_in(window, move |this, window, cx| {
            let on_text = window
                .context_stack()
                .iter()
                .any(|c| c.contains("TextView"));
            let selected = on_text && TextSelection::has_selection(window, cx);
            let idle = window.focused(cx).is_none() || this.focus.is_focused(window);
            if !selected && (idle || on_text || was.read(cx).is_focused(window, cx)) {
                let now = this.focused_composer(window, cx);
                now.update(cx, |c, cx| c.focus(window, cx));
            }
        });
    }

    /// The pane's strip: its agent, its title, its model, and its ×. The one with the focus reads
    /// in full; the other steps back.
    fn pane_header(
        &self,
        pane: Pane,
        id: &str,
        focused: bool,
        alone: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let ws = self.workspace.read(cx);
        let theme = cx.theme().clone();
        let thread = ws.thread(id);
        let title = thread.map(|t| t.title.clone()).unwrap_or_default();
        let model = thread.map(|t| ws.model_label(t).to_string());
        let logo = thread.map(|t| crate::ui::agent_logo(&t.agent, px(13.), cx));
        h_flex()
            .relative()
            .flex_none()
            .w_full()
            .h(px(HEADER))
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(7.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.06))
            .text_size(px(text::BODY))
            .when(focused, |el| el.bg(theme.foreground.opacity(0.03)))
            .when(focused && !alone, |el| {
                el.child(
                    div()
                        .id(match pane {
                            Pane::Left => "split-focus-left",
                            Pane::Right => "split-focus-right",
                        })
                        .test_support()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(theme.foreground.opacity(0.28)),
                )
            })
            .children(logo)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(focused, |el| {
                        el.text_color(theme.foreground)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!focused, |el| el.text_color(theme.muted_foreground))
                    .child(title),
            )
            .when(alone, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(to_rems(text::META))
                        .text_color(theme.muted_foreground)
                        .child("Widen the window to see both"),
                )
            })
            .when(!alone, |el| {
                el.children(model.filter(|m| !m.is_empty()).map(|m| {
                    div()
                        .flex_shrink_1()
                        .min_w(px(0.))
                        .max_w(px(140.))
                        .truncate()
                        .text_size(to_rems(text::META))
                        .text_color(theme.muted_foreground)
                        .child(m)
                }))
            })
            .child(
                crate::ui::icon_button(
                    match pane {
                        Pane::Left => "split-close-left",
                        Pane::Right => "split-close-right",
                    },
                    IconName::Close,
                    "Close pane (⌘W)", // icon_button( localizes
                )
                .xsmall()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.update(cx, |ws, cx| ws.close_pane(pane, cx))
                })),
            )
    }

    /// One pane: a whole thread, `width` wide (the rest of the row with `None`).
    #[allow(clippy::too_many_arguments)]
    fn pane_view(
        &mut self,
        pane: Pane,
        id: &str,
        width: Option<f32>,
        focused: bool,
        alone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fill = || StyleRefinement::default().size_full();
        let header = self.pane_header(pane, id, focused, alone, cx);
        // As in the single layout: cached, so a frame that moves one bar reuses the rest.
        let (view, bar, strip, composer) = match pane {
            Pane::Left => (
                self.thread_view.clone(),
                self.working_bar.clone(),
                self.background_strip.clone(),
                Composer::element(&self.composer, &mut self.composer_changed, cx),
            ),
            Pane::Right => {
                let p = self.beside_pane(window, cx);
                let (view, bar, strip) = (
                    p.thread_view.clone(),
                    p.working_bar.clone(),
                    p.background_strip.clone(),
                );
                let composer_entity = p.composer.clone();
                let composer = Composer::element(&composer_entity, &mut p.composer_changed, cx);
                (view, bar, strip, composer)
            }
        };
        let tail = view.read(cx).tail.clone();
        v_flex()
            .id(match pane {
                Pane::Left => "split-pane-left",
                Pane::Right => "split-pane-right",
            })
            .test_support()
            .when_some(width, |el, w| el.flex_none().w(px(w)))
            .when(width.is_none(), |el| el.flex_1())
            .min_w_0()
            .h_full()
            .when(!focused, |el| {
                el.capture_any_mouse_down(cx.listener(
                    move |this, e: &MouseDownEvent, window, cx| {
                        this.pane_pressed(pane, e.button, window, cx)
                    },
                ))
            })
            .child(header)
            .child(div().flex_1().min_h_0().child(view.cached(fill())))
            .child(crate::working_bar::cached(&bar, tail, cx))
            .child(crate::background_strip::cached(&strip, cx))
            .child(composer)
            .into_any_element()
    }

    /// The grab strip between the panes: drag to resize, double-click to even them up.
    fn divider(&self, width: f32, room: f32, ratio: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let dragging = self.split_drag.is_some();
        div()
            .id("split-divider")
            .test_support()
            .flex_none()
            .w(px(DIVIDER))
            .h_full()
            .flex()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .group("split-divider")
            .child(
                div()
                    .h_full()
                    .when(dragging, |el| {
                        el.w(px(2.)).bg(theme.foreground.opacity(0.35))
                    })
                    .when(!dragging, |el| {
                        el.w(px(1.))
                            .bg(theme.foreground.opacity(0.08))
                            .group_hover("split-divider", |s| s.bg(theme.foreground.opacity(0.2)))
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        this.workspace
                            .update(cx, |ws, cx| ws.set_split_ratio(0.5, cx));
                        return;
                    }
                    this.split_drag = Some(SplitDrag {
                        from: e.position.x,
                        width,
                        room,
                        ratio,
                    });
                    // Native views (browser, simulator) would swallow the drag.
                    this.workspace.update(cx, |ws, cx| {
                        ws.overlay_open = true;
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The panes side by side in `room`. With less room than two need (the window shrank, or the
    /// tools panel opened) only the one with the focus is drawn, and it says so.
    pub(super) fn split_view(
        &mut self,
        room: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(split) = self.workspace.read(cx).split_live().cloned() else {
            return div().into_any_element();
        };
        if !crate::workspace::split::fits(room) {
            let id = split.thread(split.focus).to_string();
            return self.pane_view(split.focus, &id, None, true, true, window, cx);
        }
        let ratio = self.split_drag.map_or(split.ratio, |d| d.ratio);
        let width = left_width(ratio, room);
        let left = self.pane_view(
            Pane::Left,
            &split.left,
            Some(width),
            split.focus == Pane::Left,
            false,
            window,
            cx,
        );
        let right = self.pane_view(
            Pane::Right,
            &split.right,
            None,
            split.focus == Pane::Right,
            false,
            window,
            cx,
        );
        h_flex()
            .id("split-view")
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(left)
            .child(self.divider(width, room, ratio, cx))
            .child(right)
            .into_any_element()
    }

    /// The pointer moved with the divider in hand.
    pub(super) fn drag_split(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(drag) = self.split_drag.as_mut() {
            drag.ratio = ratio_at(drag.width + (x - drag.from).as_f32(), drag.room);
            cx.notify();
        }
    }

    pub(super) fn end_split_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.split_drag.take() else {
            return;
        };
        self.workspace.update(cx, |ws, cx| {
            ws.overlay_open = false;
            ws.set_split_ratio(drag.ratio, cx);
            cx.notify();
        });
        cx.notify();
    }

    /// Where a thread dragged from the sidebar can land, while something is being dragged: the
    /// right half of a single thread opens it beside, and over a split each pane takes it. They're
    /// empty until a thread is over them. `top`: clear of the tab strip, whose own drags go on.
    /// `room`: the main area's width, `panes`: what a split's panes have of it.
    pub(super) fn drop_zones(
        &self,
        top: f32,
        room: f32,
        panes: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let ws = self.workspace.read(cx);
        if !cx.has_active_drag() || !matches!(ws.route, Route::Thread(_)) {
            return vec![];
        }
        let theme = cx.theme().clone();
        let zone = |id: &'static str, label: &'static str| {
            let tint = theme.foreground;
            let group = SharedString::from(id);
            div()
                .id(id)
                .test_support()
                .group(group.clone())
                .absolute()
                .top(px(top))
                .bottom_0()
                .flex()
                .items_center()
                .justify_center()
                .drag_over::<DraggedThread>(move |s, _, _, _| {
                    s.bg(tint.opacity(0.07)).border_color(tint.opacity(0.25))
                })
                .border_1()
                .border_color(tint.opacity(0.))
                .child(
                    div()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(crate::fluent::control(radius::BUTTON)))
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(text::BODY))
                        .text_color(theme.foreground)
                        .opacity(0.)
                        .group_drag_over::<DraggedThread>(group, |s| s.opacity(1.))
                        .child(label),
                )
        };
        match ws
            .split_live()
            .filter(|_| crate::workspace::split::fits(panes))
        {
            None => vec![
                zone("split-drop", "Open beside")
                    .right_0()
                    .w(px((room / 2.).max(0.)))
                    .on_drop(cx.listener(|this, d: &DraggedThread, _, cx| {
                        this.workspace
                            .update(cx, |ws, cx| ws.open_beside(&d.id, cx))
                    }))
                    .into_any_element(),
            ],
            Some(split) => {
                let width = left_width(split.ratio, panes);
                vec![
                    zone("split-drop-left", "Open here")
                        .left_0()
                        .w(px(width))
                        .on_drop(cx.listener(|this, d: &DraggedThread, _, cx| {
                            this.workspace
                                .update(cx, |ws, cx| ws.open_in_pane(Pane::Left, &d.id, cx))
                        }))
                        .into_any_element(),
                    zone("split-drop-right", "Open here")
                        .left(px(width + DIVIDER))
                        .right_0()
                        .on_drop(cx.listener(|this, d: &DraggedThread, _, cx| {
                            this.workspace
                                .update(cx, |ws, cx| ws.open_in_pane(Pane::Right, &d.id, cx))
                        }))
                        .into_any_element(),
                ]
            }
        }
    }

    /// ⌘\: with a split showing, the focus goes across; on a thread, pick another to go beside it.
    pub(super) fn open_beside_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.read(cx);
        if ws.ide() {
            return;
        }
        if ws.split_live().is_some() {
            self.workspace.update(cx, |ws, cx| ws.focus_other_pane(cx));
        } else if matches!(ws.route, Route::Thread(_)) {
            self.palette.update(cx, |p, cx| p.toggle_beside(window, cx));
        } else {
            self.workspace.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message: "Open a thread first, then another beside it".into(),
                    undo: None,
                })
            });
        }
    }
}
