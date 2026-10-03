//! Threads in worktrees of their own. A thread can run in a separate checkout of its project on a
//! `trek/<slug>` branch, so two threads in one project never edit the same files. This module
//! makes those checkouts and takes the work back: what changed against the base branch, reverting
//! a file, committing, pushing, a pull request, merging into the base, and removing the checkout.
//!
//! Everything here runs `git` (or `gh`) and blocks: call it off the UI thread.

use anyhow::{Context as _, Result, anyhow, bail};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Where a thread's checkout lives and what it's built on. Stored on the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The checkout, under [`worktrees_dir`].
    pub path: PathBuf,
    /// `trek/<slug>`.
    pub branch: String,
    /// The branch it started from, which it merges back into.
    pub base: String,
}

impl Worktree {
    /// The folder is gone (deleted by hand, or the disk it was on isn't there).
    pub fn is_missing(&self) -> bool {
        !self.path.join(".git").exists()
    }
}

pub const BRANCH_PREFIX: &str = "trek/";

/// Ignored files new worktrees get a copy of unless the project says otherwise: a fresh checkout
/// has none of them, and without them most apps don't start.
pub fn default_copy() -> Vec<String> {
    vec![".env".into(), ".env.local".into()]
}

/// Trek's worktrees: `<data dir>/worktrees/<repo>/<slug>`.
pub fn worktrees_dir() -> PathBuf {
    crate::paths::data_dir().join("worktrees")
}

/// Run git in `cwd`; its output, or what it said went wrong.
pub fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("PATH", crate::detect::login_path())
        // Never wait on a prompt nobody can see (credentials, an editor).
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .context("couldn't run git")?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        Err(anyhow!("{}", if err.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { err.to_string() }))
    }
}

fn ok(cwd: &Path, args: &[&str]) -> bool {
    git(cwd, args).is_ok()
}

fn count(cwd: &Path, args: &[&str]) -> usize {
    git(cwd, args).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// A branch-name slug from the thread's first message: its first few words, lowercase ASCII.
pub fn slug(text: &str) -> String {
    const MAX: usize = 32;
    let mut out = String::new();
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(6) {
        let word = word.to_ascii_lowercase();
        let room = MAX.saturating_sub(out.len() + usize::from(!out.is_empty()));
        if room < word.len().min(4) {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word[..word.len().min(room)]);
    }
    if out.is_empty() { "thread".into() } else { out }
}

/// The branch HEAD is on in `repo`, or why a worktree can't start from it.
pub fn current_branch(repo: &Path) -> Result<String> {
    if !ok(repo, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        bail!("The repository has no commits yet. Commit something first, then start a thread in a worktree.");
    }
    match git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        Ok(b) if !b.trim().is_empty() => Ok(b.trim().to_string()),
        _ => bail!("The project folder isn't on a branch (detached HEAD). Switch to a branch to start a thread in a worktree."),
    }
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    ok(repo, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
}

/// Pick the branch and folder for a new thread's worktree in `repo`: `trek/<slug>` off the
/// branch the project folder is on, at `<worktrees>/<repo>/<slug>` (Trek passes
/// [`worktrees_dir`]). Nothing is made yet ([`add`] does that).
pub fn plan(worktrees: &Path, repo: &Path, hint: &str) -> Result<Worktree> {
    let base = current_branch(repo)?;
    let name = repo.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    let root = worktrees.join(name);
    let stem = slug(hint);
    let mut n = 1;
    loop {
        let s = if n == 1 { stem.clone() } else { format!("{stem}-{n}") };
        let (path, branch) = (root.join(&s), format!("{BRANCH_PREFIX}{s}"));
        if !path.exists() && !branch_exists(repo, &branch) {
            return Ok(Worktree { path, branch, base });
        }
        n += 1;
    }
}

/// Check `wt` out: its branch if that exists (a worktree whose folder vanished), else a new one
/// off its base. Then copy `copy` (ignored files such as `.env`) in from the project folder.
pub fn add(repo: &Path, wt: &Worktree, copy: &[String]) -> Result<()> {
    // Forget a checkout git still lists for a folder that's gone.
    let _ = git(repo, &["worktree", "prune"]);
    if let Some(parent) = wt.path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("couldn't create {}", parent.display()))?;
    }
    let path = wt.path.to_string_lossy();
    if branch_exists(repo, &wt.branch) {
        git(repo, &["worktree", "add", &path, &wt.branch])?;
    } else {
        git(repo, &["worktree", "add", "-b", &wt.branch, &path, &wt.base])?;
    }
    copy_files(repo, &wt.path, copy);
    Ok(())
}

/// [`plan`] and [`add`] in one go.
pub fn create(worktrees: &Path, repo: &Path, hint: &str, copy: &[String]) -> Result<Worktree> {
    let wt = plan(worktrees, repo, hint)?;
    add(repo, &wt, copy)?;
    Ok(wt)
}

/// Copy each of `entries` (paths relative to the project: files or folders) that the project has
/// and the worktree doesn't. Returns what was copied.
pub fn copy_files(repo: &Path, dest: &Path, entries: &[String]) -> Vec<String> {
    let mut copied = vec![];
    for entry in entries.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        let rel = Path::new(entry);
        // Inside the project only.
        if !rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
            continue;
        }
        let (src, dst) = (repo.join(rel), dest.join(rel));
        if !src.exists() || dst.exists() {
            continue;
        }
        let done = if src.is_dir() {
            walkdir::WalkDir::new(&src).into_iter().flatten().all(|e| {
                let to = dst.join(e.path().strip_prefix(&src).unwrap_or(e.path()));
                if e.file_type().is_dir() { std::fs::create_dir_all(&to).is_ok() } else { std::fs::copy(e.path(), &to).is_ok() }
            })
        } else {
            dst.parent().is_none_or(|p| std::fs::create_dir_all(p).is_ok()) && std::fs::copy(&src, &dst).is_ok()
        };
        if done {
            copied.push(entry.to_string());
        }
    }
    copied
}

/// A file the thread changed, against its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// `A`dded, `M`odified, `D`eleted, or `U`ntracked (new, not added to git yet).
    pub status: char,
    pub additions: i64,
    pub deletions: i64,
}

/// Everything a worktree thread changed: its commits plus what isn't committed yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Review {
    /// Where the branch left its base; changes are counted from here.
    pub merge_base: String,
    pub files: Vec<Change>,
    /// Commits on the branch that its base doesn't have.
    pub ahead: usize,
    /// Commits on the base since the branch left it.
    pub behind: usize,
    /// Files with changes not committed yet (untracked ones included).
    pub uncommitted: usize,
    /// Commits not pushed yet, when the branch has an upstream.
    pub unpushed: Option<usize>,
}

impl Review {
    pub fn additions(&self) -> i64 {
        self.files.iter().map(|f| f.additions).sum()
    }

    pub fn deletions(&self) -> i64 {
        self.files.iter().map(|f| f.deletions).sum()
    }
}

/// Split NUL-separated git output.
fn fields(s: &str) -> impl Iterator<Item = &str> {
    s.split('\0').filter(|f| !f.is_empty())
}

fn line_count(path: &Path) -> i64 {
    std::fs::read_to_string(path).map(|s| s.lines().count() as i64).unwrap_or(0)
}

/// What the worktree changed against its base: committed, staged, unstaged and untracked.
pub fn review(wt: &Worktree) -> Result<Review> {
    let dir = &wt.path;
    let merge_base = git(dir, &["merge-base", &wt.base, "HEAD"]).map_err(|_| anyhow!("The base branch {} is gone, so there's nothing to compare with.", wt.base))?.trim().to_string();
    let mut stats = std::collections::HashMap::new();
    let numstat = git(dir, &["diff", "--no-renames", "--numstat", "-z", &merge_base])?;
    for f in fields(&numstat) {
        let mut parts = f.splitn(3, '\t');
        if let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) {
            stats.insert(p.to_string(), (a.parse().unwrap_or(0), d.parse().unwrap_or(0)));
        }
    }
    let mut files = vec![];
    let names = git(dir, &["diff", "--no-renames", "--name-status", "-z", &merge_base])?;
    let mut it = fields(&names);
    while let (Some(status), Some(path)) = (it.next(), it.next()) {
        let (additions, deletions) = stats.get(path).copied().unwrap_or((0, 0));
        files.push(Change { path: path.to_string(), status: status.chars().next().unwrap_or('M'), additions, deletions });
    }
    for path in fields(&git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?) {
        files.push(Change { path: path.to_string(), status: 'U', additions: line_count(&dir.join(path)), deletions: 0 });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let uncommitted = git(dir, &["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0);
    Ok(Review {
        ahead: count(dir, &["rev-list", "--count", &format!("{merge_base}..HEAD")]),
        behind: count(dir, &["rev-list", "--count", &format!("HEAD..{}", wt.base)]),
        unpushed: git(dir, &["rev-list", "--count", "@{u}..HEAD"]).ok().and_then(|s| s.trim().parse().ok()),
        merge_base,
        files,
        uncommitted,
    })
}

/// The diff of one changed file against the merge base (an untracked file, all added).
pub fn file_diff(wt: &Worktree, merge_base: &str, change: &Change) -> String {
    if change.status == 'U' {
        return match std::fs::read_to_string(wt.path.join(&change.path)) {
            Ok(s) => s.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"),
            Err(_) => "Binary or unreadable file".into(),
        };
    }
    git(&wt.path, &["diff", "--no-renames", merge_base, "--", &change.path]).unwrap_or_default()
}

/// Put one file back as it was at the merge base: changes and commits to it are undone in the
/// working tree (and index), and a file the thread added goes.
pub fn revert_file(wt: &Worktree, merge_base: &str, change: &Change) -> Result<()> {
    if change.status == 'U' {
        return std::fs::remove_file(wt.path.join(&change.path)).with_context(|| format!("couldn't delete {}", change.path));
    }
    let source = format!("--source={merge_base}");
    if change.status == 'A' && !ok(&wt.path, &["ls-files", "--error-unmatch", "--", &change.path]) {
        // Added in a commit but deleted since: nothing on disk or in the index to put back.
        return Ok(());
    }
    git(&wt.path, &["restore", &source, "--staged", "--worktree", "--", &change.path]).map(|_| ())
}

/// Commit everything in the worktree.
pub fn commit(dir: &Path, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        bail!("Write a commit message first.");
    }
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-m", message.trim()])?;
    Ok(())
}

/// What a commit message is written from: the diff's stat and the start of the diff itself
/// (staged or not, untracked files listed), against HEAD.
pub fn commit_context(dir: &Path, max_diff: usize) -> String {
    let stat = git(dir, &["diff", "HEAD", "--stat"]).unwrap_or_default();
    let mut diff = git(dir, &["diff", "HEAD"]).unwrap_or_default();
    if diff.len() > max_diff {
        let mut end = max_diff;
        while !diff.is_char_boundary(end) {
            end -= 1;
        }
        diff.truncate(end);
        diff.push_str("\n[diff truncated]");
    }
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    let mut out = format!("{}\n", stat.trim());
    if !untracked.trim().is_empty() {
        out.push_str(&format!("\nNew files:\n{}\n", untracked.trim()));
    }
    out.push_str(&format!("\n{diff}"));
    out
}

/// Push the branch to origin and track it.
pub fn push(wt: &Worktree) -> Result<()> {
    if git(&wt.path, &["remote"]).unwrap_or_default().lines().all(|r| r.trim() != "origin") {
        bail!("This repository has no origin remote to push to.");
    }
    git(&wt.path, &["push", "-u", "origin", &wt.branch]).map(|_| ())
}

/// The project's origin is on GitHub (pull requests go through `gh`).
pub fn github_origin(repo: &Path) -> bool {
    git(repo, &["remote", "get-url", "origin"]).is_ok_and(|u| u.contains("github.com"))
}

fn gh(dir: &Path, args: &[&str]) -> Result<String> {
    let bin = crate::detect::which("gh").context("The GitHub CLI (gh) isn't installed.")?;
    let out = Command::new(bin).args(args).current_dir(dir).env("PATH", crate::detect::login_path()).env("GH_PROMPT_DISABLED", "1").output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The open pull request for the branch, if there is one.
pub fn find_pr(wt: &Worktree) -> Option<String> {
    gh(&wt.path, &["pr", "view", &wt.branch, "--json", "url,state", "--jq", "select(.state == \"OPEN\") | .url"]).ok().filter(|u| u.starts_with("http"))
}

/// Push the branch and open a pull request into its base. Returns the pull request's URL.
pub fn create_pr(wt: &Worktree, title: &str, body: &str) -> Result<String> {
    push(wt)?;
    let out = gh(&wt.path, &["pr", "create", "--head", &wt.branch, "--base", &wt.base, "--title", title, "--body", body])?;
    out.lines().rev().find(|l| l.starts_with("http")).map(str::to_string).ok_or_else(|| anyhow!("gh didn't say where the pull request is: {out}"))
}

/// Subjects of the branch's own commits, oldest first (for a pull request's description).
pub fn commit_subjects(wt: &Worktree) -> Vec<String> {
    git(&wt.path, &["log", "--reverse", "--format=%s", &format!("{}..HEAD", wt.base)]).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()
}

/// Why the thread's branch can't be merged into its base right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeBlock {
    /// The project folder is on another branch (or none).
    NotOnBase { current: Option<String> },
    /// The project folder has changes of its own.
    BaseDirty,
    /// The worktree has changes that aren't committed: they wouldn't be merged.
    Uncommitted(usize),
    /// The base has every commit of the branch already.
    NothingToMerge,
    /// Merging conflicts in these files; nothing was changed.
    Conflicts(Vec<String>),
}

impl MergeBlock {
    pub fn explain(&self, base: &str) -> String {
        match self {
            MergeBlock::NotOnBase { current: Some(b) } => format!("The project folder is on {b}, not {base}. Switch it to {base} to merge here."),
            MergeBlock::NotOnBase { current: None } => format!("The project folder isn't on a branch. Switch it to {base} to merge here."),
            MergeBlock::BaseDirty => format!("The project folder has uncommitted changes on {base}. Commit or stash them first."),
            MergeBlock::Uncommitted(1) => "1 file isn't committed yet. Commit or revert it first.".into(),
            MergeBlock::Uncommitted(n) => format!("{n} files aren't committed yet. Commit or revert them first."),
            MergeBlock::NothingToMerge => format!("{base} already has everything on this branch."),
            MergeBlock::Conflicts(files) => {
                format!("Merging conflicts with {base} in {}. Nothing was changed; merge by hand or ask the agent to rebase onto {base}.", files.join(", "))
            }
        }
    }
}

/// What stands in the way of merging `wt` into its base in the project folder, if anything
/// (conflicts only show when merging).
pub fn merge_check(repo: &Path, wt: &Worktree) -> Option<MergeBlock> {
    let current = git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    if current.as_deref() != Some(wt.base.as_str()) {
        return Some(MergeBlock::NotOnBase { current });
    }
    if !git(repo, &["status", "--porcelain", "--untracked-files=no"]).unwrap_or_default().trim().is_empty() {
        return Some(MergeBlock::BaseDirty);
    }
    let uncommitted = git(&wt.path, &["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0);
    if uncommitted > 0 {
        return Some(MergeBlock::Uncommitted(uncommitted));
    }
    if ok(repo, &["merge-base", "--is-ancestor", &wt.branch, &wt.base]) {
        return Some(MergeBlock::NothingToMerge);
    }
    None
}

/// Merge the thread's branch into its base, in the project folder. `Ok(Err(..))`: it didn't,
/// and why; a merge that conflicts is undone.
pub fn merge(repo: &Path, wt: &Worktree) -> Result<std::result::Result<(), MergeBlock>> {
    if let Some(block) = merge_check(repo, wt) {
        return Ok(Err(block));
    }
    match git(repo, &["merge", "--no-edit", &wt.branch]) {
        Ok(_) => Ok(Ok(())),
        Err(e) => {
            let conflicted: Vec<String> = git(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(str::to_string).collect();
            if conflicted.is_empty() {
                return Err(e);
            }
            git(repo, &["merge", "--abort"])?;
            Ok(Err(MergeBlock::Conflicts(conflicted)))
        }
    }
}

/// The branch's commits are all in its base.
pub fn is_merged(repo: &Path, wt: &Worktree) -> bool {
    ok(repo, &["merge-base", "--is-ancestor", &wt.branch, &wt.base])
}

/// What removing a worktree would lose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Removal {
    /// Files with changes that aren't committed (gone with the folder).
    pub uncommitted: usize,
    /// Commits on the branch that its base doesn't have (kept unless the branch is deleted).
    pub unmerged: usize,
    /// The folder is already gone.
    pub missing: bool,
}

pub fn removal(repo: &Path, wt: &Worktree) -> Removal {
    let missing = wt.is_missing();
    let uncommitted = if missing { 0 } else { git(&wt.path, &["status", "--porcelain", "-uall"]).map(|s| s.lines().count()).unwrap_or(0) };
    let unmerged = if branch_exists(repo, &wt.branch) { count(repo, &["rev-list", "--count", &format!("{}..{}", wt.base, wt.branch)]) } else { 0 };
    Removal { uncommitted, unmerged, missing }
}

/// Remove the worktree's folder and, when its commits are all in the base (or
/// `delete_unmerged`), its branch. Uncommitted changes stop it unless `discard_uncommitted`.
/// Returns whether the branch was deleted.
pub fn remove(repo: &Path, wt: &Worktree, discard_uncommitted: bool, delete_unmerged: bool) -> Result<bool> {
    let check = removal(repo, wt);
    if check.uncommitted > 0 && !discard_uncommitted {
        bail!("The worktree has {} uncommitted {}.", check.uncommitted, if check.uncommitted == 1 { "change" } else { "changes" });
    }
    if !check.missing {
        let path = wt.path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if discard_uncommitted {
            args.push("--force");
        }
        args.push(&path);
        git(repo, &args)?;
    }
    let _ = git(repo, &["worktree", "prune"]);
    // Leave no empty `<repo>` folder behind (`remove_dir` leaves one with other worktrees be).
    if let Some(parent) = wt.path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    if !branch_exists(repo, &wt.branch) {
        return Ok(false);
    }
    if is_merged(repo, wt) {
        git(repo, &["branch", "-d", &wt.branch])?;
        Ok(true)
    } else if delete_unmerged {
        git(repo, &["branch", "-D", &wt.branch])?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// The checkout `dir` is in: the nearest folder up with a `.git` (a linked worktree's own
/// folder, unlike `store::project_root`), else `dir` itself.
pub fn checkout_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// The main checkout a linked worktree belongs to (`None` for anything else).
pub fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let dot_git = std::fs::read_to_string(dir.join(".git")).ok()?;
    let gitdir = PathBuf::from(dot_git.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { dir.join(gitdir) };
    // Submodules have a `.git` file too, without `commondir`.
    let relative = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    // `commondir` is relative (`../..`): resolve it by hand, keeping the path as git wrote it.
    let mut common = PathBuf::new();
    for c in gitdir.join(relative.trim()).components() {
        match c {
            Component::ParentDir => _ = common.pop(),
            Component::CurDir => {}
            other => common.push(other),
        }
    }
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf)).flatten().filter(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test run's temp folder: repos, their worktrees and remotes all live in it. Folders
    /// earlier runs left are cleared on first use.
    fn tmp() -> PathBuf {
        static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            const PREFIX: &str = "trek-worktree-tests-";
            let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            for e in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
                let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < hour_ago);
                if old && e.file_name().to_string_lossy().starts_with(PREFIX) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
            std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id()))
        })
        .clone()
    }

    fn worktrees() -> PathBuf {
        tmp().join("worktrees")
    }

    fn create(repo: &Path, hint: &str, copy: &[String]) -> Result<Worktree> {
        super::create(&worktrees(), repo, hint, copy)
    }

    /// A repo in a temp folder with one commit on `main`; nothing outside it is touched.
    fn repo(name: &str) -> PathBuf {
        let dir = tmp().join("repos").join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "commit.gpgsign", "false"]] {
            git(&dir, args).unwrap();
        }
        write(&dir, "README.md", "hello\n");
        write(&dir, "src/lib.rs", "fn a() {}\n");
        write(&dir, ".gitignore", ".env\n.env.local\nsecrets/\n");
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "init"]).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn statuses(r: &Review) -> Vec<(String, char)> {
        r.files.iter().map(|f| (f.path.clone(), f.status)).collect()
    }

    #[test]
    fn slugs_are_short_branch_safe_words() {
        assert_eq!(slug("Fix the login bug, please!"), "fix-the-login-bug-please");
        assert_eq!(slug("Ünïcode → only ascii words survive"), "n-code-only-ascii-words-survive");
        assert_eq!(slug("   "), "thread");
        assert_eq!(slug("a-really-long-description-that-goes-on-and-on-forever"), "a-really-long-description-that");
        assert!(slug(&"x".repeat(100)).len() <= 32);
    }

    #[test]
    fn a_worktree_starts_on_its_own_branch_with_the_env_files() {
        let r = repo("create");
        write(&r, ".env", "KEY=1\n");
        write(&r, "secrets/token", "abc\n");
        let copy = vec![".env".into(), ".env.local".into(), "secrets".into(), "../escape".into()];
        let wt = create(&r, "Add a verbose flag", &copy).unwrap();
        assert_eq!(wt.branch, "trek/add-a-verbose-flag");
        assert_eq!(wt.base, "main");
        assert!(wt.path.starts_with(worktrees().join(r.file_name().unwrap())));
        assert_eq!(std::fs::read_to_string(wt.path.join(".env")).unwrap(), "KEY=1\n");
        assert_eq!(std::fs::read_to_string(wt.path.join("secrets/token")).unwrap(), "abc\n");
        assert!(!wt.path.join(".env.local").exists());
        assert_eq!(git(&wt.path, &["branch", "--show-current"]).unwrap().trim(), "trek/add-a-verbose-flag");
        assert_eq!(main_checkout(&wt.path).unwrap(), r.canonicalize().unwrap());
        assert_eq!(main_checkout(&r), None);
        assert_eq!(checkout_root(&wt.path.join("src")), wt.path);
        // The same words again get a branch of their own.
        let again = create(&r, "Add a verbose flag", &[]).unwrap();
        assert_eq!(again.branch, "trek/add-a-verbose-flag-2");
        // The copied env files are ignored: the new worktree has nothing to review.
        assert!(review(&wt).unwrap().files.is_empty());
    }

    #[test]
    fn worktrees_need_a_branch_with_commits() {
        let empty = tmp().join(format!("empty-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]).unwrap();
        assert!(create(&empty, "x", &[]).unwrap_err().to_string().contains("no commits"));
        let r = repo("detached");
        git(&r, &["checkout", "-q", "--detach"]).unwrap();
        assert!(create(&r, "x", &[]).unwrap_err().to_string().contains("detached"));
    }

    #[test]
    fn review_covers_commits_and_uncommitted_and_untracked_work() {
        let r = repo("review");
        let wt = create(&r, "review", &[]).unwrap();
        // Committed on the branch.
        write(&wt.path, "src/lib.rs", "fn a() {}\nfn b() {}\n");
        write(&wt.path, "docs/new.md", "one\ntwo\n");
        commit(&wt.path, "Add b and docs").unwrap();
        // Staged, unstaged and untracked.
        std::fs::remove_file(wt.path.join("README.md")).unwrap();
        write(&wt.path, "docs/new.md", "one\ntwo\nthree\n");
        write(&wt.path, "notes.txt", "a\nb\nc\n");
        write(&wt.path, "staged.txt", "s\n");
        git(&wt.path, &["add", "staged.txt"]).unwrap();
        // The base moves on meanwhile.
        write(&r, "other.txt", "x\n");
        commit(&r, "Elsewhere").unwrap();

        let rv = review(&wt).unwrap();
        assert_eq!(
            statuses(&rv),
            [("README.md".into(), 'D'), ("docs/new.md".into(), 'A'), ("notes.txt".into(), 'U'), ("src/lib.rs".into(), 'M'), ("staged.txt".into(), 'A')]
        );
        let lib = rv.files.iter().find(|f| f.path == "src/lib.rs").unwrap();
        assert_eq!((lib.additions, lib.deletions), (1, 0));
        assert_eq!(rv.files.iter().find(|f| f.path == "notes.txt").unwrap().additions, 3);
        assert_eq!((rv.ahead, rv.behind, rv.uncommitted), (1, 1, 4));
        assert_eq!(rv.unpushed, None);
        // The base's own commit isn't the thread's change.
        assert!(!rv.files.iter().any(|f| f.path == "other.txt"));
        assert!(file_diff(&wt, &rv.merge_base, lib).contains("+fn b() {}"));
        assert_eq!(file_diff(&wt, &rv.merge_base, rv.files.iter().find(|f| f.path == "notes.txt").unwrap()), "+a\n+b\n+c");
        assert!(commit_context(&wt.path, 10_000).contains("notes.txt"));
    }

    #[test]
    fn reverting_puts_each_kind_of_change_back_to_the_base() {
        let r = repo("revert");
        let wt = create(&r, "revert", &[]).unwrap();
        write(&wt.path, "src/lib.rs", "changed\n");
        write(&wt.path, "added.rs", "new\n");
        commit(&wt.path, "Change and add").unwrap();
        std::fs::remove_file(wt.path.join("README.md")).unwrap();
        write(&wt.path, "scratch.txt", "tmp\n");
        let rv = review(&wt).unwrap();
        for c in &rv.files {
            revert_file(&wt, &rv.merge_base, c).unwrap();
        }
        assert!(review(&wt).unwrap().files.is_empty(), "{:?}", review(&wt).unwrap().files);
        assert_eq!(std::fs::read_to_string(wt.path.join("src/lib.rs")).unwrap(), "fn a() {}\n");
        assert_eq!(std::fs::read_to_string(wt.path.join("README.md")).unwrap(), "hello\n");
        assert!(!wt.path.join("added.rs").exists() && !wt.path.join("scratch.txt").exists());
    }

    #[test]
    fn merging_needs_the_base_checked_out_and_clean() {
        let r = repo("merge");
        let wt = create(&r, "merge", &[]).unwrap();
        assert_eq!(merge_check(&r, &wt), Some(MergeBlock::NothingToMerge));
        write(&wt.path, "feature.rs", "f\n");
        assert_eq!(merge(&r, &wt).unwrap(), Err(MergeBlock::Uncommitted(1)));
        commit(&wt.path, "Feature").unwrap();
        // The project folder on another branch, then with changes of its own.
        git(&r, &["switch", "-qc", "side"]).unwrap();
        assert_eq!(merge(&r, &wt).unwrap(), Err(MergeBlock::NotOnBase { current: Some("side".into()) }));
        git(&r, &["switch", "-q", "main"]).unwrap();
        write(&r, "README.md", "edited in the project folder\n");
        assert_eq!(merge(&r, &wt).unwrap(), Err(MergeBlock::BaseDirty));
        // Untracked files in the project folder don't block it.
        git(&r, &["checkout", "-q", "--", "README.md"]).unwrap();
        write(&r, "untracked.txt", "u\n");
        assert_eq!(merge(&r, &wt).unwrap(), Ok(()));
        assert!(r.join("feature.rs").exists());
        assert!(is_merged(&r, &wt));
        assert_eq!(merge_check(&r, &wt), Some(MergeBlock::NothingToMerge));
    }

    #[test]
    fn a_conflicting_merge_is_undone() {
        let r = repo("conflict");
        let wt = create(&r, "conflict", &[]).unwrap();
        write(&wt.path, "README.md", "theirs\n");
        commit(&wt.path, "Theirs").unwrap();
        write(&r, "README.md", "ours\n");
        commit(&r, "Ours").unwrap();
        assert_eq!(merge(&r, &wt).unwrap(), Err(MergeBlock::Conflicts(vec!["README.md".into()])));
        assert_eq!(std::fs::read_to_string(r.join("README.md")).unwrap(), "ours\n");
        assert!(git(&r, &["status", "--porcelain"]).unwrap().trim().is_empty());
    }

    #[test]
    fn removing_keeps_unmerged_work_unless_told_otherwise() {
        let r = repo("remove");
        // Uncommitted work stops it.
        let wt = create(&r, "dirty", &[]).unwrap();
        write(&wt.path, "wip.txt", "wip\n");
        assert_eq!(removal(&r, &wt), Removal { uncommitted: 1, unmerged: 0, missing: false });
        assert!(remove(&r, &wt, false, false).is_err());
        assert!(wt.path.exists());
        // Discarded on request; nothing unmerged, so the branch goes too.
        assert!(remove(&r, &wt, true, false).unwrap());
        assert!(!wt.path.exists() && !branch_exists(&r, &wt.branch));

        // Unmerged commits: the folder goes, the branch stays.
        let wt = create(&r, "unmerged", &[]).unwrap();
        write(&wt.path, "work.txt", "w\n");
        commit(&wt.path, "Work").unwrap();
        assert_eq!(removal(&r, &wt).unmerged, 1);
        assert!(!remove(&r, &wt, false, false).unwrap());
        assert!(!wt.path.exists() && branch_exists(&r, &wt.branch));
        // Brought back from its branch, with its commit.
        assert!(wt.is_missing());
        add(&r, &wt, &[]).unwrap();
        assert!(!wt.is_missing() && wt.path.join("work.txt").exists());
        // Deleting the unmerged branch takes a yes.
        assert!(remove(&r, &wt, false, true).unwrap());
        assert!(!branch_exists(&r, &wt.branch));
    }

    #[test]
    fn a_vanished_worktree_is_missing_and_comes_back() {
        let r = repo("missing");
        let wt = create(&r, "missing", &[]).unwrap();
        write(&r, ".env", "E=1\n");
        std::fs::remove_dir_all(&wt.path).unwrap();
        assert!(wt.is_missing());
        assert_eq!(removal(&r, &wt), Removal { uncommitted: 0, unmerged: 0, missing: true });
        add(&r, &wt, &default_copy()).unwrap();
        assert!(!wt.is_missing());
        assert!(wt.path.join(".env").exists());
        // Gone with its branch: it starts again from the base.
        std::fs::remove_dir_all(&wt.path).unwrap();
        git(&r, &["worktree", "prune"]).unwrap();
        git(&r, &["branch", "-D", &wt.branch]).unwrap();
        add(&r, &wt, &[]).unwrap();
        assert_eq!(git(&wt.path, &["branch", "--show-current"]).unwrap().trim(), wt.branch);
        // A missing folder can still be removed: just its branch is tidied.
        std::fs::remove_dir_all(&wt.path).unwrap();
        assert!(remove(&r, &wt, false, false).unwrap());
    }

    #[test]
    fn pushing_needs_an_origin() {
        let r = repo("push");
        let wt = create(&r, "push", &[]).unwrap();
        assert!(push(&wt).unwrap_err().to_string().contains("no origin"));
        assert!(!github_origin(&r));
        // A local bare repo stands in for the remote.
        let remote = tmp().join(format!("remote-{}.git", uuid::Uuid::new_v4().simple()));
        git(&r, &["init", "-q", "--bare", &remote.to_string_lossy()]).unwrap();
        git(&r, &["remote", "add", "origin", &remote.to_string_lossy()]).unwrap();
        write(&wt.path, "x.txt", "x\n");
        commit(&wt.path, "X").unwrap();
        push(&wt).unwrap();
        assert_eq!(review(&wt).unwrap().unpushed, Some(0));
        assert!(git(&remote, &["show-ref", "--verify", "refs/heads/trek/push"]).is_ok());
        assert_eq!(commit_subjects(&wt), ["X"]);
    }
}
