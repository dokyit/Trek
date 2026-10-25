//! The composer's look (the approved "02 Composer" design): a near-white card with a hairline
//! and a soft shadow, 30 px round chips on a quiet fill, a blue send, and menus that float as
//! cards of their own. Every colour comes from the theme (or `palette`), so Night follows Paper.

use crate::palette;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::Effort;

/// A toolbar control's height (chips, `+`, mic, send).
pub const CONTROL: f32 = 30.;
/// The card's padding, all round.
pub const PAD: f32 = 13.;
/// The card's corner radius.
pub const RADIUS: f32 = 18.;
/// Between toolbar controls.
pub const GAP: f32 = 5.;
/// A chip's label, in px.
pub const LABEL: f32 = 12.;
/// A chip's icon, in px.
pub const ICON: f32 = 14.;
/// The kit's multi-line input pads its text by this much on each side; the composer takes it
/// off its own padding, so the text lines up with the `+` under it.
pub const INPUT_INSET: f32 = 10.;
/// The prompt's text size and line height in rems: 13 px on 20 at the default 14 px UI size,
/// and they grow with it.
pub const TEXT: f32 = 13. / 14.;

fn dark(cx: &App) -> bool {
    cx.theme().mode.is_dark()
}

/// A chip's resting fill (`#F4F4F4` on Paper).
pub fn chip_fill(cx: &App) -> Hsla {
    cx.theme()
        .foreground
        .opacity(if dark(cx) { 0.07 } else { 0.045 })
}

/// A chip under the pointer (`#F2F2F2` to `#EDEDED`).
pub fn chip_hover(cx: &App) -> Hsla {
    cx.theme()
        .foreground
        .opacity(if dark(cx) { 0.11 } else { 0.075 })
}

/// A chip that's on, or whose menu is open (`#EDEDED`).
pub fn pressed(cx: &App) -> Hsla {
    cx.theme()
        .foreground
        .opacity(if dark(cx) { 0.14 } else { 0.085 })
}

/// The card and its menus (`#FDFDFD`).
pub fn card_fill(cx: &App) -> Hsla {
    cx.theme().popover
}

/// The 1 px ring round the card and its menus (about 6 % black).
pub fn hairline(cx: &App) -> Hsla {
    cx.theme()
        .foreground
        .opacity(if dark(cx) { 0.1 } else { 0.06 })
}

/// The card's soft drop shadow (0 4 16, 3.5 % black).
pub fn card_shadow(cx: &App) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., if dark(cx) { 0.35 } else { 0.05 }),
        offset: point(px(0.), px(4.)),
        blur_radius: px(16.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// A menu's shadow: the card's, a touch firmer, so it reads over the card it overlaps.
pub fn menu_shadow(cx: &App) -> Vec<BoxShadow> {
    let a = if dark(cx) { 0.4 } else { 0.07 };
    vec![
        BoxShadow {
            color: hsla(0., 0., 0., a),
            offset: point(px(0.), px(4.)),
            blur_radius: px(16.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: hsla(0., 0., 0., a * 0.5),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        },
    ]
}

/// A menu over the composer: the card's fill, hairline and shadow, radius 14, rows inset 4.
pub fn surface(cx: &App) -> Div {
    v_flex()
        .p(px(4.))
        .rounded(px(14.))
        .bg(card_fill(cx))
        .border_1()
        .border_color(hairline(cx))
        .shadow(menu_shadow(cx))
        .text_size(px(13.))
        .text_color(cx.theme().foreground)
}

/// A row in a [`surface`]: 28 px, a quiet highlight with radius 7 when hovered or `active`.
pub fn row(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let fill = chip_fill(cx);
    h_flex()
        .id(id)
        .px(px(9.))
        .min_h(px(28.))
        .gap(px(8.))
        .rounded(px(7.))
        .text_size(px(13.))
        .cursor_pointer()
        .when(active, |el| el.bg(fill))
        .hover(move |s| s.bg(fill))
}

/// A kit button (a menu's trigger) as a toolbar chip: 30 px round pill on the chips' fill,
/// 12 px label. `content`: its icon and label, side by side.
pub fn chip_button(
    button: gpui_kit::component::button::Button,
    content: Vec<AnyElement>,
    cx: &App,
) -> gpui_kit::component::button::Button {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::ButtonVariants as _;
    let fill = chip_fill(cx);
    button
        .ghost()
        .small()
        .h(px(CONTROL))
        .px(px(11.))
        .rounded(px(CONTROL / 2.))
        .bg(fill)
        .child(
            h_flex()
                .gap(px(6.))
                .text_size(px(LABEL))
                .text_color(cx.theme().muted_foreground)
                .children(content),
        )
}

/// The send / ✓ blue circle, or a quiet one (✕, mic) with `fill`.
pub fn circle(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(CONTROL))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
}

/// The design's segmented control: a pill track on the chips' fill; the chosen option is a
/// near-white thumb (1 px ring, faint shadow) with a medium label, the others muted.
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: Vec<(T, &'static str)>,
    current: T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let thumb = card_fill(cx);
    let ring = theme.foreground.opacity(if dark(cx) { 0.12 } else { 0.07 });
    h_flex()
        .h(px(CONTROL))
        .p(px(2.))
        .gap(px(2.))
        .rounded_full()
        .bg(chip_fill(cx))
        .children(
            options
                .into_iter()
                .enumerate()
                .map(move |(i, (value, label))| {
                    let selected = value == current;
                    let on_pick = on_pick.clone();
                    div()
                        .id((id, i))
                        .test_support()
                        .h_full()
                        .px(px(11.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .text_size(px(LABEL))
                        .cursor_pointer()
                        .when(selected, |el| {
                            el.bg(thumb)
                                .border_1()
                                .border_color(ring)
                                .shadow(vec![BoxShadow {
                                    color: hsla(0., 0., 0., 0.05),
                                    offset: point(px(0.), px(1.)),
                                    blur_radius: px(2.),
                                    spread_radius: px(0.),
                                    inset: false,
                                }])
                                .text_color(theme.foreground)
                                .font_weight(FontWeight::MEDIUM)
                        })
                        .when(!selected, |el| {
                            el.text_color(theme.muted_foreground)
                                .hover(|s| s.text_color(theme.foreground))
                        })
                        .child(label)
                        .on_click(move |_, window, cx| on_pick(value, window, cx))
                }),
        )
        .into_any_element()
}

/// How many of the five bars an effort fills.
pub fn effort_level(effort: Effort) -> usize {
    match effort {
        Effort::Off => 0,
        Effort::Minimal | Effort::Low => 1,
        Effort::Medium => 2,
        Effort::High => 3,
        Effort::XHigh => 4,
        Effort::Max => 5,
    }
}

/// An effort as a five-bar signal: the first [`effort_level`] bars dark, the rest faint; all
/// five blue at Max.
pub fn effort_bars(effort: Effort, cx: &App) -> Div {
    let theme = cx.theme();
    let n = effort_level(effort);
    let on = if effort == Effort::Max {
        palette::send(cx)
    } else {
        theme.foreground.opacity(0.85)
    };
    let off = theme.foreground.opacity(if dark(cx) { 0.22 } else { 0.17 });
    h_flex()
        .flex_none()
        .h(px(12.))
        .items_end()
        .gap(px(1.5))
        .children(
            [4.5f32, 6.5, 8.5, 10.5, 12.]
                .into_iter()
                .enumerate()
                .map(move |(i, h)| {
                    div()
                        .w(px(2.))
                        .h(px(h))
                        .rounded_t(px(1.))
                        .bg(if i < n { on } else { off })
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::effort_level;
    use trek_core::Effort;

    #[test]
    fn effort_bars_climb_with_effort() {
        let levels: Vec<usize> = Effort::ALL.iter().map(|e| effort_level(*e)).collect();
        assert_eq!(levels, vec![0, 1, 1, 2, 3, 4, 5]);
    }
}
