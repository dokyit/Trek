//! What the server needs from the app: the [`RemoteHost`] trait, the [`HostEvent`]s the app pushes,
//! and [`ChannelHost`], an adapter that turns trait calls into messages for a GPUI-style app to
//! drain on its own executor.

use std::future::Future;

use tokio::sync::oneshot;

use crate::protocol::{
    AnswerRequest, Basecamp, BasecampRange, CommandInfo, ErrorCode, GitBranches, GitCommitRequest, GitDiff, GitDiffRequest,
    GitStatus, GitSwitchRequest, GitTarget, Item, MacSettings, NewThreadRequest, Note, NoteSummary, Open, PrefsRequest,
    SaveNoteRequest, SendRequest, SettingsChange, Snapshot, ThreadActionRequest, ThreadSummary, Transcript, TranscriptPage,
    TurnActionDone, TurnActionRequest, Usage, WorktreeRemoveRequest,
};

pub type HostResult<T> = Result<T, HostError>;

/// A failed host call; the phone receives it as `error {code, message}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct HostError {
    pub code: ErrorCode,
    pub message: String,
}

impl HostError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// No such thread, project, agent or request.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// The request conflicts with the current state (e.g. it was already answered).
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// The request is malformed for this host (e.g. an answer of the wrong kind).
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    /// Anything else that went wrong on the Mac.
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::HostError, message)
    }
}

/// The app behind the server. Every method is called from the server's tokio tasks.
pub trait RemoteHost: Send + Sync + 'static {
    /// Every thread (with its section), project and agent.
    fn snapshot(&self) -> impl Future<Output = HostResult<Snapshot>> + Send;
    /// A thread's full transcript, with stable item ids and a monotonic per-thread seq.
    fn transcript(&self, thread_id: &str) -> impl Future<Output = HostResult<Transcript>> + Send;
    /// A follow-up message (steer, queue, or start a turn on an idle thread). `Some` when the
    /// phone carries on by itself (`/new` opens its new-thread sheet).
    fn send(&self, req: SendRequest) -> impl Future<Output = HostResult<Option<Open>>> + Send;
    /// Start a thread; returns its id.
    fn new_thread(&self, req: NewThreadRequest) -> impl Future<Output = HostResult<String>> + Send;
    /// Answer a pending approval, question or plan.
    fn answer(&self, req: AnswerRequest) -> impl Future<Output = HostResult<()>> + Send;
    /// Stop a running turn.
    fn interrupt(&self, thread_id: &str) -> impl Future<Output = HostResult<()>> + Send;
    /// The user saw the thread on the phone.
    fn mark_seen(&self, thread_id: &str) -> impl Future<Output = HostResult<()>> + Send {
        let _ = thread_id;
        async { Ok(()) }
    }
    /// Change a thread's agent, model, effort, access or plan mode.
    fn set_prefs(&self, req: PrefsRequest) -> impl Future<Output = HostResult<()>> + Send {
        let _ = req;
        async { Err(HostError::bad_request("This Mac can't change a thread's settings")) }
    }
    /// Pin, settle, archive or rename a thread.
    fn thread_action(&self, req: ThreadActionRequest) -> impl Future<Output = HostResult<()>> + Send {
        let _ = req;
        async { Err(HostError::bad_request("This Mac can't do that to a thread")) }
    }

    // What follows came after the first phones: a host that doesn't do it says so (an older Mac
    // answers `bad_request` for the unknown type itself).

    /// Plan limits of every agent that reports them.
    fn usage(&self) -> impl Future<Output = HostResult<Usage>> + Send {
        unsupported("show usage")
    }
    /// The Basecamp recap of `range`.
    fn basecamp(&self, range: BasecampRange) -> impl Future<Output = HostResult<Basecamp>> + Send {
        let _ = range;
        unsupported("show Basecamp")
    }
    /// Every note, newest first.
    fn notes(&self) -> impl Future<Output = HostResult<Vec<NoteSummary>>> + Send {
        unsupported("show notes")
    }
    fn note(&self, note_id: &str) -> impl Future<Output = HostResult<Note>> + Send {
        let _ = note_id;
        unsupported("show notes")
    }
    fn create_note(&self, body: String) -> impl Future<Output = HostResult<Note>> + Send {
        let _ = body;
        unsupported("write notes")
    }
    /// Write a note; `conflict` when it changed since the phone's copy.
    fn save_note(&self, req: SaveNoteRequest) -> impl Future<Output = HostResult<Note>> + Send {
        let _ = req;
        unsupported("write notes")
    }
    fn delete_note(&self, note_id: &str) -> impl Future<Output = HostResult<()>> + Send {
        let _ = note_id;
        unsupported("delete notes")
    }
    fn git_status(&self, target: GitTarget) -> impl Future<Output = HostResult<GitStatus>> + Send {
        let _ = target;
        unsupported("show git")
    }
    fn git_diff(&self, req: GitDiffRequest) -> impl Future<Output = HostResult<GitDiff>> + Send {
        let _ = req;
        unsupported("show git")
    }
    fn git_commit(&self, req: GitCommitRequest) -> impl Future<Output = HostResult<()>> + Send {
        let _ = req;
        unsupported("commit")
    }
    fn git_push(&self, target: GitTarget) -> impl Future<Output = HostResult<()>> + Send {
        let _ = target;
        unsupported("push")
    }
    fn git_branches(&self, target: GitTarget) -> impl Future<Output = HostResult<GitBranches>> + Send {
        let _ = target;
        unsupported("list branches")
    }
    fn git_switch(&self, req: GitSwitchRequest) -> impl Future<Output = HostResult<()>> + Send {
        let _ = req;
        unsupported("switch branches")
    }
    fn worktree_merge(&self, thread_id: &str) -> impl Future<Output = HostResult<()>> + Send {
        let _ = thread_id;
        unsupported("merge worktrees")
    }
    fn worktree_remove(&self, req: WorktreeRemoveRequest) -> impl Future<Output = HostResult<()>> + Send {
        let _ = req;
        unsupported("remove worktrees")
    }
    /// The slash commands a thread offers.
    fn commands(&self, thread_id: &str) -> impl Future<Output = HostResult<Vec<CommandInfo>>> + Send {
        let _ = thread_id;
        unsupported("list commands")
    }
    fn settings(&self) -> impl Future<Output = HostResult<MacSettings>> + Send {
        unsupported("show its settings")
    }
    /// Change the settings a phone may; returns them as they are now.
    fn set_settings(&self, change: SettingsChange) -> impl Future<Output = HostResult<MacSettings>> + Send {
        let _ = change;
        unsupported("change its settings")
    }
    /// `transcript` for a phone that has seen up to `after_seq` and wants at most the last
    /// `limit` items of a full one. A host may serve just that (the server cuts it to fit either
    /// way): only items newer than `after_seq` when `base <= after_seq <= seq`, else the last
    /// `limit` items with `more` set when it left any out. By default, the whole transcript.
    fn transcript_for(&self, thread_id: &str, after_seq: Option<u64>, limit: Option<u32>) -> impl Future<Output = HostResult<Transcript>> + Send {
        let _ = (after_seq, limit);
        self.transcript(thread_id)
    }
    /// The items just before item `before` (at most `limit`, already capped at `MAX_PAGE`). By
    /// default cut from the whole transcript.
    fn transcript_before(&self, thread_id: &str, before: &str, limit: u32) -> impl Future<Output = HostResult<TranscriptPage>> + Send {
        async move {
            let transcript = self.transcript(thread_id).await?;
            transcript.page_before(before, limit).ok_or_else(|| HostError::not_found("No such item"))
        }
    }
    /// Undo, retry, fork or rewind a turn, as the Mac's buttons do.
    fn turn_action(&self, req: TurnActionRequest) -> impl Future<Output = HostResult<TurnActionDone>> + Send {
        let _ = req;
        unsupported("undo, retry or fork turns")
    }
    /// No phone has `thread_id` open any more (its last subscriber unsubscribed or went away):
    /// the host can stop following its transcript. A `transcript` call for it after this starts
    /// following it again. Called as it happens, in order with the calls that follow.
    fn unwatch(&self, thread_id: &str) {
        let _ = thread_id;
    }
}

fn unsupported<T>(what: &str) -> std::future::Ready<HostResult<T>> {
    std::future::ready(Err(HostError::bad_request(format!("This Mac can't {what}"))))
}

/// A change the app pushes to connected phones through [`crate::RemoteHandle::push`].
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // short-lived: one per change, straight to the phones
pub enum HostEvent {
    /// Replace everything (to every authenticated phone).
    Snapshot(Snapshot),
    /// Upsert one thread (to every authenticated phone).
    Thread(ThreadSummary),
    /// A thread is gone, by id (to every authenticated phone).
    ThreadRemoved(String),
    /// Upsert one transcript item (to phones subscribed to the thread).
    Item { thread_id: String, item: Item },
    /// The thread was rewound or rewritten; subscribers re-subscribe (to phones subscribed to it).
    TranscriptReset(String),
}

// ---------------------------------------------------------------------------------------------
// ChannelHost
// ---------------------------------------------------------------------------------------------

/// A reply slot for one [`HostRequest`].
pub type Reply<T> = oneshot::Sender<HostResult<T>>;

/// One [`RemoteHost`] call, made into a message. Answer it by sending on `reply` (dropping the
/// reply sends `host_error` to the phone).
#[derive(Debug)]
pub enum HostRequest {
    Snapshot { reply: Reply<Snapshot> },
    /// See [`RemoteHost::transcript_for`].
    Transcript { thread_id: String, after_seq: Option<u64>, limit: Option<u32>, reply: Reply<Transcript> },
    TranscriptBefore { thread_id: String, before: String, limit: u32, reply: Reply<TranscriptPage> },
    TurnAction { req: TurnActionRequest, reply: Reply<TurnActionDone> },
    /// No phone has the thread open any more (nothing to reply).
    Unwatch { thread_id: String },
    Send { req: SendRequest, reply: Reply<Option<Open>> },
    NewThread { req: NewThreadRequest, reply: Reply<String> },
    Answer { req: AnswerRequest, reply: Reply<()> },
    Interrupt { thread_id: String, reply: Reply<()> },
    MarkSeen { thread_id: String, reply: Reply<()> },
    SetPrefs { req: PrefsRequest, reply: Reply<()> },
    ThreadAction { req: ThreadActionRequest, reply: Reply<()> },
    Usage { reply: Reply<Usage> },
    Basecamp { range: BasecampRange, reply: Reply<Basecamp> },
    Notes { reply: Reply<Vec<NoteSummary>> },
    Note { note_id: String, reply: Reply<Note> },
    CreateNote { body: String, reply: Reply<Note> },
    SaveNote { req: SaveNoteRequest, reply: Reply<Note> },
    DeleteNote { note_id: String, reply: Reply<()> },
    GitStatus { target: GitTarget, reply: Reply<GitStatus> },
    GitDiff { req: GitDiffRequest, reply: Reply<GitDiff> },
    GitCommit { req: GitCommitRequest, reply: Reply<()> },
    GitPush { target: GitTarget, reply: Reply<()> },
    GitBranches { target: GitTarget, reply: Reply<GitBranches> },
    GitSwitch { req: GitSwitchRequest, reply: Reply<()> },
    WorktreeMerge { thread_id: String, reply: Reply<()> },
    WorktreeRemove { req: WorktreeRemoveRequest, reply: Reply<()> },
    Commands { thread_id: String, reply: Reply<Vec<CommandInfo>> },
    Settings { reply: Reply<MacSettings> },
    SetSettings { change: SettingsChange, reply: Reply<MacSettings> },
}

/// A [`RemoteHost`] that forwards every call as a [`HostRequest`] over an `async_channel`.
///
/// ```ignore
/// let (host, requests) = ChannelHost::new();
/// let handle = RemoteServer::start(config, Arc::new(host)).await?;
/// cx.spawn(async move |cx| {
///     while let Ok(req) = requests.recv().await {
///         // answer each request from the Workspace/Store, then reply.send(result)
///     }
/// });
/// ```
#[derive(Debug, Clone)]
pub struct ChannelHost {
    tx: async_channel::Sender<HostRequest>,
}

impl ChannelHost {
    pub fn new() -> (Self, async_channel::Receiver<HostRequest>) {
        let (tx, rx) = async_channel::unbounded();
        (Self { tx }, rx)
    }

    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> HostRequest) -> HostResult<T> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(make(reply)).await.map_err(|_| HostError::other("Trek isn't answering remote requests"))?;
        rx.await.map_err(|_| HostError::other("Trek dropped the request"))?
    }
}

impl RemoteHost for ChannelHost {
    async fn snapshot(&self) -> HostResult<Snapshot> {
        self.call(|reply| HostRequest::Snapshot { reply }).await
    }

    async fn transcript(&self, thread_id: &str) -> HostResult<Transcript> {
        self.transcript_for(thread_id, None, None).await
    }

    async fn transcript_for(&self, thread_id: &str, after_seq: Option<u64>, limit: Option<u32>) -> HostResult<Transcript> {
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::Transcript { thread_id, after_seq, limit, reply }).await
    }

    async fn transcript_before(&self, thread_id: &str, before: &str, limit: u32) -> HostResult<TranscriptPage> {
        let (thread_id, before) = (thread_id.to_string(), before.to_string());
        self.call(|reply| HostRequest::TranscriptBefore { thread_id, before, limit, reply }).await
    }

    async fn turn_action(&self, req: TurnActionRequest) -> HostResult<TurnActionDone> {
        self.call(|reply| HostRequest::TurnAction { req, reply }).await
    }

    fn unwatch(&self, thread_id: &str) {
        // Unbounded: queued at once, ahead of any request made after it.
        let _ = self.tx.try_send(HostRequest::Unwatch { thread_id: thread_id.to_string() });
    }

    async fn send(&self, req: SendRequest) -> HostResult<Option<Open>> {
        self.call(|reply| HostRequest::Send { req, reply }).await
    }

    async fn new_thread(&self, req: NewThreadRequest) -> HostResult<String> {
        self.call(|reply| HostRequest::NewThread { req, reply }).await
    }

    async fn answer(&self, req: AnswerRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::Answer { req, reply }).await
    }

    async fn interrupt(&self, thread_id: &str) -> HostResult<()> {
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::Interrupt { thread_id, reply }).await
    }

    async fn mark_seen(&self, thread_id: &str) -> HostResult<()> {
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::MarkSeen { thread_id, reply }).await
    }

    async fn set_prefs(&self, req: PrefsRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::SetPrefs { req, reply }).await
    }

    async fn thread_action(&self, req: ThreadActionRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::ThreadAction { req, reply }).await
    }

    async fn usage(&self) -> HostResult<Usage> {
        self.call(|reply| HostRequest::Usage { reply }).await
    }

    async fn basecamp(&self, range: BasecampRange) -> HostResult<Basecamp> {
        self.call(|reply| HostRequest::Basecamp { range, reply }).await
    }

    async fn notes(&self) -> HostResult<Vec<NoteSummary>> {
        self.call(|reply| HostRequest::Notes { reply }).await
    }

    async fn note(&self, note_id: &str) -> HostResult<Note> {
        let note_id = note_id.to_string();
        self.call(|reply| HostRequest::Note { note_id, reply }).await
    }

    async fn create_note(&self, body: String) -> HostResult<Note> {
        self.call(|reply| HostRequest::CreateNote { body, reply }).await
    }

    async fn save_note(&self, req: SaveNoteRequest) -> HostResult<Note> {
        self.call(|reply| HostRequest::SaveNote { req, reply }).await
    }

    async fn delete_note(&self, note_id: &str) -> HostResult<()> {
        let note_id = note_id.to_string();
        self.call(|reply| HostRequest::DeleteNote { note_id, reply }).await
    }

    async fn git_status(&self, target: GitTarget) -> HostResult<GitStatus> {
        self.call(|reply| HostRequest::GitStatus { target, reply }).await
    }

    async fn git_diff(&self, req: GitDiffRequest) -> HostResult<GitDiff> {
        self.call(|reply| HostRequest::GitDiff { req, reply }).await
    }

    async fn git_commit(&self, req: GitCommitRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::GitCommit { req, reply }).await
    }

    async fn git_push(&self, target: GitTarget) -> HostResult<()> {
        self.call(|reply| HostRequest::GitPush { target, reply }).await
    }

    async fn git_branches(&self, target: GitTarget) -> HostResult<GitBranches> {
        self.call(|reply| HostRequest::GitBranches { target, reply }).await
    }

    async fn git_switch(&self, req: GitSwitchRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::GitSwitch { req, reply }).await
    }

    async fn worktree_merge(&self, thread_id: &str) -> HostResult<()> {
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::WorktreeMerge { thread_id, reply }).await
    }

    async fn worktree_remove(&self, req: WorktreeRemoveRequest) -> HostResult<()> {
        self.call(|reply| HostRequest::WorktreeRemove { req, reply }).await
    }

    async fn commands(&self, thread_id: &str) -> HostResult<Vec<CommandInfo>> {
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::Commands { thread_id, reply }).await
    }

    async fn settings(&self) -> HostResult<MacSettings> {
        self.call(|reply| HostRequest::Settings { reply }).await
    }

    async fn set_settings(&self, change: SettingsChange) -> HostResult<MacSettings> {
        self.call(|reply| HostRequest::SetSettings { change, reply }).await
    }
}
