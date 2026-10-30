//! Other machines running Trek that this one drives (`docs/MULTI-MACHINE.md`). A machine pairs the
//! way a phone does (its `trek://pair?…` link, or its address, code and fingerprint typed) and is
//! kept in `machines.json` in the data folder: the token it gave, the certificate it's pinned to,
//! its name and routes, when it was last seen, and which of this Mac's projects it has a copy of.
//! While Trek runs each paired machine has a connection of its own (`trek_remote::client::Client`),
//! which says whether it's reachable and brings its snapshot, thread rows and transcripts.
//!
//! A machine's threads show in the sidebar under its name and open in the normal thread view. Its
//! rows and transcripts are mapped to Trek's own types (`Thread`, `Item`, `PendingPermission`) and
//! held in memory only, under ids of their own (`machine:<host id>:<thread id>`): the thread view,
//! working bar and composer draw them as they draw any thread, and what the user does in them goes
//! to the machine as protocol messages (`send`, `interrupt`, `answer`, `set_prefs`, `thread_action`).
//! Nothing of theirs is ever written to the local store; the machine owns the thread.

pub mod clone;
pub mod sheets;

use crate::workspace::{PendingPermission, Route, Scope, Workspace, WorkspaceEvent};
use gpui_kit::{Context, Task};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trek_core::store::{Item, Thread, ToolStatus, now_ms};
use trek_core::transcript::Transcript;
use trek_core::{AgentId, Effort, HandHolding, RunState, ThreadSource};
use trek_remote as tr;
use trek_remote::client::{Auth, Client, Event, Paired, State};

/// How a machine's threads are told apart from this Mac's: their ids start with this.
pub const ID_PREFIX: &str = "machine:";

/// How long pairing waits for the machine to take the code before it gives up.
const PAIR_WAIT: Duration = Duration::from_secs(30);

/// How many items a thread opens with (earlier ones stay on the machine).
const OPEN_LIMIT: u32 = 300;

/// How often this computer's network is looked at while machines are paired: a new one (another
/// Wi-Fi, Tailscale coming up) has every machine dialled again at once.
const NETWORK_EVERY: Duration = Duration::from_secs(5);

/// How often Tailscale is asked who's in the tailnet while a machine can't be reached, the
/// network unchanged: a machine that just joined it is found without waiting for a change here.
const TAILNET_EVERY: Duration = Duration::from_secs(60);

/// The id a thread of machine `host` goes by here.
pub fn local_id(host: &str, thread: &str) -> String {
    format!("{ID_PREFIX}{host}:{thread}")
}

/// The machine and its own thread id, for an id made by `local_id`.
pub fn split_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(ID_PREFIX)?.rsplit_once(':')
}

/// Whether `id` is a thread living on another machine.
pub fn is_remote(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

// ---------------------------------------------------------------------------------------------
// Pairing links
// ---------------------------------------------------------------------------------------------

/// Where to reach a machine to pair with it, and the one-time code it shows: what its pairing link
/// (`trek://pair?host&code&name&hid&fp&alt`, the QR code's text) carries, or what was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLink {
    /// `host:port`, the one the link names first.
    pub routes: Vec<String>,
    pub code: String,
    /// The SHA-256 of its certificate, 64 lowercase hex characters.
    pub fingerprint: String,
    pub name: Option<String>,
    pub host_id: Option<String>,
}

impl PairLink {
    /// A pairing link as copied from the other machine's Settings › Phone & Machines.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let query = ["trek://pair?", "trek://pair/?"]
            .iter()
            .find_map(|p| {
                text.get(..p.len())
                    .filter(|head| head.eq_ignore_ascii_case(p))
                    .map(|_| &text[p.len()..])
            })
            .ok_or_else(|| "That isn't a pairing link: it starts with trek://pair?".to_string())?;
        let (mut host, mut code, mut fp, mut name, mut hid, mut alt) =
            (None, None, None, None, None, vec![]);
        for part in query.split('&') {
            let (key, raw) = part.split_once('=').unwrap_or((part, ""));
            match key {
                "host" => host = Some(percent_decode(raw)),
                "code" => code = Some(percent_decode(raw)),
                "fp" => fp = Some(percent_decode(raw)),
                "name" => name = Some(percent_decode(raw)).filter(|n| !n.trim().is_empty()),
                "hid" => hid = Some(percent_decode(raw)).filter(|n| !n.trim().is_empty()),
                // Routes are separated by bare commas; a comma inside one would be encoded.
                "alt" => {
                    alt = raw
                        .split(',')
                        .map(percent_decode)
                        .map(|r| r.trim().to_string())
                        .filter(|r| !r.is_empty())
                        .collect()
                }
                _ => {}
            }
        }
        let host = host
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .ok_or("The link names no address (host=)")?;
        let code = code
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .ok_or("The link has no pairing code (code=)")?;
        let fingerprint = full_fingerprint(&fp.ok_or(
            "The link has no fingerprint (fp=): the other machine's server runs without TLS",
        )?)?;
        let mut routes = vec![with_port(&host)];
        for r in alt.iter().map(|r| with_port(r)) {
            if !routes.contains(&r) {
                routes.push(r);
            }
        }
        Ok(Self {
            routes,
            code,
            fingerprint,
            name,
            host_id: hid,
        })
    }

    /// What was typed instead of a link: the address (`192.168.1.20` or `studio.local:7420`), the
    /// code, and the whole fingerprint.
    pub fn typed(host: &str, code: &str, fingerprint: &str) -> Result<Self, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("Type the other machine's address".into());
        }
        let code = code.trim();
        if code.is_empty() {
            return Err("Type the pairing code the other machine shows".into());
        }
        Ok(Self {
            routes: vec![with_port(host)],
            code: code.to_string(),
            fingerprint: full_fingerprint(fingerprint)?,
            name: None,
            host_id: None,
        })
    }
}

/// A fingerprint as typed or linked (any case, spaces, colons or dashes between), as the client
/// pins it.
fn full_fingerprint(text: &str) -> Result<String, String> {
    let hex: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | ':' | '-'))
        .collect::<String>()
        .to_ascii_lowercase();
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(hex);
    }
    if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("That's the short fingerprint a phone takes. A machine needs the whole one: copy the pairing link instead, which carries it".into());
    }
    Err("The fingerprint is 64 hex characters (the pairing link carries it)".into())
}

/// `host` with the phone server's usual port when it names none.
fn with_port(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    let host = ["wss://", "ws://", "https://", "http://"]
        .iter()
        .find_map(|s| host.strip_prefix(s))
        .unwrap_or(host);
    let has_port = match host.rsplit_once(':') {
        // `[fd7a::1]:7420`, or `host:7420`; a bare IPv6 address has colons but no port.
        Some((left, port)) => {
            port.parse::<u16>().is_ok() && (left.ends_with(']') || !left.contains(':'))
        }
        None => false,
    };
    if has_port {
        host.to_string()
    } else {
        format!("{host}:{}", tr::DEFAULT_PORT)
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).copied().and_then(hex),
            bytes.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------------------------
// machines.json
// ---------------------------------------------------------------------------------------------

/// What `machines.json` keeps of a paired machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    /// Its token (a secret), certificate pin, host and routes.
    pub paired: Paired,
    /// What it's called here: its own name, as it last said.
    pub name: String,
    /// When it was last connected (ms).
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// This Mac's project folders and the project each was cloned to there (its id there).
    #[serde(default)]
    pub projects: BTreeMap<String, String>,
}

impl MachineEntry {
    /// The machine's lasting id (its host id).
    pub fn id(&self) -> &str {
        &self.paired.host.id
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MachinesFile {
    /// Who this Mac is to the machines it paired with (each lists it as a device by this id).
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    machines: Vec<MachineEntry>,
}

pub fn machines_file() -> PathBuf {
    trek_core::paths::data_dir().join("machines.json")
}

/// The machines paired before, and this Mac's device id (made the first time). An entry that
/// can't be read is left out, not the rest with it.
pub fn load() -> (String, Vec<MachineEntry>) {
    let text = std::fs::read_to_string(machines_file()).unwrap_or_default();
    let file: MachinesFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(_) if text.trim().is_empty() => MachinesFile::default(),
        Err(e) => {
            tracing::warn!("machines.json: {e}; keeping the entries that read");
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            MachinesFile {
                device_id: value
                    .get("device_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                machines: value
                    .get("machines")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|m| serde_json::from_value(m.clone()).ok())
                    .collect(),
            }
        }
    };
    let device_id = file
        .device_id
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    (device_id, file.machines)
}

/// Write `machines.json` (only its owner may read it: it holds tokens).
pub fn save(device_id: &str, machines: &[MachineEntry]) -> std::io::Result<()> {
    let path = machines_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = MachinesFile {
        device_id: Some(device_id.to_string()),
        machines: machines.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    write_private(&tmp, &text)?;
    std::fs::rename(&tmp, &path)
}

/// Write `text` to a new file at `path` that only its owner can read, from the moment it exists
/// (a file made first and restricted after can be read by others in between). One left there
/// by an earlier run is replaced.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())
}

// ---------------------------------------------------------------------------------------------
// The registry, while Trek runs
// ---------------------------------------------------------------------------------------------

/// Whether a machine can be reached now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    Connecting,
    Connected,
    /// For good, until it's paired again: why.
    Stopped(String),
}

/// A transcript open here: B's items as last sent, in order.
#[derive(Debug, Default)]
struct Followed {
    items: Vec<tr::Item>,
    loaded: bool,
    /// Earlier items stayed on the machine.
    more: bool,
}

/// A paired machine and its connection.
pub struct Machine {
    pub entry: MachineEntry,
    pub reach: Reach,
    /// When the connection that was up was lost: its threads show as they last were, none of
    /// them running, until it's back. `None` while connected, and before it ever was.
    pub offline_since: Option<i64>,
    /// Where this computer's Tailscale says the machine is (`tailnet_routes`), as last given to
    /// the client.
    hints: Vec<String>,
    pub client: Option<Arc<Client>>,
    /// What it said it is (`machine_info`): its home folder above all.
    pub info: Option<tr::MachineInfo>,
    /// Its projects and agents, as its last snapshot listed them.
    pub projects: Vec<tr::ProjectSummary>,
    pub agents: Vec<tr::AgentOption>,
    /// Its thread rows, as it sent them.
    rows: Vec<tr::ThreadSummary>,
    /// The rows as Trek's threads, under local ids (`local_id`).
    pub threads: Vec<Thread>,
    followed: HashMap<String, Followed>,
    _pump: Option<Task<()>>,
}

impl Machine {
    pub fn id(&self) -> &str {
        self.entry.id()
    }

    pub fn reachable(&self) -> bool {
        self.reach == Reach::Connected
    }

    /// Its rows as Trek's threads. A machine that can't be reached can't say what its threads
    /// are doing: each shows as it last was, but none as running (no spinner, no turn clock).
    fn refresh_threads(&mut self) {
        let host = self.id().to_string();
        let offline = !self.reachable();
        self.threads = self
            .rows
            .iter()
            .map(|r| {
                let mut t = thread_of(&host, r);
                if offline && t.run_state == RunState::Working {
                    t.run_state = RunState::Idle;
                }
                t
            })
            .collect();
    }

    /// The threads the sidebar lists under its name: those not settled, and settled ones still
    /// at work (a turn running, or sub-agents out), newest first.
    pub fn listed(&self) -> Vec<&Thread> {
        let mut out: Vec<&Thread> = self
            .threads
            .iter()
            .filter(|t| t.archived_at.is_none() && t.parent_id.is_none())
            .filter(|t| t.settled_at.is_none() || self.busy(&t.id))
            .collect();
        out.sort_by(|a, b| {
            b.needs_you()
                .cmp(&a.needs_you())
                .then(b.updated_at.cmp(&a.updated_at))
        });
        out
    }

    /// Its row for the thread of local id `id`.
    fn row(&self, id: &str) -> Option<&tr::ThreadSummary> {
        let (_, tid) = split_id(id)?;
        self.rows.iter().find(|r| r.id == tid)
    }

    /// Whether its thread `id` is at work as far as here can see: its turn runs, or sub-agents or
    /// background work it started are still out. Nothing is, while the machine can't be reached.
    fn busy(&self, id: &str) -> bool {
        self.reachable()
            && self.row(id).is_some_and(|r| {
                r.run_state == tr::RunState::Working
                    || !r.sub_agents.is_empty()
                    || !r.background.is_empty()
            })
    }

    /// Its copy of this Mac's project at `path`: the one cloned there (its snapshot may not list
    /// it yet), or one of the same name.
    pub fn project_for(&self, path: &Path, name: &str) -> Option<String> {
        if let Some(id) = self.entry.projects.get(&path.display().to_string()) {
            return Some(id.clone());
        }
        let folder = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.projects
            .iter()
            .find(|p| p.name == name)
            .or_else(|| self.projects.iter().find(|p| p.name == folder))
            .map(|p| p.id.clone())
    }
}

/// Where pairing from "Add a machine…" stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing {
    Busy,
    Failed(String),
    /// Paired with the machine of this name.
    Done(String),
}

/// A clone to another machine under way (or just over), as its sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRun {
    pub host: String,
    /// The folder on the machine, as asked for (its progress lines name it).
    pub folder: String,
    /// What git said, last lines last.
    pub lines: Vec<String>,
    pub stage: CloneStage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneStage {
    Cloning,
    Copying { files: usize },
    Done { project_id: String },
    Failed(String),
}

/// Every paired machine, and what pairing and cloning are doing.
#[derive(Default)]
pub struct Machines {
    pub list: Vec<Machine>,
    /// This Mac's device id to them.
    pub device_id: String,
    pub pairing: Option<Pairing>,
    pub clone: Option<CloneRun>,
    /// This computer's addresses and tailnet, as last found (`follow_network`).
    pub here: crate::remote::Addresses,
    _network: Option<Task<()>>,
}

impl Machines {
    pub fn get(&self, host: &str) -> Option<&Machine> {
        self.list.iter().find(|m| m.id() == host)
    }

    fn get_mut(&mut self, host: &str) -> Option<&mut Machine> {
        self.list.iter_mut().find(|m| m.id() == host)
    }

    /// A machine's thread by its local id.
    pub fn thread(&self, id: &str) -> Option<&Thread> {
        if !is_remote(id) {
            return None;
        }
        let (host, _) = split_id(id)?;
        self.get(host)?.threads.iter().find(|t| t.id == id)
    }

    /// The machine of thread `id` and its row there, while the machine can be reached: a row
    /// from before it went says nothing of what runs now.
    fn reachable_row(&self, id: &str) -> Option<&tr::ThreadSummary> {
        let (host, _) = split_id(id)?;
        self.get(host).filter(|m| m.reachable())?.row(id)
    }

    /// What machine thread `id` has at work, as its row says, the way the sidebar's card shows a
    /// thread here (`Sidebar::at_work`): its sub-agents still out, by logo and name ("Sol: Review
    /// the diff" for one Trek runs there), and what its agent runs in the background, by title.
    pub fn at_work(&self, id: &str) -> (Vec<(AgentId, String)>, Vec<String>) {
        let Some(row) = self.reachable_row(id) else {
            return (vec![], vec![]);
        };
        let kids = row
            .sub_agents
            .iter()
            .filter(|k| {
                matches!(
                    k.state,
                    tr::SubAgentState::Running | tr::SubAgentState::NeedsYou
                )
            })
            .map(|k| {
                let name = match &k.model {
                    Some(model) => format!("{model}: {}", k.title),
                    None => k.title.clone(),
                };
                (AgentId::from_key(&k.agent.key), name)
            })
            .collect();
        (kids, row.background.clone())
    }

    /// How long machine thread `id`'s turn has run, while it works there.
    pub fn working_for(&self, id: &str) -> Option<Duration> {
        let row = self
            .reachable_row(id)
            .filter(|r| r.run_state == tr::RunState::Working)?;
        Some(Duration::from_millis(
            (now_ms() - row.working_since?).max(0) as u64,
        ))
    }

    /// How long machine thread `id` has waited on its longest-running sub-agent, while its own
    /// turn is over and they're still out (`waiting`).
    pub fn waited_for(&self, id: &str) -> Option<Duration> {
        let since = self
            .reachable_row(id)?
            .sub_agents
            .iter()
            .filter_map(|k| k.since)
            .min()?;
        Some(Duration::from_millis((now_ms() - since).max(0) as u64))
    }

    /// Machine threads a sub-agent under which waits on the user (the row's `needs_thread`):
    /// their cards say so, as a thread's here do (`Workspace::waiting_on_sub_agents`).
    pub fn sub_agents_need_you(&self) -> std::collections::HashSet<String> {
        self.list
            .iter()
            .flat_map(|m| {
                let host = m.id();
                m.rows
                    .iter()
                    .filter(|r| r.needs_thread.is_some())
                    .map(move |r| local_id(host, &r.id))
            })
            .collect()
    }

    /// Machine threads whose turn is over while sub-agents they started are still out: at work,
    /// waiting on them (`Workspace::waiting_threads`).
    pub fn waiting(&self) -> std::collections::HashSet<String> {
        self.list
            .iter()
            .filter(|m| m.reachable())
            .flat_map(|m| {
                let host = m.id();
                m.rows
                    .iter()
                    .filter(|r| {
                        r.run_state == tr::RunState::Idle
                            && r.needs.is_none()
                            && !r.sub_agents.is_empty()
                    })
                    .map(move |r| local_id(host, &r.id))
            })
            .collect()
    }

    /// Whether a thread on a machine that can be reached has a turn running or sub-agents out:
    /// the sidebar's clocks count by the second meanwhile.
    pub fn any_at_work(&self) -> bool {
        self.list.iter().filter(|m| m.reachable()).any(|m| {
            m.rows
                .iter()
                .any(|r| r.run_state == tr::RunState::Working || !r.sub_agents.is_empty())
        })
    }

    /// Where machine thread `id` is, as the sidebar names it out of its machine's group: its
    /// project there and the machine ("trek on T-Book").
    pub fn place(&self, id: &str) -> Option<String> {
        let (host, _) = split_id(id)?;
        let m = self.get(host)?;
        let name = &m.entry.name;
        Some(match m.row(id).map(|r| r.project.name.as_str()) {
            Some(project) if !project.is_empty() => format!("{project} on {name}"),
            _ => name.clone(),
        })
    }

    /// The model machine thread `id` runs, as the machine names it ("Opus 5.5").
    pub fn model_label(&self, id: &str) -> Option<String> {
        let (host, _) = split_id(id)?;
        self.get(host)?.row(id)?.model_label.clone()
    }

    /// The run of the clone of `folder` to `host`, if it's still the one the sheet shows.
    fn clone_run(&mut self, host: &str, folder: &str) -> Option<&mut CloneRun> {
        self.clone
            .as_mut()
            .filter(|r| r.host == host && r.folder == folder)
    }

    pub fn entries(&self) -> Vec<MachineEntry> {
        self.list.iter().map(|m| m.entry.clone()).collect()
    }

    fn save(&self) {
        if let Err(e) = save(&self.device_id, &self.entries()) {
            tracing::warn!("save machines.json: {e}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Mapping the machine's rows and items to Trek's own
// ---------------------------------------------------------------------------------------------

/// A thread row of machine `host` as one of Trek's threads (in memory only).
pub fn thread_of(host: &str, s: &tr::ThreadSummary) -> Thread {
    let run_state = match s.run_state {
        tr::RunState::Idle => RunState::Idle,
        tr::RunState::Working => RunState::Working,
        tr::RunState::NeedsYou => RunState::NeedsYou,
        tr::RunState::Failed => RunState::Failed,
    };
    let at = s.updated_at;
    Thread {
        id: local_id(host, &s.id),
        // Not one of this Mac's projects: the machine's own strip of tabs, and no local folder.
        project_id: Some(format!("{ID_PREFIX}{host}")),
        title: s.title.clone(),
        agent: AgentId::from_key(&s.agent.key),
        model: s.model.clone(),
        effort: s
            .effort
            .as_deref()
            .and_then(Effort::parse)
            .unwrap_or_default(),
        hand_holding: s
            .access
            .map(hand_holding_of)
            .unwrap_or(HandHolding::Supervised),
        source: ThreadSource::Trek,
        native_id: None,
        cwd: None,
        branch: s.branch.clone(),
        created_at: at,
        updated_at: at,
        last_seen_at: if s.unseen { 0 } else { at },
        settled_at: (s.section == tr::Section::Settled).then_some(at),
        pinned_at: s.pinned.then_some(at),
        snoozed_until: s.snoozed_until,
        archived_at: None,
        run_state,
        additions: s.additions as i64,
        deletions: s.deletions as i64,
        side_of: None,
        never_settle: false,
        imported_title: None,
        import_hidden: None,
        import_kept: false,
        worktree: None,
        native_at: None,
        reopen: None,
        paused: None,
        parent_id: s.parent_id.as_ref().map(|p| local_id(host, p)),
        started_by: None,
        continued_from: None,
        notify_starter: false,
        editor_chat: false,
    }
}

fn hand_holding_of(access: tr::Access) -> HandHolding {
    match access {
        tr::Access::Supervised => HandHolding::Supervised,
        tr::Access::AutoAcceptEdits => HandHolding::AutoAcceptEdits,
        tr::Access::Auto => HandHolding::Auto,
        tr::Access::FullAccess => HandHolding::FullAccess,
    }
}

fn access_of(h: HandHolding) -> tr::Access {
    match h {
        HandHolding::Supervised => tr::Access::Supervised,
        HandHolding::AutoAcceptEdits => tr::Access::AutoAcceptEdits,
        HandHolding::Auto => tr::Access::Auto,
        HandHolding::FullAccess => tr::Access::FullAccess,
    }
}

/// The first message of a thread continued on another machine: the conversation so far
/// (`markdown`), said to come from computer `from`.
pub fn continuing_text(from: &str, markdown: &str) -> String {
    format!(
        "Continuing this conversation from {from}:\n\n{}",
        markdown.trim()
    )
}

/// A transcript as the thread view takes it: rows (the machine's item ids, Trek's items), the
/// cards waiting on the user, and the answer streaming in, if one is.
pub struct Mapped {
    pub rows: Vec<(String, Item)>,
    pub requests: Vec<PendingPermission>,
    pub streaming: Option<usize>,
}

/// A tool call that was running when its machine went out of reach: nothing here can say it
/// still is, so it shows as ended (as a local call left running by a turn that ended does,
/// `Failed`) until the machine is back and says.
pub fn settle_running_tools(rows: &mut [(String, Item)]) {
    for (_, item) in rows {
        if let Item::Tool { status, .. } = item
            && *status == ToolStatus::Running
        {
            *status = ToolStatus::Failed;
        }
    }
}

/// The machine's items as Trek's. What they can't carry is left out (a turn's changed files, the
/// images a message had) or said in a notice (another thread it links to, a handoff).
pub fn map_items(items: &[tr::Item]) -> Mapped {
    let mut out = Mapped {
        rows: Vec::with_capacity(items.len()),
        requests: vec![],
        streaming: None,
    };
    for it in items {
        let item = match &it.body {
            tr::ItemBody::User {
                text,
                images,
                aside,
            } => {
                let text = match images {
                    0 => text.clone(),
                    1 => format!("{text}\n\n(1 image)"),
                    n => format!("{text}\n\n({n} images)"),
                };
                Item::User {
                    text,
                    images: vec![],
                    at: it.at,
                    resume: None,
                    aside: *aside,
                }
            }
            tr::ItemBody::Assistant { text, streaming } => {
                if *streaming {
                    out.streaming = Some(out.rows.len());
                }
                Item::Assistant { text: text.clone() }
            }
            tr::ItemBody::Reasoning { text } => Item::Reasoning { text: text.clone() },
            tr::ItemBody::Tool {
                call_id,
                title,
                detail,
                status,
                output,
                ..
            } => Item::Tool {
                id: if call_id.is_empty() {
                    it.id.clone()
                } else {
                    call_id.clone()
                },
                title: title.clone(),
                detail: detail.clone(),
                output: output.clone(),
                status: match status {
                    tr::ToolStatus::Running => ToolStatus::Running,
                    tr::ToolStatus::Done => ToolStatus::Done,
                    tr::ToolStatus::Failed => ToolStatus::Failed,
                    tr::ToolStatus::Denied => ToolStatus::Denied,
                },
            },
            tr::ItemBody::TurnEnd { took_secs } => Item::TurnEnd {
                at: it.at.unwrap_or_else(now_ms),
                took_secs: *took_secs,
            },
            tr::ItemBody::Notice { text } => Item::Notice { text: text.clone() },
            tr::ItemBody::Error { text } => Item::Error { text: text.clone() },
            tr::ItemBody::Limit { text, resets_at } => Item::Limit {
                text: text.clone(),
                resets_at: *resets_at,
                scope: Default::default(),
            },
            tr::ItemBody::Handoff { from, to } => Item::Notice {
                text: format!("Handed over from {from} to {to}"),
            },
            tr::ItemBody::ThreadLink { link, title, .. } => Item::Notice {
                text: format!("{} “{title}”", link.label()),
            },
            tr::ItemBody::Changes { .. } => continue,
            // Another machine's card stays its own: its buttons and its `gh` run there.
            tr::ItemBody::Card { card } => Item::Notice {
                text: card.summary(),
            },
            tr::ItemBody::Approval {
                request_id,
                title,
                detail,
                state,
            } => {
                if *state == tr::ApprovalState::Pending {
                    out.requests.push(PendingPermission {
                        request_id: request_id.clone(),
                        title: title.clone(),
                        detail: detail.clone(),
                        prompt: None,
                        after_turn: false,
                    });
                }
                continue;
            }
            tr::ItemBody::Question {
                request_id,
                questions,
                state,
                ..
            } => {
                if *state == tr::QuestionState::Pending {
                    let questions = questions
                        .iter()
                        .map(|q| trek_agents::Question {
                            question: q.question.clone(),
                            header: q.header.clone(),
                            options: q
                                .options
                                .iter()
                                .map(|o| (o.label.clone(), o.description.clone()))
                                .collect(),
                            multi: q.multi,
                            secret: q.secret,
                        })
                        .collect();
                    out.requests.push(PendingPermission {
                        request_id: request_id.clone(),
                        title: "Questions".into(),
                        detail: String::new(),
                        prompt: Some(trek_agents::Prompt::Questions(questions)),
                        after_turn: false,
                    });
                }
                continue;
            }
            tr::ItemBody::Plan {
                request_id,
                markdown,
                state,
            } => {
                if *state == tr::PlanState::Pending {
                    out.requests.push(PendingPermission {
                        request_id: request_id.clone(),
                        title: "Plan".into(),
                        detail: String::new(),
                        prompt: Some(trek_agents::Prompt::Plan(markdown.clone())),
                        after_turn: false,
                    });
                }
                continue;
            }
        };
        out.rows.push((it.id.clone(), item));
    }
    out
}

/// Put `item` in `items`: in its place when its id is there, else at the end.
fn upsert(items: &mut Vec<tr::Item>, item: tr::Item) {
    match items.iter_mut().find(|i| i.id == item.id) {
        Some(slot) => *slot = item,
        None => items.push(item),
    }
}

/// This computer's name, as the machines it pairs with list it among their devices.
pub(crate) fn this_name() -> String {
    #[cfg(target_os = "macos")]
    {
        let name = trek_core::procs::command("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            return name;
        }
    }
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Some(name) = std::env::var(var).ok().filter(|n| !n.trim().is_empty()) {
            return name;
        }
    }
    let words = crate::words::words();
    let mut name = words.computer.to_string();
    if let Some(first) = name.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    name
}

// ---------------------------------------------------------------------------------------------
// Reaching a machine from another network
// ---------------------------------------------------------------------------------------------

/// The host of a `host:port` route, without brackets or port.
fn route_host(route: &str) -> &str {
    let route = route.trim();
    if let Some(rest) = route.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match route.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => host,
        _ => route,
    }
}

/// Whether `route` is on a tailnet: Tailscale's 100.64/10 or fd7a:115c:a1e0::/48 address, or a
/// MagicDNS name. Only such a route reaches a machine from another network.
pub fn tailnet_route(route: &str) -> bool {
    let host = route_host(route);
    let host = host.split('%').next().unwrap_or(host);
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (64..128).contains(&b)
        }
        Ok(std::net::IpAddr::V6(v6)) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        Err(_) => host
            .trim_end_matches('.')
            .to_ascii_lowercase()
            .ends_with(".ts.net"),
    }
}

/// A name as Tailscale and Trek may each write it: `T-Book`, `t-book`, `T BOOK` alike.
fn plain_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The peer in this computer's tailnet that is machine `name`, by its host or MagicDNS name.
pub fn tailnet_peer<'a>(
    name: &str,
    tailnet: &'a crate::remote::Tailnet,
) -> Option<&'a crate::remote::TailnetPeer> {
    let name = plain_name(name);
    if name.is_empty() {
        return None;
    }
    tailnet.peers.iter().find(|p| {
        plain_name(&p.host_name) == name
            || plain_name(p.dns_name.split('.').next().unwrap_or_default()) == name
    })
}

/// Where this computer's tailnet says machine `name` is, on the port its routes use: what its
/// client also tries (`Client::also_try`). Only a guess by name: the client takes it only when
/// it answers with the machine's own certificate.
pub fn tailnet_routes(
    name: &str,
    routes: &[String],
    here: &crate::remote::Addresses,
) -> Vec<String> {
    let Some(peer) = here
        .tailnet
        .as_ref()
        .filter(|_| here.tailscale.is_some())
        .and_then(|t| tailnet_peer(name, t))
    else {
        return vec![];
    };
    let port = routes
        .first()
        .and_then(|r| r.rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(tr::DEFAULT_PORT);
    let mut ips = peer.ips.clone();
    ips.sort_by_key(|ip| ip.is_ipv6());
    ips.into_iter()
        .map(|ip| std::net::SocketAddr::new(ip, port).to_string())
        .collect()
}

/// Why machine `name`, which isn't connected, can't be reached from here, and what to do about
/// it, when Trek can tell: every route it knows is on a local network (none on the tailnet), or
/// this computer's own Tailscale is off, or the machine is offline in the tailnet. `this` is what
/// this computer is called ("this Mac").
pub fn reach_advice(
    name: &str,
    routes: &[String],
    here: &crate::remote::Addresses,
    this: &str,
) -> Option<String> {
    let tailscale_here = here.tailscale.is_some();
    let tailnet = here.tailnet.as_ref().filter(|_| tailscale_here);
    let peer = tailnet.and_then(|t| tailnet_peer(name, t));
    if peer.is_none() && !routes.iter().any(|r| tailnet_route(r)) {
        let account = tailnet.and_then(|t| t.account.as_deref());
        return Some(match (tailscale_here, account) {
            (true, Some(account)) => format!(
                "{name} is only reachable on its local network. Install Tailscale on it, signed in as {account} like {this}, to reach it from anywhere."
            ),
            (true, None) => format!(
                "{name} is only reachable on its local network. Install Tailscale on it, signed in to the same account as {this}, to reach it from anywhere."
            ),
            (false, _) => format!(
                "{name} is only reachable on its local network. Install Tailscale on it and on {this}, signed in to the same account, to reach it from anywhere."
            ),
        });
    }
    if !tailscale_here {
        return Some(format!(
            "{name} is on your tailnet, but Tailscale isn't on on {this}. Turn it on to reach {name} from here."
        ));
    }
    match peer {
        Some(p) if !p.online => Some(format!(
            "{name} is offline in your tailnet: it may be asleep or off, or Tailscale isn't on on it."
        )),
        _ => None,
    }
}

/// What the sidebar and Settings say of a machine that isn't connected: since when (or that it's
/// connecting), then why and what to do when Trek can tell (`reach_advice`). `None` while it's
/// connected.
pub fn offline_note(m: &Machine, here: &crate::remote::Addresses, this: &str) -> Option<String> {
    let since = match (&m.reach, m.offline_since.or(m.entry.last_seen)) {
        (Reach::Connected, _) => return None,
        (Reach::Stopped(why), _) => format!("Not connecting: {why}. Forget it and pair again."),
        (Reach::Connecting, Some(at)) => format!("Offline since {}", crate::time::clock(at)),
        (Reach::Connecting, None) => "Connecting…".to_string(),
    };
    let advice = match m.reach {
        Reach::Connecting => reach_advice(&m.entry.name, &m.entry.paired.routes, here, this),
        _ => None,
    };
    Some(match advice {
        Some(advice) if since.ends_with('…') => format!("{since} {advice}"),
        Some(advice) => format!("{since}. {advice}"),
        None => since,
    })
}

/// Look at this computer's network every few seconds while machines are paired: a change has
/// each of them dialled again at once, and the tailnet is asked where they are.
fn follow_network(cx: &mut Context<Workspace>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let mut seen: Option<crate::remote::AddressInputs> = None;
        let mut asked_at: Option<Instant> = None;
        loop {
            let now = cx
                .background_executor()
                .spawn(async { crate::remote::AddressInputs::read() })
                .await;
            let changed = seen.as_ref() != Some(&now);
            let Ok(waiting) = this.update(cx, |ws, _| {
                ws.machines
                    .list
                    .iter()
                    .any(|m| m.reach == Reach::Connecting)
            }) else {
                break;
            };
            let stale = asked_at.is_none_or(|at| at.elapsed() >= TAILNET_EVERY);
            if changed || (waiting && stale) {
                let first = seen.is_none();
                seen = Some(now.clone());
                asked_at = Some(Instant::now());
                let found = cx
                    .background_executor()
                    .spawn(async move { crate::remote::Addresses::find_in(&now) })
                    .await;
                let network_changed = changed && !first;
                if this
                    .update(cx, |ws, cx| ws.machines_network(found, network_changed, cx))
                    .is_err()
                {
                    break;
                }
            }
            cx.background_executor().timer(NETWORK_EVERY).await;
        }
    })
}

// ---------------------------------------------------------------------------------------------
// The workspace's side
// ---------------------------------------------------------------------------------------------

impl Workspace {
    /// At launch: the machines paired before, each connected again.
    pub fn start_machines(&mut self, cx: &mut Context<Self>) {
        let (device_id, entries) = load();
        self.machines.device_id = device_id;
        for entry in entries {
            self.add_machine(entry, None, cx);
        }
    }

    /// Watch this computer's network for the machines, once one is paired (not in tests: they
    /// say what the network is with `machines_network`).
    fn follow_machines_network(&mut self, cx: &mut Context<Self>) {
        if self.machines._network.is_none() && !cfg!(any(test, feature = "test-support")) {
            self.machines._network = Some(follow_network(cx));
        }
    }

    /// This computer's network as just found (`network_changed`: another than before): each
    /// machine's client is told where the tailnet says it is, and one that isn't connected (or
    /// may have lost its connection with the old network) dials again now.
    pub fn machines_network(
        &mut self,
        here: crate::remote::Addresses,
        network_changed: bool,
        cx: &mut Context<Self>,
    ) {
        if network_changed {
            tracing::info!(
                "machines: this computer's network changed (Wi-Fi {:?}, Tailscale {:?}); dialling them again",
                here.lan,
                here.tailscale
            );
        }
        for m in &mut self.machines.list {
            let Some(client) = m.client.clone() else {
                continue;
            };
            let hints = tailnet_routes(&m.entry.name, &m.entry.paired.routes, &here);
            let new_hints = hints != m.hints;
            if new_hints {
                tracing::info!(
                    machine = m.entry.name,
                    ?hints,
                    "machines: its tailnet address here"
                );
                client.also_try(hints.clone());
                m.hints = hints;
            }
            let waiting = m.reach == Reach::Connecting;
            if network_changed || (new_hints && waiting && !m.hints.is_empty()) {
                client.network_changed();
            }
        }
        self.machines.here = here;
        cx.notify();
    }

    /// Keep `entry` (in place of one with the same id) and connect to it: on `client` when
    /// pairing just made one, else with its token.
    fn add_machine(
        &mut self,
        entry: MachineEntry,
        client: Option<(Client, tokio::sync::mpsc::UnboundedReceiver<Event>)>,
        cx: &mut Context<Self>,
    ) {
        let host = entry.id().to_string();
        // The threads open here stay open: they're followed again on the new connection.
        let mut reopen: Vec<String> = vec![];
        if let Some(old) = self.machines.list.iter().position(|m| m.id() == host) {
            let old = self.machines.list.remove(old);
            if let Some(c) = &old.client {
                c.close();
            }
            reopen = old.followed.into_keys().collect();
        }
        let connected = client.is_some();
        let made = match client {
            Some(c) => Ok(c),
            None => Client::resume(&entry.paired).map(|c| {
                let events = c.events().expect("a new client's events");
                (c, events)
            }),
        };
        let mut machine = Machine {
            entry,
            reach: if connected {
                Reach::Connected
            } else {
                Reach::Connecting
            },
            offline_since: None,
            hints: vec![],
            client: None,
            info: None,
            projects: vec![],
            agents: vec![],
            rows: vec![],
            threads: vec![],
            followed: HashMap::new(),
            _pump: None,
        };
        match made {
            Ok((client, events)) => {
                let state = client.state();
                let client = Arc::new(client);
                machine._pump = Some(pump(host.clone(), events, state, cx));
                machine.client = Some(client.clone());
                if connected {
                    self.fetch_machine_info(&host, client, cx);
                }
            }
            Err(e) => machine.reach = Reach::Stopped(e.to_string()),
        }
        self.machines.list.push(machine);
        for tid in reopen {
            self.follow_machine_thread(&local_id(&host, &tid), cx);
        }
        self.follow_machines_network(cx);
        // What the tailnet says of it, as for the others.
        let here = self.machines.here.clone();
        self.machines_network(here, false, cx);
        cx.notify();
    }

    /// Ask machine `host` what it is (its home folder defaults where clones go).
    fn fetch_machine_info(&mut self, host: &str, client: Arc<Client>, cx: &mut Context<Self>) {
        if self
            .machines
            .get(host)
            .is_some_and(|m| m.info.is_some() || !m.entry.paired.serves(tr::feature::MACHINES))
        {
            return;
        }
        let host = host.to_string();
        cx.spawn(async move |this, cx| {
            let info = client.machine_info().await;
            let _ = this.update(cx, |ws, cx| match info {
                Ok(info) => {
                    if let Some(m) = ws.machines.get_mut(&host) {
                        m.info = Some(info);
                        cx.notify();
                    }
                }
                Err(e) => tracing::info!("machine {host}: machine_info: {e}"),
            });
        })
        .detach();
    }

    /// Something machine `host`'s connection brought.
    pub fn machine_event(&mut self, host: &str, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Connected { paired, .. } => {
                let Some(m) = self.machines.get_mut(host) else {
                    return;
                };
                m.reach = Reach::Connected;
                m.offline_since = None;
                m.refresh_threads();
                self.machine_threads_changed(host, cx);
                let Some(m) = self.machines.get_mut(host) else {
                    return;
                };
                m.entry.name = paired.host.name.clone();
                m.entry.paired = paired;
                m.entry.last_seen = Some(now_ms());
                let client = m.client.clone();
                self.machines.save();
                if let Some(client) = client {
                    self.fetch_machine_info(host, client, cx);
                }
            }
            Event::Disconnected { .. } => {
                let Some(m) = self.machines.get_mut(host) else {
                    return;
                };
                if m.reach == Reach::Connected {
                    m.entry.last_seen = Some(now_ms());
                    m.offline_since = Some(now_ms());
                    self.machines.save();
                }
                if let Some(m) = self.machines.get_mut(host) {
                    m.reach = Reach::Connecting;
                    m.refresh_threads();
                }
                self.machine_threads_changed(host, cx);
            }
            Event::Message(msg) => self.machine_message(host, msg, cx),
        }
        cx.notify();
    }

    /// Machine `host`'s connection ended for good.
    fn machine_stopped(&mut self, host: &str, state: State, cx: &mut Context<Self>) {
        if let Some(m) = self.machines.get_mut(host) {
            if m.reach == Reach::Connected {
                m.offline_since = Some(now_ms());
            }
            m.reach = match state {
                State::Stopped(reason) => Reach::Stopped(reason.to_string()),
                _ => Reach::Stopped("closed".into()),
            };
            m.refresh_threads();
            self.machine_threads_changed(host, cx);
            cx.notify();
        }
    }

    /// Machine `host`'s threads were mapped again (it came or went): the sidebar lists them
    /// anew, and the thread view's copies follow.
    fn machine_threads_changed(&mut self, host: &str, cx: &mut Context<Self>) {
        self.threads_gen += 1;
        let followed: Vec<String> = self
            .machines
            .get(host)
            .map(|m| m.followed.keys().cloned().collect())
            .unwrap_or_default();
        for tid in followed {
            self.machine_live(host, &tid, cx);
        }
    }

    fn machine_message(&mut self, host: &str, msg: tr::ServerMessage, cx: &mut Context<Self>) {
        let Some(m) = self.machines.get_mut(host) else {
            return;
        };
        let mut touched: Vec<String> = vec![];
        match msg {
            tr::ServerMessage::Snapshot(s) => {
                m.rows = s.threads;
                m.projects = s.projects;
                m.agents = s.agents;
                touched.extend(m.followed.keys().cloned());
            }
            tr::ServerMessage::Thread { thread } => {
                touched.push(thread.id.clone());
                match m.rows.iter_mut().find(|r| r.id == thread.id) {
                    Some(row) => *row = thread,
                    None => m.rows.push(thread),
                }
            }
            tr::ServerMessage::ThreadRemoved { thread_id } => {
                m.rows.retain(|r| r.id != thread_id);
                touched.push(thread_id);
            }
            tr::ServerMessage::Transcript {
                thread_id,
                reset,
                items,
                more,
                ..
            } => {
                let f = m.followed.entry(thread_id.clone()).or_default();
                if reset {
                    f.items = items;
                    f.more = more;
                } else {
                    for item in items {
                        upsert(&mut f.items, item);
                    }
                }
                f.loaded = true;
                touched.push(thread_id);
            }
            tr::ServerMessage::Item { thread_id, item } => {
                if let Some(f) = m.followed.get_mut(&thread_id) {
                    upsert(&mut f.items, item);
                    touched.push(thread_id);
                }
            }
            tr::ServerMessage::TranscriptReset { thread_id } => {
                if m.followed.remove(&thread_id).is_some() {
                    self.follow_machine_thread(&local_id(host, &thread_id), cx);
                }
                return;
            }
            tr::ServerMessage::CloneProgress { folder, line } => {
                if let Some(run) = self
                    .machines
                    .clone
                    .as_mut()
                    .filter(|r| r.host == host && r.folder == folder)
                {
                    run.lines.push(line);
                    let extra = run.lines.len().saturating_sub(200);
                    run.lines.drain(..extra);
                    cx.notify();
                }
                return;
            }
            _ => return,
        }
        m.refresh_threads();
        self.threads_gen += 1;
        for tid in touched {
            self.machine_live(host, &tid, cx);
        }
        cx.notify();
    }

    /// Bring the thread view's copy of a machine's thread (`live`) in line with what it sent.
    fn machine_live(&mut self, host: &str, tid: &str, cx: &mut Context<Self>) {
        let local = local_id(host, tid);
        let Some(m) = self.machines.get(host) else {
            return;
        };
        let Some(f) = m.followed.get(tid) else { return };
        let row = m.rows.iter().find(|r| r.id == tid);
        // A machine that can't be reached runs nothing that can be seen from here.
        let offline = !m.reachable();
        let working = !offline && row.is_some_and(|r| r.run_state == tr::RunState::Working);
        let since = row.and_then(|r| r.working_since);
        let plan = row.is_some_and(|r| r.plan);
        let context = row
            .and_then(|r| r.context.as_ref())
            .map(|c| (c.used, c.window));
        let mut mapped = map_items(&f.items);
        if offline {
            settle_running_tools(&mut mapped.rows);
        }
        let loaded = f.loaded;
        let live = self.live.entry(local.clone()).or_default();
        let reasoning = (working
            && matches!(mapped.rows.last(), Some((_, Item::Reasoning { .. }))))
        .then(|| mapped.rows.len() - 1);
        live.items = Transcript::stored(mapped.rows);
        live.permissions = mapped.requests;
        live.streaming = mapped.streaming.filter(|_| !offline);
        live.reasoning = reasoning;
        live.loaded = loaded;
        live.loading = !loaded;
        live.plan = plan;
        live.context = context;
        live.turn_started = match (working, live.turn_started) {
            (false, _) => None,
            (true, Some(at)) => Some(at),
            (true, None) => Some(since.map_or_else(Instant::now, |s| {
                Instant::now()
                    .checked_sub(Duration::from_millis((now_ms() - s).max(0) as u64))
                    .unwrap_or_else(Instant::now)
            })),
        };
        live.revision += 1;
        cx.emit(WorkspaceEvent::Transcript {
            id: local,
            appended: false,
        });
        cx.notify();
    }

    /// Open a machine's thread here: subscribe to its transcript (once; the connection subscribes
    /// again after a reconnect) and tell the machine it's been seen.
    pub fn follow_machine_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((host, tid)) = split_id(id).map(|(h, t)| (h.to_string(), t.to_string())) else {
            return;
        };
        let Some(m) = self.machines.get_mut(&host) else {
            return;
        };
        let Some(client) = m.client.clone() else {
            return;
        };
        if m.followed.contains_key(&tid) {
            return;
        }
        m.followed.insert(tid.clone(), Followed::default());
        let live = self.live.entry(id.to_string()).or_default();
        live.loading = !live.loaded;
        let local = id.to_string();
        cx.spawn(async move |this, cx| {
            let seen = client.request(tr::ClientMessage::MarkSeen {
                thread_id: tid.clone(),
            });
            let reply = client
                .request(tr::ClientMessage::Subscribe {
                    thread_id: tid.clone(),
                    after_seq: None,
                    limit: Some(OPEN_LIMIT),
                })
                .await;
            let _ = seen.await;
            let _ = this.update(cx, |ws, cx| match reply {
                Ok(msg @ tr::ServerMessage::Transcript { .. }) => {
                    ws.machine_message(&host, msg, cx)
                }
                other => {
                    let why = match other {
                        Err(e) => e.to_string(),
                        Ok(_) => "it answered with something else".into(),
                    };
                    if let Some(m) = ws.machines.get_mut(&host) {
                        m.followed.remove(&tid);
                    }
                    let live = ws.live.entry(local.clone()).or_default();
                    live.loading = false;
                    live.loaded = true;
                    live.items = Transcript::stored(vec![(
                        "error".into(),
                        Item::Error {
                            text: format!("Couldn't open this thread: {why}"),
                        },
                    )]);
                    live.revision += 1;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Why machine `host` can't be asked anything now (`None`: it can). A request made while it
    /// is out of reach would be held (the client has no timeout) and go whenever it's back, long
    /// after the person has forgotten it.
    fn machine_unavailable_why(&self, host: &str) -> Option<String> {
        let m = self.machines.get(host)?;
        let name = &m.entry.name;
        if m.client.is_none() {
            Some(format!("{name} isn't connected."))
        } else if !m.reachable() {
            Some(format!("{name} is offline. Try again when it's back."))
        } else {
            None
        }
    }

    /// `machine_unavailable_why`, said in a toast. True when it can't be asked (or isn't there).
    fn machine_unavailable(&mut self, host: &str, cx: &mut Context<Self>) -> bool {
        if self.machines.get(host).is_none() {
            return true;
        }
        let Some(message) = self.machine_unavailable_why(host) else {
            return false;
        };
        cx.emit(WorkspaceEvent::Toast {
            message,
            undo: None,
        });
        true
    }

    /// Send `msg` (made from the thread's id on its machine) for the machine's thread `id`; a
    /// refusal is said in a toast that names `what` was tried.
    fn machine_request(
        &mut self,
        id: &str,
        what: &'static str,
        msg: impl FnOnce(String) -> tr::ClientMessage,
        cx: &mut Context<Self>,
    ) {
        let Some((host, tid)) = split_id(id) else {
            return;
        };
        let Some(m) = self.machines.get(host) else {
            return;
        };
        let name = m.entry.name.clone();
        if self.machine_unavailable(host, cx) {
            return;
        }
        let Some(client) = self.machines.get(host).and_then(|m| m.client.clone()) else {
            return;
        };
        let msg = msg(tid.to_string());
        cx.spawn(async move |this, cx| {
            if let Err(e) = client.request(msg).await {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: format!("Couldn't {what} on {name}: {e}"),
                        undo: None,
                    })
                });
            }
        })
        .detach();
    }

    /// A message to a machine's thread (the composer of a thread on another machine).
    pub fn machine_send(
        &mut self,
        id: &str,
        text: String,
        images: Vec<PathBuf>,
        follow: Option<trek_core::settings::FollowUp>,
        cx: &mut Context<Self>,
    ) {
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        // Out of reach: the composer let go of the message as it was sent, so it goes back.
        if split_id(id).is_some_and(|(host, _)| self.machine_unavailable(host, cx)) {
            cx.emit(WorkspaceEvent::InsertIntoComposer(text));
            for image in images {
                cx.emit(WorkspaceEvent::AttachImage(image));
            }
            return;
        }
        let mode = follow.map(|f| match f {
            trek_core::settings::FollowUp::Steer => tr::SendMode::Steer,
            trek_core::settings::FollowUp::Queue => tr::SendMode::Queue,
        });
        let images = uploads(&images);
        self.machine_request(
            id,
            "send the message",
            move |thread_id| {
                tr::ClientMessage::Send(tr::SendRequest {
                    thread_id,
                    text,
                    mode,
                    images,
                })
            },
            cx,
        );
    }

    pub fn machine_interrupt(&mut self, id: &str, cx: &mut Context<Self>) {
        self.machine_request(
            id,
            "stop it",
            |thread_id| tr::ClientMessage::Interrupt { thread_id },
            cx,
        );
    }

    /// Answer a card in a machine's thread; it leaves the screen at once (the machine says so too).
    pub fn machine_answer(
        &mut self,
        id: &str,
        request_id: &str,
        response: tr::AnswerResponse,
        cx: &mut Context<Self>,
    ) {
        // Out of reach, the card stays: answered now, it would be sent whenever the machine is back.
        if split_id(id).is_some_and(|(host, _)| self.machine_unavailable(host, cx)) {
            return;
        }
        if let Some(live) = self.live.get_mut(id) {
            live.permissions.retain(|p| p.request_id != request_id);
            live.revision += 1;
        }
        let request_id = request_id.to_string();
        self.machine_request(
            id,
            "answer",
            move |thread_id| {
                tr::ClientMessage::Answer(tr::AnswerRequest {
                    thread_id,
                    request_id,
                    response,
                })
            },
            cx,
        );
        cx.notify();
    }

    /// `respond` in a machine's thread: an approval's decision, or a plan kept planning, or
    /// questions skipped.
    pub fn machine_respond(
        &mut self,
        id: &str,
        request_id: &str,
        decision: trek_agents::Decision,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .live
            .get(id)
            .and_then(|l| l.permissions.iter().find(|p| p.request_id == request_id))
            .map(|p| p.prompt.clone());
        let response = match prompt.flatten() {
            Some(trek_agents::Prompt::Plan(_)) => tr::AnswerResponse::Plan {
                approve: decision != trek_agents::Decision::Deny,
                feedback: None,
            },
            Some(trek_agents::Prompt::Questions(_)) => {
                tr::AnswerResponse::Questions { answers: vec![] }
            }
            None => tr::AnswerResponse::Approval {
                decision: match decision {
                    trek_agents::Decision::Allow => tr::Decision::Allow,
                    trek_agents::Decision::AllowForSession => tr::Decision::AllowForSession,
                    trek_agents::Decision::Deny => tr::Decision::Deny,
                },
            },
        };
        self.machine_answer(id, request_id, response, cx);
    }

    /// The composer's model, effort, access or plan changed on a machine's thread: what changed
    /// goes to the machine (it may refuse an agent or model it doesn't have).
    pub fn machine_set_prefs(
        &mut self,
        id: &str,
        prefs: crate::workspace::Prefs,
        cx: &mut Context<Self>,
    ) {
        let Some(before) = self.thread(id).cloned() else {
            return;
        };
        let plan_before = self.live.get(id).is_some_and(|l| l.plan);
        let req = |thread_id: String| {
            tr::ClientMessage::SetPrefs(tr::PrefsRequest {
                thread_id,
                agent: (prefs.agent != before.agent).then(|| prefs.agent.key()),
                model: (prefs.model != before.model)
                    .then(|| prefs.model.clone())
                    .flatten(),
                effort: (prefs.effort != before.effort).then(|| prefs.effort.as_str().to_string()),
                access: (prefs.hand_holding != before.hand_holding)
                    .then(|| access_of(prefs.hand_holding)),
                plan: (prefs.plan != plan_before).then_some(prefs.plan),
            })
        };
        // Shown at once; the machine's row confirms it (or puts it back).
        if let Some((host, _)) = split_id(id)
            && let Some(t) = self
                .machines
                .get_mut(host)
                .and_then(|m| m.threads.iter_mut().find(|t| t.id == id))
        {
            t.agent = prefs.agent.clone();
            t.model = prefs.model.clone();
            t.effort = prefs.effort;
            t.hand_holding = prefs.hand_holding;
        }
        if let Some(live) = self.live.get_mut(id) {
            live.plan = prefs.plan;
        }
        self.machine_request(id, "change it", req, cx);
        cx.notify();
    }

    /// Pin, settle, archive or rename a machine's thread.
    pub fn machine_thread_action(
        &mut self,
        id: &str,
        action: tr::ThreadAction,
        cx: &mut Context<Self>,
    ) {
        self.machine_request(
            id,
            "do that",
            move |thread_id| {
                tr::ClientMessage::ThreadAction(tr::ThreadActionRequest { thread_id, action })
            },
            cx,
        );
    }

    /// The machine the main window's draft in `project` (none: no project) starts its thread on
    /// (`None`: this Mac).
    pub fn draft_machine(&self, project: Option<&Path>) -> Option<String> {
        match project {
            Some(project) => self.project_prefs(project).machine,
            None => self.draft_machine_unfiled.clone(),
        }
        .filter(|m| self.machines.get(m).is_some())
    }

    /// Where new threads in `project` (none: no project) start: on machine `host`, or on this
    /// Mac (`None`).
    pub fn set_draft_machine(
        &mut self,
        project: Option<&Path>,
        host: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match project {
            Some(project) => self.update_project_prefs(project, |p| p.machine = host, cx),
            None => self.draft_machine_unfiled = host,
        }
        cx.notify();
    }

    /// Start a thread on machine `host` in its copy of `project` (none: a thread in no project
    /// there), with the draft's choices and `text`, and open it. Said in a toast (and the message goes back to the composer) when the
    /// machine has no copy of the project or refuses.
    pub fn machine_start_thread(
        &mut self,
        host: &str,
        project: Option<PathBuf>,
        text: String,
        images: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let prefs = self.draft_prefs.clone();
        self.start_on_machine(host, project, text, images, prefs, true, cx)
    }

    /// Continue thread `id` on machine `host`: a new thread there whose first message is
    /// `markdown` (the conversation so far), framed as coming from this computer, with the same
    /// agent and model when the machine has them, in its copy of the project when it has one
    /// (else in no project). Said in a toast when the machine can't take it.
    pub fn continue_on_machine(
        &mut self,
        host: &str,
        id: &str,
        markdown: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(m) = self.machines.get(host) else {
            return;
        };
        let project = self
            .thread(id)
            .and_then(|t| self.project_dir(t))
            .filter(|dir| {
                let name = self
                    .projects
                    .iter()
                    .find(|p| &p.path == dir)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                m.project_for(dir, &name).is_some()
            });
        let mut prefs = self.prefs_in(&Scope::Thread(id.to_string()));
        prefs.worktree = false;
        prefs.plan = false;
        let text = continuing_text(&crate::remote::host_info().name, markdown);
        self.start_on_machine(host, project, text, vec![], prefs, false, cx)
    }

    /// `machine_start_thread` with `prefs` for the agent, model, effort and access; `give_back`:
    /// a message that couldn't start goes back to the composer (one typed there).
    #[allow(clippy::too_many_arguments)]
    fn start_on_machine(
        &mut self,
        host: &str,
        project: Option<PathBuf>,
        text: String,
        images: Vec<PathBuf>,
        prefs: crate::workspace::Prefs,
        give_back: bool,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            return;
        }
        let give_back = move |ws: &mut Self, why: String, text: String, cx: &mut Context<Self>| {
            cx.emit(WorkspaceEvent::Toast {
                message: why,
                undo: None,
            });
            if give_back {
                cx.emit(WorkspaceEvent::InsertIntoComposer(text));
            }
            let _ = ws;
        };
        let Some(m) = self.machines.get(host) else {
            return give_back(self, "That machine isn't paired any more.".into(), text, cx);
        };
        let machine = m.entry.name.clone();
        if let Some(why) = self.machine_unavailable_why(host) {
            return give_back(self, why, text, cx);
        }
        let Some(client) = m.client.clone() else {
            return;
        };
        // No project: the machine's own no-project thread (its home folder), named by its id "".
        let (name, project_id) = match &project {
            None => (String::new(), String::new()),
            Some(project) => {
                let name = self
                    .projects
                    .iter()
                    .find(|p| &p.path == project)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| {
                        project
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    });
                let Some(project_id) = m.project_for(project, &name) else {
                    return give_back(
                        self,
                        format!(
                            "{name} isn't on {machine} yet. Choose “Add to another machine…” to clone it there."
                        ),
                        text,
                        cx,
                    );
                };
                (name, project_id)
            }
        };
        // The draft's agent and model when the machine has them; its default agent otherwise.
        let agent = m
            .agents
            .iter()
            .find(|a| a.key == prefs.agent.key())
            .or_else(|| m.agents.first());
        let model = prefs.model.clone().filter(|id| {
            agent.is_some_and(|a| {
                a.models
                    .iter()
                    .any(|m| crate::composer::same_model(id, &m.id))
            })
        });
        let agent_key = agent
            .map(|a| a.key.clone())
            .unwrap_or_else(|| prefs.agent.key());
        let agent_ref = tr::AgentRef::new(
            agent_key.clone(),
            agent
                .map(|a| a.name.clone())
                .unwrap_or_else(|| prefs.agent.display_name()),
            agent.and_then(|a| a.logo.as_deref()),
        );
        let req = tr::NewThreadRequest {
            project_id: project_id.clone(),
            agent: agent_key,
            model: model.clone(),
            text: text.clone(),
            // A thread in no project has no repository to make a worktree of.
            worktree: prefs.worktree && project.is_some(),
            effort: Some(prefs.effort.as_str().to_string()),
            access: Some(access_of(prefs.hand_holding)),
            plan: prefs.plan,
            images: uploads(&images),
        };
        let host = host.to_string();
        cx.spawn(async move |this, cx| {
            let reply = client.request(tr::ClientMessage::NewThread(req)).await;
            let _ = this.update(cx, |ws, cx| match reply {
                Ok(tr::ServerMessage::Ack {
                    thread_id: Some(tid),
                    ..
                }) => {
                    // Its row may come after the reply: one to show until it does.
                    if let Some(m) = ws.machines.get_mut(&host)
                        && !m.rows.iter().any(|r| r.id == tid)
                    {
                        m.rows.push(tr::ThreadSummary {
                            id: tid.clone(),
                            title: trek_core::import_title(&text),
                            project: tr::ProjectRef {
                                id: project_id,
                                name: name.clone(),
                                ..Default::default()
                            },
                            agent: agent_ref,
                            model,
                            run_state: tr::RunState::Working,
                            section: tr::Section::Working,
                            updated_at: now_ms(),
                            working_since: Some(now_ms()),
                            ..Default::default()
                        });
                        m.threads = m.rows.iter().map(|r| thread_of(&host, r)).collect();
                        ws.threads_gen += 1;
                    }
                    ws.navigate(Route::Thread(local_id(&host, &tid)), cx);
                }
                Ok(other) => give_back(
                    ws,
                    format!("{machine} didn't start the thread ({})", tr_kind(&other)),
                    text,
                    cx,
                ),
                Err(e) => give_back(
                    ws,
                    format!("Couldn't start the thread on {machine}: {e}"),
                    text,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Pair with the machine `link` points at; `machines.pairing` says how it went.
    pub fn pair_machine(&mut self, link: PairLink, cx: &mut Context<Self>) {
        if self.machines.pairing == Some(Pairing::Busy) {
            return;
        }
        let auth = Auth::Pair {
            code: link.code.clone(),
            device_id: self.machines.device_id.clone(),
            device_name: this_name(),
        };
        let client = match Client::connect(link.routes.clone(), &link.fingerprint, auth) {
            Ok(c) => c,
            Err(e) => {
                self.machines.pairing = Some(Pairing::Failed(e.to_string()));
                cx.notify();
                return;
            }
        };
        let Some(events) = client.events() else {
            return;
        };
        self.machines.pairing = Some(Pairing::Busy);
        cx.notify();
        let state = client.state();
        let first = link.routes.first().cloned().unwrap_or_default();
        cx.spawn(async move |this, cx| {
            // The first word from the machine: in (with its token), or why not. A machine that
            // never answers is given up on.
            let waited = trek_core::runtime()
                .spawn(async move {
                    let mut events = events;
                    let outcome = tokio::time::timeout(PAIR_WAIT, async {
                        loop {
                            match events.recv().await {
                                Some(Event::Connected { paired, .. }) => return Some(paired),
                                Some(_) => continue,
                                None => return None,
                            }
                        }
                    })
                    .await;
                    (outcome, events)
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                let (outcome, events) = match waited {
                    Ok(w) => w,
                    Err(e) => {
                        client.close();
                        ws.machines.pairing = Some(Pairing::Failed(e.to_string()));
                        cx.notify();
                        return;
                    }
                };
                match outcome {
                    Ok(Some(paired)) => {
                        let name = paired.host.name.clone();
                        // A machine paired again keeps the projects it was known to have.
                        let projects = ws.machines.get(&paired.host.id).map(|m| m.entry.projects.clone()).unwrap_or_default();
                        let entry = MachineEntry { paired, name: name.clone(), last_seen: Some(now_ms()), projects };
                        ws.add_machine(entry, Some((client, events)), cx);
                        ws.machines.save();
                        ws.machines.pairing = Some(Pairing::Done(name.clone()));
                        cx.emit(WorkspaceEvent::Toast { message: format!("Paired with {name}"), undo: None });
                    }
                    Ok(None) => {
                        let why = match state.borrow().clone() {
                            State::Stopped(reason) => reason.to_string(),
                            _ => "the connection closed".into(),
                        };
                        ws.machines.pairing = Some(Pairing::Failed(why));
                    }
                    Err(_) => {
                        client.close();
                        ws.machines.pairing = Some(Pairing::Failed(format!("No answer from {first}. Is Trek's phone server on there, and this {} on the same network or tailnet?", crate::words::words().computer)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Forget machine `host`: its connection closes, its threads go from here, and it's out of
    /// `machines.json`. (The machine still lists this Mac among its devices until it's unpaired there.)
    pub fn forget_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.machines.list.iter().position(|m| m.id() == host) else {
            return;
        };
        let m = self.machines.list.remove(ix);
        if let Some(c) = &m.client {
            c.close();
        }
        let prefix = local_id(host, "");
        self.live.retain(|id, _| !id.starts_with(&prefix));
        if matches!(&self.route, Route::Thread(id) if id.starts_with(&prefix)) {
            self.route = Route::Draft { project: None };
        }
        for prefs in self.settings.projects.values_mut() {
            if prefs.machine.as_deref() == Some(host) {
                prefs.machine = None;
            }
        }
        self.save_settings(cx);
        self.threads_gen += 1;
        self.machines.save();
        cx.notify();
    }

    /// Clone this Mac's `project` to machine `host` (`url` into `folder` there), copy `files`
    /// (paths in the project) into the clone, then start new threads in it there.
    pub fn clone_to_machine(
        &mut self,
        host: &str,
        project: PathBuf,
        url: String,
        folder: String,
        files: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(m) = self.machines.get(host) else {
            return;
        };
        // Out of reach, a clone would wait for the machine, "Cloning…" for as long as that takes,
        // and start whenever it came back.
        if let Some(why) = self.machine_unavailable_why(host) {
            self.machines.clone = Some(CloneRun {
                host: host.into(),
                folder,
                lines: vec![],
                stage: CloneStage::Failed(why),
            });
            cx.notify();
            return;
        }
        let Some(client) = m.client.clone() else {
            return;
        };
        let name = self
            .projects
            .iter()
            .find(|p| p.path == project)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| {
                project
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        self.machines.clone = Some(CloneRun {
            host: host.into(),
            folder: folder.clone(),
            lines: vec![],
            stage: CloneStage::Cloning,
        });
        cx.notify();
        let host = host.to_string();
        let run_folder = folder.clone();
        cx.spawn(async move |this, cx| {
            // Another clone may have taken the sheet's place by now: what this one does to the
            // run (`clone_run`) is only said while it's still the one shown.
            let fail = |this: &gpui_kit::WeakEntity<Workspace>,
                        cx: &mut gpui_kit::AsyncApp,
                        why: String| {
                let _ = this.update(cx, |ws, cx| {
                    if let Some(run) = ws.machines.clone_run(&host, &run_folder) {
                        run.stage = CloneStage::Failed(why);
                    }
                    cx.notify();
                });
            };
            let project_id = match client
                .clone_project(tr::CloneProjectRequest { url, folder, name })
                .await
            {
                Ok(id) => id,
                Err(e) => return fail(&this, cx, e.to_string()),
            };
            if !files.is_empty() {
                let _ = this.update(cx, |ws, cx| {
                    if let Some(run) = ws.machines.clone_run(&host, &run_folder) {
                        run.stage = CloneStage::Copying { files: files.len() };
                    }
                    cx.notify();
                });
                let root = project.clone();
                let read = cx
                    .background_executor()
                    .spawn(async move { clone::read_uploads(&root, &files) })
                    .await;
                let uploads = match read {
                    Ok(u) => u,
                    Err(e) => {
                        return fail(
                            &this,
                            cx,
                            format!("Cloned, but the ignored files couldn't be read: {e}"),
                        );
                    }
                };
                if let Err(e) = client.put_files(&project_id, &uploads).await {
                    return fail(
                        &this,
                        cx,
                        format!("Cloned, but copying the ignored files failed: {e}"),
                    );
                }
            }
            let _ = this.update(cx, |ws, cx| {
                let Some(m) = ws.machines.get_mut(&host) else {
                    // Forgotten while it cloned: nothing to start there.
                    if let Some(run) = ws.machines.clone_run(&host, &run_folder) {
                        run.stage = CloneStage::Failed(
                            "That machine was forgotten here before the clone finished.".into(),
                        );
                    }
                    cx.notify();
                    return;
                };
                m.entry
                    .projects
                    .insert(project.display().to_string(), project_id.clone());
                ws.machines.save();
                if let Some(run) = ws.machines.clone_run(&host, &run_folder) {
                    run.stage = CloneStage::Done { project_id };
                }
                ws.set_draft_machine(Some(&project), Some(host.clone()), cx);
                // What's typed in the draft goes there now: only if that is still the draft on
                // screen, not whatever else is being typed by the time a clone is done.
                if ws.route
                    == (Route::Draft {
                        project: Some(project),
                    })
                {
                    cx.emit(WorkspaceEvent::SendDraft);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// What can't be done to a thread on another machine yet (rewind, fork, retry, delete,
    /// snooze): said in a toast. True when `id` is such a thread.
    pub fn machine_refuses(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some((host, _)) = split_id(id) else {
            return false;
        };
        let name = self
            .machines
            .get(host)
            .map_or_else(|| "the other machine".to_string(), |m| m.entry.name.clone());
        cx.emit(WorkspaceEvent::Toast { message: format!("Do that in Trek on {name}. From here, its threads can be followed, sent to, stopped, answered, pinned, settled and archived."), undo: None });
        true
    }

    /// The machine's home folder, as it said (`machine_info`).
    pub fn machine_home(&self, host: &str) -> Option<String> {
        self.machines
            .get(host)
            .and_then(|m| m.info.as_ref())
            .map(|i| i.home.clone())
    }

    /// Whether `scope` shows a thread living on another machine.
    pub fn on_machine_in(&self, scope: &Scope) -> Option<String> {
        let id = self.thread_id_in(scope)?;
        let (host, _) = split_id(id)?;
        Some(
            self.machines
                .get(host)
                .map_or_else(|| host.to_string(), |m| m.entry.name.clone()),
        )
    }
}

/// Machine `host`'s events, handed to the workspace as they come; then why it stopped.
fn pump(
    host: String,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    state: tokio::sync::watch::Receiver<State>,
    cx: &mut Context<Workspace>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        while let Some(event) = events.recv().await {
            if this
                .update(cx, |ws, cx| ws.machine_event(&host, event, cx))
                .is_err()
            {
                return;
            }
        }
        let state = state.borrow().clone();
        let _ = this.update(cx, |ws, cx| ws.machine_stopped(&host, state, cx));
    })
}

/// Images attached to a message, as the machine takes them (JPEG and PNG; others are left out).
fn uploads(images: &[PathBuf]) -> Vec<tr::ImageUpload> {
    images
        .iter()
        .filter_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
                "image/jpeg"
            } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                "image/png"
            } else {
                return None;
            };
            Some(tr::ImageUpload {
                mime: mime.into(),
                data: tr::base64(&bytes),
            })
        })
        .collect()
}

fn tr_kind(msg: &tr::ServerMessage) -> String {
    serde_json::to_value(msg)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_running_when_its_machine_went_away_shows_as_ended() {
        let tool = |id: &str, status| {
            (
                id.to_string(),
                Item::Tool {
                    id: id.into(),
                    title: "Bash".into(),
                    detail: String::new(),
                    output: String::new(),
                    status,
                },
            )
        };
        let mut rows = vec![
            tool("a", ToolStatus::Running),
            tool("b", ToolStatus::Done),
            (
                "n".into(),
                Item::Notice {
                    text: "hello".into(),
                },
            ),
        ];
        settle_running_tools(&mut rows);
        let statuses: Vec<_> = rows
            .iter()
            .filter_map(|(_, i)| match i {
                Item::Tool { status, .. } => Some(*status),
                _ => None,
            })
            .collect();
        assert_eq!(statuses, [ToolStatus::Failed, ToolStatus::Done]);
    }

    #[cfg(unix)]
    #[test]
    fn machines_json_is_private_from_the_moment_it_exists() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("trek-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machines.json.tmp");
        // One left by an earlier run, readable by everyone, is replaced rather than reused.
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, "token").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "token");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn local_ids_round_trip() {
        let id = local_id("7f3c", "019a-thread");
        assert!(is_remote(&id));
        assert_eq!(split_id(&id), Some(("7f3c", "019a-thread")));
        assert_eq!(split_id("019a-thread"), None);
    }
}
