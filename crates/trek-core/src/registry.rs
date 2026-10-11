//! Agents the user adds beyond the built-in ones: from the ACP Registry
//! (agentclientprotocol.com/registry, a public list of ACP agents and how to get each), or a
//! command of their own. Either way they run as `AgentId::Acp(id)` agents like the built-in ones;
//! `catalog::set_added_agents` makes them known to every part of Trek.
//!
//! The registry is fetched on demand and cached in the data folder. Installing an agent prefers
//! its macOS binary (downloaded, checked against its checksum when the registry gives one, and
//! unpacked under `agents/<id>/<version>/`), then an npm package run by `npx`, then a Python one
//! run by `uvx`. Package versions must be pinned: what runs is what was listed.

use crate::types::AgentId;
use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The registry's index is a few dozen KiB; anything near this is not it.
const MAX_REGISTRY_BYTES: usize = 1 << 20;
const MAX_ICON_BYTES: usize = 64 << 10;
const MAX_ARCHIVE_BYTES: u64 = 1 << 30;
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(30);
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// `npx` fetching a package and its dependencies the first time.
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How old the cached index may be before opening the sheet fetches it again.
pub const STALE_AFTER_MS: i64 = 6 * 60 * 60 * 1000;

/// Points Trek at a registry file instead of the network (the shots harness and design reviews).
/// Icons are read from an `icons` folder beside it, as `<id>.svg`.
const FIXTURE_ENV: &str = "TREK_ACP_REGISTRY";

// ---------------------------------------------------------------------------------------------
// The index

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry {
    pub version: String,
    pub agents: Vec<RegistryAgent>,
    /// When it was fetched (ms): the cache file's age.
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub distribution: Distribution,
    /// The icon in Trek's cache, once it's been fetched.
    #[serde(skip)]
    pub icon_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    /// By target: `darwin-aarch64`, `darwin-x86_64`, `linux-…`, `windows-…`.
    #[serde(default)]
    pub binary: BTreeMap<String, BinaryTarget>,
    #[serde(default)]
    pub npx: Option<Package>,
    #[serde(default)]
    pub uvx: Option<Package>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryTarget {
    pub archive: String,
    /// The executable, relative to the unpacked archive (`./bin/devin`).
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl RegistryAgent {
    /// Where to read more: its website, else its repository.
    pub fn link(&self) -> Option<&str> {
        self.website.as_deref().or(self.repository.as_deref())
    }

    /// Lower ranks match `query` better; `None` doesn't match. Name and id first, then authors,
    /// then the description.
    pub fn rank(&self, query: &str) -> Option<u8> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Some(100);
        }
        let (id, name) = (self.id.to_lowercase(), self.name.to_lowercase());
        let authors = self.authors.join(" ").to_lowercase();
        let all = format!("{id} {name} {authors} {}", self.description.to_lowercase());
        let terms: Vec<&str> = q.split_whitespace().collect();
        if id == q || name == q {
            Some(0)
        } else if id.starts_with(&q) || name.starts_with(&q) {
            Some(10)
        } else if terms.iter().all(|t| id.contains(t) || name.contains(t)) {
            Some(20)
        } else if terms.iter().all(|t| authors.contains(t)) {
            Some(40)
        } else if terms.iter().all(|t| all.contains(t)) {
            Some(50)
        } else {
            None
        }
    }
}

impl Registry {
    /// The agents matching `query`, best first (registry order among equals).
    pub fn search(&self, query: &str) -> Vec<&RegistryAgent> {
        let mut hits: Vec<(u8, usize, &RegistryAgent)> = self.agents.iter().enumerate().filter_map(|(i, a)| a.rank(query).map(|r| (r, i, a))).collect();
        hits.sort_by_key(|(r, i, _)| (*r, *i));
        hits.into_iter().map(|(_, _, a)| a).collect()
    }

    pub fn agent(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|a| a.id == id)
    }
}

/// Read the registry's index. Entries this build can't use (a malformed id, a missing name) are
/// left out one by one rather than failing the whole list; links that aren't plain https are
/// dropped from the entry that has them.
pub fn parse(text: &str) -> Result<Registry> {
    #[derive(Deserialize)]
    struct Envelope {
        version: String,
        agents: Vec<serde_json::Value>,
    }
    let envelope: Envelope = serde_json::from_str(text).context("The ACP Registry sent something that isn't its index")?;
    let mut skipped = 0;
    let agents: Vec<RegistryAgent> = envelope
        .agents
        .into_iter()
        .take(512)
        .filter_map(|v| {
            let a = serde_json::from_value::<RegistryAgent>(v).ok().and_then(checked);
            skipped += a.is_none() as usize;
            a
        })
        .collect();
    if skipped > 0 {
        tracing::debug!("left out {skipped} ACP Registry entries this build can't read");
    }
    Ok(Registry { version: envelope.version, agents, fetched_at: 0 })
}

fn checked(mut a: RegistryAgent) -> Option<RegistryAgent> {
    a.name = a.name.trim().to_string();
    if !valid_id(&a.id) || a.name.is_empty() || a.name.len() > 160 || !valid_version(&a.version) {
        return None;
    }
    a.description = a.description.trim().chars().take(1024).collect();
    for link in [&mut a.website, &mut a.repository, &mut a.icon] {
        if link.as_deref().is_some_and(|l| !https(l)) {
            *link = None;
        }
    }
    a.distribution.binary.retain(|_, t| https(&t.archive) && command_segments(&t.cmd).is_some() && t.sha256.as_deref().is_none_or(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())));
    a.distribution.npx = a.distribution.npx.take().filter(|p| pinned_npx(&p.package));
    a.distribution.uvx = a.distribution.uvx.take().filter(|p| pinned_uvx(&p.package));
    Some(a)
}

fn https(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !host.is_empty() && !host.contains('@') && !url.contains(char::is_whitespace)
    })
}

/// A registry id, and the id of an agent the user adds: `^[a-z0-9][a-z0-9._-]*$`, at most 128.
pub fn valid_id(id: &str) -> bool {
    id.len() <= 128
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn valid_version(v: &str) -> bool {
    v.len() <= 128 && v.starts_with(|c: char| c.is_ascii_alphanumeric()) && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// An exact version, as a package must be pinned to: `1.2.3`, `v1.2.3-beta.1+build`.
fn exact_version(v: &str) -> bool {
    let v = v.strip_prefix('v').unwrap_or(v);
    let (core, rest) = v.split_once(['-', '+']).map_or((v, ""), |(c, _)| (c, &v[c.len()..]));
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        && (rest.is_empty() || rest.len() > 1)
}

/// `name@1.2.3` or `@scope/name@1.2.3`: `npx -y` would otherwise run whatever is newest.
pub fn pinned_npx(package: &str) -> bool {
    let Some((name, version)) = package.rsplit_once('@') else { return false };
    let name_ok = match name.strip_prefix('@') {
        Some(scoped) => scoped.split_once('/').is_some_and(|(s, n)| !s.is_empty() && !n.is_empty() && !n.contains('/')),
        None => !name.is_empty() && !name.contains('/'),
    };
    name_ok && !name.contains(char::is_whitespace) && exact_version(version)
}

/// `name==1.2.3` or `name@1.2.3`.
pub fn pinned_uvx(package: &str) -> bool {
    let split = package.rsplit_once("==").or_else(|| package.rsplit_once('@'));
    split.is_some_and(|(name, version)| {
        name.starts_with(|c: char| c.is_ascii_alphanumeric()) && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) && exact_version(version)
    })
}

/// A binary's command as path segments inside its install folder, or `None` if it would reach
/// outside it (absolute, a drive letter, `..`).
pub fn command_segments(cmd: &str) -> Option<Vec<String>> {
    let normal = cmd.trim().replace('\\', "/");
    let normal = normal.strip_prefix("./").unwrap_or(&normal);
    let segments: Vec<String> = normal.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
    let drive = normal.len() >= 2 && normal.as_bytes()[1] == b':';
    if segments.is_empty() || normal.starts_with('/') || drive || segments.iter().any(|s| s == "." || s == "..") {
        return None;
    }
    Some(segments)
}

/// Registry ids of agents Trek has built in, and the agent each one is. Claude and Codex are in
/// the registry as ACP adapters; Trek runs their own CLIs instead.
pub fn built_in(registry_id: &str) -> Option<AgentId> {
    Some(match registry_id {
        "claude-acp" => AgentId::ClaudeCode,
        "codex-acp" => AgentId::Codex,
        "opencode" => AgentId::OpenCode,
        "factory-droid" => AgentId::Droid,
        "github-copilot-cli" => AgentId::Acp("github-copilot".into()),
        "grok-build" => AgentId::Acp("grok".into()),
        "amp-acp" => AgentId::Acp("amp".into()),
        "pi-acp" => AgentId::Acp("pi".into()),
        id if crate::catalog::ACP_AGENTS.iter().any(|a| a.id == id) => AgentId::Acp(id.into()),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// How to get it

/// This Mac, as the registry names targets.
pub fn current_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("darwin-aarch64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Binary(BinaryTarget),
    Npx(Package),
    Uvx(Package),
}

impl Plan {
    pub fn kind(&self) -> &'static str {
        match self {
            Plan::Binary(_) => "binary",
            Plan::Npx(_) => "npx",
            Plan::Uvx(_) => "uvx",
        }
    }
}

/// How `agent` gets onto `target`: its own build first (nothing else needed), then npm, then
/// Python. `None` when it offers none of them for this machine.
pub fn plan(agent: &RegistryAgent, target: Option<&str>) -> Option<Plan> {
    let d = &agent.distribution;
    target
        .and_then(|t| d.binary.get(t))
        .cloned()
        .map(Plan::Binary)
        .or_else(|| d.npx.clone().map(Plan::Npx))
        .or_else(|| d.uvx.clone().map(Plan::Uvx))
}

// ---------------------------------------------------------------------------------------------
// Agents the user added

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSource {
    /// Installed from the ACP Registry.
    Registry,
    /// The user's own command.
    #[default]
    Command,
}

/// An environment variable an added agent starts with. A secret one's value is in the Keychain,
/// not in settings.toml.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

/// An agent the user added: what to run and how it shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddedAgent {
    /// Its id as `AgentId::Acp(id)`: the registry's id, or one made from the name.
    pub id: String,
    pub name: String,
    /// What runs: a name looked up on PATH (`npx`, `my-agent`) or a full path.
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVar>,
    pub source: AgentSource,
    /// The registry version installed.
    pub version: Option<String>,
    /// How a registry agent was installed: `binary`, `npx` or `uvx`.
    pub distribution: Option<String>,
    pub description: String,
    pub website: Option<String>,
    /// Its monochrome SVG icon, kept beside its install.
    pub icon: Option<String>,
}

/// Ids an added agent can't take: the built-in agents' own.
fn reserved(id: &str) -> bool {
    matches!(id, "claude-code" | "codex" | "opencode" | "droid" | "mock" | "mock-relay") || crate::catalog::ACP_AGENTS.iter().any(|a| a.id == id)
}

/// An id from a name: "My Agent 2" → "my-agent-2".
pub fn id_from_name(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            id.push(c);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').trim_start_matches(['.', '_']).chars().take(64).collect()
}

/// What follows `~/` (or on Windows `~\`) at the start of `command`.
fn home_relative(command: &str, windows: bool) -> Option<&str> {
    command.strip_prefix("~/").or_else(|| command.strip_prefix("~\\").filter(|_| windows))
}

/// Whether `command` is a full path on this platform: `/opt/x/agent`; on Windows a drive (`C:\x`,
/// `D:/x`) or a network share (`\\server\share\x`). A rooted path with no drive (`\x`) isn't.
fn is_full_path(command: &str, windows: bool) -> bool {
    if !windows {
        return command.starts_with('/');
    }
    let b = command.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive || command.starts_with(r"\\") || command.starts_with("//")
}

/// A program is its name on the PATH or a full path, not a path from somewhere (`./agent`,
/// `.\agent.exe`, `tools/agent`) that depends on where Trek was started.
fn check_command(command: &str, windows: bool) -> Result<()> {
    if crate::detect::is_bare_name_on(command, windows) || is_full_path(command, windows) || home_relative(command, windows).is_some() {
        return Ok(());
    }
    bail!("Give the program's full path, or just its name if it's on your PATH.");
}

/// Check what the user typed for an agent of their own and make it one. `id` is optional (made
/// from the name); `taken` are the ids already in use.
pub fn custom(name: &str, id: &str, command: &str, args: Vec<String>, env: Vec<EnvVar>, taken: &[String]) -> Result<AddedAgent> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Give the agent a name.");
    }
    let id = if id.trim().is_empty() { id_from_name(name) } else { id.trim().to_string() };
    if !valid_id(&id) {
        bail!("The id may use lowercase letters, digits, dots, dashes and underscores, starting with a letter or digit.");
    }
    if reserved(&id) {
        bail!("{id} is the id of an agent Trek has built in. Choose another.");
    }
    if taken.contains(&id) {
        bail!("You already have an agent with the id {id}.");
    }
    let command = command.trim();
    if command.is_empty() {
        bail!("Say which program starts the agent: its name on your PATH, or its full path.");
    }
    check_command(command, cfg!(windows))?;
    let mut seen = std::collections::HashSet::new();
    for v in &env {
        let ok = v.name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && v.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            bail!("{:?} isn't a valid environment variable name.", v.name);
        }
        if !seen.insert(v.name.clone()) {
            bail!("{} is set twice.", v.name);
        }
    }
    let command = match home_relative(command, cfg!(windows)) {
        Some(rest) => crate::paths::home().join(rest).display().to_string(),
        None => command.to_string(),
    };
    Ok(AddedAgent {
        id,
        name: name.to_string(),
        command,
        args: args.into_iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
        env: env.into_iter().filter(|v| !v.name.is_empty()).collect(),
        source: AgentSource::Command,
        ..Default::default()
    })
}

impl AddedAgent {
    /// The program to start, if it's here: a full path that exists, or a name found on PATH.
    ///
    /// On Windows a name finds the file under any of `PATHEXT`'s extensions (`npx` is `npx.cmd`),
    /// which `detect::batch_args_problem` then checks the arguments for before it starts.
    pub fn resolve(&self) -> Option<PathBuf> {
        self.resolve_in(crate::detect::login_path())
    }

    /// `resolve`, a name looked up on `path`.
    fn resolve_in(&self, path: &str) -> Option<PathBuf> {
        if crate::detect::is_bare_name(&self.command) {
            crate::detect::which_in(path, &self.command)
        } else {
            Some(PathBuf::from(&self.command)).filter(|p| p.is_file())
        }
    }

    /// Why it can't start: what's missing, and how it comes back.
    pub fn missing(&self) -> String {
        match (self.source, self.distribution.as_deref()) {
            (AgentSource::Registry, Some("npx")) => "npx isn't on your PATH: install Node.js to run it.".into(),
            (AgentSource::Registry, Some("uvx")) => "uvx isn't on your PATH: install uv to run it.".into(),
            (AgentSource::Registry, _) => "Its download is gone. Remove it and add it again from the ACP Registry.".into(),
            (AgentSource::Command, _) if !crate::detect::is_bare_name(&self.command) => format!("{} doesn't exist.", crate::paths::tildify(Path::new(&self.command))),
            (AgentSource::Command, _) => format!("{} isn't on your PATH.", self.command),
        }
    }

    /// Its environment, secrets read from the Keychain (one that can't be read is left out).
    pub fn launch_env(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .filter_map(|v| {
                let value = if v.secret { secret(&self.id, &v.name)? } else { v.value.clone() };
                Some((v.name.clone(), value))
            })
            .collect()
    }

    /// The command line it runs, for Settings: `npx -y @scope/agent@1.2.3 --acp`.
    pub fn command_line(&self) -> String {
        let program = if !crate::detect::is_bare_name(&self.command) { crate::paths::tildify(Path::new(&self.command)) } else { self.command.clone() };
        std::iter::once(program).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

/// The Keychain service Trek keeps secrets under (API keys too: `settings::secrets`).
const KEYCHAIN_SERVICE: &str = "dev.trek.Trek";

fn secret_account(agent: &str, var: &str) -> String {
    format!("agent-env:{agent}:{var}")
}

pub fn set_secret(agent: &str, var: &str, value: &str) -> Result<()> {
    anyhow::ensure!(!crate::paths::isolated(), "the Keychain is off in this process");
    keyring::Entry::new(KEYCHAIN_SERVICE, &secret_account(agent, var))?.set_password(value)?;
    Ok(())
}

pub fn secret(agent: &str, var: &str) -> Option<String> {
    if crate::paths::isolated() {
        return None;
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, &secret_account(agent, var)).ok()?.get_password().ok()
}

fn delete_secret(agent: &str, var: &str) {
    if !crate::paths::isolated() {
        if let Ok(e) = keyring::Entry::new(KEYCHAIN_SERVICE, &secret_account(agent, var)) {
            let _ = e.delete_credential();
        }
    }
}

/// Drop what probing added agent `id` learned (its models, its login): a new version or command
/// is asked afresh. The file is `trek_agents::acp`'s probe cache for `acp:<id>`.
pub fn forget_probe(id: &str) {
    let name: String = format!("acp:{id}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let _ = std::fs::remove_file(crate::paths::data_dir().join("acp-agents").join(format!("{name}.json")));
}

/// Where registry agents are installed, each under `<id>/<version>/`.
pub fn agents_dir() -> PathBuf {
    crate::paths::data_dir().join("agents")
}

/// Forget an added agent: its download and icon, its secrets, and what its probe learned.
pub fn remove(agent: &AddedAgent) {
    if valid_id(&agent.id) {
        let _ = std::fs::remove_dir_all(agents_dir().join(&agent.id));
        forget_probe(&agent.id);
    }
    for v in agent.env.iter().filter(|v| v.secret) {
        delete_secret(&agent.id, &v.name);
    }
}

// ---------------------------------------------------------------------------------------------
// Fetching

pub fn cache_dir() -> PathBuf {
    crate::paths::data_dir().join("acp-registry")
}

fn cache_file() -> PathBuf {
    cache_dir().join("registry.json")
}

fn icon_cache(id: &str) -> PathBuf {
    cache_dir().join("icons").join(format!("{id}.svg"))
}

/// The index fetched last time, with the icons already here. Reads the disk: not on the UI thread.
pub fn load_cached() -> Option<Registry> {
    let path = cache_file();
    let text = std::fs::read_to_string(&path).ok()?;
    let mut r = parse(&text).ok()?;
    r.fetched_at = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    attach_icons(&mut r);
    Some(r)
}

fn attach_icons(r: &mut Registry) {
    for a in &mut r.agents {
        a.icon_file = Some(icon_cache(&a.id)).filter(|p| p.is_file());
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(timeout).connect_timeout(Duration::from_secs(20)).user_agent(format!("Trek/{}", crate::VERSION)).build()?)
}

/// `url`'s body, refused past `cap` bytes.
async fn get_capped(client: &reqwest::Client, url: &str, cap: usize) -> Result<Vec<u8>> {
    let mut resp = client.get(url).send().await?.error_for_status()?;
    if resp.content_length().is_some_and(|n| n > cap as u64) {
        bail!("{url} is larger than expected");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > cap {
            bail!("{url} is larger than expected");
        }
    }
    Ok(body)
}

/// Fetch the index (and the icons not cached yet), keep it, and return it. A test process never
/// reaches the network: it reads `TREK_ACP_REGISTRY` or nothing.
pub async fn fetch() -> Result<Registry> {
    let fixture = std::env::var_os(FIXTURE_ENV).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    let text = match &fixture {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("couldn't read {}", path.display()))?,
        None if crate::paths::isolated() => bail!("The ACP Registry isn't fetched in a test process"),
        None => {
            let body = get_capped(&client(REGISTRY_TIMEOUT)?, REGISTRY_URL, MAX_REGISTRY_BYTES).await.context("Couldn't reach the ACP Registry")?;
            String::from_utf8(body).context("The ACP Registry sent something that isn't text")?
        }
    };
    let mut registry = parse(&text)?;
    let dir = cache_dir();
    std::fs::create_dir_all(dir.join("icons"))?;
    let tmp = dir.join(format!("registry.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, cache_file()))?;
    registry.fetched_at = crate::store::now_ms();
    fetch_icons(&registry, fixture.as_deref().and_then(Path::parent)).await;
    attach_icons(&mut registry);
    Ok(registry)
}

/// Cache the icons not here yet, a few at a time. An icon that can't be fetched is drawn as a
/// monogram, and asked for again next time.
async fn fetch_icons(registry: &Registry, fixture_dir: Option<&Path>) {
    use futures::StreamExt as _;
    let missing: Vec<&RegistryAgent> = registry.agents.iter().filter(|a| !icon_cache(&a.id).is_file()).collect();
    if let Some(dir) = fixture_dir {
        for a in missing {
            let _ = std::fs::copy(dir.join("icons").join(format!("{}.svg", a.id)), icon_cache(&a.id));
        }
        return;
    }
    let Ok(client) = client(Duration::from_secs(15)) else { return };
    futures::stream::iter(missing.into_iter().filter_map(|a| Some((a.id.clone(), a.icon.clone()?))))
        .for_each_concurrent(8, |(id, url)| {
            let client = client.clone();
            async move {
                match get_capped(&client, &url, MAX_ICON_BYTES).await {
                    Ok(body) if looks_like_svg(&body) => {
                        let _ = std::fs::write(icon_cache(&id), body);
                    }
                    Ok(_) => tracing::debug!("{id}'s registry icon isn't an SVG"),
                    Err(e) => tracing::debug!("{id}'s registry icon: {e:#}"),
                }
            }
        })
        .await;
}

fn looks_like_svg(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_lowercase();
    head.contains("<svg")
}

// ---------------------------------------------------------------------------------------------
// Installing

/// How far an install has got, for its row.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    Unpacking,
    /// `npx` fetching the package and its dependencies.
    Fetching,
}

/// Install `agent` on this Mac and return it as an added agent. A binary already unpacked for
/// this version is used as it is. `progress` is called from the runtime.
pub async fn install(agent: &RegistryAgent, progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<AddedAgent> {
    // Downloads and package managers stay out of tests and design reviews.
    anyhow::ensure!(!crate::paths::isolated(), "Agents aren't installed from a test process.");
    let plan = plan(agent, current_target()).ok_or_else(|| anyhow!("{} has no build for {}.", agent.name, if cfg!(windows) { "this PC" } else { "this Mac" }))?; // the app's `words` table, which core can't reach
    let home = agents_dir().join(&agent.id);
    std::fs::create_dir_all(&home)?;
    let mut added = AddedAgent {
        id: agent.id.clone(),
        name: agent.name.clone(),
        source: AgentSource::Registry,
        version: Some(agent.version.clone()),
        distribution: Some(plan.kind().into()),
        description: agent.description.clone(),
        website: agent.link().map(String::from),
        icon: keep_icon(agent, &home),
        ..Default::default()
    };
    let env = |env: &BTreeMap<String, String>| env.iter().map(|(name, value)| EnvVar { name: name.clone(), value: value.clone(), secret: false }).collect();
    match plan {
        Plan::Binary(t) => {
            let segments = command_segments(&t.cmd).ok_or_else(|| anyhow!("The registry's command for {} points outside its folder.", agent.name))?;
            let root = home.join(&agent.version);
            let exe = segments.iter().fold(root.clone(), |p, s| p.join(s));
            if !exe.is_file() {
                download_and_unpack(&t, &home, &root, &segments, progress).await?;
            }
            // Older versions go once the new one is in place (a thread still running one keeps it
            // open until it ends).
            for entry in std::fs::read_dir(&home).into_iter().flatten().flatten() {
                let name = entry.file_name();
                if entry.path().is_dir() && name != agent.version.as_str() {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
            added.command = exe.display().to_string();
            added.args = t.args;
            added.env = env(&t.env);
        }
        Plan::Npx(p) => {
            let npx = crate::detect::which("npx").ok_or_else(|| anyhow!("{} runs with npx, which isn't on your PATH. Install Node.js, then try again.", agent.name))?;
            progress(Progress::Fetching);
            warm_npx(&npx, &p.package).await?;
            added.command = "npx".into();
            added.args = ["-y".to_string(), p.package].into_iter().chain(p.args).collect();
            added.env = env(&p.env);
        }
        Plan::Uvx(p) => {
            crate::detect::which("uvx").ok_or_else(|| anyhow!("{} runs with uvx, which isn't on your PATH. Install uv, then try again.", agent.name))?;
            added.command = "uvx".into();
            added.args = std::iter::once(p.package).chain(p.args).collect();
            added.env = env(&p.env);
        }
    }
    Ok(added)
}

/// Copy the cached icon beside the install, where clearing the cache doesn't take it.
fn keep_icon(agent: &RegistryAgent, home: &Path) -> Option<String> {
    let kept = home.join("icon.svg");
    let from = agent.icon_file.clone().unwrap_or_else(|| icon_cache(&agent.id));
    std::fs::copy(&from, &kept).ok().map(|_| kept.display().to_string())
}

/// Fetch the package and its dependencies into npm's cache now, so the agent's first start
/// doesn't spend its handshake time downloading. Runs only `node -e ""`, nothing of the package.
async fn warm_npx(npx: &Path, package: &str) -> Result<()> {
    // On Windows `npx` is `npx.cmd`, which cmd.exe starts (see `detect::which`).
    let args = ["-y", "--package", package, "--", "node", "-e", ""];
    if let Some(problem) = crate::detect::batch_args_problem(npx, args) {
        bail!("npx can't fetch {package}: {problem}");
    }
    let run = tokio::process::Command::new(npx)
        .args(args)
        .env("PATH", crate::detect::login_path())
        .current_dir(crate::paths::home())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(PACKAGE_TIMEOUT, run).await.map_err(|_| anyhow!("npx was still fetching {package} after 10 minutes"))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("npx failed");
        bail!("npx couldn't fetch {package}: {}", line.trim());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ArchiveKind {
    Tar,
    Zip,
    Raw,
}

fn archive_kind(url: &str) -> ArchiveKind {
    let path = url.split(['?', '#']).next().unwrap_or_default().to_lowercase();
    if [".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".tar"].iter().any(|e| path.ends_with(e)) {
        ArchiveKind::Tar
    } else if path.ends_with(".zip") {
        ArchiveKind::Zip
    } else {
        ArchiveKind::Raw
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Download `t`'s archive into `home`, check it, and unpack it as `root`. Unpacked beside it
/// first and moved into place whole, so a failed install leaves nothing half there.
async fn download_and_unpack(t: &BinaryTarget, home: &Path, root: &Path, segments: &[String], progress: &(dyn Fn(Progress) + Send + Sync)) -> Result<()> {
    use sha2::Digest as _;
    use tokio::io::AsyncWriteExt as _;
    let kind = archive_kind(&t.archive);
    let download = home.join(format!(".download-{}", std::process::id()));
    let staging = home.join(format!(".unpack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = async {
        let mut resp = client(ARCHIVE_TIMEOUT)?.get(&t.archive).send().await.context("Couldn't start the download")?.error_for_status()?;
        let total = resp.content_length();
        if total.is_some_and(|n| n > MAX_ARCHIVE_BYTES) {
            bail!("The download is larger than 1 GB");
        }
        let mut file = tokio::fs::File::create(&download).await?;
        let mut hasher = sha2::Sha256::new();
        let (mut done, mut shown) = (0u64, 0u64);
        progress(Progress::Downloading { done: 0, total });
        while let Some(chunk) = resp.chunk().await.context("The download was cut off")? {
            done += chunk.len() as u64;
            if done > MAX_ARCHIVE_BYTES {
                bail!("The download is larger than 1 GB");
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            // About a hundred updates however large it is.
            if done - shown >= total.map_or(1 << 20, |n| (n / 100).max(1)) {
                shown = done;
                progress(Progress::Downloading { done, total });
            }
        }
        file.flush().await?;
        drop(file);
        if let Some(expected) = &t.sha256 {
            let actual = hex::encode(hasher.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                bail!("The download doesn't match the registry's checksum, so it wasn't installed");
            }
        }
        progress(Progress::Unpacking);
        unpack(&download, kind, &staging, segments).await?;
        let _ = std::fs::remove_dir_all(root);
        std::fs::rename(&staging, root)?;
        Ok(())
    }
    .await;
    let _ = std::fs::remove_file(&download);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Unpack `archive` into `into` (a raw binary is the command itself) and make the command
/// executable. Every entry is checked as it is read: none may land outside `into`.
async fn unpack(archive: &Path, kind: ArchiveKind, into: &Path, segments: &[String]) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let exe = segments.iter().fold(into.to_path_buf(), |p, s| p.join(s));
    match kind {
        ArchiveKind::Raw => {
            if let Some(parent) = exe.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(archive, &exe)?;
        }
        ArchiveKind::Tar | ArchiveKind::Zip => {
            // On a blocking thread: the archive can be large, and the readers aren't async.
            let (archive, into) = (archive.to_path_buf(), into.to_path_buf());
            tokio::task::spawn_blocking(move || if kind == ArchiveKind::Zip { unpack_zip(&archive, &into) } else { unpack_tar(&archive, &into) }).await??;
        }
    }
    if !exe.is_file() {
        bail!("The download has no {}", segments.join("/"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(&exe)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(&exe, perms)?;
    }
    Ok(())
}

fn outside(name: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("The archive has a file outside its folder ({name}), so it wasn't installed")
}

/// Unpack a `.tar.gz`, `.tgz` or `.tar` into `into`. An entry that is absolute, climbs out with
/// `..`, or links (symbolically or hard) to something outside `into` refuses the whole archive;
/// the install folder is a staging one that is discarded then. On Windows symlinks are skipped
/// (making one needs a privilege), and a file's mode is what the archive says elsewhere.
fn unpack_tar(archive: &Path, into: &Path) -> Result<()> {
    use std::io::Read as _;
    let not_an_archive = |_| anyhow!("The download isn't an archive Trek can open");
    let mut file = std::fs::File::open(archive)?;
    let mut magic = [0u8; 6];
    let n = file.read(&mut magic)?;
    std::io::Seek::rewind(&mut file)?;
    let reader: Box<dyn std::io::Read> = match &magic[..n] {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::GzDecoder::new(file)),
        [b'B', b'Z', b'h', ..] | [0xfd, b'7', b'z', b'X', b'Z', 0] => bail!("Trek opens .tar.gz, .tgz, .tar and .zip downloads, and this one is compressed another way"),
        _ => Box::new(file),
    };
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries().map_err(not_an_archive)? {
        let mut entry = entry.map_err(not_an_archive)?;
        let path = entry.path()?.into_owned();
        let name = path.to_string_lossy().into_owned();
        let Some(segments) = command_segments(&name) else {
            // `./` is the archive's own folder.
            if name.trim_start_matches("./").trim_matches('/').is_empty() {
                continue;
            }
            return Err(outside(name));
        };
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry.link_name()?.map(|t| t.into_owned()).ok_or_else(|| outside(&name))?;
            let inside = if kind.is_hard_link() {
                command_segments(&target.to_string_lossy()).is_some()
            } else {
                // Relative to the folder the link is in.
                link_stays_inside(&segments[..segments.len() - 1], &target)
            };
            if !inside {
                return Err(outside(format!("{name} → {}", target.display())));
            }
            if cfg!(windows) && kind.is_symlink() {
                continue;
            }
        }
        if !entry.unpack_in(into)? {
            return Err(outside(name));
        }
    }
    Ok(())
}

/// Whether a symlink in the folder `dir` (path segments inside the install) pointing at `target`
/// resolves, by its spelling alone, to somewhere inside it.
fn link_stays_inside(dir: &[String], target: &Path) -> bool {
    use std::path::Component;
    let mut at = dir.len();
    for part in target.components() {
        match part {
            Component::Normal(_) => at += 1,
            Component::CurDir => {}
            Component::ParentDir if at > 0 => at -= 1,
            _ => return false,
        }
    }
    true
}

/// Unpack a `.zip` into `into`. Names that aren't safe (`enclosed_name`: absolute, or reaching out
/// with `..`) refuse the archive. A symlink entry is skipped, never created. On Unix a file keeps
/// the permission bits its entry has.
fn unpack_zip(archive: &Path, into: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(archive)?)).map_err(|_| anyhow!("The download isn't an archive Trek can open"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| anyhow!("Couldn't unpack the download: {e}"))?;
        // The name as written is held to the same rule as a tar's, as well as `enclosed_name`.
        let Ok(name) = entry.name().map(|n| n.into_owned()) else {
            return Err(outside("a name that isn't text"));
        };
        if name.trim_start_matches("./").trim_matches('/').is_empty() {
            continue; // the archive's own folder
        }
        let Some(rel) = entry.enclosed_name().filter(|_| command_segments(&name).is_some()) else {
            return Err(outside(name));
        };
        let dest = into.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else if entry.is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut std::fs::File::create(&dest)?)?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Registry {
        parse(include_str!("../fixtures/acp-registry.json")).unwrap()
    }

    #[test]
    fn the_index_is_read_and_entries_it_cant_use_are_left_out() {
        let r = fixture();
        assert_eq!(r.version, "1.0.0");
        assert!(r.agent("devin").is_some() && r.agent("fast-agent").is_some());
        assert!(r.agents.iter().all(|a| valid_id(&a.id)), "the malformed id is skipped, not fatal");
        assert_eq!(r.agents.len(), 11);
        let devin = r.agent("devin").unwrap();
        assert_eq!(devin.name, "Devin");
        assert_eq!(devin.link(), Some("https://docs.devin.ai/cli"));
        assert_eq!(devin.distribution.binary["darwin-aarch64"].cmd, "./bin/devin");
        // A package without an exact version can't be installed; the entry stays listed.
        assert_eq!(r.agent("floating-npx").unwrap().distribution.npx, None);
        assert!(parse("<html>").is_err());
        assert!(parse(r#"{"version":"1.0.0","agents":[{"id":"x"}]}"#).unwrap().agents.is_empty());
    }

    #[test]
    fn a_binary_is_preferred_then_npx_then_uvx() {
        let r = fixture();
        let kind = |id: &str, t: &str| plan(r.agent(id).unwrap(), Some(t)).map(|p| p.kind());
        assert_eq!(kind("devin", "darwin-aarch64"), Some("binary"));
        assert_eq!(kind("kilo", "darwin-x86_64"), Some("binary"), "kilo has npx too");
        assert_eq!(kind("auggie", "darwin-aarch64"), Some("npx"));
        assert_eq!(kind("fast-agent", "darwin-aarch64"), Some("uvx"));
        // Kimi has no Intel Mac build and no package.
        assert_eq!(kind("kimi", "darwin-x86_64"), None);
        assert_eq!(kind("kimi", "darwin-aarch64"), Some("binary"));
        assert_eq!(kind("floating-npx", "darwin-aarch64"), None);
        assert_eq!(plan(r.agent("devin").unwrap(), None), None, "no target, no binary");
    }

    #[test]
    fn search_ranks_names_before_descriptions() {
        let r = fixture();
        let ids = |q: &str| r.search(q).into_iter().map(|a| a.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids("devin").first(), Some(&"devin"));
        assert_eq!(ids("").len(), r.agents.len());
        assert!(ids("zzzz-nothing").is_empty());
        assert_eq!(ids("gem"), ["gemini"]);
        assert!(ids("cognition").contains(&"devin"), "authors match");
    }

    #[test]
    fn ids_versions_and_commands_are_checked() {
        for ok in ["devin", "qwen-code", "a.b_c-1", "9lives"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "Devin", "-x", ".x", "a b", "a/b", &"x".repeat(129)] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert_eq!(command_segments("./bin/devin"), Some(vec!["bin".into(), "devin".into()]));
        assert_eq!(command_segments("kilo"), Some(vec!["kilo".into()]));
        assert_eq!(command_segments("./bin\\devin.exe"), Some(vec!["bin".into(), "devin.exe".into()]));
        for bad in ["/usr/bin/env", "../x", "bin/../../x", "C:\\x.exe", "", "./"] {
            assert_eq!(command_segments(bad), None, "{bad}");
        }
        assert!(pinned_npx("@augmentcode/auggie@0.36.0") && pinned_npx("cline@3.0.70") && pinned_npx("x@v1.2.3-beta.1"));
        for bad in ["cline", "cline@latest", "@scope/x", "cline@1.2", "@a@1.0.0", "a/b@1.0.0"] {
            assert!(!pinned_npx(bad), "{bad}");
        }
        assert!(pinned_uvx("fast-agent-acp==0.10.1") && pinned_uvx("minion-code@0.1.44"));
        assert!(!pinned_uvx("fast-agent-acp") && !pinned_uvx("fast-agent-acp>=0.10"));
    }

    #[test]
    fn registry_ids_of_built_in_agents_map_to_them() {
        assert_eq!(built_in("devin"), Some(AgentId::Acp("devin".into())));
        assert_eq!(built_in("github-copilot-cli"), Some(AgentId::Acp("github-copilot".into())));
        assert_eq!(built_in("grok-build"), Some(AgentId::Acp("grok".into())));
        assert_eq!(built_in("factory-droid"), Some(AgentId::Droid));
        assert_eq!(built_in("claude-acp"), Some(AgentId::ClaudeCode));
        assert_eq!(built_in("auggie"), None);
    }

    #[test]
    fn a_custom_agent_gets_an_id_from_its_name_and_is_checked() {
        let a = custom("My Agent 2", "", "my-agent", vec!["--acp".into(), " ".into()], vec![], &[]).unwrap();
        assert_eq!((a.id.as_str(), a.args.as_slice(), a.source), ("my-agent-2", &["--acp".to_string()][..], AgentSource::Command));
        assert_eq!(id_from_name("  Über Bot!! "), "ber-bot");
        assert!(custom("", "", "x", vec![], vec![], &[]).is_err());
        assert!(custom("Cursor", "", "x", vec![], vec![], &[]).is_err(), "a built-in id");
        assert!(custom("A", "Bad Id", "x", vec![], vec![], &[]).is_err());
        assert!(custom("A", "", "x", vec![], vec![], &["a".into()]).is_err(), "taken");
        assert!(custom("A", "", "", vec![], vec![], &[]).is_err());
        assert!(custom("A", "", "bin/agent", vec![], vec![], &[]).is_err(), "relative paths are ambiguous");
        // An absolute path looks different on each system.
        let abs = if cfg!(windows) { r"C:\opt\a" } else { "/opt/a" };
        let env = |n: &str| vec![EnvVar { name: n.into(), value: "v".into(), secret: false }];
        assert!(custom("A", "", abs, vec![], env("API_KEY"), &[]).is_ok());
        assert!(custom("A", "", abs, vec![], env("1BAD"), &[]).is_err());
        let a = custom("A", "", abs, vec![], env("TOKEN"), &[]).unwrap();
        assert_eq!(a.launch_env(), [("TOKEN".to_string(), "v".to_string())]);
        assert_eq!(a.command_line(), abs);
    }

    #[test]
    fn a_program_is_a_name_or_a_full_path_on_either_system() {
        let ok = |c: &str, windows| check_command(c, windows).is_ok();
        // Names, and full paths as each system writes them.
        for c in ["my-agent", "npx", "agent.exe", "~/bin/agent"] {
            assert!(ok(c, false) && ok(c, true), "{c}");
        }
        assert!(ok("/opt/x/agent", false));
        for c in [r"C:\tools\agent.exe", "D:/x/agent", r"\\server\share\agent.exe", r"~\bin\agent.exe"] {
            assert!(ok(c, true), "{c}");
        }
        // Not full: relative to wherever Trek happens to be, or rooted without a drive.
        for c in [r".\agent.exe", "./agent", "bin/agent", r"tools\agent.exe", r"\agent.exe", "/opt/x/agent", "C:agent.exe", "agent/x.exe"] {
            assert!(!ok(c, true), "{c}");
        }
        for c in ["./agent", "bin/agent", "../agent"] {
            assert!(!ok(c, false), "{c}");
        }
        assert_eq!(check_command("bin/agent", false).unwrap_err().to_string(), "Give the program's full path, or just its name if it's on your PATH.");
        // The same through `custom`, on this system.
        let abs = if cfg!(windows) { r"C:\opt\a.exe" } else { "/opt/a" };
        assert_eq!(custom("A", "", abs, vec![], vec![], &[]).unwrap().command, abs);
        assert!(custom("A", "", r".\agent.exe", vec![], vec![], &[]).is_err() == cfg!(windows));
        assert_eq!(custom("A", "", "npx", vec![], vec![], &[]).unwrap().command, "npx");
        let home = custom("A", "", if cfg!(windows) { r"~\bin\a.exe" } else { "~/bin/a" }, vec![], vec![], &[]).unwrap();
        assert!(Path::new(&home.command).starts_with(crate::paths::home()), "{}", home.command);
    }

    #[test]
    fn a_name_finds_its_script_and_a_path_is_used_as_written() {
        let dir = std::env::temp_dir().join(format!("trek added agent {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // `npx` is `npx.cmd` on Windows.
        let script = dir.join(if cfg!(windows) { "npx.cmd" } else { "npx" });
        std::fs::write(&script, "").unwrap();
        let path = std::env::join_paths([dir.as_path()]).unwrap().to_string_lossy().into_owned();
        let added = |command: &str| AddedAgent { command: command.into(), ..Default::default() };
        assert_eq!(added("npx").resolve_in(&path), Some(script.clone()));
        assert_eq!(added("uvx").resolve_in(&path), None);
        assert_eq!(added(&script.display().to_string()).resolve_in(""), Some(script.clone()));
        assert_eq!(added(&dir.join("gone").display().to_string()).resolve_in(&path), None);
        // What Settings says is missing is the path for a path, whichever slash it has.
        assert!(added(&dir.join("gone").display().to_string()).missing().ends_with("doesn't exist."));
        assert_eq!(added("npx").missing(), "npx isn't on your PATH.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksums_are_compared_without_case() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(archive_kind("https://x/a-1.0.tar.gz?sig=1"), ArchiveKind::Tar);
        assert_eq!(archive_kind("https://x/a.zip"), ArchiveKind::Zip);
        assert_eq!(archive_kind("https://x/agent"), ArchiveKind::Raw);
    }

    /// A tar entry named `name` whatever the name is: the `tar` crate's own `set_path` refuses the
    /// hostile ones these tests need.
    fn tar_entry(b: &mut tar::Builder<Vec<u8>>, name: &str, kind: tar::EntryType, mode: u32, link: Option<&str>, data: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
        if let Some(link) = link {
            h.as_gnu_mut().unwrap().linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        h.set_entry_type(kind);
        h.set_mode(mode);
        h.set_size(data.len() as u64);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }

    fn tar_gz(path: &Path, build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) {
        let mut b = tar::Builder::new(Vec::new());
        build(&mut b);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &b.into_inner().unwrap()).unwrap();
        std::fs::write(path, gz.finish().unwrap()).unwrap();
    }

    fn zip_of(path: &Path, files: &[(&str, u32)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, mode) in files {
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).unix_permissions(*mode);
            z.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut z, b"#!/bin/sh\necho hi\n").unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn archives_unpack_inside_their_folder_and_nowhere_else() {
        let dir = std::env::temp_dir().join(format!("trek-registry-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rt = crate::runtime();
        let cmd = ["bin".to_string(), "agent".to_string()];
        let script: &[u8] = b"#!/bin/sh\necho hi\n";
        let refused = |archive: &Path, kind: ArchiveKind, out: &str| {
            let err = rt.block_on(unpack(archive, kind, &dir.join(out), &cmd)).expect_err(&format!("{out} was accepted"));
            assert!(err.to_string().contains("outside its folder"), "{out}: {err}");
        };

        // A tar.gz: the folder, then the command in it (no exec bit in the archive).
        let good = dir.join("good.tar.gz");
        tar_gz(&good, |b| {
            tar_entry(b, "./", tar::EntryType::Directory, 0o755, None, &[]);
            tar_entry(b, "./bin/", tar::EntryType::Directory, 0o755, None, &[]);
            tar_entry(b, "./bin/agent", tar::EntryType::Regular, 0o644, None, script);
            tar_entry(b, "./bin/alias", tar::EntryType::Symlink, 0o777, Some("agent"), &[]);
        });
        let into = dir.join("out");
        rt.block_on(unpack(&good, ArchiveKind::Tar, &into, &cmd)).unwrap();
        let exe = into.join("bin/agent");
        assert_eq!(std::fs::read(&exe).unwrap(), script);
        #[cfg(unix)]
        {
            assert_ne!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&exe).unwrap().permissions()) & 0o111, 0);
            assert_eq!(std::fs::read_link(into.join("bin/alias")).unwrap(), Path::new("agent"), "a link that stays inside is kept");
        }
        // The command must be in it.
        let missing = rt.block_on(unpack(&good, ArchiveKind::Tar, &dir.join("out2"), &["agent".into()]));
        assert!(missing.unwrap_err().to_string().contains("no agent"));
        // A plain, uncompressed tar is read too.
        let plain = dir.join("plain.tar");
        let mut b = tar::Builder::new(Vec::new());
        tar_entry(&mut b, "bin/agent", tar::EntryType::Regular, 0o755, None, script);
        std::fs::write(&plain, b.into_inner().unwrap()).unwrap();
        rt.block_on(unpack(&plain, ArchiveKind::Tar, &dir.join("out-plain"), &cmd)).unwrap();

        // An entry that climbs out, is absolute, or links out is refused, and nothing lands outside.
        let hostile: [(&str, &str, tar::EntryType, Option<&str>); 6] = [
            ("up", "../escaped/agent", tar::EntryType::Regular, None),
            ("deep", "bin/../../escaped/agent", tar::EntryType::Regular, None),
            ("abs", "/escaped/agent", tar::EntryType::Regular, None),
            ("drive", "C:/escaped/agent", tar::EntryType::Regular, None),
            ("symlink", "bin/out", tar::EntryType::Symlink, Some("../../escaped")),
            ("hardlink", "bin/out", tar::EntryType::Link, Some("../escaped/agent")),
        ];
        for (name, entry, kind, link) in hostile {
            let archive = dir.join(format!("{name}.tar.gz"));
            tar_gz(&archive, |b| {
                tar_entry(b, "bin/agent", tar::EntryType::Regular, 0o755, None, script);
                tar_entry(b, entry, kind, 0o644, link, script);
            });
            refused(&archive, ArchiveKind::Tar, &format!("out-{name}"));
        }
        // An absolute symlink too, and nothing was written through a link that was refused.
        let absolute = dir.join("absolute.tar.gz");
        tar_gz(&absolute, |b| tar_entry(b, "bin/out", tar::EntryType::Symlink, 0o777, Some("/etc"), &[]));
        refused(&absolute, ArchiveKind::Tar, "out-absolute");
        assert!(!dir.join("escaped").exists() && !into.join("..").join("escaped").exists());

        // A zip: the same.
        let zip = dir.join("good.zip");
        zip_of(&zip, &[("bin/agent", 0o644), ("bin/readme.txt", 0o644)]);
        let zout = dir.join("zout");
        rt.block_on(unpack(&zip, ArchiveKind::Zip, &zout, &cmd)).unwrap();
        assert_eq!(std::fs::read(zout.join("bin/agent")).unwrap(), script);
        assert!(zout.join("bin/readme.txt").is_file());
        #[cfg(unix)]
        assert_ne!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(zout.join("bin/agent")).unwrap().permissions()) & 0o111, 0);
        assert!(rt.block_on(unpack(&zip, ArchiveKind::Zip, &dir.join("zout2"), &["agent".into()])).unwrap_err().to_string().contains("no agent"));
        for (name, entry) in [("zup", "../escaped/agent"), ("zdeep", "bin/../../escaped/agent"), ("zabs", "/escaped/agent")] {
            let archive = dir.join(format!("{name}.zip"));
            zip_of(&archive, &[("bin/agent", 0o755), (entry, 0o644)]);
            refused(&archive, ArchiveKind::Zip, &format!("out-{name}"));
        }
        assert!(!dir.join("escaped").exists());

        // Not an archive, or one compressed in a way Trek doesn't read.
        let junk = dir.join("junk");
        std::fs::write(&junk, vec![b'x'; 2000]).unwrap();
        assert!(rt.block_on(unpack(&junk, ArchiveKind::Tar, &dir.join("out-junk"), &cmd)).unwrap_err().to_string().contains("isn't an archive"));
        assert!(rt.block_on(unpack(&junk, ArchiveKind::Zip, &dir.join("out-junk-zip"), &cmd)).unwrap_err().to_string().contains("isn't an archive"));
        let xz = dir.join("a.tar.xz");
        std::fs::write(&xz, [0xfd, b'7', b'z', b'X', b'Z', 0, 1, 2, 3]).unwrap();
        assert!(rt.block_on(unpack(&xz, ArchiveKind::Tar, &dir.join("out-xz"), &cmd)).unwrap_err().to_string().contains("compressed another way"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
