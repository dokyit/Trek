//! Wire protocol v1: the JSON messages exchanged with the phone (see `docs/MOBILE.md`).
//!
//! Every message is a JSON object with a `"type"`. Client requests may carry an `"id"`; the reply
//! carries `"re"` with the same value. Keys are `snake_case`, timestamps are unix milliseconds.
//! Optional fields serialize as `null` and may be missing when deserializing.

use serde::{Deserialize, Serialize};

/// The protocol version this crate speaks (`hello`/`pair` carry the phone's, `welcome`/`paired`
/// the Mac's).
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Envelopes
// ---------------------------------------------------------------------------------------------

/// A message from the phone, with its optional request id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub msg: ClientMessage,
}

/// A message to the phone, with the id of the request it answers (if any).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerEnvelope {
    #[serde(flatten)]
    pub msg: ServerMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<String>,
}

impl ServerEnvelope {
    /// An unsolicited message (no `re`).
    pub fn push(msg: ServerMessage) -> Self {
        Self { msg, re: None }
    }

    /// A reply to the request with id `re`.
    pub fn reply(re: Option<String>, msg: ServerMessage) -> Self {
        Self { msg, re }
    }

    /// The JSON text of this message.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("protocol messages always serialize")
    }
}

// ---------------------------------------------------------------------------------------------
// Phone -> Mac
// ---------------------------------------------------------------------------------------------

/// Messages the phone sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message of a new device: trade a pairing code for a device token.
    Pair {
        protocol: u32,
        code: String,
        device_id: String,
        device_name: String,
        #[serde(default)]
        app_version: Option<String>,
    },
    /// First message of a paired device.
    Hello {
        protocol: u32,
        device_id: String,
        token: String,
        #[serde(default)]
        app_version: Option<String>,
    },
    /// Start receiving a thread's transcript.
    Subscribe {
        thread_id: String,
        #[serde(default)]
        after_seq: Option<u64>,
        /// A full transcript (`reset`) holds only the last `limit` items (`i…`), with the cards
        /// that follow them (`c…`) and every open request (`r…`); `more` says earlier ones were
        /// left out (`transcript_before` pages them in).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Stop receiving a thread's transcript.
    Unsubscribe { thread_id: String },
    /// The items just before `before` (an item id, `i340`), at most `limit` of them (the Mac
    /// caps it at [`MAX_PAGE`]); answered with `transcript_page`.
    TranscriptBefore { thread_id: String, before: String, limit: u32 },
    /// Undo, retry, fork or rewind, as a turn's footer and a message's actions do on the Mac.
    TurnAction(TurnActionRequest),
    /// A follow-up message to a thread.
    Send(SendRequest),
    /// Start a new thread.
    NewThread(NewThreadRequest),
    /// Answer an approval, question or plan.
    Answer(AnswerRequest),
    /// Stop a running turn.
    Interrupt { thread_id: String },
    /// The user has seen the thread.
    MarkSeen { thread_id: String },
    /// Change a thread's agent, model, effort, access or plan mode (as the Mac's composer does).
    SetPrefs(PrefsRequest),
    /// Pin, settle or archive a thread, or rename it.
    ThreadAction(ThreadActionRequest),
    /// Plan limits of every agent that reports them (answered with `usage`).
    Usage,
    /// The Basecamp recap (answered with `basecamp`).
    Basecamp {
        #[serde(default)]
        range: BasecampRange,
    },
    /// Every note, newest first (answered with `notes`).
    Notes,
    /// One note, whole (answered with `note`).
    Note { note_id: String },
    /// A new note (answered with `note`).
    CreateNote {
        #[serde(default)]
        body: String,
    },
    /// Write a note (answered with `note`).
    SaveNote(SaveNoteRequest),
    /// Move a note to the notes folder's `Deleted/`.
    DeleteNote { note_id: String },
    /// A thread's or project's git status (answered with `git_status`).
    GitStatus(GitTarget),
    /// One changed file's diff (answered with `git_diff`).
    GitDiff(GitDiffRequest),
    /// Commit every change, with a message.
    GitCommit(GitCommitRequest),
    GitPush(GitTarget),
    /// Local branches (answered with `git_branches`).
    GitBranches(GitTarget),
    /// Check out another branch (not in a worktree thread, nor while the thread works).
    GitSwitch(GitSwitchRequest),
    /// Merge a worktree thread's branch into its base, in the project folder.
    WorktreeMerge { thread_id: String },
    /// Remove a worktree thread's worktree; the thread carries on in the project folder.
    WorktreeRemove(WorktreeRemoveRequest),
    /// The slash commands a thread offers (answered with `commands`).
    Commands { thread_id: String },
    /// The Mac's settings the phone may see and change (answered with `settings`).
    Settings,
    /// Change some of them (answered with `settings`, as they are now).
    SetSettings(SettingsChange),
    /// Application-level ping (answered with `pong`).
    Ping,
}

impl ClientMessage {
    /// The `type` tag of this message.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Pair { .. } => "pair",
            Self::Hello { .. } => "hello",
            Self::Subscribe { .. } => "subscribe",
            Self::Unsubscribe { .. } => "unsubscribe",
            Self::TranscriptBefore { .. } => "transcript_before",
            Self::TurnAction(_) => "turn_action",
            Self::Send(_) => "send",
            Self::NewThread(_) => "new_thread",
            Self::Answer(_) => "answer",
            Self::Interrupt { .. } => "interrupt",
            Self::MarkSeen { .. } => "mark_seen",
            Self::SetPrefs(_) => "set_prefs",
            Self::ThreadAction(_) => "thread_action",
            Self::Usage => "usage",
            Self::Basecamp { .. } => "basecamp",
            Self::Notes => "notes",
            Self::Note { .. } => "note",
            Self::CreateNote { .. } => "create_note",
            Self::SaveNote(_) => "save_note",
            Self::DeleteNote { .. } => "delete_note",
            Self::GitStatus(_) => "git_status",
            Self::GitDiff(_) => "git_diff",
            Self::GitCommit(_) => "git_commit",
            Self::GitPush(_) => "git_push",
            Self::GitBranches(_) => "git_branches",
            Self::GitSwitch(_) => "git_switch",
            Self::WorktreeMerge { .. } => "worktree_merge",
            Self::WorktreeRemove(_) => "worktree_remove",
            Self::Commands { .. } => "commands",
            Self::Settings => "settings",
            Self::SetSettings(_) => "set_settings",
            Self::Ping => "ping",
        }
    }
}

impl ClientMessage {
    /// It only reads: it may be answered out of order with what the phone sends after it.
    pub fn is_query(&self) -> bool {
        matches!(
            self,
            Self::Usage
                | Self::Basecamp { .. }
                | Self::Notes
                | Self::Note { .. }
                | Self::GitStatus(_)
                | Self::GitDiff(_)
                | Self::GitBranches(_)
                | Self::Commands { .. }
                | Self::Settings
                | Self::TranscriptBefore { .. }
        )
    }
}

/// The most items one `transcript_before` returns.
pub const MAX_PAGE: u32 = 500;

/// `turn_action`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnActionRequest {
    pub thread_id: String,
    /// For `undo`, `retry` and `fork`: the item that ends the turn (its `turn_end`; a turn that
    /// failed, hit a limit or was interrupted ends with that item instead). For `rewind`, and a
    /// `fork` from just before a message: one of the user's messages (`user`).
    pub item_id: String,
    pub action: TurnAction,
    /// `retry` with this model (one of the thread's agent's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Put the files back as they were when the message was sent (when Trek has them).
    #[serde(default = "yes")]
    pub restore_files: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAction {
    /// Take back the turn (and everything after it); its message comes back in `ack.text`.
    Undo,
    /// Take back the turn and send its message again (with `model`, if given).
    Retry,
    /// A new thread with the conversation up to the end of the turn, or up to just before the
    /// message; `ack.thread_id` is the new thread (and `ack.text` the message, for a message).
    Fork,
    /// Take back the message and everything after it; it comes back in `ack.text`.
    Rewind,
}

/// What a `turn_action` did, for its `ack`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnActionDone {
    /// The new thread, for a fork.
    pub thread_id: Option<String>,
    /// The message taken back (for the phone's composer).
    pub text: Option<String>,
}

/// `send`: a follow-up to a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest {
    pub thread_id: String,
    pub text: String,
    /// Steer the running turn or queue after it; `None` = the Mac's follow-up setting.
    #[serde(default)]
    pub mode: Option<SendMode>,
    /// Photos attached to the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageUpload>,
}

/// A photo sent from the phone: JPEG or PNG, base64. The Mac saves it with its snapshots and
/// attaches it as the composer's photos are. Phones keep each one under about a megabyte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUpload {
    /// `image/jpeg` or `image/png`.
    pub mime: String,
    pub data: String,
}

impl ImageUpload {
    /// The image's bytes, and the file extension it takes, if it really is a JPEG or a PNG.
    pub fn decode(&self) -> Result<(Vec<u8>, &'static str), String> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(self.data.trim()).map_err(|e| format!("not base64: {e}"))?;
        // What the bytes are, not what the phone says: only photos are kept.
        let ext = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "jpg"
        } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            "png"
        } else {
            return Err("not a JPEG or PNG".into());
        };
        Ok((bytes, ext))
    }
}

/// How much a thread's agent may do without asking (the Mac's hand-holding levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    Supervised,
    AutoAcceptEdits,
    Auto,
    FullAccess,
}

/// `set_prefs`: what to change about a thread; `None` leaves it as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PrefsRequest {
    pub thread_id: String,
    /// Another agent (its key): the thread carries on with it from a recap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `low`, `medium`, `high`, `xhigh`, `max`… as the model offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<bool>,
}

/// `thread_action`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadActionRequest {
    pub thread_id: String,
    pub action: ThreadAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadAction {
    Pin,
    Unpin,
    /// Done with it: out of the inbox.
    Settle,
    /// Back into the inbox.
    Unsettle,
    Archive,
    Rename { title: String },
}

/// `save_note`: a note's new text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveNoteRequest {
    pub note_id: String,
    pub body: String,
    /// The `modified` of the copy the phone edited: if the note changed on the Mac since, nothing
    /// is written and the phone gets `conflict`. `None` writes it regardless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
}

/// Whose folder a git request is about: a thread's (its worktree, for one in a worktree), else a
/// project's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl GitTarget {
    pub fn thread(id: impl Into<String>) -> Self {
        Self { thread_id: Some(id.into()), project_id: None }
    }

    pub fn project(id: impl Into<String>) -> Self {
        Self { thread_id: None, project_id: Some(id.into()) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiffRequest {
    #[serde(flatten)]
    pub target: GitTarget,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitRequest {
    #[serde(flatten)]
    pub target: GitTarget,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSwitchRequest {
    #[serde(flatten)]
    pub target: GitTarget,
    pub branch: String,
}

/// `worktree_remove`. Without `force`, the Mac refuses when something would be lost: uncommitted
/// changes (they go with the folder), or, with `delete_branch`, commits its base doesn't have. The
/// refusal (`conflict`) says what, as the Mac's confirmation does; sending it again with `force`
/// is the user agreeing to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRemoveRequest {
    pub thread_id: String,
    /// Delete the branch too (it's kept otherwise, unless its base has all of it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delete_branch: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

/// `set_settings`: what to change; `None` leaves it as it is. Only these can be changed from a
/// phone: never Full access's unlock, API keys or the phone server itself.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SettingsChange {
    /// New threads' agent (its key), model, effort and access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    /// `""` goes back to the agent's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_access: Option<Access>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<SendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<NotifyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_when: Option<PushWhen>,
    /// The ntfy server, `https://…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_server: Option<String>,
    /// Make a new random topic (the old one stops getting notes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_push_topic: bool,
    /// Send a test notification, once the rest is changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub push_test: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_settle_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
}

/// How a follow-up reaches a working thread. On an idle thread both just start a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    /// Inject into the running turn.
    Steer,
    /// Deliver after the running turn.
    Queue,
}

/// `new_thread`: start a thread in a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewThreadRequest {
    pub project_id: String,
    /// The agent's key (`claude-code`, `codex`, `acp:cursor`…).
    pub agent: String,
    /// `None` = the project's or agent's default.
    #[serde(default)]
    pub model: Option<String>,
    pub text: String,
    /// Run in a new worktree (a request: a project that isn't a git repo runs locally).
    #[serde(default)]
    pub worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageUpload>,
}

/// `answer`: respond to a pending approval, question or plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub thread_id: String,
    pub request_id: String,
    pub response: AnswerResponse,
}

/// The user's response to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerResponse {
    Approval {
        decision: Decision,
    },
    Questions {
        answers: Vec<QA>,
    },
    Plan {
        approve: bool,
        #[serde(default)]
        feedback: Option<String>,
    },
}

/// An approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    AllowForSession,
    Deny,
}

/// One answered question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QA {
    pub question: String,
    pub answer: String,
}

// ---------------------------------------------------------------------------------------------
// Mac -> phone
// ---------------------------------------------------------------------------------------------

/// Messages the Mac sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Pairing succeeded; the connection is authenticated.
    Paired {
        protocol: u32,
        token: String,
        host: HostInfo,
    },
    /// `hello` succeeded.
    Welcome { protocol: u32, host: HostInfo },
    /// Everything the phone shows in its inbox (replaces its copy).
    Snapshot(Snapshot),
    /// Upsert of one thread.
    Thread { thread: ThreadSummary },
    /// A thread is gone.
    ThreadRemoved { thread_id: String },
    /// The reply to `subscribe`.
    Transcript {
        thread_id: String,
        /// `true` replaces the phone's copy; `false` appends to it.
        reset: bool,
        seq: u64,
        items: Vec<Item>,
        /// Earlier items were left out (a `subscribe` with a `limit`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        more: bool,
    },
    /// The reply to `transcript_before`: the items just before the one asked about, in order.
    TranscriptPage {
        thread_id: String,
        items: Vec<Item>,
        /// Still earlier ones exist.
        more: bool,
    },
    /// Upsert of one transcript item (by `id`).
    Item { thread_id: String, item: Item },
    /// The phone should `subscribe` again (the thread was rewound or rewritten).
    TranscriptReset { thread_id: String },
    /// A request succeeded.
    Ack {
        /// Set for `new_thread`: the id of the new thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// Set when the phone carries on by itself: `/new` sent to a thread asks it to open its
        /// new-thread sheet (the Mac's own screen stays where it is).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        open: Option<Open>,
        /// Set for `turn_action` when a message was taken back (`undo`, `rewind`, a `fork` from
        /// a message): its text, for the phone's composer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// The reply to `ping`.
    Pong,
    /// The reply to `usage`.
    Usage(Usage),
    /// The reply to `basecamp`.
    Basecamp(Basecamp),
    /// The reply to `notes`.
    Notes { notes: Vec<NoteSummary> },
    /// The reply to `note`, `create_note` and `save_note`.
    Note { note: Note },
    /// The reply to `git_status`.
    GitStatus(GitStatus),
    /// The reply to `git_diff`.
    GitDiff(GitDiff),
    /// The reply to `git_branches`.
    GitBranches(GitBranches),
    /// The reply to `commands`.
    Commands { thread_id: String, commands: Vec<CommandInfo> },
    /// The reply to `settings` and `set_settings`.
    Settings(MacSettings),
    /// A request failed.
    Error { code: ErrorCode, message: String },
}

/// Error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    PairingFailed,
    RateLimited,
    UnsupportedProtocol,
    NotFound,
    /// E.g. the request was already answered.
    Conflict,
    HostError,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::PairingFailed => "pairing_failed",
            Self::RateLimited => "rate_limited",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::HostError => "host_error",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A screen the phone should open for the user, as an `ack` asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
pub enum Open {
    /// The new-thread sheet, in this project (none: no project).
    NewThread {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
    },
}

/// The Mac, as the phone sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

// ---------------------------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------------------------

/// Threads, projects and agents: everything the phone's inbox and new-session sheet need.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub threads: Vec<ThreadSummary>,
    #[serde(default)]
    pub projects: Vec<ProjectSummary>,
    #[serde(default)]
    pub agents: Vec<AgentOption>,
    /// The Mac lets threads run with Full access (it's unlocked in its settings).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_access: bool,
}

/// One thread's row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub project: ProjectRef,
    pub agent: AgentRef,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_label: Option<String>,
    pub run_state: RunState,
    /// Present when the thread waits on the user.
    #[serde(default)]
    pub needs: Option<Needs>,
    pub section: Section,
    #[serde(default)]
    pub unseen: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree: bool,
    /// What a working thread is doing now ("Editing src/auth.rs").
    #[serde(default)]
    pub activity: Option<String>,
    /// When the current turn started (ms).
    #[serde(default)]
    pub working_since: Option<i64>,
    /// Last activity (ms).
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    /// The thread's effort, access and plan mode, as its composer shows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan: bool,
    /// The effort as the Mac labels it ("Extra high").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_label: Option<String>,
    /// How full its context window is, as the composer's ring shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextUse>,
    /// What it cost, as the line under the composer says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// Its sub-agents at work: Trek's, and its agent's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_agents: Vec<SubAgent>,
    /// What its agent runs in the background (dev servers, watchers), by title.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<String>,
    /// For a thread in a worktree: the branch it started from and merges back into (`branch`
    /// is the worktree's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Its folder's git state, as last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSummary>,
}

/// A context window's use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUse {
    /// Tokens in it.
    pub used: u64,
    /// Its size.
    pub window: u64,
    /// 0–100.
    pub percent: u8,
}

/// What a thread's tokens cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    /// "$1.24" (billed per token), "≈ $1.24 at API prices" (a plan covers it), "12.3K tokens ·
    /// price unknown", "202K tokens · free".
    pub label: String,
    /// How its session is billed, once its agent has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billing: Option<Billing>,
    /// The plan's name ("Claude Max") when billed through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// "Included in your Claude Max plan", "Billed per token by your API provider".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    /// A subscription covers it: the figure is what it would cost at API prices.
    Plan,
    /// Billed per token.
    Metered,
    /// A model on the Mac: nothing is billed.
    Local,
}

/// A sub-agent at work for a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgent {
    /// Whose logo it wears.
    pub agent: AgentRef,
    /// The model it runs ("Sol"), for one Trek runs; `None` for the agent's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What it was given to do.
    pub title: String,
    pub state: SubAgentState,
    /// When it started (ms; working time so far is now minus this).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentState {
    Running,
    /// Waits on an approval (in its own thread).
    NeedsYou,
    Done,
    Failed,
    Stopped,
}

/// A folder's git state in a row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitSummary {
    /// Files with changes not committed (untracked ones included).
    #[serde(default)]
    pub changed: u32,
    /// Commits not pushed to its upstream, and the upstream's not pulled.
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
}

/// A project, as referenced from a thread row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
    /// Degrees 0–359.
    pub hue: u16,
    pub monogram: String,
}

/// A project, as listed in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub hue: u16,
    pub monogram: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub is_repo: bool,
}

impl ProjectSummary {
    /// The reference a thread row carries.
    pub fn to_ref(&self) -> ProjectRef {
        ProjectRef { id: self.id.clone(), name: self.name.clone(), hue: self.hue, monogram: self.monogram.clone() }
    }
}

/// An agent, as referenced from a thread row.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentRef {
    /// Trek's `AgentId::key()`.
    pub key: String,
    pub name: String,
    /// The logo the Mac draws for it (`claude-code`, `codex`, `gemini`, `openai`…: its
    /// `assets/logos/{dark,light}/<logo>.png`); `None` draws a neutral glyph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
}

impl AgentRef {
    pub fn new(key: impl Into<String>, name: impl Into<String>, logo: Option<&str>) -> Self {
        Self { key: key.into(), name: name.into(), logo: logo.map(str::to_string) }
    }
}

/// An agent the phone can start threads with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOption {
    pub key: String,
    pub name: String,
    /// As in [`AgentRef::logo`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelOption>,
}

/// A model an agent offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    /// The efforts it takes (`low`, `medium`, `high`…), lowest first; empty when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
}

/// Why a thread waits on the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    pub kind: NeedsKind,
    /// One-line summary for the row.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsKind {
    Approval,
    Question,
    Plan,
    Failed,
    Limit,
}

/// Trek's `RunState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    #[default]
    Idle,
    Working,
    NeedsYou,
    Failed,
}

/// Trek's sidebar `Section`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Pinned,
    #[default]
    Inbox,
    Working,
    Snoozed,
    Settled,
}

// ---------------------------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------------------------

/// A thread's transcript as the host serves it: its items, and the highest seq handed out so far.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub seq: u64,
    pub items: Vec<Item>,
    /// Phones that saw a seq below this can't be brought up to date by what changed since: they
    /// get it all again (items went away, or numbering started over). 0: any seq will do.
    #[serde(default)]
    pub base: u64,
    /// The host left out earlier items (it served a `limit` itself).
    #[serde(default)]
    pub more: bool,
}

/// Items just before one, as `transcript_before` asks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub items: Vec<Item>,
    /// Still earlier ones exist.
    pub more: bool,
}

impl Transcript {
    /// The page of up to `limit` items just before the item `before`, from a whole transcript:
    /// open requests aside, and a turn's changed files counted with the item they follow. `None`
    /// when there's no such item.
    pub fn page_before(&self, before: &str, limit: u32) -> Option<TranscriptPage> {
        let items: Vec<&Item> = self.items.iter().filter(|i| !i.body.is_request()).collect();
        let end = items.iter().position(|i| i.id == before)?;
        let limit = limit.min(MAX_PAGE);
        let (mut kept, mut start) = (0, end);
        for (ix, item) in items[..end].iter().enumerate().rev() {
            if kept == limit {
                break;
            }
            if !item.body.is_attached() {
                kept += 1;
            }
            start = ix;
        }
        // Files whose turn end is on the next page go with it.
        while start < end && items[start].body.is_attached() {
            start += 1;
        }
        Some(TranscriptPage { items: items[start..end].iter().map(|i| (*i).clone()).collect(), more: start > 0 })
    }
}

impl ItemBody {
    /// An approval, question or plan card (an `r…` item).
    pub fn is_request(&self) -> bool {
        matches!(self, Self::Approval { .. } | Self::Question { .. } | Self::Plan { .. })
    }

    /// What follows another item rather than standing on its own: a turn's changed files.
    pub fn is_attached(&self) -> bool {
        matches!(self, Self::Changes { .. })
    }
}

/// One transcript item. `id` is stable; `seq` is per thread and grows on every update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub seq: u64,
    #[serde(default)]
    pub at: Option<i64>,
    #[serde(flatten)]
    pub body: ItemBody,
}

/// What an item is, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemBody {
    User {
        text: String,
        #[serde(default)]
        images: u32,
    },
    Assistant {
        text: String,
        #[serde(default)]
        streaming: bool,
    },
    Reasoning {
        text: String,
    },
    Tool {
        call_id: String,
        tool: ToolKind,
        title: String,
        #[serde(default)]
        detail: String,
        status: ToolStatus,
        /// The last 4 KiB of output (see [`truncate_output`]).
        #[serde(default)]
        output: String,
        #[serde(default)]
        added: Option<u32>,
        #[serde(default)]
        removed: Option<u32>,
    },
    Approval {
        request_id: String,
        title: String,
        #[serde(default)]
        detail: String,
        state: ApprovalState,
    },
    Question {
        request_id: String,
        questions: Vec<Question>,
        state: QuestionState,
        #[serde(default)]
        answers: Option<Vec<QA>>,
    },
    Plan {
        request_id: String,
        markdown: String,
        state: PlanState,
    },
    TurnEnd {
        took_secs: u32,
    },
    Notice {
        text: String,
    },
    Error {
        text: String,
    },
    Limit {
        text: String,
        #[serde(default)]
        resets_at: Option<i64>,
    },
    Handoff {
        from: String,
        to: String,
    },
    /// The files a turn changed, after its `turn_end`.
    Changes {
        files: Vec<ChangedFile>,
        /// Lines added and removed in all.
        #[serde(default)]
        added: u32,
        #[serde(default)]
        removed: u32,
    },
}

/// A changed file: in a turn's `changes`, or in a folder's `git_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Relative to the folder.
    pub path: String,
    pub status: FileStatus,
    /// A renamed file's old path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub removed: u32,
    /// No line counts: it isn't text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// New and not added to git yet (`git_status` only; a turn's `changes` says `added`).
    Untracked,
}

impl ItemBody {
    /// The `kind` tag of this item.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Assistant { .. } => "assistant",
            Self::Reasoning { .. } => "reasoning",
            Self::Tool { .. } => "tool",
            Self::Approval { .. } => "approval",
            Self::Question { .. } => "question",
            Self::Plan { .. } => "plan",
            Self::TurnEnd { .. } => "turn_end",
            Self::Notice { .. } => "notice",
            Self::Error { .. } => "error",
            Self::Limit { .. } => "limit",
            Self::Handoff { .. } => "handoff",
            Self::Changes { .. } => "changes",
        }
    }
}

/// A tool row's kind, from its title (like the desktop's `tool_kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Command,
    Read,
    Edit,
    Search,
    Web,
    Agent,
    #[default]
    Other,
}

impl ToolKind {
    /// A tool call's kind, from the row title agents report ("Run command", "Read", "Edit").
    /// Mirrors the desktop's `activity::tool_kind` (web search and fetch both map to `web`).
    pub fn from_title(title: &str) -> Self {
        match title {
            "Subagent" => Self::Agent,
            "Fetch" => Self::Web,
            t if t.starts_with("Search the web") || t.starts_with("Web") => Self::Web,
            t if t.starts_with("Run") || t.starts_with("Ran") => Self::Command,
            t if t.starts_with("Edit") || t.starts_with("Wr") => Self::Edit,
            t if t.starts_with("Read") => Self::Read,
            t if t.contains("Search") || t.starts_with("List") => Self::Search,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Allowed,
    AllowedForSession,
    Denied,
    /// The agent moved on by itself.
    Resolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Pending,
    Answered,
    Resolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Pending,
    Approved,
    Rejected,
    Resolved,
}

/// One question of a `question` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    #[serde(default)]
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

// ---------------------------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------------------------

/// Plan usage, as the Mac's Usage popover shows it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// The agents that reported their plan, in the Mac's agent order.
    #[serde(default)]
    pub providers: Vec<ProviderUsage>,
    /// The Mac was still asking an agent when it answered (ask again shortly for the rest).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub loading: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub agent: AgentRef,
    /// "Claude Max", "ChatGPT Plus".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// 5-hour, weekly and per-model windows. Empty: the plan has none.
    #[serde(default)]
    pub limits: Vec<UsageLimit>,
    /// Something the plan reports besides its limits (Devin's on-demand balance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Asking it partly failed (what's here may still hold).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// "5-hour limit", "Weekly limit", "Weekly · Fable".
    pub label: String,
    /// 0–100 used.
    pub percent: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// The window's length: "5h", "7d".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub window: String,
}

// ---------------------------------------------------------------------------------------------
// Basecamp
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BasecampRange {
    #[default]
    Today,
    /// Since Monday.
    Week,
    All,
}

/// The Basecamp recap, worded as the Mac words it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Basecamp {
    pub range: BasecampRange,
    /// "Good evening, Monday 5 October", or "Good evening — on the trail since 3 June".
    pub greeting: String,
    /// "Today's trek", "This week's trek", "Your trek so far".
    pub title: String,
    /// When the recap was worked out (ms).
    pub updated_at: i64,
    /// "Ready for review": what needs the user first, then finished threads not looked at yet.
    #[serde(default)]
    pub review: Vec<ReviewRow>,
    /// Nothing on the trail in the range: `invitation` says so, and there's no recap.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation: Option<String>,
    /// The recap in sentences: text, numbers worth reading first, project and model badges.
    #[serde(default)]
    pub narrative: Vec<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RecapSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub tiles: Vec<Tile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRow {
    pub thread_id: String,
    pub title: String,
    pub status: ReviewStatus,
    /// "Approval", "Question", "Plan to review", "Paused until 3 PM", "Failed"; `None` when done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub agent: AgentRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    pub updated_at: i64,
    #[serde(default)]
    pub unseen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    NeedsYou,
    Failed,
    /// Stopped at a usage limit, to go on at its reset.
    Paused,
    Done,
}

/// A piece of the narrative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Span {
    Text { text: String },
    /// A number worth reading first ("18 prompts").
    Strong { text: String },
    /// A project, drawn as its badge.
    Project {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<ProjectRef>,
    },
    /// A model, drawn with its agent's logo.
    Model { text: String, agent: AgentRef },
}

/// The recap's figures, for laying out a sentence of one's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecapSummary {
    pub prompts: u32,
    pub threads: u32,
    pub turns: u32,
    /// Agent time in seconds, and as the Mac says it ("1h 2m", "under a minute").
    pub agent_secs: u64,
    pub agent_time: String,
    pub tokens: u64,
    /// Turns that failed in the range.
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_project: Option<ProjectShare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_model: Option<ModelShare>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectShare {
    pub project: ProjectRef,
    pub prompts: u32,
    #[serde(default)]
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelShare {
    pub agent: AgentRef,
    /// "Claude Opus 5.5".
    pub label: String,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub turns: u32,
    /// Its share of the reported tokens, in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<u32>,
}

/// The elevation profile: activity drawn as a mountain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Left to right.
    pub buckets: Vec<ProfileBucket>,
    /// The summit (the busiest stretch), if anything happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summit: Option<usize>,
    /// The stretch "now" falls in, while the range is current (the hiker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<usize>,
    /// Where "now" is across the range, 0–1.
    #[serde(default)]
    pub now_at: f32,
    /// The line over it: "Summit at 2 PM", "A flat trail so far".
    pub line: String,
    /// "18 prompts".
    pub total: String,
    /// Axis labels, at fractions of its width.
    #[serde(default)]
    pub ticks: Vec<Tick>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileBucket {
    /// How high it stands: agent minutes when turns were timed, else prompts.
    pub value: f32,
    /// "2–3 PM", "Tue 3–6 PM", "Sep 14", "week of Sep 8".
    pub label: String,
    /// What the Mac says over it when it's hovered: "2–3 PM · 4 prompts · 12m of agent time".
    pub line: String,
    #[serde(default)]
    pub prompts: u32,
    #[serde(default)]
    pub agent_secs: u64,
    #[serde(default)]
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    pub at: f32,
    pub label: String,
}

/// A stat tile: a quiet label, the figure, a note under it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub kind: TileKind,
    /// "Your best model", "Left on Claude Max".
    pub label: String,
    /// "Claude Opus 5.5", "182K tokens", "1h 2m", "64%".
    pub figure: String,
    /// "77% of tokens · 9 turns", "Nothing failed today", "5-hour limit · resets in 2h".
    pub note: String,
    /// The logo beside the figure (best model, plan left).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRef>,
    /// The badge beside the figure (worked most on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    /// Tokens used so far through the range, 0–1, one point a stretch (tokens).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sparkline: Vec<f32>,
    /// How much is left, 0–100 (plan left: the bar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileKind {
    BestModel,
    WorkedMostOn,
    Tokens,
    /// Agent time, with failed turns in its note.
    AgentTime,
    PlanLeft,
}

// ---------------------------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------------------------

/// A note in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    /// Its first line with words in it, without markdown; "Untitled".
    pub title: String,
    /// The text after the title, on one line.
    #[serde(default)]
    pub preview: String,
    /// Last changed (ms).
    pub modified: i64,
}

/// A note, whole: markdown (colour and highlights are inline `<span style="color: …">` and
/// `<mark>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub modified: i64,
}

// ---------------------------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------------------------

/// A folder's git status, as the Mac's Git panel shows it. For a thread in a worktree, its
/// changes against its base (its commits and what isn't committed yet).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitStatus {
    #[serde(default)]
    pub is_repo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    /// Against the upstream (`has_upstream`), or for a worktree, against its base.
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    #[serde(default)]
    pub has_upstream: bool,
    #[serde(default)]
    pub files: Vec<ChangedFile>,
    /// For a worktree thread: what merging and removing it would do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeStatus>,
    /// Another branch can be checked out here now; else `switch_blocked` says why.
    #[serde(default)]
    pub can_switch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub branch: String,
    pub base: String,
    /// Files with changes not committed.
    #[serde(default)]
    pub uncommitted: u32,
    /// Commits not pushed, when the branch has an upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpushed: Option<u32>,
    /// Why it can't be merged into its base right now ("The project folder is on main, not
    /// develop…"); `None`: it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_blocked: Option<String>,
    /// Commits its base doesn't have (lost with the branch, if it's deleted).
    #[serde(default)]
    pub unmerged: u32,
    /// The folder is gone already.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiff {
    pub path: String,
    /// Unified diff text (an untracked file: all its lines, `+`).
    pub diff: String,
    /// It was longer than the Mac sends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitBranches {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    /// Local branches, most recently committed first.
    #[serde(default)]
    pub branches: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

/// A slash command, as the composer's `/` picker lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    /// Without the slash: `permissions full`, `compact`, `review`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: CommandKind,
    /// One of Trek's own (answered by the Mac, not the agent).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trek: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Command,
    Skill,
    Agent,
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

/// The Mac's settings a phone may see and change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacSettings {
    /// New threads' agent (its key), model (`None`: the agent's default), effort and access.
    pub default_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    pub default_effort: String,
    pub default_access: Access,
    /// What a message to a working thread does.
    pub follow_up: SendMode,
    /// The Mac's own notifications.
    pub notifications: NotifyMode,
    pub push: PushSettings,
    /// Settle finished threads after this many idle days (0: never).
    pub auto_settle_days: u32,
    pub theme: Theme,
    /// Full access is unlocked on the Mac (only the Mac can change that).
    #[serde(default)]
    pub full_access: bool,
}

/// Notifications on the phone through ntfy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSettings {
    pub enabled: bool,
    pub when: PushWhen,
    /// The ntfy server, `https://ntfy.sh` or one of the user's own.
    pub server: String,
    /// The topic notes go to (empty until push is first turned on). Anyone who has it can read
    /// them: keep it private.
    #[serde(default)]
    pub topic: String,
    /// The topic on the web (`https://ntfy.sh/<topic>`): it opens in ntfy's web app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_url: Option<String>,
    /// ntfy's subscribe link, `ntfy://<host>/<topic>` (`ntfy://ntfy.sh/trek-…`). ntfy documents it
    /// for its Android app; its iOS app registers no link scheme (as of its 2026 source), so on
    /// an iPhone the topic is added by hand in ntfy (+, then the topic; another server under
    /// "Use another server").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscribe_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyMode {
    Off,
    Banner,
    Sound,
    BannerAndSound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushWhen {
    /// Only while the user is away from the Mac (two minutes idle, or locked).
    Away,
    Always,
}

/// The Mac's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follows macOS.
    System,
    Night,
    Paper,
}

/// ntfy's links for `topic` on `server`: the web page, and the `ntfy://` subscribe link.
pub fn ntfy_links(server: &str, topic: &str) -> (Option<String>, Option<String>) {
    if topic.is_empty() {
        return (None, None);
    }
    let server = server.trim().trim_end_matches('/');
    let host = server.split_once("://").map_or(server, |(_, h)| h);
    // `ntfy://` is https; a plain-http server says so (`?secure=false`).
    let insecure = if server.starts_with("http://") { "?secure=false" } else { "" };
    (Some(format!("{server}/{topic}")), Some(format!("ntfy://{host}/{topic}{insecure}")))
}

// ---------------------------------------------------------------------------------------------
// Helpers shared with the desktop
// ---------------------------------------------------------------------------------------------

/// The most tool output an item carries (the Mac keeps the last 4 KiB).
pub const MAX_TOOL_OUTPUT: usize = 4096;

/// The last [`MAX_TOOL_OUTPUT`] bytes of `output`, cut at a char boundary.
pub fn truncate_output(output: &str) -> &str {
    if output.len() <= MAX_TOOL_OUTPUT {
        return output;
    }
    let mut start = output.len() - MAX_TOOL_OUTPUT;
    while !output.is_char_boundary(start) {
        start += 1;
    }
    &output[start..]
}

/// A project's hue in degrees: the one chosen for it, else a stable one from its name (the
/// desktop's `ui::project_hue`, in degrees instead of 0–1).
pub fn project_hue(name: &str, chosen: Option<u16>) -> u16 {
    match chosen {
        Some(deg) => deg % 360,
        None => (name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32) % 360) as u16,
    }
}

/// A project's two-letter badge (the desktop's monogram): the first two letters of a one-word
/// name, else the initials of its first two words; `·` for a name without letters.
pub fn monogram(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let letters: String = match words.as_slice() {
        [] => "·".into(),
        [one] => one.chars().take(2).collect(),
        [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
    };
    letters.to_uppercase()
}
