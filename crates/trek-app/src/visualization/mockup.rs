//! Interface mockups: a tree of semantic UI parts drawn in Trek's own controls, optionally
//! inside a device's chrome. Images are placeholders by design: nothing is ever loaded.

use super::{Frame, Look, tone};
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use trek_core::visualization::{MockDevice, MockNode, MockNodeKind, SemanticTone, progress_fraction};

use crate::assets::Lucide;

struct Mock<'a> {
    frame: &'a Frame,
    look: Look,
    /// The window's background: what a device's screen shows.
    screen: Hsla,
    frames: Cell<usize>,
}

pub(super) fn mockup(nodes: &[MockNode], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let m = Mock { frame, look: look.clone(), screen: cx.theme().background, frames: Cell::new(0) };
    let framed = nodes.iter().any(|n| n.kind == MockNodeKind::Frame);
    let body = v_flex().w_full().gap_3().children(nodes.iter().map(|node| m.node(node, cx)));
    if framed {
        // Devices stand on the inset surface, like artboards.
        super::inset(&look)
            .id(frame.id("mockup", 0))
            .test_support()
            .w_full()
            .p(px(if frame.narrow() { 12. } else { 24. }))
            .bg(look.inset.opacity(1.).blend(look.fg.opacity(0.012)))
            .child(body.items_center())
            .into_any_element()
    } else {
        v_flex()
            .id(frame.id("mockup", 0))
            .test_support()
            .w_full()
            .p_3()
            .rounded(px(10.))
            .border_1()
            .border_color(look.hairline)
            .bg(m.screen)
            .child(body)
            .into_any_element()
    }
}

/// Up to two initials from a name.
fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).flat_map(char::to_uppercase).collect()
}

impl Mock<'_> {
    fn children(&self, node: &MockNode, cx: &App) -> Vec<AnyElement> {
        node.children.iter().map(|child| self.node(child, cx)).collect()
    }

    fn color(&self, node: &MockNode, cx: &App) -> Hsla {
        tone(node.tone, cx)
    }

    fn node(&self, node: &MockNode, cx: &App) -> AnyElement {
        let el = self.part(node, cx);
        // Controls keep their own width in a column instead of stretching across it.
        match node.kind {
            MockNodeKind::Button | MockNodeKind::Badge | MockNodeKind::Avatar => h_flex().child(el).into_any_element(),
            _ => el,
        }
    }

    fn part(&self, node: &MockNode, cx: &App) -> AnyElement {
        let look = &self.look;
        let text = node.text.clone().unwrap_or_default();
        match node.kind {
            MockNodeKind::Frame => self.device(node, cx),
            MockNodeKind::Row => h_flex().w_full().items_stretch().gap_2().flex_wrap().children(self.children(node, cx)).into_any_element(),
            MockNodeKind::Column => v_flex().w_full().gap_2().children(self.children(node, cx)).into_any_element(),
            MockNodeKind::Card => v_flex()
                .flex_1()
                .min_w(px(160.))
                .gap_2()
                .p_3()
                .rounded(px(10.))
                .border_1()
                .border_color(look.hairline)
                .bg(look.fg.opacity(if look.dark { 0.03 } else { 0.015 }))
                .when(node.text.is_some(), |el| el.child(div().font_medium().text_size(px(12.5)).text_color(look.fg).child(text)))
                .children(self.children(node, cx))
                .into_any_element(),
            MockNodeKind::List => v_flex()
                .w_full()
                .gap(px(6.))
                .when(node.text.is_some(), |el| el.child(div().px_1().text_size(px(11.)).font_medium().text_color(look.muted).child(text)))
                .child(
                    v_flex().w_full().rounded(px(9.)).border_1().border_color(look.hairline).overflow_hidden().children(node.children.iter().enumerate().map(|(i, child)| {
                        h_flex()
                            .w_full()
                            .gap_2()
                            .px_3()
                            .py(px(9.))
                            .when(i > 0, |el| el.border_t_1().border_color(look.grid))
                            .child(div().flex_1().min_w_0().child(self.node(child, cx)))
                            .when(matches!(child.kind, MockNodeKind::Text), |el| el.child(Icon::new(gpui_kit::component::IconName::ChevronRight).size(px(12.)).text_color(look.muted.opacity(0.7))))
                    })),
                )
                .into_any_element(),
            MockNodeKind::Nav => h_flex()
                .w_full()
                .h(px(42.))
                .px_3()
                .gap_3()
                .border_b_1()
                .border_color(look.hairline)
                .when(node.text.is_some(), |el| el.child(div().text_size(px(13.)).font_semibold().text_color(look.fg).child(text)))
                .child(div().flex_1())
                .children(node.children.iter().map(|child| match child.kind {
                    MockNodeKind::Text => div().text_size(px(12.)).text_color(look.muted).child(child.text.clone().unwrap_or_default()).into_any_element(),
                    _ => self.node(child, cx),
                }))
                .into_any_element(),
            MockNodeKind::Tabs => {
                let active = node.value.clone().or_else(|| node.children.first().and_then(|c| c.text.clone()));
                let accent = self.color(node, cx);
                h_flex()
                    .w_full()
                    .gap(px(18.))
                    .border_b_1()
                    .border_color(look.hairline)
                    .children(node.children.iter().map(|tab| {
                        let label = tab.text.clone().unwrap_or_default();
                        let on = active.as_ref() == Some(&label);
                        v_flex()
                            .gap(px(7.))
                            .pt(px(4.))
                            .child(div().text_size(px(12.)).when(on, |el| el.font_medium()).text_color(if on { look.fg } else { look.muted }).child(label))
                            .child(div().h(px(2.)).rounded_t(px(2.)).bg(if on { accent } else { gpui_kit::transparent_black() }))
                    }))
                    .into_any_element()
            }
            MockNodeKind::Text => div().text_size(px(12.)).line_height(relative(1.45)).text_color(look.fg.opacity(0.88)).child(text).into_any_element(),
            MockNodeKind::Metric => v_flex()
                .flex_1()
                .min_w(px(104.))
                .gap(px(2.))
                .child(div().text_size(px(11.)).text_color(look.muted).child(text))
                .child(div().text_size(px(18.)).font_semibold().text_color(if node.tone.is_some() { self.color(node, cx) } else { look.fg }).child(node.value.clone().unwrap_or_default()))
                .into_any_element(),
            MockNodeKind::Button => {
                let color = self.color(node, cx);
                let primary = matches!(node.tone, None | Some(SemanticTone::Accent));
                let neutral = node.tone == Some(SemanticTone::Neutral);
                h_flex()
                    .flex_none()
                    .justify_center()
                    .px(px(12.))
                    .h(px(30.))
                    .rounded(px(8.))
                    .text_size(px(12.))
                    .font_medium()
                    .when(primary, |el| el.bg(color).text_color(gpui_kit::white()).shadow(vec![BoxShadow { color: color.opacity(0.3), offset: point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false }]))
                    .when(neutral, |el| el.border_1().border_color(look.fg.opacity(0.16)).text_color(look.fg))
                    .when(!primary && !neutral, |el| el.bg(color.opacity(0.12)).text_color(color))
                    .child(text)
                    .into_any_element()
            }
            MockNodeKind::Badge => {
                let color = self.color(node, cx);
                h_flex()
                    .flex_none()
                    .gap(px(5.))
                    .px(px(8.))
                    .h(px(20.))
                    .rounded_full()
                    .bg(color.opacity(0.12))
                    .text_size(px(10.5))
                    .font_medium()
                    .text_color(color)
                    .child(div().size(px(5.)).rounded_full().bg(color))
                    .child(text)
                    .into_any_element()
            }
            MockNodeKind::Divider => div().w_full().h(px(1.)).bg(look.hairline).into_any_element(),
            MockNodeKind::Input => {
                let typed = node.value.clone();
                v_flex()
                    .w_full()
                    .gap(px(4.))
                    .when(typed.is_some(), |el| el.child(div().text_size(px(11.)).text_color(look.muted).child(text.clone())))
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(32.))
                            .px(px(10.))
                            .gap(px(8.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(if typed.is_some() { self.color(node, cx).opacity(0.6) } else { look.fg.opacity(0.14) })
                            .bg(self.screen)
                            .child(Icon::new(gpui_kit::component::IconName::Search).size(px(13.)).text_color(look.muted))
                            .child(match typed {
                                Some(v) => div().text_size(px(12.)).text_color(look.fg).child(v),
                                None => div().text_size(px(12.)).text_color(look.muted.opacity(0.8)).child(text),
                            }),
                    )
                    .into_any_element()
            }
            MockNodeKind::Toggle => {
                let on = node.value.as_deref() == Some("on");
                let color = self.color(node, cx);
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .child(div().text_size(px(12.5)).text_color(look.fg).child(text))
                    .child(
                        div()
                            .flex_none()
                            .w(px(34.))
                            .h(px(20.))
                            .p(px(2.))
                            .rounded_full()
                            .bg(if on { color } else { look.fg.opacity(0.16) })
                            .flex()
                            .when(on, |el| el.justify_end())
                            .child(div().size(px(16.)).rounded_full().bg(gpui_kit::white()).shadow(vec![BoxShadow { color: gpui_kit::black().opacity(0.25), offset: point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }])),
                    )
                    .into_any_element()
            }
            MockNodeKind::Progress => {
                let value = node.value.clone().unwrap_or_default();
                let frac = progress_fraction(&value).unwrap_or(0.) as f32 * self.frame.arrive(0, 1);
                let color = self.color(node, cx);
                v_flex()
                    .w_full()
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_size(px(12.)).text_color(look.fg).child(text))
                            .child(div().font_family(look.mono.clone()).text_size(px(11.)).text_color(look.muted).child(if value.ends_with('%') { value.clone() } else { format!("{value}%") })),
                    )
                    .child(div().w_full().h(px(6.)).rounded_full().bg(look.fg.opacity(0.08)).child(div().h_full().w(relative(frac)).rounded_full().bg(color)))
                    .into_any_element()
            }
            MockNodeKind::Avatar => {
                let color = match node.tone {
                    Some(_) => self.color(node, cx),
                    None => crate::palette::series(text.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)), cx),
                };
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded_full()
                    .bg(look.gap.blend(color.opacity(0.28)))
                    .border_1()
                    .border_color(color.opacity(0.45))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.5))
                    .font_semibold()
                    .text_color(look.fg)
                    .child(initials(&text))
                    .into_any_element()
            }
            MockNodeKind::Image => {
                let (w, h): (Option<f32>, f32) = match node.value.as_deref() {
                    Some("square") => (Some(140.), 140.),
                    Some("tall") => (Some(130.), 190.),
                    _ => (None, 132.),
                };
                v_flex()
                    .when_some(w, |el, w| el.w(px(w)).flex_none())
                    .when(w.is_none(), |el| el.w_full())
                    .h(px(h))
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(look.grid)
                    .bg(linear_gradient(160., linear_color_stop(look.fg.opacity(0.07), 0.), linear_color_stop(look.fg.opacity(0.025), 1.)))
                    .child(Icon::new(Lucide::Image).size(px(20.)).text_color(look.muted.opacity(0.7)))
                    .when(node.text.is_some(), |el| el.child(div().px_2().truncate().text_size(px(11.)).text_color(look.muted).child(text)))
                    .into_any_element()
            }
        }
    }

    fn device(&self, node: &MockNode, cx: &App) -> AnyElement {
        let look = &self.look;
        let ix = self.frames.get();
        self.frames.set(ix + 1);
        let title = node.text.clone();
        let content = v_flex().w_full().gap_3().children(self.children(node, cx));
        let shadow = vec![
            BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.5 } else { 0.12 }), offset: point(px(0.), px(12.)), blur_radius: px(32.), spread_radius: px(-8.), inset: false },
            BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.4 } else { 0.06 }), offset: point(px(0.), px(2.)), blur_radius: px(6.), spread_radius: px(0.), inset: false },
        ];
        let lights = || {
            h_flex().gap(px(6.)).children([0xFF5F57u32, 0xFEBC2E, 0x28C840].map(|c| div().size(px(10.)).rounded_full().bg(Hsla::from(rgb(c)).opacity(if look.dark { 0.85 } else { 1. }))))
        };
        let device = node.device.unwrap_or(MockDevice::Window);
        let el = match device {
            MockDevice::Phone | MockDevice::Tablet => {
                let phone = device == MockDevice::Phone;
                let status = h_flex()
                    .w_full()
                    .h(px(if phone { 34. } else { 26. }))
                    .px(px(if phone { 24. } else { 18. }))
                    .justify_between()
                    .child(div().text_size(px(12.)).font_semibold().text_color(look.fg).child("9:41"))
                    .child(
                        h_flex()
                            .gap(px(4.))
                            .child(Icon::new(Lucide::SignalHigh).size(px(12.)).text_color(look.fg))
                            .child(Icon::new(Lucide::Wifi).size(px(12.)).text_color(look.fg))
                            .child(Icon::new(gpui_kit::component::IconName::BatteryFull).size(px(14.)).text_color(look.fg)),
                    );
                let screen = v_flex()
                    .relative()
                    .w_full()
                    .min_h(px(if phone { 440. } else { 300. }))
                    .rounded(px(if phone { 30. } else { 14. }))
                    .bg(self.screen)
                    .overflow_hidden()
                    .when(phone, |el| el.child(div().absolute().top(px(10.)).left_0().right_0().flex().justify_center().child(div().w(px(86.)).h(px(24.)).rounded_full().bg(gpui_kit::black()))))
                    .child(status)
                    .child(
                        v_flex()
                            .flex_1()
                            .w_full()
                            .px(px(16.))
                            .pt(px(8.))
                            .pb(px(12.))
                            .gap_3()
                            .when_some(title, |el, title| el.child(div().text_size(px(20.)).font_semibold().text_color(look.fg).child(title)))
                            .child(content),
                    )
                    .when(phone, |el| el.child(h_flex().w_full().h(px(20.)).justify_center().items_center().child(div().w(px(108.)).h(px(4.)).rounded_full().bg(look.fg.opacity(0.35)))));
                div()
                    .w_full()
                    .max_w(px(if phone { 296. } else { 560. }))
                    .p(px(if phone { 7. } else { 10. }))
                    .rounded(px(if phone { 38. } else { 24. }))
                    .border_1()
                    .border_color(look.fg.opacity(if look.dark { 0.16 } else { 0.14 }))
                    .bg(if look.dark { look.fg.opacity(0.07) } else { gpui_kit::black().opacity(0.82) })
                    .shadow(shadow)
                    .child(screen)
            }
            MockDevice::Browser | MockDevice::Window => {
                let browser = device == MockDevice::Browser;
                let bar = h_flex()
                    .w_full()
                    .h(px(if browser { 38. } else { 32. }))
                    .px(px(12.))
                    .gap(px(12.))
                    .border_b_1()
                    .border_color(look.hairline)
                    .bg(look.fg.opacity(if look.dark { 0.04 } else { 0.03 }))
                    .child(lights())
                    .when(browser, |el| {
                        el.child(Icon::new(gpui_kit::component::IconName::ChevronLeft).size(px(13.)).text_color(look.muted.opacity(0.7))).child(
                            h_flex().flex_1().justify_center().child(
                                h_flex()
                                    .w_full()
                                    .max_w(px(340.))
                                    .h(px(24.))
                                    .px(px(10.))
                                    .gap(px(6.))
                                    .justify_center()
                                    .rounded(px(7.))
                                    .bg(look.fg.opacity(0.06))
                                    .child(Icon::new(Lucide::Lock).size(px(10.)).text_color(look.muted))
                                    .child(div().truncate().text_size(px(11.)).text_color(look.muted).child(title.clone().unwrap_or_else(|| "app".into()))),
                            ),
                        )
                        .child(div().w(px(46.)))
                    })
                    .when(!browser, |el| {
                        el.child(div().flex_1().text_center().truncate().text_size(px(12.)).font_medium().text_color(look.fg.opacity(0.8)).child(title.clone().unwrap_or_default())).child(div().w(px(46.)))
                    });
                v_flex()
                    .w_full()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(look.fg.opacity(if look.dark { 0.14 } else { 0.12 }))
                    .bg(self.screen)
                    .shadow(shadow)
                    .overflow_hidden()
                    .child(bar)
                    .child(div().w_full().p(px(16.)).child(content))
            }
        };
        div().id(self.frame.id("frame", ix)).test_support().w_full().flex().justify_center().child(el).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn avatars_show_initials() {
        assert_eq!(super::initials("Ada Lovelace"), "AL");
        assert_eq!(super::initials("grace"), "G");
        assert_eq!(super::initials("Jean-Luc Picard Third"), "JP");
    }
}
