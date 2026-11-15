//! Trek plugins: a folder with a `plugin.toml` that changes how Trek looks and works (a theme, a
//! button in the composer, a meter in each thread's header, a page in the sidebar, a board in
//! place of the thread list, commands and the logic behind them). `docs/PLUGINS.md` is the design;
//! the built-in skill `trek-plugin` is what an agent works from.
//!
//! This module is the plain part: the manifest and what it may say, reading the plugins folder
//! (each plugin checked whole, its problems said in a sentence), the user's choices
//! (`state.toml`: what's on, what each was allowed), the CLI (`cli`) and the examples Trek ships
//! (`examples`). trek-app draws plugins and runs their logic.

pub mod cli;
pub mod examples;
pub mod ui;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The data folder's folder of plugins.
pub const DIR_NAME: &str = "plugins";
/// The data folder's folder of what plugins keep (storage, logs): outside `plugins/`, so their
/// writes don't count as edits to a plugin.
pub const DATA_DIR_NAME: &str = "plugin-data";
/// A plugin's manifest.
pub const MANIFEST: &str = "plugin.toml";
/// What the user chose, in the plugins folder.
pub const STATE_FILE: &str = "state.toml";
/// What's wrong in the folder, one line a plugin: there while something is.
pub const PROBLEMS_FILE: &str = "problems.txt";
/// A file `<id>.link` naming a folder elsewhere: a plugin under development, read in place.
pub const LINK_EXT: &str = "link";
/// Asks a running Trek to read the folder again now (`trek plugin reload`): Trek removes it.
pub const RELOAD_FILE: &str = "reload";
/// The built-in skill that tells an agent how to make one.
pub const SKILL: &str = "trek-plugin";
/// What every agent is told, so "make Trek…" finds the skill.
pub const AGENT_NOTE: &str = "To restyle or extend Trek itself (themes, mods, plugins: new buttons, pages, panels, a look of your own), follow Trek's built-in skill trek-plugin.";
/// A plugin's own storage, at most.
pub const STORAGE_LIMIT: usize = 1 << 20;
/// A plugin's log is cut back to half of this when it grows past it.
pub const LOG_LIMIT: u64 = 256 * 1024;
/// The most files one plugin may have: the folder is watched.
pub const MAX_FILES: usize = 500;

/// The plugins folder.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join(DIR_NAME)
}

/// Where plugin `id` keeps its storage and log.
pub fn data_dir(id: &str) -> PathBuf {
    crate::paths::data_dir().join(DATA_DIR_NAME).join(id)
}

/// Trek's version, which a plugin's `trek` requirement is checked against.
pub fn trek_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(semver::Version::new(0, 0, 0))
}

// ---------- permissions ----------

/// What a plugin may read or do beyond drawing and its own storage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Permission {
    ThreadsRead,
    UsageRead,
    GitRead,
    NotesRead,
    ThreadsOpen,
    ThreadsSend,
    /// A page may reach `https://<host>`.
    Network(String),
}

impl Permission {
    pub fn parse(text: &str) -> Result<Permission, String> {
        Ok(match text.trim() {
            "threads:read" => Permission::ThreadsRead,
            "usage:read" => Permission::UsageRead,
            "git:read" => Permission::GitRead,
            "notes:read" => Permission::NotesRead,
            "threads:open" => Permission::ThreadsOpen,
            "threads:send" => Permission::ThreadsSend,
            other => match other.strip_prefix("network:") {
                Some(host) if host_ok(host) => Permission::Network(host.to_ascii_lowercase()),
                Some(host) => {
                    return Err(format!(
                        "network:{host}: name one host, like network:api.example.com (no scheme, path or *)"
                    ));
                }
                None => {
                    return Err(format!(
                        "\"{other}\" isn't a permission; use threads:read, usage:read, git:read, notes:read, threads:open, threads:send or network:<host>"
                    ));
                }
            },
        })
    }

    pub fn key(&self) -> String {
        match self {
            Permission::ThreadsRead => "threads:read".into(),
            Permission::UsageRead => "usage:read".into(),
            Permission::GitRead => "git:read".into(),
            Permission::NotesRead => "notes:read".into(),
            Permission::ThreadsOpen => "threads:open".into(),
            Permission::ThreadsSend => "threads:send".into(),
            Permission::Network(h) => format!("network:{h}"),
        }
    }

    /// What it allows, as the user is asked.
    pub fn words(&self) -> String {
        match self {
            Permission::ThreadsRead => {
                "See your threads' titles, states, agents and times (not what's in them)".into()
            }
            Permission::UsageRead => "See tokens and cost, per thread and per day".into(),
            Permission::GitRead => "Read the open project's branch and commit history".into(),
            Permission::NotesRead => "Read your notes".into(),
            Permission::ThreadsOpen => "Open threads and start new ones".into(),
            Permission::ThreadsSend => "Send messages to your threads' agents".into(),
            Permission::Network(h) => format!("Reach {h} over the internet"),
        }
    }

    /// Whether it lets the plugin act, not just read (shown first, in Settings).
    pub fn acts(&self) -> bool {
        matches!(
            self,
            Permission::ThreadsOpen | Permission::ThreadsSend | Permission::Network(_)
        )
    }
}

fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with('.')
        && !host.ends_with('.')
}

// ---------- the manifest ----------

/// Where a slot's tree is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    ComposerActions,
    ComposerPlus,
    ComposerBanner,
    ComposerTop,
    ComposerBorder,
    ThreadHeader,
    SidebarTop,
    SidebarFoot,
    NewThreadAbove,
    NewThreadBelow,
    Background,
    Status,
}

impl Slot {
    pub const ALL: [Slot; 12] = [
        Slot::ComposerActions,
        Slot::ComposerPlus,
        Slot::ComposerBanner,
        Slot::ComposerTop,
        Slot::ComposerBorder,
        Slot::ThreadHeader,
        Slot::SidebarTop,
        Slot::SidebarFoot,
        Slot::NewThreadAbove,
        Slot::NewThreadBelow,
        Slot::Background,
        Slot::Status,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Slot::ComposerActions => "composer.actions",
            Slot::ComposerPlus => "composer.plus",
            Slot::ComposerBanner => "composer.banner",
            Slot::ComposerTop => "composer.top",
            Slot::ComposerBorder => "composer.border",
            Slot::ThreadHeader => "thread.header",
            Slot::SidebarTop => "sidebar.top",
            Slot::SidebarFoot => "sidebar.foot",
            Slot::NewThreadAbove => "new-thread.above",
            Slot::NewThreadBelow => "new-thread.below",
            Slot::Background => "background",
            Slot::Status => "status",
        }
    }

    pub fn parse(text: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|s| s.name() == text.trim())
    }
}

/// A surface of Trek's a plugin may replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    ThreadList,
    Composer,
    NewThread,
}

impl Surface {
    pub const ALL: [Surface; 3] = [Surface::ThreadList, Surface::Composer, Surface::NewThread];

    pub fn name(self) -> &'static str {
        match self {
            Surface::ThreadList => "thread-list",
            Surface::Composer => "composer",
            Surface::NewThread => "new-thread",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Surface::ThreadList => "Thread list",
            Surface::Composer => "Composer",
            Surface::NewThread => "New-thread screen",
        }
    }

    pub fn parse(text: &str) -> Option<Surface> {
        Surface::ALL.into_iter().find(|s| s.name() == text.trim())
    }
}

/// The Trek events a plugin's logic may hear.
pub const EVENTS: [&str; 6] = [
    "turn.start",
    "turn.end",
    "thread.needs-you",
    "file.changed",
    "theme.changed",
    "app.start",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    id: Option<String>,
    name: String,
    #[serde(default = "first_version")]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    trek: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    main: Option<String>,
    #[serde(default)]
    hooks: Vec<String>,
    #[serde(default)]
    theme: Vec<ThemeEntry>,
    #[serde(default)]
    slot: Vec<SlotEntry>,
    #[serde(default)]
    page: Vec<PageEntry>,
    #[serde(default)]
    panel: Vec<PageEntry>,
    #[serde(default)]
    replace: Vec<ReplaceEntry>,
    #[serde(default)]
    command: Vec<CommandEntry>,
}

fn first_version() -> String {
    "0.1.0".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeEntry {
    file: String,
    id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotEntry {
    at: String,
    ui: toml::Value,
    #[serde(default)]
    order: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageEntry {
    id: String,
    title: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    entry: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceEntry {
    surface: String,
    ui: toml::Value,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEntry {
    id: String,
    title: String,
    #[serde(default)]
    key: Option<String>,
    /// A function of the logic, by name.
    #[serde(default)]
    run: Option<String>,
    /// Or an action (`page:summit-log`, `set:running=true`).
    #[serde(default)]
    action: Option<String>,
}

/// A theme a plugin brings: its id in the gallery and its file.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginTheme {
    pub id: String,
    pub file: PathBuf,
}

/// A slot's tree.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotUi {
    pub slot: Slot,
    pub tree: ui::Node,
    /// Earlier first, among the slot's trees.
    pub order: i32,
}

/// A page or a panel: HTML in the plugin's folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub id: String,
    pub title: String,
    /// A Lucide icon's name.
    pub icon: String,
    /// The HTML file it opens at, inside the plugin's folder (`pages/<id>/index.html`).
    pub entry: String,
}

/// A replacement for one of Trek's surfaces.
#[derive(Debug, Clone, PartialEq)]
pub struct Replacement {
    pub surface: Surface,
    pub title: String,
    pub tree: ui::Node,
}

/// A command in the palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub id: String,
    pub title: String,
    /// GPUI's form, `cmd-alt-f` (Ctrl in place of Cmd on Windows).
    pub key: Option<String>,
    pub run: CommandRun,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandRun {
    /// The logic's function of this name.
    Logic(String),
    Action(ui::Action),
}

/// A plugin as read and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub trek: Option<semver::VersionReq>,
    pub permissions: Vec<Permission>,
    /// Its logic, a file in its folder.
    pub main: Option<String>,
    pub hooks: Vec<String>,
    pub themes: Vec<PluginTheme>,
    pub slots: Vec<SlotUi>,
    pub pages: Vec<Page>,
    pub panels: Vec<Page>,
    pub replacements: Vec<Replacement>,
    pub commands: Vec<Command>,
}

impl Manifest {
    /// Whether it asks for `p`.
    pub fn wants(&self, p: &Permission) -> bool {
        self.permissions.contains(p)
    }

    /// What it adds, in a few words for Settings ("a theme, 2 slots, a page").
    pub fn summary(&self) -> String {
        let n = |count: usize, one: &str, many: &str| -> Option<String> {
            match count {
                0 => None,
                1 => Some(one.to_string()),
                c => Some(format!("{c} {many}")),
            }
        };
        [
            n(self.themes.len(), "a theme", "themes"),
            n(self.slots.len(), "a slot", "slots"),
            n(self.pages.len(), "a page", "pages"),
            n(self.panels.len(), "a panel", "panels"),
            n(self.replacements.len(), "a replacement", "replacements"),
            n(self.commands.len(), "a command", "commands"),
            self.main.as_ref().map(|_| "logic".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ")
    }
}

/// An id: lower case letters, digits and dashes, starting with a letter, 2 to 40 long.
pub fn id_ok(id: &str) -> bool {
    (2..=40).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.ends_with('-')
}

/// A path inside the plugin's folder: relative, forward slashes, no `..`.
fn inside(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains(':')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != ".." && p != ".")
}

/// Read a plugin from its folder `dir`, whose name is `folder_id`.
pub fn read(dir: &Path, folder_id: &str) -> Result<Manifest, String> {
    let path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("there's no {MANIFEST} in it")
        } else {
            format!("{MANIFEST} couldn't be read: {e}")
        }
    })?;
    parse(&text, dir, folder_id)
}

fn toml_error(e: toml::de::Error, text: &str) -> String {
    let message = e.message().trim().replace('\n', " ");
    match e.span() {
        Some(span) => {
            let before = text.get(..span.start).unwrap_or(text);
            format!("line {}: {message}", before.matches('\n').count() + 1)
        }
        None => message,
    }
}

/// Read a manifest's `text`, its files looked for in `dir`.
pub fn parse(text: &str, dir: &Path, folder_id: &str) -> Result<Manifest, String> {
    let file: ManifestFile = toml::from_str(text).map_err(|e| toml_error(e, text))?;
    let id = file.id.clone().unwrap_or_else(|| folder_id.to_string());
    if !id_ok(&id) {
        return Err(format!(
            "id \"{id}\" must be 2-40 lower case letters, digits and dashes, starting with a letter"
        ));
    }
    if id != folder_id {
        return Err(format!(
            "its id is \"{id}\" but its folder is \"{folder_id}\": name the folder after the id"
        ));
    }
    let name = file.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 40 {
        return Err("name has to be 1 to 40 characters".into());
    }
    if file.description.chars().count() > 200 {
        return Err("description has to be 200 characters or fewer".into());
    }
    semver::Version::parse(file.version.trim())
        .map_err(|e| format!("version \"{}\" isn't a version (1.2.3): {e}", file.version))?;
    let trek = file
        .trek
        .as_deref()
        .map(|r| {
            semver::VersionReq::parse(r)
                .map_err(|e| format!("trek = \"{r}\" isn't a version requirement (>=0.4.54): {e}"))
        })
        .transpose()?;
    if let Some(req) = &trek
        && !req.matches(&trek_version())
    {
        return Err(format!(
            "it needs Trek {req}, and this is Trek {}",
            trek_version()
        ));
    }
    let mut permissions: Vec<Permission> = file
        .permissions
        .iter()
        .map(|p| Permission::parse(p))
        .collect::<Result<_, _>>()?;
    permissions.sort();
    permissions.dedup();
    let file_in = |key: &str, rel: &str| -> Result<PathBuf, String> {
        if !inside(rel) {
            return Err(format!(
                "{key} = \"{rel}\": a path inside the plugin's folder, with / between parts"
            ));
        }
        let p = dir.join(rel);
        if !p.is_file() {
            return Err(format!("{key} = \"{rel}\": there's no such file"));
        }
        Ok(p)
    };
    if let Some(main) = &file.main {
        file_in("main", main)?;
    }
    for h in &file.hooks {
        if !EVENTS.contains(&h.as_str()) {
            return Err(format!(
                "hooks: \"{h}\" isn't an event; Trek has {}",
                EVENTS.join(", ")
            ));
        }
    }
    if !file.hooks.is_empty() && file.main.is_none() {
        return Err("hooks need main, the logic that hears them".into());
    }
    let tree = |what: &str, v: &toml::Value| -> Result<ui::Node, String> {
        let json: Value = match v {
            toml::Value::String(rel) => {
                let p = file_in(&format!("{what} ui"), rel)?;
                let text = std::fs::read_to_string(&p)
                    .map_err(|e| format!("{what}: {rel} couldn't be read: {e}"))?;
                serde_json::from_str(&text)
                    .map_err(|e| format!("{what}: {rel} isn't JSON (line {}): {e}", e.line()))?
            }
            toml::Value::Table(_) => serde_json::to_value(v)
                .map_err(|e| format!("{what}: its ui couldn't be read: {e}"))?,
            _ => {
                return Err(format!(
                    "{what}: ui is a table or a .json file in the plugin"
                ));
            }
        };
        let node = ui::parse(&json).map_err(|e| format!("{what}: {e}"))?;
        for src in node.images() {
            file_in(&format!("{what} image"), src)?;
        }
        Ok(node)
    };
    let mut themes = Vec::new();
    for (i, t) in file.theme.iter().enumerate() {
        let path = file_in("[[theme]] file", &t.file)?;
        let tid = match &t.id {
            Some(tid) => tid.clone(),
            None if file.theme.len() == 1 => id.clone(),
            None => format!(
                "{id}-{}",
                crate::themes::slug(
                    &Path::new(&t.file)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| i.to_string())
                )
            ),
        };
        if !id_ok(&tid) {
            return Err(format!(
                "[[theme]] id \"{tid}\" must be lower case and dashes"
            ));
        }
        themes.push(PluginTheme {
            id: tid,
            file: path,
        });
    }
    let mut slots = Vec::new();
    for s in &file.slot {
        let slot = Slot::parse(&s.at).ok_or_else(|| {
            format!(
                "[[slot]] at = \"{}\" isn't a slot; Trek has {}",
                s.at,
                Slot::ALL.map(Slot::name).join(", ")
            )
        })?;
        let node = tree(&format!("[[slot]] {}", slot.name()), &s.ui)?;
        if slot == Slot::ComposerBorder
            && !matches!(node.kind, ui::Kind::Trek(ui::Component::FocusTimer, _))
        {
            return Err(
                "[[slot]] composer.border: only Trek's components draw on the composer's edge (trek:focus-timer)"
                    .into(),
            );
        }
        slots.push(SlotUi {
            slot,
            tree: node,
            order: s.order,
        });
    }
    let pages_of = |what: &str, entries: &[PageEntry]| -> Result<Vec<Page>, String> {
        let mut out: Vec<Page> = Vec::new();
        for p in entries {
            if !id_ok(&p.id) {
                return Err(format!(
                    "[[{what}]] id \"{}\" must be lower case and dashes",
                    p.id
                ));
            }
            if out.iter().any(|o| o.id == p.id) {
                return Err(format!("[[{what}]] \"{}\" is there twice", p.id));
            }
            let entry = p
                .entry
                .clone()
                .unwrap_or_else(|| format!("pages/{}/index.html", p.id));
            file_in(&format!("[[{what}]] {} entry", p.id), &entry)?;
            let title = p.title.trim().to_string();
            if title.is_empty() || title.chars().count() > 32 {
                return Err(format!("[[{what}]] {}: title is 1 to 32 characters", p.id));
            }
            out.push(Page {
                id: p.id.clone(),
                title,
                icon: p.icon.clone().unwrap_or_else(|| "panel-top".into()),
                entry,
            });
        }
        Ok(out)
    };
    let pages = pages_of("page", &file.page)?;
    let panels = pages_of("panel", &file.panel)?;
    let mut replacements: Vec<Replacement> = Vec::new();
    for r in &file.replace {
        let surface = Surface::parse(&r.surface).ok_or_else(|| {
            format!(
                "[[replace]] surface = \"{}\" isn't one Trek lets a plugin replace ({})",
                r.surface,
                Surface::ALL.map(Surface::name).join(", ")
            )
        })?;
        if replacements.iter().any(|o| o.surface == surface) {
            return Err(format!("[[replace]] {} is there twice", surface.name()));
        }
        replacements.push(Replacement {
            surface,
            title: r.title.clone().unwrap_or_else(|| name.clone()),
            tree: tree(&format!("[[replace]] {}", surface.name()), &r.ui)?,
        });
    }
    let mut commands: Vec<Command> = Vec::new();
    for c in &file.command {
        if !id_ok(&c.id) {
            return Err(format!(
                "[[command]] id \"{}\" must be lower case and dashes",
                c.id
            ));
        }
        if commands.iter().any(|o| o.id == c.id) {
            return Err(format!("[[command]] \"{}\" is there twice", c.id));
        }
        let run = match (&c.run, &c.action) {
            (Some(f), None) => {
                if file.main.is_none() {
                    return Err(format!(
                        "[[command]] {}: run names a function of the logic, and there's no main",
                        c.id
                    ));
                }
                if f.is_empty() || !f.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    return Err(format!("[[command]] {}: run is a function's name", c.id));
                }
                CommandRun::Logic(f.clone())
            }
            (None, Some(a)) => CommandRun::Action(
                ui::Action::parse(a).map_err(|e| format!("[[command]] {}: {e}", c.id))?,
            ),
            _ => {
                return Err(format!(
                    "[[command]] {}: give it run (a function of the logic) or action, one of them",
                    c.id
                ));
            }
        };
        if let Some(k) = &c.key
            && !key_ok(k)
        {
            return Err(format!(
                "[[command]] {}: key \"{k}\" isn't a key; write it as cmd-alt-f (cmd is Ctrl on Windows)",
                c.id
            ));
        }
        commands.push(Command {
            id: c.id.clone(),
            title: c.title.trim().to_string(),
            key: c.key.clone(),
            run,
        });
    }
    let manifest = Manifest {
        id,
        name,
        version: file.version.trim().to_string(),
        description: file.description.trim().to_string(),
        author: file.author.trim().to_string(),
        trek,
        permissions,
        main: file.main,
        hooks: file.hooks,
        themes,
        slots,
        pages,
        panels,
        replacements,
        commands,
    };
    check_actions(&manifest)?;
    Ok(manifest)
}

/// Every action in the plugin names something it has, and is one its permissions allow.
fn check_actions(m: &Manifest) -> Result<(), String> {
    let trees = m
        .slots
        .iter()
        .map(|s| (s.slot.name().to_string(), &s.tree))
        .chain(
            m.replacements
                .iter()
                .map(|r| (format!("[[replace]] {}", r.surface.name()), &r.tree)),
        );
    let command_actions = m.commands.iter().filter_map(|c| match &c.run {
        CommandRun::Action(a) => Some((format!("[[command]] {}", c.id), a)),
        CommandRun::Logic(_) => None,
    });
    let mut all: Vec<(String, &ui::Action)> = Vec::new();
    for (at, tree) in trees {
        for a in tree.actions() {
            all.push((at.clone(), a));
        }
    }
    all.extend(command_actions);
    for (at, a) in all {
        match a {
            ui::Action::Command(id) if !m.commands.iter().any(|c| &c.id == id) => {
                return Err(format!(
                    "{at}: command:{id}, but it has no [[command]] \"{id}\""
                ));
            }
            ui::Action::Page(id) if !m.pages.iter().any(|p| &p.id == id) => {
                return Err(format!("{at}: page:{id}, but it has no [[page]] \"{id}\""));
            }
            ui::Action::Open(_) | ui::Action::NewThread if !m.wants(&Permission::ThreadsOpen) => {
                return Err(format!(
                    "{at}: opening threads needs the threads:open permission"
                ));
            }
            _ => {}
        }
    }
    // Trek's components that read the user's data need the permission for it.
    for (at, tree) in m
        .slots
        .iter()
        .map(|s| (s.slot.name().to_string(), &s.tree))
        .chain(
            m.replacements
                .iter()
                .map(|r| (format!("[[replace]] {}", r.surface.name()), &r.tree)),
        )
    {
        for c in tree.components() {
            if let Some(p) = c.needs()
                && !m.wants(&p)
            {
                return Err(format!(
                    "{at}: trek:{} needs the {} permission",
                    c.name(),
                    p.key()
                ));
            }
        }
    }
    Ok(())
}

/// A key as GPUI writes one: modifiers and a key joined by dashes.
fn key_ok(k: &str) -> bool {
    let parts: Vec<&str> = k.split('-').collect();
    let Some((key, mods)) = parts.split_last() else {
        return false;
    };
    !key.is_empty()
        && mods
            .iter()
            .all(|m| matches!(*m, "cmd" | "ctrl" | "alt" | "shift" | "secondary"))
        && (key.len() == 1
            || matches!(
                *key,
                "enter" | "space" | "tab" | "escape" | "up" | "down" | "left" | "right"
            )
            || key.starts_with('f'))
}

// ---------- the folder ----------

/// Where a plugin is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Its own folder in the plugins folder.
    Installed,
    /// A folder elsewhere, named by `<id>.link` (`trek plugin dev`).
    Linked,
}

/// A plugin in the folder, read and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub manifest: Manifest,
    /// The folder its files are in.
    pub dir: PathBuf,
    pub origin: Origin,
}

/// A plugin Trek couldn't use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The plugin's folder name (its id, when it got that far).
    pub id: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub plugins: Vec<Plugin>,
    pub problems: Vec<Problem>,
}

/// Every plugin folder (and link) in `dir`: `(id, folder, origin)`, in name order.
pub fn entries(dir: &Path) -> Vec<(String, PathBuf, Origin)> {
    let mut out: Vec<(String, PathBuf, Origin)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let path = e.path();
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        return None;
                    }
                    if path.is_dir() {
                        return Some((name, path, Origin::Installed));
                    }
                    let id = name.strip_suffix(&format!(".{LINK_EXT}"))?.to_string();
                    let target = std::fs::read_to_string(&path).ok()?;
                    Some((id, PathBuf::from(target.trim()), Origin::Linked))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Read every plugin in `dir` (nothing if it isn't there).
pub fn scan(dir: &Path) -> Scan {
    let mut out = Scan::default();
    for (id, folder, origin) in entries(dir) {
        if out.plugins.iter().any(|p| p.manifest.id == id) {
            out.problems.push(Problem {
                id: id.clone(),
                message: "it's installed and linked both: remove one".into(),
            });
            continue;
        }
        if !folder.is_dir() {
            out.problems.push(Problem {
                id,
                message: format!("its link names {}, which isn't a folder", folder.display()),
            });
            continue;
        }
        match read(&folder, &id) {
            Ok(manifest) => out.plugins.push(Plugin {
                manifest,
                dir: folder,
                origin,
            }),
            Err(message) => out.problems.push(Problem { id, message }),
        }
    }
    out
}

/// What a read depends on: every file's path, size and time in the folder and in linked
/// folders (`MAX_FILES` a plugin at most), and the state file. A read is due when it changes.
pub type Fingerprint = Vec<(String, u64, Option<std::time::SystemTime>)>;

pub fn fingerprint(dir: &Path) -> Fingerprint {
    let mut out = Fingerprint::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let walk = |root: &Path, tag: &str, out: &mut Fingerprint| {
        for e in walkdir::WalkDir::new(root)
            .max_depth(6)
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy();
                !(n.starts_with('.') && e.depth() > 0) && n != "node_modules"
            })
            .flatten()
            .take(MAX_FILES)
        {
            if let Ok(meta) = e.metadata()
                && meta.is_file()
            {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((format!("{tag}/{rel}"), meta.len(), meta.modified().ok()));
            }
        }
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == PROBLEMS_FILE {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            walk(&path, &name, &mut out);
        } else if let Ok(meta) = e.metadata() {
            out.push((name.clone(), meta.len(), meta.modified().ok()));
            if name.ends_with(&format!(".{LINK_EXT}"))
                && let Ok(target) = std::fs::read_to_string(&path)
            {
                walk(Path::new(target.trim()), &name, &mut out);
            }
        }
    }
    out.sort();
    out
}

/// Keep `problems.txt` saying what's wrong (gone when nothing is); written only on a change.
pub fn write_problems(dir: &Path, problems: &[Problem]) {
    let path = dir.join(PROBLEMS_FILE);
    if problems.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let text: String = problems
        .iter()
        .map(|p| format!("{}: {}\n", p.id, p.message.replace('\n', "\n    ")))
        .collect();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&path, text);
    }
}

/// Whether a reload was asked for (`trek plugin reload`), the request taken away.
pub fn take_reload(dir: &Path) -> bool {
    std::fs::remove_file(dir.join(RELOAD_FILE)).is_ok()
}

// ---------- the user's choices ----------

/// What the user chose for one plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Turned on.
    #[serde(default)]
    pub on: bool,
    /// The permissions the user allowed, by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted: Vec<String>,
    /// Asked to be turned on (by the CLI, or an update that asks for more) and waiting for the
    /// user to allow what it asks for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waiting: bool,
    /// Why Trek stopped it, when it did (an error in its logic, a frame it couldn't draw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

/// `state.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub plugins: BTreeMap<String, Choice>,
    /// Each replaceable surface's chosen plugin; missing is Trek's own.
    #[serde(default)]
    pub replace: BTreeMap<Surface, String>,
}

impl State {
    pub fn read(dir: &Path) -> State {
        std::fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write it, if it changed.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let path = dir.join(STATE_FILE);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{STATE_FILE}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    pub fn choice(&self, id: &str) -> Choice {
        self.plugins.get(id).cloned().unwrap_or_default()
    }

    /// The permissions `m` asks for that the user hasn't allowed.
    pub fn missing(&self, m: &Manifest) -> Vec<Permission> {
        let granted = self.choice(&m.id).granted;
        m.permissions
            .iter()
            .filter(|p| !granted.contains(&p.key()))
            .cloned()
            .collect()
    }

    /// Whether `m` runs: on, everything it asks for allowed, not stopped.
    pub fn running(&self, m: &Manifest) -> bool {
        let c = self.choice(&m.id);
        c.on && c.stopped.is_none() && self.missing(m).is_empty()
    }

    /// Turn `m` on as the user (who sees what it asks for, and so allows it).
    pub fn turn_on_allowing(&mut self, m: &Manifest) {
        let c = self.plugins.entry(m.id.clone()).or_default();
        c.on = true;
        c.waiting = false;
        c.stopped = None;
        c.granted = m.permissions.iter().map(Permission::key).collect();
    }

    /// Turn `m` on from the CLI (an agent may be the one asking): on if it needs nothing it
    /// hasn't been allowed; else it waits for the user. Whether it's on.
    pub fn ask_on(&mut self, m: &Manifest) -> bool {
        let missing = self.missing(m);
        let c = self.plugins.entry(m.id.clone()).or_default();
        c.stopped = None;
        if missing.is_empty() {
            c.on = true;
            c.waiting = false;
            true
        } else {
            c.waiting = true;
            false
        }
    }

    pub fn turn_off(&mut self, id: &str) {
        let c = self.plugins.entry(id.to_string()).or_default();
        c.on = false;
        c.waiting = false;
    }

    /// Trek stopped `id`: it stays off until the user turns it on again.
    pub fn stop(&mut self, id: &str, why: &str) {
        let c = self.plugins.entry(id.to_string()).or_default();
        c.stopped = Some(why.chars().take(300).collect());
        self.replace.retain(|_, p| p != id);
    }

    pub fn forget(&mut self, id: &str) {
        self.plugins.remove(id);
        self.replace.retain(|_, p| p != id);
    }
}

// ---------- logs and storage ----------

/// Add a line to plugin `id`'s log (with the time), cutting it back when it's grown long.
pub fn log(id: &str, line: &str) {
    let dir = data_dir(id);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("log.txt");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let keep = &text[text.len().saturating_sub(LOG_LIMIT as usize / 2)..];
        let keep = keep.split_once('\n').map_or(keep, |(_, rest)| rest);
        let _ = std::fs::write(&path, keep);
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line: String = line.chars().take(2000).collect();
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{stamp} {}", line.replace('\n', " ⏎ "));
    }
}

/// The last `n` lines of plugin `id`'s log.
pub fn log_tail(id: &str, n: usize) -> Vec<String> {
    let text = std::fs::read_to_string(data_dir(id).join("log.txt")).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// Plugin `id`'s own storage, a JSON object.
pub fn storage(id: &str) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(data_dir(id).join("storage.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Keep `value` as plugin `id`'s storage; refused past `STORAGE_LIMIT`.
pub fn set_storage(id: &str, value: &serde_json::Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > STORAGE_LIMIT {
        return Err(format!(
            "storage is {} KB; a plugin keeps at most {} KB",
            text.len() / 1024,
            STORAGE_LIMIT / 1024
        ));
    }
    let dir = data_dir(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("storage.json"), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-plugins-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plugin(root: &Path, id: &str, manifest: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest).unwrap();
        dir
    }

    #[test]
    fn a_small_manifest_is_a_plugin() {
        let root = scratch("small");
        plugin(&root, "hello", "name = \"Hello\"\n");
        let s = scan(&root);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let m = &s.plugins[0].manifest;
        assert_eq!((m.id.as_str(), m.version.as_str()), ("hello", "0.1.0"));
        assert!(m.permissions.is_empty() && m.slots.is_empty());
        assert_eq!(m.summary(), "");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn everything_a_plugin_brings_is_read_and_checked() {
        let root = scratch("full");
        let dir = plugin(
            &root,
            "burn",
            r#"
name = "Burn"
version = "1.2.0"
trek = ">=0.4.0"
permissions = ["usage:read", "threads:open", "network:api.example.com"]
main = "main.js"
hooks = ["turn.end"]

[[theme]]
file = "look.toml"

[[slot]]
at = "thread.header"
ui = { type = "text", text = "{thread.cost|usd}", fg = "warning" }

[[slot]]
at = "composer.actions"
ui = "ui/button.json"

[[page]]
id = "burn"
title = "Burn"
icon = "flame"

[[replace]]
surface = "thread-list"
ui = { type = "column", children = [{ type = "text", text = "Mine" }] }

[[command]]
id = "reset"
title = "Reset the burn meter"
key = "cmd-alt-b"
run = "reset"
"#,
        );
        std::fs::write(dir.join("main.js"), "").unwrap();
        std::fs::write(dir.join("look.toml"), "name = \"Burn\"\n").unwrap();
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(
            dir.join("ui/button.json"),
            r#"{"type":"button","text":"Reset","on":"command:reset"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("pages/burn")).unwrap();
        std::fs::write(dir.join("pages/burn/index.html"), "<h1>Burn</h1>").unwrap();
        let s = scan(&root);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let m = &s.plugins[0].manifest;
        assert_eq!(m.themes[0].id, "burn");
        assert_eq!(m.slots.len(), 2);
        assert_eq!(m.pages[0].entry, "pages/burn/index.html");
        assert_eq!(m.replacements[0].surface, Surface::ThreadList);
        assert_eq!(m.commands[0].run, CommandRun::Logic("reset".into()));
        assert!(m.wants(&Permission::Network("api.example.com".into())));
        assert_eq!(
            m.summary(),
            "a theme, 2 slots, a page, a replacement, a command, logic"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_plugin_says_why() {
        let root = scratch("bad");
        let bad = |manifest: &str| {
            let _ = std::fs::remove_dir_all(root.join("x1"));
            plugin(&root, "x1", manifest);
            parse(manifest, &root.join("x1"), "x1").unwrap_err()
        };
        assert!(bad("name = \"X\"\nid = \"other\"\n").contains("folder"));
        assert!(bad("name = \"X\"\ncolour = 1\n").contains("colour"));
        assert!(bad("name = \"X\"\npermissions = [\"disk:write\"]\n").contains("disk:write"));
        assert!(bad("name = \"X\"\npermissions = [\"network:*\"]\n").contains("one host"));
        assert!(bad("name = \"X\"\ntrek = \">=99.0.0\"\n").contains("needs Trek"));
        assert!(bad("name = \"X\"\nmain = \"../../etc/passwd\"\n").contains("inside"));
        assert!(bad("name = \"X\"\nmain = \"gone.js\"\n").contains("no such file"));
        assert!(
            bad("name = \"X\"\n[[slot]]\nat = \"title\"\nui = { type = \"spacer\" }\n")
                .contains("composer.actions")
        );
        let e = bad(
            "name = \"X\"\n[[slot]]\nat = \"thread.header\"\nui = { type = \"button\", text = \"Go\", on = \"command:go\" }\n",
        );
        assert!(e.contains("no [[command]]"), "{e}");
        let e = bad(
            "name = \"X\"\n[[slot]]\nat = \"thread.header\"\nui = { type = \"button\", text = \"Go\", on = \"new-thread\" }\n",
        );
        assert!(e.contains("threads:open"), "{e}");
        let e = bad(
            "name = \"X\"\n[[slot]]\nat = \"composer.border\"\nui = { type = \"text\", text = \"x\" }\n",
        );
        assert!(e.contains("edge"), "{e}");
        let e = bad("name = \"X\"\n[[command]]\nid = \"go\"\ntitle = \"Go\"\nrun = \"go\"\n");
        assert!(e.contains("no main"), "{e}");
        let e = bad(
            "name = \"X\"\n[[command]]\nid = \"go\"\ntitle = \"Go\"\naction = \"new-thread\"\nkey = \"hyper-q\"\n",
        );
        assert!(e.contains("threads:open") || e.contains("key"), "{e}");
        let e = bad("name = \"X\"\n[[page]]\nid = \"p\"\ntitle = \"P\"\n");
        assert!(
            e.contains("lower case") || e.contains("no such file"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_folder_is_read_with_links_and_problems() {
        let root = scratch("scan");
        let dev = scratch("scan-dev");
        plugin(&root, "good", "name = \"Good\"\n");
        plugin(&root, "Bad Name", "name = \"B\"\n");
        std::fs::write(dev.join(MANIFEST), "name = \"Dev\"\nid = \"dev\"\n").unwrap();
        let dev_link = root.join("dev.link");
        std::fs::write(&dev_link, dev.display().to_string()).unwrap();
        let s = scan(&root);
        let ids: Vec<&str> = s.plugins.iter().map(|p| p.manifest.id.as_str()).collect();
        assert_eq!(ids, ["dev", "good"]);
        assert_eq!(s.plugins[0].origin, Origin::Linked);
        assert_eq!(s.problems.len(), 1, "{:?}", s.problems);
        write_problems(&root, &s.problems);
        assert!(
            std::fs::read_to_string(root.join(PROBLEMS_FILE))
                .unwrap()
                .starts_with("Bad Name: ")
        );
        // An edit in a linked folder is a change; writing problems isn't.
        let before = fingerprint(&root);
        write_problems(&root, &[]);
        assert_eq!(fingerprint(&root), before);
        std::fs::write(dev.join("main.js"), "// more").unwrap();
        assert_ne!(fingerprint(&root), before);
        assert_eq!(scan(&root.join("nope")), Scan::default());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(dev);
    }

    #[test]
    fn an_agent_cant_grant_itself_permissions() {
        let root = scratch("grant");
        plugin(&root, "quiet", "name = \"Quiet\"\n");
        plugin(
            &root,
            "nosy",
            "name = \"Nosy\"\npermissions = [\"threads:read\"]\n",
        );
        let s = scan(&root);
        let quiet = &s
            .plugins
            .iter()
            .find(|p| p.manifest.id == "quiet")
            .unwrap()
            .manifest;
        let nosy = &s
            .plugins
            .iter()
            .find(|p| p.manifest.id == "nosy")
            .unwrap()
            .manifest;
        let mut st = State::default();
        assert!(st.ask_on(quiet), "a plugin that asks for nothing goes on");
        assert!(!st.ask_on(nosy), "one that asks waits for the user");
        assert!(st.choice("nosy").waiting && !st.running(nosy));
        st.turn_on_allowing(nosy);
        assert!(st.running(nosy));
        // An update that asks for more stops it until allowed again.
        let mut more = nosy.clone();
        more.permissions.push(Permission::ThreadsSend);
        assert!(!st.running(&more));
        // Stopped by Trek: off until turned on again, its replacement given back.
        st.replace.insert(Surface::ThreadList, "nosy".into());
        st.stop("nosy", "TypeError: x is undefined");
        assert!(!st.running(nosy) && st.replace.is_empty());
        st.write(&root).unwrap();
        assert_eq!(State::read(&root), st);
        let _ = std::fs::remove_dir_all(root);
    }
}
