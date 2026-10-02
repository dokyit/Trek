//! Shared visual primitives so every surface uses the same quiet, consistent styling.

use crate::palette;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;

/// Square ghost icon button with a tooltip.
pub fn icon_button(id: impl Into<ElementId>, icon: impl Into<Icon>, tooltip: &'static str) -> Button {
    Button::new(id).ghost().small().icon(icon).tooltip(tooltip)
}

/// A plain sidebar row: icon, label, optional shortcut hint.
pub fn nav_row(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    hint: Option<&'static str>,
    active: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .mx_1()
        .px_2()
        .h(px(32.))
        .gap_2()
        .rounded(px(8.))
        .cursor_pointer()
        .text_sm()
        .when(active, |el| el.bg(theme.list_active))
        .when(!active, |el| el.hover(|s| s.bg(theme.list_hover)))
        .child(icon.small().text_color(theme.muted_foreground))
        .child(div().flex_1().child(label))
        .when_some(hint, |el, h| el.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(h)))
}

pub fn status_text(text: &'static str, color: Hsla) -> AnyElement {
    div().text_xs().font_weight(FontWeight::MEDIUM).text_color(color).child(text).into_any_element()
}

/// Two-letter project badge with a stable tint (T3-style).
pub fn monogram(name: &str, cx: &App) -> impl IntoElement {
    let letters: String = {
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        match words.as_slice() {
            [] => "·".into(),
            [one] => one.chars().take(2).collect(),
            [a, b, ..] => format!("{}{}", a.chars().next().unwrap(), b.chars().next().unwrap()),
        }
    }
    .to_uppercase();
    let hash = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let hue = (hash % 360) as f32 / 360.;
    let dark = cx.theme().mode.is_dark();
    div()
        .flex_none()
        .h(px(16.))
        .min_w(px(20.))
        .px(px(3.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.5))
        .font_weight(FontWeight::BOLD)
        .bg(hsla(hue, 0.35, if dark { 0.22 } else { 0.88 }, 1.))
        .text_color(hsla(hue, 0.55, if dark { 0.75 } else { 0.35 }, 1.))
        .child(letters)
}

/// Small agent mark at the end of a row. Generic glyphs, not vendor logos.
pub fn agent_glyph(agent: &AgentId, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let (icon, color): (Icon, Hsla) = match agent {
        AgentId::ClaudeCode => (Icon::new(crate::assets::Lucide::Asterisk), rgb(0xD97757).into()),
        AgentId::Codex => (Icon::new(IconName::SquareTerminal), muted),
        AgentId::OpenCode => (Icon::new(IconName::Frame), muted),
        AgentId::Droid => (Icon::new(IconName::Bot), muted),
        AgentId::Acp(_) => (Icon::new(IconName::Bot), muted),
        AgentId::Direct(_) => (Icon::new(IconName::Cpu), palette::sky(cx)),
    };
    icon.xsmall().text_color(color)
}

/// Thin vertical divider between composer chips.
pub fn divider(cx: &App) -> impl IntoElement {
    div().w(px(1.)).h(px(14.)).bg(cx.theme().border)
}

/// Segmented control: a muted track with the selected option raised.
pub fn segmented<T: Copy + PartialEq + 'static, L: Into<SharedString>>(
    id: &'static str,
    options: Vec<(T, L)>,
    current: T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    h_flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(8.))
        .bg(theme.muted)
        .children(options.into_iter().enumerate().map(move |(i, (value, label))| {
            let selected = value == current;
            let on_pick = on_pick.clone();
            div()
                .id((id, i))
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_sm()
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.popover).text_color(theme.foreground).shadow_xs())
                .when(!selected, |el| el.text_color(theme.muted_foreground).hover(|s| s.text_color(theme.foreground)))
                .child(label.into())
                .on_click(move |_, window, cx| on_pick(value, window, cx))
        }))
        .into_any_element()
}

/// A rounded group of settings rows separated by hairlines.
pub fn group(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let n = rows.len();
    gpui_kit::component::v_flex()
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.sidebar)
        .children(rows.into_iter().enumerate().map(move |(i, r)| {
            div().px_4().when(i + 1 < n, |el| el.border_b_1().border_color(theme.border)).child(r)
        }))
        .into_any_element()
}
