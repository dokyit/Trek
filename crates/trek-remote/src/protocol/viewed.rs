//! Files marked viewed in a diff review (feature `diff_viewed`), the marks the Mac's Git panel and
//! diff tick: a thread's (or, with no thread, a project folder's), each kept with what its diff
//! said when it was marked, so a file that changes again comes back unviewed. `git_status` lists
//! the marked files that haven't changed since (`GitStatus::viewed`); `set_viewed` ticks one or
//! clears it and answers with `git_status` as it is then.

use super::GitTarget;
use serde::{Deserialize, Serialize};

/// `set_viewed`: mark `path` (one of the folder's changed files) viewed at its diff now, or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetViewedRequest {
    #[serde(flatten)]
    pub target: GitTarget,
    pub path: String,
    pub viewed: bool,
}
