//! File checkpoints: a snapshot of a git working tree taken as a turn starts, so a rewind can put
//! the files back the way they were, and another as it ends, so what the turn changed is exactly
//! what changed between the two.
//!
//! A snapshot is a commit (parent: HEAD, when there is one) of every tracked and untracked file
//! that isn't ignored, kept under `refs/trek/checkpoints/<thread>/<item>`. It's made through a
//! temporary index seeded from the user's, so only files that changed are read again, and the
//! user's index, HEAD, branches and stash are never touched. Restoring writes files back the same
//! way. Everything here blocks on git: run it off the main thread.

use anyhow::{Context as _, Result, bail};
use std::io::Write as _;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Checkpoints kept per thread (two a turn: as it starts and as it ends); older ones are pruned
/// as new ones are taken.
pub const KEEP: usize = 200;

/// Longest a snapshot may take reading the files. The message waits for it, so past this (huge
/// untracked files, a clean filter that hangs) it goes without one.
pub const SNAPSHOT_LIMIT: Duration = Duration::from_secs(10);

/// Longest a restore (or its preview) may spend reading the files. It runs once, on request,
/// and a rewind that gives up leaves the files where they are: it gets far longer than a snapshot.
pub const RESTORE_LIMIT: Duration = Duration::from_secs(120);

/// Untracked files bigger than this are left out of snapshots: a dataset, a video or build output
/// missing from `.gitignore` would otherwise be copied into the user's `.git` on the next turn.
/// Restoring leaves them as they are.
pub const UNTRACKED_MAX: u64 = 8 << 20;

/// Checkpoints of threads archived or settled this long ago are dropped (`prune_stale`), so their
/// refs don't keep big objects alive in the user's repo for good.
pub const STALE_AFTER_MS: i64 = 30 * 86_400_000;

const REF_ROOT: &str = "refs/trek/checkpoints";
/// The line of a checkpoint's message that names its branch.
const BRANCH_TRAILER: &str = "Trek-Branch: ";
const UNDO_ROOT: &str = "refs/trek/undo";
const REVIEW_ROOT: &str = "refs/trek/review";

/// Where the checkpoint taken as `item` was sent in `thread` lives.
pub fn ref_name(thread: &str, item: &str) -> String {
    format!("{REF_ROOT}/{thread}/{item}")
}

/// Where the files as they were just before `thread`'s latest restore are kept, so it can be undone.
/// Where the baseline of `thread`'s open review is pinned: a tree no commit holds, which `git gc`
/// would otherwise drop.
pub fn review_ref(thread: &str) -> String {
    format!("{REVIEW_ROOT}/{thread}")
}

pub fn undo_ref(thread: &str) -> String {
    format!("{UNDO_ROOT}/{thread}")
}

/// What restoring a checkpoint does to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Changed since: its old contents come back.
    Modified,
    /// Created since: it's removed.
    Added,
    /// Deleted since: it comes back.
    Deleted,
    /// A repository inside this one (its own `.git`) that came, went or moved on since, or a file
    /// in one. Restoring leaves it as it is: its history isn't in the checkpoint, and removing it
    /// could lose work.
    Nested,
    /// In the checkpoint and on disk, but out of what a snapshot holds now (ignored since, or
    /// grown past `UNTRACKED_MAX`): undo couldn't bring its new contents back, so restoring
    /// leaves it as it is.
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository's top folder.
    pub path: String,
    pub change: Change,
}

/// What a restore did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored {
    /// Every difference from the checkpoint, nested repositories (left alone) included.
    pub changes: Vec<FileChange>,
    /// Files that couldn't be put back, with why; the rest were.
    pub failed: Vec<(String, String)>,
    /// The files as they were just before (`undo_ref`): restoring it undoes this one. `None`
    /// when nothing needed restoring.
    pub undo: Option<String>,
}

impl Restored {
    /// Files put back (changed, recreated or removed).
    pub fn restored(&self) -> usize {
        let touched = self.changes.iter().filter(|c| !matches!(c.change, Change::Nested | Change::Kept)).count();
        let kept = self.changes.iter().filter(|c| c.change == Change::Kept).count();
        touched.saturating_sub(self.failed.len().saturating_sub(kept))
    }
}

/// The git binary itself. macOS's `/usr/bin/git` is a shim that looks up the developer tools on
/// every call, which costs more than the git command does; a snapshot makes several calls.
fn git_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let exec_path = Command::new("git").arg("--exec-path").stdin(Stdio::null()).output().ok().filter(|o| o.status.success());
        exec_path
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()).join("git"))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("git"))
    })
}

/// An index file of Trek's own, deleted when dropped.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new() -> TempIndex {
        static N: AtomicU64 = AtomicU64::new(0);
        let name = format!("trek-index-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        TempIndex(std::env::temp_dir().join(name))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("lock"));
    }
}

/// A git working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The working tree's top folder.
    pub top: PathBuf,
    /// The user's index (per worktree).
    index: PathBuf,
    /// Longest a snapshot may spend reading the files (`SNAPSHOT_LIMIT`).
    limit: Duration,
}

impl Repo {
    /// The working tree `dir` is in; `None` when it isn't in one (or git is missing).
    pub fn find(dir: &Path) -> Option<Repo> {
        let out = base_command(dir)
            .args(["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-path", "index"])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let top = PathBuf::from(lines.next()?.trim());
        let index = PathBuf::from(lines.next()?.trim());
        (!top.as_os_str().is_empty()).then_some(Repo { top, index, limit: SNAPSHOT_LIMIT })
    }

    /// The working tree `dir` is in, or why git can't open it there (`find` says only `None`):
    /// for a folder that has a `.git`, so the user hears why it gets no checkpoints.
    pub fn open(dir: &Path) -> Result<Repo> {
        if let Some(repo) = Repo::find(dir) {
            return Ok(repo);
        }
        let out = base_command(dir).args(["rev-parse", "--show-toplevel"]).output().context("couldn't run git")?;
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().find(|l| l.starts_with("fatal:") || l.starts_with("error:")).unwrap_or(why.trim()).trim();
        bail!("git can't open {}: {}", dir.display(), if why.is_empty() { "unknown error" } else { why })
    }

    /// The same repository, with snapshots stopped after `limit`.
    pub fn with_limit(self, limit: Duration) -> Repo {
        Repo { limit, ..self }
    }

    fn git(&self, index: Option<&TempIndex>) -> Command {
        let mut c = base_command(&self.top);
        if let Some(i) = index {
            c.env("GIT_INDEX_FILE", &i.0);
        }
        c
    }

    fn run(&self, index: Option<&TempIndex>, args: &[&str], input: Option<&[u8]>) -> Result<String> {
        self.run_raw(index, args, input).map(|out| out.trim_end().to_string())
    }

    /// `run`, its output as git wrote it: a patch's last lines may be blank context, or end in
    /// spaces, and trimming them would make it another patch.
    fn run_raw(&self, index: Option<&TempIndex>, args: &[&str], input: Option<&[u8]>) -> Result<String> {
        let out = self.output(index, args, input, None)?;
        if !out.status.success() {
            bail!("git {} failed: {}", command_name(args), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run git; with `limit`, it's stopped if it runs longer.
    fn output(&self, index: Option<&TempIndex>, args: &[&str], input: Option<&[u8]>, limit: Option<Duration>) -> Result<std::process::Output> {
        let mut c = self.git(index);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn().context("couldn't run git")?;
        // Fed from a thread of its own: git may write more than a pipe holds before it has read
        // all of it, and waiting on the write while nothing drains its output would hang both.
        let feed = match input {
            Some(bytes) => {
                let mut stdin = child.stdin.take().context("git stdin")?;
                let bytes = bytes.to_vec();
                // A git that stops early (it failed) closes its end: that error says nothing new.
                Some(std::thread::spawn(move || _ = stdin.write_all(&bytes)))
            }
            None => None,
        };
        let Some(limit) = limit else {
            let out = child.wait_with_output()?;
            if let Some(f) = feed {
                let _ = f.join();
            }
            return Ok(out);
        };
        // Drain the pipes on threads of their own, so a chatty git can't stall on a full pipe
        // while this one watches the clock.
        let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = vec![];
                if let Some(mut p) = pipe {
                    let _ = p.read_to_end(&mut buf);
                }
                buf
            })
        };
        let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("git {} took longer than {:.1?}", command_name(args), limit);
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        if let Some(f) = feed {
            let _ = f.join();
        }
        Ok(std::process::Output { status, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
    }

    /// The tree the working directory holds right now: tracked and untracked files that aren't
    /// ignored, as `git add -A` sees them. Files git can't take are left out, the same way each
    /// time (a nested repository with no commit yet, an unreadable file), rather than failing it,
    /// and so are untracked files over `UNTRACKED_MAX`.
    pub fn tree_now(&self) -> Result<String> {
        let index = TempIndex::new();
        // Starting from the user's index keeps its stat data: only files that changed are read.
        if self.index.exists() {
            std::fs::copy(&self.index, &index.0).context("copy the index")?;
        }
        let started = Instant::now();
        let big = self.big_untracked(&index)?;
        let limit = Some(self.limit.saturating_sub(started.elapsed()));
        let out = if big.is_empty() {
            self.output(Some(&index), &["add", "-A", "--ignore-errors"], None, limit)?
        } else {
            let spec: String = std::iter::once(".".to_string()).chain(big.iter().map(|p| format!(":(exclude,literal){p}"))).map(|p| p + "\0").collect();
            self.output(Some(&index), &["add", "-A", "--ignore-errors", "--pathspec-from-file=-", "--pathspec-file-nul"], Some(spec.as_bytes()), limit)?
        };
        // 1: some files were left out (see above); anything else is git failing outright.
        if !matches!(out.status.code(), Some(0 | 1)) {
            bail!("git add failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        self.run(Some(&index), &["write-tree"], None)
    }

    /// Untracked files (not ignored) over `UNTRACKED_MAX`, relative to the top folder.
    fn big_untracked(&self, index: &TempIndex) -> Result<Vec<String>> {
        let out = self.output(Some(index), &["ls-files", "-z", "--others", "--exclude-standard"], None, Some(self.limit))?;
        if !out.status.success() {
            bail!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let listed = String::from_utf8_lossy(&out.stdout);
        Ok(listed
            .split('\0')
            .filter(|p| !p.is_empty() && std::fs::symlink_metadata(self.top.join(p)).is_ok_and(|m| m.is_file() && m.len() > UNTRACKED_MAX))
            .map(str::to_string)
            .collect())
    }

    /// The branch checked out, `None` on a detached HEAD.
    fn branch(&self) -> Option<String> {
        self.run(None, &["symbolic-ref", "--short", "-q", "HEAD"], None).ok().filter(|b| !b.is_empty())
    }

    /// The branch checkpoint `sha` was taken on, as its message records it (older ones don't).
    fn branch_of(&self, sha: &str) -> Option<String> {
        let message = self.run(None, &["cat-file", "commit", sha], None).ok()?;
        message.lines().rev().find_map(|l| l.strip_prefix(BRANCH_TRAILER)).map(|b| b.trim().to_string()).filter(|b| !b.is_empty())
    }

    /// Whether checkpoint `from` and `to` (another checkpoint; `None`: the files now) are of the
    /// same branch, as far as they say: a turn's count across a branch switch would list every
    /// file the branches differ in.
    pub fn same_branch(&self, from: &str, to: Option<&str>) -> bool {
        let then = self.branch_of(from);
        let now = match to {
            Some(sha) => self.branch_of(sha),
            None => self.branch(),
        };
        !matches!((then, now), (Some(a), Some(b)) if a != b)
    }

    /// A commit of `tree`, hung on HEAD when there is one. Its message names the branch checked
    /// out, so a restore can tell when another one is now (`restore`).
    fn commit(&self, tree: &str) -> Result<String> {
        let message = match self.branch() {
            Some(b) => format!("Trek checkpoint\n\n{BRANCH_TRAILER}{b}"),
            None => "Trek checkpoint".to_string(),
        };
        let commit = |parent: bool| {
            let mut args = vec!["-c", "commit.gpgSign=false", "commit-tree", tree, "-m", message.as_str()];
            if parent {
                args.extend(["-p", "HEAD"]);
            }
            self.run(None, &args, None)
        };
        // A repository without a commit yet has no HEAD to hang the snapshot on.
        commit(true).or_else(|_| commit(false))
    }

    /// Snapshot the working tree as checkpoint `item` of `thread`; returns its commit.
    pub fn snapshot(&self, thread: &str, item: &str) -> Result<String> {
        let sha = self.commit(&self.tree_now()?)?;
        self.run(None, &["update-ref", &ref_name(thread, item), &sha], None)?;
        Ok(sha)
    }

    /// What restoring checkpoint `sha` would change.
    pub fn changes_since(&self, sha: &str) -> Result<Vec<FileChange>> {
        self.plan(sha, None).map(|(_, changes)| changes)
    }

    /// What restoring just `only` (paths relative to the top folder) as checkpoint `sha` has
    /// them would change: what `restore_in` lists.
    pub fn changes_since_in(&self, sha: &str, only: &HashSet<String>) -> Result<Vec<FileChange>> {
        self.plan(sha, Some(only)).map(|(_, changes)| changes)
    }

    /// The files as they are now (a tree) and what restoring checkpoint `sha` does to each (to
    /// each of `only`, when given).
    fn plan(&self, sha: &str, only: Option<&HashSet<String>>) -> Result<(String, Vec<FileChange>)> {
        // Another branch checked out since: putting this one's files back would rewrite every
        // file the branches differ in, with HEAD left where it is.
        if let (Some(then), Some(now)) = (self.branch_of(sha), self.branch()) {
            if then != now {
                bail!("the checkpoint was taken on {then}, and {now} is checked out now. Switch back to {then} to put its files back");
            }
        }
        let now = self.clone().with_limit(self.limit.max(RESTORE_LIMIT)).tree_now()?;
        let mut changes = self.diff(sha, &now)?;
        if let Some(only) = only {
            changes.retain(|c| only.contains(&c.path));
        }
        // Removed by the restore: what stands in a deleted file's way may be one of these.
        let removed: HashSet<String> = changes.iter().filter(|c| c.change == Change::Added).map(|c| c.path.clone()).collect();
        for c in changes.iter_mut().filter(|c| c.change == Change::Deleted) {
            // On disk, yet not in the snapshot: ignored or too big now. Or a file the snapshot
            // doesn't hold stands where its folder was (an ignored `build` where `build/x` was):
            // writing it back would delete that file, and the undo couldn't bring it back.
            let path = Path::new(&c.path);
            let blocked = path.ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()).any(|a| {
                std::fs::symlink_metadata(self.top.join(a)).is_ok_and(|m| !m.is_dir()) && !removed.contains(a.to_string_lossy().as_ref())
            });
            if blocked || std::fs::symlink_metadata(self.top.join(path)).is_ok() {
                c.change = Change::Kept;
            }
        }
        Ok((now, changes))
    }

    /// `to` against `from`, as what restoring `from` does to each file.
    fn diff(&self, from: &str, to: &str) -> Result<Vec<FileChange>> {
        // Raw output, for the modes: a nested repository is an entry of mode 160000.
        let out = self.run(None, &["diff-tree", "-r", "-z", "--no-renames", "--raw", from, to], None)?;
        let mut fields = out.split('\0').filter(|f| !f.is_empty());
        let mut changes = vec![];
        while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
            // ":<old mode> <new mode> <old sha> <new sha> <status>"
            let meta: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
            let nested = meta.iter().take(2).any(|m| *m == "160000");
            let change = match meta.get(4).and_then(|s| s.chars().next()) {
                _ if nested => Change::Nested,
                Some('A') => Change::Added,
                Some('D') => Change::Deleted,
                _ => Change::Modified,
            };
            changes.push(FileChange { path: path.to_string(), change });
        }
        // Nothing inside a nested repository is touched either (one the turn turned into a plain
        // folder, say).
        let nested: Vec<String> = changes.iter().filter(|c| c.change == Change::Nested).map(|c| format!("{}/", c.path)).collect();
        for c in changes.iter_mut().filter(|c| nested.iter().any(|n| c.path.starts_with(n.as_str()))) {
            c.change = Change::Nested;
        }
        Ok(changes)
    }

    /// What changed from checkpoint `from` to `to` (a later checkpoint, or `tree_now`): each file
    /// with how it changed and the lines added and removed, renames found. Nested repositories
    /// are left out (their history isn't in the checkpoints).
    pub fn diff_stat(&self, from: &str, to: &str) -> Result<Vec<crate::changes::FileChange>> {
        let out = self.run(None, &["diff-tree", "-r", "-z", "-M", "--raw", "--numstat", from, to], None)?;
        Ok(crate::changes::parse_diff_stat(&out))
    }

    /// The patch of one file from `from` to `to` (as `diff_stat` takes them); `old` is where a
    /// renamed file was, so the rename shows as one.
    pub fn diff_patch(&self, from: &str, to: &str, path: &str, old: Option<&str>) -> Result<String> {
        let mut args = vec!["diff-tree", "-p", "-M", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", from, to, "--"];
        args.extend(old);
        args.push(path);
        self.run(None, &args, None)
    }

    /// Put the working tree back as checkpoint `sha` had it: files changed since get their old
    /// contents, deleted ones come back, and ones created since are removed (ignored files and
    /// nested repositories are left alone). The files as they were just before are kept first
    /// (`Restored::undo`, under `undo_ref(thread)`). A file that can't be put back doesn't stop
    /// the rest; it's listed in `Restored::failed`.
    pub fn restore(&self, sha: &str, thread: &str) -> Result<Restored> {
        self.restore_plan(sha, thread, None)
    }

    /// `restore`, of just `only` (paths relative to the top folder): every other file is left as
    /// it is. The undo keeps every file as it was, but restoring it should be limited the same way.
    pub fn restore_in(&self, sha: &str, thread: &str, only: &HashSet<String>) -> Result<Restored> {
        self.restore_plan(sha, thread, Some(only))
    }

    fn restore_plan(&self, sha: &str, thread: &str, only: Option<&HashSet<String>>) -> Result<Restored> {
        let (now, changes) = self.plan(sha, only)?;
        // The undo couldn't hold these: writing the old version over one would lose the new one
        // for good.
        let mut failed: Vec<(String, String)> = changes
            .iter()
            .filter(|c| c.change == Change::Kept)
            .map(|c| {
                let why = if std::fs::symlink_metadata(self.top.join(&c.path)).is_ok() {
                    "it's ignored or too big for checkpoints now, so it was left as it is"
                } else {
                    "a file checkpoints don't hold stands where its folder was, so it was left as it is"
                };
                (c.path.clone(), why.to_string())
            })
            .collect();
        if changes.iter().all(|c| c.change == Change::Nested) {
            return Ok(Restored { changes, ..Restored::default() });
        }
        if changes.iter().all(|c| matches!(c.change, Change::Nested | Change::Kept)) {
            return Ok(Restored { changes, failed, undo: None });
        }
        let undo = self.commit(&now)?;
        self.run(None, &["update-ref", &undo_ref(thread), &undo], None)?;
        let touch = |c: &&FileChange| !matches!(c.change, Change::Nested | Change::Kept);
        // Removals first: a new file may stand where the checkpoint has a folder, or the reverse.
        for c in changes.iter().filter(touch).filter(|c| c.change == Change::Added) {
            let path = self.top.join(&c.path);
            match std::fs::remove_file(&path) {
                Ok(()) => self.prune_empty_dirs(&path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failed.push((c.path.clone(), e.to_string())),
            }
        }
        let back: Vec<&str> = changes.iter().filter(touch).filter(|c| c.change != Change::Added).map(|c| c.path.as_str()).collect();
        if !back.is_empty() {
            let index = TempIndex::new();
            self.run(Some(&index), &["read-tree", sha], None)?;
            let mut input = back.join("\0").into_bytes();
            input.push(0);
            let out = self.output(Some(&index), &["checkout-index", "-f", "-z", "--stdin"], Some(&input), Some(RESTORE_LIMIT))?;
            if !out.status.success() {
                // It writes what it can and names the rest, one line each: "error: unable to
                // create file <path>: <why>", "error: unable to unlink old '<path>': <why>".
                let err = String::from_utf8_lossy(&out.stderr);
                let named = checkout_failures(&err, &back);
                if named.is_empty() {
                    failed.push((String::new(), err.trim().to_string()));
                }
                failed.extend(named);
            }
        }
        Ok(Restored { changes, failed, undo: Some(undo) })
    }

    /// Put just `paths` (relative to the top folder) back as `sha` (a checkpoint, or a tree) has
    /// them: their old contents, or removed where `sha` has none. Everything else is left as it
    /// is, and nothing is kept to undo it (`save_undo` first does). A path that can't be put back (a nested repository,
    /// a file that won't be written) doesn't stop the rest: it's returned, with why.
    pub fn restore_paths(&self, sha: &str, paths: &[&str]) -> Result<Vec<(String, String)>> {
        if paths.is_empty() {
            return Ok(vec![]);
        }
        let listed = self.ls_tree(sha, paths)?;
        let mut failed = vec![];
        // Removals first, as `restore` does: a new file may stand where the old one's folder goes.
        for p in paths.iter().filter(|p| !listed.contains_key(**p)) {
            let path = self.top.join(p);
            match std::fs::remove_file(&path) {
                Ok(()) => self.prune_empty_dirs(&path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failed.push((p.to_string(), e.to_string())),
            }
        }
        let mut back = vec![];
        for p in paths.iter().copied().filter(|p| listed.contains_key(*p)) {
            match listed.get(p) {
                Some((mode, _)) if mode == "160000" => failed.push((p.to_string(), "it's a repository of its own, so it was left as it is".into())),
                _ => back.push(p),
            }
        }
        if !back.is_empty() {
            let index = TempIndex::new();
            self.run(Some(&index), &["read-tree", sha], None)?;
            let mut input = back.join("\0").into_bytes();
            input.push(0);
            let out = self.output(Some(&index), &["checkout-index", "-f", "-z", "--stdin"], Some(&input), Some(RESTORE_LIMIT))?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                let named = checkout_failures(&err, &back);
                if named.is_empty() {
                    failed.push((String::new(), err.trim().to_string()));
                }
                failed.extend(named);
            }
        }
        Ok(failed)
    }

    /// The tree a checkpoint (or a tree) is.
    pub fn tree_of(&self, rev: &str) -> Result<String> {
        self.run(None, &["rev-parse", "--verify", "-q", &format!("{rev}^{{tree}}")], None)
    }

    /// Every path that differs between the two ends of any of `spans` (checkpoints or trees),
    /// relative to the top folder; a renamed file as both its paths. What a turn changed is the
    /// files its spans differ in, and nothing else.
    pub fn paths_changed(&self, spans: &[(String, String)]) -> Result<HashSet<String>> {
        let mut out = HashSet::new();
        for (from, to) in spans {
            let listed = self.run_raw(None, &["diff-tree", "-r", "-z", "--no-renames", "--name-only", from, to], None)?;
            out.extend(listed.split('\0').filter(|p| !p.is_empty()).map(str::to_string));
        }
        Ok(out)
    }

    /// Keep the files as they are now under `undo_ref(thread)`, ahead of putting some back by
    /// other means (`restore_paths`, `undo_hunk`); returns the commit, for `restore_in`.
    pub fn save_undo(&self, thread: &str) -> Result<String> {
        let now = self.clone().with_limit(self.limit.max(RESTORE_LIMIT)).tree_now()?;
        let undo = self.commit(&now)?;
        self.run(None, &["update-ref", &undo_ref(thread), &undo], None)?;
        Ok(undo)
    }

    /// What `rev` (a checkpoint or a tree) holds at each of `paths`: (mode, object), or `None`
    /// where it has nothing. Two revisions hold a file the same when these match.
    pub fn entries(&self, rev: &str, paths: &[&str]) -> Result<Vec<Option<(String, String)>>> {
        if paths.is_empty() {
            return Ok(vec![]);
        }
        let listed = self.ls_tree(rev, paths)?;
        Ok(paths.iter().map(|p| listed.get(*p).cloned()).collect())
    }

    /// A tree like `base` with `paths` as `from` has them: `from`'s contents, or gone where `from`
    /// has none. Both may be checkpoints or trees. A review's baseline takes the files the user
    /// kept this way.
    pub fn tree_with(&self, base: &str, from: &str, paths: &[&str]) -> Result<String> {
        let index = TempIndex::new();
        self.run(Some(&index), &["read-tree", base], None)?;
        if paths.is_empty() {
            return self.run(Some(&index), &["write-tree"], None);
        }
        let listed = self.ls_tree(from, paths)?;
        // Mode 0 takes an entry out of the index; its object name is ignored but must be one.
        let none = "0".repeat(self.tree_of(base).map_or(40, |t| t.len()));
        let mut info = String::new();
        for p in paths {
            match listed.get(*p) {
                Some((mode, sha)) => info.push_str(&format!("{mode} {sha}\t{p}\0")),
                None => info.push_str(&format!("0 {none}\t{p}\0")),
            }
        }
        self.run(Some(&index), &["update-index", "-z", "--index-info"], Some(info.as_bytes()))?;
        self.run(Some(&index), &["write-tree"], None)
    }

    /// How `path` (relative to the top folder) differs between `base` (a checkpoint or a tree)
    /// and the file on disk now, cut into hunks. A file `base` hasn't is all new; one gone from
    /// disk is all deleted. The file as it is now is written to the object store, so a hunk's
    /// undo can fall back on a three-way merge (`undo_hunk`).
    pub fn file_diff(&self, base: &str, path: &str) -> Result<crate::hunks::FileDiff> {
        let index = TempIndex::new();
        self.run(Some(&index), &["read-tree", base], None)?;
        let on_disk = std::fs::symlink_metadata(self.top.join(path)).is_ok_and(|m| m.is_file());
        if on_disk {
            self.run(None, &["--literal-pathspecs", "hash-object", "-w", "--", path], None)?;
            if !self.ls_tree(base, &[path])?.contains_key(path) {
                // Not in the baseline: an intent-to-add entry makes it show as a new file.
                self.run(Some(&index), &["--literal-pathspecs", "add", "-N", "-f", "--", path], None)?;
            }
        }
        // `a/` and `b/` whatever the user's config says (`diff.noprefix`): `git apply` takes the
        // first part of the path off. Untrimmed: a hunk may end in blank lines, or in spaces.
        let out = self.run_raw(
            Some(&index),
            &["--literal-pathspecs", "diff", "--no-color", "--no-ext-diff", "--no-renames", "--src-prefix=a/", "--dst-prefix=b/", "-U3", "--", path],
            None,
        )?;
        Ok(crate::hunks::FileDiff::parse(&out))
    }

    /// Take one hunk of a review back out of the file on disk: `patch` (`FileDiff::patch`, from
    /// `file_diff`) is applied in reverse. When the lines around it moved since, a three-way
    /// merge is tried; when that doesn't go cleanly either, the file is left exactly as it was
    /// and the error says so.
    pub fn undo_hunk(&self, path: &str, patch: &str) -> Result<()> {
        let apply = |index: Option<&TempIndex>, extra: &[&str]| {
            let mut args = vec!["apply", "-R", "--recount", "--whitespace=nowarn"];
            args.extend(extra);
            args.push("-");
            self.output(index, &args, Some(patch.as_bytes()), None)
        };
        let out = apply(None, &[])?;
        if out.status.success() {
            return Ok(());
        }
        let first = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // A three-way merge needs the file in an index: Trek's own, so the user's isn't touched.
        let file = self.top.join(path);
        let Ok(before) = std::fs::read(&file) else { bail!("the hunk doesn't apply any more: {first}") };
        let index = TempIndex::new();
        let merged = (|| -> Result<bool> {
            let blob = self.run(None, &["--literal-pathspecs", "hash-object", "-w", "--", path], None)?;
            let mode = if std::fs::metadata(&file).is_ok_and(|m| { use std::os::unix::fs::PermissionsExt as _; m.permissions().mode() & 0o111 != 0 }) { "100755" } else { "100644" };
            self.run(Some(&index), &["update-index", "--add", "--cacheinfo", &format!("{mode},{blob},{path}")], None)?;
            // Stat data for the entry, so the file reads as matching it.
            self.output(Some(&index), &["update-index", "-q", "--refresh"], None, None)?;
            let out = apply(Some(&index), &["--3way"])?;
            let text = std::fs::read(&file).unwrap_or_default();
            Ok(out.status.success() && !String::from_utf8_lossy(&text).lines().any(|l| l.starts_with("<<<<<<< ")))
        })();
        if merged.as_ref().is_ok_and(|ok| *ok) {
            return Ok(());
        }
        // Not cleanly: what was there goes back, conflict markers and all gone.
        std::fs::write(&file, &before).context("put the file back")?;
        bail!("the lines around it changed since, so it can't be taken out by itself ({first})")
    }

    /// A tree like `base` with one hunk of `path` (`patch`, as `file_diff` cut it) taken in: a
    /// review keeps a hunk by folding it into its baseline.
    pub fn keep_hunk(&self, base: &str, patch: &str) -> Result<String> {
        let index = TempIndex::new();
        self.run(Some(&index), &["read-tree", base], None)?;
        let out = self.output(Some(&index), &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"], Some(patch.as_bytes()), None)?;
        if !out.status.success() {
            bail!("the hunk doesn't fit the baseline: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        self.run(Some(&index), &["write-tree"], None)
    }

    /// The entries `tree` has for `paths` (files, taken literally): path → (mode, object).
    fn ls_tree(&self, tree: &str, paths: &[&str]) -> Result<std::collections::HashMap<String, (String, String)>> {
        let mut args = vec!["--literal-pathspecs", "ls-tree", "-r", "-z", tree, "--"];
        args.extend(paths);
        let out = self.run(None, &args, None)?;
        Ok(out
            .split('\0')
            .filter_map(|entry| {
                // "<mode> <type> <object>\t<path>"
                let (meta, path) = entry.split_once('\t')?;
                let mut meta = meta.split(' ');
                let (mode, _, sha) = (meta.next()?, meta.next()?, meta.next()?);
                Some((path.to_string(), (mode.to_string(), sha.to_string())))
            })
            .collect())
    }

    /// Remove the folders above `path` that are empty now, up to the top folder.
    fn prune_empty_dirs(&self, path: &Path) {
        let mut dir = path.parent();
        while let Some(d) = dir.filter(|d| d.starts_with(&self.top) && *d != self.top) {
            if std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }

    /// Point checkpoints of `thread` at commits that already exist (a fork's copies of another
    /// thread's checkpoints): `(item, commit)`.
    pub fn link(&self, thread: &str, checkpoints: &[(String, String)]) -> Result<()> {
        let script: String = checkpoints.iter().map(|(item, sha)| format!("update {} {sha}\n", ref_name(thread, item))).collect();
        if script.is_empty() {
            return Ok(());
        }
        self.run(None, &["update-ref", "--stdin"], Some(script.as_bytes())).map(|_| ())
    }

    /// Drop checkpoints of `thread` by item.
    pub fn delete(&self, thread: &str, items: &[String]) -> Result<()> {
        let script: String = items.iter().map(|item| format!("delete {}\n", ref_name(thread, item))).collect();
        if script.is_empty() {
            return Ok(());
        }
        self.run(None, &["update-ref", "--stdin"], Some(script.as_bytes())).map(|_| ())
    }

    /// Drop every checkpoint of `thread` (it was deleted), and what would undo its last restore;
    /// returns how many checkpoints went.
    pub fn delete_all(&self, thread: &str) -> Result<usize> {
        let refs = self.run(None, &["for-each-ref", "--format=%(refname)", &format!("{REF_ROOT}/{thread}/"), &undo_ref(thread), &review_ref(thread)], None)?;
        let refs: Vec<&str> = refs.lines().filter(|r| !r.is_empty()).collect();
        let script: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
        if !script.is_empty() {
            self.run(None, &["update-ref", "--stdin"], Some(script.as_bytes()))?;
        }
        Ok(refs.iter().filter(|r| r.starts_with(REF_ROOT)).count())
    }

    /// Keep `tree`, the baseline of `thread`'s review, from `git gc` (`None`: the review closed).
    pub fn pin_review(&self, thread: &str, tree: Option<&str>) -> Result<()> {
        match tree {
            Some(t) => self.run(None, &["update-ref", &review_ref(thread), t], None).map(|_| ()),
            None => self.run(None, &["update-ref", "-d", &review_ref(thread)], None).map(|_| ()),
        }
    }

    /// Checkpoint items of `thread` that have a ref.
    pub fn items(&self, thread: &str) -> Result<Vec<String>> {
        let prefix = format!("{REF_ROOT}/{thread}/");
        let refs = self.run(None, &["for-each-ref", "--format=%(refname)", &prefix], None)?;
        Ok(refs.lines().filter_map(|r| r.strip_prefix(&prefix)).map(str::to_string).collect())
    }
}

/// Drop the checkpoints of threads archived or settled more than `STALE_AFTER_MS` before `now`,
/// and of threads that are gone: refs and records. Returns how many threads lost theirs. Blocks
/// on git.
pub fn prune_stale(store: &crate::store::Store, now: i64) -> Result<usize> {
    let stale = store.stale_checkpoints(now - STALE_AFTER_MS)?;
    for (thread, checkpoints) in &stale {
        let mut repos: Vec<&Path> = checkpoints.iter().map(|c| c.repo.as_path()).collect();
        repos.sort();
        repos.dedup();
        // One repository failing (a lock held) mustn't stop the others; its records stay, to be
        // tried again next time.
        let mut cleared = true;
        for repo in repos {
            if let Some(r) = refs_repo(store, thread, repo) {
                if let Err(e) = r.delete_all(thread) {
                    tracing::warn!("drop checkpoints of {thread} in {}: {e:#}", repo.display());
                    cleared = false;
                }
            }
        }
        if cleared {
            store.delete_checkpoints(thread, &checkpoints.iter().map(|c| c.item_id.clone()).collect::<Vec<_>>())?;
        }
    }
    Ok(stale.len())
}

/// Where `thread`'s checkpoint refs taken in `repo` are: that working tree, else (it's gone: a
/// removed worktree, a moved folder) the thread's project, whose repository its worktrees share.
pub fn refs_repo(store: &crate::store::Store, thread: &str, repo: &Path) -> Option<Repo> {
    Repo::find(repo).or_else(|| {
        let project = store.thread(thread).ok().flatten()?.project_id?;
        let path = store.projects().ok()?.into_iter().find(|p| p.id == project)?.path;
        Repo::find(&path)
    })
}

/// The paths of `tried` that `checkout-index` names in `stderr` as not written, with why.
fn checkout_failures(stderr: &str, tried: &[&str]) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in stderr.lines() {
        let rest = line.trim().trim_start_matches("error:").trim();
        for lead in ["unable to create file ", "unable to unlink old '", "unable to create symlink ", "unable to stat just-written file "] {
            let Some(tail) = rest.strip_prefix(lead) else { continue };
            // The path is followed by ": <why>" (or "': <why>"); it may hold ": " itself, so the
            // longest tried path that fits wins.
            if let Some(path) = tried.iter().filter(|p| tail.starts_with(**p) && tail[p.len()..].trim_start_matches('\'').starts_with(':')).max_by_key(|p| p.len()) {
                let why = tail[path.len()..].trim_start_matches('\'').trim_start_matches(':').trim();
                if !out.iter().any(|(p, _): &(String, String)| p == path) {
                    out.push((path.to_string(), why.to_string()));
                }
            }
        }
    }
    out
}

/// The git command in `args` (`add`, `update-ref`), for messages.
fn command_name<'a>(args: &[&'a str]) -> &'a str {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            // `-c name=value` takes the next argument.
            "-c" => _ = it.next(),
            a if a.starts_with('-') => {}
            a => return a,
        }
    }
    "?"
}

/// git in `dir`, never reading from the terminal, never taking optional locks (the user's own git
/// commands in that repo mustn't find it locked), and deaf to git variables Trek may have
/// inherited.
fn base_command(dir: &Path) -> Command {
    let mut c = Command::new(git_bin());
    c.current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_AUTHOR_NAME", "Trek")
        .env("GIT_AUTHOR_EMAIL", "trek@localhost")
        .env("GIT_COMMITTER_NAME", "Trek")
        .env("GIT_COMMITTER_EMAIL", "trek@localhost")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        // A split index would leave a new `sharedindex.*` in `.git` for every temporary index.
        .args(["-c", "core.splitIndex=false"])
        // Patches Trek builds go back through `git apply`: the paths in them keep their `a/` and
        // `b/`, and blank context lines their space.
        .args(["-c", "diff.noprefix=false", "-c", "diff.mnemonicPrefix=false", "-c", "diff.suppressBlankEmpty=false"])
        .stdin(Stdio::null());
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repository with one commit: `a.txt`, `bin.dat` (binary), `.gitignore` (ignores
    /// `*.log`), `keep/old.txt`.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(commit: bool) -> Scratch {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("trek-checkpoint-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("keep")).unwrap();
            let s = Scratch(dir);
            s.git(&["init", "-q", "-b", "main"]);
            s.write("a.txt", "one\n");
            std::fs::write(s.0.join("bin.dat"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
            s.write(".gitignore", "*.log\n");
            s.write("keep/old.txt", "old\n");
            if commit {
                s.git(&["add", "-A"]);
                s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "init"]);
            }
            s
        }

        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git").args(args).current_dir(&self.0).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        fn write(&self, path: &str, text: &str) {
            let p = self.0.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }

        fn read(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(self.0.join(path)).ok()
        }

        fn repo(&self) -> Repo {
            Repo::find(&self.0).expect("a repo")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sorted(mut c: Vec<FileChange>) -> Vec<(String, Change)> {
        c.sort_by(|a, b| a.path.cmp(&b.path));
        c.into_iter().map(|c| (c.path, c.change)).collect()
    }

    #[test]
    fn a_file_ignored_since_keeps_its_new_contents() {
        let s = Scratch::new(true);
        s.write("notes/todo.md", "old\n");
        let repo = s.repo();
        let sha = repo.snapshot("t1", "u1").unwrap();
        // Ignored after the checkpoint and edited: the undo can't hold it, so a restore that
        // wrote "old" over it would lose this for good.
        s.write(".gitignore", "*.log\nnotes/\n");
        s.write("notes/todo.md", "NEW WORK\n");
        let preview = repo.changes_since(&sha).unwrap();
        assert!(preview.iter().any(|c| c.path == "notes/todo.md" && c.change == Change::Kept), "{preview:?}");
        let restored = repo.restore(&sha, "t1").unwrap();
        assert_eq!(s.read("notes/todo.md").as_deref(), Some("NEW WORK\n"));
        assert_eq!(s.read(".gitignore").as_deref(), Some("*.log\n"), "the rest is put back");
        assert!(restored.failed.iter().any(|(p, _)| p == "notes/todo.md"), "{:?}", restored.failed);
        assert_eq!(restored.restored(), 1);
    }

    #[test]
    fn a_checkpoint_of_another_branch_is_not_restored() {
        let s = Scratch::new(true);
        let repo = s.repo();
        s.write("a.txt", "on main\n");
        let sha = repo.snapshot("t1", "u1").unwrap();
        s.git(&["switch", "-qc", "feature"]);
        s.write("a.txt", "on feature\n");
        let err = repo.restore(&sha, "t1").unwrap_err().to_string();
        assert!(err.contains("taken on main") && err.contains("feature is checked out"), "{err}");
        assert!(repo.changes_since(&sha).is_err(), "the preview says so too");
        assert_eq!(s.read("a.txt").as_deref(), Some("on feature\n"), "nothing was touched");
        // Back on main it restores as ever.
        s.write("a.txt", "one\n");
        s.git(&["switch", "-q", "main"]);
        repo.restore(&sha, "t1").unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("on main\n"));
    }

    #[test]
    fn checkpoints_know_the_branch_they_were_taken_on() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let a = repo.snapshot("t1", "u1").unwrap();
        let b = repo.snapshot("t1", "u2").unwrap();
        assert!(repo.same_branch(&a, Some(&b)) && repo.same_branch(&a, None));
        s.git(&["switch", "-qc", "feature"]);
        assert!(!repo.same_branch(&a, None), "a turn counted across the switch would list the branch's files");
        let c = repo.snapshot("t1", "u3").unwrap();
        assert!(!repo.same_branch(&a, Some(&c)));
        // Older checkpoints don't say: they count as ever.
        let old = s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit-tree", &format!("{a}^{{tree}}"), "-m", "Trek checkpoint"]);
        assert!(repo.same_branch(&old, None));
    }

    #[test]
    fn a_review_baseline_outlives_gc_until_unpinned() {
        let s = Scratch::new(true);
        let repo = s.repo();
        s.write("loose.txt", "only in this tree\n");
        let tree = repo.tree_now().unwrap();
        repo.pin_review("t1", Some(&tree)).unwrap();
        s.git(&["-c", "gc.pruneExpire=now", "gc", "-q", "--prune=now"]);
        assert_eq!(s.git(&["cat-file", "-t", &tree]), "tree", "kept by its ref");
        assert_eq!(repo.delete_all("t1").unwrap(), 0);
        assert!(s.git(&["for-each-ref", &review_ref("t1")]).is_empty(), "a deleted thread's pin goes too");
        repo.pin_review("t1", Some(&tree)).unwrap();
        repo.pin_review("t1", None).unwrap();
        assert!(s.git(&["for-each-ref", &review_ref("t1")]).is_empty());
    }

    #[test]
    fn checkout_failures_name_only_the_files_git_named() {
        let err = "error: unable to create file ro/a.txt.bak: Permission denied\nerror: unable to unlink old 'x: y.txt': Busy\n";
        let tried = ["a.txt", "ro/a.txt.bak", "x: y.txt", "x"];
        assert_eq!(
            checkout_failures(err, &tried),
            vec![("ro/a.txt.bak".to_string(), "Permission denied".to_string()), ("x: y.txt".to_string(), "Busy".to_string())]
        );
    }

    #[test]
    fn a_folder_git_cannot_open_says_why() {
        let s = Scratch::new(true);
        s.git(&["config", "core.repositoryformatversion", "1"]);
        s.git(&["config", "extensions.trekNoSuchThing", "true"]);
        assert!(Repo::find(&s.0).is_none());
        let err = Repo::open(&s.0).unwrap_err().to_string();
        assert!(err.contains("git can't open"), "{err}");
    }

    #[test]
    fn big_untracked_files_are_left_out() {
        let s = Scratch::new(true);
        s.write("notes.txt", "small\n");
        let big = std::fs::File::create(s.0.join("data set.bin")).unwrap();
        big.set_len(UNTRACKED_MAX + 1).unwrap();
        let repo = s.repo();
        let sha = repo.snapshot("t1", "u1").unwrap();
        let files = s.git(&["ls-tree", "-r", "--name-only", &sha]);
        assert!(files.lines().any(|f| f == "notes.txt"));
        assert!(!files.lines().any(|f| f == "data set.bin"), "{files}");
        // Restoring leaves it be, whatever became of it.
        std::fs::remove_file(s.0.join("notes.txt")).unwrap();
        let restored = repo.restore(&sha, "t1").unwrap();
        assert_eq!(sorted(restored.changes), [("notes.txt".to_string(), Change::Deleted)]);
        assert_eq!(std::fs::metadata(s.0.join("data set.bin")).unwrap().len(), UNTRACKED_MAX + 1);
    }

    #[test]
    fn checkpoints_of_threads_long_put_away_are_dropped() {
        use crate::store::Store;
        use crate::types::{AgentId, Effort, HandHolding};
        let s = Scratch::new(true);
        let repo = s.repo();
        let store = Store::in_memory().unwrap();
        let new = |f: &dyn Fn(&mut crate::store::Thread)| {
            let mut t = store.create_thread(None, AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
            f(&mut t);
            store.save_thread(&t).unwrap();
            let sha = repo.snapshot(&t.id, "u1").unwrap();
            store.add_checkpoint(&t.id, "u1", &repo.top, &sha).unwrap();
            t.id
        };
        let now = 100 * 86_400_000;
        let old = now - STALE_AFTER_MS - 1;
        let active = new(&|_| {});
        let archived = new(&|t| t.archived_at = Some(old));
        let settled = new(&|t| t.settled_at = Some(old));
        let pinned = new(&|t| {
            t.settled_at = Some(old);
            t.pinned_at = Some(old);
        });
        let recent = new(&|t| t.archived_at = Some(now - 1));
        repo.restore(&store.checkpoints(&archived).unwrap()[0].sha, &archived).unwrap();
        assert_eq!(prune_stale(&store, now).unwrap(), 2);
        for (thread, kept) in [(&active, true), (&archived, false), (&settled, false), (&pinned, true), (&recent, true)] {
            assert_eq!(!store.checkpoints(thread).unwrap().is_empty(), kept, "{thread}");
            assert_eq!(!repo.items(thread).unwrap().is_empty(), kept, "{thread}");
        }
        assert!(s.git(&["for-each-ref", &undo_ref(&archived)]).is_empty());
    }

    #[test]
    fn restore_undoes_edits_new_files_deletions_and_renames() {
        let s = Scratch::new(true);
        s.write("draft.txt", "untracked before the turn\n");
        s.write("build.log", "ignored\n");
        let repo = s.repo();
        let sha = repo.snapshot("t1", "u1").unwrap();
        assert_eq!(s.git(&["rev-parse", &ref_name("t1", "u1")]), sha);
        assert_eq!(s.git(&["rev-parse", &format!("{sha}^")]), s.git(&["rev-parse", "HEAD"]), "hung on HEAD");

        // The agent's turn: edits, creates (nested), deletes, renames, rewrites the binary,
        // touches an ignored file.
        s.write("a.txt", "two\n");
        s.write("src/deep/new.rs", "fn main() {}\n");
        std::fs::remove_file(s.0.join("draft.txt")).unwrap();
        std::fs::rename(s.0.join("keep/old.txt"), s.0.join("keep/renamed.txt")).unwrap();
        std::fs::write(s.0.join("bin.dat"), [9u8, 9, 9]).unwrap();
        s.write("build.log", "changed by the turn\n");

        let expected = vec![
            ("a.txt".to_string(), Change::Modified),
            ("bin.dat".into(), Change::Modified),
            ("draft.txt".into(), Change::Deleted),
            ("keep/old.txt".into(), Change::Deleted),
            ("keep/renamed.txt".into(), Change::Added),
            ("src/deep/new.rs".into(), Change::Added),
        ];
        assert_eq!(sorted(repo.changes_since(&sha).unwrap()), expected);
        let restored = repo.restore(&sha, "t1").unwrap();
        assert_eq!(sorted(restored.changes.clone()), expected);
        assert_eq!((restored.restored(), restored.failed.len()), (6, 0));

        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert_eq!(std::fs::read(s.0.join("bin.dat")).unwrap(), [0u8, 159, 146, 150, 0, 1, 2]);
        assert_eq!(s.read("draft.txt").as_deref(), Some("untracked before the turn\n"));
        assert_eq!(s.read("keep/old.txt").as_deref(), Some("old\n"));
        assert!(!s.0.join("keep/renamed.txt").exists());
        assert!(!s.0.join("src").exists(), "folders left empty go too");
        assert_eq!(s.read("build.log").as_deref(), Some("changed by the turn\n"), "ignored files are left alone");
        assert!(repo.changes_since(&sha).unwrap().is_empty());

        // The restore itself can be undone: the files as they were just before it come back.
        let undo = restored.undo.expect("kept what it replaced");
        assert_eq!(s.git(&["rev-parse", &undo_ref("t1")]), undo);
        repo.restore(&undo, "t1").unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("two\n"));
        assert_eq!(s.read("src/deep/new.rs").as_deref(), Some("fn main() {}\n"));
        assert!(!s.0.join("draft.txt").exists());
        assert!(repo.changes_since(&undo).unwrap().is_empty());
        // Nothing to put back: nothing kept.
        assert_eq!(repo.restore(&undo, "t1").unwrap(), Restored::default());
    }

    #[test]
    fn restore_paths_puts_back_only_the_files_named() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let sha = repo.snapshot("t1", "u1").unwrap();
        s.write("a.txt", "two\n");
        s.write("keep/old.txt", "edited by hand\n");
        s.write("src/deep/new.rs", "fn main() {}\n");
        std::fs::remove_file(s.0.join(".gitignore")).unwrap();

        let failed = repo.restore_paths(&sha, &["a.txt", "src/deep/new.rs", ".gitignore"]).unwrap();
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"), "changed: its old contents");
        assert!(!s.0.join("src").exists(), "created since: removed, and the folders it leaves empty");
        assert_eq!(s.read(".gitignore").as_deref(), Some("*.log\n"), "deleted since: back");
        assert_eq!(s.read("keep/old.txt").as_deref(), Some("edited by hand\n"), "files not named are left alone");
        // From a tree as well as a checkpoint, and nothing to do is nothing done.
        s.write("a.txt", "three\n");
        repo.restore_paths(&repo.tree_of(&sha).unwrap(), &["a.txt"]).unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert!(repo.restore_paths(&sha, &[]).unwrap().is_empty());
    }

    #[test]
    fn tree_with_takes_the_named_files_from_another_tree() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let base = repo.snapshot("t1", "u1").unwrap();
        s.write("a.txt", "two\n");
        s.write("keep/old.txt", "edited\n");
        s.write("new.txt", "new\n");
        std::fs::remove_file(s.0.join("bin.dat")).unwrap();
        let now = repo.tree_now().unwrap();

        // a.txt and new.txt kept, bin.dat's deletion kept: only keep/old.txt still differs.
        let kept = repo.tree_with(&base, &now, &["a.txt", "new.txt", "bin.dat"]).unwrap();
        let left: Vec<String> = repo.diff_stat(&kept, &now).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(left, ["keep/old.txt"]);
        // What wasn't named is still the base's.
        let from_base: Vec<String> = repo.diff_stat(&base, &kept).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(from_base, ["a.txt", "bin.dat", "new.txt"]);
        // Undoing against the kept tree puts back the kept version, not the base's.
        s.write("a.txt", "three\n");
        repo.restore_paths(&kept, &["a.txt", "keep/old.txt"]).unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("two\n"));
        assert_eq!(s.read("keep/old.txt").as_deref(), Some("old\n"));
        assert_eq!(repo.tree_with(&base, &now, &[]).unwrap(), repo.tree_of(&base).unwrap());
    }

    #[test]
    fn paths_are_taken_literally() {
        let s = Scratch::new(true);
        s.write("lit[1].txt", "one\n");
        s.write("lit1.txt", "other\n");
        let repo = s.repo();
        let sha = repo.snapshot("t1", "u1").unwrap();
        s.write("lit[1].txt", "two\n");
        s.write("lit1.txt", "changed\n");
        repo.restore_paths(&sha, &["lit[1].txt"]).unwrap();
        assert_eq!(s.read("lit[1].txt").as_deref(), Some("one\n"));
        assert_eq!(s.read("lit1.txt").as_deref(), Some("changed\n"), "not matched as a glob");
    }

    #[test]
    fn diff_stat_counts_lines_between_checkpoints_and_against_the_files_now() {
        use crate::changes::FileStatus;
        let s = Scratch::new(true);
        s.write("keep/long.txt", &(1..=20).map(|i| format!("line {i}\n")).collect::<String>());
        s.git(&["add", "-A"]);
        s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "long"]);
        let repo = s.repo();
        let start = repo.snapshot("t", "u1").unwrap();
        // The turn: an edit, a new file (untracked), a deletion, a rename with a small change,
        // a binary rewrite; an ignored file doesn't count.
        s.write("a.txt", "one\ntwo\nthree\n");
        s.write("src/new.rs", "fn a() {}\nfn b() {}\n");
        std::fs::remove_file(s.0.join("keep/old.txt")).unwrap();
        let long = s.read("keep/long.txt").unwrap();
        std::fs::remove_file(s.0.join("keep/long.txt")).unwrap();
        s.write("keep/moved.txt", &long.replace("line 20\n", "line twenty\n"));
        std::fs::write(s.0.join("bin.dat"), [7u8, 0, 7]).unwrap();
        s.write("build.log", "ignored\n");
        let now = repo.tree_now().unwrap();
        let files = repo.diff_stat(&start, &now).unwrap();
        let got: Vec<(&str, FileStatus, u32, u32, bool)> = files.iter().map(|f| (f.path.as_str(), f.status.clone(), f.added, f.removed, f.binary)).collect();
        assert_eq!(
            got,
            [
                ("a.txt", FileStatus::Modified, 2, 0, false),
                ("bin.dat", FileStatus::Modified, 0, 0, true),
                ("keep/moved.txt", FileStatus::Renamed { from: "keep/long.txt".into() }, 1, 1, false),
                ("keep/old.txt", FileStatus::Deleted, 0, 1, false),
                ("src/new.rs", FileStatus::Added, 2, 0, false),
            ]
        );
        // A file's patch, a rename as one.
        let patch = repo.diff_patch(&start, &now, "keep/moved.txt", Some("keep/long.txt")).unwrap();
        assert!(patch.contains("rename from keep/long.txt") && patch.contains("-line 20") && patch.contains("+line twenty"), "{patch}");
        let patch = repo.diff_patch(&start, &now, "src/new.rs", None).unwrap();
        assert!(patch.contains("+fn b() {}"), "{patch}");
        // The next turn's checkpoint bounds this one: what came after it doesn't count.
        let next = repo.snapshot("t", "u2").unwrap();
        s.write("a.txt", "changed again later\n");
        assert_eq!(repo.diff_stat(&start, &next).unwrap(), files);
        assert!(repo.diff_stat(&next, &next).unwrap().is_empty());
    }

    #[test]
    fn the_users_index_head_branches_and_stash_are_untouched() {
        let s = Scratch::new(true);
        // Staged work, unstaged work and a stash, all the user's own.
        s.write("a.txt", "staged\n");
        s.git(&["add", "a.txt"]);
        s.write("a.txt", "staged then edited\n");
        s.write("keep/old.txt", "stashed\n");
        s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "stash", "push", "-q", "--", "keep/old.txt"]);
        let (head, branches, stash, staged) =
            (s.git(&["rev-parse", "HEAD"]), s.git(&["branch", "--list"]), s.git(&["stash", "list"]), s.git(&["diff", "--cached", "--name-only"]));
        let index_before = std::fs::read(s.0.join(".git/index")).unwrap();

        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();
        s.write("a.txt", "the agent's\n");
        s.write("new.txt", "x\n");
        repo.restore(&sha, "t").unwrap();

        assert_eq!(s.read("a.txt").as_deref(), Some("staged then edited\n"));
        assert!(!s.0.join("new.txt").exists());
        assert_eq!(std::fs::read(s.0.join(".git/index")).unwrap(), index_before, "the index file wasn't rewritten");
        assert_eq!(s.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(s.git(&["branch", "--list"]), branches);
        assert_eq!(s.git(&["stash", "list"]), stash);
        assert_eq!(s.git(&["diff", "--cached", "--name-only"]), staged);
        assert_eq!(s.git(&["show", ":a.txt"]), "staged", "what was staged stays staged");
    }

    #[test]
    fn a_repo_without_commits_still_gets_checkpoints() {
        let s = Scratch::new(false);
        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();
        assert!(Command::new("git").args(["rev-parse", "-q", "--verify", "HEAD"]).current_dir(&s.0).output().unwrap().status.code() != Some(0));
        s.write("a.txt", "changed\n");
        s.write("b.txt", "new\n");
        assert_eq!(sorted(repo.restore(&sha, "t").unwrap().changes), vec![("a.txt".to_string(), Change::Modified), ("b.txt".into(), Change::Added)]);
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert!(!s.0.join("b.txt").exists());
    }

    #[test]
    fn folders_outside_git_have_no_repo() {
        let dir = std::env::temp_dir().join(format!("trek-checkpoint-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Unless the temp folder itself sits in a repo, which it doesn't on a normal Mac.
        assert_eq!(Repo::find(&dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subfolders_snapshot_the_whole_tree() {
        let s = Scratch::new(true);
        let repo = Repo::find(&s.0.join("keep")).unwrap();
        assert_eq!(repo, s.repo());
        let sha = repo.snapshot("t", "u").unwrap();
        s.write("a.txt", "outside the subfolder\n");
        repo.restore(&sha, "t").unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
    }

    #[test]
    fn checkpoints_are_linked_listed_and_deleted_per_thread() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let a = repo.snapshot("t1", "u1").unwrap();
        let b = repo.snapshot("t1", "u2").unwrap();
        repo.snapshot("t2", "u9").unwrap();
        repo.link("fork", &[("v1".into(), a.clone()), ("v2".into(), b)]).unwrap();
        assert_eq!(repo.items("fork").unwrap(), ["v1", "v2"]);
        assert_eq!(s.git(&["rev-parse", &ref_name("fork", "v1")]), a);
        repo.delete("t1", &["u1".into()]).unwrap();
        assert_eq!(repo.items("t1").unwrap(), ["u2"]);
        assert_eq!(repo.delete_all("t1").unwrap(), 1);
        assert!(repo.items("t1").unwrap().is_empty());
        assert_eq!(repo.items("t2").unwrap(), ["u9"], "other threads keep theirs");
        // The fork's copies outlive the original's refs.
        assert_eq!(repo.restore(&a, "fork").unwrap(), Restored::default());
        // A thread's undo goes with it too.
        s.write("a.txt", "changed\n");
        repo.restore(&a, "t2").unwrap();
        assert!(!s.git(&["for-each-ref", &undo_ref("t2")]).is_empty());
        assert_eq!(repo.delete_all("t2").unwrap(), 1);
        assert!(s.git(&["for-each-ref", "refs/trek/"]).lines().all(|r| !r.contains("t2")));
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A repository of its own in `dir`, with one commit.
    fn nested_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git_in(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("lib.rs"), "// vendored\n").unwrap();
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "v"]);
    }

    #[test]
    fn nested_repositories_are_left_alone() {
        let s = Scratch::new(true);
        // One with no commit yet sits in the tree as the turn starts: git can't take it, and
        // the snapshot goes on without it.
        std::fs::create_dir_all(s.0.join("empty")).unwrap();
        git_in(&s.0.join("empty"), &["init", "-q"]);
        s.write("empty/x.txt", "x\n");
        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();

        // The turn clones something (a repository with commits), edits and adds files.
        nested_repo(&s.0.join("vendor/lib"));
        s.write("a.txt", "two\n");
        s.write("b_new.txt", "new\n");
        assert_eq!(
            sorted(repo.changes_since(&sha).unwrap()),
            vec![("a.txt".to_string(), Change::Modified), ("b_new.txt".into(), Change::Added), ("vendor/lib".into(), Change::Nested)]
        );
        let restored = repo.restore(&sha, "t").unwrap();
        assert_eq!((restored.restored(), restored.failed.clone()), (2, vec![]));
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert!(!s.0.join("b_new.txt").exists());
        assert_eq!(s.read("vendor/lib/lib.rs").as_deref(), Some("// vendored\n"), "the clone is still there");
        assert!(s.0.join("vendor/lib/.git").exists());
        assert_eq!(s.read("empty/x.txt").as_deref(), Some("x\n"));
    }

    #[test]
    fn a_file_that_cant_be_removed_doesnt_stop_the_rest() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();
        s.write("locked/new.txt", "made by the turn\n");
        s.write("a.txt", "two\n");
        use std::os::unix::fs::PermissionsExt as _;
        let locked = s.0.join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let restored = repo.restore(&sha, "t").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(restored.failed.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["locked/new.txt"]);
        assert_eq!(restored.restored(), 1);
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"), "the other file came back");
        assert!(restored.undo.is_some());
    }

    #[test]
    fn a_snapshot_that_takes_too_long_is_stopped() {
        let s = Scratch::new(true);
        // A clean filter that hangs (an LFS server that doesn't answer, say).
        s.git(&["config", "filter.slow.clean", "sleep 5; cat"]);
        s.write(".gitattributes", "*.big filter=slow\n");
        s.write("data.big", "lots\n");
        let started = std::time::Instant::now();
        let err = s.repo().with_limit(Duration::from_millis(300)).snapshot("t", "u").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
        assert!(format!("{err:#}").contains("took longer"), "{err:#}");
        assert!(s.repo().items("t").unwrap().is_empty());
    }

    /// A tracked file of twenty numbered lines, committed; returns the baseline (HEAD's tree).
    fn numbered(s: &Scratch) -> String {
        s.write("n.txt", &(1..=20).map(|i| format!("line {i}\n")).collect::<String>());
        s.git(&["add", "-A"]);
        s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "n"]);
        s.repo().tree_of("HEAD").unwrap()
    }

    fn edit_line(s: &Scratch, n: usize, to: &str) {
        let text = s.read("n.txt").unwrap();
        let out: String = text.lines().enumerate().map(|(i, l)| if i + 1 == n { format!("{to}\n") } else { format!("{l}\n") }).collect();
        s.write("n.txt", &out);
    }

    #[test]
    fn hunks_are_kept_into_the_baseline_and_undone_from_the_file() {
        let s = Scratch::new(true);
        let base = numbered(&s);
        let repo = s.repo();
        edit_line(&s, 2, "TWO");
        edit_line(&s, 18, "EIGHTEEN");
        let diff = repo.file_diff(&base, "n.txt").unwrap();
        assert_eq!(diff.hunks.len(), 2, "{diff:?}");
        // Keep the first: the baseline takes it, and only the second is left.
        let kept = repo.keep_hunk(&base, &diff.patch(0).unwrap()).unwrap();
        let left = repo.file_diff(&kept, "n.txt").unwrap();
        assert_eq!(left.hunks.len(), 1);
        assert!(left.hunks[0].body.contains("+EIGHTEEN"), "{left:?}");
        assert_eq!(s.read("n.txt").unwrap().lines().nth(1), Some("TWO"), "keeping leaves the file alone");
        // Undo the second: the file loses it, and nothing's left against the new baseline.
        repo.undo_hunk("n.txt", &left.patch(0).unwrap()).unwrap();
        let text = s.read("n.txt").unwrap();
        assert_eq!((text.lines().nth(1), text.lines().nth(17)), (Some("TWO"), Some("line 18")));
        assert!(repo.file_diff(&kept, "n.txt").unwrap().hunks.is_empty());
        assert_eq!(s.git(&["status", "--porcelain", "--", "n.txt"]), "M n.txt", "the user's index is untouched");
    }

    #[test]
    fn a_hunk_whose_surroundings_moved_merges_or_is_refused_cleanly() {
        let s = Scratch::new(true);
        let base = numbered(&s);
        let repo = s.repo();
        edit_line(&s, 10, "TEN");
        let diff = repo.file_diff(&base, "n.txt").unwrap();
        // A line in the hunk's context changed since: a three-way merge still takes it out.
        edit_line(&s, 12, "twelve, by the user");
        repo.undo_hunk("n.txt", &diff.patch(0).unwrap()).unwrap();
        let text = s.read("n.txt").unwrap();
        assert_eq!((text.lines().nth(9), text.lines().nth(11)), (Some("line 10"), Some("twelve, by the user")));
        // The hunk's own line changed since: refused, and the file is as it was.
        edit_line(&s, 5, "FIVE");
        let diff = repo.file_diff(&base, "n.txt").unwrap();
        let five = diff.hunks.iter().position(|h| h.body.contains("+FIVE")).unwrap();
        edit_line(&s, 5, "five, rewritten");
        let before = s.read("n.txt").unwrap();
        let err = repo.undo_hunk("n.txt", &diff.patch(five).unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("changed since"), "{err:#}");
        assert_eq!(s.read("n.txt").unwrap(), before);
    }

    #[test]
    fn a_new_file_is_one_hunk_kept_or_undone_whole() {
        let s = Scratch::new(true);
        let base = s.repo().tree_of("HEAD").unwrap();
        let repo = s.repo();
        s.write("src/new.rs", "fn a() {}\nfn b() {}\n");
        let diff = repo.file_diff(&base, "src/new.rs").unwrap();
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.marks(), [crate::hunks::Mark::Added { start: 0, end: 2 }]);
        let kept = repo.keep_hunk(&base, &diff.patch(0).unwrap()).unwrap();
        assert!(repo.file_diff(&kept, "src/new.rs").unwrap().hunks.is_empty(), "kept: the baseline has it");
        repo.undo_hunk("src/new.rs", &diff.patch(0).unwrap()).unwrap();
        assert!(s.read("src/new.rs").is_none(), "undone: it goes");
        assert!(s.git(&["status", "--porcelain"]).is_empty(), "and nothing else moved");
    }

    #[test]
    fn hunks_patch_the_right_file_whatever_the_users_diff_prefixes() {
        let s = Scratch::new(true);
        s.write("src/lib.rs", &(1..=8).map(|i| format!("line {i}\n")).collect::<String>());
        s.write("lib.rs", "a top-level file of the same name\n");
        s.git(&["add", "-A"]);
        s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "libs"]);
        // Headers without `a/` and `b/`: `--- src/lib.rs`, which `git apply` would read as `lib.rs`.
        s.git(&["config", "diff.noprefix", "true"]);
        s.git(&["config", "diff.mnemonicPrefix", "true"]);
        let repo = s.repo();
        let base = repo.tree_of("HEAD").unwrap();
        s.write("src/lib.rs", &(1..=8).map(|i| if i == 4 { "FOUR\n".to_string() } else { format!("line {i}\n") }).collect::<String>());
        let diff = repo.file_diff(&base, "src/lib.rs").unwrap();
        assert!(diff.header.contains("--- a/src/lib.rs") && diff.header.contains("+++ b/src/lib.rs"), "{}", diff.header);
        let kept = repo.keep_hunk(&base, &diff.patch(0).unwrap()).unwrap();
        assert!(repo.file_diff(&kept, "src/lib.rs").unwrap().hunks.is_empty(), "kept into src/lib.rs");
        assert_eq!(repo.entries(&kept, &["lib.rs"]).unwrap(), repo.entries(&base, &["lib.rs"]).unwrap(), "not into lib.rs");
        repo.undo_hunk("src/lib.rs", &diff.patch(0).unwrap()).unwrap();
        assert_eq!(s.read("src/lib.rs").unwrap().lines().nth(3), Some("line 4"));
        assert_eq!(s.read("lib.rs").as_deref(), Some("a top-level file of the same name\n"), "the other file is untouched");
    }

    #[test]
    fn hunks_ending_in_blank_lines_or_spaces_keep_and_undo_exactly() {
        let s = Scratch::new(true);
        // A file ending in blank lines: the last hunk's trailing context is blank.
        s.write("tail.txt", "a\nb\nc\n\n\n");
        s.git(&["add", "-A"]);
        s.git(&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgSign=false", "commit", "-qm", "tail"]);
        let repo = s.repo();
        let base = repo.tree_of("HEAD").unwrap();
        s.write("tail.txt", "a\nb\nC\n\n\n");
        let diff = repo.file_diff(&base, "tail.txt").unwrap();
        assert!(diff.hunks[0].body.ends_with(" \n \n"), "{:?}", diff.hunks[0].body);
        let kept = repo.keep_hunk(&base, &diff.patch(0).unwrap()).unwrap();
        assert!(repo.file_diff(&kept, "tail.txt").unwrap().hunks.is_empty());
        repo.undo_hunk("tail.txt", &diff.patch(0).unwrap()).unwrap();
        assert_eq!(s.read("tail.txt").as_deref(), Some("a\nb\nc\n\n\n"));

        // A line ending in a tab and a space is kept as it is, not trimmed.
        s.write("tail.txt", "a\nb\nc\n\n\ny\t \n");
        let diff = repo.file_diff(&base, "tail.txt").unwrap();
        let kept = repo.keep_hunk(&base, &diff.patch(0).unwrap()).unwrap();
        assert!(repo.file_diff(&kept, "tail.txt").unwrap().hunks.is_empty(), "nothing left pending");
        let back = repo.tree_now().unwrap();
        assert_eq!(repo.entries(&kept, &["tail.txt"]).unwrap(), repo.entries(&back, &["tail.txt"]).unwrap(), "the baseline holds the file byte for byte");
    }

    #[test]
    fn a_rewind_leaves_an_ignored_file_where_its_folder_was() {
        let s = Scratch::new(true);
        s.write("build/x.txt", "checkpointed\n");
        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();
        // The folder goes and an ignored file of the same name takes its place.
        std::fs::remove_dir_all(s.0.join("build")).unwrap();
        s.write(".gitignore", "*.log\nbuild\n");
        s.write("build", "ignored output\n");
        s.write("a.txt", "two\n");
        let preview = repo.changes_since(&sha).unwrap();
        assert!(preview.iter().any(|c| c.path == "build/x.txt" && c.change == Change::Kept), "{preview:?}");
        let restored = repo.restore(&sha, "t").unwrap();
        assert_eq!(s.read("build").as_deref(), Some("ignored output\n"), "the ignored file is still there");
        assert!(restored.failed.iter().any(|(p, why)| p == "build/x.txt" && why.contains("folder")), "{:?}", restored.failed);
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"), "the rest is put back");
        // A tracked file the turn put there is removed first, so the folder comes back.
        let s = Scratch::new(true);
        s.write("out/y.txt", "y\n");
        let repo = s.repo();
        let sha = repo.snapshot("t", "u").unwrap();
        std::fs::remove_dir_all(s.0.join("out")).unwrap();
        s.write("out", "a file now\n");
        repo.restore(&sha, "t").unwrap();
        assert_eq!(s.read("out/y.txt").as_deref(), Some("y\n"));
    }

    #[test]
    fn git_reading_more_than_a_pipe_holds_while_it_writes_doesnt_hang() {
        let s = Scratch::new(true);
        let repo = s.repo();
        s.write("blob.txt", &"x".repeat(1024));
        let blob = repo.run(None, &["hash-object", "-w", "blob.txt"], None).unwrap();
        // `cat-file --batch` answers each line as it reads it: 2 MB out for 80 KB in.
        let input = format!("{blob}\n").repeat(2000);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let out = repo.output(None, &["cat-file", "--batch"], Some(input.as_bytes()), None).map(|o| o.stdout.len());
            let _ = tx.send(out.map_err(|e| e.to_string()));
        });
        let got = rx.recv_timeout(Duration::from_secs(30)).expect("git and Trek waited on each other").unwrap();
        assert!(got > 2000 * 1024, "{got}");
        let _ = s;
    }

    #[test]
    fn a_restore_in_some_paths_leaves_the_rest_and_can_be_undone_the_same_way() {
        let s = Scratch::new(true);
        let repo = s.repo();
        let start = repo.snapshot("t", "u1").unwrap();
        // The turn: a.txt edited, new.txt made.
        s.write("a.txt", "the turn's\n");
        s.write("new.txt", "the turn's\n");
        let end = repo.snapshot("t", "e1").unwrap();
        // Afterwards, another thread (or the user) makes other.txt and edits keep/old.txt.
        s.write("other.txt", "not the turn's\n");
        s.write("keep/old.txt", "not the turn's\n");
        let only = repo.paths_changed(&[(start.clone(), end)]).unwrap();
        assert_eq!(only, HashSet::from(["a.txt".to_string(), "new.txt".to_string()]));
        assert_eq!(sorted(repo.changes_since_in(&start, &only).unwrap()), [("a.txt".to_string(), Change::Modified), ("new.txt".into(), Change::Added)]);
        let restored = repo.restore_in(&start, "t", &only).unwrap();
        assert_eq!(restored.restored(), 2);
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert!(s.read("new.txt").is_none());
        assert_eq!(s.read("other.txt").as_deref(), Some("not the turn's\n"), "not the turn's: left alone");
        assert_eq!(s.read("keep/old.txt").as_deref(), Some("not the turn's\n"));
        // Undone in the same paths: other.txt, written since, stays.
        s.write("later.txt", "after the restore\n");
        repo.restore_in(&restored.undo.unwrap(), "t", &only).unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("the turn's\n"));
        assert_eq!(s.read("new.txt").as_deref(), Some("the turn's\n"));
        assert_eq!(s.read("later.txt").as_deref(), Some("after the restore\n"));
        // An undo kept by hand ahead of a restore of paths.
        let undo = repo.save_undo("t").unwrap();
        repo.restore_paths(&start, &["a.txt"]).unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        repo.restore_in(&undo, "t", &HashSet::from(["a.txt".to_string()])).unwrap();
        assert_eq!(s.read("a.txt").as_deref(), Some("the turn's\n"));
    }

    /// Snapshot timings on a real working tree (not run by default): `TREK_BENCH_REPO=<a scratch
    /// clone> cargo test -p trek-core snapshot_speed -- --ignored --nocapture`. It writes objects
    /// and a ref into that repo, so point it at a copy.
    #[test]
    #[ignore]
    fn snapshot_speed() {
        let dir = PathBuf::from(std::env::var("TREK_BENCH_REPO").expect("TREK_BENCH_REPO"));
        let repo = Repo::find(&dir).unwrap();
        for i in 0..5 {
            let t = std::time::Instant::now();
            let sha = repo.snapshot("bench", &format!("u{i}")).unwrap();
            let snap = t.elapsed();
            let t = std::time::Instant::now();
            let changes = repo.changes_since(&sha).unwrap();
            println!("snapshot {snap:?}, changes {:?} ({} files)", t.elapsed(), changes.len());
        }
        repo.delete_all("bench").unwrap();
    }
}
