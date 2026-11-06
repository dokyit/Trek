//! An open tool group as a timeline: a rail with a node per step, the files and pages a step used
//! as source chips under it, a spinner on what's running and a tick where the group truly
//! finished. This module decides those things from what a transcript item records, in plain
//! functions, and has the marks and chips the transcript and the working bar draw them with.
//!
//! What Trek doesn't record isn't drawn: there are no dead-end branches or side lanes, as nothing
//! says one step led to another, and a duration only shows where it was measured.

use crate::activity::{Place, ToolKind, placement};
use crate::assets::Lucide;
use crate::fluent;
use crate::palette;
use crate::scale::text;
use crate::workspace::Workspace;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::Duration;
use trek_core::orchestrate as orch;
use trek_core::store::{Item, ToolStatus};

/// The rail's centre, from the column's left edge.
pub const RAIL_X: f32 = 8.;
/// Where a step's text starts, 32 px past the rail.
pub const TEXT_X: f32 = 40.;
/// A step's line of text.
pub const LINE: f32 = 20.;
/// From one step's line to the next's, when it has nothing under it.
pub const PITCH: f32 = 33.;
/// The gap under a step's line.
pub const GAP: f32 = PITCH - LINE;
/// Above a step's source chips, and under them to the next step.
pub const CHIPS_TOP: f32 = 7.;
pub const CHIPS_BOTTOM: f32 = 14.;
const CHIP: f32 = 26.;
const CHIP_GAP: f32 = 6.;
pub const TEXT_SIZE: f32 = text::BODY;
const NODE: f32 = 7.;
const SEARCH: f32 = 11.;
const CROSS: f32 = 8.;
const CHEVRON: f32 = 10.;
const TICK: f32 = 14.5;
const TICK_CHECK: f32 = 9.;
const SPINNER: f32 = 13.;
const SPINNER_DOT: f32 = 6.;
/// The rail stops short of a mark: this far from the centre of the one above and the one below.
const RAIL_CLEAR_ABOVE: f32 = 8.;
const RAIL_CLEAR_BELOW: f32 = 7.;
/// Chips shown under a step; the rest are counted as "+N more".
pub const CHIP_LIMIT: usize = 4;
/// The most of a live thought's latest line that's worth keeping: one line shows, truncated.
const THOUGHT_LINE: usize = 160;

/// How a timeline's group stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    /// A step is running, or the turn is still at this group.
    Live,
    /// Every step ended and the agent went on from it.
    Finished,
    /// It ended some other way: an error, a limit, an interruption, or a transcript that just
    /// stops. Nothing says it got where it was going, so it gets no tick.
    Stopped,
}

/// What comes after a group in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Nothing,
    /// The agent went on: an answer, the turn's end, another call of its own, a sub-agent.
    Moved,
    /// The turn died there, or something else cut in: a switch to another agent, whose later words
    /// say nothing of how this one's work went.
    Stopped,
}

/// What follows the group whose last item is `last`. Thoughts, the calls that start a sub-agent,
/// passing notices and the rows Trek adds itself sit between rows without ending one, and prove
/// nothing either: a PR's card follows the `gh pr create` in the group whether or not the agent
/// gets any further. A sub-agent's row does count, as only the agent's own call makes one.
pub fn after_group(items: &[Item], last: usize) -> After {
    for item in items.iter().skip(last + 1) {
        match item {
            Item::Reasoning { .. } => {}
            Item::Tool { id, .. } if inserted_by_trek(id) => {}
            Item::Tool { .. } if placement(item) == Place::Hidden => {}
            Item::Notice { .. } if !trek_core::rewind::ends_turn(item) => {}
            Item::Assistant { .. } | Item::TurnEnd { .. } | Item::Tool { .. } => {
                return After::Moved;
            }
            _ => return After::Stopped,
        }
    }
    After::Nothing
}

/// Whether a tool row's id marks it as a PR, release or run card or a linked thread's row, which
/// Trek puts in the transcript when it sees something happen.
fn inserted_by_trek(id: &str) -> bool {
    crate::pr_card::of_row(id).is_some() || trek_core::lineage::of_row(id).is_some()
}

pub fn group_state(any_running: bool, after: After, turn_running: bool) -> GroupState {
    if any_running {
        return GroupState::Live;
    }
    match after {
        After::Moved => GroupState::Finished,
        After::Nothing if turn_running => GroupState::Live,
        After::Nothing | After::Stopped => GroupState::Stopped,
    }
}

/// What an open group's header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Words<'a> {
    Thinking,
    ThoughtFor(u64),
    Summary(&'a str),
}

pub fn header_words(state: GroupState, summary: &str, thought_secs: Option<u64>) -> Words<'_> {
    match (state, thought_secs) {
        (GroupState::Live, _) => Words::Thinking,
        (_, Some(secs)) => Words::ThoughtFor(secs),
        (_, None) => Words::Summary(summary),
    }
}

/// How long a group's thoughts took in all, given each one's measured time (`None`: not measured,
/// as for any transcript read back). Unknown unless all of them were: part of the time, shown as
/// the time, would be an estimate. Tool calls aren't timed, so it's their thinking alone, and a
/// group with no thoughts has no figure.
pub fn thought_total(spans: impl Iterator<Item = Option<Duration>>) -> Option<u64> {
    let mut total = Duration::ZERO;
    let mut any = false;
    for span in spans {
        total += span?;
        any = true;
    }
    Some(total.as_secs()).filter(|secs| any && *secs > 0)
}

/// Whether the group starting at `first` is the first thing its turn did, so that the turn's clock
/// is also the group's. Later in a turn the group's own start isn't recorded, and the turn's clock
/// would overstate it.
pub fn opens_turn(items: &[Item], first: usize) -> bool {
    let before = items[..first.min(items.len())]
        .iter()
        .rev()
        .find(|item| !matches!(item, Item::Reasoning { text } if text.trim().is_empty()));
    matches!(before, None | Some(Item::User { aside: false, .. }))
}

/// The mark on a step's node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Dot,
    Search,
    Failed,
    Running,
}

pub fn mark(kind: ToolKind, status: ToolStatus) -> Mark {
    live_mark(
        matches!(kind, ToolKind::Search | ToolKind::WebSearch),
        status == ToolStatus::Running,
        matches!(status, ToolStatus::Failed | ToolStatus::Denied),
    )
}

pub fn live_mark(search: bool, running: bool, failed: bool) -> Mark {
    if running {
        Mark::Running
    } else if failed {
        Mark::Failed
    } else if search {
        Mark::Search
    } else {
        Mark::Dot
    }
}

/// A step's words, as the transcript's tool rows put them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wording<'a> {
    pub label: &'a str,
    pub detail: &'a str,
    /// The detail is what the agent wrote (a command, a pattern), in monospace.
    pub code: bool,
}

/// `activity`: a running sub-agent's progress, which stands in for the detail.
pub fn wording<'a>(title: &'a str, detail: &'a str, activity: Option<&'a str>) -> Wording<'a> {
    let subagent = title == "Subagent";
    let ours = orch::tool_label(title).or_else(|| orch::spawn_failed_label(title));
    Wording {
        label: match ours {
            Some(label) => label,
            None if subagent => detail,
            None => title,
        },
        detail: match activity {
            Some(activity) => activity,
            None if subagent || ours.is_some() => "",
            None => detail,
        },
        code: activity.is_none(),
    }
}

/// Something a step used, for its chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    File(&'a str),
    Page(&'a str),
}

/// The chips under a step and how many didn't fit: on the stack, as these are worked out for each
/// step on screen in each frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Chips<'a> {
    pub shown: [Option<Source<'a>>; CHIP_LIMIT],
    pub more: usize,
}

impl<'a> Chips<'a> {
    fn push(&mut self, source: Source<'a>) {
        if self.shown.contains(&Some(source)) {
            return;
        }
        match self.shown.iter_mut().find(|s| s.is_none()) {
            Some(slot) => *slot = Some(source),
            None => self.more += 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shown[0].is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item = Source<'a>> + '_ {
        self.shown.iter().flatten().copied()
    }
}

/// What a tool call used, from what its item records.
pub fn chips<'a>(
    kind: ToolKind,
    status: ToolStatus,
    detail: &'a str,
    output: &'a str,
) -> Chips<'a> {
    let mut chips = Chips::default();
    sources(kind, status, detail, output, &mut |source| match source {
        Some(source) => chips.push(source),
        None => chips = Chips::default(),
    });
    chips
}

/// What a step's "+N more" stands for, a line each, for its tooltip: `chips` is on the stack and
/// doesn't keep them, so they're worked out again when that's shown.
fn more_names(item: &Item) -> String {
    let Item::Tool {
        title,
        detail,
        output,
        status,
        ..
    } = item
    else {
        return String::new();
    };
    let (mut chips, mut more) = (Chips::default(), Vec::new());
    let kind = crate::activity::tool_kind(title);
    sources(kind, *status, detail, output, &mut |source| match source {
        Some(source) => {
            let counted = chips.more;
            chips.push(source);
            if chips.more > counted {
                more.push(match source {
                    Source::File(whole) | Source::Page(whole) => whole,
                });
            }
        }
        None => {
            chips = Chips::default();
            more.clear();
        }
    });
    more.join("\n")
}

/// Goes through what a tool call used: the files named in a read or a change, the address of a
/// fetch, and the files a search's output lists when that's all it lists. A call that failed used
/// nothing, and a web search records no pages apart from its prose. `None`: forget those so far.
fn sources<'a>(
    kind: ToolKind,
    status: ToolStatus,
    detail: &'a str,
    output: &'a str,
    add: &mut impl FnMut(Option<Source<'a>>),
) {
    if matches!(status, ToolStatus::Failed | ToolStatus::Denied) {
        return;
    }
    match kind {
        ToolKind::Read | ToolKind::Edit => detail_paths(detail, add),
        ToolKind::Fetch => {
            if let Some(url) = detail
                .split_whitespace()
                .find(|w| w.starts_with("https://") || w.starts_with("http://"))
            {
                add(Some(Source::Page(url)));
            }
        }
        ToolKind::Search => search_hits(output, add),
        _ => {}
    }
}

/// The files a read's or a change's detail names. Codex lists every file a change touched ("a.rs,
/// b.rs"), but one path can hold a ", " too, and with no location to give an ACP agent's detail is
/// its description or query (`tool_detail`), which names nothing. So it's a list only if every part
/// reads as a path, and an absolute path is listed only with others: after one, a relative part is
/// the rest of its name.
fn detail_paths<'a>(detail: &'a str, add: &mut impl FnMut(Option<Source<'a>>)) {
    let detail = detail.trim();
    let parts = detail.split(", ").map(str::trim).filter(|p| !p.is_empty());
    let absolute = parts.clone().next().is_some_and(is_absolute);
    if parts
        .clone()
        .all(|p| reads_as_path(p) && (!absolute || is_absolute(p)))
    {
        parts.for_each(|p| add(Some(Source::File(p))));
    } else if reads_as_path(detail) {
        add(Some(Source::File(detail)));
    }
}

fn is_absolute(path: &str) -> bool {
    path.starts_with(['/', '\\'])
        || path.starts_with("~/")
        || matches!(path.as_bytes(), [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic())
}

/// Whether a read's or a change's detail, or a part of one, reads as a path: it has a separator or
/// a file extension. Words with spaces between them are prose, unless they start at a root and end
/// in a file's name; a relative path with a space in it stays plain text, as it can't be told from
/// a sentence that mentions a file.
fn reads_as_path(path: &str) -> bool {
    if path.contains("://") || path.contains(['*', '?', '|', '<', '>', '"', '\n']) {
        return false;
    }
    if path.contains(char::is_whitespace) {
        return is_absolute(path) && has_extension(path);
    }
    path.contains(['/', '\\']) || has_extension(path)
}

fn has_extension(path: &str) -> bool {
    file_name(path).rsplit_once('.').is_some_and(|(_, ext)| {
        ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// The files a search's output lists. Its output is whatever the agent's tool printed, so it counts
/// only if every line reads as a path or `path:match`; at the first that doesn't there are no
/// chips, rather than some that may not be files. Most such outputs stop at their first line.
fn search_hits<'a>(output: &'a str, add: &mut impl FnMut(Option<Source<'a>>)) {
    let mut previous = None;
    let lines = output.lines().map(str::trim).filter(|l| !l.is_empty());
    for (n, line) in lines.enumerate() {
        // Claude's search leads with a count.
        if n == 0
            && line.starts_with("Found ")
            && (line.ends_with(" file") || line.ends_with(" files"))
        {
            continue;
        }
        let Some(path) = hit_path(line) else {
            add(None);
            return;
        };
        // A file's matches come together, each on its line.
        if previous != Some(path) {
            add(Some(Source::File(path)));
            previous = Some(path);
        }
    }
}

/// The path that starts a line of search output, if one does.
fn hit_path(line: &str) -> Option<&str> {
    // A drive's colon isn't the one before the match.
    let start = match line.as_bytes() {
        [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic() => 2,
        _ => 0,
    };
    let end = line[start..].find(':').map_or(line.len(), |n| start + n);
    let path = &line[..end];
    if path.is_empty() || path.contains(char::is_whitespace) {
        return None;
    }
    (has_extension(path) || path.contains(['/', '\\'])).then_some(path)
}

/// The last component of a path, whichever separator it uses.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// A page's address as a chip writes it: `https://www.docs.rs/serde/` → "docs.rs/serde". Trek
/// doesn't record a page's title.
pub fn page_label(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.strip_prefix("www.")
        .unwrap_or(rest)
        .trim_end_matches('/')
}

/// The padding under a step: its gap to the next one, none at the end.
pub fn step_bottom(has_chips: bool, below: bool) -> f32 {
    match (below, has_chips) {
        (false, _) => 0.,
        (true, true) => CHIPS_BOTTOM,
        (true, false) => GAP,
    }
}

/// A thought still streaming, as one line: where it has got to, without its markdown.
pub fn thought_line(text: &str) -> Option<&str> {
    let line = text
        .lines()
        .rev()
        .map(|l| l.trim().trim_matches('*').trim_start_matches('#').trim())
        .find(|l| !l.is_empty())?;
    Some(match line.char_indices().nth(THOUGHT_LINE) {
        Some((end, _)) => &line[..end],
        None => line,
    })
}

/// The dim rails and nodes. The text beside them stays in the theme's muted ink, which reads on
/// the opaque window; the design's light grey doesn't.
fn rail_colour(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.16)
}

fn node_colour(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(0.36)
}

/// Which sides of a step the rail runs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rail {
    pub above: bool,
    pub below: bool,
}

fn rail_above(cx: &App) -> Div {
    div()
        .absolute()
        .left(px(RAIL_X - 0.5))
        .w(px(1.))
        .top_0()
        .h(px(LINE / 2. - RAIL_CLEAR_BELOW))
        .bg(rail_colour(cx))
}

fn rail_below(cx: &App) -> Div {
    div()
        .absolute()
        .left(px(RAIL_X - 0.5))
        .w(px(1.))
        .top(px(LINE / 2. + RAIL_CLEAR_ABOVE))
        .bottom_0()
        .bg(rail_colour(cx))
}

/// A step's frame, with its rail and mark: what's added to it is its text column. It stretches
/// with what's in it (a call's output, a thought's text) and the rail with it.
pub fn step(rail: Rail, mark: Mark, clock: f32, still: bool, cx: &App) -> Div {
    div()
        .relative()
        .w_full()
        .pl(px(TEXT_X))
        .when(rail.above, |el| el.child(rail_above(cx)))
        .when(rail.below, |el| el.child(rail_below(cx)))
        .child(marker(mark, clock, still, cx))
}

fn marker(mark: Mark, clock: f32, still: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let (size, inner) = match mark {
        Mark::Dot => (
            NODE,
            div()
                .size_full()
                .rounded_full()
                .bg(node_colour(cx))
                .into_any_element(),
        ),
        Mark::Search => (
            SEARCH,
            Icon::new(IconName::Search)
                .size(px(SEARCH))
                .text_color(theme.muted_foreground)
                .into_any_element(),
        ),
        Mark::Failed => (
            CROSS,
            Icon::new(Lucide::X)
                .size(px(CROSS))
                .text_color(palette::red(cx))
                .into_any_element(),
        ),
        Mark::Running => (SPINNER, spinner(clock, still, cx).into_any_element()),
    };
    centred_on_rail(size).child(inner)
}

fn centred_on_rail(size: f32) -> Div {
    div()
        .absolute()
        .left(px(RAIL_X - size / 2.))
        .top(px((LINE - size) / 2.))
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
}

/// The arc round a running step's node. It's turned by the view's own clock, which is ticking
/// anyway while a turn runs, so it asks for no animation frames of its own; `still` (reduced
/// motion, or the window behind another), it holds where it is.
fn spinner(clock: f32, still: bool, cx: &App) -> Div {
    let angle = if still {
        0.
    } else {
        clock * std::f32::consts::TAU
    };
    div()
        .relative()
        .size(px(SPINNER))
        .child(
            div()
                .absolute()
                .left(px((SPINNER - SPINNER_DOT) / 2.))
                .top(px((SPINNER - SPINNER_DOT) / 2.))
                .size(px(SPINNER_DOT))
                .rounded_full()
                .bg(node_colour(cx)),
        )
        .child(
            Icon::new(Lucide::LoaderCircle)
                .size(px(SPINNER))
                .text_color(cx.theme().muted_foreground)
                .transform(Transformation::rotate(radians(angle))),
        )
}

/// The open chevron on the rail, at the head of a timeline.
pub fn chevron(cx: &App) -> Div {
    centred_on_rail(CHEVRON).child(
        Icon::new(IconName::ChevronDown)
            .size(px(CHEVRON))
            .text_color(cx.theme().muted_foreground),
    )
}

/// The foot of a group that truly finished (`GroupState::Finished`).
pub fn finished(cx: &App) -> Div {
    div()
        .relative()
        .w_full()
        .h(px(LINE))
        .pl(px(TEXT_X))
        .flex()
        .items_center()
        .text_size(px(TEXT_SIZE))
        .text_color(cx.theme().foreground)
        .child(rail_above(cx))
        .child(
            centred_on_rail(TICK)
                .rounded_full()
                .bg(palette::send(cx))
                .child(
                    Icon::new(Lucide::Check)
                        .size(px(TICK_CHECK))
                        .text_color(gpui_kit::white()),
                ),
        )
        .child("Finished thinking")
}

/// A chip's look: 4 px corners on a 1 px stroke, whatever it holds.
fn chip_look(chip: Stateful<Div>, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    chip.flex_none()
        .min_w_0()
        .max_w(px(280.))
        .h(px(CHIP))
        .pl(px(8.))
        .pr(px(9.))
        .gap(px(6.))
        .rounded(px(fluent::CONTROL_RADIUS))
        .border_1()
        .border_color(fluent::stroke(theme.border, cx))
        .bg(theme.background)
        .text_color(theme.muted_foreground)
        .text_size(px(TEXT_SIZE))
        .font_family(theme.font_family.clone())
}

fn source_chip(id: impl Into<ElementId>, source: Source<'_>, cx: &App) -> Stateful<Div> {
    let (chip, whole) = match source {
        // The badge comes out 13 px square.
        Source::File(path) => (
            crate::file_icon::chip(id, path, file_name(path).to_string(), px(12.75), cx),
            path,
        ),
        Source::Page(url) => (
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .child(
                    Icon::new(IconName::Globe)
                        .size(px(13.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(page_label(url).to_string()),
                ),
            url,
        ),
    };
    // The name alone is on the chip: its whole path is a pointer away.
    let whole = SharedString::from(whole.to_string());
    chip_look(chip, cx).tooltip(move |window, cx| Tooltip::new(whole.clone()).build(window, cx))
}

/// The chips under the step for item `ix` of `thread`: its sources, then "+N more", which names the
/// rest when pointed at. Only a step with some to name pays for the handles that takes, and the
/// names themselves are worked out only then.
pub fn source_chips(
    chips: &Chips<'_>,
    ix: usize,
    workspace: &Entity<Workspace>,
    thread: &str,
    cx: &App,
) -> Option<Div> {
    (!chips.is_empty()).then(|| {
        h_flex()
            .flex_wrap()
            .mt(px(CHIPS_TOP))
            .gap(px(CHIP_GAP))
            .children(
                chips
                    .iter()
                    .enumerate()
                    .map(|(n, source)| source_chip(("tool-file", ix * CHIP_LIMIT + n), source, cx)),
            )
            .when(chips.more > 0, |el| {
                let (workspace, thread) = (workspace.clone(), thread.to_string());
                el.child(
                    div()
                        .id(("tool-file-more", ix))
                        .flex_none()
                        .text_size(px(TEXT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("+{} more", chips.more))
                        .tooltip(move |window, cx| {
                            let names = workspace
                                .read(cx)
                                .live
                                .get(&thread)
                                .and_then(|live| live.items.get(ix))
                                .map(more_names)
                                .unwrap_or_default();
                            Tooltip::new(SharedString::from(names)).build(window, cx)
                        }),
                )
            })
    })
}

/// A file on a step's own line, in the working bar: its rows have a fixed height to animate by, so
/// its sources can't go under them there. `ink`: the text, at full foreground on a running step.
pub fn inline_chip(
    id: impl Into<ElementId>,
    path: &str,
    label: SharedString,
    ink: Hsla,
    cx: &App,
) -> Stateful<Div> {
    chip_look(crate::file_icon::chip(id, path, label, px(12.), cx), cx)
        .flex_shrink(1.)
        .max_w_full()
        .h(px(LINE))
        .pl(px(5.))
        .pr(px(6.))
        .gap(px(5.))
        .text_size(px(text::BODY))
        .text_color(ink)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Named, so the standard attribute wins over the kit's `test`, which the glob brings in.
    use std::prelude::v1::test;
    use std::time::Duration;

    fn tool(title: &str, status: ToolStatus) -> Item {
        Item::Tool {
            id: "t".into(),
            title: title.into(),
            detail: String::new(),
            output: String::new(),
            status,
        }
    }

    fn user(aside: bool) -> Item {
        Item::User {
            text: "go".into(),
            images: vec![],
            at: None,
            resume: None,
            aside,
        }
    }

    fn said() -> Item {
        Item::Assistant { text: "ok".into() }
    }

    fn thought(text: &str) -> Item {
        Item::Reasoning { text: text.into() }
    }

    fn files<'a>(chips: &Chips<'a>) -> Vec<&'a str> {
        chips
            .iter()
            .map(|s| match s {
                Source::File(f) | Source::Page(f) => f,
            })
            .collect()
    }

    #[test]
    fn a_read_shows_its_files_up_to_the_limit_then_counts_the_rest() {
        let c = chips(
            ToolKind::Read,
            ToolStatus::Done,
            "a.rs, b.rs, c.rs, d.rs, e.rs, f.rs, g.rs",
            "",
        );
        assert_eq!(files(&c), ["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert_eq!(c.more, 3);
        let c = chips(ToolKind::Edit, ToolStatus::Running, "a.rs, b.rs", "");
        assert_eq!(files(&c), ["a.rs", "b.rs"]);
        assert_eq!(c.more, 0, "all of them fit");
        let c = chips(
            ToolKind::Read,
            ToolStatus::Done,
            "a.rs, b.rs, c.rs, d.rs",
            "",
        );
        assert_eq!(c.more, 0, "exactly the limit");
    }

    #[test]
    fn a_detail_is_split_only_when_every_part_is_a_path() {
        let read = |detail| chips(ToolKind::Read, ToolStatus::Done, detail, "");
        // A real list of what a change touched, relative or absolute.
        let c = chips(ToolKind::Edit, ToolStatus::Done, "src/a.rs, b.rs", "");
        assert_eq!(files(&c), ["src/a.rs", "b.rs"]);
        assert_eq!(
            files(&read("/repo/a.rs, /repo/My Docs/b.rs")),
            ["/repo/a.rs", "/repo/My Docs/b.rs"]
        );
        assert_eq!(
            files(&read(r"C:\repo\a.rs, D:\b.rs")),
            [r"C:\repo\a.rs", r"D:\b.rs"]
        );
        // One path, with a comma or a space in it: after an absolute start, a relative part is the
        // rest of its name.
        for path in [
            r"C:\Users\Toby, Jr\a.rs",
            "/home/me/My Documents/a.rs",
            "/repo/a, b.rs",
            "Cargo.toml",
            "src/lib",
            "~/notes.md",
        ] {
            assert_eq!(files(&read(path)), [path], "{path}");
            assert_eq!(read(path).more, 0);
        }
        // Prose, and whatever else an agent with no location sends: no chips, so the row keeps
        // the words.
        for prose in [
            "Read the config",
            "Fix the bug, then test it",
            "Look in src/a.rs for it",
            "src/a.rs, and the rest",
            "docs/My Notes.md",
            "/compact the thread",
            "src/**/*.rs",
            "https://docs.rs/serde",
            "serde",
            "",
        ] {
            assert!(read(prose).is_empty(), "{prose:?}");
        }
    }

    #[test]
    fn more_names_what_the_chips_count_and_nothing_else() {
        let call = |title: &str, detail: &str, output: &str, status| Item::Tool {
            id: "t".into(),
            title: title.into(),
            detail: detail.into(),
            output: output.into(),
            status,
        };
        let six = "a.rs, b.rs, c.rs, d.rs, src/e.rs, f.rs";
        assert_eq!(
            more_names(&call("Edit", six, "", ToolStatus::Done)),
            "src/e.rs\nf.rs"
        );
        assert_eq!(chips(ToolKind::Edit, ToolStatus::Done, six, "").more, 2);
        assert_eq!(
            more_names(&call(
                "Search",
                "x",
                "a.rs\nb.rs\nc.rs\nd.rs\ne.rs",
                ToolStatus::Done
            )),
            "e.rs"
        );
        // A search whose output turns out not to be all paths has no chips to be more than.
        assert_eq!(
            more_names(&call(
                "Search",
                "x",
                "a.rs\nb.rs\nc.rs\nd.rs\ne.rs\nand more results",
                ToolStatus::Done
            )),
            ""
        );
        assert_eq!(more_names(&call("Edit", "a.rs", "", ToolStatus::Done)), "");
        assert_eq!(more_names(&said()), "");
    }

    #[test]
    fn a_failed_call_used_nothing() {
        for status in [ToolStatus::Failed, ToolStatus::Denied] {
            let c = chips(ToolKind::Read, status, "a.rs", "");
            assert!(c.is_empty());
            let c = chips(ToolKind::Fetch, status, "https://a.dev", "");
            assert!(c.is_empty());
        }
    }

    #[test]
    fn only_what_a_call_records_makes_a_chip() {
        let c = chips(
            ToolKind::Fetch,
            ToolStatus::Done,
            "https://docs.rs/serde",
            "body",
        );
        assert_eq!(c.shown[0], Some(Source::Page("https://docs.rs/serde")));
        // A fetch with no address, a command and a web search name nothing to show.
        assert!(chips(ToolKind::Fetch, ToolStatus::Done, "the docs", "").is_empty());
        assert!(chips(ToolKind::Command, ToolStatus::Done, "cat a.rs", "a.rs").is_empty());
        assert!(
            chips(
                ToolKind::WebSearch,
                ToolStatus::Done,
                "serde",
                "See https://docs.rs/serde for more"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_search_counts_only_output_that_is_all_paths() {
        let search = |output| chips(ToolKind::Search, ToolStatus::Done, "pattern", output);
        // A file's matches make one chip.
        let c =
            search("src/main.rs:3:    let cfg = parse_args();\nsrc/main.rs:9: x\nsrc/cli.rs:12:y");
        assert_eq!(files(&c), ["src/main.rs", "src/cli.rs"]);
        // Claude's list of files, with its count.
        let c = search("Found 2 files\nsrc/a.rs\nsrc/b.rs");
        assert_eq!(files(&c), ["src/a.rs", "src/b.rs"]);
        assert_eq!(
            files(&search("src/routes.rs: 12 matches")),
            ["src/routes.rs"]
        );
        assert_eq!(
            files(&search("C:\\repo\\src\\a.rs:4:let x")),
            ["C:\\repo\\src\\a.rs"]
        );
        // Past the limit.
        let c = search("a.rs\nb.rs\nc.rs\nd.rs\ne.rs\nf.rs");
        assert_eq!(c.more, 2);
        // Anything else isn't read as files, however much of it looks like some.
        for prose in [
            "No matches found",
            "src/a.rs:1:x\nand 3 more results",
            "12: let x = 1;",
            "src/a.rs:1:x\n--\nsrc/b.rs:2:y",
            "The middleware can't read the body",
            "Error: nothing here",
            "",
        ] {
            assert!(search(prose).is_empty(), "{prose:?}");
        }
    }

    #[test]
    fn names_and_addresses_read_short() {
        assert_eq!(file_name("src/auth/router.ts"), "router.ts");
        assert_eq!(file_name("C:\\repo\\main.rs"), "main.rs");
        assert_eq!(file_name("src/"), "src");
        assert_eq!(page_label("https://www.docs.rs/serde/"), "docs.rs/serde");
        assert_eq!(page_label("http://a.dev"), "a.dev");
    }

    #[test]
    fn a_group_finishes_only_when_the_agent_went_on_from_it() {
        let turn_end = Item::TurnEnd {
            at: 1,
            took_secs: 2,
        };
        let done = tool("Read", ToolStatus::Done);
        let finished = |after: After| group_state(false, after, false) == GroupState::Finished;
        assert!(finished(after_group(&[done.clone(), said()], 0)));
        assert!(finished(after_group(&[done.clone(), turn_end], 0)));
        // What sits between rows doesn't decide it.
        assert!(finished(after_group(
            &[done.clone(), thought(""), said()],
            0
        )));
        assert!(finished(after_group(
            &[
                done.clone(),
                Item::Notice {
                    text: "Compacted".into()
                },
                said()
            ],
            0
        )));
        // A step that failed is still a step that ended, if the agent went on.
        assert!(finished(after_group(
            &[tool("Read", ToolStatus::Failed), said()],
            0
        )));
        // A turn that died there didn't finish it.
        for end in [
            Item::Error { text: "no".into() },
            Item::Notice {
                text: "Interrupted".into(),
            },
            user(false),
        ] {
            let after = after_group(&[done.clone(), end], 0);
            assert_eq!(after, After::Stopped);
            assert_eq!(group_state(false, after, false), GroupState::Stopped);
        }
        // A transcript that just stops, with no turn at it.
        let after = after_group(std::slice::from_ref(&done), 0);
        assert_eq!(after, After::Nothing);
        assert_eq!(group_state(false, after, false), GroupState::Stopped);
    }

    #[test]
    fn only_the_agents_own_output_proves_it_went_on() {
        let done = tool("Run command", ToolStatus::Done);
        let interrupted = Item::Notice {
            text: "Interrupted".into(),
        };
        let row = |id: String| Item::Tool {
            id,
            title: "Trek's".into(),
            detail: String::new(),
            output: String::new(),
            status: ToolStatus::Done,
        };
        let inserted = [
            row(crate::pr_card::row_id("https://github.com/a/b/pull/1")),
            row(crate::pr_card::row_id("release-1.0~2")),
            trek_core::lineage::row(trek_core::lineage::Link::Started, "other", "Other"),
        ];
        for trek in &inserted {
            // The agent runs `gh pr create`, Trek adds a card, the user interrupts: no tick.
            let after = after_group(&[done.clone(), trek.clone(), interrupted.clone()], 0);
            assert_eq!(after, After::Stopped);
            assert_eq!(group_state(false, after, false), GroupState::Stopped);
            // With nothing after it, the turn says.
            let after = after_group(&[done.clone(), trek.clone()], 0);
            assert_eq!(after, After::Nothing);
            assert_eq!(group_state(false, after, false), GroupState::Stopped);
            assert_eq!(group_state(false, after, true), GroupState::Live);
            // It doesn't hide what the agent did next.
            let after = after_group(&[done.clone(), trek.clone(), said()], 0);
            assert_eq!(after, After::Moved);
            assert_eq!(group_state(false, after, true), GroupState::Finished);
        }

        // Another agent taking over isn't this one going on, whatever the new one goes on to say.
        let handoff = Item::Handoff {
            from: "claude-code".into(),
            from_model: None,
            to: "codex".into(),
            to_model: None,
            from_name: None,
            to_name: None,
        };
        for items in [
            vec![done.clone(), handoff.clone()],
            vec![done.clone(), handoff, said()],
        ] {
            let after = after_group(&items, 0);
            assert_eq!(after, After::Stopped);
            assert_eq!(group_state(false, after, true), GroupState::Stopped);
        }

        // Its own later message, call or sub-agent does, in a turn still running or not.
        for later in [
            said(),
            tool("Read", ToolStatus::Done),
            tool("Read", ToolStatus::Running),
            row(trek_core::orchestrate::task_row("child")),
        ] {
            let after = after_group(&[done.clone(), later], 0);
            assert_eq!(after, After::Moved);
            assert_eq!(group_state(false, after, false), GroupState::Finished);
            assert_eq!(group_state(false, after, true), GroupState::Finished);
        }
        // A running turn with a step still going is live whatever follows.
        assert_eq!(
            group_state(true, after_group(&[done.clone(), said()], 0), true),
            GroupState::Live
        );
    }

    #[test]
    fn a_group_is_live_while_a_step_runs_or_the_turn_is_at_it() {
        assert_eq!(
            group_state(true, After::Nothing, true),
            GroupState::Live,
            "a step running"
        );
        assert_eq!(
            group_state(true, After::Moved, true),
            GroupState::Live,
            "parallel to what came next"
        );
        assert_eq!(
            group_state(false, After::Nothing, true),
            GroupState::Live,
            "between calls"
        );
        assert_eq!(
            group_state(false, After::Moved, true),
            GroupState::Finished,
            "the turn went on without it"
        );
        let trailing = [tool("Read", ToolStatus::Done), thought("hmm")];
        assert_eq!(after_group(&trailing, 0), After::Nothing, "a live thought");
    }

    #[test]
    fn no_duration_without_data() {
        let s = Duration::from_secs;
        assert_eq!(thought_total(std::iter::empty()), None, "no thoughts");
        assert_eq!(thought_total([None].into_iter()), None, "read back");
        assert_eq!(
            thought_total([Some(s(9)), None].into_iter()),
            None,
            "part of it would be an estimate"
        );
        assert_eq!(
            thought_total([Some(Duration::from_millis(300))].into_iter()),
            None,
            "under a second"
        );
        assert_eq!(
            thought_total([Some(s(9)), Some(s(5))].into_iter()),
            Some(14)
        );
        assert_eq!(
            header_words(GroupState::Finished, "Read 1 file", None),
            Words::Summary("Read 1 file")
        );
        assert_eq!(
            header_words(GroupState::Stopped, "Read 1 file", None),
            Words::Summary("Read 1 file")
        );
        assert_eq!(
            header_words(GroupState::Finished, "Read 1 file", Some(14)),
            Words::ThoughtFor(14)
        );
        assert_eq!(
            header_words(GroupState::Live, "Read 1 file", Some(14)),
            Words::Thinking
        );
    }

    #[test]
    fn the_turn_s_clock_is_the_group_s_only_when_it_opened_the_turn() {
        assert!(opens_turn(
            &[user(false), tool("Read", ToolStatus::Done)],
            1
        ));
        assert!(opens_turn(
            &[user(false), thought(" "), tool("Read", ToolStatus::Done)],
            2
        ));
        assert!(opens_turn(&[tool("Read", ToolStatus::Done)], 0));
        assert!(!opens_turn(
            &[user(false), said(), tool("Read", ToolStatus::Done)],
            2
        ));
        assert!(!opens_turn(
            &[user(false), thought("plan"), tool("Read", ToolStatus::Done)],
            2
        ));
        assert!(!opens_turn(
            &[user(true), tool("Read", ToolStatus::Done)],
            1
        ));
    }

    #[test]
    fn a_step_s_mark_says_what_its_status_does() {
        let m = mark;
        assert_eq!(m(ToolKind::Read, ToolStatus::Done), Mark::Dot);
        assert_eq!(m(ToolKind::Search, ToolStatus::Done), Mark::Search);
        assert_eq!(m(ToolKind::WebSearch, ToolStatus::Done), Mark::Search);
        assert_eq!(m(ToolKind::Search, ToolStatus::Running), Mark::Running);
        assert_eq!(m(ToolKind::Search, ToolStatus::Failed), Mark::Failed);
        assert_eq!(m(ToolKind::Command, ToolStatus::Denied), Mark::Failed);
    }

    #[test]
    fn a_step_keeps_the_tool_row_s_words() {
        assert_eq!(
            wording("Run command", "ls -la", None),
            Wording {
                label: "Run command",
                detail: "ls -la",
                code: true
            }
        );
        assert_eq!(
            wording("Subagent", "Review it", None),
            Wording {
                label: "Review it",
                detail: "",
                code: true
            }
        );
        assert_eq!(
            wording("Task", "plan", Some("Reading a.rs · 2 steps")),
            Wording {
                label: "Task",
                detail: "Reading a.rs · 2 steps",
                code: false
            }
        );
    }

    #[test]
    fn steps_keep_the_design_s_pitch() {
        assert_eq!(LINE + step_bottom(false, true), PITCH);
        assert_eq!(LINE + CHIPS_TOP + CHIP + step_bottom(true, true), 67.);
        assert_eq!(
            step_bottom(true, false),
            0.,
            "nothing to reach below the last"
        );
    }

    #[test]
    fn a_live_thought_reads_as_its_latest_line() {
        assert_eq!(
            thought_line("First.\n\n**Mapping the routes**\n"),
            Some("Mapping the routes")
        );
        assert_eq!(thought_line("## Plan"), Some("Plan"));
        assert_eq!(thought_line("  \n "), None);
        assert_eq!(thought_line(""), None);
        let long = "é".repeat(THOUGHT_LINE + 40);
        assert_eq!(
            thought_line(&long).map(|l| l.chars().count()),
            Some(THOUGHT_LINE)
        );
    }
}
