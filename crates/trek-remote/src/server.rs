//! The WebSocket server: accepts phones, authenticates them, serves requests through a
//! [`RemoteHost`] and fans [`HostEvent`]s out to them.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{StatusCode, header};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use crate::host::{HostError, HostEvent, HostResult, RemoteHost};
use crate::pairing::{
    DeviceInfo, DeviceRegistry, MAX_DEVICE_FIELD, Pairing, PairingOffer, generate_code, now_ms,
    pairing_url,
};
use crate::protocol::{
    ClientEnvelope, ClientMessage, ErrorCode, HostInfo, Item, PROTOCOL_VERSION, ServerEnvelope, ServerMessage,
    Snapshot, Transcript,
};

/// How the server listens and behaves.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Where to listen (`0.0.0.0:7420` by default; a `100.x` address for "Tailscale only").
    pub bind: SocketAddr,
    /// The Mac, as phones see it.
    pub host: HostInfo,
    /// `host:port` phones should dial, for the pairing QR code (e.g. the LAN or tailnet address).
    /// `None` uses the bound address, which is useless when bound to `0.0.0.0`.
    pub advertise: Option<String>,
    /// Where paired devices are saved (`devices.json`); `None` keeps them in memory.
    pub devices_path: Option<PathBuf>,
    /// How long a new connection has to authenticate.
    pub auth_timeout: Duration,
    /// How long a pairing code stays valid.
    pub pairing_ttl: Duration,
    /// The largest message accepted from a phone.
    pub max_message: usize,
    /// How often the server pings phones (a phone silent for three intervals is dropped).
    pub ping_interval: Duration,
    /// Speak TLS with this certificate (`wss://`), pinned by phones through the pairing QR code.
    /// `None` is plain `ws://`, for tests and the demo host only.
    pub tls: Option<crate::tls::TlsIdentity>,
}

/// The default port.
pub const DEFAULT_PORT: u16 = 7420;

impl ServerConfig {
    /// The defaults from `docs/MOBILE.md`: `0.0.0.0:7420`, 10 s to authenticate, 10 min codes,
    /// 1 MiB messages, pings every 20 s, devices kept in memory.
    pub fn new(host: HostInfo) -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            host,
            advertise: None,
            devices_path: None,
            auth_timeout: Duration::from_secs(10),
            pairing_ttl: Duration::from_secs(10 * 60),
            max_message: 1 << 20,
            ping_interval: Duration::from_secs(20),
            tls: None,
        }
    }
}

/// Something the app may want to show (a toast, a device list refresh).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerNotice {
    /// A device paired (show "Tobias's iPhone paired" with an Undo that calls `revoke`).
    Paired { device_id: String, name: String },
    /// A paired device connected.
    Connected { device_id: String },
    /// A paired device disconnected.
    Disconnected { device_id: String },
}

/// Starts servers.
pub struct RemoteServer;

impl RemoteServer {
    /// Bind `config.bind` and serve phones until [`RemoteHandle::shutdown`]. Must be called within a
    /// tokio runtime.
    pub async fn start<H: RemoteHost>(config: ServerConfig, host: Arc<H>) -> io::Result<RemoteHandle> {
        let registry = match &config.devices_path {
            Some(path) => DeviceRegistry::load(path.clone())?,
            None => DeviceRegistry::in_memory(),
        };
        let tls = config.tls.as_ref().map(|t| t.acceptor()).transpose()?;
        let listener = TcpListener::bind(config.bind).await?;
        let local_addr = listener.local_addr()?;
        let (events, _) = broadcast::channel(1024);
        let (notices, _) = broadcast::channel(64);
        let (shutdown, _) = watch::channel(false);
        let inner = Arc::new(Inner {
            config,
            tls,
            local_addr,
            events,
            notices,
            shutdown,
            state: Mutex::new(State { registry, pairing: Pairing::default(), connections: HashMap::new(), next_conn: 0 }),
        });
        tracing::info!(%local_addr, "trek-remote listening");
        tokio::spawn(accept_loop(listener, inner.clone(), host));
        Ok(RemoteHandle { inner })
    }
}

/// Controls a running server. Cheap to clone.
#[derive(Clone)]
pub struct RemoteHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RemoteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHandle").field("local_addr", &self.inner.local_addr).finish()
    }
}

impl RemoteHandle {
    /// The address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Push a change to connected phones: snapshots and thread changes to every authenticated
    /// phone, items and transcript resets to phones subscribed to that thread.
    pub fn push(&self, event: HostEvent) {
        let _ = self.inner.events.send(event);
    }

    /// Show a new pairing code (replacing any active one), valid for `pairing_ttl`.
    pub fn pairing_offer(&self) -> PairingOffer {
        self.pairing_offer_with_code(&generate_code())
    }

    /// A pairing offer with a fixed code (for demos and tests). Panics if `code` isn't 8 Crockford
    /// base32 characters.
    #[doc(hidden)]
    pub fn pairing_offer_with_code(&self, code: &str) -> PairingOffer {
        let normalized = crate::pairing::normalize_code(code).expect("a valid pairing code");
        let code = format!("{}-{}", &normalized[..4], &normalized[4..]);
        let ttl = self.inner.config.pairing_ttl;
        self.inner.lock().pairing.activate(&code, ttl).expect("normalized codes are valid");
        let config = &self.inner.config;
        let advertise = config.advertise.clone().unwrap_or_else(|| self.inner.local_addr.to_string());
        PairingOffer {
            url: pairing_url(&advertise, &code, &config.host.name, &config.host.id, config.tls.as_ref().map(|t| t.fingerprint.as_str())),
            expires_at: now_ms() + ttl.as_millis() as i64,
            fingerprint: config.tls.as_ref().map(|t| t.short_fingerprint()),
            code,
        }
    }

    /// Withdraw the active pairing code.
    pub fn cancel_pairing(&self) {
        self.inner.lock().pairing.cancel();
    }

    /// Paired devices (without their token hashes).
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.inner.lock().registry.list()
    }

    /// Unpair a device and drop its live connection. Returns whether it was paired.
    pub fn revoke(&self, device_id: &str) -> bool {
        let mut state = self.inner.lock();
        let revoked = state.registry.revoke(device_id);
        if let Some(conn) = state.connections.remove(device_id) {
            let _ = conn.kick.send(Kick::Revoked);
        }
        if revoked {
            tracing::info!(device_id, "device revoked");
        }
        revoked
    }

    /// Ids of the devices connected right now.
    pub fn connected_devices(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.inner.lock().connections.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Pairings, connections and disconnections, for toasts.
    pub fn notices(&self) -> broadcast::Receiver<ServerNotice> {
        self.inner.notices.subscribe()
    }

    /// Stop listening and close every connection.
    pub fn shutdown(&self) {
        self.inner.shutdown.send_replace(true);
    }
}

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

struct Inner {
    config: ServerConfig,
    /// Made from `config.tls`.
    tls: Option<tokio_rustls::TlsAcceptor>,
    local_addr: SocketAddr,
    events: broadcast::Sender<HostEvent>,
    notices: broadcast::Sender<ServerNotice>,
    shutdown: watch::Sender<bool>,
    state: Mutex<State>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notice(&self, notice: ServerNotice) {
        let _ = self.notices.send(notice);
    }
}

struct State {
    registry: DeviceRegistry,
    pairing: Pairing,
    /// The live connection of each device.
    connections: HashMap<String, LiveConn>,
    next_conn: u64,
}

struct LiveConn {
    conn_id: u64,
    kick: oneshot::Sender<Kick>,
}

/// Why the server closes an authenticated connection.
#[derive(Debug, Clone, Copy)]
enum Kick {
    /// The same device connected again.
    Replaced,
    /// The device was revoked.
    Revoked,
}

// ---------------------------------------------------------------------------------------------
// Accepting
// ---------------------------------------------------------------------------------------------

async fn accept_loop<H: RemoteHost>(listener: TcpListener, inner: Arc<Inner>, host: Arc<H>) {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(serve(stream, peer, inner.clone(), host.clone()));
                }
                Err(err) => {
                    tracing::warn!(%err, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = stopped(&mut shutdown) => break,
        }
    }
    tracing::info!("trek-remote stopped listening");
}

/// A phone's connection under the WebSocket: TLS, or plain TCP for tests and demos.
trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

type Ws = WebSocketStream<Box<dyn Io>>;

/// Resolves once the server is shutting down (or its handle state is gone).
async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stop| *stop).await;
}

/// Refuse browsers: any upgrade carrying an `Origin` header gets a 403.
#[allow(clippy::result_large_err)] // the signature tungstenite asks for
fn refuse_browsers(req: &Request, resp: Response) -> Result<Response, ErrorResponse> {
    if req.headers().contains_key(header::ORIGIN) {
        let mut refusal = ErrorResponse::new(Some("Browsers can't connect to Trek".into()));
        *refusal.status_mut() = StatusCode::FORBIDDEN;
        return Err(refusal);
    }
    Ok(resp)
}

async fn serve<H: RemoteHost>(stream: TcpStream, peer: SocketAddr, inner: Arc<Inner>, host: Arc<H>) {
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(inner.config.max_message))
        .max_frame_size(Some(inner.config.max_message));
    // The handshake and the first message share one deadline.
    let deadline = tokio::time::Instant::now() + inner.config.auth_timeout;
    let stream: Box<dyn Io> = match &inner.tls {
        Some(acceptor) => match tokio::time::timeout_at(deadline, acceptor.accept(stream)).await {
            Ok(Ok(tls)) => Box::new(tls),
            Ok(Err(err)) => {
                tracing::debug!(%peer, %err, "TLS handshake failed");
                return;
            }
            Err(_) => {
                tracing::debug!(%peer, "TLS handshake timed out");
                return;
            }
        },
        None => Box::new(stream),
    };
    let handshake = tokio_tungstenite::accept_hdr_async_with_config(stream, refuse_browsers, Some(ws_config));
    let mut ws = match tokio::time::timeout_at(deadline, handshake).await {
        Ok(Ok(ws)) => ws,
        Ok(Err(err)) => {
            tracing::debug!(%peer, %err, "websocket handshake refused or failed");
            return;
        }
        Err(_) => {
            tracing::debug!(%peer, "websocket handshake timed out");
            return;
        }
    };
    let Some(auth) = authenticate(&mut ws, peer, &inner, deadline).await else { return };
    let device_id = auth.device_id.clone();
    let conn_id = auth.conn_id;
    tracing::info!(%peer, device_id, "phone connected");
    inner.notice(ServerNotice::Connected { device_id: device_id.clone() });

    run_session(&mut ws, &inner, host, auth).await;

    {
        let mut state = inner.lock();
        if state.connections.get(&device_id).is_some_and(|c| c.conn_id == conn_id) {
            state.connections.remove(&device_id);
        }
        state.registry.touch(&device_id, now_ms());
    }
    tracing::info!(%peer, device_id, "phone disconnected");
    inner.notice(ServerNotice::Disconnected { device_id });
}

// ---------------------------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------------------------

struct Authenticated {
    device_id: String,
    conn_id: u64,
    kick: oneshot::Receiver<Kick>,
}

/// Wait for `pair` or `hello`; anything else (or nothing before `deadline`) closes the socket.
async fn authenticate(
    ws: &mut Ws,
    peer: SocketAddr,
    inner: &Inner,
    deadline: tokio::time::Instant,
) -> Option<Authenticated> {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        let frame = tokio::select! {
            frame = ws.next() => frame,
            _ = tokio::time::sleep_until(deadline) => {
                tracing::debug!(%peer, "no pair/hello in time");
                close(ws, CloseCode::Policy, "Authentication timed out").await;
                return None;
            }
            _ = stopped(&mut shutdown) => {
                close(ws, CloseCode::Away, "Trek is shutting down").await;
                return None;
            }
        };
        let text = match frame {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return None,
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Binary(_))) => {
                reject(ws, None, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        let (re, parsed) = parse_client(&text);
        let msg = match parsed {
            Ok(msg) => msg,
            Err(bad) if bad.is_auth() => {
                reject(ws, re, ErrorCode::BadRequest, &bad.message).await;
                return None;
            }
            Err(_) => {
                reject(ws, re, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        let (protocol, device_id) = match &msg {
            ClientMessage::Pair { protocol, device_id, .. } | ClientMessage::Hello { protocol, device_id, .. } => {
                (*protocol, device_id.clone())
            }
            _ => {
                tracing::debug!(%peer, kind = msg.kind(), "unauthenticated request refused");
                reject(ws, re, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        if protocol != PROTOCOL_VERSION {
            let message = format!("This Mac speaks protocol {PROTOCOL_VERSION}, the app speaks {protocol}");
            reject(ws, re, ErrorCode::UnsupportedProtocol, &message).await;
            return None;
        }
        if !valid_device_id(&device_id) {
            reject(ws, re, ErrorCode::BadRequest, "Invalid device_id").await;
            return None;
        }

        let (kick_tx, kick) = oneshot::channel();
        let host = inner.config.host.clone();
        return match msg {
            ClientMessage::Pair { code, device_name, .. } => {
                let name = clean_name(&device_name);
                let outcome = {
                    let mut state = inner.lock();
                    match state.pairing.redeem(&code) {
                        Ok(()) => {
                            let token = state.registry.register(&device_id, &name, now_ms());
                            Ok((token, register_conn(&mut state, &device_id, kick_tx)))
                        }
                        Err(err) => Err(err),
                    }
                };
                match outcome {
                    Ok((token, conn_id)) => {
                        tracing::info!(%peer, device_id, name, "device paired");
                        inner.notice(ServerNotice::Paired { device_id: device_id.clone(), name });
                        let paired = ServerMessage::Paired { protocol: PROTOCOL_VERSION, token, host };
                        let _ = send(ws, ServerEnvelope::reply(re, paired)).await;
                        Some(Authenticated { device_id, conn_id, kick })
                    }
                    Err(err) => {
                        tracing::warn!(%peer, %err, "pairing refused");
                        reject(ws, re, ErrorCode::PairingFailed, &err.to_string()).await;
                        None
                    }
                }
            }
            ClientMessage::Hello { token, .. } => {
                let conn_id = {
                    let mut state = inner.lock();
                    if state.registry.verify(&device_id, &token) {
                        state.registry.touch(&device_id, now_ms());
                        Some(register_conn(&mut state, &device_id, kick_tx))
                    } else {
                        None
                    }
                };
                match conn_id {
                    Some(conn_id) => {
                        let welcome = ServerMessage::Welcome { protocol: PROTOCOL_VERSION, host };
                        let _ = send(ws, ServerEnvelope::reply(re, welcome)).await;
                        Some(Authenticated { device_id, conn_id, kick })
                    }
                    None => {
                        tracing::warn!(%peer, device_id, "hello with an unknown device or a wrong token");
                        reject(ws, re, ErrorCode::Unauthorized, "Unknown device or revoked token; pair again").await;
                        None
                    }
                }
            }
            _ => unreachable!("only pair and hello get here"),
        };
    }
}

/// Make this connection the device's live one, closing any older one.
fn register_conn(state: &mut State, device_id: &str, kick: oneshot::Sender<Kick>) -> u64 {
    state.next_conn += 1;
    let conn_id = state.next_conn;
    if let Some(old) = state.connections.insert(device_id.to_string(), LiveConn { conn_id, kick }) {
        let _ = old.kick.send(Kick::Replaced);
    }
    conn_id
}

fn valid_device_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_DEVICE_FIELD && !id.chars().any(char::is_control)
}

fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(MAX_DEVICE_FIELD).collect();
    let name = name.trim();
    if name.is_empty() { "iPhone".to_string() } else { name.to_string() }
}

// ---------------------------------------------------------------------------------------------
// Parsing and sending
// ---------------------------------------------------------------------------------------------

struct BadMessage {
    /// The message's `type`, if it had one.
    kind: Option<String>,
    message: String,
}

impl BadMessage {
    fn is_auth(&self) -> bool {
        matches!(self.kind.as_deref(), Some("pair" | "hello"))
    }
}

const CLIENT_TYPES: &[&str] =
    &["pair", "hello", "subscribe", "unsubscribe", "send", "new_thread", "answer", "interrupt", "mark_seen", "ping"];

/// Parse a text frame: its `id` (for the reply's `re`, even when the rest is bad) and message.
fn parse_client(text: &str) -> (Option<String>, Result<ClientMessage, BadMessage>) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => return (None, Err(BadMessage { kind: None, message: format!("Malformed JSON: {err}") })),
    };
    let Some(object) = value.as_object() else {
        return (None, Err(BadMessage { kind: None, message: "Expected a JSON object".into() }));
    };
    let re = object.get("id").and_then(|id| id.as_str()).map(str::to_string);
    let Some(kind) = object.get("type").and_then(|t| t.as_str()).map(str::to_string) else {
        return (re, Err(BadMessage { kind: None, message: "Missing \"type\"".into() }));
    };
    if !CLIENT_TYPES.contains(&kind.as_str()) {
        let message = format!("Unknown message type \"{kind}\"");
        return (re, Err(BadMessage { kind: Some(kind), message }));
    }
    match serde_json::from_value::<ClientEnvelope>(value) {
        Ok(envelope) => (re, Ok(envelope.msg)),
        Err(err) => {
            let message = format!("Bad \"{kind}\": {err}");
            (re, Err(BadMessage { kind: Some(kind), message }))
        }
    }
}

fn error(re: Option<String>, code: ErrorCode, message: impl Into<String>) -> ServerEnvelope {
    ServerEnvelope::reply(re, ServerMessage::Error { code, message: message.into() })
}

async fn send(ws: &mut Ws, envelope: ServerEnvelope) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    ws.send(Message::text(envelope.to_json())).await
}

/// Send an error, then close the socket.
async fn reject(ws: &mut Ws, re: Option<String>, code: ErrorCode, message: &str) {
    let _ = send(ws, error(re, code, message)).await;
    close(ws, CloseCode::Policy, code.as_str()).await;
}

/// Close the socket, giving the phone a moment to answer the close frame.
async fn close(ws: &mut Ws, code: CloseCode, reason: &str) {
    let frame = CloseFrame { code, reason: reason.to_string().into() };
    let _ = ws.close(Some(frame)).await;
    let drain = async { while let Some(Ok(_)) = ws.next().await {} };
    let _ = tokio::time::timeout(Duration::from_secs(2), drain).await;
}

// ---------------------------------------------------------------------------------------------
// Authenticated sessions
// ---------------------------------------------------------------------------------------------

/// Work for a connection's host worker. Host calls run one at a time, in the order the phone sent
/// them, off the connection's loop (so a slow host never stalls pings or pushes).
enum Job {
    Snapshot,
    Transcript { re: Option<String>, thread_id: String, after_seq: Option<u64>, generation: u64 },
    Call { re: Option<String>, msg: ClientMessage },
}

/// A finished [`Job`].
#[allow(clippy::large_enum_variant)] // short-lived, one per request
enum Done {
    Snapshot(HostResult<Snapshot>),
    Transcript {
        re: Option<String>,
        thread_id: String,
        after_seq: Option<u64>,
        generation: u64,
        result: HostResult<Transcript>,
    },
    Reply(ServerEnvelope),
}

async fn worker<H: RemoteHost>(host: Arc<H>, mut jobs: mpsc::UnboundedReceiver<Job>, done: mpsc::UnboundedSender<Done>) {
    while let Some(job) = jobs.recv().await {
        let finished = match job {
            // Reads are only worth doing for a live connection; actions the phone sent still run.
            Job::Snapshot | Job::Transcript { .. } if done.is_closed() => continue,
            Job::Snapshot => Done::Snapshot(host.snapshot().await),
            Job::Transcript { re, thread_id, after_seq, generation } => {
                let result = host.transcript(&thread_id).await;
                Done::Transcript { re, thread_id, after_seq, generation, result }
            }
            Job::Call { re, msg } => Done::Reply(call(&*host, re, msg).await),
        };
        let _ = done.send(finished);
    }
}

async fn call<H: RemoteHost>(host: &H, re: Option<String>, msg: ClientMessage) -> ServerEnvelope {
    let kind = msg.kind();
    let result: HostResult<Option<String>> = match msg {
        ClientMessage::Send(req) => host.send(req).await.map(|()| None),
        ClientMessage::NewThread(req) => host.new_thread(req).await.map(Some),
        ClientMessage::Answer(req) => host.answer(req).await.map(|()| None),
        ClientMessage::Interrupt { thread_id } => host.interrupt(&thread_id).await.map(|()| None),
        ClientMessage::MarkSeen { thread_id } => host.mark_seen(&thread_id).await.map(|()| None),
        other => Err(HostError::bad_request(format!("Unexpected \"{}\"", other.kind()))),
    };
    match result {
        Ok(thread_id) => ServerEnvelope::reply(re, ServerMessage::Ack { thread_id }),
        Err(err) => {
            tracing::debug!(kind, %err, "remote request failed");
            error(re, err.code, err.message)
        }
    }
}

/// A transcript subscription.
enum Sub {
    /// Waiting for the host's transcript; items pushed meanwhile are held until it's sent.
    Pending { generation: u64, held: Vec<Item>, reset: bool },
    /// Items are forwarded as they come.
    Active,
}

/// What one authenticated connection knows about its phone.
#[derive(Default)]
struct Session {
    subs: HashMap<String, Sub>,
    next_generation: u64,
    /// Snapshots requested but not sent yet; thread events wait behind them.
    snapshots_pending: u32,
    held_events: Vec<HostEvent>,
}

impl Session {
    fn on_request(
        &mut self,
        re: Option<String>,
        msg: ClientMessage,
        jobs: &mpsc::UnboundedSender<Job>,
    ) -> Vec<ServerEnvelope> {
        match msg {
            ClientMessage::Pair { .. } | ClientMessage::Hello { .. } => {
                vec![error(re, ErrorCode::BadRequest, "Already authenticated")]
            }
            ClientMessage::Ping => vec![ServerEnvelope::reply(re, ServerMessage::Pong)],
            ClientMessage::Subscribe { thread_id, after_seq } => {
                self.next_generation += 1;
                let generation = self.next_generation;
                self.subs.insert(thread_id.clone(), Sub::Pending { generation, held: Vec::new(), reset: false });
                let _ = jobs.send(Job::Transcript { re, thread_id, after_seq, generation });
                Vec::new()
            }
            ClientMessage::Unsubscribe { thread_id } => {
                self.subs.remove(&thread_id);
                match re {
                    Some(re) => vec![ServerEnvelope::reply(Some(re), ServerMessage::Ack { thread_id: None })],
                    None => Vec::new(),
                }
            }
            msg @ (ClientMessage::Send(_)
            | ClientMessage::NewThread(_)
            | ClientMessage::Answer(_)
            | ClientMessage::Interrupt { .. }
            | ClientMessage::MarkSeen { .. }) => {
                let _ = jobs.send(Job::Call { re, msg });
                Vec::new()
            }
        }
    }

    fn request_snapshot(&mut self, jobs: &mpsc::UnboundedSender<Job>) {
        self.snapshots_pending += 1;
        let _ = jobs.send(Job::Snapshot);
    }

    fn on_done(&mut self, done: Done) -> Vec<ServerEnvelope> {
        match done {
            Done::Reply(envelope) => vec![envelope],
            Done::Snapshot(result) => {
                self.snapshots_pending = self.snapshots_pending.saturating_sub(1);
                let mut out = vec![match result {
                    Ok(snapshot) => ServerEnvelope::push(ServerMessage::Snapshot(snapshot)),
                    Err(err) => error(None, err.code, err.message),
                }];
                if self.snapshots_pending == 0 {
                    for event in std::mem::take(&mut self.held_events) {
                        out.extend(self.on_event(event));
                    }
                }
                out
            }
            Done::Transcript { re, thread_id, after_seq, generation, result } => {
                let current =
                    matches!(self.subs.get(&thread_id), Some(Sub::Pending { generation: g, .. }) if *g == generation);
                let transcript = match result {
                    Ok(transcript) => transcript,
                    Err(err) => {
                        if current {
                            self.subs.remove(&thread_id);
                        }
                        return vec![error(re, err.code, err.message)];
                    }
                };
                let seq = transcript.seq;
                let reply = transcript_message(thread_id.clone(), after_seq, transcript);
                let mut out = vec![ServerEnvelope::reply(re, reply)];
                if current
                    && let Some(Sub::Pending { held, reset, .. }) = self.subs.insert(thread_id.clone(), Sub::Active)
                {
                    for item in held.into_iter().filter(|item| item.seq > seq) {
                        out.push(ServerEnvelope::push(ServerMessage::Item { thread_id: thread_id.clone(), item }));
                    }
                    if reset {
                        out.push(ServerEnvelope::push(ServerMessage::TranscriptReset { thread_id }));
                    }
                }
                out
            }
        }
    }

    fn on_event(&mut self, event: HostEvent) -> Vec<ServerEnvelope> {
        let msg = match event {
            HostEvent::Snapshot(_) | HostEvent::Thread(_) | HostEvent::ThreadRemoved(_)
                if self.snapshots_pending > 0 =>
            {
                self.held_events.push(event);
                return Vec::new();
            }
            HostEvent::Snapshot(snapshot) => ServerMessage::Snapshot(snapshot),
            HostEvent::Thread(thread) => ServerMessage::Thread { thread },
            HostEvent::ThreadRemoved(thread_id) => ServerMessage::ThreadRemoved { thread_id },
            HostEvent::Item { thread_id, item } => match self.subs.get_mut(&thread_id) {
                Some(Sub::Active) => ServerMessage::Item { thread_id, item },
                Some(Sub::Pending { held, .. }) => {
                    held.push(item);
                    return Vec::new();
                }
                None => return Vec::new(),
            },
            HostEvent::TranscriptReset(thread_id) => match self.subs.get_mut(&thread_id) {
                Some(Sub::Active) => ServerMessage::TranscriptReset { thread_id },
                Some(Sub::Pending { reset, .. }) => {
                    *reset = true;
                    return Vec::new();
                }
                None => return Vec::new(),
            },
        };
        vec![ServerEnvelope::push(msg)]
    }

    /// The phone missed events: send everything it relies on again.
    fn resync(&mut self, jobs: &mpsc::UnboundedSender<Job>) -> Vec<ServerEnvelope> {
        self.request_snapshot(jobs);
        let mut out = Vec::new();
        for (thread_id, sub) in &mut self.subs {
            match sub {
                Sub::Active => {
                    out.push(ServerEnvelope::push(ServerMessage::TranscriptReset { thread_id: thread_id.clone() }))
                }
                Sub::Pending { reset, .. } => *reset = true,
            }
        }
        out
    }
}

/// The reply to `subscribe`: only what's after `after_seq` when the host can serve it, else all.
fn transcript_message(thread_id: String, after_seq: Option<u64>, transcript: Transcript) -> ServerMessage {
    let Transcript { seq, items } = transcript;
    match after_seq {
        Some(after) if after <= seq => ServerMessage::Transcript {
            thread_id,
            reset: false,
            seq,
            items: items.into_iter().filter(|item| item.seq > after).collect(),
        },
        _ => ServerMessage::Transcript { thread_id, reset: true, seq, items },
    }
}

async fn run_session<H: RemoteHost>(ws: &mut Ws, inner: &Inner, host: Arc<H>, auth: Authenticated) {
    let Authenticated { device_id, mut kick, .. } = auth;
    // Listen for events before asking for the snapshot, so nothing falls between the two.
    let mut events = inner.events.subscribe();
    let mut shutdown = inner.shutdown.subscribe();
    let (jobs, jobs_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done) = mpsc::unbounded_channel();
    tokio::spawn(worker(host, jobs_rx, done_tx));

    let mut session = Session::default();
    session.request_snapshot(&jobs);

    let ping_every = inner.config.ping_interval;
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + ping_every, ping_every);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = Instant::now();

    loop {
        let out = tokio::select! {
            frame = ws.next() => {
                last_heard = Instant::now();
                match frame {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    Some(Ok(Message::Text(text))) => match parse_client(&text) {
                        (re, Ok(msg)) => {
                            tracing::trace!(device_id, kind = msg.kind(), "request");
                            session.on_request(re, msg, &jobs)
                        }
                        (re, Err(bad)) => vec![error(re, ErrorCode::BadRequest, bad.message)],
                    },
                    Some(Ok(Message::Binary(_))) => vec![error(None, ErrorCode::BadRequest, "Text frames only")],
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Vec::new(),
                }
            }
            Some(finished) = done.recv() => session.on_done(finished),
            event = events.recv() => match event {
                Ok(event) => session.on_event(event),
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(device_id, missed, "phone fell behind; resyncing");
                    session.resync(&jobs)
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = ping.tick() => {
                if last_heard.elapsed() > ping_every * 3 {
                    tracing::info!(device_id, "phone stopped answering pings");
                    close(ws, CloseCode::Away, "No response").await;
                    return;
                }
                if ws.send(Message::Ping(Default::default())).await.is_err() {
                    return;
                }
                Vec::new()
            }
            kicked = &mut kick => {
                match kicked {
                    Ok(Kick::Replaced) => close(ws, CloseCode::Normal, "Replaced by a newer connection").await,
                    Ok(Kick::Revoked) | Err(_) => {
                        let _ = send(ws, error(None, ErrorCode::Unauthorized, "This device was revoked")).await;
                        close(ws, CloseCode::Policy, "Revoked").await;
                    }
                }
                return;
            }
            _ = stopped(&mut shutdown) => {
                close(ws, CloseCode::Away, "Trek is shutting down").await;
                return;
            }
        };
        for envelope in out {
            if send(ws, envelope).await.is_err() {
                return;
            }
        }
    }
}

