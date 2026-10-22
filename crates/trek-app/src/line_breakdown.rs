//! The card that opens over a diff's "+504 −105" wherever Trek shows one (the Git tool, the
//! changes card under a turn, the IDE's diff and AI cards): the lines by kind of file
//! (`trek_core::file_kind`), a row each for Code, Tests, Docs and the rest that changed, added in
//! green and removed in red, then the Total under a hairline. Opens on hover after a short delay
//! and closes when the pointer leaves.

use gpui_kit::base::HoverCard;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;
use trek_core::file_kind::LineBreakdown;

/// How long the pointer rests on the counts before the card opens, and lingers after leaving.
const OPEN_DELAY: Duration = Duration::from_millis(350);
const CLOSE_DELAY: Duration = Duration::from_millis(150);

/// `counts` (the "+a −r" as the caller draws it), opening `breakdown` on hover. `id` is the
/// counts' test id (`trek-dev control hover <id>` opens the card); the card is `<id>-card`, its
/// rows `<id>-<kind>` (`FileKind::key`) and `<id>-total`.
pub fn hover(
    id: impl Into<SharedString>,
    counts: impl IntoElement,
    breakdown: LineBreakdown,
) -> AnyElement {
    let id: SharedString = id.into();
    let card_id = id.clone();
    HoverCard::new(SharedString::from(format!("{id}-hover")))
        .anchor(Anchor::TopLeft)
        .open_delay(OPEN_DELAY)
        .close_delay(CLOSE_DELAY)
        .trigger(div().id(id).test_support().flex_none().child(counts))
        .content(move |_, _, cx| card(&card_id, &breakdown, cx))
        .into_any_element()
}

/// The card itself: label on the left, the counts in two right-aligned mono columns.
fn card(id: &str, breakdown: &LineBreakdown, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (green, red) = (crate::palette::emerald(cx), crate::palette::red(cx));
    let (added, removed) = breakdown.total();
    // Each column as wide as its longest number, so the rows line up.
    let digits = |n: u32| n.to_string().len() + 1;
    let width = |col: &dyn Fn(&trek_core::file_kind::KindLines) -> u32, total: u32| {
        let chars = breakdown
            .rows
            .iter()
            .map(|k| digits(col(k)))
            .chain([digits(total)])
            .max()
            .unwrap_or(2);
        px(chars as f32 * 7.4 + 2.)
    };
    let (wa, wr) = (width(&|k| k.added, added), width(&|k| k.removed, removed));
    let mono = theme.mono_font_family.clone();
    let row = |key: String, label: &'static str, a: u32, r: u32, strong: bool| {
        h_flex()
            .id(SharedString::from(key))
            .test_support()
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(64.))
                    .text_color(theme.foreground.opacity(if strong { 0.95 } else { 0.8 }))
                    .when(strong, |el| el.font_medium())
                    .child(label),
            )
            .child(
                div()
                    .w(wa)
                    .flex_none()
                    .text_right()
                    .font_family(mono.clone())
                    .text_color(green)
                    .child(format!("+{a}")),
            )
            .child(
                div()
                    .w(wr)
                    .flex_none()
                    .text_right()
                    .font_family(mono.clone())
                    .text_color(red)
                    .child(format!("−{r}")),
            )
    };
    let card = v_flex()
        .id(SharedString::from(format!("{id}-card")))
        .test_support()
        .mt(px(6.))
        .min_w(px(168.))
        .px(px(12.))
        .py(px(9.))
        .gap(px(5.))
        .rounded(px(10.))
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
        .text_size(px(12.))
        .children(breakdown.rows.iter().map(|k| {
            row(
                format!("{id}-{}", k.kind.key()),
                k.kind.label(),
                k.added,
                k.removed,
                false,
            )
        }))
        .child(div().h(px(1.)).my(px(2.)).bg(theme.foreground.opacity(0.1)))
        .child(row(format!("{id}-total"), "Total", added, removed, true));
    div()
        .id(SharedString::from(format!("{id}-popup")))
        .child(card)
}
