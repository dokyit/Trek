//! Trek on your iPhone, the Mac side: the phone server (`trek-remote`) answered from the
//! workspace. Off until Settings › Phone turns it on. Phones pair with a one-time code shown as
//! a QR code, then connect with their own token over TLS pinned to this Mac's certificate.
//!
//! What a phone can do is what the thread's own controls do on the Mac: read threads, send a
//! follow-up (steered or queued as the user's setting says), start a thread, answer an approval,
//! a question or a plan, stop a turn. It can't run Trek's own commands (`/permissions full` would
//! raise what agents may do), and nothing is ever answered on the phone's behalf.
//!
//! Changes go out on a short tick while the server runs: thread rows that changed, and the
//! transcript items of threads a phone has open (by index; each change takes the next `seq` of
//! that thread, as the protocol asks).

use crate::workspace::{Route, Workspace, WorkspaceEvent};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use trek_core::store::{Item, Section, Thread, ToolStatus, now_ms};
use trek_core::{AgentId, RunState};
use trek_remote as tr;

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
    sent: HashMap<String, tr::ThreadSummary>,
    /// How phones answered requests, by request id, so their cards close saying so.
    answered: HashMap<String, tr::AnswerResponse>,
    /// Transcripts phones have open.
    watched: HashMap<String, Watched>,
    _tasks: Vec<Task<()>>,
}

/// A transcript a phone has open: what each item looked like when last sent.
#[derive(Default)]
struct Watched {
    seq: u64,
    items: Vec<u64>,
    /// Requests shown as cards, by id, as last sent.
    requests: Vec<(String, tr::ItemBody)>,
    revision: u64,
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
                    let ip = match reach {
                        trek_core::settings::Reach::Tailscale => addresses.tailscale.or(addresses.lan),
                        trek_core::settings::Reach::Wifi => addresses.lan.or(addresses.tailscale),
                    };
                    let advertise = format!("{}:{port}", ip.map(|i| i.to_string()).unwrap_or_else(|| "127.0.0.1".into()));
                    let mut config = tr::ServerConfig::new(host);
                    config.bind = std::net::SocketAddr::from(([0, 0, 0, 0], port));
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
                        ws.remote = Some(Remote { handle, offer: None, addresses, advertise, started_with: (port, reach), devices, connected: HashSet::new(), sent: HashMap::new(), answered: HashMap::new(), watched: HashMap::new(), _tasks: tasks });
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
            tr::HostRequest::Transcript { thread_id, reply } => {
                if self.thread(&thread_id).is_none() {
                    let _ = reply.send(Err(tr::HostError::not_found("No such thread")));
                    return;
                }
                if self.live.get(&thread_id).is_some_and(|l| l.loaded) {
                    let _ = reply.send(Ok(self.remote_transcript(&thread_id)));
                    return;
                }
                // Read from the agent's files or the database first.
                self.ensure_loaded(&thread_id, cx);
                cx.spawn(async move |this, cx| {
                    for _ in 0..200 {
                        let loaded = this.read_with(cx, |ws, _| ws.live.get(&thread_id).is_some_and(|l| l.loaded)).unwrap_or(true);
                        if loaded {
                            break;
                        }
                        cx.background_executor().timer(Duration::from_millis(25)).await;
                    }
                    let transcript = this.update(cx, |ws, _| ws.remote_transcript(&thread_id)).map_err(|_| tr::HostError::other("Trek is closing"));
                    let _ = reply.send(transcript);
                })
                .detach();
            }
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
            tr::HostRequest::MarkSeen { thread_id, reply } => {
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.last_seen_at = now_ms().max(t.updated_at);
                    let _ = self.store.save_thread(t);
                    cx.notify();
                }
                let _ = reply.send(Ok(()));
            }
        }
    }

    fn remote_send(&mut self, req: tr::SendRequest, cx: &mut Context<Self>) -> tr::HostResult<()> {
        if self.thread(&req.thread_id).is_none() {
            return Err(tr::HostError::not_found("No such thread"));
        }
        if is_trek_command(&req.text) {
            return Err(tr::HostError::bad_request("That's one of Trek's own commands: run it on the Mac."));
        }
        // An explicit choice from the phone wins over the setting, for this message only.
        let before = self.settings.general.follow_up;
        if let Some(mode) = req.mode {
            self.settings.general.follow_up = match mode {
                tr::SendMode::Steer => trek_core::settings::FollowUp::Steer,
                tr::SendMode::Queue => trek_core::settings::FollowUp::Queue,
            };
        }
        self.send_to(&req.thread_id, req.text, vec![], cx);
        self.settings.general.follow_up = before;
        Ok(())
    }

    fn remote_new_thread(&mut self, req: tr::NewThreadRequest, cx: &mut Context<Self>) -> tr::HostResult<String> {
        if is_trek_command(&req.text) {
            return Err(tr::HostError::bad_request("That's one of Trek's own commands: run it on the Mac."));
        }
        let project = match req.project_id.as_str() {
            "" => None,
            id => Some(self.project(id).map(|p| p.path.clone()).ok_or_else(|| tr::HostError::not_found("No such project"))?),
        };
        let agent = AgentId::from_key(&req.agent);
        if !self.ready_agents().contains(&agent) {
            return Err(tr::HostError::not_found(format!("{} isn't set up on this Mac", agent.display_name())));
        }
        // Started the way the Mac's composer starts one, without taking over the Mac's screen.
        let (route, prefs, tabs) = (self.route.clone(), self.draft_prefs.clone(), self.tabs.clone());
        self.route = Route::Draft { project };
        self.draft_prefs.agent = agent;
        self.draft_prefs.model = req.model.clone();
        self.draft_prefs.worktree = req.worktree;
        self.send(req.text, vec![], cx);
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
        if let Some(remote) = self.remote.as_mut() {
            remote.answered.insert(req.request_id.clone(), req.response.clone());
        }
        match (req.response, prompt) {
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
                if let Some(feedback) = feedback.filter(|f| !f.trim().is_empty() && !is_trek_command(f)) {
                    self.send_to(&id, feedback, vec![], cx);
                }
            }
            _ => return Err(tr::HostError::bad_request("That answer doesn't fit the request")),
        }
        Ok(())
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
            .or_else(|| t.paused.as_ref().filter(|_| t.run_state == RunState::Idle).map(|_| tr::Needs { kind: tr::NeedsKind::Limit, text: "Paused at its usage limit".into() }))
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
        let working_since = live.and_then(|l| l.turn_started).map(|s| now - s.elapsed().as_millis() as i64);
        tr::ThreadSummary {
            id: t.id.clone(),
            title: t.title.clone(),
            project: self.remote_project(t),
            agent: tr::AgentRef { key: t.agent.key(), name: t.agent.display_name() },
            model: t.model.clone(),
            model_label: t.model.as_ref().map(|m| self.models_for(&t.agent).into_iter().find(|x| &x.id == m).map(|x| x.name).unwrap_or_else(|| m.clone())),
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
        }
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
                let models: Vec<tr::ModelOption> = self.models_for(&agent).into_iter().map(|m| tr::ModelOption { id: m.id, label: m.name }).collect();
                tr::AgentOption { key: agent.key(), name: agent.display_name(), default_model: models.first().map(|m| m.id.clone()), models }
            })
            .collect();
        tr::Snapshot { threads, projects, agents }
    }

    /// `id`'s transcript as the phone shows it: its items by index, then its open requests.
    fn remote_items(&self, id: &str) -> (Vec<tr::ItemBody>, Vec<(String, tr::ItemBody)>) {
        let Some(live) = self.live.get(id) else { return (vec![], vec![]) };
        let cwd = self.thread(id).and_then(|t| t.cwd.clone());
        let items = live
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| match item {
                Item::User { text, images, .. } => tr::ItemBody::User { text: text.clone(), images: images.len() as u32 },
                Item::Assistant { text } => tr::ItemBody::Assistant { text: text.clone(), streaming: live.streaming == Some(ix) },
                Item::Reasoning { text } => tr::ItemBody::Reasoning { text: text.clone() },
                Item::Tool { id: call, title, detail, output, status } => {
                    let op = crate::activity::op(title, detail, cwd.as_deref());
                    let lines = live.lines.get(call).copied();
                    tr::ItemBody::Tool {
                        call_id: call.clone(),
                        tool: tr::ToolKind::from_title(title),
                        title: if op.verb.is_empty() { title.clone() } else { op.verb },
                        detail: if op.text.is_empty() { detail.clone() } else { op.text },
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
                Item::Notice { text } => tr::ItemBody::Notice { text: text.clone() },
                Item::Error { text } => tr::ItemBody::Error { text: text.clone() },
                Item::Limit { text, resets_at, .. } => tr::ItemBody::Limit { text: text.clone(), resets_at: *resets_at },
                Item::Handoff { from, to, from_name, to_name, .. } => tr::ItemBody::Handoff {
                    from: from_name.clone().unwrap_or_else(|| AgentId::from_key(from).display_name()),
                    to: to_name.clone().unwrap_or_else(|| AgentId::from_key(to).display_name()),
                },
            })
            .collect();
        let requests = live
            .permissions
            .iter()
            .map(|p| {
                let body = match &p.prompt {
                    None => tr::ItemBody::Approval { request_id: p.request_id.clone(), title: p.title.clone(), detail: p.detail.clone(), state: tr::ApprovalState::Pending },
                    Some(trek_agents::Prompt::Questions(qs)) => tr::ItemBody::Question {
                        request_id: p.request_id.clone(),
                        questions: qs
                            .iter()
                            .map(|q| tr::Question {
                                header: q.header.clone(),
                                question: q.question.clone(),
                                options: q.options.iter().map(|(label, description)| tr::QuestionOption { label: label.clone(), description: description.clone() }).collect(),
                                multi: q.multi,
                                secret: q.secret,
                            })
                            .collect(),
                        state: tr::QuestionState::Pending,
                        answers: None,
                    },
                    Some(trek_agents::Prompt::Plan(markdown)) => tr::ItemBody::Plan { request_id: p.request_id.clone(), markdown: markdown.clone(), state: tr::PlanState::Pending },
                };
                (p.request_id.clone(), body)
            })
            .collect();
        (items, requests)
    }

    fn remote_transcript(&mut self, id: &str) -> tr::Transcript {
        let (items, requests) = self.remote_items(id);
        let at = self.live.get(id).map(|l| item_times(&l.items)).unwrap_or_default();
        let revision = self.live.get(id).map(|l| l.revision).unwrap_or_default();
        let Some(remote) = self.remote.as_mut() else { return tr::Transcript::default() };
        // A phone opening the thread again starts its numbering where it left off.
        let watched = remote.watched.entry(id.to_string()).or_default();
        let mut out = Vec::with_capacity(items.len() + requests.len());
        watched.items.clear();
        for (ix, body) in items.into_iter().enumerate() {
            watched.seq += 1;
            watched.items.push(hash(&body));
            out.push(tr::Item { id: format!("i{ix}"), seq: watched.seq, at: at.get(ix).copied().flatten(), body });
        }
        watched.requests.clear();
        for (rid, body) in requests {
            watched.seq += 1;
            watched.requests.push((rid.clone(), body.clone()));
            out.push(tr::Item { id: format!("r{rid}"), seq: watched.seq, at: None, body });
        }
        watched.revision = revision;
        tr::Transcript { seq: watched.seq, items: out }
    }

    /// Send phones what changed since the last tick.
    fn push_remote_changes(&mut self) {
        let Some(remote) = self.remote.as_ref() else { return };
        if remote.connected.is_empty() {
            return;
        }
        let now = now_ms();
        let rows: Vec<tr::ThreadSummary> = self.remote_threads().into_iter().map(|t| self.remote_summary(t, now)).collect();
        let watched: Vec<String> = remote.watched.keys().cloned().collect();
        let changes: Vec<(String, u64, Vec<tr::ItemBody>, Vec<(String, tr::ItemBody)>, Vec<Option<i64>>)> = watched
            .into_iter()
            .filter_map(|id| {
                let live = self.live.get(&id)?;
                let rev = live.revision;
                if self.remote.as_ref()?.watched.get(&id).is_some_and(|w| w.revision == rev) && live.streaming.is_none() {
                    return None;
                }
                let (items, requests) = self.remote_items(&id);
                Some((id, rev, items, requests, item_times(&live.items)))
            })
            .collect();
        let Some(remote) = self.remote.as_mut() else { return };
        // Thread rows.
        let ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let gone: Vec<String> = remote.sent.keys().filter(|id| !ids.contains(id.as_str())).cloned().collect();
        for id in gone {
            remote.sent.remove(&id);
            remote.watched.remove(&id);
            remote.handle.push(tr::HostEvent::ThreadRemoved(id));
        }
        for row in rows {
            if remote.sent.get(&row.id) != Some(&row) {
                remote.sent.insert(row.id.clone(), row.clone());
                remote.handle.push(tr::HostEvent::Thread(row));
            }
        }
        // Open transcripts.
        for (id, rev, items, requests, at) in changes {
            let Some(w) = remote.watched.get_mut(&id) else { continue };
            w.revision = rev;
            // Shorter than sent (a rewind, a rewrite): the phone reads it again.
            if items.len() < w.items.len() {
                w.items.clear();
                w.requests.clear();
                remote.handle.push(tr::HostEvent::TranscriptReset(id.clone()));
                continue;
            }
            for (ix, body) in items.into_iter().enumerate() {
                let h = hash(&body);
                if w.items.get(ix) == Some(&h) {
                    continue;
                }
                w.seq += 1;
                match w.items.get_mut(ix) {
                    Some(old) => *old = h,
                    None => w.items.push(h),
                }
                remote.handle.push(tr::HostEvent::Item { thread_id: id.clone(), item: tr::Item { id: format!("i{ix}"), seq: w.seq, at: at.get(ix).copied().flatten(), body } });
            }
            // Requests: new or changed ones as they are, answered ones as resolved.
            let open: HashSet<String> = requests.iter().map(|(r, _)| r.clone()).collect();
            for (rid, last) in w.requests.iter().filter(|(r, _)| !open.contains(r)) {
                w.seq += 1;
                let body = resolved(last.clone(), remote.answered.remove(rid));
                remote.handle.push(tr::HostEvent::Item { thread_id: id.clone(), item: tr::Item { id: format!("r{rid}"), seq: w.seq, at: None, body } });
            }
            w.requests.retain(|(r, _)| open.contains(r));
            for (rid, body) in requests {
                if w.requests.iter().any(|(r, x)| *r == rid && *x == body) {
                    continue;
                }
                w.seq += 1;
                w.requests.retain(|(r, _)| *r != rid);
                w.requests.push((rid.clone(), body.clone()));
                remote.handle.push(tr::HostEvent::Item { thread_id: id.clone(), item: tr::Item { id: format!("r{rid}"), seq: w.seq, at: None, body } });
            }
        }
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
            *answers = Some(given);
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
fn item_times(items: &[Item]) -> Vec<Option<i64>> {
    items
        .iter()
        .map(|i| match i {
            Item::User { at, .. } => *at,
            Item::TurnEnd { at, .. } => Some(*at),
            _ => None,
        })
        .collect()
}

fn hash(body: &tr::ItemBody) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(body).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().chars().take(140).collect()
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
            if ws.update(cx, |ws, _| ws.push_remote_changes()).is_err() {
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
            let alive = ws.update(cx, |ws, cx| {
                let Some(remote) = ws.remote.as_mut() else { return };
                remote.devices = remote.handle.devices();
                match notice {
                    tr::ServerNotice::Paired { name, .. } => {
                        remote.offer = None;
                        cx.emit(WorkspaceEvent::Toast { message: format!("{name} is paired with Trek"), undo: None });
                    }
                    tr::ServerNotice::Connected { device_id } => {
                        remote.connected.insert(device_id);
                        // A phone that just came reads everything afresh.
                        remote.sent.clear();
                    }
                    tr::ServerNotice::Disconnected { device_id } => {
                        remote.connected.remove(&device_id);
                    }
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::is_trek_command;

    #[test]
    fn trek_commands_stay_on_the_mac() {
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
