//! What the inbox needs to know from git: which branch a thread's work is on, and whether that
//! branch has been merged (`inbox.auto_settle_on_merge`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// There is no git to run. Its text is what the user is told, so it says how to get one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitMissing;

impl GitMissing {
    #[cfg(windows)]
    pub const MESSAGE: &'static str = "Trek needs Git. Install Git for Windows (https://git-scm.com/download/win) or `winget install Git.Git`, then restart Trek.";
    #[cfg(target_os = "macos")]
    pub const MESSAGE: &'static str = "Trek needs Git. Install the Xcode command line tools (`xcode-select --install`) or `brew install git`, then restart Trek.";
    #[cfg(not(any(windows, target_os = "macos")))]
    pub const MESSAGE: &'static str =
        "Trek needs Git. Install it with your package manager, then restart Trek.";
}

impl std::fmt::Display for GitMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::MESSAGE)
    }
}

impl std::error::Error for GitMissing {}

/// The git Trek runs: the first on the login PATH, looked up once. Callers that can report an
/// error use this and say [`GitMissing`]; a restart is what picks up a git installed since.
pub fn git_binary() -> Result<&'static Path, GitMissing> {
    static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    BIN.get_or_init(|| crate::detect::which("git"))
        .as_deref()
        .ok_or(GitMissing)
}

/// What to run for git: [`git_binary`], or plain `git` when there is none (spawning it then fails
/// as not found, which [`run_error`] turns into the message). For building a `Command`.
pub fn git_program() -> &'static Path {
    git_binary().unwrap_or(Path::new("git"))
}

/// A failure to start git, as the user should hear it: [`GitMissing`] when there is no git, else
/// the error under "couldn't run git".
pub fn run_error(e: std::io::Error) -> anyhow::Error {
    explain(e, git_binary().is_ok())
}

fn explain(e: std::io::Error, found: bool) -> anyhow::Error {
    if found {
        anyhow::Error::new(e).context("couldn't run git")
    } else {
        anyhow::Error::new(GitMissing)
    }
}

/// Settings a repository's own config can't override in Trek's read-only calls: nothing from the
/// folder runs (fsmonitor, hooks), and diffs keep the `a/` `b/` prefixes Trek parses.
const READ_ONLY_CONFIG: [&str; 8] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "diff.noprefix=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "core.splitIndex=false",
];

/// The `core.hooksPath` setting that keeps every hook from running. Where there is a null device
/// to point at, that; on Windows git doesn't take `/dev/null` for a folder, so an empty folder of
/// Trek's own in the temp dir (made once per process; no files in it, so no hook to run).
fn no_hooks() -> &'static str {
    static SETTING: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SETTING.get_or_init(|| {
        let dir = std::env::temp_dir().join("trek-no-hooks");
        let value = if cfg!(windows) && std::fs::create_dir_all(&dir).is_ok() {
            // Forward slashes: backslashes in a `-c` value are escapes to git.
            dir.to_string_lossy().replace('\\', "/")
        } else {
            "/dev/null".to_string()
        };
        format!("core.hooksPath={value}")
    })
}

/// A git command for questions that change nothing, run in `dir`: no lock an agent's own git
/// would then wait on (`GIT_OPTIONAL_LOCKS=0`), no prompt, no fsmonitor or hook from the folder's
/// config, English output, the login shell's PATH, and no `GIT_*` from Trek's environment
/// pointing it at another repository. Add the subcommand and its arguments; use
/// `tokio::process::Command::from` for an async one.
pub fn read_only(dir: &Path) -> Command {
    let mut c = crate::procs::command(git_program());
    c.current_dir(dir)
        .env("PATH", crate::detect::login_path())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_EXTERNAL_DIFF")
        .args(["-c", no_hooks()])
        .args(READ_ONLY_CONFIG)
        .stdin(Stdio::null());
    c
}

/// `git <args>` through [`read_only`]: its trimmed output, when it succeeds.
pub fn read(dir: &Path, args: &[&str]) -> Option<String> {
    let out = read_only(dir)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    read(cwd, args)
}

/// The repository's default branch: origin's HEAD, else a local main, master or trunk.
fn default_branch(cwd: &Path) -> Option<String> {
    if let Some(remote) = git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return remote.split_once('/').map(|(_, b)| b.to_string());
    }
    let local = git(cwd, &["branch", "--format=%(refname:short)"])?;
    ["main", "master", "trunk"]
        .into_iter()
        .find(|b| local.lines().any(|l| l == *b))
        .map(str::to_string)
}

/// The default branch's refs that exist here: the local branch and origin's copy. Work merged on
/// the remote shows up in `origin/<default>` after a fetch, before the local branch is pulled.
fn default_refs(cwd: &Path) -> Option<(String, Vec<String>)> {
    let default = default_branch(cwd)?;
    let refs: Vec<String> = [
        format!("refs/heads/{default}"),
        format!("refs/remotes/origin/{default}"),
    ]
    .into_iter()
    .filter(|r| git(cwd, &["rev-parse", "--verify", "--quiet", r]).is_some())
    .collect();
    (!refs.is_empty()).then_some((default, refs))
}

/// Commits on `branch` that `base` doesn't have.
fn ahead(cwd: &Path, base: &str, branch: &str) -> Option<u64> {
    git(
        cwd,
        &[
            "rev-list",
            "--count",
            &format!("{base}..refs/heads/{branch}"),
        ],
    )?
    .parse()
    .ok()
}

/// The branch checked out in `cwd` when it holds commits of its own: not the default branch, and
/// not merged into it yet. `None` on the default branch, a detached HEAD, or outside a repository.
pub fn unmerged_branch(cwd: &Path) -> Option<String> {
    let branch = git(cwd, &["branch", "--show-current"]).filter(|b| !b.is_empty())?;
    let (default, refs) = default_refs(cwd)?;
    if branch == default {
        return None;
    }
    refs.iter()
        .all(|r| ahead(cwd, r, &branch).is_some_and(|n| n > 0))
        .then_some(branch)
}

/// `unmerged_branch`, if its last commit was made at or after `since` (seconds since the epoch):
/// a branch a turn that began then committed to, rather than one the folder just happened to be on.
pub fn branch_committed_since(cwd: &Path, since: i64) -> Option<String> {
    let branch = unmerged_branch(cwd)?;
    let at: i64 = git(
        cwd,
        &["log", "-1", "--format=%ct", &format!("refs/heads/{branch}")],
    )?
    .parse()
    .ok()?;
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
    if git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_none()
    {
        return BranchState::Gone;
    }
    let Some((default, refs)) = default_refs(cwd) else {
        return BranchState::Gone;
    };
    if branch != default && refs.iter().any(|r| ahead(cwd, r, branch) == Some(0)) {
        BranchState::Merged
    } else {
        BranchState::Unmerged
    }
}

/// What a failed `git` (or `gh`) said, as one plain sentence the user can act on, for the failures
/// people meet most: no remote, a login that was turned down, a push the remote has moved past, a
/// pull that would need a merge, a hook, conflicts, no identity. `None` for anything else (the
/// caller then shows git's own `fatal:` line). The full text stays reachable from the toast.
pub fn explain_failure(stderr: &str) -> Option<&'static str> {
    let s = stderr.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
    // Hooks first: a hook's own output can say anything, but git names the hook it ran.
    if has(&[
        "pre-commit hook",
        "commit-msg hook",
        "pre-push hook",
        "hook declined",
        "hook failed",
    ]) || s.contains("husky")
    {
        Some("A hook turned it down.")
    } else if has(&[
        "gh auth login",
        "not logged in to any github hosts",
        "authentication required",
    ]) {
        Some("The GitHub CLI isn't signed in. Run gh auth login in a terminal, then try again.")
    } else if has(&[
        "could not read username",
        "could not read password",
        "authentication failed",
        "permission denied (publickey)",
        "terminal prompts disabled",
        "invalid username or password",
        "repository not found",
        "access denied",
    ]) {
        Some(
            "The remote turned your login down. Sign in with a credential helper or an SSH key, then try again.",
        )
    } else if has(&[
        "does not appear to be a git repository",
        "no configured push destination",
        "no such remote",
        "no remote repository specified",
    ]) {
        Some("This project has no remote to push to. Add one with git remote add origin <url>.")
    } else if has(&[
        "non-fast-forward",
        "fetch first",
        "tip of your current branch is behind",
    ]) {
        Some("The remote has commits you don't have. Pull first, then push again.")
    } else if has(&[
        "not possible to fast-forward",
        "diverging branches",
        "need to specify how to reconcile",
    ]) {
        Some(
            "Your branch and its upstream have both moved, so pulling needs a merge. Ask the agent to merge, or do it in a terminal.",
        )
    } else if has(&[
        "no tracking information",
        "has no upstream branch",
        "no upstream configured",
    ]) {
        Some("This branch isn't tracking a remote branch yet. Push it first.")
    } else if has(&[
        "automatic merge failed",
        "unmerged files",
        "fix conflicts",
        "conflict (",
    ]) {
        Some("There are merge conflicts. Resolve them first.")
    } else if has(&[
        "author identity unknown",
        "please tell me who you are",
        "empty ident name",
    ]) {
        Some("Git doesn't know who you are. Set user.name and user.email, then commit again.")
    } else if s.contains("not currently on a branch") {
        Some("The folder isn't on a branch (detached HEAD). Switch to a branch first.")
    } else if has(&["index.lock", "another git process"]) {
        Some("Another git command is running in this folder. Try again in a moment.")
    } else if has(&["nothing to commit", "no changes added to commit"]) {
        Some("There's nothing to commit.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_failures_people_meet_are_said_in_one_plain_sentence() {
        let cases: [(&str, &str); 9] = [
            (
                "fatal: 'origin' does not appear to be a git repository\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.",
                "no remote",
            ),
            (
                "fatal: No configured push destination.\nEither specify the URL from the command-line or configure a remote repository using",
                "no remote",
            ),
            (
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                "login",
            ),
            (
                "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
                "login",
            ),
            (
                "! [rejected]        main -> main (fetch first)\nerror: failed to push some refs",
                "Pull first",
            ),
            (
                "hint: Diverging branches can't be fast-forwarded, you need to either:\nfatal: Not possible to fast-forward, aborting.",
                "merge",
            ),
            (
                "fatal: There is no tracking information for the current branch.",
                "tracking",
            ),
            (
                "To get started with GitHub CLI, please run:  gh auth login",
                "GitHub CLI",
            ),
            (
                "Author identity unknown\n\n*** Please tell me who you are.",
                "who you are",
            ),
        ];
        for (said, word) in cases {
            let plain = explain_failure(said).unwrap_or_else(|| panic!("no sentence for {said}"));
            assert!(plain.contains(word), "{said:?} -> {plain}");
            assert!(!plain.contains('\n'));
        }
        // A hook's refusal points at its output; plain git failures it doesn't know stay with git's line.
        assert!(
            explain_failure("husky - pre-commit hook exited with code 1")
                .unwrap()
                .contains("hook")
        );
        assert!(
            explain_failure("error: pathspec 'x' did not match any file(s) known to git").is_none()
        );
    }

    #[test]
    fn no_git_is_one_clear_message() {
        let gone = || std::io::Error::from(std::io::ErrorKind::NotFound);
        let said = explain(gone(), false);
        assert_eq!(said.to_string(), GitMissing::MESSAGE);
        assert!(
            said.downcast_ref::<GitMissing>().is_some(),
            "callers can tell it apart"
        );
        // Any way of not starting git, when there is none, is the same message.
        assert_eq!(
            explain(
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                false
            )
            .to_string(),
            GitMissing::MESSAGE
        );
        // With git found, a failure to start it is its own.
        let other = explain(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            true,
        );
        assert_eq!(other.to_string(), "couldn't run git");
        assert!(other.downcast_ref::<GitMissing>().is_none());
        #[cfg(windows)]
        assert_eq!(
            GitMissing.to_string(),
            "Trek needs Git. Install Git for Windows (https://git-scm.com/download/win) or `winget install Git.Git`, then restart Trek."
        );
        #[cfg(target_os = "macos")]
        assert!(GitMissing.to_string().contains("xcode-select --install"));
    }

    #[test]
    fn git_is_found_where_the_tests_run() {
        // The suite needs git anyway; what is found is a file, and what `read_only` runs.
        let found = git_binary().expect("git on PATH");
        assert!(found.is_file(), "{}", found.display());
        assert_eq!(Command::new(git_program()).get_program(), found.as_os_str());
        assert_eq!(read_only(Path::new(".")).get_program(), found.as_os_str());
    }

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
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
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
        assert_eq!(
            branch_state(&dir, "main"),
            BranchState::Unmerged,
            "the default branch is never 'merged'"
        );
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
        let (origin, clone) = (
            tmp.join(format!("trek-git-origin-{}.git", std::process::id())),
            tmp.join(format!("trek-git-clone-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&origin);
        let _ = std::fs::remove_dir_all(&clone);
        run(
            &tmp,
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        run(
            &tmp,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(
            branch_committed_since(&dir, now - 60).as_deref(),
            Some("feature")
        );
        assert_eq!(
            branch_committed_since(&dir, now + 60),
            None,
            "on the branch, but nothing committed since"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_only_calls_ignore_the_folders_fsmonitor_and_prefix_settings() {
        let dir = repo("untrusted");
        let marker = dir.join("fsmonitor-ran");
        // A repository whose config would run a command on every status, and drop diff prefixes.
        run(
            &dir,
            &[
                "config",
                "core.fsmonitor",
                &format!("touch {}; false", marker.display()),
            ],
        );
        run(&dir, &["config", "diff.noprefix", "true"]);
        std::fs::write(dir.join("first.txt"), "changed").unwrap();
        let status = read(&dir, &["status", "--porcelain"]).unwrap();
        assert!(status.contains("first.txt"));
        assert!(!marker.exists(), "fsmonitor ran");
        let diff = read(&dir, &["diff"]).unwrap();
        assert!(diff.contains("--- a/first.txt"), "{diff}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_only_calls_run_none_of_the_folders_hooks() {
        let dir = repo("hooks");
        let hook = dir.join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // The hook does turn a plain commit down...
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        run(&dir, &["add", "."]);
        let plain = Command::new("git")
            .args(["commit", "-q", "-m", "a"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            !plain.status.success(),
            "the hook didn't run for a plain commit"
        );
        // ...and doesn't through Trek's wrapper.
        let out = read_only(&dir)
            .args(["commit", "-q", "-m", "a"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            read(&dir, &["log", "-1", "--format=%s"]).as_deref(),
            Some("a")
        );
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
