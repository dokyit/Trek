//! The other end of the protocol: one Trek talking to another as the iPhone app does
//! (`ios/Trek/Net/TrekClient.swift`, `docs/MULTI-MACHINE.md`).
//!
//! It dials every route the machine has (and any [`Client::also_try`] suggests) at once, the
//! earlier ones a moment ahead, and keeps the first that answers, over TLS pinned to the SHA-256
//! of the machine's certificate (nothing else is trusted, and nothing is sent before the pin
//! matched), then `pair`s with a one-time code the first time or says `hello` with its device
//! token after. Requests are matched to their replies by `id`; everything else the machine sends
//! (snapshots, rows, transcript items, request cards, clone progress) comes out of
//! [`Client::events`]. It pings every 25 s and drops a connection silent for 62 s, reconnects with
//! a backoff doubling up to 30 s (at once after [`Client::network_changed`]), subscribes again to the threads that were open once `welcome`
//! is in (not before: the machine would refuse them), and stops for good when the machine
//! refuses the token or the code, answers with another certificate on every route, or speaks
//! another protocol version.
//!
//! ```ignore
//! let client = Client::connect(routes, &fingerprint, Auth::Pair { code, device_id, device_name })?;
//! let mut events = client.events().unwrap();
//! while let Some(event) = events.recv().await {
//!     match event {
//!         Event::Connected { paired, .. } => save(paired),        // token, fingerprint, routes
//!         Event::Message(ServerMessage::Item { thread_id, item }) => { /* … */ }
//!         _ => {}
//!     }
//! }
//! let info = client.machine_info().await?;
//! let reply = client.request(ClientMessage::Subscribe { thread_id, after_seq: None, limit: Some(200) }).await?;
//! ```
//!
//! Its futures and channels are tokio's `sync` ones, which any executor can await (GPUI's
//! included); the connection itself runs on the tokio runtime `connect` is called in, or on a
//! small one of its own when there is none.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    CryptoProvider, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{DigitallySignedStruct, SignatureScheme};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::protocol::{
    ClientEnvelope, ClientMessage, CloneProjectRequest, ErrorCode, FileUpload, HostInfo,
    MachineInfo, PROTOCOL_VERSION, PutFile, PutFilesRequest, ServerEnvelope, ServerMessage,
};

/// The largest message taken from the machine (above anything it sends; one over it ends the
/// connection, as reconnecting would only meet it again).
pub const MAX_INBOUND: usize = 16 << 20;
/// The largest message sent: the machine's own limit for what it takes.
pub const MAX_OUTBOUND: usize = 8 << 20;

const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How a client times what it does. The defaults are the phone's.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// How often it pings (`ping`, answered with `pong`).
    pub ping_every: Duration,
    /// A connection the machine has said nothing on for this long is dropped and made again.
    pub silence: Duration,
    /// The wait after the first failed attempt; it doubles with each one after, up to `backoff_max`.
    pub backoff_first: Duration,
    pub backoff_max: Duration,
    /// How long a route has to answer (TCP, TLS and the WebSocket upgrade), and the machine to
    /// answer `pair` or `hello`.
    pub connect_timeout: Duration,
    /// How far each route is dialled behind the one before it: every route is tried at once,
    /// the one that answered last a little ahead, so a dead one costs no more than this.
    pub stagger: Duration,
    /// After [`Client::network_changed`], how long a connection that was up has to answer a ping
    /// before it's taken for gone (it may have left with the old network).
    pub probe: Duration,
    /// Sent with `pair` and `hello`.
    pub app_version: Option<String>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            ping_every: Duration::from_secs(25),
            silence: Duration::from_secs(62),
            backoff_first: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            stagger: Duration::from_millis(300),
            probe: Duration::from_secs(5),
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    }
}

/// How the client authenticates.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// The first time: the code the machine shows (`ABCD-1234`), and who this is.
    Pair {
        code: String,
        device_id: String,
        device_name: String,
    },
    /// After: the device token `pair` gave.
    Hello { device_id: String, token: String },
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pair {
                device_id,
                device_name,
                ..
            } => f
                .debug_struct("Pair")
                .field("device_id", device_id)
                .field("device_name", device_name)
                .finish_non_exhaustive(),
            Self::Hello { device_id, .. } => f
                .debug_struct("Hello")
                .field("device_id", device_id)
                .finish_non_exhaustive(),
        }
    }
}

/// What to keep of a paired machine to connect to it again ([`Client::resume`]). Every
/// [`Event::Connected`] carries it as it is then (the machine's routes and features may change).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paired {
    pub device_id: String,
    /// The device token: a secret.
    pub token: String,
    /// The SHA-256 of the machine's certificate, 64 lowercase hex characters.
    pub fingerprint: String,
    pub host: HostInfo,
    /// Every `host:port` the machine answers on, the one that answered last first.
    pub routes: Vec<String>,
    /// The optional requests it serves ([`crate::protocol::feature`]).
    #[serde(default)]
    pub features: Vec<String>,
}

impl Paired {
    /// `hello` with this token.
    pub fn auth(&self) -> Auth {
        Auth::Hello {
            device_id: self.device_id.clone(),
            token: self.token.clone(),
        }
    }

    /// Whether the machine serves `feature`.
    pub fn serves(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired")
            .field("device_id", &self.device_id)
            .field("fingerprint", &self.fingerprint)
            .field("host", &self.host)
            .field("routes", &self.routes)
            .field("features", &self.features)
            .finish_non_exhaustive()
    }
}

/// Where the connection stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Dialling, authenticating, or waiting to try again.
    Connecting,
    Connected,
    /// For good: a new [`Client`] is needed (after pairing again, for most reasons).
    Stopped(StopReason),
}

/// Why a client stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StopReason {
    /// The machine doesn't know the token (never paired, or revoked).
    #[error("this machine isn't paired with it any more: {0}")]
    Unauthorized(String),
    /// The pairing code was refused.
    #[error("pairing failed: {0}")]
    PairingFailed(String),
    /// Every route answered with a certificate other than the pinned one.
    #[error("the machine's certificate changed (expected {expected}, got {seen})")]
    FingerprintMismatch { expected: String, seen: String },
    /// It speaks another protocol version.
    #[error("the machines speak different protocol versions: {0}")]
    ProtocolMismatch(String),
    /// [`Client::close`], or every [`Client`] dropped.
    #[error("closed")]
    Closed,
}

/// What comes out of [`Client::events`].
#[derive(Debug, Clone)]
pub enum Event {
    /// Authenticated on `route`: `newly_paired` after `pair` (keep `paired`, it holds the
    /// token), else after `hello` (keep it too: the routes may have changed). Threads open
    /// before are subscribed to again right after.
    Connected {
        route: String,
        paired: Paired,
        newly_paired: bool,
    },
    /// The connection dropped; the client is connecting again. Requests in flight failed with
    /// [`ClientError::Disconnected`].
    Disconnected { reason: String },
    /// Something the machine sent unasked: `snapshot`, `thread`, `unlisted`, `thread_removed`,
    /// `item` (request cards among them), `transcript_reset`, `clone_progress`; and the
    /// `transcript` answering a subscription made again after a reconnect.
    Message(ServerMessage),
}

/// A request that didn't get its reply.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClientError {
    /// The machine answered `error`.
    #[error("{code}: {message}")]
    Server { code: ErrorCode, message: String },
    /// The connection dropped before the reply came (the request may or may not have been done).
    #[error("the connection dropped before the reply came")]
    Disconnected,
    #[error("the client stopped: {0}")]
    Stopped(StopReason),
    /// The message would be over [`MAX_OUTBOUND`] bytes: it wasn't sent.
    #[error("{0} bytes is more than one message may carry")]
    TooLarge(usize),
    /// A reply of another kind than the request takes.
    #[error("unexpected reply: {0}")]
    Unexpected(String),
    /// A fingerprint that isn't a SHA-256, or no routes.
    #[error("{0}")]
    Invalid(String),
}

type Responder = oneshot::Sender<Result<ServerMessage, ClientError>>;

enum Cmd {
    Request {
        msg: ClientMessage,
        reply: Responder,
    },
    /// The network changed: dial now, or check the connection that's up.
    NetworkChanged,
    /// More `host:port`s that may reach the machine.
    AlsoTry(Vec<String>),
    Close,
}

/// A connection to another Trek, kept up until it's closed or the machine refuses it. Dropping
/// every handle closes it.
pub struct Client {
    cmds: mpsc::UnboundedSender<Cmd>,
    state: watch::Receiver<State>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Connect to the machine at the first of `routes` (`host:port`) that answers, accepting only
    /// a certificate whose SHA-256 is `fingerprint`, and authenticate with `auth`. Returns at
    /// once; [`Client::state`] and [`Client::events`] say how it goes.
    pub fn connect(
        routes: Vec<String>,
        fingerprint: &str,
        auth: Auth,
    ) -> Result<Self, ClientError> {
        Self::connect_with(routes, fingerprint, auth, ClientOptions::default())
    }

    /// Connect again to a paired machine.
    pub fn resume(paired: &Paired) -> Result<Self, ClientError> {
        Self::connect(paired.routes.clone(), &paired.fingerprint, paired.auth())
    }

    /// [`Client::connect`] with other timings.
    pub fn connect_with(
        routes: Vec<String>,
        fingerprint: &str,
        auth: Auth,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let fingerprint = fingerprint.trim().to_ascii_lowercase();
        if fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ClientError::Invalid(
                "The fingerprint must be a SHA-256, 64 hex characters".into(),
            ));
        }
        let mut clean: Vec<String> = Vec::new();
        for route in routes
            .iter()
            .map(|r| bare_route(r))
            .filter(|r| !r.is_empty())
        {
            if !clean.contains(&route) {
                clean.push(route);
            }
        }
        if clean.is_empty() {
            return Err(ClientError::Invalid("No address to dial".into()));
        }
        // Each dial makes its own; one now says whether it can be made at all.
        connector(fingerprint.clone(), Arc::default())?;
        let (cmds, cmds_rx) = mpsc::unbounded_channel();
        let (state, state_rx) = watch::channel(State::Connecting);
        let (events, events_rx) = mpsc::unbounded_channel();
        let driver = Driver {
            options,
            routes: clean,
            hints: Vec::new(),
            said: HashMap::new(),
            fingerprint,
            auth,
            cmds: cmds_rx,
            state,
            events,
            waiting: VecDeque::new(),
            open: HashMap::new(),
            next_id: 0,
        };
        spawn(driver.run());
        Ok(Self {
            cmds,
            state: state_rx,
            events: Mutex::new(Some(events_rx)),
        })
    }

    /// Send `msg` and wait for its reply: the machine's `error` is [`ClientError::Server`].
    /// Made while no connection is up, it goes once one is. `subscribe` and `unsubscribe` also
    /// say which threads to subscribe to again after a reconnect. There's no timeout: wrap it in
    /// one where it matters.
    pub async fn request(&self, msg: ClientMessage) -> Result<ServerMessage, ClientError> {
        let (reply, rx) = oneshot::channel();
        if self.cmds.send(Cmd::Request { msg, reply }).is_err() {
            return Err(self.gone());
        }
        rx.await.unwrap_or_else(|_| Err(self.gone()))
    }

    fn gone(&self) -> ClientError {
        match &*self.state.borrow() {
            State::Stopped(reason) => ClientError::Stopped(reason.clone()),
            _ => ClientError::Disconnected,
        }
    }

    /// Everything the machine sends unasked, and the connection's ups and downs, in order. There
    /// is one receiver: the first call takes it, later ones get `None`.
    pub fn events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Where the connection stands, now and as it changes.
    pub fn state(&self) -> watch::Receiver<State> {
        self.state.clone()
    }

    /// Close the connection for good; requests waiting fail with [`StopReason::Closed`].
    pub fn close(&self) {
        let _ = self.cmds.send(Cmd::Close);
    }

    /// This computer joined another network, or Tailscale came up: a client waiting to dial again
    /// dials now, from the start of its backoff; one dialling starts over (the routes it was
    /// trying may have left with the old network); one connected pings, and takes the connection
    /// for gone if nothing answers within [`ClientOptions::probe`].
    pub fn network_changed(&self) {
        let _ = self.cmds.send(Cmd::NetworkChanged);
    }

    /// More `host:port`s that may reach the machine (its tailnet address, as this computer's
    /// Tailscale lists it), dialled with its routes from the next attempt on. One kept only once
    /// it answered with the pinned certificate (it then leads the routes [`Event::Connected`]
    /// carries); another certificate there never stops the client.
    pub fn also_try(&self, routes: Vec<String>) {
        let _ = self.cmds.send(Cmd::AlsoTry(routes));
    }

    /// What the machine is: name, platform, Trek's version, home folder.
    pub async fn machine_info(&self) -> Result<MachineInfo, ClientError> {
        match self.request(ClientMessage::MachineInfo).await? {
            ServerMessage::MachineInfo(info) => Ok(info),
            other => Err(unexpected(&other)),
        }
    }

    /// `git clone` on the machine; the new project's id. Git's output comes meanwhile as
    /// `clone_progress` events.
    pub async fn clone_project(&self, req: CloneProjectRequest) -> Result<String, ClientError> {
        match self.request(ClientMessage::CloneProject(req)).await? {
            ServerMessage::Ack {
                project_id: Some(id),
                ..
            } => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    /// Write `files` under the project's folder on the machine, in as many `put_files` as it
    /// takes to keep each under the size limits.
    pub async fn put_files(
        &self,
        project_id: &str,
        files: &[FileUpload],
    ) -> Result<(), ClientError> {
        // Room for the envelope and the JSON around each file.
        let budget = MAX_OUTBOUND - (64 << 10);
        let mut batch: Vec<PutFile> = Vec::new();
        let (mut size, mut raw) = (0usize, 0usize);
        for file in files {
            let put = PutFile::new(file.path.clone(), &file.bytes, Some(file.mode));
            let cost = put.bytes_b64.len() + put.path.len() * 6 + 64;
            if cost > budget || file.bytes.len() > crate::protocol::MAX_PUT_FILES {
                return Err(ClientError::TooLarge(cost));
            }
            if !batch.is_empty()
                && (size + cost > budget || raw + file.bytes.len() > crate::protocol::MAX_PUT_FILES)
            {
                self.put_batch(project_id, std::mem::take(&mut batch))
                    .await?;
                (size, raw) = (0, 0);
            }
            batch.push(put);
            size += cost;
            raw += file.bytes.len();
        }
        if !batch.is_empty() {
            self.put_batch(project_id, batch).await?;
        }
        Ok(())
    }

    async fn put_batch(&self, project_id: &str, files: Vec<PutFile>) -> Result<(), ClientError> {
        match self
            .request(ClientMessage::PutFiles(PutFilesRequest {
                project_id: project_id.to_string(),
                files,
            }))
            .await?
        {
            ServerMessage::Ack { .. } => Ok(()),
            other => Err(unexpected(&other)),
        }
    }
}

fn unexpected(msg: &ServerMessage) -> ClientError {
    let mut text = serde_json::to_string(msg).unwrap_or_default();
    if text.len() > 200 {
        let mut end = 200;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    ClientError::Unexpected(text)
}

/// `host:port` without any scheme or trailing slash typed with it.
fn bare_route(route: &str) -> String {
    let mut r = route.trim();
    for scheme in ["wss://", "ws://", "https://", "http://"] {
        if r.len() >= scheme.len() && r[..scheme.len()].eq_ignore_ascii_case(scheme) {
            r = &r[scheme.len()..];
        }
    }
    r.trim_end_matches('/').to_string()
}

/// Run the connection on the caller's tokio runtime, or on one of the client's own.
fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(future);
        }
        Err(_) => {
            static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
            let runtime = RUNTIME.get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("trek-remote-client")
                    .enable_all()
                    .build()
                    .expect("a tokio runtime for trek-remote's client")
            });
            runtime.spawn(future);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// TLS
// ---------------------------------------------------------------------------------------------

/// Accepts exactly the certificate whose SHA-256 is `fingerprint`; remembers one it refused.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
    rejected: Arc<Mutex<Option<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = crate::tls::fingerprint(cert);
        if seen == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            *lock(&self.rejected) = Some(seen);
            Err(rustls::Error::General(
                "not the paired machine's certificate".into(),
            ))
        }
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn connector(
    fingerprint: String,
    rejected: Arc<Mutex<Option<String>>>,
) -> Result<TlsConnector, ClientError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ClientError::Invalid(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint,
            provider,
            rejected,
        }))
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

type Ws = WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;

/// Why a route didn't answer.
enum Failure {
    /// With another certificate (its fingerprint).
    Mismatch(String),
    Other(String),
}

impl Failure {
    /// As the log says it.
    fn said(&self) -> String {
        match self {
            Self::Mismatch(seen) => {
                format!("another certificate ({}…)", seen.get(..16).unwrap_or(seen))
            }
            Self::Other(why) => why.clone(),
        }
    }
}

/// TCP, TLS (pinned to `fingerprint`) and the WebSocket upgrade, to one route.
async fn open(route: &str, fingerprint: &str) -> Result<Ws, Failure> {
    let rejected = Arc::new(Mutex::new(None));
    let tls = connector(fingerprint.to_string(), rejected.clone())
        .map_err(|e| Failure::Other(e.to_string()))?;
    let tcp = TcpStream::connect(route)
        .await
        .map_err(|e| Failure::Other(e.to_string()))?;
    let _ = tcp.set_nodelay(true);
    let name = ServerName::try_from("trek.local").map_err(|e| Failure::Other(e.to_string()))?;
    let stream = match tls.connect(name, tcp).await {
        Ok(stream) => stream,
        Err(err) => {
            return Err(match lock(&rejected).take() {
                Some(seen) => Failure::Mismatch(seen),
                None => Failure::Other(format!("TLS: {err}")),
            });
        }
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_INBOUND))
        .max_frame_size(Some(MAX_INBOUND));
    let (ws, _) = tokio_tungstenite::client_async_with_config(
        format!("wss://{route}/"),
        stream,
        Some(config),
    )
    .await
    .map_err(|e| Failure::Other(format!("WebSocket: {e}")))?;
    Ok(ws)
}

/// How one route did.
struct Dialled {
    ix: usize,
    /// `None`: it answered.
    failure: Option<Failure>,
    took: Duration,
}

/// Every route at once, each `stagger` behind the one before (the first is the one that
/// answered last): the first that answers (its index), and how each one dialled did. Those
/// still dialling when one answers are dropped.
async fn open_any(
    routes: Vec<String>,
    fingerprint: String,
    timeout: Duration,
    stagger: Duration,
) -> (Option<(usize, Ws)>, Vec<Dialled>) {
    let mut tries: futures_util::stream::FuturesUnordered<_> = routes
        .iter()
        .enumerate()
        .map(|(ix, route)| {
            let fingerprint = fingerprint.as_str();
            async move {
                tokio::time::sleep(stagger.saturating_mul(ix as u32)).await;
                let started = Instant::now();
                let opened = tokio::time::timeout(timeout, open(route, fingerprint))
                    .await
                    .unwrap_or_else(|_| Err(Failure::Other("no answer".into())));
                (ix, opened, started.elapsed())
            }
        })
        .collect();
    let mut dialled = Vec::new();
    while let Some((ix, opened, took)) = tries.next().await {
        match opened {
            Ok(ws) => {
                dialled.push(Dialled {
                    ix,
                    failure: None,
                    took,
                });
                return (Some((ix, ws)), dialled);
            }
            Err(failure) => dialled.push(Dialled {
                ix,
                failure: Some(failure),
                took,
            }),
        }
    }
    (None, dialled)
}

/// Whether `text` is the machine's `Pong` to the ping sent with `id`.
fn answers(text: &str, id: &str) -> bool {
    matches!(
        serde_json::from_str::<ServerEnvelope>(text),
        Ok(ServerEnvelope { msg: ServerMessage::Pong, re: Some(re) }) if re == id
    )
}

async fn write(ws: &mut Ws, message: Message) -> Result<(), String> {
    match tokio::time::timeout(WRITE_TIMEOUT, ws.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("writing timed out".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The connection
// ---------------------------------------------------------------------------------------------

/// A thread the caller has open, to subscribe to again after a reconnect.
#[derive(Debug, Default)]
struct OpenThread {
    /// The newest `seq` seen of it, while it's known (not after a `transcript_reset`).
    seq: Option<u64>,
    limit: Option<u32>,
}

/// A request sent and not answered yet.
struct Pending {
    /// `None` for subscriptions made again after a reconnect: their reply is an event.
    reply: Option<Responder>,
    /// The thread, for a `subscribe`.
    subscribe: Option<String>,
}

enum Ended {
    Dropped(String),
    Stop(StopReason),
}

enum Authed {
    /// `paired` or `welcome`.
    Ok(ServerMessage),
    Stop(StopReason),
    Retry(String),
}

/// The most routes a client keeps (the machine's own first, then those it said before).
const MAX_ROUTES: usize = 8;

/// What the connection heard from its [`Client`] while it wasn't serving one.
enum Heard {
    Nothing,
    /// The network changed: dial now.
    Now,
    Close,
}

struct Driver {
    options: ClientOptions,
    /// The one that answered last first.
    routes: Vec<String>,
    /// [`Client::also_try`]'s, none of them in `routes`.
    hints: Vec<String>,
    /// How each route did when last dialled, as logged: a route doing as before is logged at
    /// debug only, so a machine away for a day doesn't fill the log.
    said: HashMap<String, String>,
    fingerprint: String,
    auth: Auth,
    cmds: mpsc::UnboundedReceiver<Cmd>,
    state: watch::Sender<State>,
    events: mpsc::UnboundedSender<Event>,
    /// Requests made while no connection was up, sent once one is.
    waiting: VecDeque<(ClientMessage, Responder)>,
    open: HashMap<String, OpenThread>,
    next_id: u64,
}

impl Driver {
    async fn run(mut self) {
        let reason = self.connect_loop().await;
        tracing::info!(%reason, "trek-remote client stopped");
        self.state.send_replace(State::Stopped(reason.clone()));
        for (_, reply) in self.waiting.drain(..) {
            let _ = reply.send(Err(ClientError::Stopped(reason.clone())));
        }
        self.cmds.close();
        while let Ok(cmd) = self.cmds.try_recv() {
            if let Cmd::Request { reply, .. } = cmd {
                let _ = reply.send(Err(ClientError::Stopped(reason.clone())));
            }
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// A command while no connection is up: requests are held for later.
    fn heard(&mut self, cmd: Option<Cmd>) -> Heard {
        match cmd {
            Some(Cmd::Request { msg, reply }) => {
                self.waiting.push_back((msg, reply));
                Heard::Nothing
            }
            Some(Cmd::AlsoTry(routes)) => {
                self.also_try(routes);
                Heard::Nothing
            }
            Some(Cmd::NetworkChanged) => Heard::Now,
            Some(Cmd::Close) | None => Heard::Close,
        }
    }

    /// [`Client::also_try`]: these in place of the ones suggested before.
    fn also_try(&mut self, routes: Vec<String>) {
        self.hints.clear();
        for route in routes.iter().map(|r| bare_route(r)) {
            if !route.is_empty() && !self.routes.contains(&route) && !self.hints.contains(&route) {
                self.hints.push(route);
            }
        }
        self.hints.truncate(MAX_ROUTES);
    }

    /// How each route dialled did, at info when it did otherwise than the last time.
    fn log_dialled(&mut self, candidates: &[String], dialled: &[Dialled]) {
        for d in dialled {
            let route = &candidates[d.ix];
            let result = d
                .failure
                .as_ref()
                .map_or_else(|| "answered".to_string(), Failure::said);
            let took_ms = d.took.as_millis() as u64;
            let hint = d.ix >= self.routes.len();
            if self.said.get(route) != Some(&result) {
                tracing::info!(route, hint, took_ms, result, "trek-remote client: dialled");
                self.said.insert(route.clone(), result);
            } else {
                tracing::debug!(route, hint, took_ms, result, "trek-remote client: dialled");
            }
        }
    }

    async fn connect_loop(&mut self) -> StopReason {
        let mut failures: u32 = 0;
        loop {
            if failures > 0 {
                let shift = (failures - 1).min(16);
                let delay = self
                    .options
                    .backoff_first
                    .saturating_mul(1 << shift)
                    .min(self.options.backoff_max);
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        cmd = self.cmds.recv() => match self.heard(cmd) {
                            Heard::Nothing => {}
                            Heard::Now => {
                                tracing::info!("trek-remote client: the network changed; dialling now");
                                failures = 0;
                                break;
                            }
                            Heard::Close => return StopReason::Closed,
                        },
                    }
                }
            }
            self.state.send_replace(State::Connecting);
            let candidates: Vec<String> = self.routes.iter().chain(&self.hints).cloned().collect();
            let (opened, dialled) = {
                let attempt = open_any(
                    candidates.clone(),
                    self.fingerprint.clone(),
                    self.options.connect_timeout,
                    self.options.stagger,
                );
                tokio::pin!(attempt);
                let done = loop {
                    tokio::select! {
                        done = &mut attempt => break Some(done),
                        cmd = self.cmds.recv() => match self.heard(cmd) {
                            Heard::Nothing => {}
                            // What it was dialling may have gone with the old network.
                            Heard::Now => break None,
                            Heard::Close => return StopReason::Closed,
                        },
                    }
                };
                match done {
                    Some(done) => done,
                    None => {
                        tracing::info!("trek-remote client: the network changed; dialling again");
                        failures = 0;
                        continue;
                    }
                }
            };
            self.log_dialled(&candidates, &dialled);
            let (ix, mut ws) = match opened {
                Some(opened) => opened,
                None => {
                    // Another certificate on every route of its own: the machine changed, or
                    // something poses as it. Never trust it. (With some routes merely down,
                    // another machine may just have that address on this network now: keep
                    // trying the others. A suggested route is only a guess.)
                    let own = self.routes.len();
                    let seen: Vec<&String> = dialled
                        .iter()
                        .filter(|d| d.ix < own)
                        .filter_map(|d| match &d.failure {
                            Some(Failure::Mismatch(fp)) => Some(fp),
                            _ => None,
                        })
                        .collect();
                    if seen.len() == own
                        && let Some(seen) = seen.first()
                    {
                        return StopReason::FingerprintMismatch {
                            expected: self.fingerprint.clone(),
                            seen: (*seen).clone(),
                        };
                    }
                    failures += 1;
                    continue;
                }
            };
            let route = candidates[ix].clone();
            self.routes.retain(|r| *r != route);
            self.hints.retain(|r| *r != route);
            self.routes.insert(0, route.clone());

            let reply = match authenticate(&mut ws, &self.auth, &self.options).await {
                Authed::Ok(reply) => reply,
                Authed::Stop(reason) => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
                    return reason;
                }
                Authed::Retry(why) => {
                    tracing::debug!(route, why, "trek-remote client: not authenticated");
                    failures += 1;
                    continue;
                }
            };
            let (protocol, token, host, features, routes) = match reply {
                ServerMessage::Paired {
                    protocol,
                    token,
                    host,
                    features,
                    routes,
                } => (protocol, Some(token), host, features, routes),
                ServerMessage::Welcome {
                    protocol,
                    host,
                    features,
                    routes,
                } => (protocol, None, host, features, routes),
                _ => unreachable!("authenticate returns paired or welcome"),
            };
            if protocol != PROTOCOL_VERSION {
                return StopReason::ProtocolMismatch(format!(
                    "this Trek speaks protocol {PROTOCOL_VERSION}, that one {protocol}"
                ));
            }
            let newly_paired = token.is_some();
            let token = match (token, &self.auth) {
                (Some(token), _) => token,
                (None, Auth::Hello { token, .. }) => token.clone(),
                (None, Auth::Pair { .. }) => {
                    return StopReason::ProtocolMismatch("`welcome` in answer to `pair`".into());
                }
            };
            let device_id = match &self.auth {
                Auth::Pair { device_id, .. } | Auth::Hello { device_id, .. } => device_id.clone(),
            };
            self.auth = Auth::Hello {
                device_id: device_id.clone(),
                token: token.clone(),
            };
            // The machine's routes as it says them now, then those it said before: a tailnet
            // address it leaves out while its Tailscale is down still reaches it once it's back.
            let mut merged = vec![route.clone()];
            for r in routes
                .iter()
                .map(|r| bare_route(r))
                .chain(self.routes.iter().cloned())
            {
                if !r.is_empty() && !merged.contains(&r) {
                    merged.push(r);
                }
            }
            merged.truncate(MAX_ROUTES);
            self.hints.retain(|h| !merged.contains(h));
            self.routes = merged;
            let paired = Paired {
                device_id,
                token,
                fingerprint: self.fingerprint.clone(),
                host,
                routes: self.routes.clone(),
                features,
            };
            tracing::info!(route, "trek-remote client connected");
            self.emit(Event::Connected {
                route,
                paired,
                newly_paired,
            });
            self.state.send_replace(State::Connected);

            match self.session(&mut ws).await {
                Ended::Stop(reason) => return reason,
                Ended::Dropped(reason) => {
                    tracing::info!(reason, "trek-remote client: connection dropped");
                    self.emit(Event::Disconnected { reason });
                    // Not connected while it waits to dial again.
                    self.state.send_replace(State::Connecting);
                    // Straight back, after a moment: a machine that drops every connection at
                    // once isn't hammered.
                    failures = 1;
                }
            }
        }
    }

    /// One authenticated connection, until it drops or the client stops.
    async fn session(&mut self, ws: &mut Ws) -> Ended {
        let mut pending: HashMap<String, Pending> = HashMap::new();
        let ended = self.serve(ws, &mut pending).await;
        for (_, p) in pending {
            if let Some(reply) = p.reply {
                let _ = reply.send(Err(match &ended {
                    Ended::Stop(reason) => ClientError::Stopped(reason.clone()),
                    Ended::Dropped(_) => ClientError::Disconnected,
                }));
            }
        }
        ended
    }

    async fn serve(&mut self, ws: &mut Ws, pending: &mut HashMap<String, Pending>) -> Ended {
        // The threads open before, again: now, with `welcome` in (the machine refuses anything
        // before it), and from where they were.
        let again: Vec<ClientMessage> = self
            .open
            .iter()
            .map(|(id, t)| ClientMessage::Subscribe {
                thread_id: id.clone(),
                after_seq: t.seq,
                limit: t.limit,
            })
            .collect();
        for msg in again {
            if let Err(err) = self.send(ws, pending, msg, None).await {
                return Ended::Dropped(err);
            }
        }
        // Then what was asked while no connection was up.
        while let Some((msg, reply)) = self.waiting.pop_front() {
            if let Err(err) = self.send(ws, pending, msg, Some(reply)).await {
                return Ended::Dropped(err);
            }
        }

        let every = self.options.ping_every;
        let mut ping = tokio::time::interval_at(Instant::now() + every, every);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut heard = Instant::now();
        // Set by a network change: when the connection must have answered by. Only the answer to
        // the ping the change sent (`probe_id`) clears it: a message the machine sent before the
        // change can still be on its way, and says nothing about the connection now.
        let mut probe: Option<Instant> = None;
        let mut probes = 0u64;
        let mut probe_id = String::new();
        let never = Instant::now() + Duration::from_secs(365 * 24 * 3600);
        loop {
            tokio::select! {
                frame = ws.next() => {
                    heard = Instant::now();
                    if probe.is_some()
                        && let Some(Ok(Message::Text(text))) = &frame
                        && answers(text, &probe_id)
                    {
                        probe = None;
                    }
                    match frame {
                        None => return Ended::Dropped("closed".into()),
                        Some(Err(err)) => return Ended::Dropped(err.to_string()),
                        Some(Ok(Message::Close(frame))) => {
                            return Ended::Dropped(frame.map(|f| f.reason.to_string()).filter(|r| !r.is_empty()).unwrap_or_else(|| "closed".into()));
                        }
                        Some(Ok(Message::Text(text))) => {
                            if let Some(ended) = self.receive(&text, pending) {
                                if let Ended::Stop(_) = ended {
                                    let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
                                }
                                return ended;
                            }
                        }
                        Some(Ok(_)) => {}
                    }
                }
                cmd = self.cmds.recv() => match cmd {
                    Some(Cmd::Request { msg, reply }) => {
                        if let Err(err) = self.send(ws, pending, msg, Some(reply)).await {
                            return Ended::Dropped(err);
                        }
                    }
                    Some(Cmd::AlsoTry(routes)) => self.also_try(routes),
                    // The connection may have left with the old network, and a dead one is
                    // otherwise only noticed after `silence`: it has `probe` to answer a ping.
                    Some(Cmd::NetworkChanged) => {
                        if probe.is_none() {
                            probe = Some(Instant::now() + self.options.probe);
                            probes += 1;
                            probe_id = format!("network-probe-{probes}");
                            let ping = serde_json::to_string(&ClientEnvelope { id: Some(probe_id.clone()), msg: ClientMessage::Ping }).expect("ping serializes");
                            if let Err(err) = write(ws, Message::text(ping)).await {
                                return Ended::Dropped(err);
                            }
                        }
                    }
                    Some(Cmd::Close) | None => {
                        let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
                        return Ended::Stop(StopReason::Closed);
                    }
                },
                _ = tokio::time::sleep_until(probe.unwrap_or(never)) => {
                    return Ended::Dropped("the machine stopped answering after the network changed".into());
                }
                _ = ping.tick() => {
                    // Writes may still go through to a machine that no longer answers: only
                    // hearing from it says it's there.
                    if heard.elapsed() > self.options.silence {
                        return Ended::Dropped("the machine stopped answering".into());
                    }
                    let ping = serde_json::to_string(&ClientEnvelope { id: None, msg: ClientMessage::Ping }).expect("ping serializes");
                    if let Err(err) = write(ws, Message::text(ping)).await {
                        return Ended::Dropped(err);
                    }
                }
            }
        }
    }

    /// Send a request with a fresh id.
    async fn send(
        &mut self,
        ws: &mut Ws,
        pending: &mut HashMap<String, Pending>,
        msg: ClientMessage,
        reply: Option<Responder>,
    ) -> Result<(), String> {
        self.next_id += 1;
        let id = format!("c{}", self.next_id);
        let (subscribe, limit, unsubscribe) = match &msg {
            ClientMessage::Subscribe {
                thread_id, limit, ..
            } => (Some(thread_id.clone()), *limit, None),
            ClientMessage::Unsubscribe { thread_id } => (None, None, Some(thread_id.clone())),
            _ => (None, None, None),
        };
        let text = serde_json::to_string(&ClientEnvelope {
            id: Some(id.clone()),
            msg,
        })
        .expect("client messages always serialize");
        if text.len() > MAX_OUTBOUND {
            if let Some(reply) = reply {
                let _ = reply.send(Err(ClientError::TooLarge(text.len())));
            }
            return Ok(());
        }
        if let Some(thread_id) = &subscribe {
            self.open.entry(thread_id.clone()).or_default().limit = limit;
        }
        if let Some(thread_id) = unsubscribe {
            self.open.remove(&thread_id);
        }
        pending.insert(id, Pending { reply, subscribe });
        write(ws, Message::text(text)).await
    }

    /// A message from the machine: a reply goes to its request, the rest out as events. (The
    /// `Pong` to a network change's ping has no request waiting: it only ends the probe.)
    fn receive(&mut self, text: &str, pending: &mut HashMap<String, Pending>) -> Option<Ended> {
        let ServerEnvelope { msg, re } = match serde_json::from_str::<ServerEnvelope>(text) {
            Ok(envelope) => envelope,
            Err(err) => {
                // Something newer than this Trek knows: skipped, as the phone skips it.
                tracing::debug!(%err, "trek-remote client: a message it doesn't know");
                return None;
            }
        };
        if let Some(p) = re.as_deref().and_then(|re| pending.remove(re)) {
            if let Some(thread_id) = &p.subscribe {
                match &msg {
                    ServerMessage::Transcript { seq, .. } => {
                        if let Some(t) = self.open.get_mut(thread_id) {
                            t.seq = Some(*seq);
                        }
                    }
                    ServerMessage::Error { .. } => {
                        self.open.remove(thread_id);
                    }
                    _ => {}
                }
            }
            match p.reply {
                Some(reply) => {
                    let _ = reply.send(match msg {
                        ServerMessage::Error { code, message } => {
                            Err(ClientError::Server { code, message })
                        }
                        msg => Ok(msg),
                    });
                }
                None => self.emit(Event::Message(msg)),
            }
            return None;
        }
        match &msg {
            ServerMessage::Pong => return None,
            // Revoked while connected.
            ServerMessage::Error {
                code: ErrorCode::Unauthorized,
                message,
            } => return Some(Ended::Stop(StopReason::Unauthorized(message.clone()))),
            ServerMessage::Item { thread_id, item } => {
                if let Some(seq) = self.open.get_mut(thread_id).and_then(|t| t.seq.as_mut()) {
                    *seq = (*seq).max(item.seq);
                }
            }
            ServerMessage::TranscriptReset { thread_id } => {
                if let Some(t) = self.open.get_mut(thread_id) {
                    t.seq = None;
                }
            }
            ServerMessage::ThreadRemoved { thread_id } => {
                self.open.remove(thread_id);
            }
            _ => {}
        }
        self.emit(Event::Message(msg));
        None
    }
}

/// Send `pair` or `hello` and wait for `paired` or `welcome`.
async fn authenticate(ws: &mut Ws, auth: &Auth, options: &ClientOptions) -> Authed {
    let msg = match auth.clone() {
        Auth::Pair {
            code,
            device_id,
            device_name,
        } => ClientMessage::Pair {
            protocol: PROTOCOL_VERSION,
            code,
            device_id,
            device_name,
            app_version: options.app_version.clone(),
            platform: Some(std::env::consts::OS.to_string()),
        },
        Auth::Hello { device_id, token } => ClientMessage::Hello {
            protocol: PROTOCOL_VERSION,
            device_id,
            token,
            app_version: options.app_version.clone(),
            platform: Some(std::env::consts::OS.to_string()),
        },
    };
    let text = serde_json::to_string(&ClientEnvelope {
        id: Some("auth".into()),
        msg,
    })
    .expect("client messages always serialize");
    if let Err(err) = write(ws, Message::text(text)).await {
        return Authed::Retry(err);
    }
    let answer = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let Ok(envelope) = serde_json::from_str::<ServerEnvelope>(&text) else {
                        continue;
                    };
                    match envelope.msg {
                        msg @ (ServerMessage::Paired { .. } | ServerMessage::Welcome { .. }) => {
                            return Authed::Ok(msg);
                        }
                        ServerMessage::Error { code, message } => {
                            return match code {
                                ErrorCode::Unauthorized => {
                                    Authed::Stop(StopReason::Unauthorized(message))
                                }
                                ErrorCode::PairingFailed => {
                                    Authed::Stop(StopReason::PairingFailed(message))
                                }
                                ErrorCode::UnsupportedProtocol => {
                                    Authed::Stop(StopReason::ProtocolMismatch(message))
                                }
                                // Rate limited, a device list the machine couldn't save…: again later.
                                code => Authed::Retry(format!("{code}: {message}")),
                            };
                        }
                        _ => continue,
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Authed::Retry("closed before answering".into());
                }
                Some(Err(err)) => return Authed::Retry(err.to_string()),
                Some(Ok(_)) => continue,
            }
        }
    };
    tokio::time::timeout(options.connect_timeout, answer)
        .await
        .unwrap_or_else(|_| Authed::Retry("no answer to pair or hello".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_lose_their_scheme() {
        assert_eq!(
            bare_route(" wss://studio.local:7420/ "),
            "studio.local:7420"
        );
        assert_eq!(bare_route("HTTP://10.0.0.2:7420"), "10.0.0.2:7420");
        assert_eq!(bare_route("[fd00::1]:7420"), "[fd00::1]:7420");
    }

    #[test]
    fn a_fingerprint_must_be_a_sha256_and_a_route_given() {
        let auth = Auth::Hello {
            device_id: "a".into(),
            token: "t".into(),
        };
        let err = Client::connect(vec!["127.0.0.1:1".into()], "abcd", auth.clone()).unwrap_err();
        assert!(matches!(err, ClientError::Invalid(_)));
        let err = Client::connect(vec![" ".into()], &"a".repeat(64), auth).unwrap_err();
        assert!(matches!(err, ClientError::Invalid(_)));
    }

    #[test]
    fn secrets_stay_out_of_debug() {
        let auth = Auth::Hello {
            device_id: "a".into(),
            token: "s3cret".into(),
        };
        assert!(!format!("{auth:?}").contains("s3cret"));
        let paired = Paired {
            device_id: "a".into(),
            token: "s3cret".into(),
            fingerprint: "f".into(),
            host: HostInfo {
                id: "h".into(),
                name: "Studio".into(),
                version: "0".into(),
            },
            routes: vec![],
            features: vec![],
        };
        assert!(!format!("{paired:?}").contains("s3cret"));
    }
}
