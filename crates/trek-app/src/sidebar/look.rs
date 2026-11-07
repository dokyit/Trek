//! How the sidebar draws (Figma "11 Sidebar options", Option 1 · Inbox first): its own sizes and
//! the small pieces every row is made of. Every row starts with the same state column, so a
//! thread's state, a project's badge, Study's cap and a device's glyph sit in one column and the
//! titles in the next; and every row ends on the same right-hand column, so a project's count, a
//! thread's time, a device's count and Settled's line up. Sections are small labels, never badges,
//! so a section never looks like a project. Colours are the theme's and the palette's; what isn't
//! a token yet lives here, not in the shared theme.

use crate::palette;
use crate::scale::{radius, text};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// How far a row's text sits in from its rounded hover box, on both sides: headings and rows line
/// up on it at the left, and whatever ends a row (a count, a time) lines up on it at the right.
pub const ROW_INSET: f32 = 10.;
/// The state column every row starts with (a dot, a loader, a badge, a glyph).
pub const MARK_W: f32 = 20.;
pub const MARK_H: f32 = 16.;
/// Between the state column and the title.
pub const MARK_GAP: f32 = 5.;
/// Where a title starts in its row box: past the inset and the state column. A project's name,
/// its threads' titles, an Inbox row's second line and "Show more" all start here.
pub const TEXT_INSET: f32 = ROW_INSET + MARK_W + MARK_GAP;
/// A thread, a project's header, a device, a section label: one line.
pub const LINE_H: f32 = 28.;
/// Titles and names.
pub const TITLE_SIZE: f32 = text::BODY;
/// An Inbox row's second line, "Show more", notes.
pub const META_SIZE: f32 = text::LABEL;
/// Section labels, and the right-hand column's counts and times.
pub const LABEL_SIZE: f32 = text::LABEL;
/// "Mark all read".
pub const SMALL_SIZE: f32 = text::META;
/// A quiet title, against one that's at work, open or unread at full strength.
pub const QUIET: f32 = 0.75;
/// A settled title: history, never work.
pub const SETTLED: f32 = 0.6;
/// The Inbox's wash: the one filled block in the sidebar.
pub const INBOX_WASH: f32 = 0.06;
/// The Inbox's corners.
pub const INBOX_RADIUS: f32 = radius::BUTTON;
/// Between the nav rows and the Inbox (with the nav's and the list's own padding, 14).
pub const INBOX_TOP: f32 = 6.;
/// An Inbox row's hover box sits this far in from the Inbox's border; its text then starts (and
/// ends) where every other row's does.
pub const INBOX_ROW_MX: f32 = 2.;
/// The Inbox's border.
pub const INBOX_BORDER: f32 = 1.;
/// Between one group (a project, a device, the Inbox) and the next.
pub const GROUP_GAP: f32 = 4.;
/// Between what's above a section label and the label: the group's own gap, and this.
pub const SECTION_GAP: f32 = 12.;
/// A row's corners.
pub const ROW_RADIUS: f32 = radius::CONTROL;

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

/// What ends a row, in the right-hand column: a count, a time, a device's last word. Rows give it
/// the same right inset, so it ends at the same x in every one.
pub fn trailing(id: Option<String>, text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    let el = div()
        .flex_none()
        .text_size(px(LABEL_SIZE))
        .text_color(color)
        .child(text.into());
    match id {
        Some(id) => el
            .id(SharedString::from(id))
            .test_support()
            .into_any_element(),
        None => el.into_any_element(),
    }
}

/// A section label: what a group is for ("Projects 4"). Medium, muted, its count fainter beside
/// it; never a badge, so a section never reads as a project. Air above it, a row's height itself.
pub fn section_label(text: &str, count: Option<usize>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id(SharedString::from(format!(
            "section-{}",
            text.to_lowercase()
        )))
        .test_support()
        .pt(px(SECTION_GAP - GROUP_GAP))
        .child(
            h_flex()
                .mx_2()
                .px(px(ROW_INSET))
                .h(px(LINE_H))
                .gap(px(6.))
                .text_size(px(LABEL_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(text.to_string())
                .when_some(count.filter(|n| *n > 0), |el, n| {
                    el.child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme.muted_foreground.opacity(0.6))
                            .child(n.to_string()),
                    )
                }),
        )
}

/// The Inbox: what needs the user and what finished unread, on one soft card under the nav.
pub fn inbox_card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .mx_2()
        .mt(px(INBOX_TOP))
        .pt(px(2.))
        .pb(px(4.))
        .rounded(px(crate::fluent::surface(INBOX_RADIUS)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.foreground.opacity(INBOX_WASH))
}

/// The Inbox's header: its glyph, "Inbox" and how many it holds, and `action` ("Mark all read")
/// at the right, where every row's right-hand column is.
pub fn inbox_header(count: usize, action: Option<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    let inset = ROW_INSET - INBOX_BORDER;
    h_flex()
        .h(px(LINE_H))
        .pl(px(inset))
        .pr(px(inset))
        .gap(px(MARK_GAP))
        .child(
            h_flex()
                .flex_none()
                .w(px(MARK_W))
                .h(px(MARK_H))
                .justify_center()
                .child(
                    Icon::new(crate::assets::Lucide::Inbox)
                        .size(px(14.))
                        .text_color(theme.foreground),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .gap(px(6.))
                .text_size(px(LABEL_SIZE))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child("Inbox"),
                )
                .when(count > 0, |el| {
                    el.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(count.to_string()),
                    )
                }),
        )
        .children(action)
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

/// What a paired phone's row says in the right-hand column: connected now, else how long ago it
/// was last seen ("2m ago"), else since when it's been paired.
pub fn phone_seen(connected: bool, last_seen: Option<i64>, paired_at: i64) -> String {
    if connected {
        return "Connected".into();
    }
    let Some(at) = last_seen else {
        return format!("Paired {}", crate::time::relative(paired_at));
    };
    match crate::time::relative(at).as_str() {
        "now" => "Just now".into(),
        r if r.starts_with(|c: char| c.is_ascii_digit()) => format!("{r} ago"),
        r => r.to_string(),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: its `gpui_kit::*` brings GPUI's own `test` attribute in over the standard one.
    use super::{TEXT_INSET, phone_seen, study_title};

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

    #[test]
    fn a_phone_says_how_long_ago_in_few_words() {
        let now = trek_core::store::now_ms();
        assert_eq!(phone_seen(true, Some(now - 600_000), now), "Connected");
        assert_eq!(phone_seen(false, Some(now - 120_000), now), "2m ago");
        assert_eq!(phone_seen(false, Some(now), now), "Just now");
        assert_eq!(phone_seen(false, None, now - 3 * 86_400_000), "Paired 3d");
    }
}
