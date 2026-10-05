//! What the server needs from the app: the [`RemoteHost`] trait, the [`HostEvent`]s the app pushes,
//! and [`ChannelHost`], an adapter that turns trait calls into messages for a GPUI-style app to
//! drain on its own executor.

use std::future::Future;

use tokio::sync::oneshot;

use crate::protocol::{
    AnswerRequest, ErrorCode, Item, NewThreadRequest, PrefsRequest, SendRequest, Snapshot, ThreadActionRequest, ThreadSummary,
    Transcript,
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
    /// A follow-up message (steer, queue, or start a turn on an idle thread).
    fn send(&self, req: SendRequest) -> impl Future<Output = HostResult<()>> + Send;
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
}

/// A change the app pushes to connected phones through [`crate::RemoteHandle::push`].
#[derive(Debug, Clone, PartialEq)]
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
    Transcript { thread_id: String, reply: Reply<Transcript> },
    Send { req: SendRequest, reply: Reply<()> },
    NewThread { req: NewThreadRequest, reply: Reply<String> },
    Answer { req: AnswerRequest, reply: Reply<()> },
    Interrupt { thread_id: String, reply: Reply<()> },
    MarkSeen { thread_id: String, reply: Reply<()> },
    SetPrefs { req: PrefsRequest, reply: Reply<()> },
    ThreadAction { req: ThreadActionRequest, reply: Reply<()> },
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
        let thread_id = thread_id.to_string();
        self.call(|reply| HostRequest::Transcript { thread_id, reply }).await
    }

    async fn send(&self, req: SendRequest) -> HostResult<()> {
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
}
