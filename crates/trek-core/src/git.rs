//! What the inbox needs to know from git: which branch a thread's work is on, and whether that
//! branch has been merged (`inbox.auto_settle_on_merge`).

use std::path::Path;
use std::process::Command;

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(cwd).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The repository's default branch: origin's HEAD, else a local main, master or trunk.
fn default_branch(cwd: &Path) -> Option<String> {
    if let Some(remote) = git(cwd, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return remote.split_once('/').map(|(_, b)| b.to_string());
    }
    let local = git(cwd, &["branch", "--format=%(refname:short)"])?;
    ["main", "master", "trunk"].into_iter().find(|b| local.lines().any(|l| l == *b)).map(str::to_string)
}

/// The default branch's refs that exist here: the local branch and origin's copy. Work merged on
/// the remote shows up in `origin/<default>` after a fetch, before the local branch is pulled.
fn default_refs(cwd: &Path) -> Option<(String, Vec<String>)> {
    let default = default_branch(cwd)?;
    let refs: Vec<String> = [format!("refs/heads/{default}"), format!("refs/remotes/origin/{default}")]
        .into_iter()
        .filter(|r| git(cwd, &["rev-parse", "--verify", "--quiet", r]).is_some())
        .collect();
    (!refs.is_empty()).then_some((default, refs))
}

/// Commits on `branch` that `base` doesn't have.
fn ahead(cwd: &Path, base: &str, branch: &str) -> Option<u64> {
    git(cwd, &["rev-list", "--count", &format!("{base}..refs/heads/{branch}")])?.parse().ok()
}

/// The branch checked out in `cwd` when it holds commits of its own: not the default branch, and
/// not merged into it yet. `None` on the default branch, a detached HEAD, or outside a repository.
pub fn unmerged_branch(cwd: &Path) -> Option<String> {
    let branch = git(cwd, &["branch", "--show-current"]).filter(|b| !b.is_empty())?;
    let (default, refs) = default_refs(cwd)?;
    if branch == default {
        return None;
    }
    refs.iter().all(|r| ahead(cwd, r, &branch).is_some_and(|n| n > 0)).then_some(branch)
}

/// `unmerged_branch`, if its last commit was made at or after `since` (seconds since the epoch):
/// a branch a turn that began then committed to, rather than one the folder just happened to be on.
pub fn branch_committed_since(cwd: &Path, since: i64) -> Option<String> {
    let branch = unmerged_branch(cwd)?;
    let at: i64 = git(cwd, &["log", "-1", "--format=%ct", &format!("refs/heads/{branch}")])?.parse().ok()?;
    (at >= since).then_some(branch)
}

/// Where a branch `unmerged_branch` reported stands now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchState {
    Unmerged,
    /// Every commit on it is in the default branch, locally or on origin.
    Merged,
    /// Deleted (often after a squash merge), or the folder is no longer a repository. Nothing
    /// tells merged from abandoned work any more, so it isn't worth checking again.
    Gone,
}

pub fn branch_state(cwd: &Path, branch: &str) -> BranchState {
    if git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_none() {
        return BranchState::Gone;
    }
    let Some((default, refs)) = default_refs(cwd) else { return BranchState::Gone };
    if branch != default && refs.iter().any(|r| ahead(cwd, r, branch) == Some(0)) { BranchState::Merged } else { BranchState::Unmerged }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        run(&dir, &["config", "user.email", "test@example.com"]);
        run(&dir, &["config", "user.name", "Test"]);
        run(&dir, &["config", "commit.gpgsign", "false"]);
        commit(&dir, "first");
        dir
    }

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").args(args).current_dir(dir).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    }

    fn commit(dir: &Path, msg: &str) {
        std::fs::write(dir.join(format!("{msg}.txt")), msg).unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn a_branch_counts_once_it_has_commits_and_is_merged_once_main_has_them() {
        let dir = repo("merge");
        // On the default branch, or a branch with nothing of its own yet: nothing to track.
        assert_eq!(unmerged_branch(&dir), None);
        run(&dir, &["checkout", "-q", "-b", "feature"]);
        assert_eq!(unmerged_branch(&dir), None);
        assert_eq!(branch_state(&dir, "main"), BranchState::Unmerged, "the default branch is never 'merged'");
        commit(&dir, "work");
        assert_eq!(unmerged_branch(&dir).as_deref(), Some("feature"));
        assert_eq!(branch_state(&dir, "feature"), BranchState::Unmerged);
        // More work on main doesn't merge it.
        run(&dir, &["checkout", "-q", "main"]);
        commit(&dir, "elsewhere");
        assert_eq!(branch_state(&dir, "feature"), BranchState::Unmerged);
        run(&dir, &["merge", "-q", "--no-edit", "feature"]);
        assert_eq!(branch_state(&dir, "feature"), BranchState::Merged);
        // Deleted after the merge: can't tell, and not worth asking again.
        run(&dir, &["branch", "-q", "-d", "feature"]);
        assert_eq!(branch_state(&dir, "feature"), BranchState::Gone);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_merge_on_origin_counts_before_main_is_pulled() {
        let src = repo("src");
        let tmp = std::env::temp_dir();
        let (origin, clone) = (tmp.join(format!("trek-git-origin-{}.git", std::process::id())), tmp.join(format!("trek-git-clone-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&origin);
        let _ = std::fs::remove_dir_all(&clone);
        run(&tmp, &["clone", "-q", "--bare", src.to_str().unwrap(), origin.to_str().unwrap()]);
        run(&tmp, &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()]);
        run(&clone, &["config", "user.email", "test@example.com"]);
        run(&clone, &["config", "user.name", "Test"]);
        run(&clone, &["config", "commit.gpgsign", "false"]);
        run(&clone, &["checkout", "-q", "-b", "feature"]);
        commit(&clone, "work");
        assert_eq!(unmerged_branch(&clone).as_deref(), Some("feature"));
        // The feature lands on origin's main (merged there); the local main is behind.
        run(&clone, &["push", "-q", "origin", "feature:main"]);
        run(&clone, &["fetch", "-q", "origin"]);
        assert_eq!(ahead(&clone, "refs/heads/main", "feature"), Some(1));
        assert_eq!(branch_state(&clone, "feature"), BranchState::Merged);
        for dir in [clone, origin, src] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_branch_counts_for_a_turn_only_if_committed_to_since_it_began() {
        let dir = repo("since");
        run(&dir, &["checkout", "-q", "-b", "feature"]);
        commit(&dir, "work");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(branch_committed_since(&dir, now - 60).as_deref(), Some("feature"));
        assert_eq!(branch_committed_since(&dir, now + 60), None, "on the branch, but nothing committed since");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn outside_a_repository_there_is_nothing() {
        let dir = std::env::temp_dir().join(format!("trek-git-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unmerged_branch(&dir), None);
        assert_eq!(branch_state(&dir, "feature"), BranchState::Gone);
        let _ = std::fs::remove_dir_all(dir);
    }
}
