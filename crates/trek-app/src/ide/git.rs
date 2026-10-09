//! Git for the IDE's own reads and writes (Source Control, diff tabs): a command in the folder
//! that never takes an optional lock an agent's own git would then wait on, never prompts, finds
//! git on the login PATH (a Finder launch has a bare one), and keeps the user's diff settings
//! (no prefix, colour, an external diff, fsmonitor) out of what Trek parses.

use std::path::Path;
use std::process::Command;

pub(crate) fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .env("PATH", trek_core::detect::login_path())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        // Deaf to git variables Trek may have inherited (as `trek_core::git::read_only` is); unlike
        // it, hooks stay on: a commit made here is the user's own.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_EXTERNAL_DIFF")
        .args(["--literal-pathspecs", "-c", "core.quotepath=off", "-c", "diff.noprefix=false", "-c", "core.fsmonitor=false", "-c", "color.ui=false"]);
    c
}

/// Run git in `dir`: its output, or what it said on failure.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(dir).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { err })
    }
}

/// The repository's top folder holding `dir`, named the way `dir` is (`/var/…`, not git's
/// `/private/var/…`), so paths built on it match the editor's.
pub(crate) fn top(dir: &Path) -> Option<std::path::PathBuf> {
    let real = run(dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| std::path::PathBuf::from(s.trim())).filter(|p| !p.as_os_str().is_empty())?;
    Some(crate::workspace::as_given(&real, dir))
}
