//! Side chat: a quick, separate conversation next to the main thread (same project, own session).

use crate::attachments::{self, Attaching, Outbox};
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use trek_core::store::Item;

pub struct SideChatPanel {
    workspace: Entity<Workspace>,
    thread_id: Option<String>,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    /// Images going out with the next message (pasted with ⌘V).
    outbox: Outbox,
    /// `general.send_with_cmd_enter`, as last applied to the textarea.
    cmd_enter: bool,
    _subscriptions: Vec<Subscription>,
}

impl SideChatPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cmd_enter = workspace.read(cx).settings.general.send_with_cmd_enter;
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 8).submit_on_enter(!cmd_enter).placeholder("Ask on the side…"));
        let subs = vec![
            cx.observe(&workspace, |this, ws, cx| {
                let cmd_enter = ws.read(cx).settings.general.send_with_cmd_enter;
                if cmd_enter != this.cmd_enter {
                    this.cmd_enter = cmd_enter;
                    this.input.update(cx, |s, cx| s.set_submit_on_enter(!cmd_enter, cx));
                }
                this.scroll.scroll_to_bottom();
                cx.notify();
            }),
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| {
                if let WorkspaceEvent::Transcript { id, .. } = event
                    && this.thread_id.as_ref() == Some(id)
                {
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
                if let InputEvent::PressEnter { shift, secondary } = event {
                    if if this.cmd_enter { *secondary } else { !*shift } {
                        this.submit(state.clone(), window, cx);
                    }
                }
            }),
        ];
        Self { workspace, thread_id: None, input, scroll: ScrollHandle::new(), outbox: Outbox::default(), cmd_enter, _subscriptions: subs }
    }

    fn submit(&mut self, state: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.target(cx);
        if self.outbox.hold_send(target) {
            cx.notify();
            return;
        }
        let text = state.read(cx).value().to_string();
        if text.trim().is_empty() && self.outbox.paths.is_empty() {
            return;
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        let images = std::mem::take(&mut self.outbox.paths);
        self.send(text, images, cx);
    }

    fn send(&mut self, text: String, images: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.thread_id.is_none() {
            self.thread_id = self.workspace.update(cx, |ws, cx| ws.create_side_chat(cx));
        }
        if let Some(id) = self.thread_id.clone() {
            self.workspace.update(cx, |ws, cx| ws.send_to(&id, text, images, cx));
        }
    }
}

impl SideChatPanel {
    /// Where the keys go in a side chat: its message box. A side chat that opens is ready to
    /// be typed in (`RightPanel::after_activate`).
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Attaching for SideChatPanel {
    fn outbox(&mut self) -> &mut Outbox {
        &mut self.outbox
    }

    fn target(&self, _: &App) -> String {
        self.thread_id.clone().unwrap_or_default()
    }

    fn send_held(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        self.submit(input, window, cx);
    }
}

impl Render for SideChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let live = self.thread_id.as_ref().and_then(|id| ws.live.get(id));
        let items: Vec<Item> = live.map(|l| l.items.to_vec()).unwrap_or_default();
        // While it answers, a trail word ("Breaking trail…") rather than a plain "Working…".
        let working = live.and_then(|l| l.turn_started).map(|t| crate::working_bar::trail_word(self.thread_id.as_deref().unwrap_or_default(), Some(t.elapsed())));
        let now = ws.now();
        let amber = crate::palette::amber(cx);
        let prefs = ws.prefs();
        let id = self.thread_id.clone().unwrap_or_default();
        // Answers read as they do in the transcript, a size down to suit the narrower panel.
        let thread = ws.thread(&id);
        let cwd = thread.and_then(|t| t.cwd.clone());
        let folder = thread.and_then(|t| ws.thread_project_tint(t, cx));
        let text_size = px(ws.settings.appearance.transcript_font_size()) * 0.93;
        let answering = working.is_some();
        let empty = self.input.read(cx).value().trim().is_empty() && self.outbox.paths.is_empty() && self.outbox.saving == 0;
        let none_yet = items.is_empty();
        // The box says where to type: lit while the keys go to it, and plainly a box when not.
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let send_key = if self.cmd_enter { "⌘↩ to send" } else { "↩ to send, ⇧↩ for a new line" };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .h(px(40.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(crate::ui::agent_glyph(&prefs.agent, cx))
                    .child(div().text_color(theme.muted_foreground).child("Separate session, same project"))
                    .child(div().flex_1())
                    .child(crate::ui::icon_button("side-new", crate::assets::Lucide::SquarePen, "New side chat").on_click(cx.listener(|this, _, window, cx| {
                        this.thread_id = None;
                        let handle = this.focus_handle(cx);
                        handle.focus(window, cx);
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .id("side-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .p_3()
                            .gap_3()
                            .when(none_yet, |el| {
                                el.child(
                                    v_flex()
                                        .id("side-empty")
                                        .test_support()
                                        .pt_8()
                                        .px_4()
                                        .gap_2()
                                        .items_center()
                                        .text_center()
                                        .child(Icon::new(crate::assets::Lucide::MessagesSquare).text_color(theme.muted_foreground))
                                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("A question on the side"))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .child("Type it in the box below. The answer comes here, in a session of its own, and the main thread carries on undisturbed."),
                                        ),
                                )
                            })
                            .children(items.into_iter().enumerate().filter_map(|(i, item)| match item {
                                Item::User { text, images, .. } => Some(
                                    v_flex()
                                        .items_end()
                                        .gap_1()
                                        .when(!images.is_empty(), |el| {
                                            el.child(h_flex().gap_1().flex_wrap().justify_end().children(images.into_iter().map(|p| {
                                                img(PathBuf::from(p)).h(px(72.)).max_w(px(140.)).rounded(px(8.)).object_fit(ObjectFit::Contain)
                                            })))
                                        })
                                        .when(!text.trim().is_empty(), |el| {
                                            el.child(div().max_w(relative(0.85)).px_3().py_2().rounded(px(14.)).bg(theme.secondary).text_sm().child(text))
                                        })
                                        .into_any_element(),
                                ),
                                Item::Assistant { text } if !text.is_empty() => Some(
                                    div()
                                        .id(("side-answer", i))
                                        .test_support()
                                        .child(crate::md::keyed(SharedString::from(format!("side-{id}-{i}")), text, cwd.clone(), folder, text_size, true, cx))
                                        .into_any_element(),
                                ),
                                Item::Tool { title, detail, .. } => Some(
                                    h_flex()
                                        .gap_2()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(Icon::new(IconName::SquareTerminal).xsmall())
                                        .child(div().truncate().child(format!("{title} {detail}")))
                                        .into_any_element(),
                                ),
                                Item::Error { text } => Some(div().text_xs().text_color(crate::palette::red(cx)).child(text).into_any_element()),
                                // No pause here: the row says it, and the next message tries again.
                                Item::Limit { text, resets_at, scope } => Some(
                                    h_flex()
                                        .id(("side-limit", i))
                                        .test_support()
                                        .gap_2()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx))
                                        .child(Icon::new(crate::assets::Lucide::Gauge).xsmall().text_color(amber))
                                        .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(amber).child("Usage limit reached"))
                                        .child(div().min_w_0().truncate().child(crate::thread_view::limit_when(&scope, resets_at, now)))
                                        .into_any_element(),
                                ),
                                _ => None,
                            }))
                            .when_some(working, |el, word| el.child(div().id("side-working").test_support().text_color(theme.muted_foreground).child(word))),
                    ),
            )
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(
                    div()
                        .id("side-input")
                        .test_support()
                        .px_2()
                        .pt_1()
                        .pb(px(6.))
                        .rounded(px(12.))
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(if focused { theme.ring } else { theme.foreground.opacity(0.22) })
                        .cursor_text()
                        // In "send with ⌘↩" mode, send before the textarea turns ⌘↩ into a newline.
                        .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                            if this.cmd_enter && action.secondary && !action.shift {
                                cx.stop_propagation();
                                let input = this.input.clone();
                                this.submit(input, window, cx);
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                            let handle = this.input.read(cx).focus_handle(cx);
                            handle.focus(window, cx);
                        }))
                        .when(!self.outbox.paths.is_empty() || self.outbox.saving > 0, |el| {
                            let me = cx.entity().downgrade();
                            let remove = move |i: usize, _: &mut Window, cx: &mut App| {
                                let _ = me.update(cx, |this, cx| {
                                    if i < this.outbox.paths.len() {
                                        this.outbox.paths.remove(i);
                                    }
                                    cx.notify();
                                });
                            };
                            el.child(div().pt_1().pb(px(6.)).child(attachments::thumbnails(&self.outbox.paths, px(40.), self.outbox.saving > 0, remove, cx)))
                        })
                        .child(Textarea::new(&self.input).appearance(false).on_paste({
                            let me = cx.entity().downgrade();
                            move |item, window, cx| match attachments::pasted(item) {
                                Some(p) => me
                                    .update(cx, |this, cx| {
                                        let input = this.input.clone();
                                        attachments::paste(this, p, &input, window, cx)
                                    })
                                    .is_ok(),
                                None => false,
                            }
                        }))
                        .child(
                            h_flex()
                                .pl_1()
                                .gap_2()
                                .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(send_key))
                                .child(if answering {
                                    div()
                                        .id("side-stop")
                                        .test_support()
                                        .size(px(26.))
                                        .flex_none()
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(crate::palette::red(cx))
                                        .child(div().size(px(9.)).rounded(px(2.)).bg(rgb(0xFFFFFF)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(id) = this.thread_id.clone() {
                                                this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
                                            }
                                        }))
                                        .into_any_element()
                                } else {
                                    div()
                                        .id("side-send")
                                        .test_support()
                                        .size(px(26.))
                                        .flex_none()
                                        .rounded(px(8.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(if empty { theme.foreground.opacity(0.08) } else { theme.foreground })
                                        .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                                        .child(Icon::new(IconName::ArrowUp).small().text_color(if empty { theme.muted_foreground } else { theme.background }))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let input = this.input.clone();
                                            this.submit(input, window, cx);
                                        }))
                                        .into_any_element()
                                }),
                        ),
                ),
            )
    }
}
