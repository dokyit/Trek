//! Side chat: a quick, separate conversation next to the main thread (same project, own session).

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::store::Item;

pub struct SideChatPanel {
    workspace: Entity<Workspace>,
    thread_id: Option<String>,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    /// `general.send_with_cmd_enter`, as last applied to the textarea.
    cmd_enter: bool,
    _subscriptions: Vec<Subscription>,
}

impl SideChatPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cmd_enter = workspace.read(cx).settings.general.send_with_cmd_enter;
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).submit_on_enter(!cmd_enter).placeholder("Ask on the side…"));
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
        Self { workspace, thread_id: None, input, scroll: ScrollHandle::new(), cmd_enter, _subscriptions: subs }
    }

    fn submit(&mut self, state: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let text = state.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        self.send(text, cx);
    }

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        if self.thread_id.is_none() {
            self.thread_id = self.workspace.update(cx, |ws, cx| ws.create_side_chat(cx));
        }
        if let Some(id) = self.thread_id.clone() {
            self.workspace.update(cx, |ws, cx| ws.send_to(&id, text, vec![], cx));
        }
    }
}

impl Render for SideChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let live = self.thread_id.as_ref().and_then(|id| ws.live.get(id));
        let items: Vec<Item> = live.map(|l| l.items.clone()).unwrap_or_default();
        let working = live.is_some_and(|l| l.turn_started.is_some());
        let prefs = ws.prefs();
        let id = self.thread_id.clone().unwrap_or_default();
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
                    .child(crate::ui::icon_button("side-new", crate::assets::Lucide::SquarePen, "New side chat").on_click(cx.listener(|this, _, _, cx| {
                        this.thread_id = None;
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
                            .when(items.is_empty(), |el| {
                                el.child(
                                    div()
                                        .pt_8()
                                        .text_sm()
                                        .text_center()
                                        .text_color(theme.muted_foreground)
                                        .child("Ask a quick question without derailing the main thread."),
                                )
                            })
                            .children(items.into_iter().enumerate().filter_map(|(i, item)| match item {
                                Item::User { text, .. } => Some(
                                    h_flex()
                                        .justify_end()
                                        .child(div().max_w(relative(0.85)).px_3().py_2().rounded(px(14.)).bg(theme.secondary).text_sm().child(text))
                                        .into_any_element(),
                                ),
                                Item::Assistant { text } if !text.is_empty() => {
                                    Some(div().text_sm().child(TextView::markdown(SharedString::from(format!("side-{id}-{i}")), text).selectable(true)).into_any_element())
                                }
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
                                _ => None,
                            }))
                            .when(working, |el| el.child(div().text_color(theme.muted_foreground).child("Working…"))),
                    ),
            )
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(
                    div()
                        .id("side-input")
                        .test_support()
                        .px_2()
                        .py_1()
                        .rounded(px(12.))
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.input)
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
                        .child(Textarea::new(&self.input).appearance(false)),
                ),
            )
    }
}
