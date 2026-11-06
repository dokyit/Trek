//! A diff in an editor tab: the IDE's Review (what a chat's agent changed, pending Keep or Undo)
//! and Source Control's file diffs. Inline by default, as VS Code's and Cursor's diff editors
//! are: a header per file, `@@` hunk heads, old and new line numbers, added lines on green and
//! removed ones on red, and the unchanged stretches between hunks folded ("⋯ 42 unchanged
//! lines", a click opens them). Side by side on the toolbar's toggle. A Review's files and hunks
//! each have Undo and Keep (`Workspace::undo_files`, `keep_hunk`, …); Source Control's files
//! open in the editor from their header, and their hunks stage and unstage one at a time. Line
//! comments, the keys and viewed files are `diff_review`'s.
//!
//! The hunks are read off the main thread: a Review's against its baseline
//! (`checkpoint::Repo::file_diff`, the same cut the editor's hunk bar keeps and undoes), git's
//! with `git diff`. They're read again when the review moves or files change.

use super::IdeWorkbench;
use crate::diff_review::{self, Card, DiffReview, Draft, Nav, ReviewHost};
use crate::fluent;
use crate::palette;
use crate::scale::{radius, text};
use crate::workspace::{Scope, Workspace, WorkspaceEvent};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_core::hunks::{FileDiff, Hunk};

/// Every row is this tall: the list is uniform, so a long diff only lays out what's in view.
const ROW: f32 = 22.;
/// The largest file whose text is read to open folded stretches (and to show a new file).
const READ_LIMIT: u64 = 2 << 20;
/// Rows drawn at most; past it a note says how much more there is.
const MAX_ROWS: usize = 40_000;
/// Characters of a line drawn at most.
const LINE_CHARS: usize = 2_000;

/// What a diff tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// A chat's review: its pending files against the baseline, each hunk to keep or undo.
    Review { thread: String },
    /// One file in git: the working tree against the index, or (`staged`) the index against
    /// HEAD. A file git doesn't track yet shows as all added.
    Git {
        top: PathBuf,
        rel: String,
        staged: bool,
    },
}

/// One file's diff, as read.
#[derive(Debug, Clone)]
pub struct DiffFile {
    pub rel: String,
    /// `A` added (or untracked), `M` changed, `D` deleted.
    pub status: char,
    pub diff: FileDiff,
    /// The new side's lines, to open folded stretches with (empty when it wasn't read).
    pub new_lines: Arc<Vec<String>>,
    /// Binary, or too large to show: no lines.
    pub note: Option<String>,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Add,
    Del,
    Ctx,
}

/// One side of a side-by-side row: its line number, what it is, its text.
type Side = (u32, Kind, SharedString);

#[derive(Debug, Clone)]
enum Row {
    /// Room between one file and the next.
    Space,
    File(usize),
    Hunk {
        file: usize,
        hunk: usize,
        head: SharedString,
    },
    Line {
        kind: Kind,
        old: Option<u32>,
        new: Option<u32>,
        text: SharedString,
    },
    Pair {
        left: Option<Side>,
        right: Option<Side>,
    },
    /// `lines` unchanged lines from new line `at` (1-based), folded.
    Fold {
        file: usize,
        at: u32,
        lines: u32,
    },
    Note(SharedString),
    /// A row a comment's card covers (`diff_review::card_row`).
    Card {
        card: Card,
        part: usize,
        rows: usize,
    },
}

/// How a line reads on screen: tabs as four spaces, no carriage return, cut when very long.
fn shown(text: &str) -> SharedString {
    let text = text.trim_end_matches(['\n', '\r']).replace('\t', "    ");
    if text.chars().count() > LINE_CHARS {
        return format!("{}…", text.chars().take(LINE_CHARS).collect::<String>()).into();
    }
    text.into()
}

/// A hunk's lines as rows (`Line`s, or `Pair`s side by side), its head first.
fn hunk_rows(file: usize, ix: usize, hunk: &Hunk, split: bool, out: &mut Vec<Row>) {
    let mut lines = hunk.body.split_inclusive('\n');
    let head = lines.next().unwrap_or_default();
    out.push(Row::Hunk {
        file,
        hunk: ix,
        head: shown(head),
    });
    let (mut old, mut new) = (hunk.old_start.max(1), hunk.new_start.max(1));
    if hunk.old_count == 0 {
        old = hunk.old_start + 1;
    }
    if hunk.new_count == 0 {
        new = hunk.new_start + 1;
    }
    // Side by side, a run of removed lines then added ones pairs up row by row.
    let (mut dels, mut adds): (Vec<Side>, Vec<Side>) = (vec![], vec![]);
    let flush = |dels: &mut Vec<Side>, adds: &mut Vec<Side>, out: &mut Vec<Row>| {
        let n = dels.len().max(adds.len());
        let (mut d, mut a) = (dels.drain(..), adds.drain(..));
        for _ in 0..n {
            out.push(Row::Pair {
                left: d.next(),
                right: a.next(),
            });
        }
    };
    for line in lines {
        let (sign, text) = line.split_at(line.len().min(1));
        match sign {
            "+" => {
                if split {
                    adds.push((new, Kind::Add, shown(text)));
                } else {
                    out.push(Row::Line {
                        kind: Kind::Add,
                        old: None,
                        new: Some(new),
                        text: shown(text),
                    });
                }
                new += 1;
            }
            "-" => {
                if split {
                    if !adds.is_empty() {
                        flush(&mut dels, &mut adds, out);
                    }
                    dels.push((old, Kind::Del, shown(text)));
                } else {
                    out.push(Row::Line {
                        kind: Kind::Del,
                        old: Some(old),
                        new: None,
                        text: shown(text),
                    });
                }
                old += 1;
            }
            "\\" => {
                flush(&mut dels, &mut adds, out);
                out.push(Row::Note("No newline at end of file".into()));
            }
            _ => {
                flush(&mut dels, &mut adds, out);
                let text = shown(text);
                if split {
                    out.push(Row::Pair {
                        left: Some((old, Kind::Ctx, text.clone())),
                        right: Some((new, Kind::Ctx, text)),
                    });
                } else {
                    out.push(Row::Line {
                        kind: Kind::Ctx,
                        old: Some(old),
                        new: Some(new),
                        text,
                    });
                }
                old += 1;
                new += 1;
            }
        }
    }
    flush(&mut dels, &mut adds, out);
}

/// Where hunk `h` starts and ends on each side: (first old, first new, old after, new after),
/// 1-based. An empty side starts just after the line its `@@` names.
fn span(h: &Hunk) -> (u32, u32, u32, u32) {
    let first_old = if h.old_count == 0 {
        h.old_start + 1
    } else {
        h.old_start
    };
    let first_new = if h.new_count == 0 {
        h.new_start + 1
    } else {
        h.new_start
    };
    (
        first_old,
        first_new,
        first_old + h.old_count,
        first_new + h.new_count,
    )
}

/// Every file's rows: its header, then (unless collapsed) its hunks with the unchanged
/// stretches between them folded, or opened where `opened` says (file, first new line).
fn build_rows(
    files: &[DiffFile],
    collapsed: &HashSet<String>,
    opened: &HashSet<(String, u32)>,
    split: bool,
) -> Vec<Row> {
    let mut out = vec![];
    for (f, file) in files.iter().enumerate() {
        if out.len() >= MAX_ROWS {
            out.push(Row::Note(
                format!(
                    "{} more file{} not shown",
                    files.len() - f,
                    if files.len() - f == 1 { "" } else { "s" }
                )
                .into(),
            ));
            break;
        }
        if f > 0 {
            out.push(Row::Space);
        }
        out.push(Row::File(f));
        if collapsed.contains(&file.rel) {
            continue;
        }
        if let Some(note) = &file.note {
            out.push(Row::Note(note.clone().into()));
            continue;
        }
        if file.diff.hunks.is_empty() {
            out.push(Row::Note("No changes in its lines".into()));
            continue;
        }
        // The next line of each side not drawn yet (1-based).
        let (mut old_next, mut new_next) = (1u32, 1u32);
        let total = file.new_lines.len() as u32;
        let gap = |out: &mut Vec<Row>, from_new: u32, to_new: u32, offset: i64| {
            if to_new <= from_new {
                return;
            }
            let lines = to_new - from_new;
            if opened.contains(&(file.rel.clone(), from_new)) && !file.new_lines.is_empty() {
                for n in from_new..to_new {
                    let text = file
                        .new_lines
                        .get(n as usize - 1)
                        .map(|s| shown(s))
                        .unwrap_or_default();
                    let old = (n as i64 + offset).max(1) as u32;
                    out.push(if split {
                        Row::Pair {
                            left: Some((old, Kind::Ctx, text.clone())),
                            right: Some((n, Kind::Ctx, text)),
                        }
                    } else {
                        Row::Line {
                            kind: Kind::Ctx,
                            old: Some(old),
                            new: Some(n),
                            text,
                        }
                    });
                }
            } else {
                out.push(Row::Fold {
                    file: f,
                    at: from_new,
                    lines,
                });
            }
        };
        for (h, hunk) in file.diff.hunks.iter().enumerate() {
            let (first_old, first_new, old_after, new_after) = span(hunk);
            // Lines before it that didn't change (none to show for a file that's all new or gone).
            if file.status == 'M' {
                gap(
                    &mut out,
                    new_next,
                    first_new,
                    first_old as i64 - first_new as i64,
                );
            }
            hunk_rows(f, h, hunk, split, &mut out);
            old_next = old_after;
            new_next = new_after;
            if out.len() >= MAX_ROWS {
                break;
            }
        }
        if file.status == 'M' && total >= new_next {
            gap(
                &mut out,
                new_next,
                total + 1,
                old_next as i64 - new_next as i64,
            );
        }
    }
    out
}

/// What `rows` (no cards among them yet) are, to move around on.
fn nav_of(rows: &[Row], files: &[DiffFile]) -> Vec<Nav> {
    let mut file = 0;
    // The hunk a line is in, by its numbers: opened out of a fold, it isn't in one.
    let hunk_of = |file: usize, old: Option<u32>, new: Option<u32>| -> Option<usize> {
        files.get(file)?.diff.hunks.iter().position(|h| {
            let (first_old, first_new, old_after, new_after) = span(h);
            new.is_some_and(|n| first_new <= n && n < new_after)
                || old.is_some_and(|o| first_old <= o && o < old_after)
        })
    };
    rows.iter()
        .map(|row| match row {
            Row::File(f) => {
                file = *f;
                Nav::File { file }
            }
            Row::Hunk { file, hunk, .. } => Nav::Hunk {
                file: *file,
                hunk: *hunk,
            },
            Row::Line { kind, old, new, .. } => Nav::Line {
                file,
                hunk: hunk_of(file, *old, *new),
                old: *old,
                new: *new,
                changed: *kind != Kind::Ctx,
            },
            Row::Pair { left, right } => {
                let (old, new) = (
                    left.as_ref().map(|(n, ..)| *n),
                    right.as_ref().map(|(n, ..)| *n),
                );
                Nav::Line {
                    file,
                    hunk: hunk_of(file, old, new),
                    old,
                    new,
                    changed: [left, right]
                        .into_iter()
                        .flatten()
                        .any(|(_, k, _)| *k != Kind::Ctx),
                }
            }
            _ => Nav::Skip,
        })
        .collect()
}

/// The lines of a row as a diff has them, `+`, `-` or a space first.
fn row_text(row: &Row) -> Vec<String> {
    let line = |kind: Kind, text: &SharedString| match kind {
        Kind::Add => format!("+{text}"),
        Kind::Del => format!("-{text}"),
        Kind::Ctx => format!(" {text}"),
    };
    match row {
        Row::Line { kind, text, .. } => vec![line(*kind, text)],
        // Unchanged, both sides say the same: once is enough.
        Row::Pair {
            left: Some((_, Kind::Ctx, text)),
            right: Some(_),
        } => vec![line(Kind::Ctx, text)],
        Row::Pair { left, right } => [left, right]
            .into_iter()
            .flatten()
            .map(|(_, kind, text)| line(*kind, text))
            .collect(),
        _ => vec![],
    }
}

/// What a file's diff says (`diff_review::sig_of`): a binary one by its counts.
fn file_sig(f: &DiffFile) -> String {
    let bodies = f.diff.hunks.iter().flat_map(|h| h.body.lines());
    if f.diff.hunks.is_empty() {
        let counts = format!("{}+{}-{}", f.status, f.added, f.removed);
        return diff_review::sig_of(std::iter::once(counts.as_str()));
    }
    diff_review::sig_of(bodies)
}

/// Take a hunk into the index (`reverse`: back out of it).
fn apply_to_index(top: &Path, patch: &str, reverse: bool) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut args = vec!["apply", "--cached", "--recount", "--whitespace=nowarn"];
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let mut child = super::git::command(top)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The text of a file on the new side, as lines: `None` when it's binary or too large.
fn read_lines(bytes: Option<Vec<u8>>) -> Result<Vec<String>, &'static str> {
    let Some(bytes) = bytes else {
        return Ok(vec![]);
    };
    if bytes.len() as u64 > READ_LIMIT {
        return Err("Too large to show");
    }
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("Binary file");
    }
    let text = String::from_utf8(bytes).map_err(|_| "Binary file")?;
    Ok(text.lines().map(str::to_string).collect())
}

fn read_disk(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    if meta.len() > READ_LIMIT {
        return Some(vec![0; (READ_LIMIT + 1) as usize]);
    }
    std::fs::read(path).ok()
}

/// What a file's diff header says it is: added, deleted, or changed.
fn status_of(diff: &FileDiff) -> char {
    if diff.header.contains("\nnew file mode") || diff.header.contains("--- /dev/null") {
        'A'
    } else if diff.header.contains("\ndeleted file mode") || diff.header.contains("+++ /dev/null") {
        'D'
    } else {
        'M'
    }
}

/// A file read: its hunks, the new side's lines, and the counts.
fn file(rel: &str, diff: FileDiff, new_side: Option<Vec<u8>>) -> DiffFile {
    let status = status_of(&diff);
    let binary = diff.hunks.is_empty() && diff.header.contains("Binary files");
    let (lines, mut note) = match read_lines(new_side) {
        Ok(lines) => (lines, None),
        Err(why) => (vec![], Some(why.to_string())),
    };
    if binary {
        note = Some("Binary file".into());
    }
    let (added, removed) = diff
        .hunks
        .iter()
        .map(Hunk::counts)
        .fold((0, 0), |(a, r), (da, dr)| (a + da, r + dr));
    DiffFile {
        rel: rel.to_string(),
        status,
        diff,
        new_lines: Arc::new(lines),
        note,
        added,
        removed,
    }
}

/// A file git doesn't track yet, as a diff that adds every line.
fn untracked(rel: &str, bytes: Option<Vec<u8>>) -> DiffFile {
    let header =
        format!("diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n");
    match read_lines(bytes) {
        Ok(lines) => {
            let n = lines.len() as u32;
            let mut body = format!("@@ -0,0 +1,{n} @@\n");
            for l in lines.iter() {
                body.push('+');
                body.push_str(l);
                body.push('\n');
            }
            let hunks = if n == 0 {
                vec![]
            } else {
                vec![Hunk {
                    old_start: 0,
                    old_count: 0,
                    new_start: 1,
                    new_count: n,
                    body,
                }]
            };
            DiffFile {
                rel: rel.to_string(),
                status: 'A',
                diff: FileDiff { header, hunks },
                new_lines: Arc::new(lines),
                note: None,
                added: n,
                removed: 0,
            }
        }
        Err(why) => DiffFile {
            rel: rel.to_string(),
            status: 'A',
            diff: FileDiff {
                header,
                hunks: vec![],
            },
            new_lines: Arc::new(vec![]),
            note: Some(why.into()),
            added: 0,
            removed: 0,
        },
    }
}

/// A Review's pending files against its baseline, cut into hunks as Keep and Undo take them.
fn load_review(top: &Path, base: &str, files: &[String]) -> Result<Vec<DiffFile>, String> {
    let repo = trek_core::checkpoint::Repo::find(top)
        .ok_or_else(|| format!("{} isn't a git repository any more", top.display()))?;
    files
        .iter()
        .filter_map(|rel| match repo.file_diff(base, rel) {
            // Back as it was (kept or undone meanwhile): nothing to show for it.
            Ok(diff) if diff.hunks.is_empty() && diff.header.is_empty() => None,
            Ok(diff) => Some(Ok(file(rel, diff, read_disk(&top.join(rel))))),
            Err(e) => Some(Err(format!("{e:#}"))),
        })
        .collect()
}

/// One file's diff in git: the working tree against the index, or the index against HEAD.
fn load_git(top: &Path, rel: &str, staged: bool) -> Result<Vec<DiffFile>, String> {
    let tracked = super::git::run(top, &["ls-files", "--error-unmatch", "--", rel]).is_ok();
    if !tracked && !staged {
        return Ok(vec![untracked(rel, read_disk(&top.join(rel)))]);
    }
    let mut args = vec!["diff", "--no-ext-diff", "--no-color", "--no-renames", "-U3"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", rel]);
    let out = super::git::run(top, &args)?;
    // Nothing changed (any more): no file to show.
    if out.trim().is_empty() {
        return Ok(vec![]);
    }
    let diff = FileDiff::parse(&out);
    let new_side = if staged {
        super::git::command(top)
            .args(["show", &format!(":{rel}")])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    } else {
        read_disk(&top.join(rel))
    };
    Ok(vec![file(rel, diff, new_side)])
}

pub struct DiffView {
    workspace: Entity<Workspace>,
    workbench: WeakEntity<IdeWorkbench>,
    pub source: DiffSource,
    files: Vec<DiffFile>,
    rows: std::rc::Rc<Vec<Row>>,
    /// Files folded down to their header, by path.
    collapsed: HashSet<String>,
    /// Unchanged stretches opened: (file, its first new line).
    opened: HashSet<(String, u32)>,
    /// Files marked viewed that were opened again all the same.
    viewed_open: HashSet<String>,
    split: bool,
    loading: bool,
    error: Option<String>,
    /// What the files were last read at (see `signature`): reading again only when it moves.
    seen: Option<String>,
    run: u64,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// Comments, the cursor and the keys (`diff_review`).
    review: DiffReview,
    /// What each row is, to move around on.
    nav: Vec<Nav>,
    /// The cards among the rows (`diff_review::card_ends`).
    cards: Vec<(Card, usize)>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DiffView {
    pub fn new(
        workspace: Entity<Workspace>,
        workbench: WeakEntity<IdeWorkbench>,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&workspace, |this, ws, cx| {
                this.maybe_reload(cx);
                if ws.read(cx).review_notes_epoch != this.review.seen_epoch {
                    this.rebuild(cx);
                }
            }),
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| {
                if let (WorkspaceEvent::ReviewChanged { id }, DiffSource::Review { thread }) =
                    (event, &this.source)
                {
                    if id == thread {
                        this.reload(cx);
                    }
                }
            }),
        ];
        let mut this = Self {
            workspace,
            workbench,
            source,
            files: vec![],
            rows: Default::default(),
            collapsed: HashSet::new(),
            opened: HashSet::new(),
            viewed_open: HashSet::new(),
            split: false,
            loading: true,
            error: None,
            seen: None,
            run: 0,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            review: DiffReview::default(),
            nav: vec![],
            cards: vec![],
            _load: None,
            _subscriptions: subscriptions,
        };
        this.maybe_reload(cx);
        this
    }

    /// The tab's title: "Review · Add a note" (its chat), "main.rs (Working Tree)", "main.rs (Index)".
    pub fn title(&self, cx: &App) -> String {
        match &self.source {
            // Named by its chat: two chats' reviews can be open at once.
            DiffSource::Review { thread } => {
                let title = self
                    .workspace
                    .read(cx)
                    .thread(thread)
                    .map(|t| t.title.clone())
                    .unwrap_or_default();
                let title = if title.chars().count() > 28 {
                    format!("{}…", title.chars().take(27).collect::<String>())
                } else {
                    title
                };
                if title.is_empty() {
                    "Review".into()
                } else {
                    format!("Review · {title}")
                }
            }
            DiffSource::Git { rel, staged, .. } => {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                format!(
                    "{name} ({})",
                    if *staged { "Index" } else { "Working Tree" }
                )
            }
        }
    }

    /// What decides whether the files need reading again: the review's pending files and
    /// baseline, or (for git) files changing on disk.
    fn signature(&self, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        let epoch = format!(
            "{}:{}:{}",
            ws.files_epoch, ws.agent_edits, ws.turns_finished
        );
        match &self.source {
            DiffSource::Review { thread } => {
                let base = ws
                    .review_base(thread)
                    .map(|(t, b)| format!("{}@{b}", t.display()))
                    .unwrap_or_default();
                let files: Vec<String> = ws
                    .pending_files(thread)
                    .iter()
                    .map(|f| format!("{}+{}-{}", f.path, f.added, f.removed))
                    .collect();
                format!("{base}|{}|{epoch}", files.join(","))
            }
            DiffSource::Git { .. } => epoch,
        }
    }

    fn maybe_reload(&mut self, cx: &mut Context<Self>) {
        let sig = self.signature(cx);
        if self.seen.as_ref() != Some(&sig) {
            self.seen = Some(sig);
            self.reload(cx);
        }
    }

    /// Read the files again (off the main thread); a newer read replaces one under way.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        let job: Box<dyn FnOnce() -> Result<Vec<DiffFile>, String> + Send> = match &self.source {
            DiffSource::Review { thread } => {
                let ws = self.workspace.read(cx);
                let files: Vec<String> = ws
                    .pending_files(thread)
                    .iter()
                    .map(|f| f.path.clone())
                    .collect();
                match ws.review_base(thread) {
                    Some((top, base)) if !files.is_empty() => {
                        Box::new(move || load_review(&top, &base, &files))
                    }
                    _ => Box::new(|| Ok(vec![])),
                }
            }
            DiffSource::Git { top, rel, staged } => {
                let (top, rel, staged) = (top.clone(), rel.clone(), *staged);
                Box::new(move || load_git(&top, &rel, staged))
            }
        };
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job() }).await;
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(files) => {
                        this.files = files;
                        this.error = None;
                        this.drop_stale_viewed(cx);
                    }
                    Err(e) => this.error = Some(e),
                }
                this.rebuild(cx);
                if this.review.cursor.is_none() {
                    this.review.cursor = diff_review::first_stop(&this.nav);
                }
            });
        }));
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let moved = self.compute_rows(cx);
        if !moved.is_empty() {
            if let Some(key) = self.notes_key(cx) {
                self.workspace
                    .update(cx, |ws, cx| ws.move_comments(&key, &moved, cx));
            }
        }
        self.review.seen_epoch = self.workspace.read(cx).review_notes_epoch;
        self.take_reveal();
        cx.notify();
    }

    /// The rows as things stand (comments' cards among them, files marked viewed folded); what
    /// moved under its comments.
    fn compute_rows(&mut self, cx: &App) -> Vec<(u64, u32, u32)> {
        let was = self.review.remember(&self.nav);
        let drafts = self.notes_key(cx).map_or_else(Vec::new, |key| {
            let ws = self.workspace.read(cx);
            ws.review_notes
                .get(&key)
                .map(|n| n.drafts.clone())
                .unwrap_or_default()
        });
        let mut collapsed = self.collapsed.clone();
        collapsed.extend(
            self.files
                .iter()
                .filter(|f| self.file_viewed(&f.rel, cx) && !self.viewed_open.contains(&f.rel))
                .map(|f| f.rel.clone()),
        );
        let rows = build_rows(&self.files, &collapsed, &self.opened, self.split);
        let nav = nav_of(&rows, &self.files);
        let width = self.card_width();
        let (placed, moved, editor_placed) = {
            let files = &self.files;
            let path_of = |f: usize| files.get(f).map(|f| f.rel.clone());
            let text_of = |ix: usize| row_text(&rows[ix]);
            diff_review::place_cards(
                &nav,
                &path_of,
                &text_of,
                &drafts,
                self.review.editing.as_ref(),
                ROW,
                width,
                cx,
            )
        };
        self.cards = diff_review::card_ends(&placed);
        let (rows, nav) = diff_review::merge(rows, nav, &placed, |p, part| Row::Card {
            card: p.card,
            part,
            rows: p.rows,
        });
        self.rows = std::rc::Rc::new(rows);
        self.nav = nav;
        self.review.editing_placed = editor_placed;
        self.review.restore(was, &self.nav);
        moved
    }

    /// How wide the list is, as last drawn.
    fn list_width(&self) -> f32 {
        self.scroll
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width
            .as_f32()
    }

    /// Where a card's left edge is: over the code column.
    const CARD_LEFT: f32 = 100.;

    fn card_width(&self) -> f32 {
        diff_review::card_width(self.list_width().max(480.), Self::CARD_LEFT)
    }

    /// Scroll to the card waiting to be shown, if it's in the rows now.
    fn take_reveal(&mut self) {
        let Some(id) = self.review.reveal else {
            return;
        };
        let end = self
            .cards
            .iter()
            .find(|(card, _)| *card == Card::Draft(id))
            .map(|(_, end)| *end);
        if let Some(end) = end {
            self.scroll.scroll_to_item(end, ScrollStrategy::Nearest);
            self.review.reveal = None;
        }
    }

    /// What a file's viewed mark goes by: the index's diff isn't the working tree's.
    fn viewed_name(&self, rel: &str) -> String {
        match &self.source {
            DiffSource::Git { staged: true, .. } => format!("index:{rel}"),
            _ => rel.to_string(),
        }
    }

    fn file_viewed(&self, rel: &str, cx: &App) -> bool {
        self.notes_key(cx).is_some_and(|key| {
            self.workspace
                .read(cx)
                .is_viewed(&key, &self.viewed_name(rel))
        })
    }

    /// A file marked viewed whose diff has changed since is back to being reviewed.
    fn drop_stale_viewed(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.notes_key(cx) else {
            return;
        };
        let stale: Vec<String> = {
            let ws = self.workspace.read(cx);
            self.files
                .iter()
                .filter_map(|f| {
                    let name = self.viewed_name(&f.rel);
                    let sig = ws.viewed_sig(&key, &name)?;
                    (sig != file_sig(f) && sig != diff_review::SIG_UNKNOWN).then_some(name)
                })
                .collect()
        };
        if !stale.is_empty() {
            self.workspace.update(cx, |ws, cx| {
                for name in &stale {
                    ws.set_viewed(&key, name, None, cx);
                }
            });
        }
    }

    /// Source Control's list this diff is from, by path: what `]`, `[` and the count go through.
    fn scm_list(&self, cx: &App) -> Vec<crate::ide::scm::ScmFile> {
        let DiffSource::Git { staged, .. } = &self.source else {
            return vec![];
        };
        let Some(scm) = self
            .workbench
            .upgrade()
            .and_then(|wb| wb.read(cx).scm.clone())
        else {
            return vec![];
        };
        let state = &scm.read(cx).state;
        if *staged {
            state.staged.clone()
        } else {
            state.changes.clone()
        }
    }

    /// Of this diff's files (a Source Control diff: its list's), how many are marked viewed, and
    /// how many there are.
    fn viewed_counts(&self, cx: &App) -> (usize, usize) {
        let list = self.scm_list(cx);
        if list.is_empty() {
            let viewed = self
                .files
                .iter()
                .filter(|f| self.file_viewed(&f.rel, cx))
                .count();
            return (viewed, self.files.len());
        }
        let viewed = list.iter().filter(|f| self.file_viewed(&f.rel, cx)).count();
        (viewed, list.len())
    }

    /// Open Source Control's file `to` in this tab's place, or none if there's no such file.
    fn open_scm_file(
        &mut self,
        to: Option<crate::ide::scm::ScmFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(file), DiffSource::Git { staged, .. }) = (to, &self.source) else {
            return;
        };
        let staged = *staged;
        let Some(scm) = self
            .workbench
            .upgrade()
            .and_then(|wb| wb.read(cx).scm.clone())
        else {
            return;
        };
        scm.update(cx, |s, cx| s.open_changes(&file, staged, true, window, cx));
    }

    /// The hunk at `file`, `hunk` into the index, or back out of it. Git's state moved: everything
    /// that shows it reads it again.
    fn stage_hunk_at(
        &mut self,
        file: usize,
        hunk: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let DiffSource::Git { top, staged, .. } = &self.source else {
            return;
        };
        let (top, reverse) = (top.clone(), *staged);
        let Some(patch) = self.files.get(file).and_then(|f| f.diff.patch(hunk)) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let t = top.clone();
            let result = cx
                .background_executor()
                .spawn(async move { apply_to_index(&t, &patch, reverse) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    let verb = if reverse { "Unstage" } else { "Stage" };
                    this.workspace.update(cx, |_, cx| {
                        cx.emit(WorkspaceEvent::Toast {
                            message: format!("{verb} failed: {e}"),
                            undo: None,
                        })
                    });
                    return;
                }
                this.reload(cx);
                let (wb, ws) = (this.workbench.clone(), this.workspace.clone());
                window.defer(cx, move |window, cx| {
                    let scm = wb.upgrade().and_then(|wb| wb.read(cx).scm.clone());
                    match scm {
                        Some(scm) => scm.update(cx, |s, cx| s.git_moved(window, cx)),
                        None => {
                            ws.update(cx, |ws, cx| ws.refresh_git_at(top, cx));
                        }
                    }
                });
            });
        })
        .detach();
    }

    fn toggle_file(&mut self, rel: String, cx: &mut Context<Self>) {
        if self.file_viewed(&rel, cx) {
            if !self.viewed_open.remove(&rel) {
                self.viewed_open.insert(rel);
            }
        } else if !self.collapsed.remove(&rel) {
            self.collapsed.insert(rel);
        }
        self.rebuild(cx);
    }

    /// File `f`'s viewed mark on, or off. A file that's marked folds up, and a file that's opened
    /// later stays open until it's marked again.
    fn toggle_viewed_file(&mut self, f: usize, cx: &mut Context<Self>) {
        let (Some(file), Some(key)) = (self.files.get(f), self.notes_key(cx)) else {
            return;
        };
        let (rel, name, sig) = (
            file.rel.clone(),
            self.viewed_name(&file.rel),
            file_sig(file),
        );
        let on = !self.file_viewed(&rel, cx);
        self.viewed_open.remove(&rel);
        self.workspace.update(cx, |ws, cx| {
            ws.set_viewed(&key, &name, on.then_some(sig), cx)
        });
        self.rebuild(cx);
    }

    fn open_fold(&mut self, file: usize, at: u32, cx: &mut Context<Self>) {
        if let Some(f) = self.files.get(file) {
            self.opened.insert((f.rel.clone(), at));
            self.rebuild(cx);
        }
    }

    pub fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        if self.split != split {
            self.split = split;
            self.rebuild(cx);
        }
    }

    /// The previous or next hunk's head, at the top.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let heads: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Hunk { .. }))
            .map(|(i, _)| i)
            .collect();
        if heads.is_empty() {
            return;
        }
        // The row at the top: every row is `ROW` tall (a jump still on its way counts as made).
        let top = {
            let state = self.scroll.0.borrow();
            state
                .deferred_scroll_to_item
                .as_ref()
                .map(|d| d.item_index)
                .unwrap_or_else(|| (-state.base_handle.offset().y.as_f32() / ROW).max(0.) as usize)
        };
        let next = if by > 0 {
            heads.iter().find(|h| **h > top).or(heads.last())
        } else {
            heads.iter().rev().find(|h| **h < top).or(heads.first())
        };
        if let Some(ix) = next {
            self.scroll.scroll_to_item(*ix, ScrollStrategy::Top);
            self.review.anchor = None;
            self.review.cursor = Some(diff_review::landing(&self.nav, *ix));
            cx.notify();
        }
    }

    /// The review's thread, while this is a Review whose file `rel` is still pending.
    fn pending(&self, rel: &str, cx: &App) -> Option<String> {
        let DiffSource::Review { thread } = &self.source else {
            return None;
        };
        self.workspace
            .read(cx)
            .pending_files(thread)
            .iter()
            .any(|f| f.path == rel)
            .then(|| thread.clone())
    }

    /// The file's editor tab has edits not saved: an undo (to the file on disk) would miss them.
    fn dirty_editor(&self, rel: &str, cx: &App) -> bool {
        let Some((top, _)) = (match &self.source {
            DiffSource::Review { thread } => self.workspace.read(cx).review_base(thread),
            DiffSource::Git { top, .. } => Some((top.clone(), String::new())),
        }) else {
            return false;
        };
        let path = top.join(rel);
        self.workbench.upgrade().is_some_and(|wb| {
            wb.read(cx)
                .tabs
                .iter()
                .filter_map(|t| t.editor())
                .any(|e| e.read(cx).path == path && e.read(cx).dirty)
        })
    }

    fn refuse_dirty(&self, rel: &str, cx: &mut Context<Self>) -> bool {
        if !self.dirty_editor(rel, cx) {
            return false;
        }
        let message = format!(
            "Save or drop your edits to {} first: the undo works on the file as saved.",
            rel.rsplit('/').next().unwrap_or(rel)
        );
        self.workspace.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        });
        true
    }

    /// Keep or undo hunk `hunk` of file `file` (a Review's).
    pub fn act_on_hunk(&mut self, file: usize, hunk: usize, keep: bool, cx: &mut Context<Self>) {
        let Some(f) = self.files.get(file) else {
            return;
        };
        let (rel, patch) = (f.rel.clone(), f.diff.patch(hunk));
        let (Some(thread), Some(patch)) = (self.pending(&rel, cx), patch) else {
            return;
        };
        if !keep && self.refuse_dirty(&rel, cx) {
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            if keep {
                ws.keep_hunk(&thread, &rel, patch, cx)
            } else {
                ws.undo_hunk(&thread, &rel, patch, cx)
            }
        });
    }

    /// Keep or undo file `file` whole (a Review's).
    pub fn act_on_file(&mut self, file: usize, keep: bool, cx: &mut Context<Self>) {
        let Some(rel) = self.files.get(file).map(|f| f.rel.clone()) else {
            return;
        };
        let Some(thread) = self.pending(&rel, cx) else {
            return;
        };
        if !keep && self.refuse_dirty(&rel, cx) {
            return;
        }
        self.workspace.update(cx, |ws, cx| {
            if keep {
                ws.keep_files(&thread, Some(vec![rel]), cx)
            } else {
                ws.undo_files(&thread, Some(vec![rel]), cx)
            }
        });
    }

    /// Open file `file` in the editor, on its first change.
    fn open_file(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(f) = self.files.get(file) else {
            return;
        };
        let top = match &self.source {
            DiffSource::Review { thread } => {
                self.workspace.read(cx).review_base(thread).map(|(t, _)| t)
            }
            DiffSource::Git { top, .. } => Some(top.clone()),
        };
        let Some(top) = top else { return };
        let line = f.diff.hunks.first().map(|h| h.new_start.max(1));
        let path = top.join(&f.rel);
        if f.status == 'D' {
            return;
        }
        self.workspace.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::OpenEditor {
                path,
                line,
                preview: false,
            })
        });
    }

    /// The whole diff as one patch, in a plain text tab (to copy or save).
    fn open_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text: String = self
            .files
            .iter()
            .map(|f| {
                format!(
                    "{}{}",
                    f.diff.header,
                    f.diff
                        .hunks
                        .iter()
                        .map(|h| h.body.as_str())
                        .collect::<String>()
                )
            })
            .collect();
        let title = format!("{} · Patch", self.title(cx));
        if let Some(wb) = self.workbench.upgrade() {
            window.defer(cx, move |window, cx| {
                wb.update(cx, |wb, cx| wb.open_diff(title, text, window, cx))
            });
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (added, removed) = self
            .files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let n = self.files.len();
        let (viewed, listed) = self.viewed_counts(cx);
        let review = match &self.source {
            DiffSource::Review { thread } => {
                Some((thread.clone(), self.workspace.read(cx).turn_running(thread)))
            }
            DiffSource::Git { .. } => None,
        };
        let ember = palette::ember(cx);
        let link = |id: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .flex_none()
                .h(px(22.))
                .px(px(7.))
                .gap(px(4.))
                .items_center()
                .rounded(px(fluent::control(radius::CONTROL)))
                .cursor_pointer()
        };
        let split = self.split;
        h_flex()
            .id("diff-toolbar")
            .h(px(32.))
            .flex_none()
            .px(px(12.))
            .gap(px(6.))
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .text_size(px(text::LABEL))
            .text_color(theme.muted_foreground)
            .child(Icon::new(crate::assets::Lucide::FileDiff).size(px(13.)))
            .child(
                div()
                    .text_color(theme.foreground.opacity(0.85))
                    .child(format!("{n} file{}", if n == 1 { "" } else { "s" })),
            )
            .when(added > 0 || removed > 0, |el| {
                el.child(crate::line_breakdown::hover(
                    "diff-lines",
                    h_flex()
                        .gap(px(6.))
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(text::LABEL))
                        .when(added > 0, |el| {
                            el.child(
                                div()
                                    .text_color(palette::emerald(cx))
                                    .child(format!("+{added}")),
                            )
                        })
                        .when(removed > 0, |el| {
                            el.child(
                                div()
                                    .text_color(palette::red(cx))
                                    .child(format!("−{removed}")),
                            )
                        }),
                    trek_core::file_kind::LineBreakdown::of(
                        self.files
                            .iter()
                            .map(|f| (f.rel.as_str(), f.added, f.removed)),
                    ),
                ))
            })
            .when(listed > 0, |el| {
                el.child(
                    div()
                        .id("diff-viewed")
                        .test_support()
                        .child(diff_review::viewed_label(viewed, listed)),
                )
            })
            .when(self.loading, |el| {
                el.child(Spinner::new().xsmall().color(theme.muted_foreground))
            })
            .child(div().flex_1())
            .child(
                crate::ui::icon_button("diff-prev", IconName::ArrowUp, "Previous change")
                    .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
            )
            .child(
                crate::ui::icon_button("diff-next", IconName::ArrowDown, "Next change")
                    .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
            )
            .child(
                crate::ui::icon_button(
                    "diff-split",
                    if split {
                        crate::assets::Lucide::Rows2
                    } else {
                        crate::assets::Lucide::Columns2
                    },
                    if split { "Inline" } else { "Side by side" },
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_split(!split, cx))),
            )
            .child(
                crate::ui::icon_button("diff-patch", IconName::FileText, "Open as patch")
                    .on_click(cx.listener(|this, _, window, cx| this.open_patch(window, cx))),
            )
            .when_some(review.filter(|_| n > 0), |el, (thread, busy)| {
                let (ws1, ws2, t1, t2) = (
                    self.workspace.clone(),
                    self.workspace.clone(),
                    thread.clone(),
                    thread,
                );
                el.child(div().w(px(1.)).h(px(14.)).mx(px(2.)).bg(theme.border))
                    .child(
                        link("diff-undo-all")
                            .hover(|s| {
                                s.bg(theme.foreground.opacity(0.07))
                                    .text_color(theme.foreground)
                            })
                            .when(busy, |el| el.opacity(0.45).cursor_default())
                            .child("Undo all")
                            .on_click(move |_, _, cx| {
                                ws1.update(cx, |ws, cx| ws.undo_files(&t1, None, cx))
                            }),
                    )
                    .child(
                        link("diff-keep-all")
                            .bg(ember)
                            .text_color(cx.theme().primary_foreground)
                            .hover(|s| s.opacity(0.9))
                            .child("Keep all")
                            .on_click(move |_, _, cx| {
                                ws2.update(cx, |ws, cx| ws.keep_files(&t2, None, cx))
                            }),
                    )
            })
    }

    /// Rows `range`, drawn.
    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let focused = self.focus.is_focused(window);
        let first_shown = range.start;
        let card_w = self.card_width();
        let git_diff = match &self.source {
            DiffSource::Git { staged, .. } => Some(*staged),
            DiffSource::Review { .. } => None,
        };
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let (green, red, ember) = (palette::emerald(cx), palette::red(cx), palette::ember(cx));
        let rows = self.rows.clone();
        let busy = match &self.source {
            DiffSource::Review { thread } => self.workspace.read(cx).turn_running(thread),
            DiffSource::Git { .. } => false,
        };
        let number = |n: Option<u32>| {
            div()
                .w(px(42.))
                .flex_none()
                .pr(px(8.))
                .text_right()
                .text_color(muted.opacity(0.7))
                .child(n.map(|n| n.to_string()).unwrap_or_default())
        };
        let tint = |kind: Kind| match kind {
            Kind::Add => Some(green),
            Kind::Del => Some(red),
            Kind::Ctx => None,
        };
        let small = |id: ElementId, label: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .flex_none()
                .h(px(18.))
                .px(px(6.))
                .items_center()
                .rounded(px(radius::TAG))
                .cursor_pointer()
                .font_family(theme.font_family.clone())
                .text_size(px(text::META))
                .text_color(muted)
                .hover(|s| {
                    s.bg(theme.foreground.opacity(0.08))
                        .text_color(theme.foreground)
                })
                .child(label)
        };
        range
            .filter_map(|ix| rows.get(ix).map(|r| (ix, r.clone())))
            .map(|(ix, row)| {
                let base = h_flex()
                    .id(("diff-row", ix))
                    .w_full()
                    .h(px(ROW))
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(mono.clone())
                    .text_size(px(text::LABEL));
                let marks = self.review.marks(&self.nav, ix, focused);
                match row {
                    Row::Card { card, part, rows } => diff_review::card_row(
                        self,
                        ix,
                        first_shown,
                        card,
                        part,
                        rows,
                        ROW,
                        Self::CARD_LEFT,
                        card_w,
                        cx,
                    ),
                    Row::Space => base.into_any_element(),
                    Row::Note(text) => base
                        .pl(px(108.))
                        .text_color(muted)
                        .italic()
                        .font_family(theme.font_family.clone())
                        .child(text)
                        .into_any_element(),
                    Row::File(f) => {
                        let Some(file) = self.files.get(f) else {
                            return base.into_any_element();
                        };
                        let (dir, name) = match file.rel.rsplit_once('/') {
                            Some((d, n)) => (format!("{d}/"), n.to_string()),
                            None => (String::new(), file.rel.clone()),
                        };
                        let color = match file.status {
                            'A' => green,
                            'D' => red,
                            _ => palette::amber(cx),
                        };
                        let viewed = self.file_viewed(&file.rel, cx);
                        let open = if viewed {
                            self.viewed_open.contains(&file.rel)
                        } else {
                            !self.collapsed.contains(&file.rel)
                        };
                        let pending = self.pending(&file.rel, cx).is_some();
                        let rel = file.rel.clone();
                        diff_review::head_chrome(base, marks, cx)
                            .id(("diff-file", f))
                            .test_support()
                            .px(px(8.))
                            .gap(px(6.))
                            .bg(theme.foreground.opacity(0.045))
                            .border_t_1()
                            .border_b_1()
                            .border_color(theme.border.opacity(0.7))
                            .font_family(theme.font_family.clone())
                            .cursor_pointer()
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(muted),
                            )
                            .child(crate::file_icon::badge(&name, px(13.), cx))
                            .child(
                                div()
                                    .text_color(if viewed { muted } else { theme.foreground })
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .when(!dir.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(text::LABEL))
                                        .text_color(muted)
                                        .child(dir),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(text::META))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color)
                                    .child(file.status.to_string()),
                            )
                            .when(file.added > 0, |el| {
                                el.child(
                                    div()
                                        .font_family(mono.clone())
                                        .text_size(px(text::LABEL))
                                        .text_color(green)
                                        .child(format!("+{}", file.added)),
                                )
                            })
                            .when(file.removed > 0, |el| {
                                el.child(
                                    div()
                                        .font_family(mono.clone())
                                        .text_size(px(text::LABEL))
                                        .text_color(red)
                                        .child(format!("−{}", file.removed)),
                                )
                            })
                            .child(div().w(px(12.)))
                            .when(viewed, |el| {
                                el.child(
                                    Icon::new(crate::assets::Lucide::Check)
                                        .size(px(12.))
                                        .text_color(green),
                                )
                            })
                            .child(
                                small(
                                    ("diff-viewed-file", f).into(),
                                    if viewed { "Viewed" } else { "Mark viewed" },
                                )
                                .when(viewed, |el| el.ml(px(-4.)))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_viewed_file(f, cx);
                                    },
                                )),
                            )
                            .when(file.status != 'D', |el| {
                                el.child(small(("diff-open-file", f).into(), "Open File").on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_file(f, cx);
                                    }),
                                ))
                            })
                            .when(pending, |el| {
                                el.child(
                                    small(("diff-undo-file", f).into(), "Undo file")
                                        .when(busy, |el| el.opacity(0.45))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.act_on_file(f, false, cx);
                                        })),
                                )
                                .child(
                                    small(("diff-keep-file", f).into(), "Keep file")
                                        .text_color(ember)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.act_on_file(f, true, cx);
                                        })),
                                )
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_file(rel.clone(), cx)
                                }),
                            )
                            .into_any_element()
                    }
                    Row::Hunk { file, hunk, head } => {
                        let pending = self
                            .files
                            .get(file)
                            .and_then(|f| self.pending(&f.rel, cx))
                            .is_some();
                        let n = self
                            .files
                            .get(file)
                            .map_or(0, |f| f.diff.hunks.iter().take(hunk).count());
                        let key = file * 10_000 + n;
                        diff_review::head_chrome(base, marks, cx)
                            .id(("diff-hunk", key))
                            .bg(theme.info.opacity(0.07))
                            .text_color(muted)
                            .gap(px(4.))
                            .pl(px(92.))
                            .child(head)
                            .when_some(git_diff, |el, staged| {
                                el.child(div().w(px(10.))).child(
                                    small(
                                        ("diff-stage-hunk", key).into(),
                                        if staged { "Unstage" } else { "Stage" },
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.stage_hunk_at(file, hunk, window, cx)
                                        },
                                    )),
                                )
                            })
                            .when(pending, |el| {
                                el.child(div().w(px(10.)))
                                    .child(
                                        small(("diff-undo-hunk", key).into(), "Undo")
                                            .when(busy, |el| el.opacity(0.45))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.act_on_hunk(file, hunk, false, cx)
                                            })),
                                    )
                                    .child(
                                        small(("diff-keep-hunk", key).into(), "Keep")
                                            .text_color(ember)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.act_on_hunk(file, hunk, true, cx)
                                            })),
                                    )
                            })
                            .into_any_element()
                    }
                    Row::Line {
                        kind,
                        old,
                        new,
                        text,
                    } => {
                        let color = tint(kind);
                        let el = base
                            .when_some(color, |el, c| el.bg(c.opacity(0.11)))
                            .child(
                                number(old)
                                    .when_some(color.filter(|_| kind == Kind::Del), |el, c| {
                                        el.bg(c.opacity(0.10))
                                    }),
                            )
                            .child(
                                number(new)
                                    .when_some(color.filter(|_| kind == Kind::Add), |el, c| {
                                        el.bg(c.opacity(0.10))
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_none()
                                    .text_color(color.unwrap_or(muted))
                                    .child(match kind {
                                        Kind::Add => "+",
                                        Kind::Del => "−",
                                        Kind::Ctx => "",
                                    }),
                            )
                            .child(
                                div()
                                    .pr(px(16.))
                                    .text_color(theme.foreground.opacity(if kind == Kind::Ctx {
                                        0.8
                                    } else {
                                        0.95
                                    }))
                                    .child(text),
                            );
                        diff_review::line_chrome(el, ix, marks, 2., ROW, cx)
                            .test_support()
                            .into_any_element()
                    }
                    Row::Pair { left, right } => {
                        let half = |side: Option<Side>, del: bool| {
                            let color = side.as_ref().and_then(|(_, k, _)| tint(*k));
                            h_flex()
                                .w(relative(0.5))
                                .h_full()
                                .min_w_0()
                                .overflow_hidden()
                                .when(side.is_none(), |el| el.bg(theme.foreground.opacity(0.03)))
                                .when_some(color, |el, c| el.bg(c.opacity(0.11)))
                                .when(del, |el| {
                                    el.border_r_1().border_color(theme.border.opacity(0.6))
                                })
                                .children(side.map(|(n, k, text)| {
                                    h_flex()
                                        .child(number(Some(n)))
                                        .child(
                                            div()
                                                .w(px(14.))
                                                .flex_none()
                                                .text_color(color.unwrap_or(muted))
                                                .child(match k {
                                                    Kind::Add => "+",
                                                    Kind::Del => "−",
                                                    Kind::Ctx => "",
                                                }),
                                        )
                                        .child(
                                            div()
                                                .text_color(theme.foreground.opacity(
                                                    if k == Kind::Ctx { 0.8 } else { 0.95 },
                                                ))
                                                .child(text),
                                        )
                                }))
                        };
                        let el = base.child(half(left, true)).child(half(right, false));
                        diff_review::line_chrome(el, ix, marks, 2., ROW, cx)
                            .test_support()
                            .into_any_element()
                    }
                    Row::Fold { file, at, lines } => base
                        .id(("diff-fold", ix))
                        .test_support()
                        .pl(px(92.))
                        .gap(px(6.))
                        .bg(theme.foreground.opacity(0.025))
                        .cursor_pointer()
                        .font_family(theme.font_family.clone())
                        .text_size(px(text::LABEL))
                        .text_color(muted)
                        .hover(|s| {
                            s.text_color(theme.foreground)
                                .bg(theme.foreground.opacity(0.05))
                        })
                        .child(Icon::new(crate::assets::Lucide::ChevronsUpDown).size(px(12.)))
                        .child(format!(
                            "{lines} unchanged line{}",
                            if lines == 1 { "" } else { "s" }
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_fold(file, at, cx)))
                        .into_any_element(),
                }
            })
            .collect()
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ReviewHost for DiffView {
    fn review(&self) -> &DiffReview {
        &self.review
    }

    fn review_mut(&mut self) -> &mut DiffReview {
        &mut self.review
    }

    fn workspace_entity(&self) -> Entity<Workspace> {
        self.workspace.clone()
    }

    fn nav(&self) -> &[Nav] {
        &self.nav
    }

    fn list_scroll(&self) -> &UniformListScrollHandle {
        &self.scroll
    }

    fn diff_focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// A Review's are its chat's. Source Control's aren't any one chat's: they stay with the folder
    /// while the side bar changes chats, and go to whichever is in front.
    fn notes_key(&self, _: &App) -> Option<String> {
        Some(match &self.source {
            DiffSource::Review { thread } => thread.clone(),
            DiffSource::Git { top, .. } => diff_review::folder_key(top),
        })
    }

    fn path_of(&self, file: usize) -> Option<String> {
        self.files.get(file).map(|f| f.rel.clone())
    }

    fn text_of(&self, ix: usize) -> Vec<String> {
        self.rows.get(ix).map(row_text).unwrap_or_default()
    }

    fn placements(&self) -> &[(Card, usize)] {
        &self.cards
    }

    fn rows_changed(&mut self, cx: &mut Context<Self>) {
        self.rebuild(cx);
    }

    /// A Review's files are all here; Source Control's come one to a tab.
    fn go_to_file(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let DiffSource::Git { rel, .. } = &self.source {
            let list = self.scm_list(cx);
            let at = list.iter().position(|f| &f.rel == rel);
            let next = match at {
                Some(at) if by > 0 => list.get(at + 1).cloned(),
                Some(at) => at.checked_sub(1).and_then(|at| list.get(at).cloned()),
                None => list.first().cloned(),
            };
            return self.open_scm_file(next, window, cx);
        }
        if let Some(head) = diff_review::step_file(&self.nav, self.review.cursor, by) {
            diff_review::go_to_head(self, head, cx);
        }
    }

    fn ran_out_of_hunks(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.source, DiffSource::Git { .. }) {
            self.go_to_file(by, window, cx);
        }
    }

    /// The file the cursor's in. Marked by key, the next file not viewed comes up.
    fn toggle_viewed(&mut self, advance: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self
            .review
            .cursor
            .and_then(|c| self.nav.get(c)?.file())
            .or_else(|| (!self.files.is_empty()).then_some(0))
        else {
            return;
        };
        self.toggle_viewed_file(f, cx);
        let rel = self.files.get(f).map(|file| file.rel.clone());
        if !advance || !rel.is_some_and(|rel| self.file_viewed(&rel, cx)) {
            return;
        }
        if let DiffSource::Git { rel, .. } = &self.source {
            let list = self.scm_list(cx);
            let at = list.iter().position(|f| &f.rel == rel).unwrap_or(0);
            let next = list[at..]
                .iter()
                .chain(list[..at].iter())
                .find(|f| !self.file_viewed(&f.rel, cx))
                .cloned();
            return self.open_scm_file(next, window, cx);
        }
        let next = (0..self.files.len())
            .map(|i| (f + i) % self.files.len())
            .find(|i| !self.file_viewed(&self.files[*i].rel, cx));
        let head = next.and_then(|file| self.nav.iter().position(|n| *n == Nav::File { file }));
        if let Some(head) = head {
            diff_review::go_to_head(self, head, cx);
        }
    }

    fn stage_hunk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((file, hunk)) = self.review.cursor.and_then(|c| self.nav.get(c)?.hunk()) {
            self.stage_hunk_at(file, hunk, window, cx);
        }
    }

    /// To a Review's chat; Source Control's to the chat in front of the AI side bar (a new one if
    /// it's on a new chat), which comes up to show it.
    fn deliver(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.source.clone() {
            DiffSource::Review { thread } => self.workspace.update(cx, |ws, cx| {
                ws.send_comments_in(&Scope::Thread(thread), text, cx)
            }),
            DiffSource::Git { .. } => {
                if !self
                    .workspace
                    .update(cx, |ws, cx| ws.send_comments_in(&Scope::Ide, text, cx))
                {
                    return false;
                }
                let wb = self.workbench.clone();
                window.defer(cx, move |window, cx| {
                    if let Some(wb) = wb.upgrade() {
                        wb.update(cx, |wb, cx| {
                            if !wb.layout.ai_open {
                                wb.toggle_ai(window, cx);
                            }
                        });
                    }
                });
                true
            }
        }
    }

    fn show_draft(&mut self, draft: &Draft, cx: &mut Context<Self>) {
        // Folded, it has no lines for its card to go under.
        self.collapsed.remove(&draft.path);
        if self.file_viewed(&draft.path, cx) {
            self.viewed_open.insert(draft.path.clone());
        }
        self.rebuild(cx);
    }
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Cards wrap to the width they're drawn at: measured again when that moves.
        let width = self.list_width();
        if (width - self.review.measured).abs() > 1. {
            self.review.measured = width;
            if !self.cards.is_empty() {
                self.compute_rows(cx);
            }
        }
        let body: AnyElement = if let Some(e) = &self.error {
            crate::panels::empty(format!("Couldn't read the changes: {e}"), cx).into_any_element()
        } else if self.files.is_empty() && !self.loading {
            let text = match &self.source {
                DiffSource::Review { .. } => {
                    "Nothing left to review: every change was kept or undone."
                }
                DiffSource::Git { .. } => "No changes.",
            };
            crate::panels::empty(text, cx).into_any_element()
        } else {
            // The longest row sets how far a sideways scroll goes (inline only: side by side
            // halves cut their lines).
            let widest = (!self.split)
                .then(|| {
                    self.rows
                        .iter()
                        .enumerate()
                        .map(|(i, r)| match r {
                            Row::Line { text, .. } | Row::Hunk { head: text, .. } => {
                                (i, text.len() + 20)
                            }
                            Row::File(_) => (i, 90),
                            _ => (i, 0),
                        })
                        .max_by_key(|(_, n)| *n)
                        .map(|(i, _)| i)
                })
                .flatten();
            let list = uniform_list(
                "diff-rows",
                self.rows.len(),
                cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)),
            )
            .size_full()
            .track_scroll(&self.scroll)
            .when(!self.split, |l| {
                l.with_width_from_item(widest)
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            });
            div()
                .id("diff-body")
                .size_full()
                .child(list)
                .vertical_scrollbar(&self.scroll)
                .when(!self.split, |el| el.horizontal_scrollbar(&self.scroll))
                .into_any_element()
        };
        let tray = diff_review::tray(self, cx);
        let focused = self.focus.is_focused(window);
        diff_review::keys(
            v_flex().id("diff-view").track_focus(&self.focus),
            focused,
            cx,
        )
        .test_support()
        .size_full()
        .bg(crate::ui::panel_bg(self.workspace.read(cx).glass(), cx))
        .text_color(theme.foreground)
        .child(self.toolbar(cx))
        .child(div().flex_1().min_h_0().child(body))
        .children(tray)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl DiffView {
    /// The rows as text: "file M a.rs +1 −1", "hunk @@ -1,2 +1,2 @@", "+ 2 new", "- 2 old",
    /// "  3 3 same", "fold 4 at 5", "pair 2 old | 2 new", "note …", "space".
    pub fn describe(&self) -> Vec<String> {
        let side = |s: &Option<Side>| {
            s.as_ref()
                .map(|(n, _, t)| format!("{n} {t}"))
                .unwrap_or_default()
        };
        self.rows
            .iter()
            .map(|r| match r {
                Row::Space => "space".to_string(),
                Row::File(f) => self
                    .files
                    .get(*f)
                    .map(|f| format!("file {} {} +{} −{}", f.status, f.rel, f.added, f.removed))
                    .unwrap_or_default(),
                Row::Hunk { head, .. } => format!("hunk {head}"),
                Row::Line {
                    kind: Kind::Add,
                    new,
                    text,
                    ..
                } => format!("+ {} {text}", new.unwrap_or(0)),
                Row::Line {
                    kind: Kind::Del,
                    old,
                    text,
                    ..
                } => format!("- {} {text}", old.unwrap_or(0)),
                Row::Line { old, new, text, .. } => {
                    format!("  {} {} {text}", old.unwrap_or(0), new.unwrap_or(0))
                }
                Row::Pair { left, right } => format!("pair {} | {}", side(left), side(right)),
                Row::Fold { at, lines, .. } => format!("fold {lines} at {at}"),
                Row::Note(t) => format!("note {t}"),
                Row::Card { .. } => "card".to_string(),
            })
            .collect()
    }

    /// The line the cursor's on, as `row_text` has it.
    pub fn cursor_line(&self) -> Option<String> {
        self.rows
            .get(self.review.cursor?)
            .and_then(|r| row_text(r).first().cloned())
    }

    pub fn viewed_count(&self, cx: &App) -> (usize, usize) {
        self.viewed_counts(cx)
    }

    pub fn loaded(&self) -> bool {
        !self.loading
    }

    pub fn set_split_for_test(&mut self, split: bool, cx: &mut Context<Self>) {
        self.set_split(split, cx);
    }

    pub fn open_fold_for_test(&mut self, at: u32, cx: &mut Context<Self>) {
        self.open_fold(0, at, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiffFile, FileDiff, HashSet, Nav, Row, apply_to_index, build_rows, file, file_sig, nav_of,
        row_text, untracked,
    };

    fn modified(rel: &str, patch: &str, new: &str) -> DiffFile {
        file(rel, FileDiff::parse(patch), Some(new.as_bytes().to_vec()))
    }

    fn text(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::File(_) => "file".into(),
                Row::Hunk { head, .. } => format!("hunk {head}"),
                Row::Line {
                    kind,
                    old,
                    new,
                    text,
                } => format!("{kind:?} {old:?} {new:?} {text}"),
                Row::Fold { at, lines, .. } => format!("fold {lines}@{at}"),
                Row::Pair { left, right } => format!(
                    "pair {:?} | {:?}",
                    left.as_ref().map(|s| s.0),
                    right.as_ref().map(|s| s.0)
                ),
                Row::Note(t) => format!("note {t}"),
                Row::Space => "space".into(),
                Row::Card { .. } => "card".into(),
            })
            .collect()
    }

    #[test]
    fn hunks_get_line_numbers_and_the_stretches_between_fold() {
        let new: String = (1..=20)
            .map(|n| {
                if n == 2 {
                    "two!\n".to_string()
                } else {
                    format!("{n}\n")
                }
            })
            .collect();
        let patch = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n 1\n-2\n+two!\n 3\n";
        let f = modified("f", patch, &new);
        assert_eq!((f.status, f.added, f.removed), ('M', 1, 1));
        let rows = build_rows(&[f.clone()], &HashSet::new(), &HashSet::new(), false);
        assert_eq!(
            text(&rows),
            vec![
                "file",
                "hunk @@ -1,3 +1,3 @@",
                "Ctx Some(1) Some(1) 1",
                "Del Some(2) None 2",
                "Add None Some(2) two!",
                "Ctx Some(3) Some(3) 3",
                "fold 17@4"
            ]
        );
        // Opened, the stretch shows with both sides' numbers.
        let opened: HashSet<(String, u32)> = [("f".to_string(), 4)].into();
        let rows = build_rows(&[f.clone()], &HashSet::new(), &opened, false);
        assert_eq!(rows.len(), 6 + 17);
        assert_eq!(text(&rows)[6], "Ctx Some(4) Some(4) 4");
        // Side by side, the removed and added lines share a row.
        let rows = build_rows(&[f], &HashSet::new(), &HashSet::new(), true);
        assert_eq!(text(&rows)[3], "pair Some(2) | Some(2)");
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn a_stretch_before_a_later_hunk_keeps_old_numbers_apart() {
        // Two lines were added near the top: below them old and new numbers differ by two.
        let new: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,4 @@\n 1\n+2\n+3\n 4\n@@ -20,3 +22,3 @@\n 22\n-x\n+23\n 24\n";
        let f = modified("f", patch, &new);
        let rows = build_rows(&[f.clone()], &HashSet::new(), &HashSet::new(), false);
        assert!(
            text(&rows).contains(&"fold 17@5".to_string()),
            "{:?}",
            text(&rows)
        );
        let opened: HashSet<(String, u32)> = [("f".to_string(), 5)].into();
        let rows = build_rows(&[f], &HashSet::new(), &opened, false);
        assert!(
            text(&rows).contains(&"Ctx Some(3) Some(5) 5".to_string()),
            "{:?}",
            text(&rows)
        );
    }

    #[test]
    fn rows_say_what_they_are_to_move_around_on() {
        let new: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n 1\n-x\n+2\n";
        let f = modified("f", patch, &new);
        let opened: HashSet<(String, u32)> = [("f".to_string(), 3)].into();
        let files = [f];
        let rows = build_rows(&files, &HashSet::new(), &opened, false);
        let nav = nav_of(&rows, &files);
        assert_eq!(nav[0], Nav::File { file: 0 });
        assert_eq!(nav[1], Nav::Hunk { file: 0, hunk: 0 });
        assert_eq!(
            nav[3],
            Nav::Line {
                file: 0,
                hunk: Some(0),
                old: Some(2),
                new: None,
                changed: true
            }
        );
        // Opened out of the fold after the hunk, a line isn't the hunk's.
        assert_eq!(
            nav[5],
            Nav::Line {
                file: 0,
                hunk: None,
                old: Some(3),
                new: Some(3),
                changed: false
            }
        );
        assert_eq!(row_text(&rows[3]), ["-x"]);
        assert_eq!(row_text(&rows[4]), ["+2"]);
        assert_eq!(row_text(&rows[2]), [" 1"]);

        // Side by side, a row is both lines (once, when they're the same).
        let rows = build_rows(&files, &HashSet::new(), &HashSet::new(), true);
        let nav = nav_of(&rows, &files);
        assert_eq!(row_text(&rows[2]), [" 1"]);
        assert_eq!(row_text(&rows[3]), ["-x", "+2"]);
        assert!(matches!(
            nav[3],
            Nav::Line {
                old: Some(2),
                new: Some(2),
                changed: true,
                ..
            }
        ));
    }

    #[test]
    fn what_a_file_says_changes_with_its_lines() {
        let a = modified(
            "f",
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n",
            "b\n",
        );
        let moved = modified(
            "f",
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -9 +9 @@\n-a\n+b\n",
            "b\n",
        );
        let other = modified(
            "f",
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+c\n",
            "c\n",
        );
        assert_eq!(file_sig(&a), file_sig(&moved));
        assert_ne!(file_sig(&a), file_sig(&other));
    }

    #[test]
    fn a_hunk_goes_into_the_index_and_back_out_on_its_own() {
        let dir = std::env::temp_dir().join(format!("trek-diff-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| crate::ide::git::run(&dir, args).expect("git");
        run(&["init", "-q"]);
        let before: String = (1..=30).map(|n| format!("{n}\n")).collect();
        std::fs::write(dir.join("f.txt"), &before).unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "one",
        ]);
        let after = before
            .replace("\n2\n", "\ntwo\n")
            .replace("\n28\n", "\ntwenty-eight\n");
        std::fs::write(dir.join("f.txt"), &after).unwrap();
        let diff = FileDiff::parse(&run(&["diff", "--no-color", "--", "f.txt"]));
        assert_eq!(diff.hunks.len(), 2);

        // The second hunk only: the first stays unstaged, line numbers and all.
        apply_to_index(&dir, &diff.patch(1).unwrap(), false).unwrap();
        let staged = run(&["diff", "--cached", "--no-color"]);
        assert!(
            staged.contains("+twenty-eight") && !staged.contains("+two"),
            "{staged}"
        );
        let left = run(&["diff", "--no-color"]);
        assert!(
            left.contains("+two") && !left.contains("+twenty-eight"),
            "{left}"
        );

        // Back out, by the staged diff's own hunk.
        let cached = FileDiff::parse(&staged);
        apply_to_index(&dir, &cached.patch(0).unwrap(), true).unwrap();
        assert_eq!(run(&["diff", "--cached", "--no-color"]), "");

        // A file git doesn't know yet stages by its one hunk.
        std::fs::write(dir.join("new.txt"), "a\nb\n").unwrap();
        let new = untracked("new.txt", Some(b"a\nb\n".to_vec()));
        apply_to_index(&dir, &new.diff.patch(0).unwrap(), false).unwrap();
        assert_eq!(run(&["show", ":new.txt"]), "a\nb\n");

        // A hunk that no longer fits says so.
        assert!(apply_to_index(&dir, &diff.patch(0).unwrap(), true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_files_show_every_line_and_binaries_say_so() {
        let f = untracked("n.txt", Some(b"a\nb\n".to_vec()));
        assert_eq!((f.status, f.added), ('A', 2));
        assert_eq!(
            text(&build_rows(&[f], &HashSet::new(), &HashSet::new(), false)),
            vec![
                "file",
                "hunk @@ -0,0 +1,2 @@",
                "Add None Some(1) a",
                "Add None Some(2) b"
            ]
        );
        let bin = untracked("x.bin", Some(vec![0, 1, 2]));
        assert_eq!(
            text(&build_rows(&[bin], &HashSet::new(), &HashSet::new(), false)),
            vec!["file", "note Binary file"]
        );
        // A collapsed file is its header alone.
        let f = untracked("n.txt", Some(b"a\n".to_vec()));
        assert_eq!(
            build_rows(&[f], &["n.txt".to_string()].into(), &HashSet::new(), false).len(),
            1
        );
    }
}
