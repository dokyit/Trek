//! The live cards a thread's agent leaves in the transcript for work it pushed out (the Mac's
//! `pr_card`): a pull request it opened, watches or merges, a workflow run it watches, a release.
//! A card is an item of its own (`kind: "card"`, [`super::ItemBody::Card`]) and is sent again,
//! under the same item id, whenever it moves on: its phase, its checks or jobs, a merge in flight.
//! Older phones leave the kind out, as any they don't know.
//!
//! What the card offers the user (Merge, Resolve conflicts, Re-run) goes back as `card_action`,
//! which a Mac lists in `features` ([`super::feature::CARD_ACTIONS`]); the card says which of them
//! make sense right now in [`RemoteCard::actions`].

use serde::{Deserialize, Serialize};

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// A card, as the Mac draws it: everything the phone needs to draw it the same way, and to keep
/// its clocks (how long a run has taken) going between updates.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteCard {
    /// Stable for the card in its thread (a PR's url, `release-<version>`, a run's id): what
    /// `card_action` names, and what the composer's chip follows.
    pub key: String,
    pub kind: CardKind,
    /// Where it stands.
    pub phase: CardPhase,
    /// The PR's title, "Release v<version>", or a run's workflow name.
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// Where it opens (the PR, the run, the release's run).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Branch it comes from and goes to (a run's or release's branch is `head`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub head: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base: String,
    /// `owner/repo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<u32>,
    /// A pull request's newest commits, newest first (a few: the count says how many in all).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<CardCommit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// A pull request's CI checks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CardCheck>,
    /// Conflicting paths, while it doesn't merge cleanly; `resolved` ones clear as the agent
    /// fixes them (`resolving`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolved: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resolving: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub merging: bool,
    /// The merge deleted the head branch.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub branch_deleted: bool,
    /// What's happening, or what went wrong, in a few words ("Pushing 3 commits to origin").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub step: String,
    /// Releases: the version (without its `v`) and channel (`stable`, `nightly`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel: String,
    /// Runs' and releases' jobs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<CardStage>,
    /// Runs: what the run is for (its commit's subject) and what started it (`push`,
    /// `pull_request`, `workflow_dispatch`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub event: String,
    /// When it was made, when its phase last changed, and when the run started and ended (ms;
    /// 0 until known).
    #[serde(skip_serializing_if = "is_zero")]
    pub created_ms: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub phase_at_ms: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub started_ms: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub ended_ms: i64,
    /// What `card_action` may ask of it now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<CardActionKind>,
    /// A published release's page (its notes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
}

impl RemoteCard {
    /// The card in one line, for where it isn't drawn: "Pull request #128 · Merged".
    pub fn summary(&self) -> String {
        let phase = self.phase.label();
        match self.kind {
            CardKind::Pr => match self.number {
                Some(n) => format!("Pull request #{n} · {phase}"),
                None => format!("Pull request · {phase}"),
            },
            CardKind::Release => format!("Release {} · {phase}", self.version),
            CardKind::Run | CardKind::Other => {
                let name = if self.title.is_empty() {
                    "Workflow"
                } else {
                    &self.title
                };
                format!("{name} run · {phase}")
            }
        }
    }
}

impl CardPhase {
    /// As the Mac's badge says it.
    pub fn label(self) -> &'static str {
        match self {
            CardPhase::Creating => "Creating",
            CardPhase::Open => "Open",
            CardPhase::Checking => "Checking",
            CardPhase::Conflicted => "Conflicts",
            CardPhase::Ready => "Ready to merge",
            CardPhase::Merging => "Merging",
            CardPhase::Merged => "Merged",
            CardPhase::Closed => "Closed",
            CardPhase::Releasing => "Releasing",
            CardPhase::Published => "Published",
            CardPhase::Passed => "Passed",
            CardPhase::Failed => "Failed",
            CardPhase::Other => "Updated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    #[default]
    Pr,
    Release,
    Run,
    /// A kind this build doesn't know.
    #[serde(other)]
    Other,
}

/// As the Mac's `pr_card::Phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardPhase {
    /// `gh pr create` is in flight.
    #[default]
    Creating,
    Open,
    /// Mergeability is being worked out, or checks are pending.
    Checking,
    Conflicted,
    /// Mergeable, nothing pending.
    Ready,
    Merging,
    Merged,
    Closed,
    /// A release under way.
    Releasing,
    Published,
    /// A run finished green.
    Passed,
    Failed,
    /// A phase this build doesn't know.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardCheckState {
    #[default]
    Queued,
    Running,
    Pass,
    Fail,
    /// Skipped, neutral or cancelled.
    #[serde(other)]
    Skipped,
}

/// One CI check of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardCheck {
    pub name: String,
    pub state: CardCheckState,
}

/// One job of a run or release, with when it started and ended (ms; 0 until known).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardStage {
    pub label: String,
    pub state: CardCheckState,
    #[serde(skip_serializing_if = "is_zero")]
    pub started_ms: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub ended_ms: i64,
}

/// One commit of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CardCommit {
    pub title: String,
    /// Its short hash.
    pub oid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub committed_ms: i64,
}

/// What a card's buttons do, as on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardActionKind {
    /// Merge the pull request (`gh pr merge --merge --delete-branch`).
    Merge,
    /// Ask the thread's agent to resolve the pull request's conflicts.
    Resolve,
    /// Re-run a failed run's (or release workflow's) failed jobs.
    Rerun,
}

/// `card_action`: press one of a card's buttons. `key` is [`RemoteCard::key`]; a card that no
/// longer offers `action` (it moved on meanwhile) is refused with `conflict`. Acked once the Mac
/// has started it; the card's next update shows how it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardActionRequest {
    pub thread_id: String,
    pub key: String,
    pub action: CardActionKind,
}
