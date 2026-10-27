//! The Browser's ad blocker: the user's own uBlock Origin, imported from its folder (or a .zip /
//! .crx of it). Nothing of uBlock Origin ships with Trek (it's GPL, its lists GPL / CC-BY-SA):
//! what's imported stays in the user's data folder.
//!
//! - macOS: WKWebView runs no extensions, so the extension's declarativeNetRequest rulesets (and
//!   the site-specific element hiding in its bundled filter lists) are converted to WebKit content
//!   rules (`convert`), written to `adblock/rules.json`, and compiled by WebKit into a store of
//!   Trek's own (`adblock/compiled`), once: a list is found again by its identifier, a hash of
//!   what it holds, and only compiled when that changes (a re-import, a site allowed). The lists
//!   go on every Browser view's content controller and apply from its next load.
//! - Windows: WebView2 runs unpacked Chromium extensions, Manifest V2 ones included (while Edge
//!   still does: Microsoft's schedule turns MV2 off from early 2027), so the folder is copied
//!   beside the Browser's other extensions and installed as one (`browser_extensions`).
//!
//! `AdBlocker` is shared by every Browser (a global entity): an import, a compile or a change of
//! the setting notifies it, and each Browser puts the current lists on its views.

#[cfg_attr(windows, allow(dead_code))] // Windows installs the extension instead.
pub mod convert;

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where the ad blocker keeps what it imported and compiled.
pub fn dir() -> PathBuf {
    trek_core::paths::data_dir().join("adblock")
}

/// What was imported, and what became of its rules (`adblock/state.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Imported {
    pub name: String,
    pub version: String,
    pub manifest_version: u64,
    /// The folder or archive it came from (for "Re-import").
    pub source: PathBuf,
    /// The rulesets read (their manifest ids).
    pub rulesets: Vec<String>,
    pub total: usize,
    pub active: usize,
    pub redirects: usize,
    pub skipped: usize,
    /// Skipped rules by why, largest first.
    pub skipped_by: Vec<(String, usize)>,
    pub webkit_rules: usize,
    pub cosmetic: usize,
    /// Windows: installed as an extension (no rules converted).
    pub extension: bool,
}

/// `35,864` for 35864.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Imported {
    /// "uBlock Origin 1.75.0".
    pub fn title(&self) -> String {
        format!("{} {}", self.name, self.version).trim().to_string()
    }

    /// "35,864 rules → 34,000 active, 1,864 skipped (1,189 redirects as blocks)".
    pub fn summary(&self) -> String {
        if self.extension {
            return "Installed as a browser extension".into();
        }
        let mut s = format!(
            "{} rules → {} active, {} skipped",
            thousands(self.total),
            thousands(self.active),
            thousands(self.skipped)
        );
        if self.redirects > 0 {
            s.push_str(&format!(
                " ({} redirects as blocks)",
                thousands(self.redirects)
            ));
        }
        if self.cosmetic > 0 {
            s.push_str(&format!(
                "; {} element-hiding rules",
                thousands(self.cosmetic)
            ));
        }
        s
    }
}

/// The site an address is on, as "Allow ads on this site" keeps it: its host, lowercase, without
/// `www.`. Web pages only.
#[cfg_attr(windows, allow(dead_code))] // Windows' uBlock Origin keeps its own allowed sites.
pub fn site_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty() && convert::webkit_domain(&host).is_some()).then_some(host)
}

/// An extension's folder, from the folder itself or an archive unpacked into a scratch folder
/// (removed when this goes).
struct Source {
    root: PathBuf,
    scratch: Option<PathBuf>,
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(scratch) = &self.scratch {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

/// The folder with `manifest.json`: `dir` itself, or its one subfolder that has it (a zip of a
/// folder).
fn manifest_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("manifest.json").is_file() {
        return Some(dir.to_path_buf());
    }
    let subs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("manifest.json").is_file())
        .collect();
    (subs.len() == 1).then(|| subs[0].clone())
}

fn open_source(path: &Path, scratch: &Path) -> Result<Source, String> {
    if path.is_dir() {
        let root = manifest_root(path)
            .ok_or("That folder has no manifest.json: pick the extension's own folder.")?;
        return Ok(Source {
            root,
            scratch: None,
        });
    }
    const MAX_BYTES: u64 = 256 << 20;
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_BYTES {
        return Err("That file is too large to be an extension.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let zip = super::browser_extensions::crx_zip(&bytes)?;
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let source = Source {
        root: scratch.join("unpacked"),
        scratch: Some(scratch.to_path_buf()),
    };
    let archive = scratch.join("package.zip");
    std::fs::write(&archive, zip).map_err(|e| e.to_string())?;
    trek_core::registry::unpack_zip(&archive, &source.root).map_err(|e| e.to_string())?;
    let mut source = source;
    source.root = manifest_root(&source.root).ok_or("The archive has no manifest.json.")?;
    Ok(source)
}

/// The extension's name and version from its manifest (a `__MSG_name__` name looked up in its
/// default locale's messages).
fn describe(root: &Path, manifest: &Value) -> (String, String, u64) {
    let mut name = manifest["name"].as_str().unwrap_or("Extension").to_string();
    if let Some(key) = name
        .strip_prefix("__MSG_")
        .and_then(|k| k.strip_suffix("__"))
    {
        let locale = manifest["default_locale"].as_str().unwrap_or("en");
        let messages = root.join("_locales").join(locale).join("messages.json");
        let found = std::fs::read_to_string(messages)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|m| {
                m.as_object()?
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .and_then(|(_, v)| v["message"].as_str().map(str::to_string))
            });
        name = found.unwrap_or_else(|| key.to_string());
    }
    let version = manifest["version"].as_str().unwrap_or("").to_string();
    let mv = manifest["manifest_version"].as_u64().unwrap_or(0);
    (name, version, mv)
}

/// `rel` under `root`, refusing a path that leaves it.
fn inside(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel.trim_start_matches(['/', '\\']));
    let plain = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    plain.then(|| root.join(rel))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))
}

/// The extension's enabled declarativeNetRequest rulesets: (id, rules).
#[cfg_attr(windows, allow(dead_code))]
fn rulesets(root: &Path, manifest: &Value) -> Result<Vec<(String, Vec<Value>)>, String> {
    let mut out = vec![];
    let resources = manifest["declarative_net_request"]["rule_resources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for res in resources {
        if res["enabled"].as_bool() == Some(false) {
            continue;
        }
        let id = res["id"].as_str().unwrap_or("rules").to_string();
        let Some(path) = res["path"].as_str().and_then(|p| inside(root, p)) else {
            continue;
        };
        let rules = match read_json(&path)? {
            Value::Array(rules) => rules,
            _ => return Err(format!("{} isn't a list of rules.", path.display())),
        };
        out.push((id, rules));
    }
    Ok(out)
}

/// The text of the filter lists the extension ships and turns on by default (uBlock Origin's
/// `assets/assets.json`, its local copies), for element hiding.
#[cfg_attr(windows, allow(dead_code))]
fn filter_lists(root: &Path) -> Vec<String> {
    const MAX_BYTES: u64 = 32 << 20;
    let Ok(assets) = read_json(&root.join("assets/assets.json")) else {
        return vec![];
    };
    let Some(assets) = assets.as_object() else {
        return vec![];
    };
    let mut out = vec![];
    for entry in assets.values() {
        if entry["content"] != "filters" || entry["off"] == true {
            continue;
        }
        let urls: Vec<&str> = match &entry["contentURL"] {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let local = urls
            .into_iter()
            .filter(|u| !u.contains("://"))
            .filter_map(|u| inside(root, u))
            .find(|p| p.is_file());
        if let Some(text) = local
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_BYTES))
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            out.push(text);
        }
    }
    out
}

/// Write `bytes` to `path` whole: a sibling written first, then moved over it.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Read the extension at `path`, convert its rules, and keep them in `into` (`rules.json`, with
/// `state.json` saying what they are). Slow-ish (tens of MB of JSON): off the main thread.
#[cfg_attr(windows, allow(dead_code))] // Windows installs the extension instead.
pub fn import_rules(path: &Path, into: &Path) -> Result<Imported, String> {
    let source = open_source(path, &into.join("unpacking"))?;
    let root = &source.root;
    let manifest = read_json(&root.join("manifest.json"))?;
    let (name, version, manifest_version) = describe(root, &manifest);
    let sets = rulesets(root, &manifest)?;
    if sets.is_empty() {
        return Err(format!(
            "{name} lists no declarativeNetRequest rules, which is what Trek's blocker reads. \
             Pick a uBlock Origin build with a rulesets folder (or uBlock Origin Lite)."
        ));
    }
    let mut rules = vec![];
    let mut unreadable = 0;
    for (_, set) in &sets {
        for rule in set {
            match serde_json::from_value::<convert::DnrRule>(rule.clone()) {
                Ok(r) => rules.push(r),
                Err(_) => unreadable += 1,
            }
        }
    }
    let lists = filter_lists(root);
    let cosmetic = convert::cosmetic_rules(lists.iter().map(String::as_str));
    let converted = convert::convert(&rules, cosmetic);
    let stats = &converted.stats;
    let mut skipped_by: Vec<(String, usize)> = stats
        .skipped
        .iter()
        .map(|(k, n)| (k.label().to_string(), *n))
        .collect();
    if unreadable > 0 {
        skipped_by.push(("unreadable".into(), unreadable));
    }
    skipped_by.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let imported = Imported {
        name,
        version,
        manifest_version,
        source: path.to_path_buf(),
        rulesets: sets.iter().map(|(id, _)| id.clone()).collect(),
        total: stats.total + unreadable,
        active: stats.active,
        redirects: stats.redirects,
        skipped: stats.skipped_total() + unreadable,
        skipped_by,
        webkit_rules: stats.webkit_rules,
        cosmetic: stats.cosmetic,
        extension: false,
    };
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let all: Vec<&Value> = converted
        .network
        .iter()
        .chain(&converted.cosmetic)
        .collect();
    write_atomic(
        &into.join("rules.json"),
        &serde_json::to_vec(&all).map_err(|e| e.to_string())?,
    )?;
    save_state(into, &imported)?;
    Ok(imported)
}

/// Copy the extension at `path` into `dest` (replacing what was there), for WebView2 to run from
/// a folder that stays put (it drops an extension whose files change). Windows' import.
#[cfg_attr(not(windows), allow(dead_code))] // Tested everywhere; only Windows installs.
pub fn import_extension(path: &Path, dest: &Path, into: &Path) -> Result<Imported, String> {
    let source = open_source(path, &into.join("unpacking"))?;
    let manifest = read_json(&source.root.join("manifest.json"))?;
    let (name, version, manifest_version) = describe(&source.root, &manifest);
    let staging = dest.with_extension("partial");
    let _ = std::fs::remove_dir_all(&staging);
    copy_dir(&source.root, &staging).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("Couldn't copy the extension: {e}")
    })?;
    // The store's signatures: Chromium won't load an unpacked folder that has them.
    let _ = std::fs::remove_dir_all(staging.join("_metadata"));
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&staging, dest).map_err(|e| e.to_string())?;
    let imported = Imported {
        name,
        version,
        manifest_version,
        source: path.to_path_buf(),
        extension: true,
        ..Default::default()
    };
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    save_state(into, &imported)?;
    Ok(imported)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
        // Links are left behind: an extension's files are its own.
    }
    Ok(())
}

fn save_state(into: &Path, imported: &Imported) -> Result<(), String> {
    write_atomic(
        &into.join("state.json"),
        &serde_json::to_vec_pretty(imported).map_err(|e| e.to_string())?,
    )
}

pub fn load_state(from: &Path) -> Option<Imported> {
    let text = std::fs::read_to_string(from.join("state.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The lists to compile from what's kept in `from`, with `allowed` sites let through: each
/// list's identifier (a hash of its JSON: the same rules find the same compiled list) and JSON.
#[cfg_attr(windows, allow(dead_code))] // Windows installs the extension instead.
pub fn prepared_lists(from: &Path, allowed: &[String]) -> Result<Vec<(String, String)>, String> {
    let rules = match read_json(&from.join("rules.json"))? {
        Value::Array(rules) => rules,
        _ => return Err("The ad blocker's rules are damaged: import again.".into()),
    };
    Ok(convert::Converted::lists(&rules, allowed)
        .into_iter()
        .map(|json| {
            (
                format!("{LIST_PREFIX}{:016x}", fnv1a(json.as_bytes())),
                json,
            )
        })
        .collect())
}

/// The compiled lists' identifiers start so (any other in the store is an old one).
#[cfg_attr(windows, allow(dead_code))] // Windows installs the extension instead.
pub const LIST_PREFIX: &str = "trek-adblock-";

/// FNV-1a: a hash that stays the same from one build to the next (std's may not).
#[cfg_attr(windows, allow(dead_code))] // Windows installs the extension instead.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Windows: the imported extension's folder among the Browser's extensions.
#[cfg(windows)]
const EXTENSION_FOLDER: &str = "ublock-origin-imported";

/// The ad blocker every Browser shares: what's imported, and (macOS) the compiled lists.
pub struct AdBlocker {
    pub imported: Option<Imported>,
    /// What's under way ("Importing…"), while something is.
    pub busy: Option<&'static str>,
    pub error: Option<String>,
    /// The compiled lists (macOS), for the views.
    #[cfg(target_os = "macos")]
    lists: Vec<webkit::List>,
    /// The sites allowed in the lists compiled (`None`: nothing compiled yet).
    #[cfg(target_os = "macos")]
    built_for: Option<Vec<String>>,
    /// The sites the lists should allow (the last asked for).
    #[cfg(target_os = "macos")]
    wanted: Vec<String>,
    /// An extension imported and waiting for a Browser view to install it into (Windows).
    #[cfg(windows)]
    pending: Option<PathBuf>,
    /// Where to say how an import went, once it's ready (or failed).
    announce: Option<WeakEntity<Workspace>>,
    _task: Option<Task<()>>,
}

struct Shared(Entity<AdBlocker>);
impl Global for Shared {}

impl AdBlocker {
    /// The shared ad blocker (made, from what's kept, on first use).
    pub fn get(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<Shared>() {
            return shared.0.clone();
        }
        let entity = cx.new(|_| Self {
            imported: load_state(&dir()),
            busy: None,
            error: None,
            #[cfg(target_os = "macos")]
            lists: vec![],
            #[cfg(target_os = "macos")]
            built_for: None,
            #[cfg(target_os = "macos")]
            wanted: vec![],
            #[cfg(windows)]
            pending: None,
            announce: None,
            _task: None,
        });
        cx.set_global(Shared(entity.clone()));
        entity
    }

    /// The lists for the views (macOS): empty until compiled.
    #[cfg(target_os = "macos")]
    pub fn lists(&self) -> &[webkit::List] {
        &self.lists
    }

    /// The extension folder a Browser should install (Windows), taken once.
    #[cfg(windows)]
    pub fn take_pending(&mut self) -> Option<PathBuf> {
        self.pending.take()
    }

    /// Import the extension at `path`; then (macOS) compile its rules with `allowed` sites let
    /// through, or (Windows) leave it for a Browser to install.
    pub fn import(
        &mut self,
        path: PathBuf,
        allowed: Vec<String>,
        announce: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        self.busy = Some("Importing…");
        self.error = None;
        self.announce = Some(announce);
        #[cfg(not(windows))]
        let work = cx.background_spawn(async move { import_rules(&path, &dir()) });
        #[cfg(windows)]
        let dest = super::browser_extensions::extensions_dir().join(EXTENSION_FOLDER);
        #[cfg(windows)]
        let work = {
            let dest = dest.clone();
            cx.background_spawn(async move { import_extension(&path, &dest, &dir()) })
        };
        let _ = &allowed;
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(imported) => {
                        this.imported = Some(imported);
                        #[cfg(target_os = "macos")]
                        {
                            this.built_for = None;
                            this.compile(allowed, cx);
                        }
                        #[cfg(windows)]
                        {
                            this.pending = Some(dest);
                        }
                    }
                    Err(e) => {
                        this.say(format!("Couldn't import: {e}"), cx);
                        this.error = Some(e);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Tell the workspace an import asked to hear (once).
    fn say(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(ws) = self.announce.take() {
            let _ = ws.update(cx, |_, cx| {
                cx.emit(WorkspaceEvent::Toast {
                    message,
                    undo: None,
                })
            });
        }
    }

    /// Forget the import: its rules, its compiled lists (macOS) or its copy (Windows: the
    /// extension itself is removed from the Extensions page).
    pub fn remove(&mut self, cx: &mut Context<Self>) {
        self.imported = None;
        self.error = None;
        self.busy = None;
        self._task = None;
        #[cfg(target_os = "macos")]
        {
            self.lists.clear();
            self.built_for = None;
            webkit::prune(vec![]);
        }
        // WebView2 drops an extension whose folder changes: removing it removes the extension.
        #[cfg(windows)]
        let _ = std::fs::remove_dir_all(
            super::browser_extensions::extensions_dir().join(EXTENSION_FOLDER),
        );
        let _ = std::fs::remove_file(dir().join("rules.json"));
        let _ = std::fs::remove_file(dir().join("state.json"));
        cx.notify();
    }

    /// Have the lists let `allowed` sites through (compiling them when that changed; a set
    /// compiled before is found again at once).
    #[cfg(target_os = "macos")]
    pub fn compile(&mut self, allowed: Vec<String>, cx: &mut Context<Self>) {
        self.wanted = allowed.clone();
        if self.imported.as_ref().is_none_or(|i| i.extension)
            || self.built_for.as_ref() == Some(&allowed)
            || self.busy.is_some()
        {
            return;
        }
        self.busy = Some("Compiling…");
        self.error = None;
        let prepared = {
            let allowed = allowed.clone();
            cx.background_spawn(async move { prepared_lists(&dir(), &allowed) })
        };
        self._task = Some(cx.spawn(async move |this, cx| {
            let loaded = match prepared.await {
                Ok(lists) => webkit::load(lists)
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err("WebKit didn't answer.".to_string())),
                Err(e) => Err(e),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match loaded {
                    Ok(loaded) => {
                        tracing::info!(
                            "adblock: {} list(s) ready in {:?} ({} compiled)",
                            loaded.lists.len(),
                            loaded.took,
                            loaded.compiled
                        );
                        this.lists = loaded.lists;
                        this.built_for = Some(allowed);
                        if let Some(imported) = &this.imported {
                            let line = format!("{}: {}", imported.title(), imported.summary());
                            this.say(line, cx);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("adblock: {e}");
                        this.say(format!("The ad blocker's rules didn't compile: {e}"), cx);
                        this.error = Some(e);
                        // Not again for the same sites (it would fail the same way).
                        this.built_for = Some(allowed);
                    }
                }
                // Asked for other sites meanwhile: those next.
                let wanted = this.wanted.clone();
                this.compile(wanted, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// Turn the ad blocker on or off for every Browser (the setting, then each Browser's views).
pub fn set_enabled(ws: &Entity<Workspace>, on: bool, cx: &mut App) {
    let allowed = ws.update(cx, |ws, cx| {
        ws.settings.browser.ad_blocker = on;
        ws.save_settings(cx);
        ws.settings.browser.ad_allow.clone()
    });
    AdBlocker::get(cx).update(cx, |a, cx| {
        #[cfg(target_os = "macos")]
        if on {
            a.compile(allowed, cx);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (a, allowed);
        cx.notify();
    });
}

/// Ask for the extension's folder (or, on macOS, a .zip / .crx of it) and import it, turning the
/// ad blocker on.
pub fn pick_and_import(ws: Entity<Workspace>, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        // WebView2's picker takes folders or files, not both: there, the unpacked folder.
        files: cfg!(target_os = "macos"),
        directories: true,
        multiple: false,
        prompt: Some("Import".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| import(&ws, path, cx));
    })
    .detach();
}

/// Import the extension at `path` (a first import or "Re-import"), turning the ad blocker on.
pub fn import(ws: &Entity<Workspace>, path: PathBuf, cx: &mut App) {
    let allowed = ws.update(cx, |ws, cx| {
        ws.settings.browser.ad_blocker = true;
        ws.save_settings(cx);
        ws.settings.browser.ad_allow.clone()
    });
    let announce = ws.downgrade();
    AdBlocker::get(cx).update(cx, |a, cx| a.import(path, allowed, announce, cx));
}

/// WebKit's side (macOS): a content-rule-list store in Trek's data folder, lists found in it by
/// identifier or compiled into it, and put on views.
#[cfg(target_os = "macos")]
pub mod webkit {
    use super::LIST_PREFIX;
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_foundation::{NSArray, NSError, NSString, NSURL};
    use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore, WKWebView};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    pub type List = Retained<WKContentRuleList>;

    pub struct Loaded {
        pub lists: Vec<List>,
        /// How many had to be compiled (the rest were in the store).
        pub compiled: usize,
        pub took: Duration,
    }

    fn store(mtm: MainThreadMarker) -> Option<Retained<WKContentRuleListStore>> {
        let dir = super::dir().join("compiled");
        std::fs::create_dir_all(&dir).ok()?;
        let path = NSString::from_str(&dir.to_string_lossy());
        let url = NSURL::fileURLWithPath_isDirectory(&path, true);
        // SAFETY: main thread; a file URL.
        unsafe { WKContentRuleListStore::storeWithURL(Some(&url), mtm) }
    }

    fn message(error: *mut NSError) -> String {
        // SAFETY: WebKit hands a valid error or nil.
        unsafe { error.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| "WebKit couldn't compile the rules.".into())
    }

    struct Waiting {
        got: Vec<Option<List>>,
        left: usize,
        compiled: usize,
        error: Option<String>,
        started: Instant,
        tx: async_channel::Sender<Result<Loaded, String>>,
        keep: Vec<String>,
    }

    fn arrived(waiting: &Rc<RefCell<Waiting>>, ix: usize, list: Result<List, String>) {
        let mut w = waiting.borrow_mut();
        match list {
            Ok(list) => w.got[ix] = Some(list),
            Err(e) => w.error = Some(e),
        }
        w.left -= 1;
        if w.left > 0 {
            return;
        }
        let answer = match w.error.take() {
            Some(e) => Err(e),
            None => Ok(Loaded {
                lists: w.got.drain(..).flatten().collect(),
                compiled: w.compiled,
                took: w.started.elapsed(),
            }),
        };
        let keep = std::mem::take(&mut w.keep);
        let _ = w.tx.try_send(answer);
        drop(w);
        prune(keep);
    }

    /// Find each of `lists` (identifier, JSON) in the store, or compile it there (WebKit works
    /// off the main thread and answers on it); then drop the store's other lists.
    pub fn load(lists: Vec<(String, String)>) -> async_channel::Receiver<Result<Loaded, String>> {
        let (tx, rx) = async_channel::bounded(1);
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx.try_send(Err("Not on the main thread.".into()));
            return rx;
        };
        let Some(store) = store(mtm) else {
            let _ = tx.try_send(Err("WebKit's rule store couldn't be opened.".into()));
            return rx;
        };
        let waiting = Rc::new(RefCell::new(Waiting {
            got: vec![None; lists.len()],
            left: lists.len(),
            compiled: 0,
            error: None,
            started: Instant::now(),
            tx: tx.clone(),
            keep: lists.iter().map(|(id, _)| id.clone()).collect(),
        }));
        if lists.is_empty() {
            let _ = tx.try_send(Ok(Loaded {
                lists: vec![],
                compiled: 0,
                took: Duration::ZERO,
            }));
            prune(vec![]);
            return rx;
        }
        for (ix, (id, json)) in lists.into_iter().enumerate() {
            let (waiting, compile_store) = (waiting.clone(), store.clone());
            let json = RefCell::new(Some(json));
            let identifier = NSString::from_str(&id);
            let looked_up = RcBlock::new(move |list: *mut WKContentRuleList, _: *mut NSError| {
                // SAFETY: WebKit hands a valid list or nil, on the main thread.
                if let Some(list) = unsafe { Retained::retain(list) } {
                    arrived(&waiting, ix, Ok(list));
                    return;
                }
                let Some(json) = json.borrow_mut().take() else {
                    return;
                };
                waiting.borrow_mut().compiled += 1;
                let waiting = waiting.clone();
                let compiled =
                    RcBlock::new(move |list: *mut WKContentRuleList, e: *mut NSError| {
                        // SAFETY: as above.
                        let list = unsafe { Retained::retain(list) }.ok_or_else(|| message(e));
                        arrived(&waiting, ix, list);
                    });
                // SAFETY: main thread; WebKit copies the block and the string.
                unsafe {
                    compile_store
                        .compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                            Some(&NSString::from_str(&id)),
                            Some(&NSString::from_str(&json)),
                            Some(&compiled),
                        );
                }
            });
            // SAFETY: main thread; WebKit copies the block.
            unsafe {
                store.lookUpContentRuleListForIdentifier_completionHandler(
                    Some(&identifier),
                    Some(&looked_up),
                );
            }
        }
        rx
    }

    /// Remove the store's lists other than `keep` (old imports, sites allowed before).
    pub fn prune(keep: Vec<String>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(store) = store(mtm) else {
            return;
        };
        let remover = store.clone();
        let listed = RcBlock::new(move |ids: *mut NSArray<NSString>| {
            // SAFETY: WebKit hands a valid array or nil, on the main thread.
            let Some(ids) = (unsafe { ids.as_ref() }) else {
                return;
            };
            for id in ids.iter() {
                let id = id.to_string();
                if id.starts_with(LIST_PREFIX) && !keep.contains(&id) {
                    let done = RcBlock::new(|_: *mut NSError| {});
                    // SAFETY: main thread; WebKit copies the block.
                    unsafe {
                        remover.removeContentRuleListForIdentifier_completionHandler(
                            Some(&NSString::from_str(&id)),
                            Some(&done),
                        );
                    }
                }
            }
        });
        // SAFETY: main thread; WebKit copies the block.
        unsafe { store.getAvailableContentRuleListIdentifiers(Some(&listed)) };
    }

    /// Take `remove` off `wv`'s pages and put `add` on, from its next load.
    pub fn apply(wv: &wry::WebView, remove: &[List], add: &[List]) {
        use wry::WebViewExtMacOS as _;
        let view: Retained<WKWebView> = Retained::into_super(wv.webview());
        // SAFETY: main thread (the views live there), a live view and lists.
        unsafe {
            let controller = view.configuration().userContentController();
            for list in remove {
                controller.removeContentRuleList(list);
            }
            for list in add {
                controller.addContentRuleList(list);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LIST_PREFIX, import_extension, import_rules, load_state, prepared_lists, site_of, thousands,
    };
    use serde_json::Value;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trek-adblock-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const MANIFEST: &str = r#"{
        "manifest_version": 2, "name": "__MSG_extName__", "version": "1.2.3",
        "default_locale": "en",
        "declarative_net_request": {"rule_resources": [
            {"id": "base", "enabled": true, "path": "rulesets/base.json"},
            {"id": "off", "enabled": false, "path": "rulesets/off.json"},
            {"id": "evil", "enabled": true, "path": "../outside.json"}
        ]}
    }"#;
    const RULES: &str = r#"[
        {"id": 1, "priority": 10, "action": {"type": "block"}, "condition": {"urlFilter": "||ads.com^"}},
        {"id": 2, "priority": 11, "action": {"type": "redirect", "redirect": {"extensionPath": "/noop.js"}},
            "condition": {"urlFilter": "||x.com/a.js", "resourceTypes": ["script"]}},
        {"id": 3, "action": {"type": "upgradeScheme"}, "condition": {}},
        {"id": 4, "condition": {}}
    ]"#;
    const ASSETS: &str = r#"{
        "easylist": {"content": "filters", "contentURL": ["https://x/easylist.txt", "assets/easylist.txt"]},
        "off": {"content": "filters", "off": true, "contentURL": "assets/off.txt"}
    }"#;

    fn write_extension(root: &Path) {
        let files = [
            ("manifest.json", MANIFEST),
            (
                "_locales/en/messages.json",
                r#"{"extName": {"message": "uBlock Origin"}}"#,
            ),
            ("rulesets/base.json", RULES),
            ("rulesets/off.json", "not even json"),
            ("assets/assets.json", ASSETS),
            ("assets/easylist.txt", "news.com##.ad-box\n##.generic\n"),
            ("assets/off.txt", "news.com##.never\n"),
        ];
        for (name, body) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    #[test]
    fn a_folder_imports_its_enabled_rulesets_and_hiding() {
        let base = scratch("folder");
        let ext = base.join("ext");
        write_extension(&ext);
        let into = base.join("data");
        let imported = import_rules(&ext, &into).unwrap();
        assert_eq!(imported.title(), "uBlock Origin 1.2.3");
        assert_eq!(imported.manifest_version, 2);
        assert_eq!(imported.rulesets, vec!["base"]);
        assert_eq!(
            (
                imported.total,
                imported.active,
                imported.redirects,
                imported.skipped
            ),
            (4, 2, 1, 2)
        );
        assert_eq!(imported.cosmetic, 1);
        assert_eq!(
            imported.summary(),
            "4 rules → 2 active, 2 skipped (1 redirects as blocks); 1 element-hiding rules"
        );
        assert_eq!(load_state(&into), Some(imported));
        let lists = prepared_lists(&into, &["news.com".into()]).unwrap();
        assert_eq!(lists.len(), 1);
        let rules: Vec<Value> = serde_json::from_str(&lists[0].1).unwrap();
        // ads.com (two triggers: the rest, sub-frames), the redirect, the hiding, the allow.
        assert_eq!(rules.len(), 5);
        assert_eq!(rules[3]["action"]["type"], "css-display-none");
        assert_eq!(rules[4]["action"]["type"], "ignore-previous-rules");
        // The same rules, the same identifier; other sites, another.
        assert_eq!(
            prepared_lists(&into, &["news.com".into()]).unwrap()[0].0,
            lists[0].0
        );
        assert_ne!(prepared_lists(&into, &[]).unwrap()[0].0, lists[0].0);
        assert!(lists[0].0.starts_with(LIST_PREFIX));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_archive_of_the_folder_imports_too() {
        let base = scratch("zip");
        let ext = base.join("ext");
        write_extension(&ext);
        // A zip with the extension in a folder of its own, as a CRX.
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for name in [
            "manifest.json",
            "_locales/en/messages.json",
            "rulesets/base.json",
        ] {
            zip.start_file(
                format!("uBlock0.chromium/{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&std::fs::read(ext.join(name)).unwrap())
                .unwrap();
        }
        let zip = zip.finish().unwrap().into_inner();
        let mut crx = b"Cr24".to_vec();
        crx.extend(3u32.to_le_bytes());
        crx.extend(4u32.to_le_bytes());
        crx.extend([0u8; 4]);
        crx.extend(&zip);
        let file = base.join("ublock.crx");
        std::fs::write(&file, crx).unwrap();
        let into = base.join("data");
        let imported = import_rules(&file, &into).unwrap();
        assert_eq!((imported.total, imported.active), (4, 2));
        assert_eq!(imported.cosmetic, 0);
        // The scratch folder goes.
        assert!(!into.join("unpacking").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn without_rules_or_a_manifest_it_says_so() {
        let base = scratch("none");
        std::fs::write(
            base.join("manifest.json"),
            r#"{"name": "X", "version": "1"}"#,
        )
        .unwrap();
        let e = import_rules(&base, &base.join("data")).unwrap_err();
        assert!(e.contains("declarativeNetRequest"), "{e}");
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(import_rules(&empty, &base.join("data")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_extension_is_copied_for_webview2() {
        let base = scratch("copy");
        let ext = base.join("ext");
        write_extension(&ext);
        std::fs::create_dir_all(ext.join("_metadata")).unwrap();
        let dest = base.join("extensions/ublock");
        let imported = import_extension(&ext, &dest, &base.join("data")).unwrap();
        assert!(imported.extension);
        assert_eq!(imported.summary(), "Installed as a browser extension");
        assert!(dest.join("manifest.json").is_file());
        assert!(dest.join("rulesets/base.json").is_file());
        assert!(!dest.join("_metadata").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Convert a real extension (`TREK_ADBLOCK_SOURCE`) into `TREK_ADBLOCK_OUT`, printing what
    /// became of its rules: for measuring against WebKit by hand.
    #[test]
    #[ignore]
    fn convert_a_real_extension() {
        let (Ok(source), Ok(out)) = (
            std::env::var("TREK_ADBLOCK_SOURCE"),
            std::env::var("TREK_ADBLOCK_OUT"),
        ) else {
            return;
        };
        let started = std::time::Instant::now();
        let imported = import_rules(Path::new(&source), Path::new(&out)).unwrap();
        let took = started.elapsed();
        let lists = prepared_lists(Path::new(&out), &[]).unwrap();
        for (i, (_, json)) in lists.iter().enumerate() {
            std::fs::write(Path::new(&out).join(format!("list-{i}.json")), json).unwrap();
        }
        println!(
            "{}\n{}\nconverted in {took:?}",
            imported.title(),
            imported.summary()
        );
        println!(
            "webkit rules: {} network + {} hiding",
            imported.webkit_rules, imported.cosmetic
        );
        println!("skipped: {:?}", imported.skipped_by);
    }

    #[test]
    fn sites_are_hosts_without_www() {
        assert_eq!(
            site_of("https://www.News.com/a?b").as_deref(),
            Some("news.com")
        );
        assert_eq!(site_of("http://user@x.org:8080/").as_deref(), Some("x.org"));
        assert_eq!(site_of("trek://start"), None);
        assert_eq!(site_of("https://"), None);
        assert_eq!(thousands(35864), "35,864");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }
}
