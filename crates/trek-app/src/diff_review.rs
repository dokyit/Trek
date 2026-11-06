//! Reviewing an agent's changes from its diff, in the Git tool and in the editor's diff tabs.
//!
//! Comments go on a line or a range and wait as drafts with the thread (`Notes`, kept in the
//! store), so they outlast switching files and closing the panel. "Send to agent" sends them all
//! as one message (`prompt`) through the ordinary send path: one message rather than one per
//! comment, so the agent takes them in a single pass instead of swinging between small revisions.
//! A draft stays on its code as the diff shifts (`locate`), and only shows in the list under the
//! diff (`tray`) once that code is gone.
//!
//! The keys (`KEY_CONTEXT`) belong to the diff while the diff itself has focus: `key_context` is
//! only set then, so they can't reach a text field, not even the comment box drawn inside the diff.
//!
//! Both surfaces draw uniform rows, so a comment's card takes as many rows as it's tall and is
//! drawn once, over them, by the first of them on screen (`card_row`). The rows' common shape for
//! moving around is `Nav`.

use crate::fluent;
use crate::palette;
use crate::scale::{radius, text};
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Escape, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// What the diff's own keys are bound in: set only while the diff itself has focus (`keys`).
pub const KEY_CONTEXT: &str = "DiffReview";

actions!(
    diff_review,
    [
        NextLine,
        PreviousLine,
        ExtendDown,
        ExtendUp,
        NextHunk,
        PreviousHunk,
        NextFile,
        PreviousFile,
        ToggleViewed,
        StageHunk,
        AddComment,
        SendComments,
        ClearCursor
    ]
);

/// Lines of code a comment quotes in the message, and how much of each: enough for the agent to
/// find the place, not a copy of the file.
const QUOTE_LINES: usize = 3;
const QUOTE_CHARS: usize = 120;
/// A comment's text in its card, in lines at most (all of it shows when it's edited).
const MAX_BODY_LINES: usize = 12;
/// The height of a line of a comment's text.
const BODY_LINE: f32 = 17.;
/// A card's padding and border and its head.
const CARD_CHROME: f32 = 12. + 2. + 20.;
/// The room a card keeps from the lines around it, at the least.
const CARD_GAP: f32 = 6.;
/// The comment box's lines of text.
const EDITOR_LINES: usize = 3;
/// How tall the comment box is: its head, those lines (the text field's own), its buttons.
const EDITOR_HEIGHT: f32 = CARD_CHROME + 64. + 24.;
/// What a text field keeps around its text, drawn without a frame: taken back, so the text lines
/// up with the rest of the card.
const INPUT_INSET: f32 = 10.;
/// The widest a card gets, on a wide diff.
const CARD_WIDTH: f32 = 560.;

/// Which numbering a comment's lines are in: the file before the change, for lines it removed, or
/// after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Old,
    New,
}

/// A comment not sent yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub id: u64,
    pub path: String,
    pub side: Side,
    pub first: u32,
    pub last: u32,
    /// The lines it's on as the diff shows them (`+`, `-` or a space first), cut short.
    pub quote: Vec<String>,
    pub body: String,
}

impl Draft {
    /// `src/a.rs:42`, `src/a.rs:42-44`.
    pub fn place(&self) -> String {
        if self.first == self.last {
            format!("{}:{}", self.path, self.first)
        } else {
            format!("{}:{}-{}", self.path, self.first, self.last)
        }
    }
}

/// Where a new comment goes: a `Draft` without its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnchor {
    pub path: String,
    pub side: Side,
    pub first: u32,
    pub last: u32,
    pub quote: Vec<String>,
}

/// What a thread (or, with no thread, a folder: `folder_key`) has of a review: its comments, and
/// its files marked viewed, each with what its diff was then (`sig_of`), so a file that changes
/// again comes back unviewed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    #[serde(default)]
    pub drafts: Vec<Draft>,
    #[serde(default)]
    pub viewed: HashMap<String, String>,
    #[serde(default)]
    pub next_id: u64,
}

/// Where comments collect when no thread is on show: the editor's diffs (any chat of the folder
/// can take them) and a draft's.
pub fn folder_key(dir: &std::path::Path) -> String {
    format!("dir:{}", dir.display())
}

/// A viewed mark that hasn't seen its file's diff yet (marked from the list, not the diff).
pub const SIG_UNKNOWN: &str = "?";

/// What a diff says, to tell whether it has changed: its lines, not its `@@` heads.
pub fn sig_of<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in lines.filter(|l| !l.starts_with("@@")) {
        l.trim_end().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// A line as a comment quotes it.
fn quoted(line: &str) -> String {
    let line = line.trim_end().replace('\t', "    ");
    if line.chars().count() > QUOTE_CHARS {
        format!("{}…", line.chars().take(QUOTE_CHARS).collect::<String>())
    } else {
        line
    }
}

/// The lines a comment quotes, out of the ones it's on.
pub fn quote_of(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = lines.iter().take(QUOTE_LINES).map(|l| quoted(l)).collect();
    if lines.len() > QUOTE_LINES {
        out.push("…".into());
    }
    out
}

/// In order down the files, whatever order they were written in.
pub fn sorted(mut drafts: Vec<Draft>) -> Vec<Draft> {
    drafts.sort_by(|a, b| (&a.path, a.first, a.last, a.id).cmp(&(&b.path, b.first, b.last, b.id)));
    drafts
}

/// The one message comments go in: for each its place, a little of the code, and what was said.
pub fn prompt(drafts: &[Draft]) -> String {
    let mut out = if drafts.len() == 1 {
        "I left a comment on your changes. Please address it:\n".to_string()
    } else {
        format!(
            "I left {} comments on your changes. Please address them together:\n",
            drafts.len()
        )
    };
    for (i, d) in sorted(drafts.to_vec()).iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {}{}\n",
            i + 1,
            d.place(),
            if d.side == Side::Old {
                " (removed lines)"
            } else {
                ""
            }
        ));
        for q in &d.quote {
            out.push_str(&format!("> {q}\n"));
        }
        out.push_str(d.body.trim());
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// "Line 42", "Lines 42-44", and which side when it's the old one.
fn lines_label(side: Side, first: u32, last: u32) -> String {
    let lines = if first == last {
        format!("Line {first}")
    } else {
        format!("Lines {first}-{last}")
    };
    match side {
        Side::Old => format!("{lines}, removed"),
        Side::New => lines,
    }
}

// ───────────────────────────── moving around ─────────────────────────────

/// What a row of a diff is, to move around on: a surface draws its own rows, this says what each
/// one is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    /// Nothing to stop on: room, notes, folds, a comment's card.
    Skip,
    File {
        file: usize,
    },
    Hunk {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        /// None for a line outside the hunks, opened out of a fold.
        hunk: Option<usize>,
        old: Option<u32>,
        new: Option<u32>,
        changed: bool,
    },
}

impl Nav {
    pub fn file(self) -> Option<usize> {
        match self {
            Nav::Skip => None,
            Nav::File { file } | Nav::Hunk { file, .. } | Nav::Line { file, .. } => Some(file),
        }
    }

    pub fn hunk(self) -> Option<(usize, usize)> {
        match self {
            Nav::Hunk { file, hunk } => Some((file, hunk)),
            Nav::Line {
                file,
                hunk: Some(hunk),
                ..
            } => Some((file, hunk)),
            _ => None,
        }
    }

    fn is_line(self) -> bool {
        matches!(self, Nav::Line { .. })
    }
}

/// The next (`by` > 0) or previous row the cursor stops on: lines, and the head of a file (all a
/// collapsed one has).
pub fn step_line(nav: &[Nav], from: Option<usize>, by: isize) -> Option<usize> {
    let stop = |i: &usize| matches!(nav[*i], Nav::Line { .. } | Nav::File { .. });
    match from {
        Some(f) if by > 0 => (f + 1..nav.len()).find(stop),
        Some(f) => (0..f.min(nav.len())).rev().find(stop),
        None => first_stop(nav),
    }
}

/// Where a cursor with nowhere better to be goes: the first change, or else the first stop.
pub fn first_stop(nav: &[Nav]) -> Option<usize> {
    nav.iter()
        .position(|n| matches!(n, Nav::Line { changed: true, .. }))
        .or_else(|| {
            nav.iter()
                .position(|n| matches!(n, Nav::Line { .. } | Nav::File { .. }))
        })
}

/// The first changed line under the head at `head` (a hunk's or a file's), or `head` itself.
pub fn landing(nav: &[Nav], head: usize) -> usize {
    let mut first_line = None;
    for (i, n) in nav.iter().enumerate().skip(head + 1) {
        match n {
            Nav::Line { changed: true, .. } => return i,
            Nav::Line { .. } => {
                first_line.get_or_insert(i);
            }
            Nav::File { .. } => break,
            Nav::Hunk { .. } if matches!(nav[head], Nav::Hunk { .. }) => break,
            _ => {}
        }
    }
    first_line.unwrap_or(head)
}

/// The next or previous hunk's head from `from`.
pub fn step_hunk(nav: &[Nav], from: Option<usize>, by: isize) -> Option<usize> {
    let is_head = |i: &usize| matches!(nav[*i], Nav::Hunk { .. });
    let Some(f) = from.filter(|f| *f < nav.len()) else {
        return if by > 0 {
            (0..nav.len()).find(is_head)
        } else {
            (0..nav.len()).rev().find(is_head)
        };
    };
    if by > 0 {
        return (f + 1..nav.len()).find(is_head);
    }
    // Back from inside a hunk means the one before it, not this one's head.
    let start = nav[f]
        .hunk()
        .and_then(|h| {
            (0..=f)
                .rev()
                .find(|i| matches!(nav[*i], Nav::Hunk { .. }) && nav[*i].hunk() == Some(h))
        })
        .unwrap_or(f);
    (0..start).rev().find(is_head)
}

/// The next or previous file's head from `from`.
pub fn step_file(nav: &[Nav], from: Option<usize>, by: isize) -> Option<usize> {
    let is_file = |i: &usize| matches!(nav[*i], Nav::File { .. });
    let here = from.and_then(|f| nav.get(f).copied()).and_then(Nav::file);
    match here {
        Some(here) if by > 0 => (0..nav.len()).find(|i| is_file(i) && nav[*i].file() > Some(here)),
        Some(here) => (0..nav.len())
            .rev()
            .find(|i| is_file(i) && nav[*i].file() < Some(here)),
        None if by > 0 => (0..nav.len()).find(is_file),
        None => (0..nav.len()).rev().find(is_file),
    }
}

/// The lines of `file` among the rows `lo..=hi`.
fn lines_between(nav: &[Nav], lo: usize, hi: usize, file: usize) -> Vec<usize> {
    (lo..=hi.min(nav.len().saturating_sub(1)))
        .filter(|i| nav[*i].is_line() && nav[*i].file() == Some(file))
        .collect()
}

/// A comment on the rows from `cursor` to `anchor`, with the lines of the cursor's file among them:
/// numbered as the new file when any of them is in it, else (only removed lines) as the old one.
pub fn anchor_of(
    nav: &[Nav],
    cursor: usize,
    anchor: Option<usize>,
    path_of: &dyn Fn(usize) -> Option<String>,
    text_of: &dyn Fn(usize) -> Vec<String>,
) -> Option<LineAnchor> {
    let file = nav.get(cursor)?.file()?;
    let (lo, hi) = {
        let other = anchor.unwrap_or(cursor);
        (cursor.min(other), cursor.max(other))
    };
    let rows = lines_between(nav, lo, hi, file);
    if rows.is_empty() {
        return None;
    }
    let numbers = |side: Side| -> Vec<u32> {
        rows.iter()
            .filter_map(|i| match (nav[*i], side) {
                (Nav::Line { new, .. }, Side::New) => new,
                (Nav::Line { old, .. }, Side::Old) => old,
                _ => None,
            })
            .collect()
    };
    let (side, nums) = match numbers(Side::New) {
        n if !n.is_empty() => (Side::New, n),
        _ => (Side::Old, numbers(Side::Old)),
    };
    let lines: Vec<String> = rows.iter().flat_map(|i| text_of(*i)).collect();
    Some(LineAnchor {
        path: path_of(file)?,
        side,
        first: *nums.iter().min()?,
        last: *nums.iter().max()?,
        quote: quote_of(&lines),
    })
}

/// A line of one file in a diff: its row and its numbers.
#[derive(Clone, Copy, Debug)]
pub struct FileLine {
    pub row: usize,
    pub old: Option<u32>,
    pub new: Option<u32>,
}

impl FileLine {
    fn number(&self, side: Side) -> Option<u32> {
        match side {
            Side::Old => self.old,
            Side::New => self.new,
        }
    }
}

/// Where a comment now sits in its file's lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub first: u32,
    pub last: u32,
    /// The row its card goes under.
    pub after: usize,
}

/// A comment's place in the diff as it is now: where it was if its code still is, else the nearest
/// line with that code (the diff above it moved), else nowhere (the code is gone, or this is
/// another diff of the file).
pub fn locate(
    side: Side,
    first: u32,
    last: u32,
    quote: &[String],
    lines: &[FileLine],
    text_of: &dyn Fn(usize) -> Vec<String>,
) -> Option<Spot> {
    let matches = |l: &FileLine| match quote.first() {
        Some(want) => text_of(l.row).iter().any(|t| quoted(t) == *want),
        None => true,
    };
    let exact = lines
        .iter()
        .find(|l| l.number(side) == Some(first))
        .filter(|l| matches(l));
    let start = match exact {
        Some(l) => l.number(side)?,
        None => lines
            .iter()
            .filter(|l| l.number(side).is_some() && matches(l))
            .min_by_key(|l| l.number(side).unwrap_or(0).abs_diff(first))?
            .number(side)?,
    };
    let last = start + last.saturating_sub(first);
    let after = lines
        .iter()
        .filter(|l| l.number(side).is_some_and(|n| n <= last))
        .max_by_key(|l| l.number(side))?
        .row;
    Some(Spot {
        first: start,
        last,
        after,
    })
}

// ───────────────────────────── a surface's state ─────────────────────────────

/// What's being written: a new comment, or an old one again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Writing {
    New(LineAnchor),
    Draft(u64),
}

/// The comment box.
pub struct Editing {
    pub what: Writing,
    pub input: Entity<TextareaState>,
    _enter: Subscription,
}

/// Where the cursor goes when a diff has just been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrive {
    First,
    LastHunk,
}

/// A surface's review state: the cursor and selection (rows of its list), the comment box and the
/// list under the diff.
#[derive(Default)]
pub struct DiffReview {
    pub cursor: Option<usize>,
    /// The selection's other end.
    pub anchor: Option<usize>,
    /// A drag selecting lines started on this row.
    drag_from: Option<usize>,
    pub tray_open: bool,
    pub editing: Option<Editing>,
    /// The comment box has a place in the diff (else it shows in the list under it).
    pub editing_placed: bool,
    /// Scroll to this comment once it's in the rows.
    pub reveal: Option<u64>,
    pub arrive: Option<Arrive>,
    /// `Workspace::review_notes_epoch` the rows were last built at.
    pub seen_epoch: u64,
    /// The width the cards were last measured at.
    pub measured: f32,
}

impl DiffReview {
    /// The cursor and the selection's other end, by what they're on, for `restore` once the rows
    /// have been built again (rows come and go around them).
    pub fn remember(&self, nav: &[Nav]) -> (Option<(usize, Nav)>, Option<Nav>) {
        (
            self.cursor.and_then(|c| Some((c, *nav.get(c)?))),
            self.anchor.and_then(|a| nav.get(a).copied()),
        )
    }

    pub fn restore(&mut self, was: (Option<(usize, Nav)>, Option<Nav>), nav: &[Nav]) {
        let (cursor, anchor) = was;
        self.cursor = cursor.and_then(|(ix, on)| {
            nav.iter()
                .position(|n| *n == on)
                // Its file was collapsed: its head. Its hunk went (staged, kept): whatever's there now.
                .or_else(|| {
                    on.file().and_then(|f| {
                        nav.iter()
                            .position(|n| *n == Nav::File { file: f })
                            .filter(|_| !nav.iter().any(|n| n.is_line() && n.file() == Some(f)))
                    })
                })
                .or_else(|| {
                    let ix = ix.min(nav.len().checked_sub(1)?);
                    (ix..nav.len())
                        .find(|i| nav[*i].is_line())
                        .or_else(|| step_line(nav, Some(ix), -1))
                })
        });
        self.anchor = anchor.and_then(|on| nav.iter().position(|n| *n == on));
    }

    /// The rows selected, ends included.
    fn selection(&self) -> Option<(usize, usize)> {
        let c = self.cursor?;
        let a = self.anchor?;
        Some((c.min(a), c.max(a)))
    }

    /// How row `ix` is marked.
    pub fn marks(&self, nav: &[Nav], ix: usize, focused: bool) -> Marks {
        let in_review = focused || self.editing.is_some();
        let hunk = self.cursor.and_then(|c| nav.get(c)?.hunk());
        let selected = self
            .selection()
            .is_some_and(|(lo, hi)| (lo..=hi).contains(&ix))
            && nav.get(ix).is_some_and(|n| n.is_line())
            && nav.get(ix).and_then(|n| n.file()) == self.cursor.and_then(|c| nav.get(c)?.file());
        Marks {
            current: in_review && self.cursor == Some(ix),
            in_hunk: in_review && hunk.is_some() && nav.get(ix).and_then(|n| n.hunk()) == hunk,
            selected,
            // A selection's last line: where it's commented on from.
            selection_end: selected && self.selection().is_some_and(|(_, hi)| hi == ix),
        }
    }
}

/// How a row is marked for the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub current: bool,
    pub in_hunk: bool,
    pub selected: bool,
    pub selection_end: bool,
}

/// A card and the rows it covers, under the row `after` (before cards are put in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub after: usize,
    pub card: Card,
    pub rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Draft(u64),
    Editor,
}

/// How tall a saved comment's card is, `width` wide.
fn saved_height(body: &str, width: f32, cx: &App) -> f32 {
    let lines = body_lines(body, width - 22., cx).min(MAX_BODY_LINES);
    CARD_CHROME + lines as f32 * BODY_LINE
}

/// How many rows a card that tall takes.
fn rows_for(height: f32, row: f32) -> usize {
    ((height + CARD_GAP) / row).ceil() as usize
}

/// The lines `text` wraps to at `width`, in a card's font.
fn body_lines(text: &str, width: f32, cx: &App) -> usize {
    let font = font(cx.theme().font_family.clone());
    let mut wrapper = cx.text_system().line_wrapper(font, px(12.));
    text.split('\n')
        .map(|line| {
            let fragments = [LineFragment::text(line)];
            1 + wrapper.wrap_line(&fragments, px(width.max(40.))).count()
        })
        .sum()
}

/// Where the cards go in a diff's rows (`nav`, with no cards in it yet), what moved (comment, its
/// new first and last line), and whether the comment box found a place.
pub fn place_cards(
    nav: &[Nav],
    path_of: &dyn Fn(usize) -> Option<String>,
    text_of: &dyn Fn(usize) -> Vec<String>,
    drafts: &[Draft],
    editing: Option<&Editing>,
    row: f32,
    width: f32,
    cx: &App,
) -> (Vec<Placed>, Vec<(u64, u32, u32)>, bool) {
    let mut placed = vec![];
    let mut moved = vec![];
    if drafts.is_empty() && editing.is_none() {
        return (placed, moved, false);
    }
    let mut by_path: HashMap<String, Vec<FileLine>> = HashMap::new();
    for (ix, n) in nav.iter().enumerate() {
        if let Nav::Line { file, old, new, .. } = *n {
            if let Some(path) = path_of(file) {
                by_path
                    .entry(path)
                    .or_default()
                    .push(FileLine { row: ix, old, new });
            }
        }
    }
    let mut editor_placed = false;
    let writing = editing.map(|e| &e.what);
    for d in drafts {
        let Some(spot) = by_path
            .get(&d.path)
            .and_then(|lines| locate(d.side, d.first, d.last, &d.quote, lines, text_of))
        else {
            continue;
        };
        if (spot.first, spot.last) != (d.first, d.last) {
            moved.push((d.id, spot.first, spot.last));
        }
        if writing == Some(&Writing::Draft(d.id)) {
            editor_placed = true;
            placed.push(Placed {
                after: spot.after,
                card: Card::Editor,
                rows: rows_for(EDITOR_HEIGHT, row),
            });
        } else {
            placed.push(Placed {
                after: spot.after,
                card: Card::Draft(d.id),
                rows: rows_for(saved_height(&d.body, width, cx), row),
            });
        }
    }
    if let Some(Writing::New(a)) = writing {
        if let Some(spot) = by_path
            .get(&a.path)
            .and_then(|lines| locate(a.side, a.first, a.last, &a.quote, lines, text_of))
        {
            editor_placed = true;
            placed.push(Placed {
                after: spot.after,
                card: Card::Editor,
                rows: rows_for(EDITOR_HEIGHT, row),
            });
        }
    }
    placed.sort_by_key(|p| p.after);
    (placed, moved, editor_placed)
}

/// `rows` and `nav` with the cards' rows put in, each made by `card` (the placement, which of its
/// rows).
pub fn merge<R>(
    rows: Vec<R>,
    nav: Vec<Nav>,
    placed: &[Placed],
    card: impl Fn(&Placed, usize) -> R,
) -> (Vec<R>, Vec<Nav>) {
    if placed.is_empty() {
        return (rows, nav);
    }
    let extra: usize = placed.iter().map(|p| p.rows).sum();
    let mut out = Vec::with_capacity(rows.len() + extra);
    let mut out_nav = Vec::with_capacity(rows.len() + extra);
    let mut next = placed.iter().peekable();
    for (ix, (row, n)) in rows.into_iter().zip(nav).enumerate() {
        out.push(row);
        out_nav.push(n);
        while let Some(p) = next.next_if(|p| p.after == ix) {
            for part in 0..p.rows {
                out.push(card(p, part));
                out_nav.push(Nav::Skip);
            }
        }
    }
    (out, out_nav)
}

// ───────────────────────────── what a surface does ─────────────────────────────

/// A surface that draws a diff to review.
pub trait ReviewHost: Sized + 'static {
    fn review(&self) -> &DiffReview;
    fn review_mut(&mut self) -> &mut DiffReview;
    fn workspace_entity(&self) -> Entity<Workspace>;
    fn nav(&self) -> &[Nav];
    fn list_scroll(&self) -> &UniformListScrollHandle;
    fn diff_focus(&self) -> FocusHandle;
    /// Whose comments these are: a thread's, or a folder's.
    fn notes_key(&self, cx: &App) -> Option<String>;
    fn path_of(&self, file: usize) -> Option<String>;
    /// The lines of row `ix` as a diff shows them, `+`, `-` or a space first.
    fn text_of(&self, ix: usize) -> Vec<String>;
    /// The cards in the rows and their last rows (`card_ends`).
    fn placements(&self) -> &[(Card, usize)];
    /// Build the rows again (comments or the comment box moved).
    fn rows_changed(&mut self, cx: &mut Context<Self>);
    /// To the next or previous file.
    fn go_to_file(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>);
    /// Past the last hunk (or before the first). On a surface showing one file at a time, that's
    /// the next file's.
    fn ran_out_of_hunks(&mut self, _by: isize, _window: &mut Window, _cx: &mut Context<Self>) {}
    /// Mark the file under the cursor viewed, or not; a keystroke (`advance`) goes on to the next.
    fn toggle_viewed(&mut self, advance: bool, window: &mut Window, cx: &mut Context<Self>);
    /// Stage or unstage the hunk under the cursor, where this diff has staging.
    fn stage_hunk(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
    /// Send `text` where this surface's messages go: whether it went (`send_comments_in`).
    fn deliver(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) -> bool;
    /// Bring `draft`'s file on show, for its card to be found (`DiffReview::reveal`).
    fn show_draft(&mut self, draft: &Draft, cx: &mut Context<Self>);
    /// Escape with nothing left to clear: the keys go back to where messages are written.
    fn leave(&mut self, _cx: &mut Context<Self>) {}
}

/// Scroll so `ix` shows, and put the cursor there.
fn put_cursor<T: ReviewHost>(this: &mut T, ix: usize, extend: bool, cx: &mut Context<T>) {
    let review = this.review_mut();
    if extend {
        if review.anchor.is_none() {
            review.anchor = review.cursor;
        }
    } else {
        review.anchor = None;
    }
    review.cursor = Some(ix);
    this.list_scroll()
        .scroll_to_item(ix, ScrollStrategy::Nearest);
    cx.notify();
}

fn move_line<T: ReviewHost>(this: &mut T, by: isize, extend: bool, cx: &mut Context<T>) {
    let from = this.review().cursor;
    let Some(to) = step_line(this.nav(), from, by) else {
        return;
    };
    // A selection stays in its file.
    if extend {
        let here = from.and_then(|f| this.nav().get(f)?.file());
        if !this.nav()[to].is_line() || (from.is_some() && this.nav()[to].file() != here) {
            return;
        }
    }
    put_cursor(this, to, extend, cx);
}

/// To hunk head `head`: the head at the top, the cursor on its first change.
pub fn go_to_head<T: ReviewHost>(this: &mut T, head: usize, cx: &mut Context<T>) {
    let line = landing(this.nav(), head);
    let review = this.review_mut();
    review.anchor = None;
    review.cursor = Some(line);
    this.list_scroll().scroll_to_item(head, ScrollStrategy::Top);
    cx.notify();
}

fn move_hunk<T: ReviewHost>(this: &mut T, by: isize, window: &mut Window, cx: &mut Context<T>) {
    match step_hunk(this.nav(), this.review().cursor, by) {
        Some(head) => go_to_head(this, head, cx),
        None => this.ran_out_of_hunks(by, window, cx),
    }
}

/// The current row for keys that need one: the first change if the cursor isn't anywhere yet.
fn ensure_cursor<T: ReviewHost>(this: &mut T) -> Option<usize> {
    if this.review().cursor.is_none() {
        let first = first_stop(this.nav());
        this.review_mut().cursor = first;
    }
    this.review().cursor
}

/// `c`: a comment on the selection, or the line under the cursor. On a line that already has a
/// comment exactly there, that comment again.
pub fn begin_comment<T: ReviewHost>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    let Some(cursor) = ensure_cursor(this) else {
        return;
    };
    if this.review().editing.is_some() {
        focus_editor(this, window, cx);
        return;
    }
    let anchor = {
        let path_of = |f: usize| this.path_of(f);
        let text_of = |ix: usize| this.text_of(ix);
        anchor_of(this.nav(), cursor, this.review().anchor, &path_of, &text_of)
    };
    let Some(anchor) = anchor else {
        return;
    };
    let same = this.notes_key(cx).and_then(|key| {
        let ws = this.workspace_entity();
        let ws = ws.read(cx);
        ws.review_notes.get(&key)?.drafts.iter().find_map(|d| {
            (d.path == anchor.path
                && d.side == anchor.side
                && (d.first, d.last) == (anchor.first, anchor.last))
                .then_some(d.id)
        })
    });
    match same {
        Some(id) => begin_edit(this, id, window, cx),
        None => open_editor(this, Writing::New(anchor), String::new(), window, cx),
    }
}

/// Edit comment `id` again.
pub fn begin_edit<T: ReviewHost>(this: &mut T, id: u64, window: &mut Window, cx: &mut Context<T>) {
    let Some(key) = this.notes_key(cx) else {
        return;
    };
    let draft = {
        let ws = this.workspace_entity();
        let ws = ws.read(cx);
        ws.review_notes
            .get(&key)
            .and_then(|n| n.drafts.iter().find(|d| d.id == id).cloned())
    };
    let Some(draft) = draft else {
        return;
    };
    open_editor(this, Writing::Draft(id), draft.body.clone(), window, cx);
    this.show_draft(&draft, cx);
}

fn open_editor<T: ReviewHost>(
    this: &mut T,
    what: Writing,
    text: String,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let on_cmd_enter = this
        .workspace_entity()
        .read(cx)
        .settings
        .general
        .send_with_cmd_enter;
    let input = cx.new(|cx| {
        let mut state = TextareaState::new(window, cx)
            .auto_grow(EDITOR_LINES, EDITOR_LINES)
            .submit_on_enter(!on_cmd_enter)
            .placeholder("Leave a comment");
        state.set_value(text, window, cx);
        state
    });
    let enter = cx.subscribe_in(
        &input,
        window,
        move |this: &mut T, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift, secondary } = event {
                let submit = if on_cmd_enter { *secondary } else { !*shift };
                if submit {
                    commit_comment(this, window, cx);
                }
            }
        },
    );
    this.review_mut().editing = Some(Editing {
        what,
        input,
        _enter: enter,
    });
    this.rows_changed(cx);
    focus_editor(this, window, cx);
    scroll_to_editor(this, cx);
}

fn focus_editor<T: ReviewHost>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    if let Some(input) = this.review().editing.as_ref().map(|e| e.input.clone()) {
        input.update(cx, |s, cx| s.focus(window, cx));
    }
}

/// Bring the comment box into view.
fn scroll_to_editor<T: ReviewHost>(this: &mut T, cx: &mut Context<T>) {
    let last = this
        .placements()
        .iter()
        .find(|(card, _)| *card == Card::Editor)
        .map(|(_, end)| *end);
    if let Some(last) = last {
        this.list_scroll()
            .scroll_to_item(last, ScrollStrategy::Nearest);
        cx.notify();
    }
}

/// The rows' cards as (card, its last row), once merged.
pub fn card_ends(placed: &[Placed]) -> Vec<(Card, usize)> {
    let mut out = vec![];
    let mut added = 0;
    for p in placed {
        added += p.rows;
        out.push((p.card, p.after + added));
    }
    out
}

/// Save the comment box's text: a new comment, or the old one changed. Empty, it's dropped.
pub fn commit_comment<T: ReviewHost>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    let (Some(editing), Some(key)) = (this.review_mut().editing.take(), this.notes_key(cx)) else {
        return;
    };
    let body = editing.input.read(cx).value().trim().to_string();
    let ws = this.workspace_entity();
    ws.update(cx, |ws, cx| match editing.what {
        Writing::New(anchor) if !body.is_empty() => {
            ws.add_comment(&key, anchor, body, cx);
        }
        Writing::New(_) => {}
        Writing::Draft(id) => ws.edit_comment(&key, id, body, cx),
    });
    this.review_mut().anchor = None;
    this.rows_changed(cx);
    this.diff_focus().focus(window, cx);
}

/// Leave the comment box; what was in it goes.
pub fn cancel_comment<T: ReviewHost>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    if this.review_mut().editing.take().is_some() {
        this.rows_changed(cx);
    }
    this.diff_focus().focus(window, cx);
}

fn remove_comment<T: ReviewHost>(this: &mut T, id: u64, window: &mut Window, cx: &mut Context<T>) {
    let Some(key) = this.notes_key(cx) else {
        return;
    };
    if this
        .review()
        .editing
        .as_ref()
        .is_some_and(|e| e.what == Writing::Draft(id))
    {
        this.review_mut().editing = None;
        this.diff_focus().focus(window, cx);
    }
    this.workspace_entity()
        .update(cx, |ws, cx| ws.remove_comment(&key, id, cx));
    this.rows_changed(cx);
}

/// Send every comment, as one message, where this surface's messages go; they're gone from the
/// drafts once it's on its way, and stay when it can't go yet.
pub fn send_comments<T: ReviewHost>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    let Some(key) = this.notes_key(cx) else {
        return;
    };
    let drafts = this
        .workspace_entity()
        .read(cx)
        .review_notes
        .get(&key)
        .map(|n| n.drafts.clone())
        .unwrap_or_default();
    if drafts.is_empty() {
        return;
    }
    if !this.deliver(prompt(&drafts), window, cx) {
        return;
    }
    this.review_mut().tray_open = false;
    this.workspace_entity()
        .update(cx, |ws, cx| ws.drop_comments(&key, cx));
    this.rows_changed(cx);
}

/// Take every comment off, with an undo: they were written by hand.
fn clear_comments<T: ReviewHost>(this: &mut T, cx: &mut Context<T>) {
    let Some(key) = this.notes_key(cx) else {
        return;
    };
    this.review_mut().tray_open = false;
    let ws = this.workspace_entity();
    ws.update(cx, |ws, cx| {
        let drafts = ws.drop_comments(&key, cx);
        if !drafts.is_empty() {
            let n = drafts.len();
            cx.emit(WorkspaceEvent::Toast {
                message: format!("Cleared {n} comment{}", if n == 1 { "" } else { "s" }),
                undo: Some(crate::workspace::UndoAction::RestoreComments { key, drafts }),
            });
        }
    });
    this.rows_changed(cx);
}

/// A line pressed: the cursor comes to it, and a drag from it (or shift) selects.
fn press_line<T: ReviewHost>(this: &mut T, ix: usize, shift: bool, cx: &mut Context<T>) {
    let review = this.review_mut();
    if shift && review.cursor.is_some() {
        if review.anchor.is_none() {
            review.anchor = review.cursor;
        }
    } else {
        review.anchor = None;
    }
    review.cursor = Some(ix);
    review.drag_from = Some(ix);
    cx.notify();
}

fn drag_line<T: ReviewHost>(this: &mut T, ix: usize, cx: &mut Context<T>) {
    let nav_file = |this: &T, row: usize| this.nav().get(row).and_then(|n| n.file());
    let Some(from) = this.review().drag_from else {
        return;
    };
    if ix == this.review().cursor.unwrap_or(ix) || nav_file(this, ix) != nav_file(this, from) {
        return;
    }
    let review = this.review_mut();
    if review.anchor.is_none() {
        review.anchor = Some(from);
    }
    review.cursor = Some(ix);
    cx.notify();
}

/// The keys of a diff, on its focusable element; they fire only while it has the focus itself.
pub fn keys<T: ReviewHost>(el: Stateful<Div>, focused: bool, cx: &mut Context<T>) -> Stateful<Div> {
    el.when(focused, |el| el.key_context(KEY_CONTEXT))
        .on_action(cx.listener(|this: &mut T, _: &NextLine, _, cx| move_line(this, 1, false, cx)))
        .on_action(
            cx.listener(|this: &mut T, _: &PreviousLine, _, cx| move_line(this, -1, false, cx)),
        )
        .on_action(cx.listener(|this: &mut T, _: &ExtendDown, _, cx| move_line(this, 1, true, cx)))
        .on_action(cx.listener(|this: &mut T, _: &ExtendUp, _, cx| move_line(this, -1, true, cx)))
        .on_action(
            cx.listener(|this: &mut T, _: &NextHunk, window, cx| move_hunk(this, 1, window, cx)),
        )
        .on_action(
            cx.listener(|this: &mut T, _: &PreviousHunk, window, cx| {
                move_hunk(this, -1, window, cx)
            }),
        )
        .on_action(
            cx.listener(|this: &mut T, _: &NextFile, window, cx| this.go_to_file(1, window, cx)),
        )
        .on_action(
            cx.listener(|this: &mut T, _: &PreviousFile, window, cx| {
                this.go_to_file(-1, window, cx)
            }),
        )
        .on_action(cx.listener(|this: &mut T, _: &ToggleViewed, window, cx| {
            ensure_cursor(this);
            this.toggle_viewed(true, window, cx)
        }))
        .on_action(cx.listener(|this: &mut T, _: &StageHunk, window, cx| {
            ensure_cursor(this);
            this.stage_hunk(window, cx)
        }))
        .on_action(
            cx.listener(|this: &mut T, _: &AddComment, window, cx| begin_comment(this, window, cx)),
        )
        .on_action(
            cx.listener(|this: &mut T, _: &SendComments, window, cx| {
                send_comments(this, window, cx)
            }),
        )
        .on_action(cx.listener(|this: &mut T, _: &ClearCursor, _, cx| {
            let review = this.review_mut();
            if review.anchor.take().is_none() {
                review.cursor = None;
                this.leave(cx);
            }
            cx.notify();
        }))
}

// ───────────────────────────── drawing ─────────────────────────────

/// A small control on a card or a row, as on a diff's file and hunk rows.
fn small(id: impl Into<ElementId>, label: &'static str, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .flex_none()
        .h(px(18.))
        .px(px(6.))
        .items_center()
        .rounded(px(radius::TAG))
        .cursor_pointer()
        .text_size(px(text::META))
        .text_color(theme.muted_foreground)
        .hover({
            let (bg, fg) = (theme.foreground.opacity(0.08), theme.foreground);
            move |s| s.bg(bg).text_color(fg)
        })
        .child(label)
}

/// A control of the strip under a diff, as on its toolbar.
fn link(id: &'static str) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(22.))
        .px(px(7.))
        .gap(px(4.))
        .items_center()
        .rounded(px(fluent::control(radius::CONTROL)))
        .cursor_pointer()
}

/// The ember control: a diff's one forward action.
fn ember_link(id: &'static str, cx: &App) -> Stateful<Div> {
    link(id)
        .bg(palette::ember(cx))
        .text_color(cx.theme().primary_foreground)
        .hover(|s| s.opacity(0.9))
}

/// A card's box.
fn card_box(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .px(px(10.))
        .py(px(6.))
        .rounded(px(fluent::control(radius::CONTROL)))
        .border_1()
        .border_color(theme.border.opacity(0.7))
        .bg(theme.foreground.opacity(0.045))
        .font_family(theme.font_family.clone())
        .text_size(px(text::LABEL))
        .whitespace_normal()
}

/// A saved comment.
fn saved_card<T: ReviewHost>(d: &Draft, cx: &mut Context<T>) -> Stateful<Div> {
    let theme = cx.theme().clone();
    let id = d.id;
    let n = id as usize;
    card_box(("diff-comment", n), cx)
        .child(
            h_flex()
                .h(px(20.))
                .gap(px(6.))
                .items_center()
                .text_size(px(text::META))
                .text_color(theme.muted_foreground)
                .child(Icon::new(crate::assets::Lucide::MessageSquare).size(px(12.)))
                .child(lines_label(d.side, d.first, d.last))
                .child(div().flex_1())
                .child(
                    small(("diff-comment-edit", n), "Edit", cx)
                        .on_click(cx.listener(move |this: &mut T, _, window, cx| {
                            cx.stop_propagation();
                            begin_edit(this, id, window, cx)
                        }))
                        .test_support(),
                )
                .child(
                    small(("diff-comment-remove", n), "Remove", cx)
                        .on_click(cx.listener(move |this: &mut T, _, window, cx| {
                            cx.stop_propagation();
                            remove_comment(this, id, window, cx)
                        }))
                        .test_support(),
                ),
        )
        .child(
            div()
                .line_height(px(BODY_LINE))
                .line_clamp(MAX_BODY_LINES)
                .text_color(theme.foreground.opacity(0.9))
                .child(d.body.clone()),
        )
}

/// The comment box.
fn editor_card<T: ReviewHost>(
    editing: &Editing,
    label: String,
    new: bool,
    cx: &mut Context<T>,
) -> Stateful<Div> {
    let theme = cx.theme().clone();
    card_box("diff-comment-editor", cx)
        .border_color(theme.border)
        .on_action(
            cx.listener(|this: &mut T, _: &Escape, window, cx| cancel_comment(this, window, cx)),
        )
        .child(
            h_flex()
                .h(px(20.))
                .gap(px(6.))
                .items_center()
                .text_size(px(text::META))
                .text_color(theme.muted_foreground)
                .child(Icon::new(crate::assets::Lucide::MessageSquarePlus).size(px(12.)))
                .child(label),
        )
        .child(
            div()
                .id("diff-comment-input")
                .test_support()
                .mx(px(-INPUT_INSET))
                .mt(px(-6.))
                .child(
                    Textarea::new(&editing.input)
                        .appearance(false)
                        .text_size(px(text::LABEL)),
                ),
        )
        .child(
            h_flex()
                .h(px(24.))
                .gap(px(6.))
                .items_center()
                .justify_end()
                .child(
                    small("diff-comment-cancel", "Cancel", cx)
                        .on_click(cx.listener(|this: &mut T, _, window, cx| {
                            cx.stop_propagation();
                            cancel_comment(this, window, cx)
                        }))
                        .test_support(),
                )
                .child(
                    ember_link("diff-comment-save", cx)
                        .text_size(px(text::META))
                        .child(if new { "Comment" } else { "Save" })
                        .on_click(cx.listener(|this: &mut T, _, window, cx| {
                            cx.stop_propagation();
                            commit_comment(this, window, cx)
                        }))
                        .test_support(),
                ),
        )
}

/// What the comment box says it's on.
fn writing_label<T: ReviewHost>(this: &T, what: &Writing, cx: &App) -> String {
    match what {
        Writing::New(a) => format!(
            "Comment on {}",
            lines_label(a.side, a.first, a.last).to_lowercase()
        ),
        Writing::Draft(id) => this
            .notes_key(cx)
            .and_then(|key| {
                let ws = this.workspace_entity();
                let ws = ws.read(cx);
                let notes = ws.review_notes.get(&key)?;
                notes.drafts.iter().find(|d| d.id == *id).map(|d| {
                    format!(
                        "Editing {}",
                        lines_label(d.side, d.first, d.last).to_lowercase()
                    )
                })
            })
            .unwrap_or_else(|| "Editing a comment".into()),
    }
}

/// A row of a card: the first of them on screen draws the whole card over them, centred in them;
/// the rest stay empty (rows that have scrolled away aren't drawn at all).
pub fn card_row<T: ReviewHost>(
    this: &T,
    ix: usize,
    first_shown: usize,
    card: Card,
    part: usize,
    rows: usize,
    row: f32,
    left: f32,
    width: f32,
    cx: &mut Context<T>,
) -> AnyElement {
    let blank = div().h(px(row)).w_full();
    if part != 0 && ix != first_shown {
        return blank.into_any_element();
    }
    let mut height = EDITOR_HEIGHT;
    let el = match card {
        Card::Editor => this.review().editing.as_ref().map(|e| {
            let label = writing_label(this, &e.what, cx);
            editor_card(e, label, matches!(e.what, Writing::New(_)), cx)
        }),
        Card::Draft(id) => {
            let draft = this.notes_key(cx).and_then(|key| {
                let ws = this.workspace_entity();
                let ws = ws.read(cx);
                ws.review_notes
                    .get(&key)?
                    .drafts
                    .iter()
                    .find(|d| d.id == id)
                    .cloned()
            });
            draft.map(|d| {
                height = saved_height(&d.body, width, cx);
                saved_card(&d, cx)
            })
        }
    };
    let Some(el) = el else {
        return blank.into_any_element();
    };
    let room = rows as f32 * row - height;
    blank
        .relative()
        .child(
            el.absolute()
                .left(px(left))
                .top(px(room / 2. - part as f32 * row))
                .w(px(width))
                .test_support(),
        )
        .into_any_element()
}

/// A card's width in a list `list` wide, with its left edge at `left`.
pub fn card_width(list: f32, left: f32) -> f32 {
    (list - left - 12.).clamp(200., CARD_WIDTH)
}

/// The cursor's marks on a row, and on a line the way to comment on it: a press puts the cursor
/// there and a drag selects; the "+" in the gutter shows on hover (and stays on a selection's last
/// line, so what's selected has somewhere to go).
pub fn line_chrome<T: ReviewHost>(
    el: Stateful<Div>,
    ix: usize,
    marks: Marks,
    plus_left: f32,
    row: f32,
    cx: &mut Context<T>,
) -> Stateful<Div> {
    let theme = cx.theme().clone();
    let ember = palette::ember(cx);
    let plus_size = (row - 5.).min(17.);
    let plus_tip = format!(
        "Comment ({})",
        crate::keys::hint(crate::keys::Id::DiffComment)
    );
    el.group("diff-line")
        .relative()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this: &mut T, e: &MouseDownEvent, _, cx| {
                press_line(this, ix, e.modifiers.shift, cx)
            }),
        )
        .on_mouse_move(cx.listener(move |this: &mut T, e: &MouseMoveEvent, _, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                drag_line(this, ix, cx);
            } else {
                this.review_mut().drag_from = None;
            }
        }))
        .when(marks.selected, |el| {
            el.child(div().absolute().inset_0().bg(theme.info.opacity(0.14)))
        })
        .when(marks.current, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme.foreground.opacity(0.07)),
            )
        })
        .when(marks.in_hunk || marks.current, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(if marks.current {
                        ember
                    } else {
                        ember.opacity(0.4)
                    }),
            )
        })
        .child(
            div()
                .id(("diff-line-plus", ix))
                .test_support()
                .absolute()
                .left(px(plus_left))
                .top(px((row - plus_size) / 2.))
                .size(px(plus_size))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::TAG))
                .cursor_pointer()
                .bg(ember)
                .when(!marks.selection_end, |el| {
                    el.invisible().group_hover("diff-line", |s| s.visible())
                })
                .hover(|s| s.opacity(0.85))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(plus_tip.clone()).build(window, cx)
                })
                .child(
                    Icon::new(IconName::Plus)
                        .size(px(11.))
                        .text_color(cx.theme().primary_foreground),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this: &mut T, _, window, cx| {
                    cx.stop_propagation();
                    if this
                        .review()
                        .selection()
                        .is_none_or(|(lo, hi)| !(lo..=hi).contains(&ix))
                    {
                        press_line(this, ix, false, cx);
                    }
                    this.review_mut().drag_from = None;
                    begin_comment(this, window, cx);
                })),
        )
}

/// The cursor's mark on a hunk's head or a file's: the bar, no comment to start there.
pub fn head_chrome(el: Stateful<Div>, marks: Marks, cx: &App) -> Stateful<Div> {
    let theme = cx.theme().clone();
    let ember = palette::ember(cx);
    el.relative()
        .when(marks.current, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme.foreground.opacity(0.07)),
            )
        })
        .when(marks.in_hunk || marks.current, |el| {
            el.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(if marks.current {
                        ember
                    } else {
                        ember.opacity(0.4)
                    }),
            )
        })
}

/// The strip under a diff while it has comments: how many, Clear, Send to agent; opened, each
/// comment with its place, to go to it, edit it or remove it. The comment box shows here too when
/// its line isn't in this diff.
pub fn tray<T: ReviewHost>(this: &T, cx: &mut Context<T>) -> Option<AnyElement> {
    let key = this.notes_key(cx)?;
    let drafts = {
        let ws = this.workspace_entity();
        let ws = ws.read(cx);
        sorted(
            ws.review_notes
                .get(&key)
                .map(|n| n.drafts.clone())
                .unwrap_or_default(),
        )
    };
    let review = this.review();
    let stray_editor = review.editing.as_ref().filter(|_| !review.editing_placed);
    if drafts.is_empty() && stray_editor.is_none() {
        return None;
    }
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let open = review.tray_open;
    let n = drafts.len();
    let stray = stray_editor.map(|e| {
        let label = writing_label(this, &e.what, cx);
        div()
            .p(px(8.))
            .child(editor_card(e, label, matches!(e.what, Writing::New(_)), cx).test_support())
    });
    let list = open.then(|| {
        v_flex()
            .id("diff-comments-list")
            .test_support()
            .max_h(px(180.))
            .overflow_y_scroll()
            .py(px(2.))
            .text_size(px(text::LABEL))
            .children(drafts.iter().enumerate().map(|(i, d)| {
                let (id, draft) = (d.id, d.clone());
                h_flex()
                    .id(("diff-comments-row", i))
                    .test_support()
                    .group("diff-comments-row")
                    .h(px(26.))
                    .px(px(12.))
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.5))
                            .truncate()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(text::LABEL))
                            .text_color(muted)
                            .child(d.place()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground.opacity(0.85))
                            .child(d.body.replace('\n', " ")),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(2.))
                            .invisible()
                            .group_hover("diff-comments-row", |s| s.visible())
                            .child(
                                small(("diff-comments-edit", i), "Edit", cx)
                                    .on_click(cx.listener(move |this: &mut T, _, window, cx| {
                                        cx.stop_propagation();
                                        begin_edit(this, id, window, cx)
                                    }))
                                    .test_support(),
                            )
                            .child(
                                small(("diff-comments-remove", i), "Remove", cx)
                                    .on_click(cx.listener(move |this: &mut T, _, window, cx| {
                                        cx.stop_propagation();
                                        remove_comment(this, id, window, cx)
                                    }))
                                    .test_support(),
                            ),
                    )
                    .on_click(cx.listener(move |this: &mut T, _, _, cx| {
                        this.show_draft(&draft, cx);
                        this.review_mut().reveal = Some(id);
                        this.rows_changed(cx);
                    }))
            }))
    });
    let send_tip = format!(
        "Send to agent ({})",
        crate::keys::hint(crate::keys::Id::DiffSend)
    );
    let strip = h_flex()
        .id("diff-comments")
        .test_support()
        .h(px(32.))
        .flex_none()
        .px(px(12.))
        .gap(px(6.))
        .items_center()
        .text_size(px(text::LABEL))
        .text_color(muted)
        .child(
            link("diff-comments-toggle")
                .ml(px(-7.))
                .when(n == 0, |el| el.invisible())
                .hover({
                    let (bg, fg) = (theme.foreground.opacity(0.07), theme.foreground);
                    move |s| s.bg(bg).text_color(fg)
                })
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.)),
                )
                .child(
                    div()
                        .text_color(theme.foreground.opacity(0.85))
                        .child(format!("{n} comment{}", if n == 1 { "" } else { "s" })),
                )
                .on_click(cx.listener(|this: &mut T, _, _, cx| {
                    let review = this.review_mut();
                    review.tray_open = !review.tray_open;
                    cx.notify();
                }))
                .test_support(),
        )
        .child(div().flex_1())
        .when(n > 0, |el| {
            el.child(
                link("diff-comments-clear")
                    .hover({
                        let (bg, fg) = (theme.foreground.opacity(0.07), theme.foreground);
                        move |s| s.bg(bg).text_color(fg)
                    })
                    .child("Clear")
                    .on_click(cx.listener(|this: &mut T, _, _, cx| clear_comments(this, cx)))
                    .test_support(),
            )
            .child(
                ember_link("diff-comments-send", cx)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(send_tip.clone())
                            .build(window, cx)
                    })
                    .child("Send to agent")
                    .on_click(
                        cx.listener(|this: &mut T, _, window, cx| send_comments(this, window, cx)),
                    )
                    .test_support(),
            )
        });
    Some(
        v_flex()
            .flex_none()
            .border_t_1()
            .border_color(theme.border.opacity(0.6))
            .children(list)
            .children(stray)
            .child(strip)
            .into_any_element(),
    )
}

/// `· x of y viewed`, after what comes before it in a header.
pub fn viewed_label(viewed: usize, of: usize) -> String {
    format!("· {viewed} of {of} viewed")
}

#[cfg(test)]
mod tests {
    use super::{
        Card, DiffReview, Draft, FileLine, Nav, Notes, Placed, QUOTE_CHARS, Side, Spot, anchor_of,
        card_ends, first_stop, landing, locate, merge, prompt, quote_of, sig_of, step_file,
        step_hunk, step_line,
    };

    fn draft(id: u64, path: &str, first: u32, last: u32, body: &str) -> Draft {
        Draft {
            id,
            path: path.into(),
            side: Side::New,
            first,
            last,
            quote: vec!["+let x = 1;".into()],
            body: body.into(),
        }
    }

    #[test]
    fn a_message_lists_each_comment_with_its_place_code_and_words_in_file_order() {
        let mut gone = draft(2, "b.rs", 7, 9, "Why did this go?");
        gone.side = Side::Old;
        gone.quote = vec!["-old();".into(), "-older();".into()];
        let message = prompt(&[gone, draft(1, "a.rs", 42, 42, "  Use bar() here.\n")]);
        assert_eq!(
            message,
            "I left 2 comments on your changes. Please address them together:\n\
             \n1. a.rs:42\n> +let x = 1;\nUse bar() here.\n\
             \n2. b.rs:7-9 (removed lines)\n> -old();\n> -older();\nWhy did this go?"
        );
        assert!(prompt(&[draft(1, "a.rs", 1, 1, "x")]).starts_with("I left a comment"));
    }

    #[test]
    fn a_quote_is_brief() {
        let lines: Vec<String> = (0..5).map(|n| format!("+line {n}\t")).collect();
        assert_eq!(quote_of(&lines), ["+line 0", "+line 1", "+line 2", "…"]);
        let long = format!("+{}", "x".repeat(300));
        let q = quote_of(std::slice::from_ref(&long));
        assert_eq!(q[0].chars().count(), QUOTE_CHARS + 1);
        assert!(q[0].ends_with('…'));
    }

    fn line(file: usize, hunk: usize, old: Option<u32>, new: Option<u32>, changed: bool) -> Nav {
        Nav::Line {
            file,
            hunk: Some(hunk),
            old,
            new,
            changed,
        }
    }

    /// Two files: the first with two hunks, the second collapsed-looking (a head alone) then a third.
    fn rows() -> Vec<Nav> {
        vec![
            Nav::File { file: 0 },               // 0
            Nav::Hunk { file: 0, hunk: 0 },      // 1
            line(0, 0, Some(1), Some(1), false), // 2
            line(0, 0, Some(2), None, true),     // 3
            line(0, 0, None, Some(2), true),     // 4
            Nav::Skip,                           // 5
            Nav::Hunk { file: 0, hunk: 1 },      // 6
            line(0, 1, None, Some(9), true),     // 7
            Nav::Skip,                           // 8
            Nav::File { file: 1 },               // 9
            Nav::File { file: 2 },               // 10
            Nav::Hunk { file: 2, hunk: 0 },      // 11
            line(2, 0, None, Some(1), true),     // 12
        ]
    }

    #[test]
    fn the_cursor_steps_over_lines_hunks_and_files() {
        let nav = rows();
        assert_eq!(first_stop(&nav), Some(3));
        assert_eq!(
            step_line(&nav, Some(4), 1),
            Some(7),
            "over a head and a card"
        );
        assert_eq!(
            step_line(&nav, Some(7), 1),
            Some(9),
            "a collapsed file's head"
        );
        assert_eq!(step_line(&nav, Some(2), -1), Some(0));
        assert_eq!(step_line(&nav, Some(12), 1), None);
        assert_eq!(step_line(&nav, None, 1), Some(3));

        assert_eq!(step_hunk(&nav, None, 1), Some(1));
        assert_eq!(step_hunk(&nav, Some(3), 1), Some(6));
        assert_eq!(
            step_hunk(&nav, Some(7), 1),
            Some(11),
            "on into the next file"
        );
        assert_eq!(step_hunk(&nav, Some(12), 1), None);
        assert_eq!(step_hunk(&nav, Some(7), -1), Some(1), "not its own head");
        assert_eq!(step_hunk(&nav, Some(3), -1), None);
        assert_eq!(landing(&nav, 1), 3, "the first change, past the context");
        assert_eq!(landing(&nav, 9), 9, "collapsed: its head");
        assert_eq!(landing(&nav, 0), 3);

        assert_eq!(step_file(&nav, Some(3), 1), Some(9));
        assert_eq!(step_file(&nav, Some(9), 1), Some(10));
        assert_eq!(step_file(&nav, Some(10), 1), None);
        assert_eq!(step_file(&nav, Some(12), -1), Some(9));
        assert_eq!(step_file(&nav, Some(3), -1), None);
    }

    fn paths(file: usize) -> Option<String> {
        Some(format!("f{file}.rs"))
    }

    fn texts(ix: usize) -> Vec<String> {
        vec![format!("+row {ix}")]
    }

    #[test]
    fn a_comment_on_a_range_is_numbered_as_the_new_file_unless_only_removed_lines_are_in_it() {
        let nav = rows();
        let one = anchor_of(&nav, 4, None, &paths, &texts).unwrap();
        assert_eq!(
            (one.side, one.first, one.last, one.path.as_str()),
            (Side::New, 2, 2, "f0.rs")
        );
        let gone = anchor_of(&nav, 3, None, &paths, &texts).unwrap();
        assert_eq!((gone.side, gone.first, gone.last), (Side::Old, 2, 2));
        let range = anchor_of(&nav, 4, Some(2), &paths, &texts).unwrap();
        assert_eq!((range.side, range.first, range.last), (Side::New, 1, 2));
        assert_eq!(range.quote, ["+row 2", "+row 3", "+row 4"]);
        // A selection dragged across files keeps to the cursor's.
        let across = anchor_of(&nav, 12, Some(4), &paths, &texts).unwrap();
        assert_eq!(across.path, "f2.rs");
        assert_eq!((across.first, across.last), (1, 1));
        assert!(
            anchor_of(&nav, 9, None, &paths, &texts).is_none(),
            "a collapsed file has no line to comment on"
        );
    }

    #[test]
    fn a_comment_stays_with_its_code_as_the_diff_shifts_and_goes_quiet_when_the_code_is_gone() {
        let lines: Vec<FileLine> = (0..6)
            .map(|i| FileLine {
                row: i,
                old: None,
                new: Some(10 + i as u32),
            })
            .collect();
        let text = |row: usize| vec![format!("+code {row}")];
        // Where it was.
        assert_eq!(
            locate(Side::New, 12, 13, &["+code 2".to_string()], &lines, &text),
            Some(Spot {
                first: 12,
                last: 13,
                after: 3
            })
        );
        // The same code four lines down: the range moves with it.
        assert_eq!(
            locate(Side::New, 8, 9, &["+code 2".to_string()], &lines, &text),
            Some(Spot {
                first: 12,
                last: 13,
                after: 3
            })
        );
        assert_eq!(
            locate(
                Side::New,
                12,
                12,
                &["+no such code".to_string()],
                &lines,
                &text
            ),
            None
        );
        // The other side's numbers don't count.
        assert_eq!(
            locate(Side::Old, 12, 12, &["+code 2".to_string()], &lines, &text),
            None
        );
    }

    #[test]
    fn cards_add_their_rows_and_the_cursor_stays_on_what_it_was_on() {
        let rows_in: Vec<&str> = vec!["a", "b", "c"];
        let nav = vec![
            line(0, 0, None, Some(1), true),
            line(0, 0, None, Some(2), true),
            line(0, 0, None, Some(3), true),
        ];
        let mut review = DiffReview {
            cursor: Some(2),
            ..Default::default()
        };
        let was = review.remember(&nav);
        let placed = vec![Placed {
            after: 0,
            card: Card::Draft(1),
            rows: 2,
        }];
        let (out, out_nav) = merge(rows_in, nav, &placed, |_, _| "card");
        assert_eq!(out, ["a", "card", "card", "b", "c"]);
        review.restore(was, &out_nav);
        assert_eq!(review.cursor, Some(4));
        assert_eq!(card_ends(&placed), [(Card::Draft(1), 2)]);
    }

    #[test]
    fn a_cursor_whose_line_went_lands_near_where_it_was() {
        let nav = rows();
        let mut review = DiffReview {
            cursor: Some(7),
            ..Default::default()
        };
        let was = review.remember(&nav);
        // Hunk 1 gone (staged): the next line on.
        let after: Vec<Nav> = nav[..6]
            .iter()
            .copied()
            .chain(nav[9..].iter().copied())
            .collect();
        review.restore(was, &after);
        assert_eq!(
            after[review.cursor.unwrap()],
            line(2, 0, None, Some(1), true)
        );
    }

    #[test]
    fn what_a_diff_says_is_told_apart_by_its_lines_not_its_heads() {
        let a = sig_of(["@@ -1 +1 @@", "-a", "+b"].into_iter());
        let moved = sig_of(["@@ -9 +9 @@", "-a", "+b"].into_iter());
        let other = sig_of(["@@ -1 +1 @@", "-a", "+c"].into_iter());
        assert_eq!(a, moved);
        assert_ne!(a, other);
    }

    #[test]
    fn notes_read_back_as_they_were_saved() {
        let mut notes = Notes::default();
        notes.drafts.push(draft(1, "a.rs", 1, 2, "x"));
        notes.viewed.insert("a.rs".into(), "abc".into());
        let text = serde_json::to_string(&notes).unwrap();
        assert_eq!(serde_json::from_str::<Notes>(&text).unwrap(), notes);
        assert_eq!(
            serde_json::from_str::<Notes>("{}").unwrap(),
            Notes::default()
        );
    }
}
