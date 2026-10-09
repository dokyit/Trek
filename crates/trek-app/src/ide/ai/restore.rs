//! What putting the files back to a message's checkpoint would change, for the AI side bar's
//! edit, rewind, undo and retry: worked out off the main thread, then listed (as the harness's
//! composer and transcript list it) so the user picks whether files go back too.

use std::path::PathBuf;
use trek_core::checkpoint::{Change, FileChange};

/// Files a tooltip or a card lists before "and N more".
pub(crate) const LISTED: usize = 8;

/// The files restoring would change.
#[derive(Debug, Clone)]
pub(crate) enum Files {
    Checking,
    Changes(Vec<FileChange>),
    Failed(String),
    /// No checkpoint to put them back from (why).
    Unavailable(String),
}

impl Files {
    /// Nothing to put back: the files are as they were, or can't be.
    pub(crate) fn nothing(&self) -> bool {
        match self {
            Files::Unavailable(_) => true,
            Files::Changes(c) => c.iter().all(|f| !acts(f)),
            Files::Checking | Files::Failed(_) => false,
        }
    }
}

/// A change restoring makes (nested repositories and newer files are left as they are).
pub(crate) fn acts(f: &FileChange) -> bool {
    !matches!(f.change, Change::Nested | Change::Kept)
}

/// What restoring does to `f`: "revert", "delete", "bring back", "left as is".
pub(crate) fn verb(f: &FileChange) -> &'static str {
    match f.change {
        Change::Modified => "revert",
        Change::Added => "delete",
        Change::Deleted => "bring back",
        Change::Nested | Change::Kept => "left as is",
    }
}

/// "Restore 3 files", and a tooltip listing them.
pub(crate) fn summary(changes: &[FileChange]) -> (String, String) {
    let acting: Vec<&FileChange> = changes.iter().filter(|f| acts(f)).collect();
    let mut tip = String::from("Put the files back as they were when this message was first sent:");
    for f in acting.iter().take(LISTED) {
        tip.push_str(&format!("\n{} {}", verb(f), f.path));
    }
    if acting.len() > LISTED {
        tip.push_str(&format!("\nand {} more", acting.len() - LISTED));
    }
    let n = acting.len();
    (format!("Restore {n} file{}", if n == 1 { "" } else { "s" }), tip)
}

/// The checkpoint `item` of `thread` would restore from (its repository and commit), or why
/// there's none.
pub(crate) fn checkpoint(ws: &crate::workspace::Workspace, thread: &str, item: &str) -> Result<(PathBuf, String), String> {
    match ws.restorable_checkpoint(thread, item) {
        Some(c) => Ok((c.repo, c.sha)),
        None => Err(ws.no_checkpoint(thread, item).unwrap_or(crate::workspace::NoCheckpoint::Missing).explain().to_string()),
    }
}

/// What restoring to `sha` of `repo` changes (run off the main thread).
pub(crate) fn read(repo: PathBuf, sha: String) -> Files {
    let result = trek_core::checkpoint::Repo::find(&repo).ok_or_else(|| anyhow::anyhow!("{} isn't a git repository any more", repo.display())).and_then(|r| r.changes_since(&sha));
    match result {
        Ok(changes) => Files::Changes(changes),
        Err(e) => Files::Failed(format!("{e:#}")),
    }
}
