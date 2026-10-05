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
    },
    /// Stop receiving a thread's transcript.
    Unsubscribe { thread_id: String },
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
            Self::Send(_) => "send",
            Self::NewThread(_) => "new_thread",
            Self::Answer(_) => "answer",
            Self::Interrupt { .. } => "interrupt",
            Self::MarkSeen { .. } => "mark_seen",
            Self::SetPrefs(_) => "set_prefs",
            Self::ThreadAction(_) => "thread_action",
            Self::Ping => "ping",
        }
    }
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
    },
    /// The reply to `ping`.
    Pong,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A project, as referenced from a thread row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRef {
    /// Trek's `AgentId::key()`.
    pub key: String,
    pub name: String,
}

/// An agent the phone can start threads with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOption {
    pub key: String,
    pub name: String,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Idle,
    Working,
    NeedsYou,
    Failed,
}

/// Trek's sidebar `Section`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Pinned,
    Inbox,
    Working,
    Snoozed,
    Settled,
}

// ---------------------------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------------------------

/// A thread's transcript as the host serves it: every item, and the highest seq handed out so far.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Transcript {
    pub seq: u64,
    pub items: Vec<Item>,
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
