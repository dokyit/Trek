//! Trek on your iPhone, the Mac side: the phone server (`trek-remote`) answered from the
//! workspace. Off until Settings › Phone turns it on. Phones pair with a one-time code shown as
//! a QR code, then connect with their own token over TLS pinned to this Mac's certificate.
//!
//! What a phone can do is what the Mac's own controls do: read threads, send a follow-up (steered
//! or queued as the user's setting says), start a thread, answer an approval, a question or a
//! plan, stop a turn; read usage, Basecamp and notes; review, commit and merge git work; change
//! some settings. Trek's own commands run as typed on the Mac, except that Full access stays
//! locked until the Mac unlocks it (whichever way it's asked for), and commands that move the
//! Mac's window (`/new`) open the phone's sheet instead. Nothing is ever answered on the phone's
//! behalf.
//!
//! Changes go out on a short tick while the server runs: thread rows that changed, and the
//! transcript items of threads a phone has open (by index; each change takes the next `seq` of
//! that thread, as the protocol asks). A tick looks only at what can have changed: nothing for a
//! thread whose transcript didn't move, and from the first item edited (or the one streaming)
//! on for one that did. A thread no phone has open any more isn't followed (the server says
//! when, `unwatch`); one opened again carries on from where it was. Long transcripts open at
//! their end (`subscribe` with a `limit`) and are paged in (`transcript_before`); items no phone
//! has been sent aren't followed either.

use crate::workspace::{ForkAt, LiveThread, Route, Workspace, WorkspaceEvent};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use trek_core::store::{Item, Section, Thread, ToolStatus, now_ms};
use trek_core::{AgentId, RunState};
use trek_remote as tr;

mod basecamp;
mod commands;
mod git;
mod notes;
mod settings;
mod usage;

/// How often changes go out to phones.
const TICK: Duration = Duration::from_millis(250);
/// Settled threads beyond this many (newest first) stay off the phone.
const SETTLED_ON_PHONE: usize = 150;

/// The running phone server and what's been sent through it.
pub struct Remote {
    pub handle: tr::RemoteHandle,
    /// The pairing code on screen, if any.
    pub offer: Option<tr::PairingOffer>,
    /// This Mac's addresses as found when the server started.
    pub addresses: Addresses,
    /// Where the pairing code points phones (`host:port`).
    pub advertise: String,
    /// The port and reach it started with: a change restarts it.
    started_with: (u16, trek_core::settings::Reach),
    /// Paired devices and which are connected, refreshed from the server's notices.
    pub devices: Vec<tr::DeviceInfo>,
    pub connected: HashSet<String>,
    /// Thread rows as last sent.
    pub(crate) sent: HashMap<String, tr::ThreadSummary>,
    /// How phones answered requests, by thread and request id, so their cards close saying so.
    pub(crate) answered: HashMap<(String, String), tr::AnswerResponse>,
    /// Transcripts phones have open: followed every tick.
    pub(crate) watched: HashMap<String, Watched>,
    /// Transcripts no phone has open any more, as they were last sent: nothing is done for them
    /// each tick, and a phone opening one again carries on where they left off (seq, and what
    /// changed since).
    pub(crate) dormant: HashMap<String, Watched>,
    _tasks: Vec<Task<()>>,
}

/// A transcript phones follow: what each item looked like when last sent.
pub(crate) struct Watched {
    /// The last seq handed out.
    pub(crate) seq: u64,
    /// Phones that saw less than this get the whole transcript again (`tr::Transcript::base`).
    base: u64,
    /// Each item, by index: its seq, and the hash of what was sent.
    items: Vec<Sent>,
    /// Items before this one haven't gone to any phone (a subscribe with a `limit` left them
    /// out): changes to them aren't sent, and a page of them brings them as they are.
    from: usize,
    /// Requests shown as cards, by id, as last sent, with their seq.
    requests: Vec<(String, tr::ItemBody, u64)>,
    /// The files each turn changed, by the index of its turn end, as last sent.
    changes: HashMap<usize, Sent>,
    /// What the transcript was last looked at with: its revision, the item streaming, the
    /// thread's folder (tool rows name paths in it).
    revision: u64,
    streaming: Option<usize>,
    cwd: Option<PathBuf>,
    /// Turns' changed files are looked at again on the next tick (one was counted, or waits).
    recount: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Sent {
    seq: u64,
    /// `None`: not sent yet.
    hash: Option<u64>,
}

impl Watched {
    /// Nothing sent yet, `len` items numbered. Seqs start at the time (in µs) so a phone holding
    /// seqs from before Trek last started can't take them for this run's.
    fn fresh(len: usize) -> Self {
        let seq = now_ms().max(0) as u64 * 1000;
        Watched {
            seq,
            base: seq,
            items: vec![Sent { seq, hash: None }; len],
            from: len,
            requests: vec![],
            changes: HashMap::new(),
            revision: 0,
            streaming: None,
            cwd: None,
            recount: false,
        }
    }

    /// The transcript went shorter (a rewind, a rewrite): numbered afresh, nothing sent, and
    /// phones that saw it before read it whole again.
    fn restart(&mut self, len: usize) {
        self.seq += 1;
        self.base = self.seq;
        self.items = vec![Sent { seq: self.seq, hash: None }; len];
        self.from = len;
        self.requests.clear();
        self.changes.clear();
    }
}

/// This Mac's IPv4 addresses a phone could reach.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Addresses {
    pub lan: Option<Ipv4Addr>,
    pub tailscale: Option<Ipv4Addr>,
}

impl Addresses {
    pub fn find() -> Self {
        let mut out = Self::default();
        for ip in interface_addresses() {
            let [a, b, ..] = ip.octets();
            if a == 100 && (64..128).contains(&b) {
                out.tailscale.get_or_insert(ip);
            } else if ip.is_private() && out.lan.is_none() {
                out.lan = Some(ip);
            }
        }
        // The address the default route leaves from, when the interface list didn't say.
        if out.lan.is_none() {
            out.lan = std::net::UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| s.connect("192.168.0.1:9").map(|_| s))
                .and_then(|s| s.local_addr())
                .ok()
                .and_then(|a| match a.ip() {
                    IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(v4),
                    _ => None,
                });
        }
        out
    }
}

pub(crate) fn remote_endpoint(addresses: &Addresses, reach: trek_core::settings::Reach, port: u16) -> std::io::Result<(SocketAddr, String)> {
    match reach {
        trek_core::settings::Reach::Tailscale => {
            let ip = addresses.tailscale.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "Tailscale isn't connected; choose Wi-Fi or connect Tailscale"))?;
            Ok((SocketAddr::from((ip, port)), format!("{ip}:{port}")))
        }
        trek_core::settings::Reach::Wifi => {
            let ip = addresses.lan.or(addresses.tailscale).unwrap_or(Ipv4Addr::LOCALHOST);
            Ok((SocketAddr::from(([0, 0, 0, 0], port)), format!("{ip}:{port}")))
        }
    }
}

/// The IPv4 addresses of the interfaces that are up (not loopback).
fn interface_addresses() -> Vec<Ipv4Addr> {
    let mut out = vec![];
    // SAFETY: getifaddrs hands back a list we only read, then free with freeifaddrs.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return out;
        }
        let mut cur = list;
        while !cur.is_null() {
            let ifa = &*cur;
            let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0 && ifa.ifa_flags & libc::IFF_LOOPBACK as u32 == 0;
            if up && !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                out.push(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    out
}

fn mobile_dir() -> PathBuf {
    trek_core::paths::data_dir().join("mobile")
}

/// The Mac as phones see it: a lasting id, its name, Trek's version.
fn host_info() -> tr::HostInfo {
    let id_file = mobile_dir().join("host-id");
    let id = std::fs::read_to_string(&id_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| {
        let id = uuid_like();
        let _ = std::fs::create_dir_all(mobile_dir());
        let _ = std::fs::write(&id_file, &id);
        id
    });
    tr::HostInfo { id, name: computer_name(), version: env!("CARGO_PKG_VERSION").to_string() }
}

fn uuid_like() -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (now_ms(), std::process::id(), trek_core::paths::home()).hash(&mut h);
    format!("{:016x}{:08x}", h.finish(), std::process::id())
}

/// "Tobias's MacBook Pro", as Sharing settings name it.
fn computer_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".to_string())
}

impl Workspace {
    /// Start or stop the phone server as the settings say.
    pub fn sync_remote(&mut self, cx: &mut Context<Self>) {
        let want = self.settings.mobile.enabled;
        let wanted_with = (self.settings.mobile.port, self.settings.mobile.reach);
        match (&self.remote, want) {
            (None, true) if !self.remote_starting => self.start_remote(cx),
            (Some(_), false) => self.stop_remote(cx),
            // Another port or address: start again with it (paired phones stay paired).
            (Some(r), true) if r.started_with != wanted_with => {
                self.stop_remote(cx);
                self.start_remote(cx);
            }
            _ => {}
        }
    }

    fn start_remote(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        self.remote_starting = true;
        let port = self.settings.mobile.port;
        let reach = self.settings.mobile.reach;
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    let host = host_info();
                    let identity = tr::TlsIdentity::load_or_create(&mobile_dir(), &host.name)?;
                    let addresses = Addresses::find();
                    let (bind, advertise) = remote_endpoint(&addresses, reach, port)?;
                    let mut config = tr::ServerConfig::new(host);
                    config.bind = bind;
                    config.advertise = Some(advertise.clone());
                    config.devices_path = Some(mobile_dir().join("devices.json"));
                    config.tls = Some(identity);
                    let (host, requests) = tr::ChannelHost::new();
                    let host = Arc::new(host);
                    let handle = trek_core::runtime()
                        .spawn(async move {
                            // A server just stopped (a new port or address) lets go of its port a
                            // moment later.
                            let mut tries = 0;
                            loop {
                                match tr::RemoteServer::start(config.clone(), host.clone()).await {
                                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 20 => {
                                        tries += 1;
                                        tokio::time::sleep(Duration::from_millis(100)).await;
                                    }
                                    other => break other,
                                }
                            }
                        })
                        .await
                        .map_err(std::io::Error::other)??;
                    anyhow::Ok((handle, requests, addresses, advertise))
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                ws.remote_starting = false;
                match started {
                    Ok((handle, requests, addresses, advertise)) if ws.settings.mobile.enabled => {
                        let tasks = vec![serve_requests(cx.weak_entity(), requests, cx), push_changes(cx.weak_entity(), cx), hear_notices(cx.weak_entity(), handle.notices(), cx)];
                        let devices = handle.devices();
                        ws.remote = Some(Remote { handle, offer: None, addresses, advertise, started_with: (port, reach), devices, connected: HashSet::new(), sent: HashMap::new(), answered: HashMap::new(), watched: HashMap::new(), dormant: HashMap::new(), _tasks: tasks });
                    }
                    // Turned off while it started.
                    Ok((handle, ..)) => handle.shutdown(),
                    Err(e) => {
                        ws.settings.mobile.enabled = false;
                        ws.save_settings(cx);
                        cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't start the phone server: {e:#}"), undo: None });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A phone server on the loopback, plain and in memory, with a phone taken as connected:
    /// what `start_remote` sets up, for tests of what goes out to phones.
    #[cfg(test)]
    pub(crate) fn start_test_remote(&mut self) {
        let mut config = tr::ServerConfig::new(tr::HostInfo { id: "test".into(), name: "Test Mac".into(), version: "0".into() });
        config.bind = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let (host, _requests) = tr::ChannelHost::new();
        let handle = trek_core::runtime().block_on(tr::RemoteServer::start(config, Arc::new(host))).expect("a test server");
        let connected = HashSet::from(["phone".to_string()]);
        let started_with = (0, trek_core::settings::Reach::Wifi);
        self.remote = Some(Remote { handle, offer: None, addresses: Addresses::default(), advertise: String::new(), started_with, devices: vec![], connected, sent: HashMap::new(), answered: HashMap::new(), watched: HashMap::new(), dormant: HashMap::new(), _tasks: vec![] });
    }

    fn stop_remote(&mut self, cx: &mut Context<Self>) {
        if let Some(remote) = self.remote.take() {
            remote.handle.shutdown();
        }
        cx.notify();
    }

    /// Show a new pairing code (QR code and text) in Settings › Phone.
    pub fn offer_pairing(&mut self, cx: &mut Context<Self>) {
        if let Some(remote) = self.remote.as_mut() {
            remote.offer = Some(remote.handle.pairing_offer());
            cx.notify();
        }
    }

    pub fn cancel_pairing(&mut self, cx: &mut Context<Self>) {
        if let Some(remote) = self.remote.as_mut() {
            remote.handle.cancel_pairing();
            remote.offer = None;
            cx.notify();
        }
    }

    pub fn revoke_device(&mut self, device_id: &str, cx: &mut Context<Self>) {
        if let Some(remote) = self.remote.as_mut() {
            remote.handle.revoke(device_id);
            remote.devices = remote.handle.devices();
            remote.connected.remove(device_id);
            cx.notify();
        }
    }

    // ---- requests from phones ----

    pub(crate) fn remote_request(&mut self, req: tr::HostRequest, cx: &mut Context<Self>) {
        match req {
            tr::HostRequest::Snapshot { reply } => {
                let _ = reply.send(Ok(self.remote_snapshot()));
            }
            tr::HostRequest::Transcript { thread_id, after_seq, limit, reply } => {
                let id = thread_id.clone();
                self.when_loaded(thread_id, reply, cx, move |ws, cx| Ok(ws.remote_transcript(&id, after_seq, limit, cx)));
            }
            tr::HostRequest::TranscriptBefore { thread_id, before, limit, reply } => {
                let id = thread_id.clone();
                self.when_loaded(thread_id, reply, cx, move |ws, cx| ws.remote_transcript_before(&id, &before, limit, cx));
            }
            tr::HostRequest::TurnAction { req, reply } => {
                self.when_loaded(req.thread_id.clone(), reply, cx, move |ws, cx| ws.remote_turn_action(req, cx));
            }
            tr::HostRequest::Unwatch { thread_id } => self.remote_unwatch(&thread_id),
            tr::HostRequest::Send { req, reply } => {
                let _ = reply.send(self.remote_send(req, cx));
            }
            tr::HostRequest::NewThread { req, reply } => {
                let _ = reply.send(self.remote_new_thread(req, cx));
            }
            tr::HostRequest::Answer { req, reply } => {
                let _ = reply.send(self.remote_answer(req, cx));
            }
            tr::HostRequest::Interrupt { thread_id, reply } => {
                if self.thread(&thread_id).is_none() {
                    let _ = reply.send(Err(tr::HostError::not_found("No such thread")));
                    return;
                }
                self.interrupt(&thread_id, cx);
                let _ = reply.send(Ok(()));
            }
            tr::HostRequest::SetPrefs { req, reply } => {
                let _ = reply.send(self.remote_set_prefs(req, cx));
            }
            tr::HostRequest::ThreadAction { req, reply } => {
                let _ = reply.send(self.remote_thread_action(req, cx));
            }
            tr::HostRequest::MarkSeen { thread_id, reply } => {
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.last_seen_at = now_ms().max(t.updated_at);
                    let _ = self.store.save_thread(t);
                    cx.notify();
                }
                let _ = reply.send(Ok(()));
            }
            tr::HostRequest::Usage { reply } => self.remote_usage(reply, cx),
            tr::HostRequest::Basecamp { range, reply } => self.remote_basecamp(range, reply, cx),
            tr::HostRequest::Notes { reply } => {
                let _ = reply.send(Ok(notes::list()));
            }
            tr::HostRequest::Note { note_id, reply } => {
                let _ = reply.send(notes::get(&note_id));
            }
            tr::HostRequest::CreateNote { body, reply } => {
                let _ = reply.send(self.remote_create_note(body, cx));
            }
            tr::HostRequest::SaveNote { req, reply } => {
                let _ = reply.send(self.remote_save_note(req, cx));
            }
            tr::HostRequest::DeleteNote { note_id, reply } => {
                let _ = reply.send(self.remote_delete_note(&note_id, cx));
            }
            tr::HostRequest::GitStatus { target, reply } => self.remote_git_status(target, reply, cx),
            tr::HostRequest::GitDiff { req, reply } => self.remote_git_diff(req, reply, cx),
            tr::HostRequest::GitCommit { req, reply } => self.remote_git_commit(req, reply, cx),
            tr::HostRequest::GitPush { target, reply } => self.remote_git_push(target, reply, cx),
            tr::HostRequest::GitBranches { target, reply } => self.remote_git_branches(target, reply, cx),
            tr::HostRequest::GitSwitch { req, reply } => self.remote_git_switch(req, reply, cx),
            tr::HostRequest::WorktreeMerge { thread_id, reply } => self.remote_worktree_merge(&thread_id, reply, cx),
            tr::HostRequest::WorktreeRemove { req, reply } => self.remote_worktree_remove(req, reply, cx),
            tr::HostRequest::Commands { thread_id, reply } => {
                let _ = reply.send(self.remote_commands(&thread_id));
            }
            tr::HostRequest::Settings { reply } => {
                let _ = reply.send(Ok(self.remote_settings()));
            }
            tr::HostRequest::SetSettings { change, reply } => {
                let _ = reply.send(self.remote_set_settings(change, cx));
            }
        }
    }

    fn remote_send(&mut self, req: tr::SendRequest, cx: &mut Context<Self>) -> tr::HostResult<Option<tr::Open>> {
        let Some(t) = self.thread(&req.thread_id).cloned() else { return Err(tr::HostError::not_found("No such thread")) };
        // Trek's own commands: `/new` is the phone's to carry out, `/consult` and `/restate` become
        // the message the composer would send, the rest are answered as on the Mac.
        let text = match self.phone_command(&t, &req.text)? {
            commands::Phone::Open(open) => return Ok(Some(open)),
            commands::Phone::Send(text) => text,
        };
        // An explicit choice from the phone wins over the setting, for this message only.
        let before = self.settings.general.follow_up;
        if let Some(mode) = req.mode {
            self.settings.general.follow_up = match mode {
                tr::SendMode::Steer => trek_core::settings::FollowUp::Steer,
                tr::SendMode::Queue => trek_core::settings::FollowUp::Queue,
            };
        }
        let images = save_uploads(&req.images)?;
        self.send_to(&req.thread_id, text, images, cx);
        self.settings.general.follow_up = before;
        Ok(None)
    }

    /// A thread's agent, model, effort, access or plan mode, changed as its composer would.
    fn remote_set_prefs(&mut self, req: tr::PrefsRequest, cx: &mut Context<Self>) -> tr::HostResult<()> {
        if self.thread(&req.thread_id).is_none() {
            return Err(tr::HostError::not_found("No such thread"));
        }
        let scope = crate::workspace::Scope::Thread(req.thread_id.clone());
        let mut prefs = self.prefs_in(&scope);
        if let Some(key) = &req.agent {
            let agent = AgentId::from_key(key);
            if !self.ready_agents().contains(&agent) {
                return Err(tr::HostError::not_found(format!("{} isn't set up on this Mac", agent.display_name())));
            }
            if agent != prefs.agent {
                // Another agent starts on its default model, as its model menu shows first.
                prefs.model = crate::composer::default_model(&self.models_for(&agent)).map(|m| m.id.clone());
                prefs.agent = agent;
            }
        }
        let models = self.models_for(&prefs.agent);
        if let Some(model) = &req.model {
            if !models.is_empty() && !models.iter().any(|m| &m.id == model) {
                return Err(tr::HostError::not_found(format!("{model} isn't one of {}'s models", prefs.agent.display_name())));
            }
            prefs.model = Some(model.clone());
        }
        if let Some(effort) = &req.effort {
            prefs.effort = trek_core::Effort::parse(effort).ok_or_else(|| tr::HostError::bad_request(format!("No effort called {effort}")))?;
        }
        // An effort the model doesn't take becomes the nearest it does (Codex has no "max").
        if let Some(m) = prefs.model.as_ref().and_then(|id| models.iter().find(|m| crate::composer::same_model(id, &m.id))).filter(|m| !m.efforts.is_empty()) {
            prefs.effort = prefs.effort.clamp_to(&m.efforts);
        }
        if let Some(access) = req.access {
            prefs.hand_holding = hand_holding(access, &self.settings)?;
        }
        if let Some(plan) = req.plan {
            prefs.plan = plan;
        }
        self.set_prefs_in(&scope, prefs, cx);
        Ok(())
    }

    fn remote_thread_action(&mut self, req: tr::ThreadActionRequest, cx: &mut Context<Self>) -> tr::HostResult<()> {
        let Some(t) = self.thread(&req.thread_id).cloned() else { return Err(tr::HostError::not_found("No such thread")) };
        let id = t.id.as_str();
        match req.action {
            tr::ThreadAction::Pin if t.pinned_at.is_none() => self.toggle_pin(id, cx),
            tr::ThreadAction::Unpin if t.pinned_at.is_some() => self.toggle_pin(id, cx),
            tr::ThreadAction::Pin | tr::ThreadAction::Unpin => {}
            tr::ThreadAction::Settle => self.settle(id, cx),
            tr::ThreadAction::Unsettle => self.unsettle(id, cx),
            tr::ThreadAction::Archive => self.archive(id, cx),
            tr::ThreadAction::Rename { title } => self.rename(id, title, cx),
        }
        Ok(())
    }

    fn remote_new_thread(&mut self, req: tr::NewThreadRequest, cx: &mut Context<Self>) -> tr::HostResult<String> {
        let agent = AgentId::from_key(&req.agent);
        let text = self.phone_first_message(&agent, req.model.as_deref(), &req.text)?;
        let project = match req.project_id.as_str() {
            "" => None,
            id => Some(self.project(id).map(|p| p.path.clone()).ok_or_else(|| tr::HostError::not_found("No such project"))?),
        };
        if !self.ready_agents().contains(&agent) {
            return Err(tr::HostError::not_found(format!("{} isn't set up on this Mac", agent.display_name())));
        }
        let images = save_uploads(&req.images)?;
        let hand_holding = req.access.map(|a| hand_holding(a, &self.settings)).transpose()?;
        let effort = req.effort.as_deref().map(|e| trek_core::Effort::parse(e).ok_or_else(|| tr::HostError::bad_request(format!("No effort called {e}")))).transpose()?;
        // Started the way the Mac's composer starts one, without taking over the Mac's screen.
        let (route, prefs, tabs) = (self.route.clone(), self.draft_prefs.clone(), self.tabs.clone());
        self.route = Route::Draft { project };
        self.draft_prefs.agent = agent;
        self.draft_prefs.model = req.model.clone();
        self.draft_prefs.worktree = req.worktree;
        self.draft_prefs.plan = req.plan;
        if let Some(h) = hand_holding {
            self.draft_prefs.hand_holding = h;
        }
        if let Some(e) = effort {
            self.draft_prefs.effort = e;
        }
        self.send(text, images, cx);
        let started = match &self.route {
            Route::Thread(id) => Some(id.clone()),
            _ => None,
        };
        self.route = route;
        self.draft_prefs = prefs;
        self.tabs = tabs;
        cx.notify();
        started.ok_or_else(|| tr::HostError::other("The thread didn't start"))
    }

    fn remote_answer(&mut self, req: tr::AnswerRequest, cx: &mut Context<Self>) -> tr::HostResult<()> {
        let id = req.thread_id.clone();
        if self.thread(&id).is_none() {
            return Err(tr::HostError::not_found("No such thread"));
        }
        let pending = self.live.get(&id).and_then(|l| l.permissions.iter().find(|p| p.request_id == req.request_id)).map(|p| p.prompt.clone());
        let Some(prompt) = pending else {
            return Err(tr::HostError::conflict("Already answered"));
        };
        let phone_response = req.response.clone();
        let mut response = req.response;
        if let (tr::AnswerResponse::Questions { answers }, Some(trek_agents::Prompt::Questions(questions))) = (&mut response, &prompt) {
            restore_phone_answers(answers, questions);
        }
        if let Some(remote) = self.remote.as_mut() {
            remote.answered.insert((id.clone(), req.request_id.clone()), phone_response);
        }
        match (response, prompt) {
            (tr::AnswerResponse::Approval { decision }, None) => {
                let decision = match decision {
                    tr::Decision::Allow => trek_agents::Decision::Allow,
                    tr::Decision::AllowForSession => trek_agents::Decision::AllowForSession,
                    tr::Decision::Deny => trek_agents::Decision::Deny,
                };
                self.respond(&id, &req.request_id, decision, cx);
            }
            (tr::AnswerResponse::Questions { answers }, Some(trek_agents::Prompt::Questions(_))) => {
                self.answer(&id, &req.request_id, answers.into_iter().map(|qa| (qa.question, qa.answer)).collect(), cx);
            }
            (tr::AnswerResponse::Plan { approve: true, .. }, Some(trek_agents::Prompt::Plan(_))) => self.approve_plan(&id, &req.request_id, cx),
            (tr::AnswerResponse::Plan { approve: false, feedback }, Some(trek_agents::Prompt::Plan(_))) => {
                self.respond(&id, &req.request_id, trek_agents::Decision::Deny, cx);
                // Feedback is a message like any other (`/new` in it opens nothing: it was an answer).
                let thread = self.thread(&id).cloned();
                if let (Some(feedback), Some(t)) = (feedback.filter(|f| !f.trim().is_empty()), thread)
                    && let Ok(commands::Phone::Send(text)) = self.phone_command(&t, &feedback)
                {
                    self.send_to(&id, text, vec![], cx);
                }
            }
            _ => return Err(tr::HostError::bad_request("That answer doesn't fit the request")),
        }
        Ok(())
    }

    /// Answer `reply` with `f` once thread `id` is read in (from the agent's files or the
    /// database), as opening it on the Mac would.
    fn when_loaded<T: 'static>(
        &mut self,
        id: String,
        reply: tr::Reply<T>,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) -> tr::HostResult<T> + 'static,
    ) {
        if self.thread(&id).is_none() {
            let _ = reply.send(Err(tr::HostError::not_found("No such thread")));
            return;
        }
        if self.live.get(&id).is_some_and(|l| l.loaded && !l.loading) {
            let _ = reply.send(f(self, cx));
            return;
        }
        self.ensure_loaded(&id, cx);
        cx.spawn(async move |this, cx| {
            for _ in 0..200 {
                let loaded = this.read_with(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.loaded && !l.loading)).unwrap_or(true);
                if loaded {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(25)).await;
            }
            let answer = this.update(cx, |ws, cx| f(ws, cx)).unwrap_or_else(|_| Err(tr::HostError::other("Trek is closing")));
            let _ = reply.send(answer);
        })
        .detach();
    }

    /// Undo, retry, fork or rewind, as the turn footer's and message's buttons do on the Mac
    /// (after its confirmation, files restored when asked and Trek has them), without moving
    /// the Mac's screen.
    fn remote_turn_action(&mut self, req: tr::TurnActionRequest, cx: &mut Context<Self>) -> tr::HostResult<tr::TurnActionDone> {
        let id = req.thread_id.as_str();
        let Some(live) = self.live.get(id) else { return Err(tr::HostError::not_found("No such thread")) };
        let ix = req.item_id.strip_prefix('i').and_then(|n| n.parse::<usize>().ok()).filter(|ix| *ix < live.items.len());
        let Some(ix) = ix else { return Err(tr::HostError::not_found("No such item")) };
        let item = live.items[ix].clone();
        let key = live.items.id_at(ix).unwrap_or_default().to_string();
        let message = matches!(item, Item::User { aside: false, .. });
        let ends_turn = trek_core::rewind::ends_turn(&item);
        let refuse_busy = |what: &str| tr::HostError::bad_request(format!("Stop the running turn to {what}"));
        let busy = self.turn_running(id);
        // Restored as the confirmation does by default: when there's a checkpoint to go back to.
        let restore = |ws: &Self, message: &str| req.restore_files && ws.restorable_checkpoint(id, message).is_some();
        match req.action {
            tr::TurnAction::Undo | tr::TurnAction::Retry => {
                let what = if req.action == tr::TurnAction::Undo { "undo" } else { "retry" };
                if !ends_turn {
                    return Err(tr::HostError::bad_request("That item doesn't end a turn"));
                }
                if busy {
                    return Err(refuse_busy(what));
                }
                let Some(start) = self.turn_start_item(id, &key) else {
                    return Err(tr::HostError::bad_request("This turn didn't start from a message of yours"));
                };
                let restore = restore(self, &start);
                if req.action == tr::TurnAction::Undo {
                    let (text, _) = self.undo_turn(id, &key, restore, cx).ok_or_else(|| tr::HostError::other("The turn couldn't be undone"))?;
                    return Ok(tr::TurnActionDone { thread_id: None, text: Some(text) });
                }
                if let Some(model) = &req.model
                    && let Some(agent) = self.thread(id).map(|t| t.agent.clone())
                {
                    let models = self.models_for(&agent);
                    if !models.is_empty() && !models.iter().any(|m| crate::composer::same_model(model, &m.id)) {
                        return Err(tr::HostError::not_found(format!("{model} isn't one of {}'s models", agent.display_name())));
                    }
                }
                self.retry(id, &key, req.model.clone(), restore, cx);
                Ok(tr::TurnActionDone::default())
            }
            tr::TurnAction::Fork => {
                let at = if message {
                    ForkAt::Before(key)
                } else if ends_turn {
                    ForkAt::After(key)
                } else {
                    return Err(tr::HostError::bad_request("Fork from one of your messages or the end of a turn"));
                };
                let (fork, message) = self.fork_quietly(id, &at, cx).ok_or_else(|| tr::HostError::other("Couldn't fork the thread"))?;
                Ok(tr::TurnActionDone { thread_id: Some(fork), text: message.map(|(text, _)| text) })
            }
            tr::TurnAction::Rewind => {
                if !message {
                    return Err(tr::HostError::bad_request("Only your own messages can be rewound to"));
                }
                if busy {
                    return Err(refuse_busy("rewind"));
                }
                let restore = restore(self, &key);
                let (text, _) = self.rewind(id, &key, restore, cx).ok_or_else(|| tr::HostError::other("The thread couldn't be rewound"))?;
                Ok(tr::TurnActionDone { thread_id: None, text: Some(text) })
            }
        }
    }

    /// No phone has `id` open any more: it's no longer followed each tick.
    fn remote_unwatch(&mut self, id: &str) {
        let Some(remote) = self.remote.as_mut() else { return };
        if let Some(w) = remote.watched.remove(id) {
            remote.dormant.insert(id.to_string(), w);
        }
    }

    /// A turn of `id` was counted (or may have moved): phones following it hear on the next tick.
    pub(crate) fn remote_turn_changes_moved(&mut self, id: &str) {
        if let Some(remote) = self.remote.as_mut()
            && let Some(w) = remote.watched.get_mut(id).or_else(|| remote.dormant.get_mut(id))
        {
            w.recount = true;
        }
    }

    // ---- what phones see ----

    fn remote_project(&self, t: &Thread) -> tr::ProjectRef {
        match t.project_id.as_deref().and_then(|p| self.project(p)) {
            Some(p) => {
                let hue = tr::project_hue(&p.name, self.project_prefs(&p.path).color);
                tr::ProjectRef { id: p.id.clone(), name: p.name.clone(), hue, monogram: tr::monogram(&p.name) }
            }
            None => tr::ProjectRef { id: String::new(), name: "No project".into(), hue: 220, monogram: "··".into() },
        }
    }

    /// A thread's row on the phone.
    fn remote_summary(&self, t: &Thread, now: i64) -> tr::ThreadSummary {
        let live = self.live.get(&t.id);
        let needs = live.and_then(|l| l.permissions.first()).map(|p| match &p.prompt {
            Some(trek_agents::Prompt::Questions(q)) => tr::Needs { kind: tr::NeedsKind::Question, text: q.first().map(|q| q.question.clone()).unwrap_or_default() },
            Some(trek_agents::Prompt::Plan(_)) => tr::Needs { kind: tr::NeedsKind::Plan, text: "Plan ready to review".into() },
            None => tr::Needs { kind: tr::NeedsKind::Approval, text: format!("{} {}", p.title, first_line(&p.detail)).trim().to_string() },
        });
        let needs = needs
            .or_else(|| t.paused.as_ref().filter(|_| t.run_state == RunState::Idle).map(|p| tr::Needs { kind: tr::NeedsKind::Limit, text: if p.wrapped { "Wrapped up before its usage limit" } else { "Paused at its usage limit" }.into() }))
            .or_else(|| {
                (t.run_state == RunState::Failed).then(|| {
                    let last_error = live.and_then(|l| l.items.iter().rev().find_map(|i| match i {
                        Item::Error { text } => Some(first_line(text)),
                        _ => None,
                    }));
                    tr::Needs { kind: tr::NeedsKind::Failed, text: last_error.unwrap_or_else(|| "The turn failed".into()) }
                })
            });
        // (A snoozed thread that needs you is in the inbox already: it raised its hand.)
        let section = match t.section(now).unwrap_or(Section::Settled) {
            Section::Pinned => tr::Section::Pinned,
            Section::Inbox => tr::Section::Inbox,
            Section::Working => tr::Section::Working,
            Section::Snoozed => tr::Section::Snoozed,
            Section::Settled => tr::Section::Settled,
        };
        let activity = live.filter(|_| t.run_state == RunState::Working).and_then(|l| {
            l.items.iter().rev().find_map(|i| match i {
                Item::Tool { title, detail, status: ToolStatus::Running, .. } => Some(crate::activity::op(title, detail, t.cwd.as_deref())).map(|op| format!("{} {}", op.verb, op.text).trim().to_string()),
                _ => None,
            })
        });
        let working_since = live.and_then(|l| l.turn_started).map(instant_ms);
        // The composer's context ring and cost line, once the thread has been opened (here or on
        // the phone): what its session reported.
        let context = live.and_then(|l| l.context).filter(|(_, window)| *window > 0).map(|(used, window)| tr::ContextUse {
            used,
            window,
            percent: ((used as f64 / window as f64) * 100.).round().clamp(0., 100.) as u8,
        });
        let cost = live.and_then(|l| l.spend.as_ref()).and_then(|spend| {
            let billing = self.billing_of(t);
            Some(tr::Cost {
                label: crate::cost::label(billing.as_ref(), spend)?,
                detail: crate::cost::billing_note(billing.as_ref()),
                plan: match &billing {
                    Some(trek_agents::Billing::Plan(plan)) => plan.clone(),
                    _ => None,
                },
                billing: billing.map(|b| match b {
                    trek_agents::Billing::Plan(_) => tr::Billing::Plan,
                    trek_agents::Billing::Metered => tr::Billing::Metered,
                    trek_agents::Billing::Local => tr::Billing::Local,
                }),
            })
        });
        let (sub_agents, background) = self.remote_at_work(t);
        let git = t.cwd.as_ref().and_then(|c| self.git_info.get(c)).filter(|g| g.is_repo).map(|g| tr::GitSummary {
            changed: g.changed as u32,
            ahead: g.ahead,
            behind: g.behind,
            default_branch: g.default_branch.clone(),
        });
        tr::ThreadSummary {
            id: t.id.clone(),
            title: t.title.clone(),
            project: self.remote_project(t),
            agent: agent_ref(&t.agent),
            model: t.model.clone(),
            // The default's name too ("Opus 5.5"), as the composer's model pill shows it.
            model_label: Some(self.model_label(t)),
            run_state: match t.run_state {
                RunState::Idle => tr::RunState::Idle,
                RunState::Working => tr::RunState::Working,
                RunState::NeedsYou => tr::RunState::NeedsYou,
                RunState::Failed => tr::RunState::Failed,
            },
            needs,
            section,
            unseen: t.is_unseen(),
            pinned: t.pinned_at.is_some(),
            branch: t.worktree.as_ref().map(|w| w.branch.clone()).or_else(|| t.cwd.as_ref().and_then(|c| self.git_info.get(c)).and_then(|g| g.branch.clone())),
            worktree: t.worktree.is_some(),
            activity,
            working_since,
            updated_at: t.updated_at,
            additions: t.additions.max(0) as u32,
            deletions: t.deletions.max(0) as u32,
            effort: Some(t.effort.as_str().to_string()),
            access: Some(match t.hand_holding {
                trek_core::HandHolding::Supervised => tr::Access::Supervised,
                trek_core::HandHolding::AutoAcceptEdits => tr::Access::AutoAcceptEdits,
                trek_core::HandHolding::Auto => tr::Access::Auto,
                trek_core::HandHolding::FullAccess => tr::Access::FullAccess,
            }),
            plan: live.is_some_and(|l| l.plan),
            effort_label: Some(t.effort.label().to_string()),
            context,
            cost,
            sub_agents,
            background,
            base: t.worktree.as_ref().map(|w| w.base.clone()),
            git,
        }
    }

    /// What `t` has at work, as its sidebar card and its tooltip say: its sub-agents (Trek's, and
    /// its agent's own, in its turn or in the background), then what else its agent runs in the
    /// background, by title.
    fn remote_at_work(&self, t: &Thread) -> (Vec<tr::SubAgent>, Vec<String>) {
        use crate::workspace::TaskState;
        let mut kids: Vec<tr::SubAgent> = self
            .running_children(&t.id)
            .into_iter()
            .map(|c| tr::SubAgent {
                agent: agent_ref(&c.agent),
                model: Some(self.model_label(c)),
                title: c.title.clone(),
                state: match self.task_state(&c.id) {
                    TaskState::Running => tr::SubAgentState::Running,
                    TaskState::NeedsYou => tr::SubAgentState::NeedsYou,
                    TaskState::Done => tr::SubAgentState::Done,
                    TaskState::Failed => tr::SubAgentState::Failed,
                    TaskState::Cancelled => tr::SubAgentState::Stopped,
                },
                // Rounded: what it's worked for is time paused at a limit aside, so this can
                // drift by the odd millisecond between ticks.
                since: Some((now_ms() - self.task_elapsed(&c.id).as_millis() as i64) / 1000 * 1000),
            })
            .collect();
        let Some(l) = self.live.get(&t.id) else { return (kids, vec![]) };
        let own = |title: &str, started: std::time::Instant| tr::SubAgent {
            agent: agent_ref(&t.agent),
            model: None,
            title: title.to_string(),
            state: tr::SubAgentState::Running,
            since: Some(instant_ms(started)),
        };
        let out = |id: &str| l.tasks.iter().any(|k| k.id == id && k.done.is_none());
        kids.extend(l.tasks.iter().filter(|k| k.done.is_none()).map(|k| own(&k.description, k.started)));
        kids.extend(l.background_agents().filter(|b| !b.task.call.as_deref().is_some_and(out)).map(|b| own(&b.task.title, b.started)));
        (kids, l.background_work().map(|b| b.task.title.clone()).collect())
    }

    /// The threads a phone lists: the sidebar's, without side chats and sub-agents, and only the
    /// newest settled ones.
    fn remote_threads(&self) -> Vec<&Thread> {
        let now = now_ms();
        let mut settled = 0;
        let mut out = vec![];
        let mut threads: Vec<&Thread> = self.threads.iter().filter(|t| t.section(now).is_some() && t.import_hidden.is_none()).collect();
        threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        for t in threads {
            if t.section(now) == Some(Section::Settled) {
                settled += 1;
                if settled > SETTLED_ON_PHONE {
                    continue;
                }
            }
            out.push(t);
        }
        out
    }

    fn remote_snapshot(&self) -> tr::Snapshot {
        let now = now_ms();
        let threads = self.remote_threads().into_iter().map(|t| self.remote_summary(t, now)).collect();
        let projects = self
            .workspace_projects()
            .into_iter()
            .map(|p| tr::ProjectSummary {
                id: p.id.clone(),
                name: p.name.clone(),
                hue: tr::project_hue(&p.name, self.project_prefs(&p.path).color),
                monogram: tr::monogram(&p.name),
                branch: self.git_info.get(&p.path).and_then(|g| g.branch.clone()),
                is_repo: p.is_repo,
            })
            .collect();
        // What the composer's model picker offers: signed-in agents, API keys, local models.
        let agents = self
            .ready_agents()
            .into_iter()
            .map(|agent| {
                let infos = self.models_for(&agent);
                // The one the model menu starts on (Opus 5.5 for Claude Code, else the first).
                let default_model = crate::composer::default_model(&infos).map(|m| m.id.clone());
                let models = infos
                    .into_iter()
                    .map(|m| tr::ModelOption { id: m.id, label: m.name, efforts: m.efforts.iter().map(|e| e.as_str().to_string()).collect() })
                    .collect();
                tr::AgentOption { key: agent.key(), name: agent.display_name(), logo: crate::ui::logo_key(&agent).map(str::to_string), default_model, models }
            })
            .collect();
        tr::Snapshot { threads, projects, agents, full_access: self.settings.permissions.full_access_unlocked }
    }

    /// Bring `w` up to date with `id`'s transcript, looking only at what can have changed since
    /// it last was (`Transcript::take_edited_from`, the item streaming then and now): the events
    /// phones following it need, in order. Turns' changed files are looked at again when the
    /// transcript moved or a count came in (`remote_turn_changes_moved`).
    fn sync_watched(&mut self, id: &str, w: &mut Watched, cx: &mut Context<Self>) -> Vec<tr::HostEvent> {
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        let Some(live) = self.live.get_mut(id) else { return vec![] };
        let edited = live.items.take_edited_from();
        let (len, revision, streaming) = (live.items.len(), live.revision, live.streaming);
        let moved = revision != w.revision;
        // Requests come and go with a revision bump, mostly: looked at whenever they differ too.
        let asks = moved || live.permissions.len() != w.requests.len() || live.permissions.iter().zip(&w.requests).any(|(p, (r, ..))| p.request_id != *r);
        if edited.is_none() && !asks && streaming == w.streaming && cwd == w.cwd && !w.recount {
            return vec![];
        }
        let mut out = vec![];
        // Shorter than sent (a rewind, a rewrite): the phone reads it again.
        if len < w.items.len() {
            w.restart(len);
            (w.revision, w.streaming, w.cwd) = (revision, streaming, cwd);
            out.push(tr::HostEvent::TranscriptReset(id.to_string()));
            return out;
        }
        // Nothing before the first edit changed, nor anything but the item streaming now or
        // before (its flag), unless the folder its paths are shown in did.
        let mut floor = edited.unwrap_or(len).min(w.items.len());
        for s in [w.streaming, streaming].into_iter().flatten() {
            floor = floor.min(s);
        }
        if cwd != w.cwd {
            floor = 0;
        }
        let unsent = Sent { seq: w.seq, hash: None };
        w.items.resize(len, unsent);
        let live = &self.live[id];
        for ix in floor.max(w.from)..len {
            let body = item_body(live, cwd.as_deref(), ix);
            let h = hash(&body);
            if w.items[ix].hash == Some(h) {
                continue;
            }
            w.seq += 1;
            w.items[ix] = Sent { seq: w.seq, hash: Some(h) };
            out.push(item_event(id, format!("i{ix}"), w.seq, item_time(&live.items[ix]), body));
        }
        if asks {
            // Requests: new or changed ones as they are, answered ones as resolved.
            let requests = request_cards(live);
            let open: HashSet<&str> = requests.iter().map(|(r, _)| r.as_str()).collect();
            for (rid, last, _) in w.requests.iter().filter(|(r, ..)| !open.contains(r.as_str())) {
                w.seq += 1;
                let answer = self.remote.as_mut().and_then(|r| r.answered.remove(&(id.to_string(), rid.clone())));
                out.push(item_event(id, format!("r{rid}"), w.seq, None, resolved(last.clone(), answer)));
            }
            w.requests.retain(|(r, ..)| open.contains(r.as_str()));
            for (rid, body) in requests {
                if w.requests.iter().any(|(r, x, _)| *r == rid && *x == body) {
                    continue;
                }
                w.seq += 1;
                w.requests.retain(|(r, ..)| *r != rid);
                w.requests.push((rid.clone(), body.clone(), w.seq));
                out.push(item_event(id, format!("r{rid}"), w.seq, None, body));
            }
        }
        // Turns' changed files, new or changed (after the items they follow, so a turn end
        // reaches the phone before its files do): the turns that just ended, or all of them
        // when a count came in.
        let recount = std::mem::take(&mut w.recount);
        if recount || moved {
            let first = if recount { w.from } else { floor.max(w.from) };
            let counted = self.sync_changes(id, w, first..len, cx);
            let live = &self.live[id];
            out.extend(counted.into_iter().map(|(end, seq, body)| item_event(id, format!("c{end}"), seq, item_time(&live.items[end]), body)));
        }
        (w.revision, w.streaming, w.cwd) = (revision, streaming, cwd);
        out
    }

    /// Have the turns ending in `range` counted (off the main thread, for those counted from
    /// git), and record (and return, by turn end, with their seq) the counts that are in and
    /// changed.
    fn sync_changes(&mut self, id: &str, w: &mut Watched, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<(usize, u64, tr::ItemBody)> {
        let Some(live) = self.live.get(id) else { return vec![] };
        let range = range.start..range.end.min(live.items.len());
        let ends: Vec<usize> = range.filter(|ix| matches!(live.items[*ix], Item::TurnEnd { .. })).collect();
        for &end in &ends {
            self.load_turn_changes(id, end, cx);
        }
        let mut out = vec![];
        for end in ends {
            if self.turn_changes_waiting(id, end) {
                w.recount = true;
            }
            let Some(body) = turn_changes(self, id, end) else { continue };
            let h = hash(&body);
            if w.changes.get(&end).and_then(|s| s.hash) == Some(h) {
                continue;
            }
            w.seq += 1;
            w.changes.insert(end, Sent { seq: w.seq, hash: Some(h) });
            out.push((end, w.seq, body));
        }
        out
    }

    /// Phones may hold `id`'s items from `start` on: they're followed from there (their turns'
    /// changed files too). What it converted on the way, to be sent now.
    fn send_from(&mut self, id: &str, w: &mut Watched, start: usize, cx: &mut Context<Self>) -> Converted {
        let mut out = Converted::default();
        if start >= w.from {
            return out;
        }
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        let Some(live) = self.live.get(id) else { return out };
        let old = w.from.min(live.items.len()).min(w.items.len());
        for ix in start..old {
            let body = item_body(live, cwd.as_deref(), ix);
            w.items[ix].hash = Some(hash(&body));
            out.items.insert(ix, body);
        }
        w.from = start;
        // What's counted now goes out with them; what comes in later, on a tick.
        out.changes = self.sync_changes(id, w, start..old, cx).into_iter().map(|(end, _, body)| (end, body)).collect();
        out
    }

    /// The reply to a phone's `subscribe`: what changed since `after_seq` when that can be told,
    /// else the last `limit` items (all of them without one), with what follows them, and every
    /// open request. From then on the thread is followed every tick.
    pub(crate) fn remote_transcript(&mut self, id: &str, after_seq: Option<u64>, limit: Option<u32>, cx: &mut Context<Self>) -> tr::Transcript {
        let Some(remote) = self.remote.as_mut() else { return tr::Transcript::default() };
        // A phone opening the thread again carries on where it left off.
        let known = remote.watched.remove(id).or_else(|| remote.dormant.remove(id));
        let mut w = match known {
            Some(mut w) => {
                let events = self.sync_watched(id, &mut w, cx);
                if let Some(remote) = self.remote.as_ref() {
                    for event in events {
                        remote.handle.push(event);
                    }
                }
                w
            }
            None => {
                let cwd = self.thread(id).and_then(|t| t.cwd.clone());
                let Some(live) = self.live.get_mut(id) else { return tr::Transcript::default() };
                let _ = live.items.take_edited_from();
                let mut w = Watched::fresh(live.items.len());
                (w.revision, w.streaming, w.cwd) = (live.revision, live.streaming, cwd);
                for (rid, body) in request_cards(live) {
                    w.seq += 1;
                    w.requests.push((rid, body, w.seq));
                }
                w
            }
        };
        let len = w.items.len();
        let start = limit.map_or(0, |l| len.saturating_sub(l as usize));
        let mut converted = self.send_from(id, &mut w, start, cx);
        let delta = after_seq.filter(|a| w.base <= *a && *a <= w.seq);
        let newer = |seq: u64| delta.is_none_or(|after| seq > after);
        let first = if delta.is_some() { w.from } else { start };
        let mut items = vec![];
        if let Some(live) = self.live.get(id) {
            let cwd = self.thread(id).and_then(|t| t.cwd.clone());
            for ix in first..len.min(live.items.len()) {
                let at = item_time(&live.items[ix]);
                if newer(w.items[ix].seq) {
                    let body = converted.items.remove(&ix).unwrap_or_else(|| item_body(live, cwd.as_deref(), ix));
                    items.push(tr::Item { id: format!("i{ix}"), seq: w.items[ix].seq, at, body });
                }
                if let Some(c) = w.changes.get(&ix).filter(|c| newer(c.seq))
                    && let Some(body) = converted.changes.remove(&ix).or_else(|| turn_changes(self, id, ix))
                {
                    items.push(tr::Item { id: format!("c{ix}"), seq: c.seq, at, body });
                }
            }
        }
        for (rid, body, seq) in &w.requests {
            if newer(*seq) {
                items.push(tr::Item { id: format!("r{rid}"), seq: *seq, at: None, body: body.clone() });
            }
        }
        let out = tr::Transcript { seq: w.seq, items, base: w.base, more: delta.is_none() && start > 0 };
        if let Some(remote) = self.remote.as_mut() {
            remote.watched.insert(id.to_string(), w);
        }
        out
    }

    /// The reply to `transcript_before`: up to `limit` items just before item `before` (`i…`,
    /// or a turn's `c…`, which follows its turn end), with their turns' changed files.
    pub(crate) fn remote_transcript_before(&mut self, id: &str, before: &str, limit: u32, cx: &mut Context<Self>) -> tr::HostResult<tr::TranscriptPage> {
        let len = self.live.get(id).map_or(0, |l| l.items.len());
        let index = |n: &str| n.parse::<usize>().ok().filter(|n| *n < len);
        let end = match before.split_at_checked(1) {
            Some(("i", n)) => index(n),
            Some(("c", n)) => index(n).map(|n| n + 1),
            _ => None,
        };
        let Some(end) = end else { return Err(tr::HostError::not_found("No such item")) };
        let start = end.saturating_sub(limit.min(tr::MAX_PAGE) as usize);
        let mut w = self.remote.as_mut().and_then(|r| r.watched.remove(id));
        let mut converted = Converted::default();
        if let Some(w) = w.as_mut() {
            let events = self.sync_watched(id, w, cx);
            if let Some(remote) = self.remote.as_ref() {
                for event in events {
                    remote.handle.push(event);
                }
            }
            converted = self.send_from(id, w, start, cx);
        }
        let mut items = vec![];
        if let Some(live) = self.live.get(id) {
            let cwd = self.thread(id).and_then(|t| t.cwd.clone());
            // A thread no phone follows (it wasn't subscribed to) pages with no seqs.
            let seq = |ix: usize| w.as_ref().and_then(|w| w.items.get(ix)).map_or(0, |s| s.seq);
            for ix in start..end.min(live.items.len()) {
                let at = item_time(&live.items[ix]);
                let body = converted.items.remove(&ix).unwrap_or_else(|| item_body(live, cwd.as_deref(), ix));
                items.push(tr::Item { id: format!("i{ix}"), seq: seq(ix), at, body });
                // A turn's changed files follow its end (not the files asked to come before).
                if !matches!(live.items[ix], Item::TurnEnd { .. }) || (ix + 1 == end && before.starts_with('c')) {
                    continue;
                }
                if let Some(body) = converted.changes.remove(&ix).or_else(|| turn_changes(self, id, ix)) {
                    let seq = w.as_ref().and_then(|w| w.changes.get(&ix)).map_or(0, |c| c.seq);
                    items.push(tr::Item { id: format!("c{ix}"), seq, at, body });
                }
            }
        }
        if let (Some(w), Some(remote)) = (w, self.remote.as_mut()) {
            remote.watched.insert(id.to_string(), w);
        }
        Ok(tr::TranscriptPage { items, more: start > 0 })
    }

    /// Send phones what changed since the last tick: thread rows, and the transcripts they
    /// follow (only what can have changed in them).
    pub(crate) fn push_remote_changes(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.as_ref() else { return };
        if remote.connected.is_empty() {
            return;
        }
        let now = now_ms();
        let rows: Vec<tr::ThreadSummary> = self.remote_threads().into_iter().map(|t| self.remote_summary(t, now)).collect();
        let Some(remote) = self.remote.as_mut() else { return };
        // Thread rows.
        let ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let gone: Vec<String> = remote.sent.keys().filter(|id| !ids.contains(id.as_str())).cloned().collect();
        for id in gone {
            remote.sent.remove(&id);
            remote.watched.remove(&id);
            remote.dormant.remove(&id);
            remote.handle.push(tr::HostEvent::ThreadRemoved(id));
        }
        for row in rows {
            if remote.sent.get(&row.id) != Some(&row) {
                remote.sent.insert(row.id.clone(), row.clone());
                remote.handle.push(tr::HostEvent::Thread(row));
            }
        }
        // Open transcripts.
        let watched: Vec<String> = remote.watched.keys().cloned().collect();
        for id in watched {
            let Some(mut w) = self.remote.as_mut().and_then(|r| r.watched.remove(&id)) else { continue };
            let events = self.sync_watched(&id, &mut w, cx);
            let Some(remote) = self.remote.as_mut() else { return };
            for event in events {
                remote.handle.push(event);
            }
            remote.watched.insert(id, w);
        }
    }

    pub(crate) fn remote_notice(&mut self, notice: tr::ServerNotice, cx: &mut Context<Self>) {
        let Some(remote) = self.remote.as_mut() else { return };
        remote.devices = remote.handle.devices();
        match notice {
            tr::ServerNotice::Paired { name, .. } => {
                remote.offer = None;
                cx.emit(WorkspaceEvent::Toast { message: format!("{name} is paired with Trek"), undo: None });
            }
            tr::ServerNotice::Connected { device_id } => {
                remote.connected.insert(device_id);
            }
            tr::ServerNotice::Disconnected { device_id } => {
                remote.connected.remove(&device_id);
            }
        }
        cx.notify();
    }
}

/// Items and turns' changed files converted for the phone, by index.
#[derive(Default)]
struct Converted {
    items: HashMap<usize, tr::ItemBody>,
    changes: HashMap<usize, tr::ItemBody>,
}

const MAX_PHONE_ITEM_TEXT: usize = (2 << 20) - (16 << 10);

fn clip_text(text: &str) -> String {
    clip_text_to(text, MAX_PHONE_ITEM_TEXT)
}

/// Keep a string within its JSON budget: escaping quotes, slashes and control characters counts.
fn clip_text_to(text: &str, limit: usize) -> String {
    let encoded = |c: char| match c {
        '"' | '\\' | '\u{8}' | '\t' | '\n' | '\u{c}' | '\r' => 2,
        '\0'..='\u{1f}' => 6,
        _ => c.len_utf8(),
    };
    let encoded_len = |value: &str| value.chars().map(&encoded).sum::<usize>();
    if encoded_len(text) + 2 <= limit {
        return text.to_string();
    }
    let mut omitted = text.len();
    loop {
        let marker = format!("\n\n[{omitted} bytes omitted]\n\n");
        let available = limit.saturating_sub(encoded_len(&marker) + 2);
        let (mut used, mut head) = (0, 0);
        for (at, c) in text.char_indices() {
            let len = encoded(c);
            if used + len > available / 2 {
                break;
            }
            used += len;
            head = at + c.len_utf8();
        }
        let mut tail = text.len();
        for (at, c) in text.char_indices().rev() {
            let len = encoded(c);
            if used + len > available || at < head {
                break;
            }
            used += len;
            tail = at;
        }
        let actual = tail - head;
        if actual == omitted {
            return format!("{}{}{}", &text[..head], marker, &text[tail..]);
        }
        omitted = actual;
    }
}

/// Item `ix` of `live` as the phone shows it (`cwd`: its thread's folder).
fn item_body(live: &LiveThread, cwd: Option<&std::path::Path>, ix: usize) -> tr::ItemBody {
    match &live.items[ix] {
        Item::User { text, images, .. } => tr::ItemBody::User { text: clip_text(text), images: images.len() as u32 },
        Item::Assistant { text } => tr::ItemBody::Assistant { text: clip_text(text), streaming: live.streaming == Some(ix) },
        Item::Reasoning { text } => tr::ItemBody::Reasoning { text: clip_text(text) },
        Item::Tool { id: call, title, detail, output, status } => {
            let op = crate::activity::op(title, detail, cwd);
            let lines = live.lines.get(call).copied();
            tr::ItemBody::Tool {
                call_id: call.clone(),
                tool: tr::ToolKind::from_title(title),
                title: clip_text_to(if op.verb.is_empty() { title } else { &op.verb }, 64 << 10),
                detail: clip_text_to(if op.text.is_empty() { detail } else { &op.text }, 256 << 10),
                status: match status {
                    ToolStatus::Running => tr::ToolStatus::Running,
                    ToolStatus::Done => tr::ToolStatus::Done,
                    ToolStatus::Failed => tr::ToolStatus::Failed,
                    ToolStatus::Denied => tr::ToolStatus::Denied,
                },
                output: tr::truncate_output(output).to_string(),
                added: lines.map(|l| l.0),
                removed: lines.map(|l| l.1),
            }
        }
        Item::TurnEnd { took_secs, .. } => tr::ItemBody::TurnEnd { took_secs: *took_secs },
        Item::Notice { text } => tr::ItemBody::Notice { text: clip_text(text) },
        Item::Error { text } => tr::ItemBody::Error { text: clip_text(text) },
        Item::Limit { text, resets_at, .. } => tr::ItemBody::Limit { text: clip_text(text), resets_at: *resets_at },
        Item::Handoff { from, to, from_name, to_name, .. } => tr::ItemBody::Handoff {
            from: from_name.clone().unwrap_or_else(|| AgentId::from_key(from).display_name()),
            to: to_name.clone().unwrap_or_else(|| AgentId::from_key(to).display_name()),
        },
    }
}

/// The requests open in `live`, as cards, by request id.
fn request_cards(live: &LiveThread) -> Vec<(String, tr::ItemBody)> {
    live.permissions
        .iter()
        .map(|p| {
            let body = match &p.prompt {
                None => tr::ItemBody::Approval {
                    request_id: p.request_id.clone(),
                    title: clip_text_to(&p.title, 64 << 10),
                    detail: clip_text_to(&p.detail, MAX_PHONE_ITEM_TEXT - (64 << 10)),
                    state: tr::ApprovalState::Pending,
                },
                Some(trek_agents::Prompt::Questions(qs)) => tr::ItemBody::Question {
                    request_id: p.request_id.clone(),
                    questions: phone_questions(qs),
                    state: tr::QuestionState::Pending,
                    answers: None,
                },
                Some(trek_agents::Prompt::Plan(markdown)) => tr::ItemBody::Plan { request_id: p.request_id.clone(), markdown: clip_text(markdown), state: tr::PlanState::Pending },
            };
            (p.request_id.clone(), body)
        })
        .collect()
}

fn phone_questions(questions: &[trek_agents::Question]) -> Vec<tr::Question> {
    let fields = questions.iter().map(|q| 2 + q.options.len() * 2).sum::<usize>().max(1);
    // A resolved card also carries its answers; leave half of the body's budget for them.
    let each = (MAX_PHONE_ITEM_TEXT / 2) / fields;
    questions
        .iter()
        .map(|q| tr::Question {
            header: clip_text_to(&q.header, each),
            question: clip_text_to(&q.question, each),
            options: q.options.iter().map(|(label, description)| tr::QuestionOption { label: clip_text_to(label, each), description: clip_text_to(description, each) }).collect(),
            multi: q.multi,
            secret: q.secret,
        })
        .collect()
}

fn phone_answers(answers: Vec<tr::QA>) -> Vec<tr::QA> {
    let each = (MAX_PHONE_ITEM_TEXT / 2) / (answers.len() * 2).max(1);
    answers.into_iter().map(|qa| tr::QA { question: clip_text_to(&qa.question, each), answer: clip_text_to(&qa.answer, each) }).collect()
}

/// Questions and option labels are identities in the phone's answer. Put clipped ones back
/// before the workspace records secrets or hands the answer to the agent.
fn restore_phone_answers(answers: &mut [tr::QA], questions: &[trek_agents::Question]) {
    let shown = phone_questions(questions);
    for (answer_ix, answer) in answers.iter_mut().enumerate() {
        let question_ix = shown
            .get(answer_ix)
            .filter(|q| q.question == answer.question)
            .map(|_| answer_ix)
            .or_else(|| shown.iter().position(|q| q.question == answer.question));
        let Some(question_ix) = question_ix else { continue };
        let (shown, original) = (&shown[question_ix], &questions[question_ix]);
        let restore = |part: &str| shown.options.iter().position(|o| o.label == part).and_then(|ix| original.options.get(ix)).map(|o| o.0.as_str());
        if let Some(label) = restore(&answer.answer) {
            answer.answer = label.to_string();
        } else {
            let parts: Vec<&str> = answer.answer.split(", ").collect();
            if let Some(labels) = parts.iter().map(|part| restore(part)).collect::<Option<Vec<_>>>() {
                answer.answer = labels.join(", ");
            }
        }
        answer.question = original.question.clone();
    }
}

/// An item upsert for the phones following thread `id`.
fn item_event(id: &str, item_id: String, seq: u64, at: Option<i64>, body: tr::ItemBody) -> tr::HostEvent {
    tr::HostEvent::Item { thread_id: id.to_string(), item: tr::Item { id: item_id, seq, at, body } }
}

/// What the turn ending at `turn_end` changed, as a `changes` item, once the Mac has counted it.
fn turn_changes(ws: &Workspace, id: &str, turn_end: usize) -> Option<tr::ItemBody> {
    #[cfg(test)]
    if let Some(files) = tests::TURN_FILES.with(|f| f.borrow().get(&(id.to_string(), turn_end)).cloned()) {
        return Some(changes_item(files));
    }
    let changes = ws.turn_changes(id, turn_end)?;
    Some(changes_item(
        changes
            .files
            .into_iter()
            .map(|f| {
                let (status, from) = match f.status {
                    trek_core::changes::FileStatus::Added => (tr::FileStatus::Added, None),
                    trek_core::changes::FileStatus::Modified => (tr::FileStatus::Modified, None),
                    trek_core::changes::FileStatus::Deleted => (tr::FileStatus::Deleted, None),
                    trek_core::changes::FileStatus::Renamed { from } => (tr::FileStatus::Renamed, Some(from)),
                };
                tr::ChangedFile { path: f.path, status, from, added: f.added, removed: f.removed, binary: f.binary }
            })
            .collect(),
    ))
}

/// A `changes` item for `files`, with their totals.
fn changes_item(files: Vec<tr::ChangedFile>) -> tr::ItemBody {
    let (added, removed) = files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    tr::ItemBody::Changes { files, added, removed }
}

/// An agent as phones see it: its key, name and the logo the Mac draws for it.
pub(crate) fn agent_ref(agent: &AgentId) -> tr::AgentRef {
    tr::AgentRef { key: agent.key(), name: agent.display_name(), logo: crate::ui::logo_key(agent).map(str::to_string) }
}

/// An instant as unix ms, the same each time it's asked (`now - elapsed` drifts by the time it
/// takes to ask, which would send an unchanged row again every tick).
fn instant_ms(at: std::time::Instant) -> i64 {
    static ANCHOR: std::sync::LazyLock<(std::time::Instant, i64)> = std::sync::LazyLock::new(|| (std::time::Instant::now(), now_ms()));
    let (then, ms) = *ANCHOR;
    match at.checked_duration_since(then) {
        Some(after) => ms + after.as_millis() as i64,
        None => ms - then.duration_since(at).as_millis() as i64,
    }
}

/// A request answered: its card as last sent, saying how a phone answered it, or just closed
/// when it was answered on the Mac (or the turn ended).
fn resolved(mut body: tr::ItemBody, answer: Option<tr::AnswerResponse>) -> tr::ItemBody {
    match (&mut body, answer) {
        (tr::ItemBody::Approval { state, .. }, Some(tr::AnswerResponse::Approval { decision })) => {
            *state = match decision {
                tr::Decision::Allow => tr::ApprovalState::Allowed,
                tr::Decision::AllowForSession => tr::ApprovalState::AllowedForSession,
                tr::Decision::Deny => tr::ApprovalState::Denied,
            }
        }
        (tr::ItemBody::Question { state, answers, .. }, Some(tr::AnswerResponse::Questions { answers: given })) => {
            *state = tr::QuestionState::Answered;
            *answers = Some(phone_answers(given));
        }
        (tr::ItemBody::Plan { state, .. }, Some(tr::AnswerResponse::Plan { approve, .. })) => {
            *state = if approve { tr::PlanState::Approved } else { tr::PlanState::Rejected }
        }
        (tr::ItemBody::Approval { state, .. }, _) => *state = tr::ApprovalState::Resolved,
        (tr::ItemBody::Question { state, .. }, _) => *state = tr::QuestionState::Resolved,
        (tr::ItemBody::Plan { state, .. }, _) => *state = tr::PlanState::Resolved,
        _ => {}
    }
    body
}

/// When each item happened, as far as the transcript says: a message's time, a turn's end.
fn item_time(item: &Item) -> Option<i64> {
    match item {
        Item::User { at, .. } => *at,
        Item::TurnEnd { at, .. } => Some(*at),
        _ => None,
    }
}

/// What a phone would read of `body`, hashed (its JSON, written straight into the hasher).
fn hash(body: &tr::ItemBody) -> u64 {
    struct Hashing(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for Hashing {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut h = Hashing(std::collections::hash_map::DefaultHasher::new());
    let _ = serde_json::to_writer(&mut h, body);
    h.0.finish()
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().chars().take(140).collect()
}

/// The access level a phone asked for, if this Mac allows it: Full access only once it's been
/// unlocked in the Mac's settings.
fn hand_holding(access: tr::Access, settings: &trek_core::settings::Settings) -> tr::HostResult<trek_core::HandHolding> {
    Ok(match access {
        tr::Access::Supervised => trek_core::HandHolding::Supervised,
        tr::Access::AutoAcceptEdits => trek_core::HandHolding::AutoAcceptEdits,
        tr::Access::Auto => trek_core::HandHolding::Auto,
        tr::Access::FullAccess if settings.permissions.full_access_unlocked => trek_core::HandHolding::FullAccess,
        tr::Access::FullAccess => return Err(tr::HostError::bad_request("Full access is locked on this Mac: unlock it in Trek's Permissions settings first.")),
    })
}

/// Photos from the phone, saved with the composer's snapshots for the agent to read.
fn save_uploads(images: &[tr::ImageUpload]) -> tr::HostResult<Vec<PathBuf>> {
    if images.is_empty() {
        return Ok(vec![]);
    }
    let dir = trek_core::paths::data_dir().join("snapshots");
    std::fs::create_dir_all(&dir).map_err(|e| tr::HostError::other(format!("Couldn't keep the photo: {e}")))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d at %H.%M.%S");
    images
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let (bytes, ext) = img.decode().map_err(|e| tr::HostError::bad_request(format!("That photo couldn't be read: {e}")))?;
            if bytes.len() > 10 << 20 {
                return Err(tr::HostError::bad_request("That photo is too big (10 MB at most)"));
            }
            let path = dir.join(format!("iPhone {stamp} {}.{ext}", i + 1));
            std::fs::write(&path, bytes).map_err(|e| tr::HostError::other(format!("Couldn't keep the photo: {e}")))?;
            Ok(path)
        })
        .collect()
}

/// One of Trek's own slash commands, which only the Mac runs.
pub fn is_trek_command(text: &str) -> bool {
    let Some(cmd) = text.trim_start().strip_prefix('/').and_then(|c| c.split_whitespace().next()) else { return false };
    let cmd = cmd.to_lowercase();
    crate::workspace::BUILTIN_COMMANDS.iter().any(|(name, _)| name.split_whitespace().next() == Some(cmd.as_str()))
        || matches!(cmd.as_str(), "clear" | "access" | "mode" | "permissions")
}

/// Answer phones' requests on the main thread, as they come.
fn serve_requests(ws: WeakEntity<Workspace>, requests: async_channel::Receiver<tr::HostRequest>, cx: &mut Context<Workspace>) -> Task<()> {
    cx.spawn(async move |_, cx: &mut AsyncApp| {
        while let Ok(req) = requests.recv().await {
            if ws.update(cx, |ws, cx| ws.remote_request(req, cx)).is_err() {
                break;
            }
        }
    })
}

/// Push what changed, every tick, while the server runs.
fn push_changes(ws: WeakEntity<Workspace>, cx: &mut Context<Workspace>) -> Task<()> {
    cx.spawn(async move |_, cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(TICK).await;
            let pushed = ws.update(cx, |ws, cx| ws.push_remote_changes(cx));
            if pushed.is_err() {
                break;
            }
        }
    })
}

/// Pairings and connections: a toast, and the device list in Settings › Phone.
fn hear_notices(ws: WeakEntity<Workspace>, mut notices: tokio::sync::broadcast::Receiver<tr::ServerNotice>, cx: &mut Context<Workspace>) -> Task<()> {
    let (tx, rx) = async_channel::unbounded();
    trek_core::runtime().spawn(async move {
        loop {
            match notices.recv().await {
                Ok(n) => {
                    if tx.send(n).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    cx.spawn(async move |_, cx: &mut AsyncApp| {
        while let Ok(notice) = rx.recv().await {
            let alive = ws.update(cx, |ws, cx| ws.remote_notice(notice, cx));
            if alive.is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::is_trek_command;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        /// Changed files tests give turns, by (thread, turn end index), until the Mac works
        /// them out itself (`turn_changes`).
        pub(crate) static TURN_FILES: RefCell<HashMap<(String, usize), Vec<trek_remote::ChangedFile>>> = RefCell::default();
    }

    #[test]
    fn trek_commands_are_told_apart_from_the_agent_s() {
        assert!(is_trek_command("/permissions full"));
        assert!(is_trek_command("  /PERMISSIONS full"));
        assert!(is_trek_command("/access full"));
        assert!(is_trek_command("/new"));
        assert!(is_trek_command("/consult opus: hi"));
        assert!(!is_trek_command("/compact"), "the agent's own commands go through");
        assert!(!is_trek_command("fix the /permissions page"));
        assert!(!is_trek_command("plain words"));
    }
}
