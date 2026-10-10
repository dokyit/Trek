//! Agent CLI updates: which version of each agent CLI is installed, how it was installed (read
//! from where its binary really lives), the newest version on that same channel, and the command
//! that updates it the way its vendor documents.
//!
//! Channels: npm (and Bun, pnpm) global packages are asked of the npm registry and updated with
//! the package manager that installed them; Homebrew formulae and casks (macOS only) are asked of
//! formulae.brew.sh (a third-party tap's formula of its GitHub repository) and updated with
//! `brew upgrade`; WinGet packages (Windows: Claude Code's and Copilot CLI's) with `winget
//! upgrade`; anything else came from the vendor's own installer, whose release feed says what's
//! newest and whose CLI updates itself (`claude update`, `grok update`, …).
//!
//! On Windows the layouts differ, not the channels: npm's global prefix has no `lib` folder
//! (`%APPDATA%\npm\node_modules\<package>`), a package's shim is a `.cmd` (read for the package it
//! runs), package managers are `.cmd` files found with `PATHEXT`, the PATH separator is `;`, and
//! scripts (the package move, a vendor's `install.ps1`) run through PowerShell, not `/bin/sh`.
//! The release feeds name versions only, not per-platform downloads, so they are the same on both.
//!
//! Everything here is read-only except `run_update`. Checks never fail as a whole: an agent
//! whose feed can't be reached (offline) keeps the newest version the last check found
//! (`carry_over`), and the results are cached (`Snapshot`) so a launch shows them at once.

use crate::detect::{login_path, which};
use crate::types::AgentId;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::Duration;

/// What separates the folders of a PATH string.
const PATH_SEP: char = if cfg!(windows) { ';' } else { ':' };

/// An agent CLI Trek can keep up to date.
#[derive(Debug, Clone, Copy)]
pub struct Harness {
    /// `AgentId::key()` of the agent it runs.
    pub agent: &'static str,
    pub binary: &'static str,
    /// The npm package it's published as.
    pub npm: Option<&'static str>,
    /// The package it was published as before it moved to `npm`: an install of that one is still
    /// this agent, and updates by moving to the new package.
    pub moved_from: Option<&'static str>,
    /// Later major versions published as packages of their own, `(major, package)`: an install at
    /// that major is that package's (OpenCode 2 is `@opencode/cli`, 1.x `opencode-ai`).
    pub majors: &'static [(u64, &'static str)],
    /// Its own update command, run with the installed binary: how a native install updates.
    pub self_update: Option<&'static [&'static str]>,
    /// The vendor's install script, for a native install with no update command of its own
    /// (re-running it is how the vendor says to update).
    pub installer: Option<&'static str>,
    /// Where a native install's newest release is published.
    pub feed: Feed,
    /// The Homebrew formulae and casks that are this CLI. Another formula with the same binary
    /// name (`grok`, a regex tool) isn't the agent: Trek leaves it alone.
    pub brew: &'static [&'static str],
    /// The agent's ACP adapter rather than its CLI: a row of its own beside the CLI's.
    pub adapter: bool,
}

impl Harness {
    pub fn name(&self) -> String {
        let name = AgentId::from_key(self.agent).display_name();
        if self.adapter { format!("{name} ACP adapter") } else { name }
    }

    /// The binary `detect` finds the agent by: only agents Trek detects are checked, so a CLI
    /// that merely shares a name (`amp`, an editor) never is when its agent isn't installed.
    pub fn detected_by(&self) -> &'static str {
        match self.agent.strip_prefix("acp:") {
            Some(id) => crate::catalog::ACP_AGENTS.iter().find(|a| a.id == id).map_or(self.binary, |a| a.binary),
            None => self.binary,
        }
    }

    /// The npm package the version `installed` is published as.
    pub fn npm_for(&self, installed: Option<&str>) -> Option<&'static str> {
        let major = installed.and_then(numbers).and_then(|(n, _)| n.first().copied());
        major.and_then(|m| self.majors.iter().rev().find(|(at, _)| m >= *at)).map(|(_, p)| *p).or(self.npm)
    }

    /// `package` is the one this agent's package moved from.
    pub fn moved(&self, package: &str) -> bool {
        self.moved_from == Some(package)
    }

    /// `install` is this agent's: a global package by its own name, or one of its Homebrew
    /// formulae or casks. What else is on PATH under its name is another tool.
    pub fn owns(&self, install: &Install) -> bool {
        match install {
            Install::Npm { package, .. } | Install::Bun { package } | Install::Pnpm { package } => {
                self.npm == Some(package.as_str()) || self.moved(package) || self.majors.iter().any(|(_, p)| p == package)
            }
            Install::Brew { formula: name, .. } | Install::Cask { cask: name, .. } => self.brew.contains(&name.as_str()),
            Install::Winget { id } => self.winget().contains(&id.as_str()),
            Install::Volta | Install::Native => true,
        }
    }

    /// The WinGet package ids that are this CLI: Claude Code's `winget install Anthropic.ClaudeCode`
    /// (its documentation), and Copilot CLI's `GitHub.Copilot` (the folder its WinGet install
    /// lays out). Another package of the same binary name isn't the agent.
    pub fn winget(&self) -> &'static [&'static str] {
        match self.agent {
            "claude-code" => &["Anthropic.ClaudeCode"],
            "acp:github-copilot" => &["GitHub.Copilot"],
            _ => &[],
        }
    }

    /// `install` as it applies to this agent: a WinGet package that isn't one of its known ids
    /// is read as the vendor's own install (its CLI's `update`), not as a package Trek would
    /// `winget upgrade` on a guess.
    pub fn reading(&self, install: Install) -> Install {
        match install {
            Install::Winget { id } if !self.winget().contains(&id.as_str()) => Install::Native,
            other => other,
        }
    }

    /// The line that re-runs the vendor's installer (`installer`), as this platform's shell runs it:
    /// the `curl … | sh` of `installer`, or on Windows the PowerShell line the vendor documents
    /// (`catalog`'s install hint for the agent).
    pub fn installer_line(&self) -> Option<&'static str> {
        let unix = self.installer?;
        if !cfg!(windows) {
            return Some(unix);
        }
        crate::catalog::agent_setup(self.agent.strip_prefix("acp:").unwrap_or(self.agent)).map(|s| s.install)
    }
}

/// A vendor's own release feed, for installs that came from its installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// The npm package's `latest`: the vendor's native builds carry the same versions.
    Npm,
    /// Claude Code's installer channel (`downloads.claude.ai/…/latest`, a bare version).
    ClaudeReleases,
    /// Its stable channel, a little behind latest, for those who chose it
    /// (`autoUpdatesChannel: "stable"`): `claude update` keeps them on it.
    ClaudeStable,
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

/// The agent CLIs Trek runs, as `detect` and `catalog` know them. Amp and Pi run through ACP
/// adapters (separate packages): their CLIs and their adapters each have a row.
pub const HARNESSES: &[Harness] = &[
    Harness { agent: "claude-code", binary: "claude", npm: Some("@anthropic-ai/claude-code"), moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::ClaudeReleases, brew: &["claude-code"], adapter: false },
    Harness { agent: "codex", binary: "codex", npm: Some("@openai/codex"), moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::Npm, brew: &["codex"], adapter: false },
    Harness { agent: "opencode", binary: "opencode", npm: Some("opencode-ai"), moved_from: None, majors: &[(2, "@opencode/cli")], self_update: Some(&["upgrade"]), installer: None, feed: Feed::Npm, brew: &["opencode", "opencode-v2"], adapter: false },
    Harness { agent: "droid", binary: "droid", npm: None, moved_from: None, majors: &[], self_update: None, installer: Some("curl -fsSL https://app.factory.ai/cli | sh"), feed: Feed::DroidInstaller, brew: &["droid"], adapter: false },
    Harness { agent: "acp:cursor", binary: "cursor-agent", npm: None, moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::CursorInstaller, brew: &["cursor-cli"], adapter: false },
    Harness { agent: "acp:github-copilot", binary: "copilot", npm: Some("@github/copilot"), moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::Npm, brew: &["copilot-cli"], adapter: false },
    Harness { agent: "acp:gemini", binary: "gemini", npm: Some("@google/gemini-cli"), moved_from: None, majors: &[], self_update: None, installer: None, feed: Feed::Npm, brew: &["gemini-cli"], adapter: false },
    Harness { agent: "acp:kimi", binary: "kimi", npm: Some("@moonshot-ai/kimi-code"), moved_from: None, majors: &[], self_update: Some(&["upgrade"]), installer: None, feed: Feed::Npm, brew: &["kimi-cli"], adapter: false },
    Harness { agent: "acp:qwen-code", binary: "qwen", npm: Some("@qwen-code/qwen-code"), moved_from: None, majors: &[], self_update: None, installer: None, feed: Feed::Npm, brew: &["qwen-code"], adapter: false },
    Harness { agent: "acp:grok", binary: "grok", npm: None, moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::GrokStable, brew: &[], adapter: false },
    Harness { agent: "acp:devin", binary: "devin", npm: None, moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::DevinManifest, brew: &[], adapter: false },
    Harness { agent: "acp:goose", binary: "goose", npm: None, moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::GitHub("block/goose"), brew: &["block-goose-cli"], adapter: false },
    Harness { agent: "acp:amp", binary: "amp", npm: Some("@sourcegraph/amp"), moved_from: None, majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::Npm, brew: &[], adapter: false },
    Harness { agent: "acp:amp", binary: "amp-acp", npm: Some("amp-acp"), moved_from: None, majors: &[], self_update: None, installer: None, feed: Feed::Npm, brew: &[], adapter: true },
    Harness { agent: "acp:pi", binary: "pi", npm: Some("@earendil-works/pi-coding-agent"), moved_from: Some("@mariozechner/pi-coding-agent"), majors: &[], self_update: Some(&["update"]), installer: None, feed: Feed::Npm, brew: &[], adapter: false },
    Harness { agent: "acp:pi", binary: "pi-acp", npm: Some("pi-acp"), moved_from: None, majors: &[], self_update: None, installer: None, feed: Feed::Npm, brew: &[], adapter: true },
];

/// The harness a row is for, by its id (the CLI's binary name: unique, unlike agents, which
/// may have an adapter row too).
pub fn harness(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.binary == id)
}

/// How an agent CLI got onto this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Install {
    /// A global npm package, under `prefix`: `<prefix>/lib/node_modules/<package>`, or on Windows
    /// `<prefix>\node_modules\<package>` (`%APPDATA%\npm` unless the user set another).
    Npm { package: String, prefix: PathBuf },
    Bun { package: String },
    Pnpm { package: String },
    /// A Homebrew formula; `tap` is `None` for homebrew/core.
    Brew { formula: String, prefix: PathBuf, tap: Option<String> },
    Cask { cask: String, prefix: PathBuf },
    /// A WinGet package (`winget upgrade --id <id>`).
    Winget { id: String },
    /// A Volta-managed package: its binary on PATH is Volta's shim.
    Volta,
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
            Install::Winget { .. } => "WinGet",
            Install::Volta => "Volta",
            Install::Native => "Installer",
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

/// What `names` leaves out of `path`: its root (`/`, or `C:\`). A verbatim path, which is what
/// `canonicalize` gives on Windows (`\\?\C:\…`), comes back as the plain one: npm, handed the
/// verbatim form as its prefix, doesn't understand it.
fn root_of(path: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::VerbatimDisk(drive) => root.push(format!("{}:", drive as char)),
                Prefix::VerbatimUNC(server, share) => root.push(format!(r"\\{}\{}", server.to_string_lossy(), share.to_string_lossy())),
                _ => root.push(c.as_os_str()),
            },
            Component::RootDir => root.push(c.as_os_str()),
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        root.push("/");
    }
    root
}

fn rooted(root: &Path, parts: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    p.extend(parts);
    p
}

/// `run` is a stretch of folders in `parts`, one after the other. Windows paths don't care about case.
fn has_run(parts: &[String], run: &[&str]) -> bool {
    parts.windows(run.len()).any(|w| w.iter().zip(run).all(|(a, b)| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }))
}

/// The WinGet package whose folder (`…\WinGet\Packages\<Id>_<source>`) holds the binary.
fn winget_package(parts: &[String]) -> Option<String> {
    let i = parts.windows(2).position(|w| w[0].eq_ignore_ascii_case("WinGet") && w[1].eq_ignore_ascii_case("Packages"))?;
    let id = parts.get(i + 2)?.split('_').next()?;
    (!id.is_empty()).then(|| id.to_string())
}

/// How the binary at `resolved` (symlinks followed) was installed, from where it lives. A
/// formula's tap isn't in its path: `detect_install` reads it from the keg's receipt.
pub fn install_of(resolved: &Path) -> Install {
    let parts = names(resolved);
    let root = root_of(resolved);
    let at = |name: &str| parts.iter().position(|p| p == name);
    // Before Cellar: Volta itself may be a formula, its shim a link into its keg. On Windows
    // Volta's home is `%LOCALAPPDATA%\Volta`, its shims `bin\<tool>.exe` copies of one program.
    let volta_dir = has_run(&parts, &[".volta", "tools", "image", "packages"]) || (cfg!(windows) && has_run(&parts, &["Volta", "tools", "image", "packages"]));
    let volta_shim = parts.last().is_some_and(|n| n == "volta-shim") || (cfg!(windows) && parts.len() >= 3 && has_run(&parts[parts.len() - 3..parts.len() - 1], &["Volta", "bin"]));
    if volta_shim || volta_dir {
        return Install::Volta;
    }
    // Homebrew exists on macOS only: a Windows folder called Cellar is just a folder.
    if !cfg!(windows) {
        // Before node_modules: a formula may be an npm package installed into its keg
        // (gemini-cli, qwen-code), and only brew may change a keg.
        if let Some(i) = at("Cellar").filter(|i| parts.len() > i + 1) {
            return Install::Brew { formula: parts[i + 1].clone(), prefix: rooted(&root, &parts[..i]), tap: None };
        }
        if let Some(i) = at("Caskroom").filter(|i| parts.len() > i + 1) {
            return Install::Cask { cask: parts[i + 1].clone(), prefix: rooted(&root, &parts[..i]) };
        }
    }
    if cfg!(windows)
        && let Some(id) = winget_package(&parts)
    {
        return Install::Winget { id };
    }
    if let Some(i) = at("node_modules") {
        let Some(first) = parts.get(i + 1) else { return Install::Native };
        let package = match (first.starts_with('@'), parts.get(i + 2)) {
            (true, Some(name)) => format!("{first}/{name}"),
            (true, None) => return Install::Native,
            (false, _) => first.clone(),
        };
        if has_run(&parts, &[".bun", "install", "global"]) {
            return Install::Bun { package };
        }
        if parts[..i].iter().any(|p| p == "pnpm") {
            return Install::Pnpm { package };
        }
        // npm's prefix holds `lib/node_modules` on Unix, and on Windows `node_modules` itself.
        let end = if i > 0 && parts[i - 1] == "lib" { i - 1 } else { i };
        return Install::Npm { package, prefix: rooted(&root, &parts[..end]) };
    }
    Install::Native
}

/// The tap a formula came from, from its keg's `INSTALL_RECEIPT.json`; `None` for homebrew/core.
pub fn tap_from_receipt(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tap = v["source"]["tap"].as_str()?;
    (tap != "homebrew/core" && tap.contains('/')).then(|| tap.to_string())
}

/// What a package manager's shell-script shim runs: pnpm puts one in its global bin folder
/// (`exec node "$basedir/global/5/node_modules/<package>/cli.js"`) where npm and Bun link.
pub fn shim_target(shim: &Path, text: &str) -> Option<PathBuf> {
    if !text.starts_with("#!") {
        return None;
    }
    let dir = shim.parent()?;
    let target = text.split('"').find_map(|t| t.strip_prefix("$basedir/").filter(|t| t.contains("node_modules/")))?;
    Some(lexical(&dir.join(target)))
}

/// `path` with its `.` and `..` folded away, without asking the disk: the shim's folder may be
/// reached through `..`.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// What a `.cmd` shim runs. npm (cmd-shim) and pnpm write one beside every global binary, and
/// the script it runs is named by a quoted `%dp0%\…\node_modules\<package>\…` (older ones
/// `%~dp0\…`), `dp0` being the shim's own folder:
/// `endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*`.
pub fn cmd_shim_target(shim: &Path, text: &str) -> Option<PathBuf> {
    let dir = shim.parent()?;
    let target = text.split('"').find_map(|t| t.strip_prefix("%dp0%").or_else(|| t.strip_prefix("%~dp0")).filter(|t| t.contains("node_modules")))?;
    Some(lexical(&dir.join(target.trim_start_matches(['\\', '/']))))
}

/// What a Bun shim runs. `bun add -g` on Windows leaves `<name>.exe` (one launcher program for all)
/// beside `<name>.bunx`, whose UTF-16 text holds the path of the script, relative to the folder
/// or absolute. Read for the first path in it that goes through `node_modules`.
pub fn bunx_target(shim: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16_lossy(&units);
    let path = text.split(['\0', '"']).find(|t| t.contains("node_modules"))?.trim_start_matches(r"\??\");
    let path = Path::new(path);
    Some(lexical(&if path.is_absolute() { path.to_path_buf() } else { shim.parent()?.join(path) }))
}

/// The script a binary runs: what a package manager's shim points at (a shell script's, or on
/// Windows a `.cmd`'s or Bun's), or itself.
pub fn script_of(resolved: &Path) -> PathBuf {
    let small = std::fs::metadata(resolved).is_ok_and(|m| m.len() < 8 * 1024);
    let extension = resolved.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let target = match extension.as_deref() {
        Some("cmd" | "bat") if cfg!(windows) => small.then(|| std::fs::read_to_string(resolved).ok()).flatten().and_then(|t| cmd_shim_target(resolved, &t)),
        Some("exe") if cfg!(windows) => std::fs::read(resolved.with_extension("bunx")).ok().filter(|b| b.len() < 8 * 1024).and_then(|b| bunx_target(resolved, &b)),
        _ => small.then(|| std::fs::read_to_string(resolved).ok()).flatten().and_then(|t| shim_target(resolved, &t)),
    };
    target.unwrap_or_else(|| resolved.to_path_buf())
}

/// A vendor installer's marker (Pi's `managed-install.json`): what's under its folder is the
/// installer's, though it lays out an npm prefix, and updates with the CLI's own command.
const VENDOR_MARKERS: &[&str] = &["managed-install.json"];

/// `install_of`, reading through a script shim, with a formula's tap read from its keg.
pub fn detect_install(resolved: &Path) -> Install {
    let script = script_of(resolved);
    let install = install_of(&script);
    let vendors = || script.ancestors().skip(1).any(|dir| VENDOR_MARKERS.iter().any(|m| dir.join(m).is_file()));
    match install {
        Install::Npm { .. } if vendors() => Install::Native,
        Install::Brew { formula, prefix, .. } => {
            let parts = names(&script);
            let keg = parts.iter().position(|p| p == "Cellar").and_then(|i| parts.get(i + 2)).map(|v| prefix.join("Cellar").join(&formula).join(v));
            let tap = keg.and_then(|k| std::fs::read_to_string(k.join("INSTALL_RECEIPT.json")).ok()).and_then(|j| tap_from_receipt(&j));
            Install::Brew { formula, prefix, tap }
        }
        other => other,
    }
}

/// The folder of the npm package `script` is part of (`…/node_modules/<package>`).
pub fn package_dir(script: &Path) -> Option<PathBuf> {
    let parts = names(script);
    let i = parts.iter().rposition(|p| p == "node_modules")?;
    let first = parts.get(i + 1)?;
    let end = if first.starts_with('@') { i + 3 } else { i + 2 };
    (parts.len() >= end).then(|| rooted(&root_of(script), &parts[..end]))
}

/// The version in a package's `package.json`.
pub fn version_in_package_json(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_version(v["version"].as_str()?)
}

/// The version of the npm package the binary at `resolved` runs, from its `package.json`.
fn package_version(resolved: &Path) -> Option<String> {
    let dir = package_dir(&script_of(resolved))?;
    version_in_package_json(&std::fs::read_to_string(dir.join("package.json")).ok()?)
}

/// The version in a CLI's `--version` output: "2.1.289 (Claude Code)", "codex-cli 0.160.0",
/// "GitHub Copilot CLI 1.0.91.", "grok 1.0.46 (2765805b9442) [stable]" → the dotted number, with
/// any pre-release or build suffix ("2026.10.01-e373342"). Only `[0-9A-Za-z.+-]`: a version
/// goes into a shell line (`update_command`).
pub fn parse_version(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\''))
        .map(|t| t.trim_start_matches(['v', 'V']).trim_end_matches(['.', ':']))
        .find(|t| {
            let core = t.split(['-', '+']).next().unwrap_or_default();
            safe_version(t) &&
            core.contains('.') && core.split('.').all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(String::from)
}

/// `v` is made of `[0-9A-Za-z.+-]` only, so it's safe in a shell line as it is.
fn safe_version(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
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

/// Where to ask for `h`'s newest version, installed as `install` (at version `installed`);
/// `None` when its vendor publishes nowhere Trek can read.
pub fn source(h: &Harness, install: &Install, installed: Option<&str>) -> Option<Source> {
    match install {
        // An install of the package it moved from is compared with the new one: the old one's
        // `latest` stays where it was left.
        Install::Npm { package, .. } | Install::Bun { package } | Install::Pnpm { package } if h.moved(package) => h.npm.map(|p| Source::Npm(p.into())),
        Install::Npm { package, .. } | Install::Bun { package } | Install::Pnpm { package } => Some(Source::Npm(package.clone())),
        Install::Brew { formula, tap: None, .. } => Some(Source::Formula(formula.clone())),
        Install::Brew { formula, tap: Some(tap), .. } => Some(Source::TapFormula { tap: tap.clone(), formula: formula.clone() }),
        Install::Cask { cask, .. } => Some(Source::Cask(cask.clone())),
        Install::Volta => h.npm_for(installed).map(|p| Source::Npm(p.into())),
        // WinGet publishes what the vendor's own release feed does.
        Install::Native | Install::Winget { .. } => match h.feed {
            Feed::Npm => h.npm_for(installed).map(|p| Source::Npm(p.into())),
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
        // The feeds name a version, not a download: the same on every platform. Cursor's and
        // Factory's are their install scripts, and each has a Windows one that names it too.
        Source::Feed(feed) => match feed {
            Feed::Npm => vec![],
            Feed::ClaudeReleases => vec!["https://downloads.claude.ai/claude-code-releases/latest".into()],
            Feed::ClaudeStable => vec!["https://downloads.claude.ai/claude-code-releases/stable".into()],
            Feed::GrokStable => vec!["https://x.ai/cli/stable".into()],
            Feed::DevinManifest => vec!["https://static.devin.ai/cli/current/manifest.json".into()],
            Feed::CursorInstaller => vec!["https://cursor.com/install".into(), "https://cursor.com/install?win32=true".into()],
            Feed::DroidInstaller => vec!["https://app.factory.ai/cli".into(), "https://app.factory.ai/cli/windows".into()],
            Feed::GitHub(repo) => vec![format!("https://api.github.com/repos/{repo}/releases/latest")],
        },
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
            Feed::ClaudeReleases | Feed::ClaudeStable | Feed::GrokStable => plain(body.lines().next().map(|l| l.trim().to_string())),
            Feed::DevinManifest => plain(json()?["version"].as_str().map(String::from)),
            Feed::CursorInstaller => plain(between(body, "downloads.cursor.com/lab/", '/').map(String::from)),
            // `VER="0.233.0"` in the shell script, `$version = '0.237.0'` in the PowerShell one.
            Feed::DroidInstaller => plain(body.lines().find_map(|l| {
                let l = l.trim();
                let value = l.strip_prefix("VER=").or_else(|| l.strip_prefix("$version").and_then(|v| v.trim_start().strip_prefix('=')))?;
                Some(value.trim().trim_matches(['"', '\'']).to_string())
            })),
            Feed::GitHub(_) => plain(json()?["tag_name"].as_str().map(String::from)),
        },
    }
}

/// A command that updates an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCommand {
    /// A bare name is looked up on the PATH the command runs with.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Folders ahead of the login PATH while it runs.
    #[serde(default)]
    pub path: Vec<PathBuf>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// What particular exit codes mean, said instead of "stopped with code n" (a package move
    /// that put the old package back).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_exit: Vec<(i32, String)>,
    /// What to show instead of the program and its arguments, for a script that runs through
    /// PowerShell (`irm https://… | iex`) and does more than it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

/// The extensions `CreateProcess` can run, in the order `PATHEXT` gives them (its own default
/// when it names none of them).
fn runnable_extensions() -> Vec<String> {
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    let mut found: Vec<String> = pathext.split(';').map(|e| e.trim().to_ascii_lowercase()).filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.as_str())).collect();
    if found.is_empty() {
        found = [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec();
    }
    found
}

/// The file a bare `program` is, in `search_path`'s folders: that name on Unix; on Windows
/// that name with an extension from `PATHEXT` (`npm` is `npm.cmd`; the extensionless `npm`
/// beside it is a shell script Windows can't run, and is never taken).
fn resolve_program(program: &Path, search_path: &str) -> Option<PathBuf> {
    if program.is_absolute() {
        return Some(program.to_path_buf());
    }
    let name = program.to_string_lossy();
    let extensions = runnable_extensions();
    let names: Vec<String> = if !cfg!(windows) || extensions.iter().any(|e| name.to_ascii_lowercase().ends_with(e.as_str())) {
        vec![name.to_string()]
    } else {
        extensions.iter().map(|e| format!("{name}{e}")).collect()
    };
    std::env::split_paths(search_path).filter(|d| !d.as_os_str().is_empty()).find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
}

/// Windows PowerShell, the one every Windows has (PowerShell 7 isn't on all of them).
fn powershell() -> PathBuf {
    let system = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    system.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("powershell"))
}

impl UpdateCommand {
    pub fn new(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self { program: program.into(), args: args.iter().map(|a| a.to_string()).collect(), path: vec![], env: vec![], on_exit: vec![], display: None }
    }

    /// `script` run by PowerShell, which is how a Windows installer line (`irm … | iex`) and the
    /// package move run where there's no `/bin/sh`. Shown as `shown`.
    pub fn powershell(script: &str, shown: &str) -> Self {
        Self { display: Some(shown.to_string()), ..Self::new(powershell(), &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]) }
    }

    /// The PATH it runs with.
    pub fn search_path(&self) -> String {
        self.path.iter().map(|p| p.display().to_string()).chain(std::iter::once(login_path().to_string())).collect::<Vec<_>>().join(&PATH_SEP.to_string())
    }

    /// As the user would type it: "npm install -g @openai/codex@latest", "claude update".
    pub fn shown(&self) -> String {
        if let Some(shown) = &self.display {
            return shown.clone();
        }
        let program = self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.program.display().to_string());
        // A script runs through `sh -c`: show the line itself, or what it's named after it (the
        // script's `$0`) when it does more than it says.
        if program == "sh" && self.args.first().is_some_and(|a| a == "-c") {
            return self.args.get(2).unwrap_or(&self.args[1]).clone();
        }
        // On Windows `claude.exe` and `npm.cmd` are typed `claude` and `npm`.
        let program = match program.rsplit_once('.') {
            Some((stem, ext)) if cfg!(windows) && [".com", ".exe", ".bat", ".cmd"].contains(&format!(".{}", ext.to_ascii_lowercase()).as_str()) => stem.to_string(),
            _ => program,
        };
        std::iter::once(program).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

/// A package move's exit codes: the new package didn't install, and the old one is back
/// (`PUT_BACK`) or couldn't be put back either (`LOST`).
pub const PUT_BACK: i32 = 75;
pub const LOST: i32 = 76;

/// The vendor's documented way to update `h`, installed as `install` (at version `installed`)
/// with its binary at `binary`: the package manager that installed it, or for its own
/// installer, the CLI's own update command (or the install script again). A self-updating cask
/// updates itself too. `None` when there's no way Trek can run.
pub fn update_command(h: &Harness, install: &Install, binary: &Path, auto_updates: bool, installed: Option<&str>) -> Option<UpdateCommand> {
    let own = || h.self_update.map(|args| UpdateCommand::new(binary, args));
    // Installed as the package it moved from: that one goes first (its binary blocks the new
    // one's, and uninstalling it after a forced install takes the shared link with it), then the
    // new one comes in its place. Should that fail (offline, a registry error), the old one is
    // put back, from the package manager's cache if need be: never no agent at all.
    let moving = |remove: &str, add: &str, offline: &str, old: &str| {
        let new = h.npm?;
        // Into a shell line: only a version that can't run anything.
        let back = installed.filter(|v| safe_version(v)).map_or_else(|| old.to_string(), |v| format!("{old}@{v}"));
        let shown = format!("{remove} {old} && {add} {new}@latest");
        let name = h.name();
        let command = if cfg!(windows) {
            // The same steps in PowerShell, which reads a bare `@scope/name` as splatting: each
            // package is quoted. Stopping on an error makes a missing program exit 1, not go on.
            let step = |line: String, test: &str, then: String| format!("{line}; if ($LASTEXITCODE -{test}) {{ {then} }}");
            let script = [
                "$ErrorActionPreference = 'Stop'".to_string(),
                step(format!("{remove} '{old}'"), "ne 0", "exit $LASTEXITCODE".into()),
                step(format!("{add} '{new}@latest'"), "eq 0", "exit 0".into()),
                step(format!("{add} {offline}'{back}'"), "eq 0", format!("exit {PUT_BACK}")),
                format!("exit {LOST}"),
            ]
            .join("; ");
            UpdateCommand::powershell(&script, &shown)
        } else {
            // Into a shell line: only a version that can't run anything.
            let script = format!("{remove} {old} || exit $?; {add} {new}@latest && exit 0; {add} {offline}{back} && exit {PUT_BACK}; exit {LOST}");
            UpdateCommand::new("/bin/sh", &["-c", &script, &shown])
        };
        Some(UpdateCommand {
            on_exit: vec![
                (PUT_BACK, format!("Couldn't install {new}, so {back} was put back.")),
                (LOST, format!("Couldn't install {new}, nor put {back} back: {name} isn't installed now. Run {add} {new} to install it.")),
            ],
            ..command
        })
    };
    // Where an npm prefix keeps its programs: `<prefix>/bin`, and on Windows the prefix itself
    // (`npm.cmd`, `node.exe` and the shims of its packages, an nvm version's folder included).
    let npm_dir = |prefix: &Path| if cfg!(windows) { prefix.to_path_buf() } else { prefix.join("bin") };
    match install {
        Install::Npm { package, prefix } if h.moved(package) => moving("npm uninstall -g", "npm install -g", "--prefer-offline ", package).map(|c| UpdateCommand {
            path: vec![npm_dir(prefix)],
            env: vec![("npm_config_prefix".into(), prefix.display().to_string())],
            ..c
        }),
        Install::Bun { package } if h.moved(package) => moving("bun remove -g", "bun add -g", "", package),
        Install::Pnpm { package } if h.moved(package) => moving("pnpm remove -g", "pnpm add -g", "--prefer-offline ", package),
        // The npm and node of the install's own prefix come first (an nvm version's, Homebrew's),
        // and the prefix is set outright: a prefix of the user's own (`~/.npm-global`) has
        // neither, so the npm on PATH installs there.
        Install::Npm { package, prefix } => Some(UpdateCommand {
            path: vec![npm_dir(prefix)],
            env: vec![("npm_config_prefix".into(), prefix.display().to_string())],
            ..UpdateCommand::new("npm", &["install", "-g", &format!("{package}@latest")])
        }),
        Install::Volta => h.npm_for(installed).map(|p| UpdateCommand::new("volta", &["install", &format!("{p}@latest")])),
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
        // `winget upgrade`, as Claude Code's documentation says: WinGet's installs don't update themselves.
        Install::Winget { id } => Some(UpdateCommand::new("winget", &["upgrade", "--id", id, "--exact", "--accept-package-agreements", "--accept-source-agreements"])),
        Install::Native => own().or_else(|| {
            h.installer_line().map(|line| if cfg!(windows) { UpdateCommand::powershell(line, line) } else { UpdateCommand::new("/bin/sh", &["-c", line]) })
        }),
    }
}

/// One agent CLI as the last check found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentVersion {
    /// Which row it is: its harness's binary name (`harness`). An agent may have two, its CLI
    /// and its ACP adapter.
    #[serde(default)]
    pub id: String,
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
    /// The check could compare: it knows both the installed version and the newest one.
    pub fn compared(&self) -> bool {
        self.installed.is_some() && self.latest.is_some()
    }

    /// A newer version is out, and Trek knows how to install it.
    pub fn update_available(&self) -> bool {
        self.command.is_some() && matches!((&self.latest, &self.installed), (Some(l), Some(i)) if is_newer(l, i))
    }
}

/// When a check couldn't reach an agent's feed, keep the newest version the check before found
/// for it, as long as nothing was installed since (else it may be stale).
pub fn carry_over(fresh: &mut [AgentVersion], before: &[AgentVersion]) {
    for v in fresh.iter_mut().filter(|v| v.latest.is_none()) {
        if let Some(old) = before.iter().find(|o| o.id == v.id && o.installed == v.installed && o.install == v.install) {
            v.latest = old.latest.clone();
            // With it, the command that check worked out for it: how an install updates may
            // depend on what its feed said (a self-updating cask), unknown this time.
            if old.command.is_some() {
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
        let mut snap: Snapshot = std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        // Cached before rows had ids: one row per agent, its CLI's.
        for v in snap.agents.iter_mut().filter(|v| v.id.is_empty()) {
            if let Some(h) = HARNESSES.iter().find(|h| h.agent == v.agent && !h.adapter) {
                v.id = h.binary.into();
            }
        }
        snap.agents.retain(|v| !v.id.is_empty());
        snap
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

/// The version of `h` installed at `binary`: what `--version` says, or its npm package's
/// `package.json` when it says nothing. ACP adapters aren't asked at all: they don't answer
/// `--version`, they start serving ACP.
pub async fn installed_version(h: &Harness, binary: &Path) -> Option<String> {
    if !h.adapter
        && let Some(v) = crate::detect::version_of(binary).await.and_then(|out| parse_version(&out))
    {
        return Some(v);
    }
    package_version(&std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf()))
}

/// Claude Code's settings put it on its stable channel (`autoUpdatesChannel`).
pub fn claude_on_stable(settings: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(settings).ok().is_some_and(|v| v["autoUpdatesChannel"].as_str() == Some("stable"))
}

/// Read-only: the channel the user picked in Claude Code's own settings.
fn claude_channel_stable() -> bool {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".claude"));
    std::fs::read_to_string(dir.join("settings.json")).is_ok_and(|s| claude_on_stable(&s))
}

/// Check one agent: `None` when its agent isn't installed (Trek doesn't detect it), or what's
/// on PATH under its name is another tool (another package, another formula).
pub async fn check(h: &Harness, client: &reqwest::Client) -> Option<AgentVersion> {
    which(h.detected_by())?;
    let binary = which(h.binary)?;
    let resolved = std::fs::canonicalize(&binary).unwrap_or_else(|_| binary.clone());
    let install = h.reading(detect_install(&resolved));
    if !h.owns(&install) {
        return None;
    }
    let installed = installed_version(h, &binary).await;
    let source = source(h, &install, installed.as_deref()).map(|s| match s {
        Source::Feed(Feed::ClaudeReleases) if claude_channel_stable() => Source::Feed(Feed::ClaudeStable),
        s => s,
    });
    let latest = match source {
        Some(s) => fetch_latest(client, &s).await,
        None => Err("Its vendor publishes no release feed Trek can read".into()),
    };
    let auto_updates = latest.as_ref().is_ok_and(|l| l.auto_updates);
    Some(AgentVersion {
        id: h.binary.into(),
        agent: h.agent.into(),
        name: h.name(),
        command: update_command(h, &install, &binary, auto_updates, installed.as_deref()),
        binary,
        installed,
        install,
        latest: latest.as_ref().ok().map(|l| l.version.clone()),
        error: latest.err(),
    })
}

/// Check every agent CLI on this computer, side by side. Takes a few seconds; never fails as a whole.
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
    let path = cmd.search_path();
    let Some(program) = resolve_program(&cmd.program, &path).filter(|p| p.is_file()) else {
        return Outcome::Failed { summary: format!("{} isn't installed.", cmd.program.display()), output: String::new() };
    };
    let mut run = tokio::process::Command::new(&program);
    run.args(&cmd.args).env("PATH", path).envs(cmd.env.iter().map(|(k, v)| (k, v))).env("NO_COLOR", "1").stdin(std::process::Stdio::null()).kill_on_drop(true);
    // Trek has no console to share: without this each `.cmd` and PowerShell opens one.
    #[cfg(windows)]
    run.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    let run = run.output();
    let out = match tokio::time::timeout(UPDATE_TIMEOUT, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Outcome::Failed { summary: format!("Couldn't run {}: {e}", cmd.shown()), output: String::new() },
        Err(_) => return Outcome::Failed { summary: format!("{} took longer than 15 minutes, so Trek stopped it.", cmd.shown()), output: String::new() },
    };
    let output = strip_ansi(&format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))).trim().to_string();
    if !out.status.success() {
        if let Some((_, said)) = out.status.code().and_then(|c| cmd.on_exit.iter().find(|(e, _)| *e == c)) {
            return Outcome::Failed { summary: said.clone(), output };
        }
        let code = out.status.code().map_or("a signal".to_string(), |c| format!("code {c}"));
        return Outcome::Failed { summary: format!("{} stopped with {code}.", cmd.shown()), output };
    }
    // The binary on PATH may have moved (a new keg): look it up again.
    let Some(h) = harness(&v.id) else { return verify(v, None, output) };
    let binary = which(h.binary).unwrap_or_else(|| v.binary.clone());
    verify(v, installed_version(h, &binary).await, output)
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
            id: HARNESSES.iter().find(|h| h.agent == agent && !h.adapter).map_or(agent, |h| h.binary).into(),
            agent: agent.into(),
            name: AgentId::from_key(agent).display_name(),
            binary: PathBuf::from(format!("/opt/homebrew/bin/{program}")),
            installed: Some(installed.into()),
            latest: Some(latest.into()),
            install,
            command: Some(UpdateCommand { args: parts.map(String::from).collect(), ..UpdateCommand::new(program, &[]) }),
            error: None,
        }
    };
    let npm = |p: &str| Install::Npm { package: p.into(), prefix: "/opt/homebrew".into() };
    vec![
        entry("codex", "0.159.2", "0.160.0", npm("@openai/codex"), "npm install -g @openai/codex@latest"),
        entry("opencode", "1.18.32", "1.18.34", Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) }, "brew upgrade anomalyco/tap/opencode"),
        entry("acp:pi", "0.99.2", "1.0.2", npm("@earendil-works/pi-coding-agent"), "npm install -g @earendil-works/pi-coding-agent@latest"),
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
            // Gemini (yargs' `.version()`) and Pi (`console.log(VERSION)`) print the bare number.
            ("0.62.0\n", "0.62.0"),
            ("1.0.2", "1.0.2"),
        ] {
            assert_eq!(parse_version(out).as_deref(), Some(want), "{out}");
        }
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("build 2765805b9442"), None);
        // Nothing a shell would run: such a token isn't a version.
        assert_eq!(parse_version("1.2.3-$(touch x)"), None);
        assert_eq!(parse_version("1.2.3-`id`"), None);
        assert_eq!(parse_version("1.2.3-a$b 1.2.4"), Some("1.2.4".into()));
        assert_eq!(version_in_package_json(r#"{"version": "1.2.3-`id`"}"#), None);
    }

    #[test]
    fn an_unsafe_installed_version_stays_out_of_the_shell_line() {
        let old = Install::Npm { package: "@mariozechner/pi-coding-agent".into(), prefix: "/opt/homebrew".into() };
        let cmd = update_command(h("acp:pi"), &old, Path::new("/opt/homebrew/bin/pi"), false, Some("0.73.1-$(id)")).unwrap();
        assert!(cmd.args.iter().all(|a| !a.contains("$(id)")), "{:?}", cmd.args);
        let cmd = update_command(h("acp:pi"), &old, Path::new("/opt/homebrew/bin/pi"), false, Some("0.73.1")).unwrap();
        assert!(cmd.args.iter().any(|a| a.contains("@mariozechner/pi-coding-agent@0.73.1")), "{:?}", cmd.args);
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
        // Homebrew is macOS's: on Windows these are folders like any other (see
        // `homebrew_is_not_looked_for_on_windows`).
        if !cfg!(windows) {
            assert_eq!(p("/opt/homebrew/Cellar/opencode/1.18.34/bin/opencode"), Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: None });
            assert_eq!(p("/usr/local/Cellar/block-goose-cli/1.9.3/bin/goose"), Install::Brew { formula: "block-goose-cli".into(), prefix: "/usr/local".into(), tap: None });
            assert_eq!(p("/opt/homebrew/Caskroom/copilot-cli/1.0.69/copilot"), Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() });
            // Formulae that are npm packages installed into their keg: brew's, not npm's.
            assert_eq!(
                p("/opt/homebrew/Cellar/gemini-cli/0.46.0/libexec/lib/node_modules/@google/gemini-cli/dist/index.js"),
                Install::Brew { formula: "gemini-cli".into(), prefix: "/opt/homebrew".into(), tap: None }
            );
            assert_eq!(
                p("/usr/local/Cellar/qwen-code/0.24.7/libexec/lib/node_modules/@qwen-code/qwen-code/cli.js"),
                Install::Brew { formula: "qwen-code".into(), prefix: "/usr/local".into(), tap: None }
            );
            let keg = install_of(Path::new("/opt/homebrew/Cellar/gemini-cli/0.46.0/libexec/lib/node_modules/@google/gemini-cli/dist/index.js"));
            assert!(h("acp:gemini").owns(&keg));
            assert_eq!(update_command(h("acp:gemini"), &keg, Path::new("/opt/homebrew/bin/gemini"), false, None).unwrap().shown(), "brew upgrade gemini-cli");
        }
        // A global prefix of the user's own (npm's fix for EACCES).
        assert_eq!(
            p("/Users/me/.npm-global/lib/node_modules/@openai/codex/bin/codex.js"),
            Install::Npm { package: "@openai/codex".into(), prefix: "/Users/me/.npm-global".into() }
        );
        // Volta: the binary on PATH is its shim (itself maybe in Volta's own keg), or a package image.
        assert_eq!(p("/Users/me/.volta/bin/volta-shim"), Install::Volta);
        assert_eq!(p("/opt/homebrew/Cellar/volta/2.0.2/bin/volta-shim"), Install::Volta);
        assert_eq!(p("/Users/me/.volta/tools/image/packages/@openai/codex/lib/node_modules/@openai/codex/bin/codex.js"), Install::Volta);
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
    fn pnpm_shims_are_read_for_the_package_they_run() {
        // pnpm's global bin is a script, not a link.
        let shim = "#!/bin/sh\nbasedir=$(dirname \"$(echo \"$0\" | sed -e 's,\\\\,/,g')\")\n\ncase `uname` in\n    *CYGWIN*) basedir=`cygpath -w \"$basedir\"`;;\nesac\n\nif [ -x \"$basedir/node\" ]; then\n  exec \"$basedir/node\"  \"$basedir/global/5/node_modules/@qwen-code/qwen-code/cli.js\" \"$@\"\nelse\n  exec node  \"$basedir/global/5/node_modules/@qwen-code/qwen-code/cli.js\" \"$@\"\nfi\n";
        let at = Path::new("/Users/me/Library/pnpm/qwen");
        let target = shim_target(at, shim).unwrap();
        assert_eq!(target, Path::new("/Users/me/Library/pnpm/global/5/node_modules/@qwen-code/qwen-code/cli.js"));
        assert_eq!(install_of(&target), Install::Pnpm { package: "@qwen-code/qwen-code".into() });
        let up = "#!/bin/sh\nexec node \"$basedir/../lib/node_modules/pi-acp/dist/index.js\" \"$@\"\n";
        assert_eq!(shim_target(Path::new("/x/bin/pi-acp"), up).unwrap(), Path::new("/x/lib/node_modules/pi-acp/dist/index.js"));
        // A binary, or a script that runs no package, isn't a shim.
        assert_eq!(shim_target(at, "\u{7f}ELF"), None);
        assert_eq!(shim_target(at, "#!/bin/sh\nexec \"$basedir/real-binary\"\n"), None);
    }

    #[test]
    fn only_the_agents_own_packages_and_formulae_count() {
        let brew = |formula: &str| Install::Brew { formula: formula.into(), prefix: "/opt/homebrew".into(), tap: None };
        // `grok` the regex tool and `amp` the editor are formulae too: not the agents.
        assert!(!h("acp:grok").owns(&brew("grok")));
        assert!(!h("acp:amp").owns(&brew("amp")));
        assert!(h("acp:goose").owns(&brew("block-goose-cli")));
        assert!(h("opencode").owns(&Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) }));
        assert!(h("acp:github-copilot").owns(&Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() }));
        assert!(!h("codex").owns(&Install::Npm { package: "codex".into(), prefix: "/usr/local".into() }), "another npm package named codex");
        assert!(h("codex").owns(&Install::Npm { package: "@openai/codex".into(), prefix: "/usr/local".into() }));
        assert!(h("acp:grok").owns(&Install::Native));
        // Agents are found by what `detect` looks for: Amp and Pi by their adapters.
        assert_eq!(h("acp:amp").detected_by(), "amp-acp");
        assert_eq!(h("acp:pi").detected_by(), "pi-acp");
        assert_eq!(h("acp:github-copilot").detected_by(), "copilot");
        assert_eq!(h("claude-code").detected_by(), "claude");
        // The adapters have rows of their own.
        let adapter = harness("amp-acp").unwrap();
        assert!(adapter.adapter);
        assert_eq!(adapter.name(), "Amp ACP adapter");
        assert_eq!(update_command(adapter, &Install::Npm { package: "amp-acp".into(), prefix: "/opt/homebrew".into() }, Path::new("/opt/homebrew/bin/amp-acp"), false, None).unwrap().shown(), "npm install -g amp-acp@latest");
    }

    #[test]
    fn claude_on_its_stable_channel_is_compared_with_stable() {
        assert!(claude_on_stable(r#"{"model":"opus","autoUpdatesChannel":"stable"}"#));
        assert!(!claude_on_stable(r#"{"autoUpdatesChannel":"latest"}"#));
        assert!(!claude_on_stable("{}"));
        assert!(!claude_on_stable("not json"));
        assert_eq!(urls(&Source::Feed(Feed::ClaudeStable)), ["https://downloads.claude.ai/claude-code-releases/stable"]);
        assert_eq!(parse_latest(&Source::Feed(Feed::ClaudeStable), "2.1.285\n").unwrap().version, "2.1.285");
    }

    #[test]
    fn a_formulas_tap_comes_from_its_receipt() {
        let receipt = r#"{"homebrew_version":"5.1.0","source":{"tap":"anomalyco/tap","spec":"stable","versions":{"stable":"1.18.34"}}}"#;
        assert_eq!(tap_from_receipt(receipt).as_deref(), Some("anomalyco/tap"));
        assert_eq!(tap_from_receipt(r#"{"source":{"tap":"homebrew/core"}}"#), None);
        assert_eq!(tap_from_receipt("not json"), None);
    }

    /// The CLI row of `agent`.
    fn h(agent: &str) -> &'static Harness {
        HARNESSES.iter().find(|h| h.agent == agent && !h.adapter).unwrap()
    }

    #[test]
    fn opencode_2_updates_on_its_own_channel() {
        // OpenCode 2 is `@opencode/cli` on npm (binaries `opencode` and `opencode2`), the tap's
        // `opencode-v2` formula and homebrew/core's `opencode`; 1.x goes on as `opencode-ai`.
        let oc = h("opencode");
        let npm = install_of(Path::new("/usr/local/lib/node_modules/@opencode/cli/bin/opencode.exe"));
        assert_eq!(npm, Install::Npm { package: "@opencode/cli".into(), prefix: "/usr/local".into() });
        assert!(oc.owns(&npm));
        assert_eq!(source(oc, &npm, Some("2.0.26")), Some(Source::Npm("@opencode/cli".into())));
        let bin = Path::new("/usr/local/bin/opencode");
        assert_eq!(update_command(oc, &npm, bin, false, Some("2.0.26")).unwrap().shown(), "npm install -g @opencode/cli@latest");
        // The vendor's installer: its version says which line it's on.
        assert_eq!(source(oc, &Install::Native, Some("2.0.26")), Some(Source::Npm("@opencode/cli".into())));
        assert_eq!(source(oc, &Install::Native, Some("1.18.35")), Some(Source::Npm("opencode-ai".into())));
        assert_eq!(source(oc, &Install::Native, None), Some(Source::Npm("opencode-ai".into())));
        assert_eq!(update_command(oc, &Install::Volta, bin, false, Some("2.0.3")).unwrap().shown(), "volta install @opencode/cli@latest");
        assert_eq!(update_command(oc, &Install::Volta, bin, false, Some("1.18.35")).unwrap().shown(), "volta install opencode-ai@latest");
        let tapped = Install::Brew { formula: "opencode-v2".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) };
        assert!(oc.owns(&tapped));
        assert_eq!(update_command(oc, &tapped, bin, false, Some("2.0.25")).unwrap().shown(), "brew upgrade anomalyco/tap/opencode-v2");
        assert_eq!(urls(&source(oc, &tapped, Some("2.0.25")).unwrap())[1], "https://raw.githubusercontent.com/anomalyco/homebrew-tap/HEAD/opencode-v2.rb");
        // What 2's `--version` and the update feed say.
        assert_eq!(parse_version("opencode v2.0.26").as_deref(), Some("2.0.26"));
        assert_eq!(parse_latest(&Source::Npm("@opencode/cli".into()), r#"{"name":"@opencode/cli","version":"2.0.26"}"#).unwrap().version, "2.0.26");
        assert!(is_newer("2.0.26", "2.0.25") && !is_newer("1.18.35", "2.0.26"));
    }

    #[test]
    fn the_newest_version_is_asked_of_the_channel_it_came_through() {
        let npm = Install::Npm { package: "@openai/codex".into(), prefix: "/opt/homebrew".into() };
        assert_eq!(source(h("codex"), &npm, None), Some(Source::Npm("@openai/codex".into())));
        assert_eq!(urls(&Source::Npm("@openai/codex".into())), ["https://registry.npmjs.org/@openai/codex/latest"]);
        let tapped = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) };
        let s = source(h("opencode"), &tapped, None).unwrap();
        assert_eq!(
            urls(&s),
            ["https://raw.githubusercontent.com/anomalyco/homebrew-tap/HEAD/Formula/opencode.rb", "https://raw.githubusercontent.com/anomalyco/homebrew-tap/HEAD/opencode.rb"]
        );
        let core = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: None };
        assert_eq!(urls(&source(h("opencode"), &core, None).unwrap()), ["https://formulae.brew.sh/api/formula/opencode.json"]);
        // Native installs: the vendor's feed, or the npm package its builds share versions with.
        assert_eq!(source(h("claude-code"), &Install::Native, None), Some(Source::Feed(Feed::ClaudeReleases)));
        assert_eq!(source(h("codex"), &Install::Native, None), Some(Source::Npm("@openai/codex".into())));
        assert_eq!(source(h("acp:grok"), &Install::Native, None), Some(Source::Feed(Feed::GrokStable)));
        assert_eq!(source(h("acp:devin"), &Install::Native, None), Some(Source::Feed(Feed::DevinManifest)));
        assert_eq!(source(h("acp:goose"), &Install::Native, None), Some(Source::Feed(Feed::GitHub("block/goose"))));
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
        // Where an npm prefix keeps its programs, and how PATH strings are joined.
        let (dir, sep) = (|prefix: &str| if cfg!(windows) { PathBuf::from(prefix) } else { Path::new(prefix).join("bin") }, if cfg!(windows) { ";" } else { ":" });
        let shown = |agent: &str, install: Install, auto: bool| update_command(h(agent), &install, bin, auto, None).map(|c| c.shown());
        let npm = |p: &str| Install::Npm { package: p.into(), prefix: "/opt/homebrew".into() };
        assert_eq!(shown("acp:kimi", npm("@moonshot-ai/kimi-code"), false).as_deref(), Some("npm install -g @moonshot-ai/kimi-code@latest"));
        // The npm (and node) of the prefix it's in first on PATH, and that prefix set outright.
        let cmd = update_command(h("codex"), &npm("@openai/codex"), bin, false, None).unwrap();
        assert_eq!(cmd.program, Path::new("npm"));
        assert_eq!(cmd.path, [dir("/opt/homebrew")]);
        assert_eq!(cmd.env, [("npm_config_prefix".to_string(), "/opt/homebrew".to_string())]);
        assert!(cmd.search_path().starts_with(&format!("{}{sep}", dir("/opt/homebrew").display())), "{}", cmd.search_path());
        // A prefix of the user's own has no npm in it: the one on PATH installs into it.
        let own = Install::Npm { package: "@openai/codex".into(), prefix: "/Users/me/.npm-global".into() };
        let cmd = update_command(h("codex"), &own, bin, false, None).unwrap();
        assert_eq!(cmd.env, [("npm_config_prefix".to_string(), "/Users/me/.npm-global".to_string())]);
        assert_eq!(cmd.shown(), "npm install -g @openai/codex@latest");
        let nvm = Install::Npm { package: "@openai/codex".into(), prefix: "/Users/me/.nvm/versions/node/v22.3.0".into() };
        assert_eq!(update_command(h("codex"), &nvm, bin, false, None).unwrap().path, [dir("/Users/me/.nvm/versions/node/v22.3.0")]);
        assert_eq!(shown("codex", Install::Volta, false).as_deref(), Some("volta install @openai/codex@latest"));
        assert_eq!(shown("acp:grok", Install::Volta, false), None, "no package to install");
        assert_eq!(shown("acp:gemini", Install::Bun { package: "@google/gemini-cli".into() }, false).as_deref(), Some("bun add -g @google/gemini-cli@latest"));
        let tapped = Install::Brew { formula: "opencode".into(), prefix: "/opt/homebrew".into(), tap: Some("anomalyco/tap".into()) };
        assert_eq!(shown("opencode", tapped, false).as_deref(), Some("brew upgrade anomalyco/tap/opencode"));
        let core = Install::Brew { formula: "block-goose-cli".into(), prefix: "/usr/local".into(), tap: None };
        assert_eq!(update_command(h("acp:goose"), &core, bin, false, None).unwrap().program, Path::new("/usr/local/bin/brew"));
        // A cask that updates itself does so with its own command; another one through brew.
        let cask = Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() };
        assert_eq!(shown("acp:github-copilot", cask.clone(), true).as_deref(), Some("x update"));
        assert_eq!(shown("acp:github-copilot", cask, false).as_deref(), Some("brew upgrade --cask copilot-cli"));
        // Native installs: the CLI's own command, the install script, or nothing.
        assert_eq!(shown("claude-code", Install::Native, false).as_deref(), Some("x update"));
        assert_eq!(shown("opencode", Install::Native, false).as_deref(), Some("x upgrade"));
        let droid = update_command(h("droid"), &Install::Native, bin, false, None).unwrap();
        if cfg!(windows) {
            // Factory's PowerShell installer, re-run: it's the update.
            assert_eq!(droid.program.file_stem().unwrap().to_string_lossy().to_lowercase(), "powershell");
            assert_eq!(droid.shown(), "irm https://app.factory.ai/cli/windows | iex");
            assert_eq!(droid.args.last().unwrap(), "irm https://app.factory.ai/cli/windows | iex");
        } else {
            assert_eq!(droid.program, Path::new("/bin/sh"));
            assert_eq!(droid.shown(), "curl -fsSL https://app.factory.ai/cli | sh");
        }
        assert_eq!(shown("acp:pi", Install::Native, false).as_deref(), Some("x update"));
        assert_eq!(shown("acp:gemini", Install::Native, false), None, "no update command, no installer");
    }

    #[test]
    fn pi_moved_package_and_its_old_installs_move_with_it() {
        let pi = h("acp:pi");
        let new = Install::Npm { package: "@earendil-works/pi-coding-agent".into(), prefix: "/opt/homebrew".into() };
        let old = Install::Npm { package: "@mariozechner/pi-coding-agent".into(), prefix: "/opt/homebrew".into() };
        assert!(pi.owns(&new) && pi.owns(&old));
        assert!(!pi.owns(&Install::Npm { package: "pi".into(), prefix: "/opt/homebrew".into() }));
        // Both are compared with the new package: the old one's `latest` stopped at 0.73.1.
        assert_eq!(source(pi, &new, None), Some(Source::Npm("@earendil-works/pi-coding-agent".into())));
        assert_eq!(source(pi, &old, None), Some(Source::Npm("@earendil-works/pi-coding-agent".into())));
        assert_eq!(source(pi, &Install::Bun { package: "@mariozechner/pi-coding-agent".into() }, None), Some(Source::Npm("@earendil-works/pi-coding-agent".into())));
        let bin = Path::new("/opt/homebrew/bin/pi");
        assert_eq!(update_command(pi, &new, bin, false, None).unwrap().shown(), "npm install -g @earendil-works/pi-coding-agent@latest");
        // The old package goes first: its `pi` would block the new one's.
        let cmd = update_command(pi, &old, bin, false, Some("0.73.1")).unwrap();
        if cfg!(windows) {
            assert_eq!(cmd.program.file_stem().unwrap().to_string_lossy().to_lowercase(), "powershell");
            // Each package is quoted for PowerShell, where a bare `@scope/name` is splatting.
            let script = cmd.args.last().unwrap();
            assert!(script.contains("npm uninstall -g '@mariozechner/pi-coding-agent'; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"), "{script}");
            assert!(script.contains("npm install -g '@earendil-works/pi-coding-agent@latest'; if ($LASTEXITCODE -eq 0) { exit 0 }"), "{script}");
            assert!(script.contains("npm install -g --prefer-offline '@mariozechner/pi-coding-agent@0.73.1'; if ($LASTEXITCODE -eq 0) { exit 75 }"), "{script}");
            assert!(script.ends_with("; exit 76"), "{script}");
        } else {
            assert_eq!(cmd.program, Path::new("/bin/sh"));
        }
        assert_eq!(cmd.shown(), "npm uninstall -g @mariozechner/pi-coding-agent && npm install -g @earendil-works/pi-coding-agent@latest");
        assert_eq!(cmd.path, [if cfg!(windows) { PathBuf::from("/opt/homebrew") } else { PathBuf::from("/opt/homebrew/bin") }]);
        assert_eq!(cmd.env, [("npm_config_prefix".to_string(), "/opt/homebrew".to_string())]);
        let pnpm = update_command(pi, &Install::Pnpm { package: "@mariozechner/pi-coding-agent".into() }, bin, false, None).unwrap();
        assert_eq!(pnpm.shown(), "pnpm remove -g @mariozechner/pi-coding-agent && pnpm add -g @earendil-works/pi-coding-agent@latest");
        // An old install a check found then (`latest` from the new package) has an update out.
        let v = AgentVersion {
            id: "pi".into(),
            agent: "acp:pi".into(),
            name: pi.name(),
            binary: bin.into(),
            installed: Some("0.73.1".into()),
            latest: Some("1.0.2".into()),
            install: old.clone(),
            command: update_command(pi, &old, bin, false, Some("0.73.1")),
            error: None,
        };
        assert!(v.update_available());
    }

    /// A fake `npm` in `root` (`root\npm.cmd` on Windows, where the prefix itself is on PATH;
    /// `root/bin/npm` on Unix) that appends each line of arguments it's given to `log` and
    /// fails with `npm error <line>` for each of `fail`, exiting 1.
    fn fake_npm(root: &Path, log: &Path, fail: &[&str]) {
        #[cfg(windows)]
        {
            // The redirection comes first: `echo 0.73.1>> log` would redirect handle 1.
            let fails: String = fail.iter().map(|f| format!("if \"%*\"==\"{f}\" (>&2 echo npm error {f}& exit /b 1)\r\n")).collect();
            let npm = format!("@echo off\r\n>> \"{}\" echo %*\r\n{fails}exit /b 0\r\n", log.display());
            std::fs::write(root.join("npm.cmd"), npm).unwrap();
        }
        #[cfg(unix)]
        {
            let fails: String = fail.iter().map(|f| format!("  \"{f}\") echo \"npm error {f}\" >&2; exit 1;;\n")).collect();
            let npm = format!("#!/bin/sh\necho \"$*\" >> \"{}\"\ncase \"$*\" in\n{fails}esac\nexit 0\n", log.display());
            let fake = root.join("bin/npm");
            std::fs::write(&fake, npm).unwrap();
            std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
    }

    /// Pi's move to its new package, run against a fake `npm` that logs what it's asked and fails
    /// at `fail` (each a line of arguments): the old package is never simply gone.
    fn move_pi(fail: &[&str]) -> (Outcome, Vec<String>) {
        let root = std::env::temp_dir().join(format!("trek-pi-move-{}-{}", std::process::id(), fail.len()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let log = root.join("npm.log");
        fake_npm(&root, &log, fail);
        let old = Install::Npm { package: "@mariozechner/pi-coding-agent".into(), prefix: root.clone() };
        let v = AgentVersion {
            id: "pi".into(),
            agent: "acp:pi".into(),
            name: "Pi".into(),
            binary: root.join("bin/pi"),
            installed: Some("0.73.1".into()),
            latest: Some("1.0.2".into()),
            command: update_command(h("acp:pi"), &old, &root.join("bin/pi"), false, Some("0.73.1")),
            install: old,
            error: None,
        };
        let outcome = crate::runtime().block_on(run_update(&v));
        let ran = std::fs::read_to_string(&log).unwrap_or_default().lines().map(String::from).collect();
        let _ = std::fs::remove_dir_all(&root);
        (outcome, ran)
    }

    #[test]
    fn a_failed_package_move_puts_the_old_package_back() {
        let (remove, add, back) = ("uninstall -g @mariozechner/pi-coding-agent", "install -g @earendil-works/pi-coding-agent@latest", "install -g --prefer-offline @mariozechner/pi-coding-agent@0.73.1");
        // Offline mid-update: the new package doesn't come, the old one goes back from the cache.
        let (outcome, ran) = move_pi(&[add]);
        assert_eq!(ran, [remove, add, back]);
        let Outcome::Failed { summary, output } = outcome else { panic!("{outcome:?}") };
        assert_eq!(summary, "Couldn't install @earendil-works/pi-coding-agent, so @mariozechner/pi-coding-agent@0.73.1 was put back.");
        assert!(output.contains("npm error install -g @earendil-works"), "{output}");
        // Neither comes: the summary says Pi is gone and how to get it.
        let (outcome, ran) = move_pi(&[add, back]);
        assert_eq!(ran, [remove, add, back]);
        let Outcome::Failed { summary, .. } = outcome else { panic!() };
        assert!(summary.starts_with("Couldn't install @earendil-works/pi-coding-agent, nor put @mariozechner/pi-coding-agent@0.73.1 back: Pi isn't installed now."), "{summary}");
        // The old one wouldn't go: nothing else is tried, nothing changed.
        let (outcome, ran) = move_pi(&[remove]);
        assert_eq!(ran, [remove]);
        let Outcome::Failed { summary, .. } = outcome else { panic!() };
        assert_eq!(summary, "npm uninstall -g @mariozechner/pi-coding-agent && npm install -g @earendil-works/pi-coding-agent@latest stopped with code 1.");
    }

    #[test]
    fn pis_own_installer_updates_with_pi_update() {
        // Pi's installer lays out an npm prefix of its own, marked as its: the installer's.
        let root = std::env::temp_dir().join(format!("trek-pi-managed-{}", std::process::id()));
        let script = root.join("releases/1.0.2/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "#!/usr/bin/env node\n").unwrap();
        assert!(matches!(detect_install(&script), Install::Npm { .. }), "unmarked, it's an npm prefix");
        std::fs::write(root.join("managed-install.json"), r#"{"kind":"pi-managed-install","schemaVersion":1}"#).unwrap();
        assert_eq!(detect_install(&script), Install::Native);
        assert_eq!(update_command(h("acp:pi"), &Install::Native, Path::new("/Users/me/.pi/agent/bin/pi"), false, None).unwrap().shown(), "pi update");
        assert_eq!(source(h("acp:pi"), &Install::Native, None), Some(Source::Npm("@earendil-works/pi-coding-agent".into())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn adapters_versions_come_from_their_packages() {
        // amp-acp and pi-acp start serving ACP whatever their arguments: no `--version`.
        assert_eq!(package_dir(Path::new("/opt/homebrew/lib/node_modules/pi-acp/dist/index.js")).unwrap(), Path::new("/opt/homebrew/lib/node_modules/pi-acp"));
        assert_eq!(
            package_dir(Path::new("/Users/me/Library/pnpm/global/5/node_modules/@sourcegraph/amp/dist/main.js")).unwrap(),
            Path::new("/Users/me/Library/pnpm/global/5/node_modules/@sourcegraph/amp")
        );
        assert_eq!(package_dir(Path::new("/Users/me/.local/bin/grok")), None);
        assert_eq!(package_dir(Path::new("/x/node_modules/@scope")), None);
        let pi_acp = r#"{
  "name": "pi-acp",
  "version": "0.0.34",
  "description": "ACP adapter for pi coding agent",
  "bin": { "pi-acp": "dist/index.js" }
}"#;
        assert_eq!(version_in_package_json(pi_acp).as_deref(), Some("0.0.34"));
        let amp_acp = r#"{"name":"amp-acp","version":"0.9.0","type":"module","bin":{"amp-acp":"dist/index.js"}}"#;
        assert_eq!(version_in_package_json(amp_acp).as_deref(), Some("0.9.0"));
        assert_eq!(version_in_package_json("{}"), None);
        // Read from disk, through the binary's link (on Windows, through its `.cmd`: a symlink
        // there needs a privilege the tests can't count on).
        #[cfg(windows)]
        {
            let root = std::env::temp_dir().join(format!("trek-adapter-{}", std::process::id()));
            let pkg = root.join(r"node_modules\pi-acp");
            std::fs::create_dir_all(pkg.join("dist")).unwrap();
            std::fs::write(pkg.join("package.json"), pi_acp).unwrap();
            std::fs::write(pkg.join(r"dist\index.js"), "#!/usr/bin/env node\n").unwrap();
            let shim = root.join("pi-acp.cmd");
            std::fs::write(&shim, CODEX_CMD.replace(r"@openai\codex\bin\codex.js", r"pi-acp\dist\index.js")).unwrap();
            let adapter = harness("pi-acp").unwrap();
            assert_eq!(crate::runtime().block_on(installed_version(adapter, &shim)).as_deref(), Some("0.0.34"));
            let _ = std::fs::remove_dir_all(&root);
        }
        #[cfg(unix)]
        {
            let root = std::env::temp_dir().join(format!("trek-adapter-{}", std::process::id()));
            let pkg = root.join("lib/node_modules/pi-acp");
            std::fs::create_dir_all(pkg.join("dist")).unwrap();
            std::fs::write(pkg.join("package.json"), pi_acp).unwrap();
            std::fs::write(pkg.join("dist/index.js"), "#!/usr/bin/env node\n").unwrap();
            std::fs::create_dir_all(root.join("bin")).unwrap();
            let link = root.join("bin/pi-acp");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(pkg.join("dist/index.js"), &link).unwrap();
            let adapter = harness("pi-acp").unwrap();
            assert_eq!(crate::runtime().block_on(installed_version(adapter, &link)).as_deref(), Some("0.0.34"));
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// npm's `.cmd` shim for `@openai/codex`, as `npm i -g` writes it (copied from a real one).
    #[cfg(windows)]
    const CODEX_CMD: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";

    #[test]
    #[cfg(windows)]
    fn npm_on_windows_has_no_lib_folder() {
        let p = |s: &str| install_of(Path::new(s));
        let npm = |package: &str, prefix: &str| Install::Npm { package: package.into(), prefix: prefix.into() };
        // The default prefix, `%APPDATA%\npm`.
        let roaming = r"C:\Users\me\AppData\Roaming\npm";
        assert_eq!(p(r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js"), npm("@openai/codex", roaming));
        assert_eq!(p(r"C:\Users\me\AppData\Roaming\npm\node_modules\opencode-ai\bin\opencode.exe"), npm("opencode-ai", roaming));
        assert_eq!(p(r"C:\Users\me\AppData\Roaming\npm\node_modules\@opencode\cli\bin\opencode.exe"), npm("@opencode/cli", roaming));
        // `canonicalize` gives `\\?\C:\…`, which npm can't take as a prefix: it's the plain path.
        assert_eq!(p(r"\\?\C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js"), npm("@openai/codex", roaming));
        assert_eq!(p(r"\\?\UNC\server\share\npm\node_modules\opencode-ai\bin\opencode"), npm("opencode-ai", r"\\server\share\npm"));
        // nvm for Windows (and Node's own folder) keep packages beside node.exe.
        assert_eq!(p(r"C:\Users\me\AppData\Local\nvm\v22.3.0\node_modules\@openai\codex\bin\codex.js"), npm("@openai/codex", r"C:\Users\me\AppData\Local\nvm\v22.3.0"));
        // A Unix-shaped prefix (npm in Git Bash or MSYS) still has its `lib`.
        assert_eq!(p(r"C:\npm\lib\node_modules\opencode-ai\bin\opencode"), npm("opencode-ai", r"C:\npm"));
        // Other package managers.
        assert_eq!(p(r"C:\Users\me\AppData\Local\pnpm\global\5\node_modules\@qwen-code\qwen-code\cli.js"), Install::Pnpm { package: "@qwen-code/qwen-code".into() });
        assert_eq!(p(r"C:\Users\me\.bun\install\global\node_modules\@google\gemini-cli\dist\index.js"), Install::Bun { package: "@google/gemini-cli".into() });
        assert_eq!(p(r"C:\Users\me\AppData\Local\Volta\bin\codex.exe"), Install::Volta);
        assert_eq!(p(r"C:\Users\me\AppData\Local\Volta\tools\image\packages\@openai\codex\node_modules\@openai\codex\bin\codex.js"), Install::Volta);
        assert_eq!(
            p(r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\Anthropic.ClaudeCode_Microsoft.Winget.Source_8wekyb3d8bbwe\claude.exe"),
            Install::Winget { id: "Anthropic.ClaudeCode".into() }
        );
        // The vendors' own installers, where they put the CLIs.
        for native in [
            r"C:\Users\me\.local\bin\claude.exe",
            r"C:\Users\me\AppData\Local\devin\cli\bin\devin.exe",
            r"C:\Users\me\AppData\Local\devin\cli\_versions\3000.11.3\devin.exe",
            r"C:\Users\me\.grok\bin\grok.exe",
            r"C:\Users\me\AppData\Local\cursor-agent\cursor-agent.exe",
            r"C:\Users\me\bin\droid.exe",
            r"C:\Users\me\scoop\shims\opencode.exe",
            r"C:\Users\me\.opencode\bin\opencode.exe",
        ] {
            assert_eq!(p(native), Install::Native, "{native}");
        }
    }

    #[test]
    fn homebrew_is_not_looked_for_on_windows() {
        // The code stays; a Windows path just can't be Homebrew's, whatever its folders are called.
        let keg = install_of(Path::new("/opt/homebrew/Cellar/opencode/1.18.34/bin/opencode"));
        let cask = install_of(Path::new("/opt/homebrew/Caskroom/copilot-cli/1.0.69/copilot"));
        if cfg!(windows) {
            assert_eq!((keg, cask), (Install::Native, Install::Native));
        } else {
            assert!(matches!(keg, Install::Brew { .. }) && matches!(cask, Install::Cask { .. }));
        }
    }

    #[test]
    #[cfg(windows)]
    fn a_cmd_shim_is_read_for_the_package_it_runs() {
        let at = Path::new(r"C:\nvm\v22.3.0\codex.cmd");
        let target = cmd_shim_target(at, CODEX_CMD).unwrap();
        assert_eq!(target, Path::new(r"C:\nvm\v22.3.0\node_modules\@openai\codex\bin\codex.js"));
        assert_eq!(install_of(&target), Install::Npm { package: "@openai/codex".into(), prefix: r"C:\nvm\v22.3.0".into() });
        // Older cmd-shim's `%~dp0\`, and a script reached through `..` (a prefix with `lib`).
        let old = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\lib\\node_modules\\pi-acp\\dist\\index.js\" %*\r\n)\r\n";
        assert_eq!(cmd_shim_target(Path::new(r"C:\x\bin\pi-acp.cmd"), old).unwrap(), Path::new(r"C:\x\lib\node_modules\pi-acp\dist\index.js"));
        // pnpm's global bin: its packages live under `global\5`.
        let pnpm = "@ECHO off\r\n\"%_prog%\"  \"%dp0%\\global\\5\\node_modules\\@qwen-code\\qwen-code\\cli.js\" %*\r\n";
        let target = cmd_shim_target(Path::new(r"C:\Users\me\AppData\Local\pnpm\qwen.cmd"), pnpm).unwrap();
        assert_eq!(install_of(&target), Install::Pnpm { package: "@qwen-code/qwen-code".into() });
        // A batch file that runs no package isn't a shim.
        assert_eq!(cmd_shim_target(at, "@echo off\r\necho \"%dp0%\\hello\"\r\n"), None);
        assert!(cmd_shim_target(at, "@echo off\r\ncall \"%dp0%\\node_modules\\.bin\\x\"\r\n").is_some(), "any node_modules path counts");
        // From disk: the shim in an npm prefix says what's installed there.
        let root = std::env::temp_dir().join(format!("trek-cmd-shim-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("codex.cmd"), CODEX_CMD).unwrap();
        assert_eq!(detect_install(&root.join("codex.cmd")), Install::Npm { package: "@openai/codex".into(), prefix: root.clone() });
        assert_eq!(script_of(&root.join("codex.cmd")), root.join(r"node_modules\@openai\codex\bin\codex.js"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn a_bun_shim_is_read_for_the_package_it_runs() {
        // UNVERIFIED against a real Bun: the `.bunx` file is read as UTF-16 text holding the script's
        // path, which is how Bun's source writes it; a layout that differs reads as the vendor's own.
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let shim = Path::new(r"C:\Users\me\.bun\bin\gemini.exe");
        let relative = utf16("..\\install\\global\\node_modules\\@google\\gemini-cli\\dist\\index.js\0");
        let target = bunx_target(shim, &relative).unwrap();
        assert_eq!(target, Path::new(r"C:\Users\me\.bun\install\global\node_modules\@google\gemini-cli\dist\index.js"));
        assert_eq!(install_of(&target), Install::Bun { package: "@google/gemini-cli".into() });
        let absolute = utf16("\\??\\C:\\Users\\me\\.bun\\install\\global\\node_modules\\qwen\\cli.js\0");
        assert_eq!(install_of(&bunx_target(shim, &absolute).unwrap()), Install::Bun { package: "qwen".into() });
        assert_eq!(bunx_target(shim, &utf16("not a script\0")), None);
        assert_eq!(bunx_target(shim, &[1]), None);
    }

    #[test]
    fn winget_installs_update_with_winget() {
        let claude = h("claude-code");
        let winget = Install::Winget { id: "Anthropic.ClaudeCode".into() };
        assert!(claude.owns(&winget));
        assert!(!h("codex").owns(&winget), "another agent's package id");
        assert!(!claude.owns(&Install::Winget { id: "Other.Tool".into() }));
        assert_eq!(winget.label(), "WinGet");
        // Copilot CLI's WinGet package (the one installed on a real Windows machine).
        let copilot = Install::Winget { id: "GitHub.Copilot".into() };
        assert!(h("acp:github-copilot").owns(&copilot) && !claude.owns(&copilot));
        assert_eq!(h("acp:github-copilot").reading(copilot.clone()), copilot);
        // A package of another id isn't `winget upgrade`d on a guess: it's the vendor's own install.
        assert_eq!(h("codex").reading(Install::Winget { id: "OpenAI.Codex".into() }), Install::Native);
        assert_eq!(claude.reading(Install::Native), Install::Native);
        // Its versions are the vendor's own feed's.
        assert_eq!(source(claude, &winget, None), Some(Source::Feed(Feed::ClaudeReleases)));
        let cmd = update_command(claude, &winget, Path::new(r"C:\x\claude.exe"), false, Some("2.1.1")).unwrap();
        assert_eq!(cmd.program, Path::new("winget"));
        assert_eq!(cmd.shown(), "winget upgrade --id Anthropic.ClaudeCode --exact --accept-package-agreements --accept-source-agreements");
    }

    #[test]
    fn a_program_is_found_the_way_the_platform_runs_it() {
        let root = std::env::temp_dir().join(format!("trek-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (first, second) = (root.join("first"), root.join("second"));
        for dir in [&first, &second] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let search = std::env::join_paths([&first, &second]).unwrap().to_string_lossy().into_owned();
        // On Windows the extensionless `tool` is a shell script, which Windows can't run: only
        // `tool.cmd` (and `.exe`, …) count, in `PATHEXT` order, and the first folder's one first.
        std::fs::write(first.join("tool"), "#!/bin/sh\n").unwrap();
        assert_eq!(resolve_program(Path::new("tool"), &search), if cfg!(windows) { None } else { Some(first.join("tool")) });
        if cfg!(windows) {
            std::fs::write(second.join("tool.exe"), "").unwrap();
            assert_eq!(resolve_program(Path::new("tool"), &search), Some(second.join("tool.exe")));
            std::fs::write(first.join("tool.cmd"), "").unwrap();
            assert_eq!(resolve_program(Path::new("tool"), &search), Some(first.join("tool.cmd")));
            assert_eq!(resolve_program(Path::new("tool.exe"), &search), Some(second.join("tool.exe")), "a name with its extension is taken as it is");
        }
        assert_eq!(resolve_program(Path::new("nothing"), &search), None);
        // An absolute path is the program, found or not (`run_update` says it isn't there).
        assert_eq!(resolve_program(&first.join("tool"), &search), Some(first.join("tool")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_npm_update_runs_the_npm_of_its_prefix() {
        let root = std::env::temp_dir().join(format!("trek-npm-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let log = root.join("npm.log");
        fake_npm(&root, &log, &[]);
        let install = Install::Npm { package: "@openai/codex".into(), prefix: root.clone() };
        // An id no harness has: nothing real is asked its version afterwards.
        let v = AgentVersion { command: update_command(h("codex"), &install, &root.join("codex"), false, None), install, ..version("fake", "0.159.2", Some("0.160.0")) };
        let outcome = crate::runtime().block_on(run_update(&v));
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().collect::<Vec<_>>(), ["install -g @openai/codex@latest"]);
        assert!(matches!(outcome, Outcome::Failed { ref summary, .. } if summary.contains("didn't say its version")), "{outcome:?}");
        // A program that isn't there is said so.
        let v = AgentVersion { command: Some(UpdateCommand::new("trek-no-such-package-manager", &["update"])), ..version("fake", "1.0.0", None) };
        let Outcome::Failed { summary, .. } = crate::runtime().block_on(run_update(&v)) else { panic!() };
        assert_eq!(summary, "trek-no-such-package-manager isn't installed.");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `std::process::Command` runs a `.cmd` through `cmd.exe`, which splits its command line by
    /// rules of its own. Rust quotes each argument for them (a space, `&`, `^` or `%` arrives as
    /// one literal argument) and refuses what it can't quote safely (a line break), as an
    /// `InvalidInput` error: "batch file arguments are invalid". Package names and flags need none
    /// of this, but update commands are built from data (a package id, a path), so what happens to
    /// the rest is pinned here.
    #[test]
    #[cfg(windows)]
    fn a_cmd_files_arguments_arrive_whole_or_are_refused() {
        let root = std::env::temp_dir().join(format!("trek-cmd-args-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("args.log");
        // `%1`, quotes and all (`%~1` would put `a&b` into the line `echo` is parsed from).
        // Redirection first: a digit before `>>` is a handle.
        std::fs::write(root.join("fake.cmd"), format!("@echo off\r\n>> \"{}\" echo [%1][%2]\r\n", log.display())).unwrap();
        let run = |args: &[&str]| std::process::Command::new(root.join("fake.cmd")).args(args).output();
        for (args, logged) in [
            (&["install", "-g"][..], "[install][-g]"),
            (&["@openai/codex@latest"], "[@openai/codex@latest][]"),
            (&["a b", "c"], r#"["a b"][c]"#),
            (&["100%", "x"], r#"["100%"][x]"#),
            (&["%PATH%"], r#"["%PATH%"][]"#),
            (&["a&b"], r#"["a&b"][]"#),
            (&["^x"], r#"["^x"][]"#),
            (&[r"--prefix=C:\Program Files\x"], r#"["--prefix=C:\Program Files\x"][]"#),
        ] {
            let _ = std::fs::remove_file(&log);
            assert!(run(args).unwrap().status.success(), "{args:?}");
            assert_eq!(std::fs::read_to_string(&log).unwrap().trim_end(), logged, "{args:?}");
        }
        // Refused, not mangled: nothing runs.
        let _ = std::fs::remove_file(&log);
        for args in [&["a\nb"][..], &["a\r"]] {
            let err = run(args).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{args:?}");
            assert!(err.to_string().contains("batch file arguments are invalid"), "{err}");
        }
        assert!(!log.exists());
        // So an update command with such an argument fails with that said, and runs nothing.
        let bad = UpdateCommand::new(root.join("fake.cmd"), &["bad\nargument"]);
        let v = AgentVersion { command: Some(bad), ..version("fake", "1.0.0", Some("1.1.0")) };
        let Outcome::Failed { summary, .. } = crate::runtime().block_on(run_update(&v)) else { panic!() };
        assert!(summary.starts_with("Couldn't run") && summary.contains("batch file arguments are invalid"), "{summary}");
        assert!(!log.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn commands_on_windows_read_as_they_are_typed() {
        // A binary found as `claude.exe` or `npm.cmd` is typed `claude`, `npm`.
        assert_eq!(UpdateCommand::new(r"C:\Users\me\.local\bin\claude.exe", &["update"]).shown(), "claude update");
        assert_eq!(UpdateCommand::new(r"C:\nvm\v22\codex.cmd", &["update"]).shown(), "codex update");
        assert_eq!(UpdateCommand::new("npm", &["install", "-g", "x@latest"]).shown(), "npm install -g x@latest");
        // PATH is `;`-separated, the login PATH last.
        let cmd = UpdateCommand { path: vec![r"C:\nvm\v22".into(), r"C:\other".into()], ..UpdateCommand::new("npm", &[]) };
        assert!(cmd.search_path().starts_with(r"C:\nvm\v22;C:\other;"), "{}", cmd.search_path());
        // The vendor's PowerShell installers, run by Windows PowerShell with what's shown as typed.
        let droid = h("droid");
        let line = droid.installer_line().unwrap();
        assert!(line.starts_with("irm https://") && line.ends_with(" | iex"), "{line}");
        let cmd = update_command(droid, &Install::Native, Path::new(r"C:\Users\me\bin\droid.exe"), false, None).unwrap();
        assert_eq!((cmd.shown().as_str(), cmd.args.last().map(String::as_str)), (line, Some(line)));
        assert!(cmd.args.windows(2).any(|w| w == ["-ExecutionPolicy", "Bypass"]) && cmd.args.contains(&"-NoProfile".to_string()), "{:?}", cmd.args);
        // The CLIs with their own update command use it, not the installer.
        for (agent, bin) in [("claude-code", r"C:\Users\me\.local\bin\claude.exe"), ("acp:grok", r"C:\Users\me\.grok\bin\grok.exe"), ("acp:devin", r"C:\Users\me\AppData\Local\devin\cli\bin\devin.exe"), ("acp:cursor", r"C:\Users\me\AppData\Local\cursor-agent\cursor-agent.exe")] {
            let cmd = update_command(h(agent), &Install::Native, Path::new(bin), false, None).unwrap();
            assert_eq!(cmd.program, Path::new(bin));
            assert_eq!(cmd.args, ["update"], "{agent}");
        }
    }

    #[test]
    fn the_windows_install_scripts_of_cursor_and_factory_name_their_version_too() {
        assert_eq!(urls(&Source::Feed(Feed::CursorInstaller)), ["https://cursor.com/install", "https://cursor.com/install?win32=true"]);
        assert_eq!(urls(&Source::Feed(Feed::DroidInstaller)), ["https://app.factory.ai/cli", "https://app.factory.ai/cli/windows"]);
        // Cursor's PowerShell script (`$downloadUrl = 'https://downloads.cursor.com/lab/<version>/'`).
        let cursor = "$downloadUrl = 'https://downloads.cursor.com/lab/2026.10.01-e373342/'\r\n$agentPath = \"$env:LOCALAPPDATA\\cursor-agent\"\r\n";
        assert_eq!(parse_latest(&Source::Feed(Feed::CursorInstaller), cursor).unwrap().version, "2026.10.01-e373342");
        // Factory's: `$version = '0.237.0'`, where the shell one has `VER="0.233.0"`.
        let droid = "$ErrorActionPreference = 'Stop'\r\n$version = '0.237.0'\r\n$baseUrl = \"https://downloads.factory.ai\"\r\n";
        assert_eq!(parse_latest(&Source::Feed(Feed::DroidInstaller), droid).unwrap().version, "0.237.0");
        assert_eq!(parse_latest(&Source::Feed(Feed::DroidInstaller), "VER=\"0.233.0\"\n").unwrap().version, "0.233.0");
        // The feeds say a version, not a download per platform: Devin's manifest lists its
        // Windows builds (`x86_64-pc-windows`, `aarch64-pc-windows`) beside the Mac's, and the
        // version is the same for all.
        let devin = r#"{"version":"3000.11.3","platforms":{"aarch64-pc-windows":{"url":"…"},"x86_64-pc-windows":{"url":"…"},"aarch64-apple-darwin":{"url":"…"}}}"#;
        assert_eq!(parse_latest(&Source::Feed(Feed::DevinManifest), devin).unwrap().version, "3000.11.3");
    }

    fn version(agent: &str, installed: &str, latest: Option<&str>) -> AgentVersion {
        AgentVersion {
            id: agent.into(),
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
    fn an_offline_check_keeps_the_command_the_last_one_worked_out() {
        // Copilot's cask updates itself: `copilot update`, as the cask's feed said. Offline, the
        // feed can't say so, and brew's path would come up in its place.
        let cask = Install::Cask { cask: "copilot-cli".into(), prefix: "/opt/homebrew".into() };
        let bin = Path::new("/opt/homebrew/bin/copilot");
        let copilot = h("acp:github-copilot");
        let entry = |latest: Option<&str>, auto: bool| AgentVersion {
            install: cask.clone(),
            command: update_command(copilot, &cask, bin, auto, Some("1.0.91")),
            ..version("copilot", "1.0.91", latest)
        };
        let before = [entry(Some("1.0.92"), true)];
        let mut fresh = [entry(None, false)];
        assert_eq!(fresh[0].command.as_ref().unwrap().shown(), "brew upgrade --cask copilot-cli");
        carry_over(&mut fresh, &before);
        assert_eq!(fresh[0].latest.as_deref(), Some("1.0.92"));
        assert_eq!(fresh[0].command.as_ref().unwrap().shown(), "copilot update");
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
            // An installer to re-run is a line this platform's shell can run.
            assert!(h.installer.is_none() || h.installer_line().is_some_and(|l| if cfg!(windows) { l.starts_with("irm https://") && l.ends_with(" | iex") } else { l.starts_with("curl ") }), "{} installer", h.agent);
            assert_eq!(harness(h.binary).map(|o| o.agent), Some(h.agent), "{} is one row", h.binary);
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
