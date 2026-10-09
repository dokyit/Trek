//! Keep / Undo: reviewing what a chat's agent changed, file by file. Agents write to disk as they
//! go, so nothing is staged: a review is a baseline, the files as they were before the chat's
//! first turn not yet reviewed (that turn's checkpoint), and what has changed since.
//!
//! A review opens when a turn starts in a thread that has a tab in the editor's AI side bar and
//! no review open. Its pending files are those changed since the baseline that this thread
//! touched: the files its agent's edit tools named, and those its turns' checkpoints differ in
//! (each turn from the checkpoint taken as it started to the one taken as it ended, a running
//! one to the files now; the same span the turn's changes card counts). That keeps the user's
//! own edits in other files, and other threads', out, during the turns and after them. Keep
//! takes a file's contents now into the baseline; Undo puts the baseline's back (not while a
//! turn runs: it would race the agent), keeping the files as they were first so a toast can
//! take it back. A file changed since its last turn ended (by the user, say) isn't undone whole:
//! that would lose those edits. Undo all asks twice. Once nothing is pending and no turn runs,
//! the review closes, and the next turn starts another.
//!
//! Outside git the files come from the agent's edit tools alone, and can't be undone: Keep
//! takes them off the list until the agent edits them again.
//!
//! A review is kept in the store as it moves (its start, baseline and what was kept), so it
//! outlives the app. Hunks go the same way as files: Keep folds one into the baseline, Undo takes
//! it back out of the file on disk (`checkpoint::Repo::keep_hunk`, `undo_hunk`). The git work
//! runs off the main thread.

use super::{Workspace, WorkspaceEvent, in_repo};
use crate::activity::{ToolKind, tool_kind};
use gpui_kit::{Context, Task};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use trek_core::changes::{FileChange, FileStatus};
use trek_core::checkpoint::Repo;
use trek_core::store::{Item, ToolStatus};

/// How long changes settle before the pending files are worked out again (an agent's edits
/// come in bursts).
const SETTLE: Duration = Duration::from_millis(250);

/// How long a first Undo all waits for the second that does it.
const UNDO_ALL_CONFIRM: Duration = Duration::from_secs(8);

/// One thread's review.
pub struct Review {
    /// The message whose turn the review starts at: its checkpoint is the baseline.
    pub start: String,
    /// In a git repository (the thread's folder was when it opened).
    pub git: bool,
    /// What the pending files' paths are relative to: the repository's top folder, or outside
    /// git the thread's folder. `None` until first worked out.
    pub root: Option<PathBuf>,
    /// The baseline: the start checkpoint's tree, with the files kept since taken in. `None`
    /// until the checkpoint is in.
    base: Option<String>,
    /// Files changed since the baseline that this thread touched, by path.
    pub pending: Vec<FileChange>,
    /// Outside git: files kept, with how many of the agent's edits to them there had been then.
    kept_edits: HashMap<String, usize>,
    /// Worked out at least once.
    pub counted: bool,
    /// Bumped per piece of work started, so an older one's result is dropped.
    run: u64,
    /// Working out the pending files again soon (`SETTLE`), or now.
    _work: Option<Task<()>>,
    /// Keep or Undo under way: its result moves the files and the baseline. Its own slot:
    /// working the files out again mustn't drop it half done.
    _op: Option<Task<()>>,
    /// Keep or Undo under way.
    busy: bool,
    /// Where each pending file stood as the last turn that changed it ended (a checkpoint):
    /// undoing a file changed since would lose those changes.
    ends: HashMap<String, String>,
    /// When Undo all was asked for once: asked again soon after, it goes ahead.
    undo_all_asked: Option<Instant>,
    /// Asked to work out the pending files again, and not done yet.
    settling: bool,
}

/// What the store keeps of a review.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    start: String,
    git: bool,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    kept_edits: HashMap<String, usize>,
}

/// The review the editor shows a file against (`Workspace::file_review`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReview {
    pub thread: String,
    /// The repository's top folder (as the thread's folder names it) and the file under it.
    pub top: PathBuf,
    pub rel: String,
    /// The baseline tree.
    pub base: String,
}

impl Review {
    fn new(start: String, git: bool) -> Self {
        Self {
            start,
            git,
            root: None,
            base: None,
            pending: vec![],
            kept_edits: HashMap::new(),
            counted: false,
            run: 0,
            _work: None,
            _op: None,
            busy: false,
            ends: HashMap::new(),
            undo_all_asked: None,
            settling: false,
        }
    }

    /// Lines added and removed over the pending files.
    pub fn totals(&self) -> (u32, u32) {
        self.pending.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed))
    }

    pub fn is_pending(&self, path: &str) -> bool {
        self.pending.iter().any(|f| f.path == path)
    }

    /// Undo all was asked for once, and asking again now does it.
    pub fn undo_all_armed(&self) -> bool {
        self.undo_all_asked.is_some_and(|at| at.elapsed() <= UNDO_ALL_CONFIRM)
    }
}

/// What working out a review needs from the transcript, read on the main thread.
struct Inputs {
    cwd: PathBuf,
    /// The checkpoint the review starts from.
    start: Option<String>,
    /// Its turns' checkpoints, oldest first: as each started, and as it ended (`None`: the
    /// running turn, up to the files now).
    spans: Vec<(String, Option<String>)>,
    /// Files the agent's edit tools named since the start, as reported (absolute or relative to
    /// `cwd`), with how many edits each.
    edits: Vec<(String, usize)>,
    /// Lines each edit-tool file was reported to change, where every call said.
    lines: HashMap<String, Option<(u32, u32)>>,
}

impl Workspace {
    /// The review of `id`'s changes, while one is open.
    pub fn review(&self, id: &str) -> Option<&Review> {
        self.reviews.get(id)
    }

    /// `id`'s files waiting on Keep or Undo.
    pub fn pending_files(&self, id: &str) -> &[FileChange] {
        self.reviews.get(id).map_or(&[], |r| &r.pending)
    }

    /// Where `id`'s review stands in git: the repository's top folder and the baseline tree, once
    /// both are known (the IDE's diff tab reads the pending files' hunks against it).
    pub fn review_base(&self, id: &str) -> Option<(PathBuf, String)> {
        let r = self.reviews.get(id).filter(|r| r.git)?;
        Some((r.root.clone()?, r.base.clone()?))
    }

    /// Whether an agent changed `path` (absolute) and the change hasn't been kept or undone:
    /// its editor tab says so.
    pub fn agent_edited(&self, path: &Path) -> bool {
        self.reviews.values().any(|r| r.root.as_deref().and_then(|root| path.strip_prefix(root).ok()).is_some_and(|rel| r.is_pending(&rel.to_string_lossy())))
    }

    /// A turn started on `id` with message `item`: a review opens at it when the thread is in
    /// the AI side bar and has none open (a later turn joins the open one).
    pub(super) fn review_turn_started(&mut self, id: &str, item: &str, cx: &mut Context<Self>) {
        if self.reviews.contains_key(id) || !self.ide_chat.tabs.iter().any(|t| matches!(t, super::IdeTab::Thread(t) if t == id)) {
            return;
        }
        let git = in_repo(self.thread(id).and_then(|t| t.cwd.as_deref()));
        self.reviews.insert(id.to_string(), Review::new(item.to_string(), git));
        self.save_review(id);
        self.review_moved(id, cx);
    }

    /// The store keeps `id`'s review as it is now.
    fn save_review(&self, id: &str) {
        let Some(r) = self.reviews.get(id) else { return };
        let saved = Saved { start: r.start.clone(), git: r.git, base: r.base.clone(), kept_edits: r.kept_edits.clone() };
        if let Ok(data) = serde_json::to_string(&saved) {
            if let Err(e) = self.store.set_review(id, Some(&data)) {
                tracing::warn!("save the review of {id}: {e:#}");
            }
        }
    }

    /// Reviews left open when the app last quit come back, their pending files worked out again.
    pub(super) fn restore_reviews(&mut self, cx: &mut Context<Self>) {
        for (id, data) in self.store.reviews().unwrap_or_default() {
            let Ok(saved) = serde_json::from_str::<Saved>(&data) else { continue };
            if self.thread(&id).is_none_or(|t| t.archived_at.is_some()) {
                let _ = self.store.set_review(&id, None);
                continue;
            }
            let mut review = Review::new(saved.start, saved.git);
            review.base = saved.base;
            review.kept_edits = saved.kept_edits;
            self.reviews.insert(id.clone(), review);
            self.review_moved(&id, cx);
        }
    }

    /// `id`'s files may have moved (the agent edited, a turn ended, a checkpoint came in, files
    /// were put back): its pending files are worked out again once things settle.
    pub(crate) fn review_moved(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.get_mut(id) else { return };
        review.run += 1;
        review.settling = true;
        let run = review.run;
        let id = id.to_string();
        review._work = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |ws, cx| ws.count_review(&id, run, cx));
        }));
    }

    /// What `count_review` reads from the transcript; `None` when the review is over (its start
    /// left the transcript) or must wait (its checkpoint is still being taken).
    fn review_inputs(&mut self, id: &str, cx: &mut Context<Self>) -> Option<Inputs> {
        let (start, git) = self.reviews.get(id).map(|r| (r.start.clone(), r.git))?;
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        // A review kept from before a restart: its transcript is read first.
        if self.live.get(id).is_none_or(|l| !l.loaded) && cwd.is_some() {
            self.ensure_loaded(id, cx);
            if self.live.get(id).is_none_or(|l| !l.loaded) {
                return None;
            }
        }
        let Some((live, cwd)) = self.live.get(id).zip(cwd) else {
            self.close_review(id, cx);
            return None;
        };
        let Some(from) = live.items.position(&start) else {
            // Rewound past its start: there's nothing of it left to review.
            self.close_review(id, cx);
            return None;
        };
        let mut start_sha = None;
        let mut spans = vec![];
        if git {
            let taken: HashMap<String, String> = self.store.checkpoints(id).unwrap_or_default().into_iter().map(|c| (c.item_id, c.sha)).collect();
            if !taken.contains_key(&start) {
                if live.checkpoint_failed.contains_key(&start) {
                    // No baseline to be had: the agent's edits are what there is.
                    if let Some(r) = self.reviews.get_mut(id) {
                        r.git = false;
                    }
                } else {
                    // Still being taken: worked out once it's in (`git_done`).
                    return None;
                }
            }
            start_sha = taken.get(&start).cloned();
            let running = live.turn_started.is_some();
            let git_work = live.git_busy || !live.git_jobs.is_empty();
            let stretches = self.turn_stretches(id, from);
            let last = stretches.len().saturating_sub(1);
            let live = self.live.get(id)?;
            let sha = |pos: usize| live.items.id_at(pos).and_then(|i| taken.get(i)).cloned();
            for (k, s) in stretches.iter().enumerate() {
                let Some(from) = sha(s.start) else { continue };
                match s.end.map(sha) {
                    Some(Some(to)) => spans.push((from, Some(to))),
                    // Its end checkpoint is still being taken: worked out once it's in.
                    Some(None) => return None,
                    None if k == last && running => spans.push((from, None)),
                    // Just ended: its end checkpoint is on its way.
                    None if k == last && git_work => return None,
                    // Nothing ends it (from before Trek took end checkpoints, or that failed):
                    // its agent's edits stand for it.
                    None => {}
                }
            }
        }
        let live = self.live.get(id)?;
        let mut edits: Vec<(String, usize)> = vec![];
        let mut lines: HashMap<String, Option<(u32, u32)>> = HashMap::new();
        for item in &live.items[from..] {
            let Item::Tool { id: call, title, detail, status, .. } = item else { continue };
            if matches!(status, ToolStatus::Failed | ToolStatus::Denied) || (tool_kind(title) != ToolKind::Edit && title != "Delete") {
                continue;
            }
            let paths: Vec<&str> = detail.split(", ").map(str::trim).filter(|p| !p.is_empty()).collect();
            let counted = if paths.len() == 1 { live.lines.get(call).copied() } else { None };
            for p in paths {
                match edits.iter_mut().find(|(e, _)| e == p) {
                    Some((_, n)) => *n += 1,
                    None => edits.push((p.to_string(), 1)),
                }
                let entry = lines.entry(p.to_string()).or_insert(Some((0, 0)));
                *entry = match (*entry, counted) {
                    (Some((a, r)), Some((da, dr))) => Some((a + da, r + dr)),
                    _ => None,
                };
            }
        }
        Some(Inputs { cwd, start: start_sha, spans, edits, lines })
    }

    /// Work out `id`'s pending files (run `run` of `review_moved`).
    fn count_review(&mut self, id: &str, run: u64, cx: &mut Context<Self>) {
        if self.reviews.get(id).is_none_or(|r| r.run != run || r.busy) {
            return;
        }
        let Some(inputs) = self.review_inputs(id, cx) else { return };
        let Some(review) = self.reviews.get_mut(id) else { return };
        if !review.git {
            // From the agent's edit tools: what it edited, less what was kept since its last edit.
            let cwd = inputs.cwd.clone();
            review.root = Some(cwd.clone());
            review.pending = inputs
                .edits
                .iter()
                .map(|(p, n)| (relative(p, &cwd), *n, inputs.lines.get(p).copied().flatten()))
                .filter(|(p, n, _)| review.kept_edits.get(p).is_none_or(|k| n > k))
                .map(|(path, _, lines)| FileChange { path, status: FileStatus::Modified, added: lines.map_or(0, |l| l.0), removed: lines.map_or(0, |l| l.1), binary: false, lines_known: lines.is_some() })
                .collect();
            review.pending.sort_by(|a, b| a.path.cmp(&b.path));
            review.counted = true;
            review.settling = false;
            self.review_counted(id, cx);
            return;
        }
        let base = review.base.clone();
        let task = cx.spawn({
            let id = id.to_string();
            async move |this, cx| {
                let counted = cx.background_executor().spawn(async move { count(inputs, base) }).await;
                let _ = this.update(cx, |ws, cx| {
                    let Some(review) = ws.reviews.get_mut(&id).filter(|r| r.run == run && !r.busy) else { return };
                    review.settling = false;
                    match counted {
                        Ok((top, base, pending, ends)) => {
                            review.root = Some(top);
                            review.ends = ends;
                            let first = review.base.is_none();
                            review.base.get_or_insert(base);
                            review.pending = pending;
                            review.counted = true;
                            if first {
                                ws.save_review(&id);
                            }
                        }
                        Err(e) => tracing::warn!("review of {id}: {e:#}"),
                    }
                    ws.review_counted(&id, cx);
                });
            }
        });
        if let Some(r) = self.reviews.get_mut(id) {
            r._work = Some(task);
        }
    }

    /// The pending files are in: with none left and no turn running, the review is over.
    fn review_counted(&mut self, id: &str, cx: &mut Context<Self>) {
        let done = self.reviews.get(id).is_some_and(|r| r.counted && r.pending.is_empty()) && !self.turn_running(id);
        if done {
            self.close_review(id, cx);
        } else {
            cx.emit(WorkspaceEvent::ReviewChanged { id: id.to_string() });
            cx.notify();
        }
    }

    fn close_review(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(review) = self.reviews.remove(id) {
            let _ = self.store.set_review(id, None);
            if let Some(top) = review.root.filter(|_| review.base.is_some()) {
                let thread = id.to_string();
                cx.background_executor()
                    .spawn(async move {
                        if let Some(Err(e)) = Repo::find(&top).map(|r| r.pin_review(&thread, None)) {
                            tracing::warn!("unpin review of {thread}: {e:#}");
                        }
                    })
                    .detach();
            }
            cx.emit(WorkspaceEvent::ReviewChanged { id: id.to_string() });
            cx.notify();
        }
    }

    /// Keep `paths` of `id`'s pending files (all of them with `None`): their contents now become
    /// the baseline. Allowed while a turn runs: a later edit makes a file pending again.
    pub fn keep_files(&mut self, id: &str, paths: Option<Vec<String>>, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.get_mut(id).filter(|r| !r.busy) else { return };
        let paths = paths.unwrap_or_else(|| review.pending.iter().map(|f| f.path.clone()).collect());
        let paths: Vec<String> = paths.into_iter().filter(|p| review.is_pending(p)).collect();
        if paths.is_empty() {
            return;
        }
        // Off the list right away; the next count says what's left.
        review.pending.retain(|f| !paths.contains(&f.path));
        if !review.git {
            let edits = self.live.get(id).map(|l| edit_counts(&l.items, l.items.position(&review.start).unwrap_or(0))).unwrap_or_default();
            let cwd = review.root.clone().unwrap_or_default();
            for p in paths {
                let n = edits.iter().find(|(e, _)| relative(e, &cwd) == p).map_or(0, |(_, n)| *n);
                review.kept_edits.insert(p, n);
            }
            self.save_review(id);
            self.review_counted(id, cx);
            return;
        }
        let (Some(top), Some(base)) = (review.root.clone(), review.base.clone()) else { return };
        review.busy = true;
        review.run += 1;
        let tid = id.to_string();
        let thread = tid.clone();
        let task = cx.spawn(async move |this, cx| {
            let kept = cx
                .background_executor()
                .spawn(async move {
                    let r = Repo::find(&top).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", top.display()))?;
                    let now = r.tree_now()?;
                    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                    let tree = r.tree_with(&base, &now, &refs)?;
                    r.pin_review(&thread, Some(&tree))?;
                    Ok::<_, anyhow::Error>(tree)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let Some(review) = ws.reviews.get_mut(&tid) else { return };
                review.busy = false;
                match kept {
                    Ok(tree) => {
                        review.base = Some(tree);
                        ws.save_review(&tid);
                        ws.review_counted(&tid, cx);
                    }
                    // Not kept: they're still pending (the count puts them back on the list).
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't keep the changes: {e:#}"), undo: None }),
                }
                ws.review_moved(&tid, cx);
            });
        });
        if let Some(r) = self.reviews.get_mut(id) {
            r._op = Some(task);
        }
    }

    /// Undo `paths` of `id`'s pending files (all of them with `None`): the baseline's contents
    /// come back, or the file goes where the baseline had none. The files as they were are kept
    /// first (`checkpoint::Repo::save_undo`), and the toast can put them back. A file changed
    /// since the last turn that changed it ended (the user's edits, a formatter) is left as it
    /// is, and the toast says so: undoing it whole would lose those. All of them asks first:
    /// the first call says what it would do, a second soon after does it. Refused while a turn
    /// runs, and outside git (there's nothing to put back from).
    pub fn undo_files(&mut self, id: &str, paths: Option<Vec<String>>, cx: &mut Context<Self>) {
        if self.turn_running(id) {
            cx.emit(WorkspaceEvent::Toast { message: "Stop the running turn to undo its changes.".into(), undo: None });
            return;
        }
        let Some(review) = self.reviews.get_mut(id).filter(|r| !r.busy && r.git) else { return };
        let all = paths.is_none();
        if all && review.undo_all_asked.is_none_or(|at| at.elapsed() > UNDO_ALL_CONFIRM) {
            let n = review.pending.len();
            if n == 0 {
                return;
            }
            review.undo_all_asked = Some(Instant::now());
            let what = if n == 1 { "the 1 file".to_string() } else { format!("all {n} files") };
            cx.emit(WorkspaceEvent::Toast { message: format!("Undo all again to put back {what} this chat changed."), undo: None });
            cx.emit(WorkspaceEvent::ReviewChanged { id: id.to_string() });
            cx.notify();
            return;
        }
        review.undo_all_asked = None;
        let paths = paths.unwrap_or_else(|| review.pending.iter().map(|f| f.path.clone()).collect());
        // A rename goes back whole: the new name goes, the old one comes back.
        let mut targets: Vec<String> = vec![];
        for f in review.pending.iter().filter(|f| paths.contains(&f.path)) {
            targets.push(f.path.clone());
            if let FileStatus::Renamed { from } = &f.status {
                targets.push(from.clone());
            }
        }
        let (Some(top), Some(base)) = (review.root.clone(), review.base.clone()) else { return };
        if targets.is_empty() {
            return;
        }
        let ends: Vec<(String, Option<String>)> = targets.iter().map(|t| (t.clone(), review.ends.get(t).cloned())).collect();
        review.pending.retain(|f| !paths.contains(&f.path));
        review.busy = true;
        review.run += 1;
        let tid = id.to_string();
        let thread = tid.clone();
        let task = cx.spawn(async move |this, cx| {
            let undone = cx
                .background_executor()
                .spawn(async move {
                    let r = Repo::find(&top).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", top.display()))?;
                    let undo = r.save_undo(&thread)?;
                    // Changed since its turn ended: as the undo kept it, against that turn's end.
                    let mut changed = vec![];
                    let mut go = vec![];
                    for (path, end) in &ends {
                        let moved = match end {
                            Some(end) => r.entries(end, &[path])? != r.entries(&undo, &[path])?,
                            None => false,
                        };
                        if moved { changed.push(path.clone()) } else { go.push(path.clone()) }
                    }
                    let refs: Vec<&str> = go.iter().map(String::as_str).collect();
                    let failed = r.restore_paths(&base, &refs)?;
                    Ok::<_, anyhow::Error>((undo, go, changed, failed, r.top))
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if let Some(review) = ws.reviews.get_mut(&tid) {
                    review.busy = false;
                }
                match undone {
                    Ok((undo, go, changed, failed, top)) => {
                        let mut message = vec![];
                        let undone: Vec<String> = go.iter().filter(|p| !failed.iter().any(|(f, _)| f == *p)).cloned().collect();
                        if !undone.is_empty() {
                            message.push(format!("Undid {}", files_named(&undone)));
                        }
                        if !changed.is_empty() {
                            let (one, it) = if changed.len() == 1 { ("was", "it") } else { ("were", "them") };
                            message.push(format!("{} {one} changed since the turn ended, so {it} stayed: undo {it} a change at a time, or keep {it}", files_named(&changed)));
                        }
                        if let Some((path, why)) = failed.first() {
                            let more = if failed.len() > 1 { format!(" and {} more", failed.len() - 1) } else { String::new() };
                            let what = if path.is_empty() { why.clone() } else { format!("{path}: {why}") };
                            message.push(format!("Couldn't undo {what}{more}"));
                        }
                        let undo = (!undone.is_empty()).then(|| super::UndoAction::Unrestore { thread: tid.clone(), repo: top.clone(), sha: undo, paths: go });
                        if !message.is_empty() {
                            cx.emit(WorkspaceEvent::Toast { message: message.join(" · "), undo });
                        }
                        // Open editors show the files put back; the folder's git state moved.
                        ws.files_epoch += 1;
                        ws.refresh_git_at(top, cx);
                        ws.forget_turn_changes(&tid, false, cx);
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't undo the changes: {e:#}"), undo: None }),
                }
                ws.review_moved(&tid, cx);
                cx.notify();
            });
        });
        if let Some(r) = self.reviews.get_mut(id) {
            r._op = Some(task);
        }
    }

    /// The review the editor shows `path` (absolute) against: the AI side bar's chat's, while the
    /// file is pending there and the review has its baseline. Otherwise the editor diffs
    /// against HEAD.
    pub fn file_review(&self, path: &Path) -> Option<FileReview> {
        let id = self.ide_chat.active_thread()?;
        let r = self.reviews.get(id).filter(|r| r.git)?;
        let (top, base) = (r.root.clone()?, r.base.clone()?);
        let rel = path.strip_prefix(&top).ok()?.to_string_lossy().to_string();
        r.is_pending(&rel).then(|| FileReview { thread: id.to_string(), top, rel, base })
    }

    /// Keep one hunk of `id`'s pending file `rel` (`patch`: the file's header and that hunk, as
    /// `checkpoint::Repo::file_diff` cut it): the baseline takes it in, the rest stays pending.
    pub fn keep_hunk(&mut self, id: &str, rel: &str, patch: String, cx: &mut Context<Self>) {
        let Some(review) = self.reviews.get_mut(id).filter(|r| !r.busy && r.git) else { return };
        let (Some(top), Some(base)) = (review.root.clone(), review.base.clone()) else { return };
        review.busy = true;
        review.run += 1;
        let (tid, rel) = (id.to_string(), rel.to_string());
        let thread = tid.clone();
        let task = cx.spawn(async move |this, cx| {
            let kept = cx
                .background_executor()
                .spawn(async move {
                    let r = Repo::find(&top).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", top.display()))?;
                    let tree = r.keep_hunk(&base, &patch)?;
                    r.pin_review(&thread, Some(&tree))?;
                    Ok::<_, anyhow::Error>(tree)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let Some(review) = ws.reviews.get_mut(&tid) else { return };
                review.busy = false;
                match kept {
                    Ok(tree) => {
                        review.base = Some(tree);
                        ws.save_review(&tid);
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't keep that change to {rel}: {e:#}"), undo: None }),
                }
                ws.review_moved(&tid, cx);
                cx.emit(WorkspaceEvent::ReviewChanged { id: tid.clone() });
                cx.notify();
            });
        });
        if let Some(r) = self.reviews.get_mut(id) {
            r._op = Some(task);
        }
    }

    /// Undo one hunk of `id`'s pending file `rel`: it's taken back out of the file on disk. The
    /// files as they were are kept first, and the toast can put the file back. Refused while a
    /// turn runs, and with a toast when the lines around it changed too much for it to come out
    /// by itself (the file is left as it was).
    pub fn undo_hunk(&mut self, id: &str, rel: &str, patch: String, cx: &mut Context<Self>) {
        if self.turn_running(id) {
            cx.emit(WorkspaceEvent::Toast { message: "Stop the running turn to undo its changes.".into(), undo: None });
            return;
        }
        let Some(review) = self.reviews.get_mut(id).filter(|r| !r.busy && r.git) else { return };
        let Some(top) = review.root.clone() else { return };
        review.busy = true;
        review.run += 1;
        let (tid, rel) = (id.to_string(), rel.to_string());
        let thread = tid.clone();
        let task = cx.spawn(async move |this, cx| {
            let path = rel.clone();
            let undone = cx
                .background_executor()
                .spawn(async move {
                    let r = Repo::find(&top).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", top.display()))?;
                    let undo = r.save_undo(&thread)?;
                    r.undo_hunk(&path, &patch).map(|()| (undo, r.top))
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if let Some(review) = ws.reviews.get_mut(&tid) {
                    review.busy = false;
                }
                match undone {
                    Ok((undo, top)) => {
                        let name = Path::new(&rel).file_name().map_or(rel.clone(), |n| n.to_string_lossy().to_string());
                        let undo = super::UndoAction::Unrestore { thread: tid.clone(), repo: top.clone(), sha: undo, paths: vec![rel.clone()] };
                        cx.emit(WorkspaceEvent::Toast { message: format!("Undid a change to {name}"), undo: Some(undo) });
                        ws.files_epoch += 1;
                        ws.refresh_git_at(top, cx);
                        ws.forget_turn_changes(&tid, false, cx);
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't undo that change to {rel}: {e:#}"), undo: None }),
                }
                ws.review_moved(&tid, cx);
                cx.notify();
            });
        });
        if let Some(r) = self.reviews.get_mut(id) {
            r._op = Some(task);
        }
    }

    /// Show `id`'s pending changes in the editor's Review tab (`ide::diff_view`), which reads
    /// each file's hunks against the baseline itself.
    pub fn open_review(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.reviews.get(id).is_some_and(|r| r.git && !r.pending.is_empty()) {
            cx.emit(WorkspaceEvent::OpenReview { thread: id.to_string() });
        }
    }

    /// The pending files are being worked out (or kept, or undone) now.
    #[cfg(test)]
    pub fn review_settled(&self, id: &str) -> bool {
        self.reviews.get(id).is_none_or(|r| r.counted && !r.busy && !r.settling)
    }
}

/// "notes.md", "notes.md and lib.rs", "3 files": what a toast calls `paths`.
fn files_named(paths: &[String]) -> String {
    let name = |p: &String| Path::new(p).file_name().map_or(p.clone(), |n| n.to_string_lossy().to_string());
    match paths {
        [one] => name(one),
        [a, b] => format!("{} and {}", name(a), name(b)),
        more => format!("{} files", more.len()),
    }
}

/// Edits per file named since item `from`.
fn edit_counts(items: &trek_core::transcript::Transcript, from: usize) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = vec![];
    for item in &items[from.min(items.len())..] {
        let Item::Tool { title, detail, status, .. } = item else { continue };
        if matches!(status, ToolStatus::Failed | ToolStatus::Denied) || (tool_kind(title) != ToolKind::Edit && title != "Delete") {
            continue;
        }
        for p in detail.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
            match out.iter_mut().find(|(e, _)| e == p) {
                Some((_, n)) => *n += 1,
                None => out.push((p.to_string(), 1)),
            }
        }
    }
    out
}

/// `path` (as an agent reported it) relative to `root` when inside it; agents may report `/tmp`
/// as `/private/tmp`.
fn relative(path: &str, root: &Path) -> String {
    let p = Path::new(path);
    let p = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let alt = root.strip_prefix("/").ok().map(|r| Path::new("/private").join(r));
    p.strip_prefix(root).ok().or_else(|| alt.as_deref().and_then(|a| p.strip_prefix(a).ok())).map_or_else(|| path.to_string(), |r| r.display().to_string())
}

/// The pending files: what changed from the baseline (`base`, else the start checkpoint's
/// tree) to the files now, among those the thread touched. Returns the repository's top folder,
/// the baseline, the pending files, and for each file a turn's checkpoints changed, where it stood
/// as the last of them ended. Blocks on git.
fn count(inputs: Inputs, base: Option<String>) -> anyhow::Result<(PathBuf, String, Vec<FileChange>, HashMap<String, String>)> {
    let r = Repo::find(&inputs.cwd).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", inputs.cwd.display()))?;
    let base = match base {
        Some(b) => b,
        None => r.tree_of(inputs.start.as_deref().ok_or_else(|| anyhow::anyhow!("no checkpoint to start from"))?)?,
    };
    let now = r.tree_now()?;
    // What the thread touched: its edit tools' files, and what changed over its turns' spans
    // (from each one's start to its end, the running one's to now; not between them).
    // Paths reported relative to the thread's folder, which may be below the top folder.
    let mut touched: HashSet<String> = inputs.edits.iter().map(|(p, _)| relative(&inputs.cwd.join(p).display().to_string(), &r.top)).collect();
    let mut ends: HashMap<String, String> = HashMap::new();
    for (from, to) in &inputs.spans {
        let to = to.as_deref().unwrap_or(now.as_str());
        for path in r.paths_changed(&[(from.clone(), to.to_string())])? {
            ends.insert(path.clone(), to.to_string());
            touched.insert(path);
        }
    }
    let pending = r
        .diff_stat(&base, &now)?
        .into_iter()
        .filter(|f| touched.contains(&f.path) || matches!(&f.status, FileStatus::Renamed { from } if touched.contains(from)))
        .collect();
    Ok((as_given(&r.top, &inputs.cwd), base, pending, ends))
}

/// The repository's top folder `top` (as git names it: links resolved) in the form the thread's
/// folder `cwd` was given in (`/var/…` rather than `/private/var/…`), so paths built on it match
/// the ones the editor and the Explorer use.
pub fn as_given(top: &Path, cwd: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    match canon.strip_prefix(top) {
        Ok(below) => cwd.ancestors().nth(below.components().count()).map_or_else(|| top.to_path_buf(), Path::to_path_buf),
        Err(_) => top.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::relative;
    use std::path::Path;

    #[test]
    fn reported_paths_are_taken_relative_to_the_folder() {
        assert_eq!(relative("/p/src/a.rs", Path::new("/p")), "src/a.rs");
        assert_eq!(relative("src/a.rs", Path::new("/p")), "src/a.rs");
        assert_eq!(relative("/private/tmp/x/a.rs", Path::new("/tmp/x")), "a.rs");
        assert_eq!(relative("/elsewhere/a.rs", Path::new("/p")), "/elsewhere/a.rs");
    }
}
