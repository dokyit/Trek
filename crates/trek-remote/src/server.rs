//! The WebSocket server: accepts phones, authenticates them, serves requests through a
//! [`RemoteHost`] and fans [`HostEvent`]s out to them.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{StatusCode, header};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};

use crate::host::{CloneProgress, HostError, HostEvent, HostResult, RemoteHost};
use crate::pairing::{
    DeviceInfo, DeviceRegistry, MAX_DEVICE_FIELD, Pairing, PairingOffer, generate_code, now_ms,
    pairing_url_with_routes, source,
};
use crate::protocol::{
    ClientEnvelope, ClientMessage, ErrorCode, HostInfo, Item, MAX_PAGE, PROTOCOL_VERSION,
    ServerEnvelope, ServerMessage, Snapshot, ThreadAction, ThreadActionRequest, Transcript,
};

/// How the server listens and behaves.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Where to listen (`0.0.0.0:7420` by default; Trek listens on `[::]:7420`, every interface
    /// and IPv4 too, and decides per connection with `admit`).
    pub bind: SocketAddr,
    /// The Mac, as phones see it.
    pub host: HostInfo,
    /// `host:port` phones should dial, for the pairing QR code (e.g. the LAN or tailnet address).
    /// `None` uses the bound address, which is useless when bound to `0.0.0.0`.
    pub advertise: Option<String>,
    /// More addresses to listen on besides `bind` (the tailnet's, beside the Wi-Fi's). One that
    /// can't be bound is left out, with a warning: `bind` alone decides whether the server starts.
    pub also_bind: Vec<SocketAddr>,
    /// Every `host:port` phones may dial, `advertise` first: in the pairing code (`alt`) and in
    /// `welcome`/`paired`, so a phone tries the next when one doesn't answer. Empty: `advertise` only.
    /// [`RemoteHandle::set_routes`] changes both while the server runs.
    pub routes: Vec<String>,
    /// Which connections to serve, by where they come from and which of this machine's addresses
    /// they reached: one refused is closed at once, before TLS. `None` serves every one.
    pub admit: Option<Admission>,
    /// Where paired devices are saved (`devices.json`); `None` keeps them in memory.
    pub devices_path: Option<PathBuf>,
    /// How long a new connection has to authenticate.
    pub auth_timeout: Duration,
    /// How long a pairing code stays valid.
    pub pairing_ttl: Duration,
    /// The largest message accepted from an authenticated phone (before that,
    /// [`MAX_PRE_AUTH`]).
    pub max_message: usize,
    /// How often the server pings phones (a phone silent for three intervals is dropped).
    pub ping_interval: Duration,
    /// Speak TLS with this certificate (`wss://`), pinned by phones through the pairing QR code.
    /// `None` is plain `ws://`, for tests and the demo host only.
    pub tls: Option<crate::tls::TlsIdentity>,
    /// The optional requests the host serves, for `welcome` and `paired` (see
    /// [`crate::protocol::feature`]). None by default.
    pub features: Vec<String>,
}

/// The default port.
pub const DEFAULT_PORT: u16 = 7420;

/// Whether to serve a connection, from the address it comes from (`peer`) and the one of this
/// machine's it reached (`local`), both with IPv4-mapped IPv6 turned back into IPv4. Asked once per
/// connection, as it's accepted, on the accepting task: keep it quick.
#[derive(Clone)]
pub struct Admission {
    admits: Arc<dyn Fn(SocketAddr, SocketAddr) -> bool + Send + Sync>,
    describe: Option<Arc<dyn Fn(SocketAddr, SocketAddr) -> String + Send + Sync>>,
}

impl Admission {
    pub fn new(admits: impl Fn(SocketAddr, SocketAddr) -> bool + Send + Sync + 'static) -> Self {
        Self {
            admits: Arc::new(admits),
            describe: None,
        }
    }

    /// How the log names the way a connection from `peer` to `local` came (`Wi-Fi on en0`,
    /// `Tailscale on utun4`, `link-local on en7`), in place of what the server can tell from the
    /// addresses alone ([`network_of`]). Asked once per connection, as it's accepted: keep it
    /// quick too.
    pub fn describing(
        mut self,
        describe: impl Fn(SocketAddr, SocketAddr) -> String + Send + Sync + 'static,
    ) -> Self {
        self.describe = Some(Arc::new(describe));
        self
    }

    /// Whether a connection from `peer` to `local` is served.
    pub fn admits(&self, peer: SocketAddr, local: SocketAddr) -> bool {
        (self.admits)(canonical(peer), canonical(local))
    }

    /// How the connection came, for the log.
    pub fn describe(&self, peer: SocketAddr, local: SocketAddr) -> Option<String> {
        self.describe
            .as_ref()
            .map(|d| d(canonical(peer), canonical(local)))
    }
}

/// Which kind of network `peer` is on, as far as its address says: the log's account of a
/// connection when Trek gives none ([`Admission::describing`]).
pub fn network_of(peer: std::net::IpAddr) -> &'static str {
    use std::net::IpAddr;
    match peer.to_canonical() {
        ip if ip.is_loopback() => "this machine",
        IpAddr::V4(v4) if v4.is_link_local() => "link-local",
        IpAddr::V4(v4) if v4.is_private() => "local network",
        IpAddr::V4(v4) if v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]) => {
            "tailnet or carrier-grade NAT"
        }
        IpAddr::V6(v6) if v6.segments()[0] & 0xffc0 == 0xfe80 => "link-local",
        IpAddr::V6(v6) if v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0] => "tailnet",
        IpAddr::V6(v6) if v6.segments()[0] & 0xfe00 == 0xfc00 => "local network",
        _ => "elsewhere",
    }
}

impl std::fmt::Debug for Admission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Admission(..)")
    }
}

/// `addr` with an IPv4-mapped IPv6 address (`[::ffff:192.168.1.2]`, what a dual-stack socket
/// sees of IPv4) written as the IPv4 it is. An IPv6 address keeps its scope (the interface of a
/// link-local one: `fe80::1%en7`, a phone on a USB cable).
fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(v4.into(), v6.port()),
            None => addr,
        },
        SocketAddr::V4(_) => addr,
    }
}

/// The last device that authenticated: who, when, from where, and on which of this machine's
/// addresses (the route it dialled, as far as the machine can tell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastConnection {
    pub device_id: String,
    /// Unix ms.
    pub at: i64,
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

/// Connections that haven't authenticated yet, at most.
pub const PRE_AUTH_TOTAL: usize = 16;
/// ...and from one address (an IPv6 one by its /64), so one device on the network can't take
/// every place.
pub const PRE_AUTH_PER_ADDRESS: usize = 4;
/// What a connection may send after the upgrade before it has authenticated (`pair` and `hello`
/// take a few hundred bytes); more closes it.
pub const MAX_PRE_AUTH: usize = 4 << 10;
/// The most of an upgrade request read (with anything the phone sends right behind it).
const MAX_UPGRADE: usize = 8 << 10;

impl ServerConfig {
    /// The defaults from `docs/MOBILE.md`: `0.0.0.0:7420`, 10 s to authenticate, 10 min codes,
    /// 8 MiB messages once authenticated, pings every 20 s, devices kept in memory.
    pub fn new(host: HostInfo) -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            host,
            advertise: None,
            also_bind: Vec::new(),
            routes: Vec::new(),
            admit: None,
            devices_path: None,
            auth_timeout: Duration::from_secs(10),
            pairing_ttl: Duration::from_secs(10 * 60),
            // Photos come base64 in `send`: a few of a megabyte each.
            max_message: 8 << 20,
            ping_interval: Duration::from_secs(20),
            tls: None,
            features: Vec::new(),
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

/// `addr`, listening. An IPv6 address takes IPv4 too (`[::]` serves `127.0.0.1`): macOS and
/// Linux do that by default, Windows only when asked, so it's asked for everywhere. The rest is
/// what `TcpListener::bind` does (its backlog, and its SO_REUSEADDR where that only lets a restart
/// rebind at once rather than letting another program take the port).
fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    if addr.is_ipv6() {
        socket2::SockRef::from(&socket).set_only_v6(false)?;
    }
    #[cfg(not(windows))]
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}

/// Starts servers.
pub struct RemoteServer;

impl RemoteServer {
    /// Bind `config.bind` and serve phones until [`RemoteHandle::shutdown`]. Must be called within a
    /// tokio runtime.
    pub async fn start<H: RemoteHost>(
        config: ServerConfig,
        host: Arc<H>,
    ) -> io::Result<RemoteHandle> {
        let registry = match &config.devices_path {
            Some(path) => DeviceRegistry::load(path.clone())?,
            None => DeviceRegistry::in_memory(),
        };
        let tls = config.tls.as_ref().map(|t| t.acceptor()).transpose()?;
        let listener = bind(config.bind)?;
        let local_addr = listener.local_addr()?;
        let mut more = Vec::new();
        for &addr in &config.also_bind {
            match bind(addr).and_then(|l| Ok((l.local_addr()?, l))) {
                Ok(bound) => more.push(bound),
                Err(err) => tracing::warn!(%addr, %err, "trek-remote couldn't listen there too"),
            }
        }
        let also = more.iter().map(|(addr, _)| *addr).collect();
        let (events, _) = broadcast::channel(1024);
        let (notices, _) = broadcast::channel(64);
        let (shutdown, _) = watch::channel(false);
        let unwatch = {
            let host = host.clone();
            Box::new(move |thread_id: &str| host.unwatch(thread_id))
        };
        let inner = Arc::new(Inner {
            tls,
            local_addr,
            also,
            events,
            notices,
            shutdown,
            pre_auth: PreAuthSlots::new(PRE_AUTH_TOTAL, PRE_AUTH_PER_ADDRESS),
            watchers: Watchers {
                counts: Mutex::new(HashMap::new()),
                unwatch,
            },
            state: Mutex::new(State {
                registry,
                pairing: Pairing::default(),
                connections: HashMap::new(),
                next_conn: 0,
                advertise: config.advertise.clone(),
                routes: config.routes.clone(),
                last: None,
            }),
            config,
        });
        tracing::info!(%local_addr, "trek-remote listening");
        for (addr, l) in more {
            tracing::info!(local_addr = %addr, "trek-remote listening there too");
            tokio::spawn(accept_loop(l, inner.clone(), host.clone()));
        }
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
        f.debug_struct("RemoteHandle")
            .field("local_addr", &self.inner.local_addr)
            .finish()
    }
}

impl RemoteHandle {
    /// The address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Every address it listens on: `local_addr`, then those of `also_bind` it could bind.
    pub fn listening(&self) -> Vec<SocketAddr> {
        std::iter::once(self.inner.local_addr)
            .chain(self.inner.also.iter().copied())
            .collect()
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
        let (advertise, routes) = {
            let mut state = self.inner.lock();
            state
                .pairing
                .activate(&code, ttl)
                .expect("normalized codes are valid");
            (state.advertise.clone(), state.routes.clone())
        };
        let config = &self.inner.config;
        let advertise = advertise.unwrap_or_else(|| self.inner.local_addr.to_string());
        let routes = if routes.is_empty() {
            vec![advertise]
        } else {
            routes
        };
        PairingOffer {
            url: pairing_url_with_routes(
                &routes,
                &code,
                &config.host.name,
                &config.host.id,
                config.tls.as_ref().map(|t| t.fingerprint.as_str()),
            ),
            expires_at: now_ms() + ttl.as_millis() as i64,
            fingerprint: config.tls.as_ref().map(|t| t.short_fingerprint()),
            code,
        }
    }

    /// Change what phones are told to dial (`advertise` first, then the rest of `routes`): the
    /// pairing codes offered from now on and every `welcome` and `paired` that follows carry it.
    /// What's listening and who's connected stay as they are.
    pub fn set_routes(&self, advertise: Option<String>, routes: Vec<String>) {
        let mut state = self.inner.lock();
        state.advertise = advertise;
        state.routes = routes;
    }

    /// What phones are told to dial, as `welcome` gives it now.
    pub fn routes(&self) -> Vec<String> {
        self.inner.lock().routes.clone()
    }

    /// The last device that authenticated, since the server started.
    pub fn last_connection(&self) -> Option<LastConnection> {
        self.inner.lock().last.clone()
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
    /// The `also_bind` addresses it listens on too.
    also: Vec<SocketAddr>,
    events: broadcast::Sender<HostEvent>,
    notices: broadcast::Sender<ServerNotice>,
    shutdown: watch::Sender<bool>,
    pre_auth: PreAuthSlots,
    watchers: Watchers,
    state: Mutex<State>,
}

/// How many sessions have each thread open, across every phone: the host hears when a thread's
/// last one lets go ([`RemoteHost::unwatch`]).
struct Watchers {
    counts: Mutex<HashMap<String, usize>>,
    unwatch: Box<dyn Fn(&str) + Send + Sync>,
}

impl Watchers {
    fn counts(&self) -> MutexGuard<'_, HashMap<String, usize>> {
        self.counts.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn add(&self, thread_id: &str) {
        *self.counts().entry(thread_id.to_string()).or_default() += 1;
    }

    fn remove(&self, thread_id: &str) {
        let mut counts = self.counts();
        let Some(n) = counts.get_mut(thread_id) else {
            return;
        };
        *n -= 1;
        if *n == 0 {
            counts.remove(thread_id);
            // Under the lock: a subscribe that follows is told after this.
            (self.unwatch)(thread_id);
        }
    }

    /// The host read `thread_id` for a subscription dropped meanwhile: if nobody has it open,
    /// say so again.
    fn recheck(&self, thread_id: &str) {
        let counts = self.counts();
        if !counts.contains_key(thread_id) {
            (self.unwatch)(thread_id);
        }
    }
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
    /// What phones are told to dial (`config.advertise` and `config.routes` until
    /// [`RemoteHandle::set_routes`]).
    advertise: Option<String>,
    routes: Vec<String>,
    last: Option<LastConnection>,
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

type Counts = Arc<Mutex<HashMap<IpAddr, usize>>>;

fn counts(counts: &Counts) -> MutexGuard<'_, HashMap<IpAddr, usize>> {
    counts.lock().unwrap_or_else(|e| e.into_inner())
}

/// Places for connections that haven't authenticated yet: `total` in all, `per_address` for one
/// address.
struct PreAuthSlots {
    total: Arc<Semaphore>,
    per_address: usize,
    by_address: Counts,
}

/// A connection's place among the unauthenticated, given back when dropped.
struct PreAuth {
    _permit: OwnedSemaphorePermit,
    address: IpAddr,
    by_address: Counts,
}

impl PreAuthSlots {
    fn new(total: usize, per_address: usize) -> Self {
        Self {
            total: Arc::new(Semaphore::new(total)),
            per_address,
            by_address: Counts::default(),
        }
    }

    /// A place for a connection from `ip`, unless its address or everyone has used them up.
    fn take(&self, ip: IpAddr) -> Option<PreAuth> {
        let address = source(ip);
        let mut by_address = counts(&self.by_address);
        if by_address
            .get(&address)
            .is_some_and(|n| *n >= self.per_address)
        {
            return None;
        }
        let permit = self.total.clone().try_acquire_owned().ok()?;
        *by_address.entry(address).or_default() += 1;
        Some(PreAuth {
            _permit: permit,
            address,
            by_address: self.by_address.clone(),
        })
    }
}

impl Drop for PreAuth {
    fn drop(&mut self) {
        let mut by_address = counts(&self.by_address);
        if let Some(n) = by_address.get_mut(&self.address) {
            *n -= 1;
            if *n == 0 {
                by_address.remove(&self.address);
            }
        }
    }
}

/// How much more a connection may send before it authenticates; `usize::MAX` once it has.
#[derive(Clone)]
struct Allowance(Arc<AtomicUsize>);

impl Allowance {
    fn new(bytes: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(bytes)))
    }

    fn set(&self, bytes: usize) {
        self.0.store(bytes, Ordering::Relaxed);
    }
}

/// A stream that reads no more than its [`Allowance`]: past it, reading fails. It caps what an
/// unauthenticated phone can make the server hold, whatever the WebSocket limits.
struct Capped<S> {
    inner: S,
    allowance: Allowance,
}

impl<S: AsyncRead + Unpin> AsyncRead for Capped<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let left = this.allowance.0.load(Ordering::Relaxed);
        if left == usize::MAX {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }
        if left == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too much sent before authenticating",
            )));
        }
        let n = left.min(buf.remaining());
        let mut limited = ReadBuf::new(&mut buf.initialize_unfilled()[..n]);
        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut limited))?;
        let read = limited.filled().len();
        buf.advance(read);
        // Only this connection's task reads, and it lifts the cap between reads.
        this.allowance.set(left - read);
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Capped<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

async fn accept_loop<H: RemoteHost>(listener: TcpListener, inner: Arc<Inner>, host: Arc<H>) {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let peer = canonical(peer);
                    let local = stream.local_addr().map(canonical).unwrap_or(inner.local_addr);
                    let via = inner
                        .config
                        .admit
                        .as_ref()
                        .and_then(|admit| admit.describe(peer, local))
                        .unwrap_or_else(|| network_of(peer.ip()).to_string());
                    if inner.config.admit.as_ref().is_some_and(|admit| !admit.admits(peer, local)) {
                        tracing::info!(%peer, %local, via, "connection refused: not from a network this machine serves");
                        continue;
                    }
                    let _ = stream.set_nodelay(true);
                    let Some(slot) = inner.pre_auth.take(peer.ip()) else {
                        tracing::info!(%peer, via, "connection refused: too many unauthenticated connections");
                        continue;
                    };
                    tracing::info!(%peer, %local, via, "connection");
                    tokio::spawn(serve(stream, peer, local, via, inner.clone(), host.clone(), slot));
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

#[allow(clippy::too_many_arguments)]
async fn serve<H: RemoteHost>(
    stream: TcpStream,
    peer: SocketAddr,
    local: SocketAddr,
    via: String,
    inner: Arc<Inner>,
    host: Arc<H>,
    unauthenticated: PreAuth,
) {
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(inner.config.max_message))
        .max_frame_size(Some(inner.config.max_message));
    // The handshake and the first message share one deadline.
    let deadline = tokio::time::Instant::now() + inner.config.auth_timeout;
    let stream: Box<dyn Io> = match &inner.tls {
        Some(acceptor) => match tokio::time::timeout_at(deadline, acceptor.accept(stream)).await {
            Ok(Ok(tls)) => Box::new(tls),
            Ok(Err(err)) => {
                tracing::info!(%peer, via, %err, "TLS handshake failed");
                return;
            }
            Err(_) => {
                tracing::info!(%peer, via, "TLS handshake timed out");
                return;
            }
        },
        None => Box::new(stream),
    };
    let allowance = Allowance::new(MAX_UPGRADE);
    let stream: Box<dyn Io> = Box::new(Capped {
        inner: stream,
        allowance: allowance.clone(),
    });
    let handshake =
        tokio_tungstenite::accept_hdr_async_with_config(stream, refuse_browsers, Some(ws_config));
    let mut ws = match tokio::time::timeout_at(deadline, handshake).await {
        Ok(Ok(ws)) => ws,
        Ok(Err(err)) => {
            tracing::info!(%peer, via, %err, "websocket handshake refused or failed");
            return;
        }
        Err(_) => {
            tracing::info!(%peer, via, "websocket handshake timed out");
            return;
        }
    };
    allowance.set(MAX_PRE_AUTH);
    let Some(auth) = authenticate(&mut ws, peer, &via, &inner, deadline).await else {
        return;
    };
    allowance.set(usize::MAX);
    drop(unauthenticated);
    let device_id = auth.device_id.clone();
    let conn_id = auth.conn_id;
    inner.lock().last = Some(LastConnection {
        device_id: device_id.clone(),
        at: now_ms(),
        peer,
        local,
    });
    tracing::info!(%peer, %local, via, device_id, "phone connected");
    inner.notice(ServerNotice::Connected {
        device_id: device_id.clone(),
    });

    run_session(&mut ws, &inner, host, auth).await;

    {
        let mut state = inner.lock();
        let current = state
            .connections
            .get(&device_id)
            .is_some_and(|c| c.conn_id == conn_id);
        if current {
            state.connections.remove(&device_id);
            // Keep the notice ordered with registration: a replacement cannot become current
            // between this check and the old connection saying it went away.
            inner.notice(ServerNotice::Disconnected {
                device_id: device_id.clone(),
            });
        }
        // A stopped server (restarted with a new port or address) leaves devices.json to the
        // one that replaced it: its list here may be stale by now.
        if !*inner.shutdown.borrow() {
            if current {
                state.registry.disconnected(&device_id, now_ms());
            } else {
                // A newer connection of the same device took over: it's still connected.
                state.registry.touch(&device_id, now_ms());
            }
        }
    }
    tracing::info!(%peer, device_id, "phone disconnected");
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
    via: &str,
    inner: &Inner,
    deadline: tokio::time::Instant,
) -> Option<Authenticated> {
    let mut shutdown = inner.shutdown.subscribe();
    loop {
        let frame = tokio::select! {
            frame = ws.next() => frame,
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!(%peer, via, "not authenticated: no pair or hello in time");
                close(ws, CloseCode::Policy, "Authentication timed out").await;
                return None;
            }
            _ = stopped(&mut shutdown) => {
                close(ws, CloseCode::Away, "Trek is shutting down").await;
                return None;
            }
        };
        let text = match frame {
            None | Some(Ok(Message::Close(_))) => {
                tracing::info!(%peer, via, "not authenticated: closed before pair or hello");
                return None;
            }
            Some(Err(err)) => {
                tracing::info!(%peer, via, %err, "not authenticated: the connection failed");
                return None;
            }
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Binary(_))) => {
                tracing::info!(%peer, via, "not authenticated: binary before pair or hello");
                reject(ws, None, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        let (re, parsed) = parse_client(&text);
        let msg = match parsed {
            Ok(msg) => msg,
            Err(bad) if bad.is_auth() => {
                // What was wrong with it stays out: serde may quote the code or token it held.
                tracing::info!(%peer, via, "not authenticated: a malformed pair or hello");
                reject(ws, re, ErrorCode::BadRequest, &bad.message).await;
                return None;
            }
            Err(_) => {
                tracing::info!(%peer, via, "not authenticated: not a message it knows");
                reject(ws, re, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        let (protocol, device_id, app_version, platform) = match &msg {
            ClientMessage::Pair {
                protocol,
                device_id,
                app_version,
                platform,
                ..
            }
            | ClientMessage::Hello {
                protocol,
                device_id,
                app_version,
                platform,
                ..
            } => (
                *protocol,
                device_id.clone(),
                app_version.clone(),
                platform.clone(),
            ),
            _ => {
                tracing::info!(%peer, via, kind = msg.kind(), "not authenticated: a request before pair or hello");
                reject(ws, re, ErrorCode::Unauthorized, "Pair or say hello first").await;
                return None;
            }
        };
        if protocol != PROTOCOL_VERSION {
            let message =
                format!("This Mac speaks protocol {PROTOCOL_VERSION}, the app speaks {protocol}");
            tracing::info!(%peer, via, protocol, "not authenticated: another protocol version");
            reject(ws, re, ErrorCode::UnsupportedProtocol, &message).await;
            return None;
        }
        if !valid_device_id(&device_id) {
            tracing::info!(%peer, via, "not authenticated: an invalid device id");
            reject(ws, re, ErrorCode::BadRequest, "Invalid device_id").await;
            return None;
        }

        let (kick_tx, kick) = oneshot::channel();
        let (host, features, routes) = (
            inner.config.host.clone(),
            inner.config.features.clone(),
            inner.lock().routes.clone(),
        );
        return match msg {
            ClientMessage::Pair {
                code, device_name, ..
            } => {
                let name = clean_name(&device_name);
                let outcome: HostResult<(String, u64)> = {
                    let mut state = inner.lock();
                    state
                        .pairing
                        .check(&code, peer.ip())
                        .map_err(|err| HostError::new(ErrorCode::PairingFailed, err.to_string()))
                        .and_then(|()| {
                            let token = state
                                .registry
                                .register(&device_id, &name, now_ms())
                                .map_err(|err| {
                                    HostError::other(format!(
                                        "Couldn't save the paired device: {err}"
                                    ))
                                })?;
                            state.pairing.cancel();
                            state.registry.connected(
                                &device_id,
                                now_ms(),
                                app_version.as_deref(),
                                platform.as_deref(),
                            );
                            Ok((token, register_conn(&mut state, &device_id, kick_tx)))
                        })
                };
                match outcome {
                    Ok((token, conn_id)) => {
                        tracing::info!(%peer, via, device_id, name, "device paired");
                        inner.notice(ServerNotice::Paired {
                            device_id: device_id.clone(),
                            name,
                        });
                        let paired = ServerMessage::Paired {
                            protocol: PROTOCOL_VERSION,
                            token,
                            host,
                            features,
                            routes,
                        };
                        let _ = send(ws, ServerEnvelope::reply(re, paired)).await;
                        Some(Authenticated {
                            device_id,
                            conn_id,
                            kick,
                        })
                    }
                    Err(err) => {
                        tracing::warn!(%peer, via, %err, "pairing refused");
                        reject(ws, re, err.code, &err.message).await;
                        None
                    }
                }
            }
            ClientMessage::Hello { token, .. } => {
                let conn_id = {
                    let mut state = inner.lock();
                    if state.registry.verify(&device_id, &token) {
                        state.registry.connected(
                            &device_id,
                            now_ms(),
                            app_version.as_deref(),
                            platform.as_deref(),
                        );
                        Some(register_conn(&mut state, &device_id, kick_tx))
                    } else {
                        None
                    }
                };
                match conn_id {
                    Some(conn_id) => {
                        let welcome = ServerMessage::Welcome {
                            protocol: PROTOCOL_VERSION,
                            host,
                            features,
                            routes,
                        };
                        let _ = send(ws, ServerEnvelope::reply(re, welcome)).await;
                        Some(Authenticated {
                            device_id,
                            conn_id,
                            kick,
                        })
                    }
                    None => {
                        tracing::warn!(%peer, via, device_id, "not authenticated: hello with an unknown device or a wrong token");
                        reject(
                            ws,
                            re,
                            ErrorCode::Unauthorized,
                            "Unknown device or revoked token; pair again",
                        )
                        .await;
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
    if let Some(old) = state
        .connections
        .insert(device_id.to_string(), LiveConn { conn_id, kick })
    {
        let _ = old.kick.send(Kick::Replaced);
    }
    conn_id
}

fn valid_device_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= MAX_DEVICE_FIELD && !id.chars().any(char::is_control)
}

fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_FIELD)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "iPhone".to_string()
    } else {
        name.to_string()
    }
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

const CLIENT_TYPES: &[&str] = &[
    "pair",
    "hello",
    "subscribe",
    "unsubscribe",
    "transcript_before",
    "turn_action",
    "send",
    "new_thread",
    "answer",
    "interrupt",
    "mark_seen",
    "set_prefs",
    "thread_action",
    "usage",
    "basecamp",
    "notes",
    "note",
    "create_note",
    "save_note",
    "delete_note",
    "git_status",
    "git_diff",
    "git_commit",
    "git_push",
    "git_branches",
    "git_switch",
    "worktree_merge",
    "worktree_remove",
    "commands",
    "settings",
    "set_settings",
    "ping",
    "search",
    "use_reset",
    "queue_action",
    "attachment",
    "background_output",
    "stop_background",
    "icon",
    "note_asset",
    "add_note_image",
    "note_file",
    "machine_info",
    "clone_project",
    "put_files",
    "card_action",
    "update_phone",
];

/// Parse a text frame: its `id` (for the reply's `re`, even when the rest is bad) and message.
fn parse_client(text: &str) -> (Option<String>, Result<ClientMessage, BadMessage>) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return (
                None,
                Err(BadMessage {
                    kind: None,
                    message: format!("Malformed JSON: {err}"),
                }),
            );
        }
    };
    let Some(object) = value.as_object() else {
        return (
            None,
            Err(BadMessage {
                kind: None,
                message: "Expected a JSON object".into(),
            }),
        );
    };
    let re = object
        .get("id")
        .and_then(|id| id.as_str())
        .map(str::to_string);
    let Some(kind) = object
        .get("type")
        .and_then(|t| t.as_str())
        .map(str::to_string)
    else {
        return (
            re,
            Err(BadMessage {
                kind: None,
                message: "Missing \"type\"".into(),
            }),
        );
    };
    if !CLIENT_TYPES.contains(&kind.as_str()) {
        let message = format!("Unknown message type \"{kind}\"");
        return (
            re,
            Err(BadMessage {
                kind: Some(kind),
                message,
            }),
        );
    }
    match serde_json::from_value::<ClientEnvelope>(value) {
        Ok(envelope) => (re, Ok(envelope.msg)),
        Err(err) => {
            let message = format!("Bad \"{kind}\": {err}");
            (
                re,
                Err(BadMessage {
                    kind: Some(kind),
                    message,
                }),
            )
        }
    }
}

fn error(re: Option<String>, code: ErrorCode, message: impl Into<String>) -> ServerEnvelope {
    ServerEnvelope::reply(
        re,
        ServerMessage::Error {
            code,
            message: message.into(),
        },
    )
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

async fn write_before<T>(
    deadline: Duration,
    future: impl Future<Output = Result<T, tokio_tungstenite::tungstenite::Error>>,
) -> Result<T, tokio_tungstenite::tungstenite::Error> {
    tokio::time::timeout(deadline, future)
        .await
        .unwrap_or_else(|_| {
            Err(tokio_tungstenite::tungstenite::Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "socket write timed out",
            )))
        })
}

async fn write(ws: &mut Ws, message: Message) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    write_before(WRITE_TIMEOUT, ws.send(message)).await
}

async fn send(
    ws: &mut Ws,
    envelope: ServerEnvelope,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    write(ws, Message::text(envelope.to_json())).await
}

/// Send an error, then close the socket.
async fn reject(ws: &mut Ws, re: Option<String>, code: ErrorCode, message: &str) {
    let _ = send(ws, error(re, code, message)).await;
    close(ws, CloseCode::Policy, code.as_str()).await;
}

/// Close the socket, giving the phone a moment to answer the close frame.
async fn close(ws: &mut Ws, code: CloseCode, reason: &str) {
    let frame = CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = tokio::time::timeout(WRITE_TIMEOUT, ws.close(Some(frame))).await;
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
    Transcript {
        re: Option<String>,
        thread_id: String,
        after_seq: Option<u64>,
        limit: Option<u32>,
        generation: u64,
    },
    Call {
        re: Option<String>,
        msg: ClientMessage,
    },
}

/// A finished [`Job`].
#[allow(clippy::large_enum_variant)] // short-lived, one per request
enum Done {
    Snapshot(HostResult<Snapshot>),
    Transcript {
        re: Option<String>,
        thread_id: String,
        after_seq: Option<u64>,
        limit: Option<u32>,
        generation: u64,
        result: HostResult<Transcript>,
    },
    Reply {
        envelope: ServerEnvelope,
        query: bool,
    },
    /// A push for this connection alone, ahead of the reply it comes before (`clone_progress`).
    Push(ServerEnvelope),
}

async fn worker<H: RemoteHost>(
    host: Arc<H>,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    done: mpsc::UnboundedSender<Done>,
) {
    // Changes that may take a while (a commit, a push, a merge, a delete) run off this loop so
    // an approval or an interrupt sent meanwhile isn't held up, but one at a time and in the
    // order they came: a push sent after a commit must not overtake it.
    let (in_order, mut in_order_rx) = mpsc::unbounded_channel::<(Option<String>, ClientMessage)>();
    tokio::spawn({
        let (host, done) = (host.clone(), done.clone());
        async move {
            while let Some((re, msg)) = in_order_rx.recv().await {
                let envelope = call(&*host, re, msg, &done).await;
                let _ = done.send(Done::Reply {
                    envelope,
                    query: false,
                });
            }
        }
    });
    while let Some(job) = jobs.recv().await {
        let finished = match job {
            // Reads are only worth doing for a live connection; actions the phone sent still run.
            Job::Snapshot | Job::Transcript { .. } if done.is_closed() => continue,
            Job::Snapshot => Done::Snapshot(host.snapshot().await),
            Job::Transcript {
                re,
                thread_id,
                after_seq,
                limit,
                generation,
            } => {
                let result = host.transcript_for(&thread_id, after_seq, limit).await;
                Done::Transcript {
                    re,
                    thread_id,
                    after_seq,
                    limit,
                    generation,
                    result,
                }
            }
            // Reads that may take a while (asking the agents for their usage, a recap of all
            // time, git) don't hold up what the phone does next; nor does a clone, which may take
            // minutes (the machine that asked waits for its `ack` before it sends files).
            Job::Call { re, msg }
                if runs_long(&msg)
                    && !msg.is_query()
                    && !matches!(msg, ClientMessage::CloneProject(_)) =>
            {
                let _ = in_order.send((re, msg));
                continue;
            }
            Job::Call { re, msg } if msg.is_query() || runs_long(&msg) => {
                let (host, done) = (host.clone(), done.clone());
                tokio::spawn(async move {
                    let query = msg.is_query();
                    let _ = done.send(Done::Reply {
                        envelope: call(&*host, re, msg, &done).await,
                        query,
                    });
                });
                continue;
            }
            Job::Call { re, msg } => Done::Reply {
                envelope: call(&*host, re, msg, &done).await,
                query: false,
            },
        };
        let _ = done.send(finished);
    }
}

/// An action that may take minutes on the Mac (not a read, so it isn't counted as a query).
fn runs_long(msg: &ClientMessage) -> bool {
    matches!(
        msg,
        ClientMessage::CloneProject(_)
            | ClientMessage::GitCommit(_)
            | ClientMessage::GitPush(_)
            | ClientMessage::WorktreeMerge { .. }
            | ClientMessage::WorktreeRemove(_)
            | ClientMessage::ThreadAction(ThreadActionRequest {
                action: ThreadAction::Delete { .. },
                ..
            })
    )
}

async fn call<H: RemoteHost>(
    host: &H,
    re: Option<String>,
    msg: ClientMessage,
    done: &mpsc::UnboundedSender<Done>,
) -> ServerEnvelope {
    let kind = msg.kind();
    let ack = |thread_id: Option<String>| ServerMessage::Ack {
        thread_id,
        open: None,
        text: None,
        project_id: None,
    };
    let result: HostResult<ServerMessage> = match msg {
        ClientMessage::Send(req) => host.send(req).await.map(|open| ServerMessage::Ack {
            thread_id: None,
            open,
            text: None,
            project_id: None,
        }),
        ClientMessage::TranscriptBefore {
            thread_id,
            before,
            limit,
        } => host
            .transcript_before(&thread_id, &before, limit.min(MAX_PAGE))
            .await
            .map(|page| ServerMessage::TranscriptPage {
                thread_id,
                items: page.items,
                more: page.more,
            }),
        ClientMessage::TurnAction(req) => {
            host.turn_action(req).await.map(|done| ServerMessage::Ack {
                thread_id: done.thread_id,
                open: None,
                text: done.text,
                project_id: None,
            })
        }
        ClientMessage::NewThread(req) => host.new_thread(req).await.map(|id| ack(Some(id))),
        ClientMessage::Answer(req) => host.answer(req).await.map(|()| ack(None)),
        ClientMessage::Interrupt { thread_id } => {
            host.interrupt(&thread_id).await.map(|()| ack(None))
        }
        ClientMessage::MarkSeen { thread_id } => {
            host.mark_seen(&thread_id).await.map(|()| ack(None))
        }
        ClientMessage::SetPrefs(req) => host.set_prefs(req).await.map(|()| ack(None)),
        ClientMessage::ThreadAction(req) => host.thread_action(req).await.map(|()| ack(None)),
        ClientMessage::Usage => host.usage().await.map(ServerMessage::Usage),
        ClientMessage::Basecamp { range } => {
            host.basecamp(range).await.map(ServerMessage::Basecamp)
        }
        ClientMessage::Notes => host
            .notes()
            .await
            .map(|notes| ServerMessage::Notes { notes }),
        ClientMessage::Note { note_id } => host
            .note(&note_id)
            .await
            .map(|note| ServerMessage::Note { note }),
        ClientMessage::CreateNote { body } => host
            .create_note(body)
            .await
            .map(|note| ServerMessage::Note { note }),
        ClientMessage::SaveNote(req) => host
            .save_note(req)
            .await
            .map(|note| ServerMessage::Note { note }),
        ClientMessage::DeleteNote { note_id } => {
            host.delete_note(&note_id).await.map(|()| ack(None))
        }
        ClientMessage::GitStatus(target) => {
            host.git_status(target).await.map(ServerMessage::GitStatus)
        }
        ClientMessage::GitDiff(req) => host.git_diff(req).await.map(ServerMessage::GitDiff),
        ClientMessage::GitCommit(req) => host.git_commit(req).await.map(|()| ack(None)),
        ClientMessage::GitPush(target) => host.git_push(target).await.map(|()| ack(None)),
        ClientMessage::GitBranches(target) => host
            .git_branches(target)
            .await
            .map(ServerMessage::GitBranches),
        ClientMessage::GitSwitch(req) => host.git_switch(req).await.map(|()| ack(None)),
        ClientMessage::WorktreeMerge { thread_id } => {
            host.worktree_merge(&thread_id).await.map(|()| ack(None))
        }
        ClientMessage::WorktreeRemove(req) => host.worktree_remove(req).await.map(|()| ack(None)),
        ClientMessage::Commands { thread_id } => {
            host.commands(&thread_id)
                .await
                .map(|commands| ServerMessage::Commands {
                    thread_id,
                    commands,
                })
        }
        ClientMessage::Settings => host.settings().await.map(ServerMessage::Settings),
        ClientMessage::SetSettings(change) => {
            host.set_settings(change).await.map(ServerMessage::Settings)
        }
        ClientMessage::Search(req) => host.search(req).await.map(ServerMessage::SearchResults),
        ClientMessage::UseReset { credit_id } => {
            host.use_reset(&credit_id).await.map(|()| ack(None))
        }
        ClientMessage::QueueAction(req) => {
            host.queue_action(req).await.map(|text| ServerMessage::Ack {
                thread_id: None,
                open: None,
                text,
                project_id: None,
            })
        }
        ClientMessage::Attachment(req) => host.attachment(req).await.map(ServerMessage::Attachment),
        ClientMessage::BackgroundOutput { thread_id, task_id } => host
            .background_output(&thread_id, &task_id)
            .await
            .map(ServerMessage::BackgroundOutput),
        ClientMessage::StopBackground { thread_id, task_id } => host
            .stop_background(&thread_id, &task_id)
            .await
            .map(|()| ack(None)),
        ClientMessage::Icon(req) => host.icon(req).await.map(ServerMessage::Icon),
        ClientMessage::NoteAsset(req) => host.note_asset(req).await.map(ServerMessage::NoteAsset),
        ClientMessage::AddNoteImage(req) => {
            host.add_note_image(req).await.map(ServerMessage::NoteImage)
        }
        ClientMessage::NoteFile(req) => host.note_file(req).await.map(ServerMessage::NoteFileChunk),
        ClientMessage::MachineInfo => host.machine_info().await.map(ServerMessage::MachineInfo),
        ClientMessage::CloneProject(req) => match req.check() {
            Ok(()) => {
                let done = done.clone();
                let progress = CloneProgress::new(req.folder.clone(), move |msg| {
                    let _ = done.send(Done::Push(ServerEnvelope::push(msg)));
                });
                host.clone_project(req, progress)
                    .await
                    .map(|id| ServerMessage::Ack {
                        thread_id: None,
                        open: None,
                        text: None,
                        project_id: Some(id),
                    })
            }
            Err(why) => Err(HostError::bad_request(why)),
        },
        // Checked here, whatever the host does: paths inside the project, the size in bounds.
        ClientMessage::PutFiles(req) => match req.decode() {
            Ok(files) => host
                .put_files(&req.project_id, files)
                .await
                .map(|()| ack(None)),
            Err(why) => Err(HostError::bad_request(why)),
        },
        ClientMessage::CardAction(req) => host.card_action(req).await.map(|()| ack(None)),
        ClientMessage::UpdatePhone(req) => host.update_phone(req).await.map(|()| ack(None)),
        other => Err(HostError::bad_request(format!(
            "Unexpected \"{}\"",
            other.kind()
        ))),
    };
    match result {
        Ok(reply) => ServerEnvelope::reply(re, reply),
        Err(err) => {
            tracing::debug!(kind, %err, "remote request failed");
            error(re, err.code, err.message)
        }
    }
}

/// A transcript subscription.
enum Sub {
    /// Waiting for the host's transcript; items pushed meanwhile are held until it's sent.
    Pending {
        generation: u64,
        held: Vec<Item>,
        reset: bool,
    },
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
    queries_in_flight: u8,
    held_events: Vec<HostEvent>,
}

impl Session {
    fn on_request(
        &mut self,
        re: Option<String>,
        msg: ClientMessage,
        jobs: &mpsc::UnboundedSender<Job>,
        watchers: &Watchers,
    ) -> Vec<ServerEnvelope> {
        match msg {
            ClientMessage::Pair { .. } | ClientMessage::Hello { .. } => {
                vec![error(re, ErrorCode::BadRequest, "Already authenticated")]
            }
            ClientMessage::Ping => vec![ServerEnvelope::reply(re, ServerMessage::Pong)],
            ClientMessage::Subscribe {
                thread_id,
                after_seq,
                limit,
            } => {
                self.next_generation += 1;
                let generation = self.next_generation;
                let sub = Sub::Pending {
                    generation,
                    held: Vec::new(),
                    reset: false,
                };
                if self.subs.insert(thread_id.clone(), sub).is_none() {
                    watchers.add(&thread_id);
                }
                let _ = jobs.send(Job::Transcript {
                    re,
                    thread_id,
                    after_seq,
                    limit,
                    generation,
                });
                Vec::new()
            }
            ClientMessage::Unsubscribe { thread_id } => {
                if self.subs.remove(&thread_id).is_some() {
                    watchers.remove(&thread_id);
                }
                match re {
                    Some(re) => vec![ServerEnvelope::reply(
                        Some(re),
                        ServerMessage::Ack {
                            thread_id: None,
                            open: None,
                            text: None,
                            project_id: None,
                        },
                    )],
                    None => Vec::new(),
                }
            }
            msg if msg.is_query() && self.queries_in_flight >= 8 => {
                vec![error(
                    re,
                    ErrorCode::RateLimited,
                    "Too many queries in flight",
                )]
            }
            // Everything else is the host's to answer, in the order sent.
            msg => {
                if msg.is_query() {
                    self.queries_in_flight += 1;
                }
                let _ = jobs.send(Job::Call { re, msg });
                Vec::new()
            }
        }
    }

    fn request_snapshot(&mut self, jobs: &mpsc::UnboundedSender<Job>) {
        self.snapshots_pending += 1;
        let _ = jobs.send(Job::Snapshot);
    }

    /// The connection is over: what it had open, it has open no more.
    fn close(&mut self, watchers: &Watchers) {
        for thread_id in std::mem::take(&mut self.subs).into_keys() {
            watchers.remove(&thread_id);
        }
    }

    fn on_done(&mut self, done: Done, watchers: &Watchers) -> Vec<ServerEnvelope> {
        match done {
            Done::Reply { envelope, query } => {
                if query {
                    self.queries_in_flight = self.queries_in_flight.saturating_sub(1);
                }
                vec![envelope]
            }
            Done::Push(envelope) => vec![envelope],
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
            Done::Transcript {
                re,
                thread_id,
                after_seq,
                limit,
                generation,
                result,
            } => {
                let current = matches!(self.subs.get(&thread_id), Some(Sub::Pending { generation: g, .. }) if *g == generation);
                if !self.subs.contains_key(&thread_id) {
                    // Unsubscribed while the host read it (and so started following it again).
                    watchers.recheck(&thread_id);
                }
                let transcript = match result {
                    Ok(transcript) => transcript,
                    Err(err) => {
                        if current {
                            self.subs.remove(&thread_id);
                            watchers.remove(&thread_id);
                        }
                        return vec![error(re, err.code, err.message)];
                    }
                };
                let seq = transcript.seq;
                let reply = transcript_message(thread_id.clone(), after_seq, limit, transcript);
                let mut out = vec![ServerEnvelope::reply(re, reply)];
                if current
                    && let Some(Sub::Pending { held, reset, .. }) =
                        self.subs.insert(thread_id.clone(), Sub::Active)
                {
                    for item in held.into_iter().filter(|item| item.seq > seq) {
                        out.push(ServerEnvelope::push(ServerMessage::Item {
                            thread_id: thread_id.clone(),
                            item,
                        }));
                    }
                    if reset {
                        out.push(ServerEnvelope::push(ServerMessage::TranscriptReset {
                            thread_id,
                        }));
                    }
                }
                out
            }
        }
    }

    fn on_event(&mut self, event: HostEvent) -> Vec<ServerEnvelope> {
        let msg = match event {
            HostEvent::Snapshot(_)
            | HostEvent::Thread(_)
            | HostEvent::Unlisted(_)
            | HostEvent::ThreadRemoved(_)
                if self.snapshots_pending > 0 =>
            {
                self.held_events.push(event);
                return Vec::new();
            }
            HostEvent::Snapshot(snapshot) => ServerMessage::Snapshot(snapshot),
            HostEvent::Thread(thread) => ServerMessage::Thread { thread },
            HostEvent::Unlisted(thread) => ServerMessage::Unlisted { thread },
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
            HostEvent::PhoneUpdate(update) => ServerMessage::PhoneUpdate(update),
        };
        vec![ServerEnvelope::push(msg)]
    }

    /// The phone missed events: send everything it relies on again.
    fn resync(&mut self, jobs: &mpsc::UnboundedSender<Job>) -> Vec<ServerEnvelope> {
        self.request_snapshot(jobs);
        let mut out = Vec::new();
        for (thread_id, sub) in &mut self.subs {
            match sub {
                Sub::Active => out.push(ServerEnvelope::push(ServerMessage::TranscriptReset {
                    thread_id: thread_id.clone(),
                })),
                Sub::Pending { reset, .. } => *reset = true,
            }
        }
        out
    }
}

/// The reply to `subscribe`: only what's after `after_seq` when the host can serve it, else all
/// (or, with a `limit`, the last `limit` items with what follows them, and every open request).
fn transcript_message(
    thread_id: String,
    after_seq: Option<u64>,
    limit: Option<u32>,
    transcript: Transcript,
) -> ServerMessage {
    let Transcript {
        seq,
        items,
        base,
        more,
    } = transcript;
    match after_seq {
        Some(after) if base <= after && after <= seq => ServerMessage::Transcript {
            thread_id,
            reset: false,
            seq,
            items: items.into_iter().filter(|item| item.seq > after).collect(),
            more: false,
        },
        _ => {
            let (items, cut) = last_items(items, limit);
            ServerMessage::Transcript {
                thread_id,
                reset: true,
                seq,
                items,
                more: more || cut,
            }
        }
    }
}

/// The last `limit` items (those standing on their own: a turn's changed files go with the turn
/// end they follow) and every open request; and whether any were left out.
fn last_items(items: Vec<Item>, limit: Option<u32>) -> (Vec<Item>, bool) {
    let Some(limit) = limit else {
        return (items, false);
    };
    let (requests, mut rest): (Vec<Item>, Vec<Item>) =
        items.into_iter().partition(|i| i.body.is_request());
    let mut kept = 0;
    let mut start = rest.len();
    for (ix, item) in rest.iter().enumerate().rev() {
        if kept == limit {
            break;
        }
        if !item.body.is_attached() {
            kept += 1;
        }
        start = ix;
    }
    let cut = start > 0;
    let mut out = rest.split_off(start);
    out.extend(requests);
    (out, cut)
}

async fn run_session<H: RemoteHost>(ws: &mut Ws, inner: &Inner, host: Arc<H>, auth: Authenticated) {
    let mut session = Session::default();
    session_loop(ws, inner, host, auth, &mut session).await;
    session.close(&inner.watchers);
}

async fn session_loop<H: RemoteHost>(
    ws: &mut Ws,
    inner: &Inner,
    host: Arc<H>,
    auth: Authenticated,
    session: &mut Session,
) {
    let Authenticated {
        device_id,
        mut kick,
        ..
    } = auth;
    let watchers = &inner.watchers;
    // Listen for events before asking for the snapshot, so nothing falls between the two.
    let mut events = inner.events.subscribe();
    let mut shutdown = inner.shutdown.subscribe();
    let (jobs, jobs_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done) = mpsc::unbounded_channel();
    tokio::spawn(worker(host, jobs_rx, done_tx));

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
                            session.on_request(re, msg, &jobs, watchers)
                        }
                        (re, Err(bad)) => vec![error(re, ErrorCode::BadRequest, bad.message)],
                    },
                    Some(Ok(Message::Binary(_))) => vec![error(None, ErrorCode::BadRequest, "Text frames only")],
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => Vec::new(),
                }
            }
            Some(finished) = done.recv() => session.on_done(finished, watchers),
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
                if write(ws, Message::Ping(Default::default())).await.is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn pre_auth_places_are_capped_per_address() {
        let slots = PreAuthSlots::new(6, 4);
        let (a, b) = (
            IpAddr::from([192, 168, 1, 66]),
            IpAddr::from([192, 168, 1, 7]),
        );
        let held: Vec<PreAuth> = (0..4).map(|_| slots.take(a).expect("a place")).collect();
        assert!(slots.take(a).is_none(), "a fifth from one address waits");
        // Another device still gets in, up to the total.
        let b1 = slots.take(b).expect("another address isn't held up");
        let _b2 = slots.take(b).expect("another address isn't held up");
        assert!(
            slots.take(IpAddr::from([10, 0, 0, 1])).is_none(),
            "six in all"
        );
        drop(b1);
        assert!(slots.take(IpAddr::from([10, 0, 0, 1])).is_some());
        // An IPv6 device is one address, however many it has in its /64.
        let v6 = PreAuthSlots::new(16, 1);
        let _one = v6.take("fd00::1".parse().unwrap()).unwrap();
        assert!(v6.take("fd00::2:3".parse().unwrap()).is_none());
        drop(held);
        assert!(slots.take(a).is_some());
        assert_eq!(
            counts(&slots.by_address).get(&a),
            None,
            "places given back are forgotten"
        );
    }

    #[tokio::test]
    async fn capped_reads_stop_at_the_allowance() {
        let (mut phone, server) = tokio::io::duplex(1 << 16);
        let allowance = Allowance::new(10);
        let mut capped = Capped {
            inner: server,
            allowance: allowance.clone(),
        };
        phone.write_all(&[7; 32]).await.unwrap();
        let mut buf = [0; 64];
        assert_eq!(capped.read(&mut buf).await.unwrap(), 10);
        assert_eq!(
            capped.read(&mut buf).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        allowance.set(usize::MAX);
        assert_eq!(capped.read(&mut buf).await.unwrap(), 22);
    }

    #[tokio::test]
    async fn socket_write_deadline_expires() {
        let pending = std::future::pending::<Result<(), tokio_tungstenite::tungstenite::Error>>();
        let err = write_before(Duration::from_millis(1), pending)
            .await
            .unwrap_err();
        assert!(
            matches!(err, tokio_tungstenite::tungstenite::Error::Io(ref err) if err.kind() == io::ErrorKind::TimedOut)
        );
    }
}
