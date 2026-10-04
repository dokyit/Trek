//! Agent CLI updates: which version of each agent CLI is installed, how it was installed (read
//! from where its binary really lives), the newest version on that same channel, and the command
//! that updates it the way its vendor documents.
//!
//! Channels: npm (and Bun, pnpm) global packages are asked of the npm registry and updated with
//! the package manager that installed them; Homebrew formulae and casks are asked of
//! formulae.brew.sh (a third-party tap's formula of its GitHub repository) and updated with
//! `brew upgrade`; anything else came from the vendor's own installer, whose release feed says
//! what's newest and whose CLI updates itself (`claude update`, `grok update`, …).
//!
//! Everything here is read-only except `run_update`. Checks never fail as a whole: an agent
//! whose feed can't be reached (offline) keeps the newest version the last check found
//! (`carry_over`), and the results are cached (`Snapshot`) so a launch shows them at once.

use crate::detect::{login_path, which};
use crate::types::AgentId;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// An agent CLI Trek can keep up to date.
#[derive(Debug, Clone, Copy)]
pub struct Harness {
    /// `AgentId::key()` of the agent it runs.
    pub agent: &'static str,
    pub binary: &'static str,
    /// The npm package it's published as.
    pub npm: Option<&'static str>,
    /// Its own update command, run with the installed binary: how a native install updates.
    pub self_update: Option<&'static [&'static str]>,
    /// The vendor's install script, for a native install with no update command of its own
    /// (re-running it is how the vendor says to update).
    pub installer: Option<&'static str>,
    /// Where a native install's newest release is published.
    pub feed: Feed,
}

impl Harness {
    pub fn name(&self) -> String {
        AgentId::from_key(self.agent).display_name()
    }
}

/// A vendor's own release feed, for installs that came from its installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// The npm package's `latest`: the vendor's native builds carry the same versions.
    Npm,
    /// Claude Code's installer channel (`downloads.claude.ai/…/latest`, a bare version).
    ClaudeReleases,
    /// Grok's stable channel (`x.ai/cli/stable`, a bare version).
    GrokStable,
    /// Devin's release manifest (`{"version": …}`).
    DevinManifest,
    /// Cursor's install script, which names the build it installs.
    CursorInstaller,
    /// Factory's install script (`VER="…"`).
    DroidInstaller,
    /// A GitHub repository's latest release (`tag_name`).
    GitHub(&'static str),
}

/// The agent CLIs Trek runs, as `detect` and `catalog` know them. Amp and Pi are checked by
/// their own CLIs, which their ACP adapters run.
pub const HARNESSES: &[Harness] = &[
    Harness { agent: "claude-code", binary: "claude", npm: Some("@anthropic-ai/claude-code"), self_update: Some(&["update"]), installer: None, feed: Feed::ClaudeReleases },
    Harness { agent: "codex", binary: "codex", npm: Some("@openai/codex"), self_update: Some(&["update"]), installer: None, feed: Feed::Npm },
    Harness { agent: "opencode", binary: "opencode", npm: Some("opencode-ai"), self_update: Some(&["upgrade"]), installer: None, feed: Feed::Npm },
    Harness { agent: "droid", binary: "droid", npm: None, self_update: None, installer: Some("curl -fsSL https://app.factory.ai/cli | sh"), feed: Feed::DroidInstaller },
    Harness { agent: "acp:cursor", binary: "cursor-agent", npm: None, self_update: Some(&["update"]), installer: None, feed: Feed::CursorInstaller },
    Harness { agent: "acp:github-copilot", binary: "copilot", npm: Some("@github/copilot"), self_update: Some(&["update"]), installer: None, feed: Feed::Npm },
    Harness { agent: "acp:gemini", binary: "gemini", npm: Some("@google/gemini-cli"), self_update: None, installer: None, feed: Feed::Npm },
    Harness { agent: "acp:kimi", binary: "kimi", npm: Some("@moonshot-ai/kimi-code"), self_update: Some(&["upgrade"]), installer: None, feed: Feed::Npm },
    Harness { agent: "acp:qwen-code", binary: "qwen", npm: Some("@qwen-code/qwen-code"), self_update: None, installer: None, feed: Feed::Npm },
    Harness { agent: "acp:grok", binary: "grok", npm: None, self_update: Some(&["update"]), installer: None, feed: Feed::GrokStable },
    Harness { agent: "acp:devin", binary: "devin", npm: None, self_update: Some(&["update"]), installer: None, feed: Feed::DevinManifest },
    Harness { agent: "acp:goose", binary: "goose", npm: None, self_update: Some(&["update"]), installer: None, feed: Feed::GitHub("block/goose") },
    Harness { agent: "acp:amp", binary: "amp", npm: Some("@sourcegraph/amp"), self_update: Some(&["update"]), installer: None, feed: Feed::Npm },
    Harness { agent: "acp:pi", binary: "pi", npm: Some("@mariozechner/pi-coding-agent"), self_update: None, installer: None, feed: Feed::Npm },
];

pub fn harness(agent: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.agent == agent)
}

/// How an agent CLI got onto this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Install {
    /// A global npm package, under `prefix` (`<prefix>/lib/node_modules/<package>`).
    Npm { package: String, prefix: PathBuf },
    Bun { package: String },
    Pnpm { package: String },
    /// A Homebrew formula; `tap` is `None` for homebrew/core.
    Brew { formula: String, prefix: PathBuf, tap: Option<String> },
    Cask { cask: String, prefix: PathBuf },
    /// The vendor's own installer.
    Native,
}

impl Install {
    /// "npm", "Homebrew", …: how it's described next to its versions.
    pub fn label(&self) -> &'static str {
        match self {
            Install::Npm { .. } => "npm",
            Install::Bun { .. } => "Bun",
            Install::Pnpm { .. } => "pnpm",
            Install::Brew { .. } | Install::Cask { .. } => "Homebrew",
            Install::Native => "Installer",
        }
    }

    fn package(&self) -> Option<&str> {
        match self {
            Install::Npm { package, .. } | Install::Bun { package } | Install::Pnpm { package } => Some(package),
            _ => None,
        }
    }
}

fn names(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn rooted(parts: &[String]) -> PathBuf {
    let mut p = PathBuf::from("/");
    p.extend(parts);
    p
}

/// How the binary at `resolved` (symlinks followed) was installed, from where it lives. A
/// formula's tap isn't in its path: `detect_install` reads it from the keg's receipt.
pub fn install_of(resolved: &Path) -> Install {
    let parts = names(resolved);
    let at = |name: &str| parts.iter().position(|p| p == name);
    if let Some(i) = at("node_modules") {
        let Some(first) = parts.get(i + 1) else { return Install::Native };
        let package = match (first.starts_with('@'), parts.get(i + 2)) {
            (true, Some(name)) => format!("{first}/{name}"),
            (true, None) => return Install::Native,
            (false, _) => first.clone(),
        };
        let path = resolved.to_string_lossy();
        if path.contains("/.bun/install/global/") {
            return Install::Bun { package };
        }
        if parts[..i].iter().any(|p| p == "pnpm") {
            return Install::Pnpm { package };
        }
        let end = if i > 0 && parts[i - 1] == "lib" { i - 1 } else { i };
        return Install::Npm { package, prefix: rooted(&parts[..end]) };
    }
    if let Some(i) = at("Cellar").filter(|i| parts.len() > i + 1) {
        return Install::Brew { formula: parts[i + 1].clone(), prefix: rooted(&parts[..i]), tap: None };
    }
    if let Some(i) = at("Caskroom").filter(|i| parts.len() > i + 1) {
        return Install::Cask { cask: parts[i + 1].clone(), prefix: rooted(&parts[..i]) };
    }
    Install::Native
}

/// The tap a formula came from, from its keg's `INSTALL_RECEIPT.json`; `None` for homebrew/core.
pub fn tap_from_receipt(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tap = v["source"]["tap"].as_str()?;
    (tap != "homebrew/core" && tap.contains('/')).then(|| tap.to_string())
}

/// `install_of`, with a formula's tap read from its keg.
pub fn detect_install(resolved: &Path) -> Install {
    match install_of(resolved) {
        Install::Brew { formula, prefix, .. } => {
            let parts = names(resolved);
            let keg = parts.iter().position(|p| p == "Cellar").and_then(|i| parts.get(i + 2)).map(|v| prefix.join("Cellar").join(&formula).join(v));
            let tap = keg.and_then(|k| std::fs::read_to_string(k.join("INSTALL_RECEIPT.json")).ok()).and_then(|j| tap_from_receipt(&j));
            Install::Brew { formula, prefix, tap }
        }
        other => other,
    }
}

/// The version in a CLI's `--version` output: "2.1.289 (Claude Code)", "codex-cli 0.160.0",
/// "GitHub Copilot CLI 1.0.91.", "grok 1.0.46 (2765805b9442) [stable]" → the dotted number, with
/// any pre-release or build suffix ("2026.10.01-e373342").
pub fn parse_version(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\''))
        .map(|t| t.trim_start_matches(['v', 'V']).trim_end_matches(['.', ':']))
        .find(|t| {
            let core = t.split(['-', '+']).next().unwrap_or_default();
            core.contains('.') && core.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(String::from)
}

fn numbers(v: &str) -> Option<(Vec<u64>, bool)> {
    let (core, suffix) = match v.find(['-', '+']) {
        Some(i) => (&v[..i], v[i..].starts_with('-')),
        None => (v, false),
    };
    Some((core.split('.').map(|n| n.parse().ok()).collect::<Option<Vec<u64>>>()?, suffix))
}

/// `latest` is a newer release than `installed`. Versions compare number by number; at equal
/// numbers a pre-release ("1.2.0-beta.1") is older than the release. A version that isn't
/// numbered never counts as newer: Trek offers no update it can't order.
pub fn is_newer(latest: &str, installed: &str) -> bool {
    let (Some((a, a_pre)), Some((b, b_pre))) = (numbers(latest), numbers(installed)) else { return false };
    let n = a.len().max(b.len());
    let pad = |v: &[u64]| (0..n).map(|i| v.get(i).copied().unwrap_or(0)).collect::<Vec<_>>();
    match pad(&a).cmp(&pad(&b)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // Same numbers: only a release after its pre-release. Two builds of a day (Cursor's
        // "2026.10.01-e373342") don't order.
        std::cmp::Ordering::Equal => b_pre && !a_pre,
    }
}

/// Where the newest version of an install is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Npm(String),
    Formula(String),
    TapFormula { tap: String, formula: String },
    Cask(String),
    Feed(Feed),
}

/// Where to ask for `h`'s newest version, installed as `install`; `None` when its vendor
/// publishes nowhere Trek can read.
pub fn source(h: &Harness, install: &Install) -> Option<Source> {
    match install {
        Install::Npm { package, .. } | Install::Bun { package } | Install::Pnpm { package } => Some(Source::Npm(package.clone())),
        Install::Brew { formula, tap: None, .. } => Some(Source::Formula(formula.clone())),
        Install::Brew { formula, tap: Some(tap), .. } => Some(Source::TapFormula { tap: tap.clone(), formula: formula.clone() }),
        Install::Cask { cask, .. } => Some(Source::Cask(cask.clone())),
        Install::Native => match h.feed {
            Feed::Npm => h.npm.map(|p| Source::Npm(p.into())),
            feed => Some(Source::Feed(feed)),
        },
    }
}

/// The URLs to try for `source`, in order.
pub fn urls(source: &Source) -> Vec<String> {
    match source {
        Source::Npm(p) => vec![format!("https://registry.npmjs.org/{p}/latest")],
        Source::Formula(f) => vec![format!("https://formulae.brew.sh/api/formula/{f}.json")],
        Source::TapFormula { tap, formula } => {
            // A tap "user/repo" is the GitHub repository user/homebrew-repo.
            let (user, repo) = tap.split_once('/').unwrap_or((tap, tap));
            let base = format!("https://raw.githubusercontent.com/{user}/homebrew-{repo}/HEAD");
            vec![format!("{base}/Formula/{formula}.rb"), format!("{base}/{formula}.rb")]
        }
        Source::Cask(c) => vec![format!("https://formulae.brew.sh/api/cask/{c}.json")],
        Source::Feed(feed) => vec![match feed {
            Feed::Npm => return vec![],
            Feed::ClaudeReleases => "https://downloads.claude.ai/claude-code-releases/latest".into(),
            Feed::GrokStable => "https://x.ai/cli/stable".into(),
            Feed::DevinManifest => "https://static.devin.ai/cli/current/manifest.json".into(),
            Feed::CursorInstaller => "https://cursor.com/install".into(),
            Feed::DroidInstaller => "https://app.factory.ai/cli".into(),
            Feed::GitHub(repo) => format!("https://api.github.com/repos/{repo}/releases/latest"),
        }],
    }
}

/// What a feed says is newest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    /// A cask that updates itself (Homebrew leaves it alone unless asked to be greedy).
    pub auto_updates: bool,
}

/// The text between `start` and the next `end` after it.
fn between<'a>(text: &'a str, start: &str, end: char) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let rest = &text[from..];
    Some(&rest[..rest.find(end)?])
}

/// Read the newest version out of `body`, a response from one of `source`'s URLs.
pub fn parse_latest(source: &Source, body: &str) -> Option<Latest> {
    let json = || serde_json::from_str::<serde_json::Value>(body).ok();
    let plain = |v: Option<String>| v.and_then(|v| parse_version(&v)).map(|version| Latest { version, auto_updates: false });
    match source {
        Source::Npm(_) => plain(json()?["version"].as_str().map(String::from)),
        Source::Formula(_) => plain(json()?["versions"]["stable"].as_str().map(String::from)),
        Source::Cask(_) => {
            let v = json()?;
            // Casks may append a build after a comma ("1.2.3,4567").
            let version = parse_version(v["version"].as_str()?.split(',').next()?)?;
            Some(Latest { version, auto_updates: v["auto_updates"].as_bool().unwrap_or(false) })
        }
        Source::TapFormula { .. } => plain(body.lines().find_map(|l| l.trim().strip_prefix("version ").map(|v| v.trim().trim_matches('"').to_string()))),
        Source::Feed(feed) => match feed {
            Feed::Npm => None,
            Feed::ClaudeReleases | Feed::GrokStable => plain(body.lines().next().map(|l| l.trim().to_string())),
            Feed::DevinManifest => plain(json()?["version"].as_str().map(String::from)),
            Feed::CursorInstaller => plain(between(body, "downloads.cursor.com/lab/", '/').map(String::from)),
            Feed::DroidInstaller => plain(body.lines().find_map(|l| l.trim().strip_prefix("VER=").map(|v| v.trim_matches('"').to_string()))),
            Feed::GitHub(_) => plain(json()?["tag_name"].as_str().map(String::from)),
        },
    }
}

/// A command that updates an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl UpdateCommand {
    fn new(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self { program: program.into(), args: args.iter().map(|a| a.to_string()).collect() }
    }

    /// As the user would type it: "npm install -g @openai/codex@latest", "claude update".
    pub fn shown(&self) -> String {
        let program = self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.program.display().to_string());
        // The installer runs through `sh -c`: show the script line itself.
        if program == "sh" && self.args.first().is_some_and(|a| a == "-c") {
            return self.args[1..].join(" ");
        }
        std::iter::once(program).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

/// The vendor's documented way to update `h`, installed as `install` with its binary at
/// `binary`: the package manager that installed it, or for its own installer, the CLI's own
/// update command (or the install script again). A self-updating cask updates itself too.
/// `None` when there's no way Trek can run.
pub fn update_command(h: &Harness, install: &Install, binary: &Path, auto_updates: bool) -> Option<UpdateCommand> {
    let own = || h.self_update.map(|args| UpdateCommand::new(binary, args));
    match install {
        Install::Npm { package, prefix } => Some(UpdateCommand::new(prefix.join("bin/npm"), &["install", "-g", &format!("{package}@latest")])),
        Install::Bun { package } => Some(UpdateCommand::new("bun", &["add", "-g", &format!("{package}@latest")])),
        Install::Pnpm { package } => Some(UpdateCommand::new("pnpm", &["add", "-g", &format!("{package}@latest")])),
        Install::Brew { formula, prefix, tap } => {
            let name = tap.as_ref().map_or_else(|| formula.clone(), |t| format!("{t}/{formula}"));
            Some(UpdateCommand::new(prefix.join("bin/brew"), &["upgrade", &name]))
        }
        Install::Cask { cask, prefix } => match own().filter(|_| auto_updates) {
            Some(cmd) => Some(cmd),
            None => Some(UpdateCommand::new(prefix.join("bin/brew"), &["upgrade", "--cask", cask])),
        },
        Install::Native => own().or_else(|| h.installer.map(|script| UpdateCommand::new("/bin/sh", &["-c", script]))),
    }
}

/// One agent CLI as the last check found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentVersion {
    /// `AgentId::key()`.
    pub agent: String,
    pub name: String,
    /// Where it is on PATH.
    pub binary: PathBuf,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub install: Install,
    pub command: Option<UpdateCommand>,
    /// Why the newest version isn't known ("offline", "no feed").
    #[serde(default)]
    pub error: Option<String>,
}

impl AgentVersion {
    /// A newer version is out, and Trek knows how to install it.
    pub fn update_available(&self) -> bool {
        self.command.is_some() && matches!((&self.latest, &self.installed), (Some(l), Some(i)) if is_newer(l, i))
    }
}

/// When a check couldn't reach an agent's feed, keep the newest version the check before found
/// for it, as long as nothing was installed since (else it may be stale).
pub fn carry_over(fresh: &mut [AgentVersion], before: &[AgentVersion]) {
    for v in fresh.iter_mut().filter(|v| v.latest.is_none()) {
        if let Some(old) = before.iter().find(|o| o.agent == v.agent && o.installed == v.installed && o.install == v.install) {
            v.latest = old.latest.clone();
            if v.command.is_none() {
                v.command = old.command.clone();
            }
        }
    }
}

/// The last check's results, as cached between launches.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Milliseconds since the epoch.
    pub checked_at: i64,
    pub agents: Vec<AgentVersion>,
}

impl Snapshot {
    fn path() -> PathBuf {
        crate::paths::data_dir().join("agent-versions.json")
    }

    pub fn load() -> Snapshot {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::write(Self::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// How often a check runs in the background.
pub const CHECK_EVERY_MS: i64 = 12 * 60 * 60 * 1000;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("Trek/{}", crate::VERSION))
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap_or_default()
}

async fn fetch_latest(client: &reqwest::Client, source: &Source) -> Result<Latest, String> {
    let mut last = "Nothing to ask".to_string();
    for url in urls(source) {
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => match r.text().await {
                Ok(body) => match parse_latest(source, &body) {
                    Some(l) => return Ok(l),
                    None => last = format!("No version in {url}"),
                },
                Err(e) => last = format!("Couldn't read {url}: {e}"),
            },
            Ok(r) => last = format!("{url} answered {}", r.status()),
            Err(e) if e.is_connect() || e.is_timeout() => last = "Offline, or the release feed didn't answer".into(),
            Err(e) => last = format!("Couldn't reach {url}: {e}"),
        }
    }
    Err(last)
}

/// What `<binary> --version` says, parsed.
pub async fn installed_version(binary: &Path) -> Option<String> {
    parse_version(&crate::detect::version_of(binary).await?)
}

/// Check one agent: `None` when its CLI isn't installed, or what's on PATH under its name is
/// another tool (a global npm package by another name).
pub async fn check(h: &Harness, client: &reqwest::Client) -> Option<AgentVersion> {
    let binary = which(h.binary)?;
    let resolved = std::fs::canonicalize(&binary).unwrap_or_else(|_| binary.clone());
    let install = detect_install(&resolved);
    if let (Some(found), Some(ours)) = (install.package(), h.npm) {
        if found != ours {
            return None;
        }
    }
    let installed = installed_version(&binary).await;
    let latest = match source(h, &install) {
        Some(s) => fetch_latest(client, &s).await,
        None => Err("Its vendor publishes no release feed Trek can read".into()),
    };
    let auto_updates = latest.as_ref().is_ok_and(|l| l.auto_updates);
    Some(AgentVersion {
        agent: h.agent.into(),
        name: h.name(),
        command: update_command(h, &install, &binary, auto_updates),
        binary,
        installed,
        install,
        latest: latest.as_ref().ok().map(|l| l.version.clone()),
        error: latest.err(),
    })
}

/// Check every agent CLI on this Mac, side by side. Takes a few seconds; never fails as a whole.
pub async fn check_all() -> Vec<AgentVersion> {
    let client = client();
    futures::future::join_all(HARNESSES.iter().map(|h| check(h, &client))).await.into_iter().flatten().collect()
}

/// How an update went.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Now at `version` (as `--version` says afterwards).
    Updated { version: String, output: String },
    /// `summary` is a sentence; `output` what the command printed.
    Failed { summary: String, output: String },
}

/// Terminal colours and cursor moves out of a command's output.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters, then one final byte in @–~.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\r' {
            // Progress lines redraw over themselves: keep the line, drop the carriage return.
            if chars.peek() != Some(&'\n') {
                out.push('\n');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// How long an update may take before Trek gives up on it.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Run `v`'s update command, then ask the CLI its version again: a command that exits fine but
/// leaves the old version installed failed too.
pub async fn run_update(v: &AgentVersion) -> Outcome {
    let Some(cmd) = &v.command else {
        return Outcome::Failed { summary: format!("Trek doesn't know how to update {}.", v.name), output: String::new() };
    };
    let program = if cmd.program.is_absolute() { Some(cmd.program.clone()) } else { which(&cmd.program.to_string_lossy()) };
    let Some(program) = program.filter(|p| p.is_file()) else {
        return Outcome::Failed { summary: format!("{} isn't installed.", cmd.program.display()), output: String::new() };
    };
    let run = tokio::process::Command::new(&program)
        .args(&cmd.args)
        .env("PATH", login_path())
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(UPDATE_TIMEOUT, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Outcome::Failed { summary: format!("Couldn't run {}: {e}", cmd.shown()), output: String::new() },
        Err(_) => return Outcome::Failed { summary: format!("{} took longer than 15 minutes, so Trek stopped it.", cmd.shown()), output: String::new() },
    };
    let output = strip_ansi(&format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))).trim().to_string();
    if !out.status.success() {
        let code = out.status.code().map_or("a signal".to_string(), |c| format!("code {c}"));
        return Outcome::Failed { summary: format!("{} stopped with {code}.", cmd.shown()), output };
    }
    // The binary on PATH may have moved (a new keg): look it up again.
    let binary = harness(&v.agent).and_then(|h| which(h.binary)).unwrap_or_else(|| v.binary.clone());
    verify(v, installed_version(&binary).await, output)
}

/// What an update that ran without error came to, given the version installed afterwards.
pub fn verify(v: &AgentVersion, now: Option<String>, output: String) -> Outcome {
    match now {
        Some(now) if v.latest.as_ref().is_none_or(|l| !is_newer(l, &now)) => Outcome::Updated { version: now, output },
        Some(now) => Outcome::Failed { summary: format!("{} is still at {now} after the update.", v.name), output },
        None => Outcome::Failed { summary: format!("{} didn't say its version after the update.", v.name), output },
    }
}

/// A pretend update for design review and end-to-end checks of the UI (`TREK_AGENT_UPDATES=dry-run`):
/// it waits a moment and reports the newest version, changing nothing.
pub async fn dry_run(v: &AgentVersion) -> Outcome {
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let shown = v.command.as_ref().map(|c| c.shown()).unwrap_or_default();
    Outcome::Updated { version: v.latest.clone().or_else(|| v.installed.clone()).unwrap_or_default(), output: format!("Dry run: Trek would have run `{shown}`. Nothing was changed.") }
}

/// Made-up agents with updates out, as MonoCode's card shows them, for design review
/// (`TREK_AGENT_UPDATES=mock`).
pub fn mock_versions() -> Vec<AgentVersion> {
    let entry = |agent: &str, installed: &str, latest: &str, install: Install, cmd: &str| {
        let mut parts = cmd.split(' ');
        let program = parts.next().unwrap_or_default();
        AgentVersion {
            agent: agent.into(),
            name: AgentId::from_key(agent).display_name(),
            binary: PathBuf::from(format!("/opt/homebrew/bin/{program}")),
            installed: Some(installed.into()),
            latest: Some(latest.into()),
            install,
            command: Some(UpdateCommand { program: program.into(), args: parts.map(String::from).collect() }),
            error: None,
        }
    };
    let npm = |p: &str| Install::Npm { package: p.into(), prefix: "/opt/homebrew".into() };
    vec![
        entry("codex", "0.159.2", "0.160.0", npm("@openai/codex"), "npm install -g @openai/codex@latest"),
        entry("opencode", "1.18.32", "1.18.34", Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) }, "brew upgrade anomalyco/tap/opencode"),
        entry("acp:pi", "0.85.1", "1.0.0", npm("@mariozechner/pi-coding-agent"), "npm install -g @mariozechner/pi-coding-agent@latest"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_out_of_every_clis_version_line() {
        for (out, want) in [
            ("2.1.289 (Claude Code)", "2.1.289"),
            ("codex-cli 0.160.0", "0.160.0"),
            ("1.18.34", "1.18.34"),
            ("GitHub Copilot CLI 1.0.91.\nRun 'copilot update' to check for updates.", "1.0.91"),
            ("0.42.0", "0.42.0"),
            ("grok 1.0.46 (2765805b9442) [stable]", "1.0.46"),
            ("devin 3000.11.3 (9c803229faa4)", "3000.11.3"),
            ("2026.10.01-e373342", "2026.10.01-e373342"),
            ("0.0.1791107882-gfe04cc", "0.0.1791107882-gfe04cc"),
            ("goose v1.9.3", "1.9.3"),
            ("qwen version: 0.24.7", "0.24.7"),
            ("Droid v0.233.0", "0.233.0"),
        ] {
            assert_eq!(parse_version(out).as_deref(), Some(want), "{out}");
        }
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("build 2765805b9442"), None);
    }

    #[test]
    fn newer_compares_number_by_number() {
        assert!(is_newer("0.160.0", "0.159.2"));
        assert!(is_newer("1.0.0", "0.85.1"));
        assert!(is_newer("1.18.34", "1.18.32"));
        assert!(is_newer("2.1.1", "0.42.0"));
        assert!(is_newer("1.10.0", "1.9.9"), "not string order");
        assert!(is_newer("1.2.1", "1.2"));
        assert!(!is_newer("1.2.0", "1.2"));
        assert!(!is_newer("2.1.289", "2.1.289"));
        assert!(!is_newer("2.1.285", "2.1.289"), "a stable channel behind latest is no update");
        assert!(is_newer("1.2.0", "1.2.0-beta.1"));
        assert!(!is_newer("1.2.0-beta.1", "1.2.0"));
        // Cursor's builds: date and hash.
        assert!(is_newer("2026.10.01-e373342", "2026.09.12-a1b2c3d"));
        assert!(!is_newer("2026.10.01-e373342", "2026.10.01-ffff000"));
        assert!(!is_newer("nightly", "1.0.0"));
    }

    #[test]
    fn how_it_was_installed_shows_in_where_it_lives() {
        let p = |s: &str| install_of(Path::new(s));
        assert_eq!(
            p("/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
            Install::Npm { package: "@anthropic-ai/claude-code".into(), prefix: "/opt/homebrew".into() }
        );
        assert_eq!(
            p("/Users/me/.nvm/versions/node/v22.3.0/lib/node_modules/@openai/codex/bin/codex.js"),
            Install::Npm { package: "@openai/codex".into(), prefix: "/Users/me/.nvm/versions/node/v22.3.0".into() }
        );
        assert_eq!(p("/usr/local/lib/node_modules/opencode-ai/bin/opencode"), Install::Npm { package: "opencode-ai".into(), prefix: "/usr/local".into() });
        assert_eq!(p("/Users/me/.bun/install/global/node_modules/@google/gemini-cli/dist/index.js"), Install::Bun { package: "@google/gemini-cli".into() });
        assert_eq!(p("/Users/me/Library/pnpm/global/5/node_modules/@qwen-code/qwen-code/cli.js"), Install::Pnpm { package: "@qwen-code/qwen-code".into() });
        assert_eq!(p("/opt/homebrew/Cellar/opencode/1.18.34/bin/opencode"), Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: None });
        assert_eq!(p("/usr/local/Cellar/block-goose-cli/1.9.3/bin/goose"), Install::Brew { formula: "block-goose-cli".into(), prefix: "/usr/local".into(), tap: None });
        assert_eq!(p("/opt/homebrew/Caskroom/copilot-cli/1.0.69/copilot"), Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() });
        // The vendors' own installers.
        for native in [
            "/Users/me/.codex/packages/standalone/releases/0.160.0-aarch64-apple-darwin/bin/codex",
            "/Users/me/.local/share/claude/versions/2.1.289",
            "/Users/me/.grok/downloads/grok-1.0.46-macos-aarch64",
            "/Users/me/.local/share/devin/cli/_versions/3000.11.3/bin/devin",
            "/Users/me/.local/share/cursor-agent/versions/2026.10.01-e373342/cursor-agent",
            "/Users/me/.opencode/bin/opencode",
        ] {
            assert_eq!(p(native), Install::Native, "{native}");
        }
    }

    #[test]
    fn a_formulas_tap_comes_from_its_receipt() {
        let receipt = r#"{"homebrew_version":"5.1.0","source":{"tap":"anomalyco/tap","spec":"stable","versions":{"stable":"1.18.34"}}}"#;
        assert_eq!(tap_from_receipt(receipt).as_deref(), Some("anomalyco/tap"));
        assert_eq!(tap_from_receipt(r#"{"source":{"tap":"homebrew/core"}}"#), None);
        assert_eq!(tap_from_receipt("not json"), None);
    }

    fn h(agent: &str) -> &'static Harness {
        harness(agent).unwrap()
    }

    #[test]
    fn the_newest_version_is_asked_of_the_channel_it_came_through() {
        let npm = Install::Npm { package: "@openai/codex".into(), prefix: "/opt/homebrew".into() };
        assert_eq!(source(h("codex"), &npm), Some(Source::Npm("@openai/codex".into())));
        assert_eq!(urls(&Source::Npm("@openai/codex".into())), ["https://registry.npmjs.org/@openai/codex/latest"]);
        let tapped = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) };
        let s = source(h("opencode"), &tapped).unwrap();
        assert_eq!(
            urls(&s),
            ["https://raw.githubusercontent.com/anomalyco/homebrew-tap/HEAD/Formula/opencode.rb", "https://raw.githubusercontent.com/anomalyco/homebrew-tap/HEAD/opencode.rb"]
        );
        let core = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: None };
        assert_eq!(urls(&source(h("opencode"), &core).unwrap()), ["https://formulae.brew.sh/api/formula/opencode.json"]);
        // Native installs: the vendor's feed, or the npm package its builds share versions with.
        assert_eq!(source(h("claude-code"), &Install::Native), Some(Source::Feed(Feed::ClaudeReleases)));
        assert_eq!(source(h("codex"), &Install::Native), Some(Source::Npm("@openai/codex".into())));
        assert_eq!(source(h("acp:grok"), &Install::Native), Some(Source::Feed(Feed::GrokStable)));
        assert_eq!(source(h("acp:devin"), &Install::Native), Some(Source::Feed(Feed::DevinManifest)));
        assert_eq!(source(h("acp:goose"), &Install::Native), Some(Source::Feed(Feed::GitHub("block/goose"))));
    }

    #[test]
    fn recorded_feed_responses_give_the_newest_version() {
        let npm = r#"{"name":"@openai/codex","version":"0.160.0","bin":{"codex":"bin/codex.js"},"dist":{"tarball":"https://registry.npmjs.org/@openai/codex/-/codex-0.160.0.tgz"}}"#;
        assert_eq!(parse_latest(&Source::Npm("@openai/codex".into()), npm).unwrap().version, "0.160.0");
        let formula = r#"{"name":"opencode","full_name":"opencode","tap":"homebrew/core","versions":{"stable":"2.0.20","head":null,"bottle":true},"revision":0}"#;
        assert_eq!(parse_latest(&Source::Formula("opencode".into()), formula).unwrap().version, "2.0.20");
        let cask = r#"{"token":"copilot-cli","version":"1.0.91","auto_updates":true,"url":"https://github.com/github/copilot-cli/releases/download/v1.0.91/copilot-darwin-arm64.tar.gz"}"#;
        assert_eq!(parse_latest(&Source::Cask("copilot-cli".into()), cask), Some(Latest { version: "1.0.91".into(), auto_updates: true }));
        let cask = r#"{"token":"some-app","version":"3.4.1,20261002","auto_updates":false}"#;
        assert_eq!(parse_latest(&Source::Cask("some-app".into()), cask).unwrap().version, "3.4.1");
        let tap = "# This file was generated by GoReleaser. DO NOT EDIT.\nclass Opencode < Formula\n  desc \"The AI coding agent built for the terminal.\"\n  homepage \"https://github.com/anomalyco/opencode\"\n  version \"1.18.34\"\n\n  depends_on \"ripgrep\"\n";
        assert_eq!(parse_latest(&Source::TapFormula { tap: "anomalyco/tap".into(), formula: "opencode".into() }, tap).unwrap().version, "1.18.34");
        assert_eq!(parse_latest(&Source::Feed(Feed::ClaudeReleases), "2.1.289\n").unwrap().version, "2.1.289");
        assert_eq!(parse_latest(&Source::Feed(Feed::GrokStable), "1.0.46").unwrap().version, "1.0.46");
        let devin = r#"{"version":"3000.11.3","platforms":{"aarch64-apple-darwin":{"url":"https://static.devin.ai/cli/3000.11.3/devin-3000.11.3-aarch64-apple-darwin.tar.gz"}}}"#;
        assert_eq!(parse_latest(&Source::Feed(Feed::DevinManifest), devin).unwrap().version, "3000.11.3");
        let cursor = "TEMP_EXTRACT_DIR=\"$HOME/.local/share/cursor-agent/versions/.tmp-2026.10.01-e373342-$(date +%s)\"\nDOWNLOAD_URL=\"https://downloads.cursor.com/lab/2026.10.01-e373342/${OS}/${ARCH}/agent-cli-package.tar.gz\"\n";
        assert_eq!(parse_latest(&Source::Feed(Feed::CursorInstaller), cursor).unwrap().version, "2026.10.01-e373342");
        let droid = "binary_name=\"droid\"\nVER=\"0.233.0\"\nBASE_URL=\"https://downloads.factory.ai\"\n";
        assert_eq!(parse_latest(&Source::Feed(Feed::DroidInstaller), droid).unwrap().version, "0.233.0");
        let github = r#"{"tag_name":"v1.9.3","name":"v1.9.3","draft":false}"#;
        assert_eq!(parse_latest(&Source::Feed(Feed::GitHub("block/goose")), github).unwrap().version, "1.9.3");
        // Error pages and HTML aren't versions.
        assert_eq!(parse_latest(&Source::Npm("x".into()), "<html>rate limited</html>"), None);
        assert_eq!(parse_latest(&Source::Feed(Feed::GrokStable), "<!DOCTYPE html>"), None);
    }

    #[test]
    fn updates_run_the_vendors_documented_command() {
        let bin = Path::new("/opt/homebrew/bin/x");
        let shown = |agent: &str, install: Install, auto: bool| update_command(h(agent), &install, bin, auto).map(|c| c.shown());
        let npm = |p: &str| Install::Npm { package: p.into(), prefix: "/opt/homebrew".into() };
        assert_eq!(shown("acp:kimi", npm("@moonshot-ai/kimi-code"), false).as_deref(), Some("npm install -g @moonshot-ai/kimi-code@latest"));
        // The npm that installed it, not whichever is first on PATH.
        let cmd = update_command(h("codex"), &npm("@openai/codex"), bin, false).unwrap();
        assert_eq!(cmd.program, Path::new("/opt/homebrew/bin/npm"));
        assert_eq!(shown("acp:gemini", Install::Bun { package: "@google/gemini-cli".into() }, false).as_deref(), Some("bun add -g @google/gemini-cli@latest"));
        let tapped = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) };
        assert_eq!(shown("opencode", tapped, false).as_deref(), Some("brew upgrade anomalyco/tap/opencode"));
        let core = Install::Brew { formula: "block-goose-cli".into(), prefix: "/usr/local".into(), tap: None };
        assert_eq!(update_command(h("acp:goose"), &core, bin, false).unwrap().program, Path::new("/usr/local/bin/brew"));
        // A cask that updates itself does so with its own command; another one through brew.
        let cask = Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() };
        assert_eq!(shown("acp:github-copilot", cask.clone(), true).as_deref(), Some("x update"));
        assert_eq!(shown("acp:github-copilot", cask, false).as_deref(), Some("brew upgrade --cask copilot-cli"));
        // Native installs: the CLI's own command, the install script, or nothing.
        assert_eq!(shown("claude-code", Install::Native, false).as_deref(), Some("x update"));
        assert_eq!(shown("opencode", Install::Native, false).as_deref(), Some("x upgrade"));
        let droid = update_command(h("droid"), &Install::Native, bin, false).unwrap();
        assert_eq!(droid.program, Path::new("/bin/sh"));
        assert_eq!(droid.shown(), "curl -fsSL https://app.factory.ai/cli | sh");
        assert_eq!(shown("acp:pi", Install::Native, false), None);
    }

    fn version(agent: &str, installed: &str, latest: Option<&str>) -> AgentVersion {
        AgentVersion {
            agent: agent.into(),
            name: agent.into(),
            binary: "/bin/x".into(),
            installed: Some(installed.into()),
            latest: latest.map(String::from),
            install: Install::Native,
            command: Some(UpdateCommand::new("/bin/x", &["update"])),
            error: None,
        }
    }

    #[test]
    fn an_offline_check_keeps_what_the_last_one_found() {
        let before = vec![version("codex", "0.159.2", Some("0.160.0")), version("acp:kimi", "0.42.0", Some("2.1.1"))];
        // Offline now; Kimi was updated by hand meanwhile, so its old find no longer applies.
        let mut fresh = vec![version("codex", "0.159.2", None), version("acp:kimi", "2.1.1", None)];
        carry_over(&mut fresh, &before);
        assert_eq!(fresh[0].latest.as_deref(), Some("0.160.0"));
        assert!(fresh[0].update_available());
        assert_eq!(fresh[1].latest, None);
        assert!(!fresh[1].update_available());
    }

    #[test]
    fn an_update_counts_once_the_new_version_answers() {
        let v = version("codex", "0.159.2", Some("0.160.0"));
        assert_eq!(verify(&v, Some("0.160.0".into()), "ok".into()), Outcome::Updated { version: "0.160.0".into(), output: "ok".into() });
        // Already past what the check found (a release came out meanwhile).
        assert!(matches!(verify(&v, Some("0.161.0".into()), String::new()), Outcome::Updated { .. }));
        let Outcome::Failed { summary, .. } = verify(&v, Some("0.159.2".into()), String::new()) else { panic!() };
        assert_eq!(summary, "codex is still at 0.159.2 after the update.");
        assert!(matches!(verify(&v, None, String::new()), Outcome::Failed { .. }));
    }

    #[test]
    fn command_output_loses_its_terminal_codes() {
        assert_eq!(strip_ansi("\u{1b}[32madded\u{1b}[0m 1 package\r\nok"), "added 1 package\nok");
        assert_eq!(strip_ansi("10%\r50%\r100%"), "10%\n50%\n100%");
    }

    #[test]
    fn every_harness_is_an_agent_trek_knows() {
        for h in HARNESSES {
            let id = AgentId::from_key(h.agent);
            assert_eq!(id.key(), h.agent);
            assert_ne!(h.name(), h.agent.trim_start_matches("acp:"), "{} has a display name", h.agent);
            assert!(h.feed != Feed::Npm || h.npm.is_some(), "{} needs its package for its feed", h.agent);
            assert!(h.self_update.is_some() || h.installer.is_some() || h.npm.is_some(), "{} can be updated somehow", h.agent);
        }
    }

    #[test]
    fn snapshots_round_trip() {
        let snap = Snapshot { checked_at: 42, agents: mock_versions() };
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(back, snap);
        assert!(snap.agents.iter().all(|a| a.update_available()));
    }

    /// The real check on this machine, read-only: `cargo test -p trek-core live_agent_versions -- --ignored --nocapture`.
    #[test]
    #[ignore = "asks the network and the agent CLIs installed here"]
    fn live_agent_versions() {
        let found = crate::runtime().block_on(check_all());
        for v in &found {
            let cmd = v.command.as_ref().map(|c| c.shown()).unwrap_or_default();
            println!("{:<16} {:<10} {:>22} -> {:<22} update: {:<5} via `{cmd}`{}", v.name, v.install.label(), v.installed.as_deref().unwrap_or("?"), v.latest.as_deref().unwrap_or("?"), v.update_available(), v.error.as_ref().map(|e| format!("  ({e})")).unwrap_or_default());
        }
    }
}
