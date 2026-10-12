//! What the phone gained to keep up with the Mac: Codex's rate-limit resets, the follow-ups
//! queued while a turn runs, a message's images, what an agent runs in the background, and the
//! icons of added agents and projects. A Mac that does these lists their names in
//! `welcome`/`paired` `features` ([`super::feature`]); a phone hides them on a Mac that doesn't.

use serde::{Deserialize, Serialize};

/// `bytes` as the wire carries them: base64.
pub fn base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A rate-limit reset Codex granted and the user hasn't spent ("Usage limit resets").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetCredit {
    pub id: String,
    /// "Full reset (Weekly + 5 hr)".
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When it lapses (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// A follow-up waiting to go, in a thread's row: after the running turn, or (`reset`) once a
/// usage limit resets.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub images: u32,
    /// It waits for the thread's usage limit to reset, not for a turn to end.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

/// `queue_action`: change or drop one of a thread's queued follow-ups. `index` counts among
/// those with the same `reset`; `text` is the message as the phone shows it, so one that has
/// gone out meanwhile (another took its place) is refused with `conflict`. A removed message
/// comes back in `ack.text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueActionRequest {
    pub thread_id: String,
    pub index: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
    pub text: String,
    pub action: QueueAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueAction {
    /// Don't send it.
    Remove,
    /// Send this instead, in its place.
    Edit { text: String },
}

/// `attachment`: image `index` of a message (`item_id`, a `user` item), as a JPEG that fits in
/// `max_px` (the Mac caps it at [`MAX_ATTACHMENT_PX`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRequest {
    pub thread_id: String,
    pub item_id: String,
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_px: Option<u32>,
}

/// The longest side of an image the Mac sends.
pub const MAX_ATTACHMENT_PX: u32 = 2048;

/// The reply to `attachment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub thread_id: String,
    pub item_id: String,
    pub index: u32,
    /// `image/jpeg`.
    pub mime: String,
    /// Base64.
    pub data: String,
    /// As sent (after scaling down).
    pub width: u32,
    pub height: u32,
}

/// Something a thread's agent runs in the background: a dev server, a watcher (not its own
/// sub-agents, which are `sub_agents`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTask {
    /// The agent's id for it.
    pub id: String,
    pub kind: BackgroundKind,
    /// The command, or what it is.
    pub title: String,
    /// The last line it printed, as last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// When Trek first heard of it (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// Its output can be read while it runs (`background_output`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readable: bool,
    /// The agent can stop it (`stop_background`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stoppable: bool,
    /// Asked to stop, not gone yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    /// A shell command left running.
    Shell,
    /// A command whose output the agent watches.
    Monitor,
    #[default]
    #[serde(other)]
    Other,
}

/// The reply to `background_output`: the end of a task's output (its last 400 lines, 64 KiB at
/// most), or none when it hasn't been read yet or can't be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundOutput {
    pub thread_id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// `icon`: the icon an added agent (`agent`, its key) or a project (`project_id`) shows on the
/// Mac, whose rows carry its `icon` hash.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IconRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// The reply to `icon`: a square PNG, 96 px.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Icon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The same as the row's `icon`: the phone keeps it by this.
    pub hash: String,
    /// Base64.
    pub png: String,
    /// A shape to draw in a colour of the phone's choosing (its alpha is what counts): the
    /// text colour for an agent, the project's ink for a project. Otherwise a picture as it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub template: bool,
}
