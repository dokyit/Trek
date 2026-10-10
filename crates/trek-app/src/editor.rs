//! Editor: an in-app code surface. A project file opens here editable — syntax-highlighted,
//! line-numbered, searchable — and saves back with ⌘S, so a fix doesn't leave the app.
//! Deep links (`trek://edit?path=…&line=…`) land here from editor extensions.
//!
//! Its line fills show what changed against HEAD, or, while the AI side bar's chat has the file
//! pending review, against that review's baseline: then the changes are hunks, and a bar on the
//! hunk in view keeps or undoes it (⌘Y, ⌥⌘⌫: its toast takes the undo back), steps between
//! hunks (▲▼), and a bar at the bottom steps between the review's files and keeps or undoes the
//! whole file.
//!
//! ⌘K (in the editor) opens a prompt card over the picked lines (the caret's line, with none).
//! Its request goes to the AI side bar's chat as an ordinary message with those lines quoted and
//! a block asking for an edit of exactly them (`trek_core::inline_edit`); the agent edits the
//! file, and the edit comes back as a pending hunk like any other. A card over the text rather
//! than between lines: the editor has no block decorations (yet).

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One changed line range vs `HEAD`, for the editor's diff fills.
enum DiffMark {
    Added,
    Changed,
    /// Content deleted at this position — a marker block on the line after it.
    Deleted,
    /// A diff's `-` line (a Review tab): the whole line, red.
    Removed,
}

/// A unified diff's own lines as fills (a Review tab): `+` lines green, `-` lines red, hunk
/// heads faint.
fn patch_marks(text: &str) -> Vec<(u32, u32, DiffMark)> {
    let mut out: Vec<(u32, u32, DiffMark)> = vec![];
    for (row, line) in text.lines().enumerate() {
        let row = row as u32;
        let kind = if line.starts_with("+++") || line.starts_with("---") {
            continue;
        } else if line.starts_with('+') {
            DiffMark::Added
        } else if line.starts_with('-') {
            DiffMark::Removed
        } else if line.starts_with("@@") {
            DiffMark::Changed
        } else {
            continue;
        };
        // Runs of the same kind are one fill.
        match out.last_mut() {
            Some((_, end, k)) if *end == row && std::mem::discriminant(k) == std::mem::discriminant(&kind) => *end = row + 1,
            _ => out.push((row, row + 1, kind)),
        }
    }
    out
}

/// `git diff -U0` hunks for `path`, as 0-based row ranges: (start_row, end_row, kind).
/// Untracked files count as all-added. Not a repo (or clean): empty. Runs git twice, so it's
/// called off the main thread (`refresh_diff`).
fn diff_line_ranges(path: &Path) -> Vec<(u32, u32, DiffMark)> {
    let Some(dir) = path.parent() else { return vec![] };
    // Read-only: no optional locks (an agent's commit mustn't meet our index.lock), git found on
    // the login PATH, the user's diff settings kept out of what's parsed.
    let status = crate::ide::git::command(dir).args(["status", "--porcelain", "--", &path.display().to_string()]).output().ok();
    if let Some(out) = status.filter(|o| o.status.success()) {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.starts_with("??") {
            let n = std::fs::read_to_string(path).map(|t| t.lines().count() as u32).unwrap_or(0);
            return vec![(0, n, DiffMark::Added)];
        }
    }
    let Ok(out) = crate::ide::git::command(dir).args(["diff", "--no-color", "--no-ext-diff", "--unified=0", "--", &path.display().to_string()]).output() else {
        return vec![];
    };
    let mut marks = vec![];
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        // @@ -old_start[,old_count] +new_start[,new_count] @@
        let Some(rest) = line.strip_prefix("@@ -") else { continue };
        let Some((old, rest)) = rest.split_once(' ') else { continue };
        let Some(new) = rest.strip_prefix('+').and_then(|r| r.split_once(' ')).map(|(n, _)| n) else { continue };
        let num = |s: &str| s.split(',').next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let cnt = |s: &str| s.split(',').nth(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
        let (_os, oc) = (num(old), cnt(old));
        let (ns, nc) = (num(new), cnt(new));
        let row = ns.saturating_sub(1);
        if nc == 0 {
            marks.push((row, row + 1, DiffMark::Deleted));
        } else {
            let kind = if oc > 0 { DiffMark::Changed } else { DiffMark::Added };
            marks.push((row, row + nc, kind));
        }
    }
    marks
}

/// A review's hunks as line fills: added and changed rows, and a marker where lines went.
fn review_marks(diff: &FileDiff) -> Vec<(u32, u32, DiffMark)> {
    diff.marks()
        .into_iter()
        .map(|m| match m {
            Mark::Added { start, end } => (start, end, DiffMark::Added),
            Mark::Deleted { at, .. } => (at, at + 1, DiffMark::Deleted),
        })
        .collect()
}

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, TabSize};
use gpui_kit::base::input::{Point, RopeExt};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::keys::{self, Id};
use crate::palette;
use crate::workspace::{FileReview, Workspace, WorkspaceEvent};
use gpui_kit::component::input::{Input, InputState};
use trek_core::hunks::{FileDiff, Mark};

/// What a too-big or unreadable file gets instead of an editor: a note and a way out.
const MAX_FILE_BYTES: u64 = 2_000_000;

actions!(trek_editor, [SaveFile, InlineEdit, KeepHunk, UndoHunk, NextHunk, PreviousHunk]);

pub fn key_bindings() -> Vec<KeyBinding> {
    crate::keys::bindings(crate::keys::Group::Editor)
}

pub struct EditorView {
    workspace: Entity<Workspace>,
    pub(crate) path: PathBuf,
    state: Entity<EditorState>,
    /// Text as last loaded or saved — `value() != saved` means dirty.
    saved: String,
    pub(crate) dirty: bool,
    /// Why the file didn't load (too big, unreadable, gone). The view is read-only then.
    problem: Option<String>,
    /// The language server this doc is open on, when one covers its extension.
    lsp: Option<(Arc<crate::lsp_client::Client>, String)>,
    /// Keeps the diagnostics watcher alive.
    _diag_watch: Option<Task<()>>,
    /// Git-diff line fills (added/changed/deleted vs HEAD).
    diff_marks: Option<gpui_kit::base::input::RangeDecorationCollection>,
    /// The fills last painted, for tests: (first row, end row, kind).
    #[cfg(test)]
    painted: Vec<(u32, u32, &'static str)>,
    /// Reading the diff fills in the background; a newer read replaces it.
    _diff_read: Option<Task<()>>,
    /// The language server's program, for the status bar (`None`: no server for this file).
    pub(crate) lsp_name: Option<&'static str>,
    /// The file changed on disk under unsaved edits and the user was told: the next save
    /// overwrites it, as long as the file is still as it was then (a hash of what was on disk).
    overwrite_armed: Option<u64>,
    /// The file is being read (off the main thread): read-only until it's in.
    loading: bool,
    /// Where to put the caret once it's in (a deep link's line).
    pending_line: Option<u32>,
    /// Not a file on disk (a Review's diff): read-only, never saved or reloaded.
    pub(crate) scratch: bool,
    /// The review the fills are against, while the AI side bar's chat has this file pending.
    review: Option<FileReview>,
    /// The file's hunks against that review's baseline, as last read.
    hunks: Option<FileDiff>,
    /// The hunk the bar acts on, by index.
    hunk: usize,
    /// Put the caret on the first hunk once they're read (the file bar stepped here).
    pub(crate) jump_to_hunk: bool,
    /// ⌘K's prompt card, while it's open.
    inline: Option<InlineCard>,
    _sub: Subscription,
    _watch: Vec<Subscription>,
}

/// ⌘K's prompt over the picked lines.
struct InlineCard {
    input: Entity<InputState>,
    /// The lines (1-based, inclusive) and their text.
    lines: (u32, u32),
    text: String,
    _sub: Subscription,
}

impl EditorView {
    pub fn new(workspace: Entity<Workspace>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lang = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
        // Empty and read-only until the file is read, off the main thread: a big file, one
        // iCloud evicted, or a device or pipe someone named mustn't hold up the window.
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .language(lang.clone())
                .line_number(true)
                .searchable(true)
                // Code scrolls sideways: a wrap mid-identifier reads as two lines that aren't.
                .soft_wrap(false)
                .tab_size(TabSize { tab_size: 4, hard_tabs: false });
            s.set_readonly(true, cx);
            s
        });

        let sub = cx.subscribe(&state, |this, s, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && !this.loading {
                let text = s.read(cx).value().to_string();
                if let Some((client, uri)) = &this.lsp {
                    client.did_change(uri, &text);
                }
                let dirty = text.as_str() != this.saved.as_str();
                if dirty != this.dirty {
                    this.dirty = dirty;
                    cx.notify();
                }
            }
        });
        let watch = vec![
            // The bars and the card sit over the text: they follow it as it scrolls.
            cx.observe(&state, |this, _, cx| {
                if this.inline.is_some() || this.hunks.as_ref().is_some_and(|h| !h.hunks.is_empty()) {
                    cx.notify();
                }
            }),
            // The chat's review of this file moved (an agent edited it, a hunk was kept), or the
            // file started or stopped being pending in the chat in front: the fills follow.
            cx.subscribe(&workspace, |this, _, event: &WorkspaceEvent, cx| {
                if let WorkspaceEvent::ReviewChanged { id } = event {
                    if this.review.as_ref().is_some_and(|r| r.thread == *id) || this.workspace.read(cx).file_review(&this.path).is_some() {
                        this.refresh_diff(cx);
                    }
                }
            }),
            cx.observe(&workspace, |this, ws, cx| {
                if ws.read(cx).file_review(&this.path) != this.review {
                    this.refresh_diff(cx);
                }
                if ws.read(cx).reveal_hunk.as_ref() == Some(&this.path) {
                    this.jump_to_hunk = true;
                    ws.update(cx, |ws, _| ws.reveal_hunk = None);
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            workspace,
            path: path.clone(),
            state,
            saved: String::new(),
            dirty: false,
            problem: None,
            lsp: None,
            _diag_watch: None,
            diff_marks: None,
            #[cfg(test)]
            painted: vec![],
            _diff_read: None,
            lsp_name: None,
            overwrite_armed: None,
            loading: true,
            pending_line: None,
            scratch: false,
            review: None,
            hunks: None,
            hunk: 0,
            jump_to_hunk: false,
            inline: None,
            _sub: sub,
            _watch: watch,
        };
        // Opened from the file bar: on its first hunk.
        if this.workspace.read(cx).reveal_hunk.as_ref() == Some(&this.path) {
            this.jump_to_hunk = true;
            this.workspace.update(cx, |ws, _| ws.reveal_hunk = None);
        }
        cx.spawn_in(window, async move |this, cx| {
            let (text, problem) = cx.background_executor().spawn(async move { load(&path) }).await;
            let _ = this.update_in(cx, |this, window, cx| this.loaded(text, problem, lang, window, cx));
        })
        .detach();
        this
    }

    /// The file is read: it shows, editable unless `problem` says why not, and a language
    /// server that covers it opens it.
    fn loaded(&mut self, text: String, problem: Option<String>, lang: String, window: &mut Window, cx: &mut Context<Self>) {
        let readonly = problem.is_some();
        self.saved = text.clone();
        self.problem = problem;
        self.state.update(cx, |s, cx| {
            s.set_value(text.clone(), window, cx);
            s.set_readonly(readonly, cx);
        });
        self.loading = false;
        self.dirty = false;
        // A language server covers this file? Providers on the state, the doc opened, and a
        // watcher folding publishDiagnostics into the gutter.
        let path = self.path.clone();
        tracing::info!("lsp: {} lang={lang} problem={:?}", path.display(), self.problem);
        let workspace = self.workspace.clone();
        let state = self.state.clone();
        let lsp = self.problem.is_none().then(|| {
            crate::lsp_client::uri_for(&path).and_then(|uri| {
                workspace.update(cx, |ws, _| ws.lsp_for(&path, &lang)).map(|client| {
                    state.update(cx, |s, _| *s.lsp_mut() = client.lsp_for(uri.clone(), workspace.clone()));
                    client.did_open(&uri, &text);
                    (client, uri)
                })
            })
        }).flatten();
        self._diag_watch = lsp.as_ref().map(|(client, uri)| {
            let rx = client.watch_diagnostics();
            let uri = uri.clone();
            let state = state.clone();
            let (ws, file) = (workspace.clone(), path.clone());
            cx.spawn(async move |_, cx| {
                while let Ok((u, diags)) = rx.recv().await {
                    if u != uri {
                        continue;
                    }
                    cx.update_entity(&state, |s, cx| {
                        let rope = s.text().clone();
                        if let Some(set) = s.diagnostics_mut() {
                            set.reset(&rope);
                            set.extend(diags.iter().cloned().map(gpui_kit::base::input::Diagnostic::from));
                        }
                        cx.notify();
                    });
                    // Problems and the status bar count them too.
                    cx.update_entity(&ws, |ws, cx| ws.set_diagnostics(file.clone(), diags, cx));
                }
            })
        });
        self.lsp_name = lsp.as_ref().and_then(|_| crate::lsp_client::spec(&lang)).map(|(cmd, _, _)| cmd);
        self.lsp = lsp;
        if let Some(line) = self.pending_line.take() {
            self.goto_line(line, window, cx);
        }
        self.refresh_diff(cx);
        cx.notify();
    }

    /// The file is still being read.
    #[cfg(test)]
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// A read-only document that isn't a file (a Review's diff), its tab named `name` and its
    /// text highlighted as `language`.
    pub fn scratch(workspace: Entity<Workspace>, name: &str, text: &str, language: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx).language(language.to_string()).line_number(true).searchable(true).soft_wrap(false);
            s.set_value(text.to_string(), window, cx);
            s.set_readonly(true, cx);
            s
        });
        let sub = cx.subscribe(&state, |_, _, _: &InputEvent, _| {});
        let mut this = Self {
            workspace,
            path: PathBuf::from(name),
            state,
            saved: text.to_string(),
            dirty: false,
            problem: None,
            lsp: None,
            _diag_watch: None,
            diff_marks: None,
            #[cfg(test)]
            painted: vec![],
            _diff_read: None,
            lsp_name: None,
            overwrite_armed: None,
            loading: false,
            pending_line: None,
            scratch: true,
            review: None,
            hunks: None,
            hunk: 0,
            jump_to_hunk: false,
            inline: None,
            _sub: sub,
            _watch: vec![],
        };
        if language == "diff" {
            this.paint_diff(patch_marks(text), true, cx);
        }
        this
    }

    /// A scratch document's text, replaced (a Review opened again).
    pub(crate) fn set_scratch_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.saved = text.to_string();
        self.state.update(cx, |s, cx| {
            s.set_readonly(false, cx);
            s.set_value(text.to_string(), window, cx);
            s.set_readonly(true, cx);
        });
        self.paint_diff(patch_marks(text), true, cx);
        cx.notify();
    }

    /// The fills drawn now: (first row, end row, kind), as `added`, `changed`, `deleted`,
    /// `removed` — for tests.
    #[cfg(test)]
    pub fn fills(&self) -> Vec<(u32, u32, &'static str)> {
        self.painted.clone()
    }

    /// The selected lines (1-based, first and last) and their text; `None` with nothing
    /// selected. A selection ending at the start of a line doesn't take that line.
    pub fn selection(&self, cx: &App) -> Option<((u32, u32), String)> {
        let s = self.state.read(cx);
        let range = s.selected_range();
        if range.is_empty() {
            return None;
        }
        let text = s.text();
        let (from, to) = (text.offset_to_point(range.start), text.offset_to_point(range.end));
        let last = if to.column == 0 && to.row > from.row { to.row - 1 } else { to.row };
        Some(((from.row as u32 + 1, last as u32 + 1), s.selected_text().to_string()))
    }

    /// Paint changed lines — added/modified fills, a marker where lines went away — against
    /// the chat's review baseline while the file is pending there, else against HEAD. Git runs
    /// in the background; the fills land when it's done.
    pub(crate) fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        if self.scratch {
            return;
        }
        let path = self.path.clone();
        let review = self.workspace.read(cx).file_review(&path);
        self.review = review.clone();
        self._diff_read = Some(cx.spawn(async move |this, cx| {
            let against = review.map(|r| (r.top, r.base, r.rel));
            let (lines, hunks) = cx
                .background_executor()
                .spawn(async move {
                    let hunks = against.and_then(|(top, base, rel)| {
                        let repo = trek_core::checkpoint::Repo::find(&top)?;
                        repo.file_diff(&base, &rel).map_err(|e| tracing::warn!("review diff of {rel}: {e:#}")).ok()
                    });
                    match hunks {
                        Some(h) => (review_marks(&h), Some(h)),
                        None => (diff_line_ranges(&path), None),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let review = hunks.is_some();
                this.hunk = this.hunk.min(hunks.as_ref().map_or(0, |h| h.hunks.len().saturating_sub(1)));
                this.hunks = hunks;
                this.paint_diff(lines, review, cx);
                cx.notify();
            });
        }));
    }

    fn paint_diff(&mut self, lines: Vec<(u32, u32, DiffMark)>, review: bool, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.painted = lines
                .iter()
                .map(|(a, b, k)| (*a, *b, match k { DiffMark::Added => "added", DiffMark::Changed => "changed", DiffMark::Deleted => "deleted", DiffMark::Removed => "removed" }))
                .collect();
        }
        // The collection and the ranges are computed inside the state's update; the set
        // itself is another update, so it happens outside.
        let diff_marks = &mut self.diff_marks;
        let self_scratch = self.scratch;
        let (decs, marks) = self.state.update(cx, |s, cx| {
            if diff_marks.is_none() {
                *diff_marks = Some(s.create_range_decorations_collection(vec![], cx));
            }
            let text = s.text().clone();
            let last_row = text.offset_to_point(text.len()).row as u32;
            let theme = cx.theme().clone();
            let decs: Vec<_> = lines
                .into_iter()
                // A deletion at the very end marks the last line.
                .map(|(a, b, kind)| if matches!(kind, DiffMark::Deleted) && a > last_row { (last_row, last_row + 1, kind) } else { (a, b, kind) })
                .filter(|(a, _, _)| *a <= last_row)
                .map(|(a, b, kind)| {
                    let b = b.min(last_row + 1);
                    let start = text.point_to_offset(Point::new(a as usize, 0));
                    let end = if matches!(kind, DiffMark::Deleted) {
                        start + 4.min(text.len().saturating_sub(start))
                    } else {
                        text.point_to_offset(Point::new(b as usize, 0)).max(start)
                    };
                    // A review's hunks read stronger than the quiet HEAD fills.
                    let color = match (kind, review) {
                        (DiffMark::Removed, _) => theme.danger.opacity(0.16),
                        (DiffMark::Changed, true) if self_scratch => theme.info.opacity(0.10),
                        (DiffMark::Added | DiffMark::Changed, true) => theme.success.opacity(0.17),
                        (DiffMark::Deleted, true) => theme.danger.opacity(0.5),
                        (DiffMark::Added, false) => theme.success.opacity(0.12),
                        (DiffMark::Changed, false) => theme.info.opacity(0.10),
                        (DiffMark::Deleted, false) => theme.danger.opacity(0.35),
                    };
                    gpui_kit::base::input::RangeDecoration::new(start..end)
                        .with_style(gpui_kit::base::input::RangeDecorationStyle::Fill)
                        .with_color(color)
                })
                .collect();
            (decs, diff_marks.clone().unwrap())
        });
        marks.set(decs, cx);
    }

    /// Show the file as it is on disk now: another branch or commit was checked out, or a rewind
    /// put files back. Unsaved edits stay (saving them asks first, `save_now`).
    pub(crate) fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scratch || self.loading {
            return;
        }
        let (text, problem) = load(&self.path);
        if self.dirty || (text == self.saved && problem == self.problem) {
            self.refresh_diff(cx);
            return;
        }
        self.saved = text.clone();
        self.problem = problem;
        self.overwrite_armed = None;
        let readonly = self.problem.is_some();
        self.state.update(cx, |s, cx| {
            s.set_value(text.clone(), window, cx);
            s.set_readonly(readonly, cx);
        });
        if let Some((client, uri)) = &self.lsp {
            client.did_change(uri, &text);
        }
        self.dirty = false;
        self.refresh_diff(cx);
        cx.notify();
    }

    /// An agent may have written the file: a clean buffer shows it as it is on disk now. Nothing
    /// happens when it's the same, or while there are unsaved edits (saving them asks first).
    pub(crate) fn reload_if_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scratch || self.dirty || self.loading {
            return;
        }
        let (text, problem) = load(&self.path);
        if text != self.saved || problem != self.problem {
            self.reload_from_disk(window, cx);
        }
    }

    /// Put the caret on a line (deep links point here) and take the focus.
    pub fn goto_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            self.pending_line = Some(line);
            return;
        }
        self.state.update(cx, |s, cx| s.set_cursor_position(Position::new(line.saturating_sub(1), 0), window, cx));
    }

    /// Take the window's focus (the deferred focus call lands here).
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.focus(window, cx));
    }

    fn save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
    }

    /// Write the buffer back to its file. No-op when clean.
    pub(crate) fn save_now(&mut self, cx: &mut Context<Self>) {
        if !self.dirty || self.scratch || self.loading {
            return;
        }
        // Changed on disk since it was loaded (another branch checked out, a rewind, an agent):
        // writing now would quietly put the old version back. Say so; saving again overwrites,
        // unless it changed once more since it was said.
        let on_disk = read_text(&self.path).ok();
        let seen = text_hash(on_disk.as_deref());
        if on_disk.as_deref() != Some(self.saved.as_str()) && self.overwrite_armed != Some(seen) {
            self.overwrite_armed = Some(seen);
            let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let what = if on_disk.is_some() { "changed on disk" } else { "was removed from disk" };
            let message = format!("{name} {what} since you opened it (another branch, a rewind or an agent). Save again to write your version over it.");
            self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            return;
        }
        let text = self.state.read(cx).value().to_string();
        match write_atomically(&self.path, &text) {
            Ok(()) => {
                if let Some((client, uri)) = &self.lsp {
                    client.did_save(uri, &text);
                }
                self.saved = text;
                self.dirty = false;
                self.overwrite_armed = None;
                cx.notify();
                self.refresh_diff(cx);
            }
            Err(e) => self.workspace.update(cx, |ws, cx| {
                cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't save {}: {e}", self.path.display()).into(), undo: None });
                let _ = ws;
            }),
        }
    }
}

/// What a file holds as text, read safely: only a regular file (a device or a pipe would never
/// end, or never start), at most `MAX_FILE_BYTES`, and UTF-8.
fn read_text(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Opening a pipe for reading waits for a writer; not with O_NONBLOCK. (Windows has no FIFOs:
    // it opens as it is, and `is_file` below turns devices away.)
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, "too big"));
    }
    let mut bytes = vec![];
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, "too big"));
    }
    String::from_utf8(bytes).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not text"))
}

/// A hash of what a file held (`None`: it was gone), to tell one version on disk from another.
fn text_hash(text: Option<&str>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Write `text` to `path` so that a crash or a full disk midway leaves the old file whole: into
/// a file of its own next to it (the real file, through any link), flushed to disk, then moved
/// over it. The file keeps its permissions.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = dir.join(format!(".{name}.trek-save-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Read the file; a reason for read-only otherwise.
fn load(path: &Path) -> (String, Option<String>) {
    match read_text(path) {
        Ok(t) => (t, None),
        Err(e) => {
            let why = match e.kind() {
                std::io::ErrorKind::FileTooLarge => "Too big to edit — open it in your editor.".to_string(),
                std::io::ErrorKind::InvalidData => "Not text — open it in another app.".to_string(),
                std::io::ErrorKind::InvalidInput => "Not a file (a device, a pipe or a folder) — Trek edits files only.".to_string(),
                _ => format!("Can't read it: {e}"),
            };
            (String::new(), Some(why))
        }
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        if let Some((client, uri)) = &self.lsp {
            client.did_close(uri);
        }
    }
}

/// Hunks and ⌘K.
impl EditorView {
    /// Hunks of this file pending review in the AI side bar's chat (none: the fills are HEAD's).
    pub fn pending_hunks(&self) -> usize {
        self.review.as_ref().and(self.hunks.as_ref()).map_or(0, |h| h.hunks.len())
    }

    /// The hunk the bar acts on: the caret's, else the one last stepped to.
    fn current_hunk(&self, cx: &App) -> Option<usize> {
        let hunks = &self.hunks.as_ref().filter(|_| self.review.is_some())?.hunks;
        if hunks.is_empty() {
            return None;
        }
        let row = self.state.read(cx).cursor_position().line;
        Some(hunks.iter().position(|h| (h.first_row()..=h.last_row()).contains(&row)).unwrap_or(self.hunk.min(hunks.len() - 1)))
    }

    /// Put the caret on hunk `ix` (scrolled into view).
    fn goto_hunk(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.hunks.as_ref().and_then(|h| h.hunks.get(ix)).map(|h| h.first_row()) else { return };
        self.hunk = ix;
        self.state.update(cx, |s, cx| {
            s.set_cursor_position(Position::new(row, 0), window, cx);
            // A third of the way down, so the bars under or over it have room.
            if let Some(line) = s.line_height() {
                let height = s.input_bounds().size.height;
                let y = (line * row as f32 - height / 3.).max(px(0.));
                let x = s.scroll_offset().x;
                s.set_scroll_offset(gpui_kit::point(x, -y), cx);
            }
        });
        cx.notify();
    }

    /// ▲▼: the previous or next hunk, round the ends.
    pub(crate) fn step_hunk(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.pending_hunks();
        let Some(at) = self.current_hunk(cx).filter(|_| n > 0) else { return };
        self.goto_hunk((at as isize + step).rem_euclid(n as isize) as usize, window, cx);
    }

    /// Keep (⌘Y) or undo (⌥⌘⌫) the hunk the bar is on.
    pub(crate) fn act_on_hunk(&mut self, keep: bool, cx: &mut Context<Self>) {
        let (Some(review), Some(ix)) = (self.review.clone(), self.current_hunk(cx)) else { return };
        let Some(patch) = self.hunks.as_ref().and_then(|h| h.patch(ix)) else { return };
        if !keep && self.dirty {
            // The undo goes to the file on disk, which these edits aren't in yet.
            let message = "Save or drop your edits to this file first: the undo works on the file as saved.".to_string();
            self.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message, undo: None }));
            return;
        }
        self.hunk = ix;
        self.workspace.update(cx, |ws, cx| if keep { ws.keep_hunk(&review.thread, &review.rel, patch, cx) } else { ws.undo_hunk(&review.thread, &review.rel, patch, cx) });
    }

    /// Keep or undo this whole file of the chat's review (the file bar).
    fn act_on_file(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(review) = self.review.clone() else { return };
        self.workspace.update(cx, |ws, cx| if keep { ws.keep_files(&review.thread, Some(vec![review.rel.clone()]), cx) } else { ws.undo_files(&review.thread, Some(vec![review.rel.clone()]), cx) });
    }

    /// The file bar's ‹ ›: the review's previous or next pending file, on its first hunk.
    fn step_file(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(review) = self.review.clone() else { return };
        let ws = self.workspace.read(cx);
        let files = ws.pending_files(&review.thread);
        let Some(at) = files.iter().position(|f| f.path == review.rel) else { return };
        let next = files[(at as isize + step).rem_euclid(files.len() as isize) as usize].path.clone();
        if next == review.rel {
            return;
        }
        let path = review.top.join(next);
        self.workspace.update(cx, |ws, cx| {
            ws.reveal_hunk = Some(path.clone());
            ws.open_editor(path, None, cx);
        });
    }

    fn keep_hunk_action(&mut self, _: &KeepHunk, _: &mut Window, cx: &mut Context<Self>) {
        self.act_on_hunk(true, cx);
    }

    fn undo_hunk_action(&mut self, _: &UndoHunk, _: &mut Window, cx: &mut Context<Self>) {
        self.act_on_hunk(false, cx);
    }

    /// ⌘K: the prompt card over the picked lines (the caret's line, with none); again, it closes.
    pub(crate) fn open_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline.take().is_some() {
            self.focus(window, cx);
            cx.notify();
            return;
        }
        if self.scratch || self.problem.is_some() {
            return;
        }
        let (lines, text) = self.selection(cx).unwrap_or_else(|| {
            let s = self.state.read(cx);
            let row = s.cursor_position().line;
            let rope = s.text();
            let from = rope.point_to_offset(Point::new(row as usize, 0));
            let to = rope.point_to_offset(Point::new(row as usize + 1, 0)).max(from);
            ((row + 1, row + 1), rope.slice(from..to).to_string())
        });
        let which = if lines.0 == lines.1 { format!("line {}", lines.0) } else { format!("lines {}–{}", lines.0, lines.1) };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("Edit {which}: say what to change…")));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.run_inline(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.inline = Some(InlineCard { input, lines, text, _sub: sub });
        cx.notify();
    }

    fn inline_action(&mut self, _: &InlineEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.open_inline(window, cx);
    }

    /// Enter in the card: the request goes to the AI side bar's chat.
    fn run_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.inline.take() else { return };
        let instruction = card.input.read(cx).value().trim().to_string();
        if instruction.is_empty() {
            self.inline = Some(card);
            return;
        }
        let path = self.path.clone();
        self.workspace.update(cx, |ws, cx| ws.inline_edit(&path, card.lines, card.text, &instruction, cx));
        self.focus(window, cx);
        cx.notify();
    }

    fn close_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline.take().is_some() {
            self.focus(window, cx);
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn inline_open(&self) -> bool {
        self.inline.is_some()
    }

    /// Type `text` into ⌘K's card and run it (tests, the shots harness).
    #[cfg(feature = "shots")]
    pub fn type_inline(&mut self, text: &str, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.inline.is_none() {
            self.open_inline(window, cx);
        }
        let Some(card) = &self.inline else { return };
        card.input.update(cx, |i, cx| i.set_value(text.to_string(), window, cx));
        if run {
            self.run_inline(window, cx);
        }
    }

    /// Where `row` (0-based) is drawn, relative to the editor's top-left: its top and bottom,
    /// and the text's left edge. `None` when it's not laid out (scrolled away, not drawn yet).
    fn row_bounds(&self, row: u32, cx: &App) -> Option<(f32, f32, f32)> {
        let s = self.state.read(cx);
        let rope = s.text();
        let last = rope.offset_to_point(rope.len()).row as u32;
        let offset = rope.point_to_offset(Point::new(row.min(last) as usize, 0));
        let b = s.range_to_bounds(&(offset..offset))?;
        let origin = s.input_bounds().origin;
        Some(((b.origin.y - origin.y).as_f32(), (b.origin.y + b.size.height - origin.y).as_f32(), (b.origin.x - origin.x).as_f32()))
    }

    /// The bar on the hunk in view: ▲▼, Undo ⌥⌘⌫, Keep ⌘Y. It sits at the right under the
    /// hunk's last line (over its first near the bottom), or at the top when the hunk is out
    /// of view.
    fn hunk_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ix = self.current_hunk(cx)?;
        let hunks = self.hunks.as_ref()?;
        let n = hunks.hunks.len();
        let hunk = &hunks.hunks[ix];
        let height = self.state.read(cx).input_bounds().size.height.as_f32();
        // Under the hunk; over it when that's off the bottom (or behind the file bar); else
        // at the top.
        let fits = |t: &f32| *t > 0. && *t < height - 76.;
        let top = self
            .row_bounds(hunk.last_row(), cx)
            .map(|(_, bottom, _)| bottom + 3.)
            .filter(fits)
            .or_else(|| self.row_bounds(hunk.first_row(), cx).map(|(top, _, _)| top - 33.).filter(fits))
            .unwrap_or(8.);
        let theme = cx.theme().clone();
        let busy = self.review.as_ref().is_some_and(|r| self.workspace.read(cx).turn_running(&r.thread));
        let ember = palette::ember(cx);
        let (added, removed) = hunk.counts();
        let removed_text = hunk.removed_text();
        let button = |id: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .h(px(22.))
                .px(px(8.))
                .gap(px(5.))
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(12.))
        };
        let kbd = |k: String, c: Hsla| div().text_size(px(10.5)).text_color(c).child(k);
        Some(
            h_flex()
                .id("editor-hunk-bar")
                .test_support()
                .absolute()
                .top(px(top))
                .right(px(22.))
                .p(px(3.))
                .gap(px(2.))
                .items_center()
                .rounded(px(7.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_md()
                .text_color(theme.muted_foreground)
                .child(
                    h_flex()
                        .gap(px(1.))
                        .child(crate::ui::icon_button("editor-hunk-prev", IconName::ChevronUp, "Previous change").on_click(cx.listener(|this, _, window, cx| this.step_hunk(-1, window, cx))))
                        .child(crate::ui::icon_button("editor-hunk-next", IconName::ChevronDown, "Next change").on_click(cx.listener(|this, _, window, cx| this.step_hunk(1, window, cx)))),
                )
                .child(
                    h_flex()
                        .id("editor-hunk-count")
                        .flex_none()
                        .whitespace_nowrap()
                        .gap(px(4.))
                        .px(px(4.))
                        .text_size(px(11.5))
                        .font_family(theme.mono_font_family.clone())
                        .when(n > 1, |el| el.child(format!("{}/{n}", ix + 1)))
                        .when(added > 0, |el| el.child(span(format!("+{added}"), palette::emerald(cx))))
                        .when(removed > 0, |el| el.child(span(format!("−{removed}"), palette::red(cx))))
                        .when(!removed_text.is_empty(), |el| {
                            el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("Taken out:\n{}", removed_text.trim_end())).build(window, cx))
                        }),
                )
                .child(
                    button("editor-hunk-undo")
                        .when(busy, |el| el.opacity(0.45).cursor_default())
                        .hover(|s| s.bg(theme.foreground.opacity(0.07)).text_color(theme.foreground))
                        .child("Undo")
                        .child(kbd(keys::hint(Id::UndoHunk), theme.muted_foreground.opacity(0.7)))
                        .on_click(cx.listener(|this, _, _, cx| this.act_on_hunk(false, cx))),
                )
                .child(
                    button("editor-hunk-keep")
                        .bg(ember)
                        .text_color(gpui_kit::white())
                        .hover(|s| s.opacity(0.9))
                        .child("Keep")
                        .child(kbd(keys::hint(Id::KeepHunk), gpui_kit::white().opacity(0.75)))
                        .on_click(cx.listener(|this, _, _, cx| this.act_on_hunk(true, cx))),
                )
                .into_any_element(),
        )
    }

    /// The bar at the bottom while the chat's review has this file: ‹ name, File i of n › ·
    /// Undo file · Keep file.
    fn file_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let review = self.review.as_ref()?;
        let ws = self.workspace.read(cx);
        let files = ws.pending_files(&review.thread);
        let at = files.iter().position(|f| f.path == review.rel)?;
        let n = files.len();
        let busy = ws.turn_running(&review.thread);
        let theme = cx.theme().clone();
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let link = |id: &'static str| {
            h_flex()
                .id(id)
                .test_support()
                .h(px(22.))
                .px(px(6.))
                .gap(px(4.))
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.foreground.opacity(0.07)).text_color(theme.foreground))
        };
        Some(
            h_flex()
                .absolute()
                .bottom(px(14.))
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    h_flex()
                        .id("editor-file-bar")
                        .test_support()
                        .px(px(6.))
                        .py(px(3.))
                        .gap(px(4.))
                        .items_center()
                        .rounded(px(9.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .shadow_md()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .whitespace_nowrap()
                        .child(link("editor-file-prev").when(n < 2, |el| el.opacity(0.4)).child(Icon::new(IconName::ChevronLeft).size(px(13.))).on_click(cx.listener(|this, _, _, cx| this.step_file(-1, cx))))
                        .child(crate::file_icon::badge(&name, px(13.), cx))
                        .child(div().text_color(theme.foreground).font_weight(FontWeight::MEDIUM).child(name))
                        .child(div().px(px(4.)).child(format!("File {} of {n}", at + 1)))
                        .child(link("editor-file-next").when(n < 2, |el| el.opacity(0.4)).child(Icon::new(IconName::ChevronRight).size(px(13.))).on_click(cx.listener(|this, _, _, cx| this.step_file(1, cx))))
                        .child(div().w(px(1.)).h(px(14.)).mx(px(4.)).bg(theme.border))
                        .child(
                            link("editor-file-undo")
                                .when(busy, |el| el.opacity(0.45).cursor_default())
                                .child("Undo file")
                                .on_click(cx.listener(|this, _, _, cx| this.act_on_file(false, cx))),
                        )
                        .child(link("editor-file-keep").text_color(palette::ember(cx)).child("Keep file").on_click(cx.listener(|this, _, _, cx| this.act_on_file(true, cx)))),
                )
                .into_any_element(),
        )
    }

    /// ⌘K's card: over the picked lines, or under them when there's no room above.
    fn inline_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.inline.as_ref()?;
        const HEIGHT: f32 = 92.;
        let height = self.state.read(cx).input_bounds().size.height.as_f32();
        let first = self.row_bounds(card.lines.0 - 1, cx);
        let last = self.row_bounds(card.lines.1 - 1, cx);
        let (top, left) = match (first, last) {
            (Some((t, _, l)), _) if t - HEIGHT - 4. >= 4. => (t - HEIGHT - 4., l),
            (_, Some((_, b, l))) if b + HEIGHT + 4. <= height => (b + 4., l),
            (Some((_, _, l)), _) | (_, Some((_, _, l))) => (8., l),
            _ => (8., 60.),
        };
        let theme = cx.theme().clone();
        // Where it goes: the file's own ⌘K chat (made by the first edit), never the chat in front.
        let (chat, prefs) = self.workspace.read(cx).inline_target(&self.path);
        let (a, b) = card.lines;
        let which = if a == b { format!("Edit line {a}") } else { format!("Edit lines {a}–{b}") };
        let model = prefs.model.clone().unwrap_or_else(|| prefs.agent.display_name());
        let kbd = |k: String, label: &'static str| h_flex().gap(px(4.)).child(div().text_color(theme.foreground.opacity(0.8)).child(k)).child(label);
        Some(
            v_flex()
                .id("editor-inline")
                .test_support()
                .absolute()
                .top(px(top))
                .left(px(left.max(8.)))
                .w(px(440.))
                .max_w(relative(0.9))
                .p(px(8.))
                .gap(px(6.))
                .rounded(px(9.))
                .border_1()
                .border_color(palette::ember(cx).opacity(0.55))
                .bg(theme.popover)
                .shadow_lg()
                .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    cx.stop_propagation();
                    this.close_inline(window, cx);
                }))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .text_size(px(11.5))
                        .text_color(theme.muted_foreground)
                        .child(div().text_color(palette::ember(cx)).child("✦"))
                        .child(div().text_color(theme.foreground).font_weight(FontWeight::MEDIUM).child(which))
                        .child(div().id("editor-inline-chat").test_support().flex_1().min_w_0().truncate().child(format!("· in {chat}")))
                        .child(crate::ui::agent_glyph(&prefs.agent, cx))
                        .child(div().flex_none().max_w(px(140.)).truncate().child(model)),
                )
                .child(Input::new(&card.input).small())
                .child(
                    h_flex()
                        .gap(px(12.))
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground)
                        .child(kbd(keys::localize("↵").into_owned(), "Run"))
                        .child(kbd("Esc".into(), "Cancel"))
                        .child(div().flex_1())
                        .child("The change comes back to keep or undo"),
                )
                .into_any_element(),
        )
    }
}

fn span(text: String, color: Hsla) -> impl IntoElement {
    div().text_color(color).child(text)
}

impl EditorView {
    /// The text's state: the workbench watches it for the caret (the status bar's Ln, Col).
    pub fn text_state(&self) -> Entity<EditorState> {
        self.state.clone()
    }

    /// The caret, 1-based (line, column).
    pub fn caret(&self, cx: &App) -> (u32, u32) {
        let p = self.state.read(cx).cursor_position();
        (p.line + 1, p.character + 1)
    }

    /// Why the file is read-only, when it is (too big, not text, gone).
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Save now (⌘S, or Save in the close prompt); `false` when it didn't land (the on-disk
    /// guard spoke up, or the write failed).
    pub(crate) fn save_for_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.save_now(cx);
        !self.dirty
    }
}

#[cfg(test)]
impl EditorView {
    pub fn dirty(&self) -> bool {
        self.dirty
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // ~ for home, since headers are tight; elsewhere the path as it is.
        let dir = self.path.parent().map(trek_core::paths::tildify).unwrap_or_default();
        let dirty = self.dirty;
        let path = self.path.clone();
        // In the editor the tab, breadcrumbs and ⌘S stand in for this header; a read-only file
        // still says why, in a slim note.
        if self.workspace.read(cx).ide() {
            // The file bar stepped here: the caret goes to the first hunk once they're read.
            if self.jump_to_hunk && self.pending_hunks() > 0 {
                self.jump_to_hunk = false;
                cx.defer_in(window, |this, window, cx| this.goto_hunk(0, window, cx));
            }
            let hunks = self.pending_hunks() > 0;
            let (hunk_bar, file_bar, card) = if self.scratch { (None, None, None) } else { (self.hunk_bar(cx), self.file_bar(cx), self.inline_card(cx)) };
            return v_flex()
                .size_full()
                .key_context(if hunks { "TrekEditor IdeEditor hunks" } else { "TrekEditor IdeEditor" })
                .on_action(cx.listener(Self::save))
                .on_action(cx.listener(Self::inline_action))
                .on_action(cx.listener(Self::keep_hunk_action))
                .on_action(cx.listener(Self::undo_hunk_action))
                .on_action(cx.listener(|this, _: &NextHunk, window, cx| this.step_hunk(1, window, cx)))
                .on_action(cx.listener(|this, _: &PreviousHunk, window, cx| this.step_hunk(-1, window, cx)))
                .when_some(self.problem.clone(), |el, p| {
                    el.child(h_flex().px_3().h(px(26.)).flex_none().border_b_1().border_color(theme.border).text_xs().text_color(palette::red(cx)).child(p))
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(Editor::new(&self.state).appearance(false).bordered(false).h(relative(1.)))
                        .children(hunk_bar)
                        .children(file_bar)
                        .children(card),
                )
                .into_any_element();
        }

        v_flex()
            .size_full()
            .key_context("TrekEditor")
            .on_action(cx.listener(Self::save))
            .child(
                h_flex()
                    .px_3()
                    .h(px(40.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(Icon::new(crate::assets::Lucide::FilePen).small().text_color(theme.muted_foreground))
                    .child(div().min_w_0().truncate().font_medium().child(name))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(dir))
                    .when(dirty, |el| el.child(div().text_color(palette::ember(cx)).child("●")))
                    .when_some(self.problem.clone(), |el, p| {
                        el.child(div().text_xs().text_color(palette::red(cx)).child(p))
                    })
                    .child(
                        Button::new("editor-save").ghost().small().label("Save").tooltip(keys::shared("Save (⌘S)"))
                            .when(!dirty, |b| b.disabled(true))
                            .on_click(cx.listener(|this, _, window, cx| this.save(&SaveFile, window, cx))),
                    )
                    .child(crate::ui::icon_button("editor-open-default", IconName::ExternalLink, "Open in default app").on_click(move |_, _, cx| {
                        cx.open_with_system(&path);
                    })),
            )
            .child(div().flex_1().min_h_0().child(Editor::new(&self.state).appearance(false).bordered(false).h(relative(1.))))
            .into_any_element()
    }
}
