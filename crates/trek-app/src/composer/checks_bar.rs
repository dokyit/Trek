//! The bar above the composer when a pull request the thread opened has failing checks: what's
//! failing, and the way to hand it to the agent (`workspace::pr_checks`).

use super::Composer;
use crate::fluent;
use crate::palette;
use crate::scale::{radius, text};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl Composer {
    pub(super) fn checks_bar(
        &mut self,
        id: &str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.workspace.read(cx).pr_checks_bar_up(id) {
            return None;
        }
        let offer = self.workspace.read(cx).pr_checks_offer(id)?;
        let theme = cx.theme().clone();
        let tint = palette::failed(cx);
        let action = |key: &'static str, label: &'static str, strong: bool| {
            div()
                .id(key)
                .test_support()
                .flex_none()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(fluent::control(radius::CONTROL)))
                .cursor_pointer()
                .text_color(if strong {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .when(strong, |el| el.font_medium())
                .hover(|s| {
                    s.bg(theme.foreground.opacity(0.06))
                        .text_color(theme.foreground)
                })
                .child(label)
        };
        let detail = offer.detail();
        let whole = SharedString::from(detail.clone());
        let title = if offer.stopped {
            "CI still failing"
        } else {
            "CI failed"
        };
        let (send, dismiss) = {
            let (ws, id) = (self.workspace.clone(), id.to_string());
            let (ws2, id2) = (ws.clone(), id.clone());
            (
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    ws.update(cx, |ws, cx| ws.pr_checks_send(&id, cx))
                },
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    ws2.update(cx, |ws, cx| ws.pr_checks_dismiss(&id2, cx))
                },
            )
        };
        let url = offer.url.clone();
        // Its place in the stack of bars above the prompt.
        let corners = self.bar_corners();
        Some(
            v_flex()
                .id("checks-bar")
                .test_support()
                .mx(px(12.))
                .rounded_t(px(corners))
                .border_1()
                .border_b_0()
                .border_color(tint.opacity(0.28))
                .bg(tint.opacity(0.07))
                .text_size(px(text::BODY))
                .child(
                    h_flex()
                        .h(px(36.))
                        .pl(px(12.))
                        .pr(px(6.))
                        .gap(px(8.))
                        .child(if offer.sending {
                            gpui_kit::component::spinner::Spinner::new()
                                .xsmall()
                                .color(theme.muted_foreground)
                                .into_any_element()
                        } else {
                            Icon::new(IconName::CircleX)
                                .small()
                                .text_color(tint)
                                .into_any_element()
                        })
                        .child(
                            div()
                                .id("checks-title")
                                .test_support()
                                .flex_none()
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .child(
                            div()
                                .id("checks-detail")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .when(!compact, |el| el.child(detail))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(whole.clone())
                                        .build(window, cx)
                                }),
                        )
                        .when(offer.sending, |el| {
                            el.child(
                                div()
                                    .id("checks-sending")
                                    .test_support()
                                    .flex_none()
                                    .px(px(8.))
                                    .text_color(theme.muted_foreground)
                                    .child("Reading the log…"),
                            )
                        })
                        .when(!offer.sending, |el| {
                            el.child(action("checks-send", "Send to agent", true).on_click(send))
                                .child(
                                    action("checks-open", "View on GitHub", false)
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                                .child(action("checks-dismiss", "Dismiss", false).on_click(dismiss))
                        }),
                )
                .into_any_element(),
        )
    }
}
