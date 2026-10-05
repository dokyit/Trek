//! File checkpoints: a snapshot of a git working tree taken as a turn starts, so a rewind can put
//! the files back the way they were.
//!
//! A snapshot is a commit (parent: HEAD, when there is one) of every tracked and untracked file
//! that isn't ignored, kept under `refs/trek/checkpoints/<thread>/<item>`. It's made through a
//! temporary index seeded from the user's, so only files that changed are read again, and the
//! user's index, HEAD, branches and stash are never touched. Restoring writes files back the same
//! way. Everything here blocks on git: run it off the main thread.

use anyhow::{Context as _, Result, bail};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Checkpoints kept per thread; older ones are pruned as new ones are taken.
pub const KEEP: usize = 100;

/// Longest a snapshot may take reading the files. The message waits for it, so past this (huge
/// untracked files, a clean filter that hangs) it goes without one.
pub const SNAPSHOT_LIMIT: Duration = Duration::from_secs(10);

/// Untracked files bigger than this are left out of snapshots: a dataset, a video or build output
/// missing from `.gitignore` would otherwise be copied into the user's `.git` on the next turn.
/// Restoring leaves them as they are.
pub const UNTRACKED_MAX: u64 = 8 << 20;

/// Checkpoints of threads archived or settled this long ago are dropped (`prune_stale`), so their
/// refs don't keep big objects alive in the user's repo for good.
pub const STALE_AFTER_MS: i64 = 30 * 86_400_000;

const REF_ROOT: &str = "refs/trek/checkpoints";
const UNDO_ROOT: &str = "refs/trek/undo";

/// Where the checkpoint taken as `item` was sent in `thread` lives.
pub fn ref_name(thread: &str, item: &str) -> String {
    format!("{REF_ROOT}/{thread}/{item}")
}

/// Where the files as they were just before `thread`'s latest restore are kept, so it can be undone.
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
        self.changes.iter().filter(|c| c.change != Change::Nested).count().saturating_sub(self.failed.len())
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
        let out = self.output(index, args, input, None)?;
        if !out.status.success() {
            bail!("git {} failed: {}", command_name(args), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// Run git; with `limit`, it's stopped if it runs longer.
    fn output(&self, index: Option<&TempIndex>, args: &[&str], input: Option<&[u8]>, limit: Option<Duration>) -> Result<std::process::Output> {
        let mut c = self.git(index);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn().context("couldn't run git")?;
        if let Some(bytes) = input {
            let mut stdin = child.stdin.take().context("git stdin")?;
            stdin.write_all(bytes)?;
        }
        let Some(limit) = limit else { return Ok(child.wait_with_output()?) };
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

    /// A commit of `tree`, hung on HEAD when there is one.
    fn commit(&self, tree: &str) -> Result<String> {
        let commit = |parent: bool| {
            let mut args = vec!["-c", "commit.gpgSign=false", "commit-tree", tree, "-m", "Trek checkpoint"];
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
        let now = self.tree_now()?;
        self.diff(sha, &now)
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
        let mut args = vec!["diff-tree", "-p", "-M", "--no-color", "--no-ext-diff", from, to, "--"];
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
        let now = self.tree_now()?;
        let changes = self.diff(sha, &now)?;
        if changes.iter().all(|c| c.change == Change::Nested) {
            return Ok(Restored { changes, ..Restored::default() });
        }
        let undo = self.commit(&now)?;
        self.run(None, &["update-ref", &undo_ref(thread), &undo], None)?;
        let touch = |c: &&FileChange| c.change != Change::Nested;
        let mut failed = vec![];
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
            let out = self.output(Some(&index), &["checkout-index", "-f", "-z", "--stdin"], Some(&input), None)?;
            if !out.status.success() {
                // It writes what it can and names the rest.
                let err = String::from_utf8_lossy(&out.stderr);
                for path in back.iter().filter(|p| err.contains(*p)) {
                    failed.push((path.to_string(), err.lines().find(|l| l.contains(*path)).unwrap_or_default().trim().to_string()));
                }
                if failed.is_empty() {
                    failed.push((String::new(), err.trim().to_string()));
                }
            }
        }
        Ok(Restored { changes, failed, undo: Some(undo) })
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
        let refs = self.run(None, &["for-each-ref", "--format=%(refname)", &format!("{REF_ROOT}/{thread}/"), &undo_ref(thread)], None)?;
        let refs: Vec<&str> = refs.lines().filter(|r| !r.is_empty()).collect();
        let script: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
        if !script.is_empty() {
            self.run(None, &["update-ref", "--stdin"], Some(script.as_bytes()))?;
        }
        Ok(refs.iter().filter(|r| r.starts_with(REF_ROOT)).count())
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
        for repo in repos {
            if let Some(r) = Repo::find(repo) {
                r.delete_all(thread)?;
            }
        }
        store.delete_checkpoints(thread, &checkpoints.iter().map(|c| c.item_id.clone()).collect::<Vec<_>>())?;
    }
    Ok(stale.len())
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
