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

/// Checkpoints kept per thread; older ones are pruned as new ones are taken.
pub const KEEP: usize = 100;

const REF_ROOT: &str = "refs/trek/checkpoints";

/// Where the checkpoint taken as `item` was sent in `thread` lives.
pub fn ref_name(thread: &str, item: &str) -> String {
    format!("{REF_ROOT}/{thread}/{item}")
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the repository's top folder.
    pub path: String,
    pub change: Change,
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
        (!top.as_os_str().is_empty()).then_some(Repo { top, index })
    }

    fn git(&self, index: Option<&TempIndex>) -> Command {
        let mut c = base_command(&self.top);
        if let Some(i) = index {
            c.env("GIT_INDEX_FILE", &i.0);
        }
        c
    }

    fn run(&self, index: Option<&TempIndex>, args: &[&str], input: Option<&[u8]>) -> Result<String> {
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
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("git {} failed: {}", args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"?"), err.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    /// The tree the working directory holds right now: tracked and untracked files that aren't
    /// ignored, as `git add -A` sees them.
    fn tree_now(&self) -> Result<String> {
        let index = TempIndex::new();
        // Starting from the user's index keeps its stat data: only files that changed are read.
        if self.index.exists() {
            std::fs::copy(&self.index, &index.0).context("copy the index")?;
        }
        self.run(Some(&index), &["add", "-A"], None)?;
        self.run(Some(&index), &["write-tree"], None)
    }

    /// Snapshot the working tree as checkpoint `item` of `thread`; returns its commit.
    pub fn snapshot(&self, thread: &str, item: &str) -> Result<String> {
        let tree = self.tree_now()?;
        let commit = |parent: bool| {
            let mut args = vec!["-c", "commit.gpgSign=false", "commit-tree", tree.as_str(), "-m", "Trek checkpoint"];
            if parent {
                args.extend(["-p", "HEAD"]);
            }
            self.run(None, &args, None)
        };
        // A repository without a commit yet has no HEAD to hang the snapshot on.
        let sha = commit(true).or_else(|_| commit(false))?;
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
        let out = self.run(None, &["diff-tree", "-r", "-z", "--no-renames", "--name-status", from, to], None)?;
        let mut fields = out.split('\0').filter(|f| !f.is_empty());
        let mut changes = vec![];
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            let change = match status.chars().next() {
                Some('A') => Change::Added,
                Some('D') => Change::Deleted,
                _ => Change::Modified,
            };
            changes.push(FileChange { path: path.to_string(), change });
        }
        Ok(changes)
    }

    /// Put the working tree back as checkpoint `sha` had it: files changed since get their old
    /// contents, deleted ones come back, and ones created since are removed (ignored files are
    /// left alone). Returns what changed.
    pub fn restore(&self, sha: &str) -> Result<Vec<FileChange>> {
        let changes = self.changes_since(sha)?;
        // Removals first: a new file may stand where the checkpoint has a folder, or the reverse.
        for c in changes.iter().filter(|c| c.change == Change::Added) {
            let path = self.top.join(&c.path);
            match std::fs::remove_file(&path) {
                Ok(()) => self.prune_empty_dirs(&path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("remove {}", c.path)),
            }
        }
        let back: Vec<&str> = changes.iter().filter(|c| c.change != Change::Added).map(|c| c.path.as_str()).collect();
        if !back.is_empty() {
            let index = TempIndex::new();
            self.run(Some(&index), &["read-tree", sha], None)?;
            let mut input = back.join("\0").into_bytes();
            input.push(0);
            self.run(Some(&index), &["checkout-index", "-f", "-z", "--stdin"], Some(&input))?;
        }
        Ok(changes)
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

    /// Drop every checkpoint of `thread` (it was deleted); returns how many went.
    pub fn delete_all(&self, thread: &str) -> Result<usize> {
        let refs = self.run(None, &["for-each-ref", "--format=%(refname)", &format!("{REF_ROOT}/{thread}/")], None)?;
        let script: String = refs.lines().filter(|r| !r.is_empty()).map(|r| format!("delete {r}\n")).collect();
        if !script.is_empty() {
            self.run(None, &["update-ref", "--stdin"], Some(script.as_bytes()))?;
        }
        Ok(refs.lines().filter(|r| !r.is_empty()).count())
    }

    /// Checkpoint items of `thread` that have a ref.
    pub fn items(&self, thread: &str) -> Result<Vec<String>> {
        let prefix = format!("{REF_ROOT}/{thread}/");
        let refs = self.run(None, &["for-each-ref", "--format=%(refname)", &prefix], None)?;
        Ok(refs.lines().filter_map(|r| r.strip_prefix(&prefix)).map(str::to_string).collect())
    }
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
        assert_eq!(sorted(repo.restore(&sha).unwrap()), expected);

        assert_eq!(s.read("a.txt").as_deref(), Some("one\n"));
        assert_eq!(std::fs::read(s.0.join("bin.dat")).unwrap(), [0u8, 159, 146, 150, 0, 1, 2]);
        assert_eq!(s.read("draft.txt").as_deref(), Some("untracked before the turn\n"));
        assert_eq!(s.read("keep/old.txt").as_deref(), Some("old\n"));
        assert!(!s.0.join("keep/renamed.txt").exists());
        assert!(!s.0.join("src").exists(), "folders left empty go too");
        assert_eq!(s.read("build.log").as_deref(), Some("changed by the turn\n"), "ignored files are left alone");
        assert!(repo.changes_since(&sha).unwrap().is_empty());
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
        repo.restore(&sha).unwrap();

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
        assert_eq!(sorted(repo.restore(&sha).unwrap()), vec![("a.txt".to_string(), Change::Modified), ("b.txt".into(), Change::Added)]);
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
        repo.restore(&sha).unwrap();
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
        assert_eq!(repo.restore(&a).unwrap(), vec![]);
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
