//! What the phone gained to show a thread as the Mac's thread view does: a turn's "Verified"
//! badge, a handoff's two agents, the videos a message attaches and what `watch_video` saw, the
//! limit bar's state and buttons, the context ring's card with Compact, the confirm card's file
//! list before an undo, retry or rewind, and sending with consultants.
//!
//! Fields on items and rows are additive (sent only when there's something in them; an older Mac
//! never sends them, an older phone ignores them). The requests are listed in `welcome`/`paired`
//! `features`: [`super::feature::MEDIA`], [`super::feature::LIMIT_ACTIONS`],
//! [`super::feature::CONTEXT_CARD`], [`super::feature::TURN_PREVIEW`] and
//! [`super::feature::CONSULT`].

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// On items and rows
// ---------------------------------------------------------------------------------------------

/// A turn that ran the project's verification CLI (`turn_end.verified`): whether its last run of
/// each check passed, and the commands that ran it, newest last (at most [`MAX_VERIFIED`], each
/// cut to about 200 characters).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TurnVerified {
    pub passed: bool,
    #[serde(default)]
    pub commands: Vec<String>,
}

/// The most commands a [`TurnVerified`] lists.
pub const MAX_VERIFIED: usize = 5;

/// What a `watch_video` call watched (`tool.watch`), from the manifest its answer names: the
/// frames the model saw (files on the Mac, fetched with `media`), and the video when it's still
/// there.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WatchedVideo {
    /// The file or link it was asked to watch.
    #[serde(default)]
    pub source: String,
    /// The video on the Mac (the user's own file, or the download), absolute.
    #[serde(default)]
    pub video: Option<String>,
    /// Seconds.
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub frames: Vec<WatchFrame>,
}

/// One frame the model saw.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WatchFrame {
    /// Seconds into the video.
    pub t: f64,
    /// The frame's file on the Mac, absolute.
    pub path: String,
}

// Items compare whole (the Mac sends one again only when it changed); a frame's time is always a
// finite number (the Mac drops any other), so equality is one.
impl Eq for WatchedVideo {}
impl Eq for WatchFrame {}

/// A thread paused at a usage limit (`ThreadSummary::pause`), as the limit bar over its composer
/// shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LimitPause {
    /// When the limit resets (ms), when that's known.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// It resumes by itself at the reset ("Resume at reset" was chosen).
    #[serde(default)]
    pub resume: bool,
    /// The agent wrapped up ahead of the limit rather than being cut off at it ("Wrapped up
    /// before the limit").
    #[serde(default)]
    pub wrapped: bool,
}

// ---------------------------------------------------------------------------------------------
// `media`: files a thread names
// ---------------------------------------------------------------------------------------------

/// `media`: a file one of the thread's items names, as the Mac's transcript shows it: an image or
/// a file an answer links (`![…](path)`, `[…](path)`), a video a message attaches (`user.videos`),
/// or what `watch_video` saw (`tool.watch`). `path` is absolute or relative to the thread's
/// folder, as written. Anything else is refused (`not_found`).
///
/// An image with `max_px` comes back whole, scaled down to fit (a JPEG, with its size); anything
/// else in raw pieces of at most [`super::MAX_NOTE_FILE_CHUNK`] from `offset`, as `note_file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRequest {
    pub thread_id: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_px: Option<u32>,
    #[serde(default)]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

/// The reply to `media`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub thread_id: String,
    /// As asked.
    pub path: String,
    /// `image/jpeg` for a scaled image; else from the file's extension (`video/mp4`,
    /// `video/quicktime`, `image/png`, …).
    pub mime: String,
    pub offset: u64,
    /// The whole file's length in bytes (a scaled image's: the bytes sent).
    pub total: u64,
    /// Base64 of the piece (empty past the end).
    pub bytes_b64: String,
    /// A scaled image's size, as sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl Media {
    /// The piece's bytes.
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(self.bytes_b64.trim())
            .map_err(|e| format!("not base64: {e}"))
    }
}

// ---------------------------------------------------------------------------------------------
// `context_card`: the context ring's card
// ---------------------------------------------------------------------------------------------

/// The reply to `context_card`: the card the Mac's context ring opens, as it would draw it now.
/// `parts` are the bar's segments in the Mac's order, each with what its flyout says, and cover
/// the whole window: `system`, `mcp`, `skills`, `messages` (each only when it has tokens), then
/// `free` and `reserved`, adding up to `window`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCard {
    pub thread_id: String,
    /// Tokens in the window, as the agent last reported.
    pub used: u64,
    pub window: u64,
    /// What the agent keeps free so compacting has room (estimated): not part of `used`.
    pub reserved: u64,
    #[serde(default)]
    pub parts: Vec<ContextPart>,
    pub compact: CompactState,
}

/// A segment of the context bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPart {
    /// `system` · `mcp` · `skills` · `messages` · `free` · `reserved` (a phone draws one it
    /// doesn't know in a neutral colour).
    pub kind: String,
    /// The Mac's legend label ("System", "MCP", "Skills", "Messages", "Free", "Reserved").
    pub label: String,
    pub tokens: u64,
    /// The flyout's rows.
    #[serde(default)]
    pub detail: Vec<ContextDetail>,
    /// The flyout's muted line under them.
    #[serde(default)]
    pub note: Option<String>,
}

/// A row of a segment's flyout: a label, and its value where there is one ("12.3K").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDetail {
    pub label: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// Whether the card's Compact now can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactState {
    /// `thread_action {kind: "compact"}` sends the agent's `/compact`.
    Ready,
    /// A `/compact` is running now ("Summarizing messages").
    Running,
    /// A turn runs, or the thread is paused at a usage limit: it waits.
    Busy,
    /// Nothing to compact yet (no message sent).
    #[serde(rename = "none")]
    Nothing,
}

// ---------------------------------------------------------------------------------------------
// `turn_preview`: the confirm card
// ---------------------------------------------------------------------------------------------

/// `turn_preview`: what the Mac's confirmation before an `undo`, `retry` or `rewind` says, for
/// the same item `turn_action` would take (`fork` asks nothing: `bad_request`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPreviewRequest {
    pub thread_id: String,
    pub item_id: String,
    pub action: super::TurnAction,
}

/// The reply to `turn_preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPreview {
    pub thread_id: String,
    pub item_id: String,
    /// The agent can't take its own session back to that point: it carries on in a new one
    /// with a recap (the card says so).
    #[serde(default)]
    pub recap: bool,
    /// What "Also restore files" would do, file by file (empty: the files are as they were);
    /// `None` when there's nothing to restore them to (`unavailable` says why).
    #[serde(default)]
    pub files: Option<Vec<PreviewFile>>,
    /// Why the files can't be restored, in the Mac's words.
    #[serde(default)]
    pub unavailable: Option<String>,
}

/// A file restoring would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewFile {
    /// Relative to the repository's top folder.
    pub path: String,
    pub change: PreviewChange,
}

/// What restoring does to a file (the Mac's words in brackets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewChange {
    /// Changed since: its old contents come back ("revert").
    Revert,
    /// Created since: it's removed ("delete").
    Delete,
    /// Deleted since: it comes back ("bring back").
    BringBack,
    /// A repository inside this one: left as it is ("left as is").
    Nested,
    /// Newer, and out of what checkpoints hold: left as it is ("left as is (newer, not in
    /// checkpoints)").
    Kept,
}

// ---------------------------------------------------------------------------------------------
// `consult`: sending with consultants
// ---------------------------------------------------------------------------------------------

/// `send.consult`: consultants picked in the phone's composer, as the Mac's composer sends them
/// (the message goes wrapped in what Trek tells the agent about them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsultRequest {
    /// Consultants by key, `agent/model/effort` (`codex/gpt-5.6-sol/high`).
    pub with: Vec<String>,
    #[serde(default)]
    pub style: ConsultStyle,
    #[serde(default)]
    pub then: ConsultThen,
    /// An arena's judge, by key (`None`: Trek picks, of another model than the thread's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
}

/// How the agent works with its consultants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsultStyle {
    /// They review and suggest; the agent decides.
    #[default]
    Advise,
    /// They go back and forth until they agree.
    Discuss,
    /// Each drafts a design, a judge scores them blind, the agent synthesises one.
    Arena,
}

/// What happens once the consultants have had their say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsultThen {
    #[default]
    Implement,
    /// Stop there: change nothing.
    Report,
}
