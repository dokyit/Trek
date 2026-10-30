//! The Browser's own surfaces for what pages ask: the bar above the page (a dialog, a permission,
//! a link for another app; `asks.rs` words them) and the sheet a popup shows in over the page
//! that opened it (Google's sign-in window, say).

use super::asks::{self, AskKind, Reply};
use super::{BrowserPanel, Tab, host_of, tool_button};
use crate::assets::Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl BrowserPanel {
    /// The front tab's oldest ask, as a bar above its page (`None`: it asks nothing).
    pub(super) fn ask_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ask = self.asks.iter().find(|a| a.tab == self.active)?.clone();
        let theme = cx.theme().clone();
        let ember = crate::palette::ember(cx);
        if let AskKind::Prompt { default, .. } = &ask.kind
            && self.prompt.as_ref().is_none_or(|(id, ..)| *id != ask.id)
        {
            let input = cx.new(|cx| InputState::new(window, cx));
            input.update(cx, |s, cx| {
                s.set_value(default.clone(), window, cx);
                s.focus(window, cx);
                s.select_all(window, cx);
            });
            // Enter answers, as the dialog's default button would.
            let sub = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let text = input.read(cx).value().to_string();
                    this.answer_front_ask(Reply::Text(text), cx);
                }
            });
            self.prompt = Some((ask.id, input, sub));
        }
        let icon = match &ask.kind {
            AskKind::Alert(_) | AskKind::Confirm(_) | AskKind::Prompt { .. } => Lucide::MessageSquare,
            AskKind::Camera | AskKind::CameraAndMicrophone => Lucide::Camera,
            AskKind::Microphone => Lucide::Mic,
            AskKind::Location => Lucide::Compass,
            AskKind::OpenApp { .. } => Lucide::SquareArrowOutUpRight,
        };
        let (no, yes) = ask.buttons();
        let prompt = matches!(ask.kind, AskKind::Prompt { .. })
            .then(|| self.prompt.as_ref().map(|(_, input, _)| input.clone()))
            .flatten();
        let reply_yes = prompt.clone();
        let buttons = h_flex()
            .flex_none()
            .gap(px(6.))
            .children(no.map(|label| {
                Button::new("browser-ask-no")
                    .xsmall()
                    .outline()
                    .label(label)
                    .on_click(cx.listener(|this, _, _, cx| this.answer_front_ask(Reply::No, cx)))
            }))
            .child(
                Button::new("browser-ask-yes")
                    .xsmall()
                    .primary()
                    .label(yes)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let reply = match &reply_yes {
                            Some(input) => Reply::Text(input.read(cx).value().to_string()),
                            None => Reply::Yes,
                        };
                        this.answer_front_ask(reply, cx);
                    })),
            );
        Some(
            v_flex()
                .id("browser-ask")
                .px(px(12.))
                .py(px(7.))
                .gap(px(6.))
                .text_xs()
                .bg(ember.opacity(0.08))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(Icon::new(icon).xsmall().text_color(ember))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .child(ask.headline()),
                        )
                        .child(buttons),
                )
                .children(ask.message().filter(|m| !m.trim().is_empty()).map(|m| {
                    div()
                        .id("browser-ask-message")
                        .pl(px(20.))
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .whitespace_normal()
                        .child(m.to_string())
                }))
                .children(prompt.map(|input| div().pl(px(20.)).child(Input::new(&input).small())))
                .into_any_element(),
        )
    }

    /// Answer the front tab's oldest ask (the one the bar shows).
    pub(super) fn answer_front_ask(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if let Some(ix) = self.asks.iter().position(|a| a.tab == self.active) {
            self.answer_ask(ix, reply, cx);
        }
    }

    /// Popup `tab` as a sheet: the size its page asked for, centred where its opener's page
    /// was, under a header with its title, its site, and a way to make it a tab or close it.
    /// The opener's own view is hidden meanwhile (a native view can't be drawn over), so the
    /// sheet sits on a plain backdrop.
    pub(super) fn sheet_view(&self, tab: &Tab, size: (f64, f64), cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let area = self.area.get().size;
        let area = if area.width > px(0.) {
            (f64::from(f32::from(area.width)), f64::from(f32::from(area.height)))
        } else {
            (size.0 + 48., size.1 + 84.)
        };
        let (w, h) = asks::sheet_size(size, area);
        let id = tab.id;
        let secure = tab.page.url.starts_with("https://");
        let site = host_of(&tab.page.url).unwrap_or_default();
        let title = tab.page.label();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.foreground.opacity(0.05))
            .child(
                v_flex()
                    .id("browser-sheet")
                    .w(px(w as f32))
                    .h(px(h as f32 + 36.))
                    .max_w_full()
                    .max_h_full()
                    .rounded(px(10.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(36.))
                            .pl(px(12.))
                            .pr(px(4.))
                            .gap(px(8.))
                            .text_xs()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                Icon::new(if secure { Lucide::Lock } else { Lucide::AppWindow })
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(6.))
                                    .child(div().min_w_0().truncate().child(title))
                                    .when(!site.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .flex_none()
                                                .text_color(theme.muted_foreground)
                                                .child(site),
                                        )
                                    }),
                            )
                            .child(
                                tool_button("browser-sheet-tab", Lucide::Maximize2, "Show as a tab")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(tab) = this.tab_mut(id) {
                                            tab.sheet = None;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                tool_button("browser-sheet-close", Lucide::X, "Close").on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.close_tab(id, window, cx)
                                    }),
                                ),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(Self::page_view(tab, cx))),
            )
            .into_any_element()
    }
}
