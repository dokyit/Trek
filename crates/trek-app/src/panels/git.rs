//! Source control: branch, changed files with stats, a diff viewer, commit and push. For a thread
//! in a worktree it reviews what the thread changed against its base (its commits and what isn't
//! committed yet), and takes the work further: revert a file, commit, push, open a pull request,
//! merge into the base, remove the worktree. It also shows what one turn of a thread changed
//! (`show_turn`, from the card under the turn's answer): between the checkpoints around it.

use crate::palette;
use crate::workspace::{TurnRange, Workspace};
use gpui_kit::base::ScrollbarHandle;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use trek_core::worktree::{self, Change, MergeBlock, Review, Worktree};

#[derive(Clone, Debug)]
pub(crate) struct FileChange {
    pub(crate) path: String,
    pub(crate) status: String,
    pub(crate) additions: i64,
    pub(crate) deletions: i64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub(crate) is_repo: bool,
    pub(crate) branch: String,
    /// (behind, ahead) its upstream.
    pub(crate) upstream: Option<(u32, u32)>,
    pub(crate) files: Vec<FileChange>,
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Add,
    Del,
    Hunk,
    Meta,
    Ctx,
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    // No optional locks: a status refresh mustn't hold index.lock while a switch or commit runs.
    let out = Command::new("git").args(args).current_dir(cwd).env("PATH", trek_core::detect::login_path()).env("GIT_OPTIONAL_LOCKS", "0").output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `git` for questions that change nothing: through `trek_core::git::read_only`, so the folder's
/// own config runs nothing (fsmonitor, hooks) and takes no lock.
pub(crate) fn git_read(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = trek_core::git::read_only(cwd).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub(crate) fn snapshot(cwd: &Path) -> Snapshot {
    if git_read(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Snapshot::default();
    }
    let branch = git_read(cwd, &["branch", "--show-current"]).unwrap_or_default().trim().to_string();
    let upstream = git_read(cwd, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]).ok().and_then(|s| {
        let mut it = s.split_whitespace().filter_map(|n| n.parse::<u32>().ok());
        Some((it.next()?, it.next()?))
    });
    let mut stats = std::collections::HashMap::new();
    let numstat = git_read(cwd, &["diff", "HEAD", "--numstat"]).or_else(|_| git_read(cwd, &["diff", "--cached", "--numstat"])).unwrap_or_default();
    for line in numstat.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() == 3 {
            stats.insert(parts[2].to_string(), (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0)));
        }
    }
    let mut files = Vec::new();
    for line in git_read(cwd, &["status", "--porcelain=v1", "-uall"]).unwrap_or_default().lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[..2].trim().to_string();
        let mut path = line[3..].to_string();
        if let Some((_, to)) = path.split_once(" -> ") {
            path = to.to_string();
        }
        let (additions, deletions) = match stats.get(&path) {
            Some(s) => *s,
            None if status == "??" => (untracked_text(&cwd.join(&path)).map(|s| s.lines().count() as i64).unwrap_or(0), 0),
            None => (0, 0),
        };
        files.push(FileChange { path, status, additions, deletions });
    }
    Snapshot { is_repo: true, branch, upstream, files }
}

/// A worktree's changes in the shape the file list draws (`U`ntracked shows as `??` does).
fn review_files(review: &Review) -> Vec<FileChange> {
    review
        .files
        .iter()
        .map(|c| FileChange { path: c.path.clone(), status: if c.status == 'U' { "??".into() } else { c.status.to_string() }, additions: c.additions, deletions: c.deletions })
        .collect()
}

fn diff_lines(text: &str) -> Vec<(LineKind, String)> {
    text.lines()
        .take(5000)
        .map(|l| {
            let kind = if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") {
                LineKind::Meta
            } else if l.starts_with("@@") {
                LineKind::Hunk
            } else if l.starts_with('+') {
                LineKind::Add
            } else if l.starts_with('-') {
                LineKind::Del
            } else {
                LineKind::Ctx
            };
            (kind, l.to_string())
        })
        .filter(|(k, _)| *k != LineKind::Meta)
        .collect()
}

/// The largest untracked file that's read to count or show its lines: the list is read again
/// whenever the files change, and an agent's log or a dataset left in the folder can run to
/// gigabytes.
const UNTRACKED_READ: u64 = 4 << 20;

/// An untracked file's text, when it's text and no larger than `UNTRACKED_READ`.
fn untracked_text(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    (size <= UNTRACKED_READ).then(|| std::fs::read_to_string(path).ok()).flatten()
}

fn file_diff(cwd: &Path, file: &FileChange) -> Vec<(LineKind, String)> {
    let text = if file.status == "??" {
        match untracked_text(&cwd.join(&file.path)) {
            Some(s) => s.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"),
            None => "Binary, unreadable, or too large to show".into(),
        }
    } else {
        git_read(cwd, &["diff", "HEAD", "--", &file.path]).or_else(|_| git_read(cwd, &["diff", "--", &file.path])).unwrap_or_default()
    };
    diff_lines(&text)
}

/// A pull request's description: the agent's last answer (it usually sums up the work), then the
/// branch's commits.
fn pr_body(summary: Option<&str>, commits: &[String]) -> String {
    let mut out = String::new();
    if let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        let mut end = s.len().min(3000);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&s[..end]);
        if end < s.len() {
            out.push('…');
        }
    }
    if !commits.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("Commits:\n");
        for c in commits {
            out.push_str(&format!("- {c}\n"));
        }
    }
    out.trim_end().to_string()
}

/// How a pull request's link reads: `owner/repo#12` for a GitHub one, else the link itself.
fn pr_label(url: &str) -> String {
    let path = url.trim_end_matches('/').split_once("github.com/").map(|(_, p)| p);
    match path.and_then(|p| p.rsplit_once("/pull/")) {
        Some((repo, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("{repo}#{n}"),
        _ => url.to_string(),
    }
}

/// The thread on screen runs in a worktree: what the panel reviews. (Not its title: a rename
/// mustn't start the review over.)
#[derive(Clone, Debug, PartialEq)]
struct Target {
    thread: String,
    project: PathBuf,
    wt: Worktree,
}

/// One turn's changes, shown in place of the working tree's (`GitPanel::show_turn`).
#[derive(Clone, Debug)]
struct TurnView {
    thread: String,
    range: TurnRange,
    files: Vec<FileChange>,
    /// Where renamed files were, by where they are now.
    renamed: HashMap<String, String>,
}

/// A turn's files in the shape the file list draws, in the order its card lists them.
fn turn_files(changes: &trek_core::changes::TurnChanges) -> (Vec<FileChange>, HashMap<String, String>) {
    use trek_core::changes::FileStatus;
    let mut renamed = HashMap::new();
    let files = crate::changes_card::folders(changes)
        .into_iter()
        .flat_map(|(_, files)| files)
        .map(|f| {
            let status = match &f.status {
                FileStatus::Added => "A",
                FileStatus::Modified => "M",
                FileStatus::Deleted => "D",
                FileStatus::Renamed { from } => {
                    renamed.insert(f.path.clone(), from.clone());
                    "R"
                }
            };
            FileChange { path: f.path.clone(), status: status.into(), additions: f.added as i64, deletions: f.removed as i64 }
        })
        .collect();
    (files, renamed)
}

/// What a worktree thread changed, and what can be done with it.
#[derive(Clone, Debug)]
struct ReviewState {
    review: Review,
    merge_block: Option<MergeBlock>,
    /// `gh` is installed and the project's origin is on GitHub.
    can_pr: bool,
}

pub struct GitPanel {
    workspace: Entity<Workspace>,
    cwd: Option<PathBuf>,
    snap: Snapshot,
    /// Set for a thread in a worktree; `review` then holds what it changed (or why that failed).
    target: Option<Target>,
    review: Option<Result<ReviewState, String>>,
    /// Open pull requests by branch, looked up once each (`None`: none open).
    prs: HashMap<String, Option<String>>,
    loading: bool,
    selected: Option<String>,
    diff: Vec<(LineKind, String)>,
    /// The diff pane's scroll position (both axes); a new selection starts at the top left.
    diff_scroll: UniformListScrollHandle,
    message: Entity<InputState>,
    busy: Option<&'static str>,
    turns_seen: u64,
    /// The thread's worktree is being made (or made again): nothing to review yet.
    preparing: bool,
    /// The editor's Source Control: the IDE folder's changes, whatever thread is on screen.
    ide: bool,
    _subscriptions: Vec<Subscription>,
    _task: Option<Task<()>>,
    /// Reading the selected file's diff; a newer selection replaces it.
    _diff: Option<Task<()>>,
    /// A turn's changes on show instead of the working tree's, until closed or another thread
    /// comes on screen.
    turn: Option<TurnView>,
}

impl GitPanel {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with(workspace, false, window, cx)
    }

    fn with(workspace: Entity<Workspace>, ide: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        let sub = cx.observe(&workspace, |this, ws, cx| {
            let (cwd, turns, target, preparing, thread) = {
                let ws = ws.read(cx);
                // Another branch checked out counts as news too: the changed files are other ones.
                let thread = if this.ide { None } else { ws.current_thread().map(|t| t.id.clone()) };
                (this.cwd_in(ws), ws.turns_finished + ws.files_epoch, this.target(ws), this.preparing(ws), thread)
            };
            if this.turn.as_ref().is_some_and(|t| Some(&t.thread) != thread.as_ref()) {
                this.close_turn(cx);
            }
            if cwd != this.cwd || turns != this.turns_seen || target != this.target || preparing != this.preparing {
                this.turns_seen = turns;
                this.refresh(cx);
            }
        });
        let mut this = Self {
            workspace,
            cwd: None,
            snap: Snapshot::default(),
            target: None,
            review: None,
            prs: HashMap::new(),
            loading: false,
            selected: None,
            diff: vec![],
            diff_scroll: UniformListScrollHandle::new(),
            message,
            busy: None,
            turns_seen: 0,
            preparing: false,
            ide,
            _subscriptions: vec![sub],
            _task: None,
            _diff: None,
            turn: None,
        };
        this.refresh(cx);
        this
    }

    /// The diff pane's scroll offset, for tests.
    #[cfg(test)]
    pub(crate) fn diff_offset(&self) -> Point<Pixels> {
        self.diff_scroll.offset()
    }

    /// Show what the turn ending at `end` (by item id) of `thread` changed, `path`'s diff open
    /// (else the first file's). A turn that wasn't counted from git has no diff to show: the
    /// working tree's changes show, `path` among them if it's there.
    pub fn show_turn(&mut self, thread: String, end: String, path: Option<String>, cx: &mut Context<Self>) {
        let Some((range, changes)) = self.workspace.read(cx).turn_range(&thread, &end) else {
            self.close_turn(cx);
            if let Some(p) = path.filter(|p| self.snap.files.iter().any(|f| &f.path == p)) {
                self.select(p, cx);
            }
            return;
        };
        let (files, renamed) = turn_files(&changes);
        let first = path.filter(|p| files.iter().any(|f| &f.path == p)).or_else(|| files.first().map(|f| f.path.clone()));
        self.turn = Some(TurnView { thread, range, files, renamed });
        self.selected = None;
        self.diff.clear();
        if let Some(p) = first {
            self.select(p, cx);
        }
        cx.notify();
    }

    /// Back to the working tree's changes.
    fn close_turn(&mut self, cx: &mut Context<Self>) {
        if self.turn.take().is_some() {
            self.selected = None;
            self.diff.clear();
            self._diff = None;
            cx.notify();
        }
    }

    /// The files listed: the turn's on show, else the working tree's.
    fn files(&self) -> &[FileChange] {
        match &self.turn {
            Some(t) => &t.files,
            None => &self.snap.files,
        }
    }

    /// The turn on show (its files) and the file selected, for tests.
    #[cfg(test)]
    pub(crate) fn turn_shown(&self) -> Option<(Vec<String>, Option<String>)> {
        self.turn.as_ref().map(|t| (t.files.iter().map(|f| f.path.clone()).collect(), self.selected.clone()))
    }

    fn target_in(ws: &Workspace) -> Option<Target> {
        let t = ws.current_thread()?;
        Some(Target { thread: t.id.clone(), project: ws.project_dir(t)?, wt: t.worktree.clone()? })
    }

    /// The thread on screen is waiting for its worktree to be made.
    fn preparing_in(ws: &Workspace) -> bool {
        ws.current_thread().and_then(|t| ws.live.get(&t.id)).is_some_and(|l| l.preparing)
    }

    /// The folder shown: the one on screen, or in the editor the IDE folder.
    fn cwd_in(&self, ws: &Workspace) -> Option<PathBuf> {
        if self.ide { ws.ide_root.clone() } else { ws.current_cwd() }
    }

    /// The worktree thread to review; the editor shows the folder's working tree instead.
    fn target(&self, ws: &Workspace) -> Option<Target> {
        if self.ide { None } else { Self::target_in(ws) }
    }

    fn preparing(&self, ws: &Workspace) -> bool {
        !self.ide && Self::preparing_in(ws)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (cwd, target, preparing) = {
            let ws = self.workspace.read(cx);
            (self.cwd_in(ws), self.target(ws), self.preparing(ws))
        };
        if target != self.target || preparing != self.preparing {
            self.review = None;
            self.selected = None;
            self.diff.clear();
            self._diff = None;
        }
        self.cwd = cwd;
        self.target = target.clone();
        self.preparing = preparing;
        let Some(cwd) = self.cwd.clone() else {
            self.snap = Snapshot::default();
            cx.notify();
            return;
        };
        if let Some(target) = target {
            // Until the worktree is made there's nothing to look at (and it isn't missing).
            if preparing {
                self.loading = false;
                self._task = None;
                cx.notify();
                return;
            }
            self.loading = true;
            cx.notify();
            self.refresh_review(target, cx);
            return;
        }
        self.loading = true;
        cx.notify();
        self._task = Some(cx.spawn(async move |this, cx| {
            let c = cwd.clone();
            let snap = cx.background_executor().spawn(async move { snapshot(&c) }).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.snap = snap;
                this.files_changed(cx);
            });
        }));
    }

    fn refresh_review(&mut self, target: Target, cx: &mut Context<Self>) {
        let lookup_pr = !self.prs.contains_key(&target.wt.branch);
        self._task = Some(cx.spawn(async move |this, cx| {
            let t = target.clone();
            let state = cx
                .background_executor()
                .spawn(async move {
                    if t.wt.is_missing() {
                        return Err("missing".to_string());
                    }
                    let review = worktree::review(&t.wt).map_err(|e| e.to_string())?;
                    let can_pr = trek_core::detect::which("gh").is_some() && worktree::github_origin(&t.project);
                    Ok(ReviewState { merge_block: worktree::merge_check(&t.project, &t.wt), can_pr, review })
                })
                .await;
            let can_pr = state.as_ref().is_ok_and(|s| s.can_pr);
            let _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&target) {
                    return;
                }
                this.loading = false;
                this.snap = Snapshot { is_repo: true, branch: target.wt.branch.clone(), upstream: None, files: state.as_ref().map(|s| review_files(&s.review)).unwrap_or_default() };
                this.review = Some(state);
                this.files_changed(cx);
            });
            // The open pull request, once per branch (it asks GitHub).
            if can_pr && lookup_pr {
                let wt = target.wt.clone();
                let pr = cx.background_executor().spawn(async move { worktree::find_pr(&wt) }).await;
                let _ = this.update(cx, |this, cx| {
                    this.prs.insert(target.wt.branch.clone(), pr);
                    cx.notify();
                });
            }
        }));
    }

    /// The file list changed: keep the selection if its file is still there.
    fn files_changed(&mut self, cx: &mut Context<Self>) {
        // A turn's files don't change with the working tree's.
        if self.turn.is_some() {
            cx.notify();
            return;
        }
        if self.selected.as_ref().is_some_and(|s| !self.snap.files.iter().any(|f| &f.path == s)) {
            self.selected = None;
            self.diff.clear();
        }
        if let Some(sel) = self.selected.clone() {
            self.select(sel, cx);
        }
        cx.notify();
    }

    fn merge_base(&self) -> Option<String> {
        match &self.review {
            Some(Ok(s)) => Some(s.review.merge_base.clone()),
            _ => None,
        }
    }

    fn change(&self, path: &str) -> Option<Change> {
        match &self.review {
            Some(Ok(s)) => s.review.files.iter().find(|c| c.path == path).cloned(),
            _ => None,
        }
    }

    fn select(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(turn) = self.turn.clone() {
            self.select_in_turn(turn, path, cx);
            return;
        }
        let (Some(cwd), Some(file)) = (self.cwd.clone(), self.snap.files.iter().find(|f| f.path == path).cloned()) else { return };
        // A worktree's diff is against where it left its base: its commits show too.
        let against_base = self.target.clone().zip(self.merge_base()).zip(self.change(&path));
        let target = self.target.clone();
        self.selected = Some(path.clone());
        self.diff_scroll.set_offset(point(px(0.), px(0.)));
        // Only the newest selection's diff lands: an older one still being read is dropped.
        self._diff = Some(cx.spawn(async move |this, cx| {
            let lines = cx
                .background_executor()
                .spawn(async move {
                    match against_base {
                        Some(((t, base), change)) => diff_lines(&worktree::file_diff(&t.wt, &base, &change)),
                        None => file_diff(&cwd, &file),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.selected.as_deref() == Some(path.as_str()) && this.target == target {
                    this.diff = lines;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// `select` for a turn on show: the file's diff between the turn's snapshots.
    fn select_in_turn(&mut self, turn: TurnView, path: String, cx: &mut Context<Self>) {
        if !turn.files.iter().any(|f| f.path == path) {
            return;
        }
        self.selected = Some(path.clone());
        self.diff_scroll.set_offset(point(px(0.), px(0.)));
        let old = turn.renamed.get(&path).cloned();
        let range = turn.range.clone();
        let p = path.clone();
        self._diff = Some(cx.spawn(async move |this, cx| {
            let lines = cx
                .background_executor()
                .spawn(async move {
                    let repo = trek_core::checkpoint::Repo::find(&range.repo).ok_or_else(|| anyhow::anyhow!("not a git repository any more"));
                    match repo.and_then(|r| r.diff_patch(&range.from, &range.to, &p, old.as_deref())) {
                        Ok(text) if text.lines().any(|l| l.starts_with("Binary files ")) => vec![(LineKind::Ctx, "Binary file".to_string())],
                        Ok(text) => diff_lines(&text),
                        Err(e) => vec![(LineKind::Ctx, format!("Couldn't read the diff: {e:#}"))],
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.selected.as_deref() == Some(path.as_str()) && this.turn.as_ref().is_some_and(|t| t.range == turn.range) {
                    this.diff = lines;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn run(&mut self, label: &'static str, steps: Vec<Vec<String>>, window: &mut Window, cx: &mut Context<Self>) {
        self.run_then(label, steps, |_, _, _, _| {}, window, cx);
    }

    /// `run`, then `done` if every step succeeded.
    fn run_then(
        &mut self,
        label: &'static str,
        steps: Vec<Vec<String>>,
        done: impl FnOnce(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cwd) = self.cwd.clone() else { return };
        self.run_op(
            label,
            move || {
                let mut last = String::new();
                for step in steps {
                    let args: Vec<&str> = step.iter().map(String::as_str).collect();
                    last = git(&cwd, &args).map_err(|e| anyhow::anyhow!(e))?;
                }
                Ok(last)
            },
            done,
            window,
            cx,
        );
    }

    /// Run `op` off the UI thread with `label`'s button spinning; say how it went, then
    /// refresh. `done` gets what it returned.
    fn run_op<T: Send + 'static>(
        &mut self,
        label: &'static str,
        op: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = Some(label);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { op() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(v) => {
                        crate::toast::push(window, format!("{label} done"), cx);
                        done(this, v, window, cx);
                    }
                    Err(e) => crate::toast::push(window, crate::toast::Toast::error(format!("{e:#}")), cx),
                }
                this.refresh(cx);
                // The branch chip, the changed count and the IDE's status bar read the same
                // checkout: read it now rather than at the next poll.
                if let Some(cwd) = this.cwd.clone() {
                    this.workspace.update(cx, |ws, cx| ws.refresh_checkout(cwd, cx));
                }
            });
        })
        .detach();
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).value().trim().to_string();
        if msg.is_empty() {
            crate::toast::push(window, "Write a commit message first", cx);
            return;
        }
        // The message goes once the commit is made: if a hook or signing turns it down, it's
        // still there to try again (unless the user has started another meanwhile).
        let sent = msg.clone();
        if let Some(t) = self.target.clone() {
            self.run_op("Commit", move || worktree::commit(&t.wt.path, &msg), move |this, _, window, cx| this.committed(&sent, window, cx), window, cx);
            return;
        }
        let steps = vec![vec!["add".into(), "-A".into()], vec!["commit".into(), "-m".into(), msg]];
        self.run_then("Commit", steps, move |this, _, window, cx| this.committed(&sent, window, cx), window, cx);
    }

    #[cfg(test)]
    pub(crate) fn message_text(&self, cx: &App) -> String {
        self.message.read(cx).value().to_string()
    }

    /// `message` was committed: the box empties, unless the user has started another meanwhile.
    fn committed(&mut self, message: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.message.read(cx).value().trim() == message {
            self.message.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.target.clone() {
            self.run_op("Push", move || worktree::push(&t.wt), |_, _, _, _| {}, window, cx);
            return;
        }
        let step = if self.snap.upstream.is_some() { vec!["push".to_string()] } else { vec!["push".into(), "-u".into(), "origin".into(), "HEAD".into()] };
        self.run("Push", vec![step], window, cx);
    }

    /// Write a commit message from the worktree's diff with a small model.
    fn generate_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.cwd.clone() else { return };
        self.busy = Some("Generate");
        cx.notify();
        let (tx, rx) = async_channel::bounded(1);
        trek_core::runtime().spawn(async move {
            let context = tokio::task::spawn_blocking(move || worktree::commit_context(&dir, 8_000)).await.unwrap_or_default();
            let _ = tx.send(trek_agents::generate_commit_message(&context).await.map_err(|e| format!("{e:#}"))).await;
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv().await.unwrap_or_else(|_| Err("cancelled".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(msg) => this.message.update(cx, |s, cx| s.set_value(msg, window, cx)),
                    Err(e) => crate::toast::push(window, crate::toast::Toast::error(format!("Couldn't write a message: {e}")), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_pr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        let ws = self.workspace.read(cx);
        let summary = ws.last_answer(&t.thread);
        let title = ws.thread(&t.thread).map(|t| t.title.clone()).unwrap_or_default();
        let branch = t.wt.branch.clone();
        self.run_op(
            "Pull request",
            move || {
                let body = pr_body(summary.as_deref(), &worktree::commit_subjects(&t.wt));
                worktree::create_pr(&t.wt, &title, &body)
            },
            move |this, url, _, cx| {
                cx.open_url(&url);
                this.prs.insert(branch, Some(url));
            },
            window,
            cx,
        );
    }

    fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target.clone() else { return };
        self.busy = Some("Merge");
        cx.notify();
        let task = self.workspace.update(cx, |ws, cx| ws.merge_worktree(&t.thread, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(block)) => crate::toast::push(window, block.explain(&t.wt.base), cx),
                    Err(e) => crate::toast::push(window, crate::toast::Toast::error(format!("{e:#}")), cx),
                }
                this.refresh(cx);
                // The branch chip, the changed count and the IDE's status bar read the same
                // checkout: read it now rather than at the next poll.
                if let Some(cwd) = this.cwd.clone() {
                    this.workspace.update(cx, |ws, cx| ws.refresh_checkout(cwd, cx));
                }
            });
        })
        .detach();
    }

    /// Put a file back as it is on the base, after asking.
    fn confirm_revert(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(t), Some(base), Some(change)) = (self.target.clone(), self.merge_base(), self.change(&path)) else { return };
        let me = cx.entity().downgrade();
        let what = match change.status {
            'U' | 'A' => "The thread added it; reverting deletes it.".to_string(),
            'D' => format!("The thread deleted it; reverting brings back the version on {}.", t.wt.base),
            _ => format!("Its changes since {} are undone, committed ones included.", t.wt.base),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (me, t, base, change) = (me.clone(), t.clone(), base.clone(), change.clone());
            alert
                .title(format!("Revert {}?", change.path))
                .description(format!("{what} This can't be undone."))
                .confirm()
                .ok_text("Revert")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (t, base, change) = (t.clone(), base.clone(), change.clone());
                    let _ = me.update(cx, |this, cx| this.run_op("Revert", move || worktree::revert_file(&t.wt, &base, &change), |_, _, _, _| {}, window, cx));
                    true
                })
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let (ahead, behind) = self.snap.upstream.map(|(b, a)| (a, b)).unwrap_or((0, 0));
        // A worktree's base may have moved on since the branch left it: merging then makes a
        // merge commit, and the branch hasn't seen what's new there.
        let base_ahead = match (&self.target, &self.review) {
            (Some(t), Some(Ok(s))) if s.review.behind > 0 => Some((s.review.behind, t.wt.base.clone())),
            _ => None,
        };
        h_flex()
            .px_3()
            .h(px(40.))
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(Icon::new(crate::assets::Lucide::GitBranch).small().text_color(muted))
            .child(div().min_w_0().truncate().font_medium().child(if self.snap.branch.is_empty() { "detached".to_string() } else { self.snap.branch.clone() }))
            .when_some(self.target.as_ref(), |el, t| el.child(div().flex_none().text_xs().text_color(muted).child(format!("off {}", t.wt.base))))
            .when(ahead > 0, |el| el.child(div().text_xs().text_color(muted).child(format!("↑{ahead}"))))
            .when(behind > 0, |el| el.child(div().text_xs().text_color(muted).child(format!("↓{behind}"))))
            .when_some(base_ahead, |el, (n, base)| {
                let tip = format!("{base} has {n} {} this branch doesn't", if n == 1 { "commit" } else { "commits" });
                el.child(
                    div()
                        .id("git-base-ahead")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{base} ↑{n}"))
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)),
                )
            })
            .child(div().flex_1())
            .when(self.loading, |el| el.child(Spinner::new().xsmall().color(muted)))
            .child(crate::ui::icon_button("git-refresh", IconName::RefreshCw, "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
    }

    fn file_list(&self, summary: String, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let total_add: i64 = self.files().iter().map(|f| f.additions).sum();
        let total_del: i64 = self.files().iter().map(|f| f.deletions).sum();
        let selected = self.selected.clone();
        let revertible = self.target.is_some() && self.turn.is_none();
        v_flex()
            .id("git-files")
            .max_h(px(260.))
            .overflow_y_scroll()
            .py_1()
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child(summary)
                    .child(div().flex_1())
                    .child(div().text_color(palette::emerald(cx)).child(format!("+{total_add}")))
                    .child(div().pl_1().text_color(palette::red(cx)).child(format!("−{total_del}"))),
            )
            .children(self.files().iter().map(|f| {
                let color = match f.status.as_str() {
                    "??" | "A" => palette::emerald(cx),
                    "D" => palette::red(cx),
                    _ => palette::amber(cx),
                };
                let letter = if f.status == "??" { "U".to_string() } else { f.status.chars().next().unwrap_or('M').to_string() };
                let path = f.path.clone();
                let is_sel = selected.as_ref() == Some(&f.path);
                let (dir, name) = match f.path.rsplit_once('/') {
                    Some((d, n)) => (format!("{d}/"), n.to_string()),
                    None => (String::new(), f.path.clone()),
                };
                let revert_path = f.path.clone();
                h_flex()
                    .id(SharedString::from(format!("gf-{}", f.path)))
                    .test_support()
                    .group("git-file")
                    .mx_1()
                    .px_2()
                    .h(px(28.))
                    .gap_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_sel, |el| el.bg(theme.list_active))
                    .when(!is_sel, |el| el.hover(|s| s.bg(theme.list_hover)))
                    .child(div().w(px(12.)).text_xs().font_semibold().text_color(color).child(letter))
                    .child(h_flex().flex_1().min_w_0().overflow_hidden().child(div().flex_none().child(name)).child(div().pl_1().truncate().text_xs().text_color(muted).child(dir)))
                    .when(f.additions > 0, |el| el.child(div().text_xs().text_color(palette::emerald(cx)).child(format!("+{}", f.additions))))
                    .when(f.deletions > 0, |el| el.child(div().text_xs().text_color(palette::red(cx)).child(format!("−{}", f.deletions))))
                    .when(revertible, |el| {
                        el.child(
                            div().invisible().group_hover("git-file", |s| s.visible()).child(
                                crate::ui::icon_button(SharedString::from(format!("gf-revert-{}", f.path)), IconName::Undo2, "Revert to the base version")
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.confirm_revert(revert_path.clone(), window, cx)
                                    })),
                            ),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), cx)))
            }))
    }

    fn diff_view(&self, empty: &'static str, cx: &mut Context<Self>) -> AnyElement {
        if self.selected.is_none() {
            return super::empty(if self.files().is_empty() { empty } else { "Select a file to view its diff." }, cx).into_any_element();
        }
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let diff_lines = self.diff.clone();
        let add_bg = palette::emerald(cx).opacity(0.12);
        let del_bg = palette::red(cx).opacity(0.12);
        // The list measures one item for the content width: the longest line, so a sideways
        // swipe can reach past the viewport's edge rather than clip the line there.
        let widest = diff_lines.iter().map(|(_, t)| t.chars().count()).enumerate().max_by_key(|&(_, n)| n).map(|(i, _)| i);
        let list = uniform_list("git-diff", diff_lines.len(), move |range, _, cx| {
            let theme = cx.theme();
            range
                .map(|i| {
                    let (kind, text) = &diff_lines[i];
                    div()
                        .px_3()
                        .h(px(19.))
                        .whitespace_nowrap()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .when(*kind == LineKind::Add, |el| el.bg(add_bg))
                        .when(*kind == LineKind::Del, |el| el.bg(del_bg))
                        .when(*kind == LineKind::Hunk, |el| el.text_color(theme.muted_foreground).bg(theme.muted))
                        .child(text.clone())
                })
                .collect()
        })
        .size_full()
        .track_scroll(&self.diff_scroll)
        .with_width_from_item(widest)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained);
        div()
            .id("git-diff-view")
            .test_support()
            .size_full()
            .child(list)
            .vertical_scrollbar(&self.diff_scroll)
            .horizontal_scrollbar(&self.diff_scroll)
            .into_any_element()
    }

    /// A turn's changes: its files and their diffs, and the way back to the working tree's.
    fn render_turn(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let n = self.files().len();
        let header = h_flex()
            .px_3()
            .h(px(40.))
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .child(Icon::new(crate::assets::Lucide::FileDiff).small().text_color(muted))
            .child(div().min_w_0().truncate().font_medium().child("Changes in this turn"))
            .child(div().flex_1())
            .child(Button::new("git-turn-close").small().ghost().icon(IconName::ArrowLeft).label("Working tree").on_click(cx.listener(|this, _, _, cx| this.close_turn(cx))));
        let summary = if n == 1 { "1 file changed".to_string() } else { format!("{n} files changed") };
        v_flex()
            .id("git-turn")
            .test_support()
            .size_full()
            .child(header)
            .child(self.file_list(summary, cx))
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme.border).child(self.diff_view("This turn changed no files.", cx)))
            .into_any_element()
    }

    /// A worktree thread whose folder is gone.
    fn missing(&self, t: &Target, cx: &mut Context<Self>) -> AnyElement {
        let (ws1, ws2, id1, id2) = (self.workspace.clone(), self.workspace.clone(), t.thread.clone(), t.thread.clone());
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .px_6()
            .gap_3()
            .text_center()
            .child(div().text_sm().font_medium().child("Worktree missing"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("The folder for {} is gone. Bring it back from its branch, or move the thread to the project folder.", t.wt.branch)))
            .child(
                h_flex()
                    .gap_2()
                    .child(Button::new("git-wt-recreate").small().outline().label("Recreate from branch").on_click(move |_, _, cx| {
                        let id = id1.clone();
                        ws1.update(cx, |ws, cx| ws.recreate_worktree(&id, cx))
                    }))
                    .child(Button::new("git-wt-local").small().ghost().label("Run in project folder").on_click(move |_, _, cx| {
                        let id = id2.clone();
                        ws2.update(cx, |ws, cx| ws.run_in_project_folder(&id, cx))
                    })),
            )
            .into_any_element()
    }

    /// Review of a worktree thread: its changes against the base, then commit, push, pull
    /// request, merge and remove.
    fn render_review(&mut self, t: Target, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.preparing {
            return v_flex()
                .id("git-wt-preparing")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new().small().color(theme.muted_foreground))
                .child(div().text_sm().text_color(theme.muted_foreground).child(format!("Making the worktree for {}…", t.wt.branch)))
                .into_any_element();
        }
        let state = match self.review.clone() {
            Some(Err(e)) if e == "missing" => return self.missing(&t, cx),
            Some(Err(e)) => return v_flex().size_full().child(self.header(cx)).child(super::empty(e, cx)).into_any_element(),
            Some(Ok(s)) => Some(s),
            None => None,
        };
        let review = state.as_ref().map(|s| s.review.clone()).unwrap_or_default();
        let mut parts = vec![format!("{} changed", review.files.len())];
        if review.ahead > 0 {
            parts.push(format!("{} {}", review.ahead, if review.ahead == 1 { "commit" } else { "commits" }));
        }
        if review.uncommitted > 0 {
            parts.push(format!("{} uncommitted", review.uncommitted));
        }
        let pr = self.prs.get(&t.wt.branch).cloned().flatten();
        let can_pr = state.as_ref().is_some_and(|s| s.can_pr);
        let block = state.as_ref().and_then(|s| s.merge_block.clone());
        let nothing_to_merge = block.as_ref() == Some(&MergeBlock::NothingToMerge);
        let generate = self.workspace.read(cx).can_write_with_claude();
        let busy = self.busy;
        let base = t.wt.base.clone();
        let (ws, id) = (self.workspace.clone(), t.thread.clone());

        let commit = v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex().gap_1().child(div().id("git-message").test_support().flex_1().child(Input::new(&self.message).small())).when(generate, |el| {
                    el.child(
                        crate::ui::icon_button("git-generate", crate::assets::Lucide::Sparkles, "Write a message from the changes")
                            .loading(busy == Some("Generate"))
                            .disabled(review.uncommitted == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.generate_message(window, cx))),
                    )
                }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("git-commit")
                            .small()
                            .primary()
                            .flex_1()
                            .loading(busy == Some("Commit"))
                            .disabled(review.uncommitted == 0)
                            .label("Commit all")
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    )
                    .child(
                        Button::new("git-push")
                            .small()
                            .outline()
                            .loading(busy == Some("Push"))
                            .disabled(review.ahead == 0)
                            .icon(IconName::ArrowUp)
                            .label(match review.unpushed {
                                Some(n) if n > 0 => format!("Push {n}"),
                                _ => "Push".into(),
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.push(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when(pr.is_none() && can_pr, |el| {
                        el.child(
                            Button::new("git-pr")
                                .small()
                                .outline()
                                .flex_1()
                                .loading(busy == Some("Pull request"))
                                .disabled(review.ahead == 0)
                                .icon(crate::assets::Lucide::GitPullRequestCreate)
                                .label("Create pull request")
                                .on_click(cx.listener(|this, _, window, cx| this.create_pr(window, cx))),
                        )
                    })
                    .child(
                        Button::new("git-merge")
                            .small()
                            .outline()
                            .flex_1()
                            .loading(busy == Some("Merge"))
                            .disabled(nothing_to_merge)
                            .icon(crate::assets::Lucide::GitMerge)
                            .label(format!("Merge into {base}"))
                            .on_click(cx.listener(|this, _, window, cx| this.merge(window, cx))),
                    ),
            )
            // What stands in the way of merging (nothing to say when there's nothing to merge).
            .when_some(block.filter(|b| *b != MergeBlock::NothingToMerge), |el, b| el.child(div().text_xs().text_color(theme.muted_foreground).child(b.explain(&base))))
            // The pull request, as a link that opens it.
            .when_some(pr.clone(), |el, url| {
                let label = pr_label(&url);
                el.child(
                    h_flex()
                        .id("git-pr-link")
                        .test_support()
                        .gap(px(6.))
                        .min_w_0()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(theme.foreground.opacity(0.85))
                        .hover(|s| s.text_color(theme.foreground).underline())
                        .child(Icon::new(crate::assets::Lucide::GitPullRequest).small().text_color(theme.muted_foreground))
                        .child(div().min_w_0().truncate().child(label))
                        .child(Icon::new(IconName::ExternalLink).xsmall().text_color(theme.muted_foreground))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(Button::new("git-reveal").small().ghost().icon(IconName::FolderOpen).label("Show in Finder").on_click({
                        let path = t.wt.path.clone();
                        move |_, _, cx| cx.reveal_path(&path)
                    }))
                    .child(div().flex_1())
                    .child(Button::new("git-remove").small().ghost().icon(crate::assets::Lucide::Trash).label("Remove worktree…").on_click(move |_, window, cx| {
                        crate::worktree_ui::confirm_remove(ws.clone(), id.clone(), window, cx)
                    })),
            );

        v_flex()
            .size_full()
            .child(self.header(cx))
            .child(self.file_list(parts.join(" · "), cx))
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme.border).child(self.diff_view("No changes yet against the base branch.", cx)))
            .child(commit)
            .into_any_element()
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        if self.turn.is_some() {
            return self.render_turn(cx);
        }
        if self.cwd.is_none() {
            return super::empty("Open a project to see its changes.", cx).into_any_element();
        }
        if let Some(t) = self.target.clone() {
            return self.render_review(t, cx);
        }
        if !self.snap.is_repo && !self.loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_sm().text_color(muted).child("This folder isn't a Git repository."))
                .child(Button::new("git-init").small().outline().label("Initialize repository").on_click(cx.listener(|this, _, window, cx| {
                    this.run("Initialize", vec![vec!["init".into()]], window, cx)
                })))
                .into_any_element();
        }
        let header = self.header(cx);
        let files = self.file_list(format!("{} changed", self.snap.files.len()), cx);
        let diff = self.diff_view("No changes. The working tree is clean.", cx);

        let commit = v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(Input::new(&self.message).small())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("git-commit")
                            .small()
                            .primary()
                            .flex_1()
                            .loading(self.busy == Some("Commit"))
                            .disabled(self.snap.files.is_empty())
                            .label("Commit all")
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    )
                    .child(
                        Button::new("git-push")
                            .small()
                            .outline()
                            .loading(self.busy == Some("Push"))
                            .icon(IconName::ArrowUp)
                            .label("Push")
                            .on_click(cx.listener(|this, _, window, cx| this.push(window, cx))),
                    ),
            );

        v_flex()
            .size_full()
            .child(header)
            .child(files)
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme.border).child(diff))
            .child(commit)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{pr_body, pr_label};

    #[test]
    fn pull_request_links_read_as_repo_and_number() {
        assert_eq!(pr_label("https://github.com/dokyit/Trek/pull/42"), "dokyit/Trek#42");
        assert_eq!(pr_label("https://github.com/dokyit/Trek/pull/42/"), "dokyit/Trek#42");
        assert_eq!(pr_label("https://example.com/merge/7"), "https://example.com/merge/7");
    }

    #[test]
    fn pull_requests_describe_the_work_then_the_commits() {
        let commits = vec!["Add a verbose flag".to_string(), "Test it".to_string()];
        assert_eq!(pr_body(Some("  Added `--verbose`.\n"), &commits), "Added `--verbose`.\n\nCommits:\n- Add a verbose flag\n- Test it");
        assert_eq!(pr_body(None, &commits[..1]), "Commits:\n- Add a verbose flag");
        let long = "é".repeat(2000);
        let body = pr_body(Some(&long), &[]);
        assert!(body.ends_with('…') && body.len() <= 3003);
    }
}
