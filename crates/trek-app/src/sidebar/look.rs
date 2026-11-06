//! How the sidebar draws (Figma "07 Sidebar"): its own sizes and the small pieces every row is
//! made of. Every row starts with the same state column, so a thread's state, a project's badge,
//! Study's cap and a device's glyph sit in one column and the titles in the next; sections are
//! small labels, never badges, so a section never looks like a project. Colours are the theme's
//! and the palette's; what isn't a token yet lives here, not in the shared theme.

use crate::palette;
use crate::scale::{radius, text};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// How far a row's text sits in from its rounded hover box: headings and rows line up on it.
pub const ROW_INSET: f32 = 10.;
/// The state column every row starts with (a dot, a loader, a badge, a glyph).
pub const MARK_W: f32 = 20.;
pub const MARK_H: f32 = 16.;
/// Between the state column and the title.
pub const MARK_GAP: f32 = 5.;
/// Where a title starts in its row box: past the inset and the state column. A project's name,
/// its threads' titles, a card's second line and "Show more" all start here.
pub const TEXT_INSET: f32 = ROW_INSET + MARK_W + MARK_GAP;
/// A quiet thread, a project's header, a device: one line.
pub const LINE_H: f32 = 28.;
/// Titles and names.
pub const TITLE_SIZE: f32 = text::BODY;
/// A card's second line, "Show more", notes.
pub const META_SIZE: f32 = text::LABEL;
/// Section labels and small counts.
pub const LABEL_SIZE: f32 = text::META;
/// A quiet title, against one that's open or unread at full strength.
pub const QUIET: f32 = 0.84;
/// A settled title: history, never work.
pub const SETTLED: f32 = 0.6;
/// The Needs you block's amber wash: the only filled block in the sidebar.
pub const NEEDS_WASH: f32 = 0.07;
/// Its count's pill.
pub const NEEDS_PILL: f32 = 0.16;
/// Air above a section label.
pub const SECTION_TOP: f32 = 14.;
/// Between one project's group and the next.
pub const GROUP_GAP: f32 = 6.;
/// A row's corners.
pub const ROW_RADIUS: f32 = radius::BUTTON;
/// The Needs you block's corners.
pub const TRAY_RADIUS: f32 = radius::CARD;

/// What a row's state column shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Quiet: nothing.
    None,
    /// Its turn runs, or it waits on its sub-agents: the ember loader, held still.
    Working,
    /// It waits on the user (an approval, a question, a plan, a cut-off turn): amber.
    NeedsYou,
    /// Its turn or its checks failed: red.
    Failed,
    /// It finished and the user hasn't looked: emerald.
    Unread,
    /// Work still running in the background (a dev server, a watcher, sub-agents): sky, smaller.
    Background,
    /// A Study session: the cap, in Study's indigo.
    Study,
    Pinned,
    Snoozed,
}

/// The state column for `mark`, `id`'d when there's something in it.
pub fn mark(id: Option<SharedString>, mark: Mark, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let dot = |size: f32, color: Hsla| div().flex_none().size(px(size)).rounded_full().bg(color);
    let icon = |icon: Icon, size: f32, color: Hsla| icon.size(px(size)).text_color(color);
    let inner = match mark {
        Mark::None => None,
        Mark::Working => Some(
            icon(
                Icon::new(crate::assets::Lucide::LoaderCircle),
                13.,
                palette::working(cx),
            )
            .into_any_element(),
        ),
        Mark::NeedsYou => Some(dot(7., theme.warning).into_any_element()),
        Mark::Failed => Some(dot(7., theme.red).into_any_element()),
        Mark::Unread => Some(dot(7., theme.success).into_any_element()),
        Mark::Background => Some(dot(6., palette::sky(cx)).into_any_element()),
        Mark::Study => Some(
            icon(
                Icon::new(crate::assets::Lucide::GraduationCap),
                14.,
                palette::indigo(cx),
            )
            .into_any_element(),
        ),
        Mark::Pinned => Some(
            icon(
                Icon::new(crate::assets::Lucide::Pin),
                12.,
                theme.muted_foreground,
            )
            .into_any_element(),
        ),
        Mark::Snoozed => Some(
            icon(
                Icon::new(crate::assets::Lucide::Clock),
                12.,
                theme.muted_foreground,
            )
            .into_any_element(),
        ),
    };
    let column = h_flex()
        .flex_none()
        .w(px(MARK_W))
        .h(px(MARK_H))
        .items_center()
        .justify_center()
        .children(inner);
    match id {
        Some(id) if mark != Mark::None => column.id(id).test_support().into_any_element(),
        _ => column.into_any_element(),
    }
}

/// A section label: what a group is for. Small, semi-bold, muted, with a count; a chevron when
/// it folds (right while folded). Never a badge: that's what keeps a section from reading as a
/// project.
pub fn section_label(
    text: &str,
    count: Option<usize>,
    chevron: Option<bool>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id(SharedString::from(format!(
            "section-{}",
            text.to_lowercase()
        )))
        .mx_2()
        .pl(px(ROW_INSET))
        .pr(px(8.))
        .pt(px(SECTION_TOP))
        .pb(px(6.))
        .gap(px(6.))
        .text_size(px(LABEL_SIZE))
        .line_height(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted_foreground)
        .child(text.to_string())
        .when_some(count.filter(|n| *n > 0), |el, n| {
            el.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child(n.to_string()),
            )
        })
        .child(div().flex_1())
        .when_some(chevron, |el, folded| {
            el.child(
                Icon::new(if folded {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
        })
        .test_support()
}

/// Needs you's own header, inside its block: amber words and an amber count.
pub fn needs_header(count: usize, cx: &App) -> Div {
    let amber = palette::needs_you(cx);
    h_flex()
        .pl(px(6.))
        .pr(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .gap(px(6.))
        .text_size(px(LABEL_SIZE))
        .line_height(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(amber)
        .child("Needs you")
        .when(count > 0, |el| {
            el.child(
                div()
                    .h(px(15.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(cx.theme().warning.opacity(NEEDS_PILL))
                    .text_size(px(text::META))
                    .child(count.to_string()),
            )
        })
}

/// The Needs you block: everything that waits on the user, on one amber wash.
pub fn needs_tray(cx: &App) -> Div {
    v_flex()
        .mx_2()
        .mt(px(4.))
        .px(px(4.))
        .pb(px(4.))
        .rounded(px(crate::fluent::surface(TRAY_RADIUS)))
        .bg(cx.theme().warning.opacity(NEEDS_WASH))
}

/// A device's glyph in the state column (a Mac's laptop, the phone), with its reach dot over its
/// corner, ringed in the sidebar's colour so it reads apart from the glyph.
pub fn device_glyph(phone: bool, reachable: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let dot = if reachable {
        theme.success
    } else {
        theme.muted_foreground.opacity(0.6)
    };
    div()
        .relative()
        .flex_none()
        .w(px(MARK_W))
        .h(px(MARK_H))
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(if phone {
                crate::assets::Lucide::Smartphone
            } else {
                crate::assets::Lucide::Laptop
            })
            .size(px(15.))
            .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .absolute()
                .right(px(0.))
                .bottom(px(-1.))
                .size(px(8.))
                .rounded_full()
                .border_1()
                .border_color(theme.sidebar)
                .bg(dot),
        )
}

/// A quiet block on the titles' line: the empty inbox, a search with nothing found.
pub fn empty_note(id: &'static str, title: &str, body: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .test_support()
        .mx_2()
        .pl(px(TEXT_INSET))
        .pr(px(14.))
        .py(px(14.))
        .gap(px(3.))
        .child(
            div()
                .text_size(px(TITLE_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground.opacity(0.9))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(META_SIZE))
                .text_color(theme.muted_foreground)
                .child(body.to_string()),
        )
}

/// A Study session's title without its kind ("Learn · Transformers" is "Transformers", Learn),
/// when it starts with one; else the title as it is, and no kind.
pub fn study_title(title: &str) -> (&str, Option<&'static str>) {
    for kind in [
        trek_core::study::Kind::Learn,
        trek_core::study::Kind::Practice,
    ] {
        let label = kind.label();
        if let Some(rest) = title
            .strip_prefix(label)
            .and_then(|r| r.strip_prefix(" · "))
            .filter(|r| !r.trim().is_empty())
        {
            return (rest, Some(label));
        }
    }
    (title, None)
}

/// Whether `t` is a Study session: it works in the study folder (`workspace::study`).
pub fn is_study(t: &trek_core::store::Thread, study_dir: &std::path::Path) -> bool {
    t.project_id.is_none() && t.cwd.as_deref() == Some(study_dir)
}

/// What a paired phone's row says: connected now, else when it was last seen.
pub fn phone_seen(connected: bool, last_seen: Option<i64>, paired_at: i64) -> String {
    if connected {
        return "Connected".into();
    }
    let Some(at) = last_seen else {
        return format!("Paired {}", crate::time::relative(paired_at));
    };
    match crate::time::relative(at).as_str() {
        "now" => "Seen just now".into(),
        r if r.starts_with(|c: char| c.is_ascii_digit()) => format!("Seen {r} ago"),
        r => format!("Seen {r}"),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: its `gpui_kit::*` brings GPUI's own `test` attribute in over the standard one.
    use super::{TEXT_INSET, study_title};

    #[test]
    fn a_session_title_gives_its_kind_back() {
        assert_eq!(
            study_title("Learn · the chain rule"),
            ("the chain rule", Some("Learn"))
        );
        assert_eq!(
            study_title("Practice · Transformers, heavy math"),
            ("Transformers, heavy math", Some("Practice"))
        );
        // Renamed by the user, or nothing after the kind: the title as it is.
        assert_eq!(study_title("My own name"), ("My own name", None));
        assert_eq!(study_title("Learn · "), ("Learn · ", None));
        assert_eq!(study_title("Learning Rust"), ("Learning Rust", None));
    }

    #[test]
    fn titles_line_up_past_the_state_column() {
        // The nav rows (`ui::nav_row`) put their labels 35 pt into the row: so does every row here.
        assert_eq!(TEXT_INSET, 35.);
    }
}
