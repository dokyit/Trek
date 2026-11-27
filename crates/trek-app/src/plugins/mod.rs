//! Trek plugins in the app (`trek_core::plugins` has the format, the folder and the CLI;
//! `docs/PLUGINS.md` the design): the plugins installed and which are running, the folder read
//! again whenever it changes, and the user's choices kept.
//!
//! What a running plugin brings is indexed here once, when the folder is read, so a frame asks a
//! map and draws: with no plugins every slot's list is empty, and nothing else is made.

pub mod data;
pub mod focus;
pub mod hiker;
pub mod logic;
pub mod page;
pub mod render;
pub mod trail;

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use trek_core::plugins::{self as core, Manifest, Plugin, Problem, Slot, State};

/// A running plugin's slot tree, with whose it is.
#[derive(Clone)]
pub struct Placed {
    pub plugin: Rc<str>,
    pub tree: Rc<trek_core::plugins::ui::Node>,
    pub dir: Rc<PathBuf>,
}

/// The plugins: what's installed, what's wrong, what the user chose, and what the running ones
/// bring, by place.
pub struct Plugins {
    dir: PathBuf,
    pub plugins: Vec<Plugin>,
    pub problems: Vec<Problem>,
    pub state: State,
    fingerprint: core::Fingerprint,
    /// Each slot's trees, from the running plugins, in order.
    slots: BTreeMap<Slot, Vec<Placed>>,
    /// The surfaces a running plugin replaces, as the user chose (`State::replace`).
    replacements: BTreeMap<trek_core::plugins::Surface, Placed>,
    /// The running plugins' text effects over the composer's draft.
    effects: Rc<Vec<trek_core::plugins::TextEffect>>,
    /// The plugin whose log Settings shows, and its last lines (read when it's opened or the
    /// folder changes, never while drawing).
    pub log: Option<(String, Vec<String>)>,
    /// Each plugin's state, as its logic or its `set:` actions left it: what `{state.…}` reads.
    pub states: HashMap<String, Value>,
    /// How many times the folder has been read again: an open page reloads when it changes.
    pub generation: u64,
}

impl Global for Plugins {}

impl Plugins {
    fn index(&mut self) {
        self.slots.clear();
        self.replacements.clear();
        self.effects = Rc::new(
            self.plugins
                .iter()
                .filter(|p| self.state.running(&p.manifest))
                .flat_map(|p| p.manifest.effects.iter().cloned())
                .collect(),
        );
        for (surface, id) in &self.state.replace {
            let Some(p) = self.plugins.iter().find(|p| &p.manifest.id == id) else {
                continue;
            };
            if !self.state.running(&p.manifest) {
                continue;
            }
            if let Some(r) = p
                .manifest
                .replacements
                .iter()
                .find(|r| r.surface == *surface)
            {
                self.replacements.insert(
                    *surface,
                    Placed {
                        plugin: id.as_str().into(),
                        tree: Rc::new(r.tree.clone()),
                        dir: Rc::new(p.dir.clone()),
                    },
                );
            }
        }
        for p in &self.plugins {
            if !self.state.running(&p.manifest) {
                continue;
            }
            let id: Rc<str> = p.manifest.id.as_str().into();
            let dir = Rc::new(p.dir.clone());
            for s in &p.manifest.slots {
                self.slots.entry(s.slot).or_default().push(Placed {
                    plugin: id.clone(),
                    tree: Rc::new(s.tree.clone()),
                    dir: dir.clone(),
                });
            }
        }
        for list in self.slots.values_mut() {
            list.sort_by_key(|p| p.plugin.clone());
        }
        let orders: BTreeMap<(Slot, Rc<str>), i32> = self
            .plugins
            .iter()
            .flat_map(|p| {
                p.manifest
                    .slots
                    .iter()
                    .map(|s| ((s.slot, Rc::<str>::from(p.manifest.id.as_str())), s.order))
            })
            .collect();
        for (slot, list) in self.slots.iter_mut() {
            list.sort_by_key(|p| {
                (
                    orders.get(&(*slot, p.plugin.clone())).copied().unwrap_or(0),
                    p.plugin.clone(),
                )
            });
        }
    }

    pub fn find(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.id == id)
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }
}

/// The trees a slot shows: empty with no plugins (the common case, and free).
pub fn slot(slot: Slot, cx: &App) -> &[Placed] {
    cx.try_global::<Plugins>()
        .and_then(|p| p.slots.get(&slot))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The running plugins' text effects (none, usually).
pub fn text_effects(cx: &App) -> Rc<Vec<trek_core::plugins::TextEffect>> {
    cx.try_global::<Plugins>()
        .map(|p| p.effects.clone())
        .unwrap_or_default()
}

/// The running plugin's surface the user chose in place of Trek's `surface`, if any.
pub fn replacement(surface: trek_core::plugins::Surface, cx: &App) -> Option<Placed> {
    cx.try_global::<Plugins>()?
        .replacements
        .get(&surface)
        .cloned()
}

/// The running plugins that offer a replacement for `surface`: (id, the replacement's title).
pub fn replacements_for(surface: trek_core::plugins::Surface, cx: &App) -> Vec<(String, String)> {
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    p.plugins
        .iter()
        .filter(|pl| p.state.running(&pl.manifest))
        .filter_map(|pl| {
            pl.manifest
                .replacements
                .iter()
                .find(|r| r.surface == surface)
                .map(|r| (pl.manifest.id.clone(), r.title.clone()))
        })
        .collect()
}

/// Show plugin `plugin`'s replacement for `surface` (`None`: Trek's own).
pub fn choose_replacement(
    surface: trek_core::plugins::Surface,
    plugin: Option<&str>,
    ws: &Entity<Workspace>,
    cx: &mut App,
) {
    update_state(ws, cx, |s| match plugin {
        Some(id) => {
            s.replace.insert(surface, id.to_string());
        }
        None => {
            s.replace.remove(&surface);
        }
    });
}

/// Whether plugin `id` is running.
pub fn running(id: &str, cx: &App) -> bool {
    cx.try_global::<Plugins>()
        .and_then(|p| p.find(id).map(|pl| p.state.running(&pl.manifest)))
        .unwrap_or(false)
}

/// Whether one of plugin `id`'s looks is the one on screen.
pub fn look_in_use(m: &Manifest, cx: &App) -> bool {
    let shown = crate::themes::shown_id(cx);
    m.themes.iter().any(|t| t.id == shown)
}

/// A look was chosen over another (`from` to `to`; Settings › Appearance, the palette, a plugin
/// turned on, an agent's `apply`). The running plugin whose look `from` was, if `to` isn't its
/// own, is put off and remembers it: a plugin's look is on only while it's the one on screen,
/// so Settings never shows a plugin on whose look isn't, and turning it on brings the look
/// back (`set_on`).
pub fn look_changed(from: &str, to: &str, ws: &Entity<Workspace>, cx: &mut App) {
    if from == to {
        return;
    }
    let Some(p) = cx.try_global::<Plugins>() else {
        return;
    };
    let displaced: Vec<String> = p
        .plugins
        .iter()
        .filter(|pl| {
            let m = &pl.manifest;
            p.state.running(m)
                && m.themes.iter().any(|t| t.id == from)
                && !m.themes.iter().any(|t| t.id == to)
        })
        .map(|pl| pl.manifest.id.clone())
        .collect();
    if displaced.is_empty() {
        return;
    }
    update_state(ws, cx, |s| {
        for id in &displaced {
            s.turn_off(id);
            if let Some(c) = s.plugins.get_mut(id) {
                c.look = Some(from.to_string());
            }
        }
    });
    for id in &displaced {
        core::log(
            id,
            &format!("turned off: the look {to} was chosen over its own"),
        );
    }
}

/// The running plugins' manifests.
pub fn running_manifests(cx: &App) -> Vec<Manifest> {
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    p.plugins
        .iter()
        .filter(|pl| p.state.running(&pl.manifest))
        .map(|pl| pl.manifest.clone())
        .collect()
}

/// This install's key for sealing grants (`trek_core::plugins::grant`), read from the system's
/// secret store the first time a plugin that asks for permissions is there, and kept. `None`
/// while the store can't be read.
fn grant_key() -> Option<core::grant::Key> {
    static KEY: std::sync::OnceLock<core::grant::Key> = std::sync::OnceLock::new();
    if let Some(key) = KEY.get() {
        return Some(*key);
    }
    let key = core::grant::key()?;
    Some(*KEY.get_or_init(|| key))
}

/// The user's choices as read from `dir`, less every grant that isn't sealed for its plugin as
/// it is now: one written into the file by hand, or one for a plugin whose files or permissions
/// changed since the user allowed it. Those plugins wait in Settings. The secret store is read
/// only when some plugin asks for permissions.
fn checked_state(dir: &std::path::Path, scan: &core::Scan) -> State {
    let mut state = State::read(dir);
    check(&mut state, &scan.plugins);
    state
}

/// Take away from `state` every grant that isn't sealed for its plugin as it is now (above).
fn check(state: &mut State, plugins: &[Plugin]) {
    if plugins.iter().any(|p| !p.manifest.permissions.is_empty()) {
        for id in state.verify(plugins, grant_key().as_ref()) {
            core::log(
                &id,
                "its permissions aren't sealed for it as it is now (its files changed, or the grant wasn't made in Settings): it waits to be allowed",
            );
        }
    }
}

/// Read the folder, index what's running, and watch it. Before any window opens.
pub fn init(ws: &Entity<Workspace>, cx: &mut App) {
    let dir = core::dir();
    let fingerprint = core::fingerprint(&dir);
    let scan = core::scan(&dir);
    let state = checked_state(&dir, &scan);
    let _ = core::take_reload(&dir);
    if !scan.problems.is_empty() || dir.join(core::PROBLEMS_FILE).exists() {
        core::write_problems(&dir, &scan.problems);
    }
    let mut plugins = Plugins {
        dir,
        plugins: scan.plugins,
        problems: scan.problems,
        state,
        fingerprint,
        slots: BTreeMap::new(),
        replacements: BTreeMap::new(),
        effects: Default::default(),
        log: None,
        states: HashMap::new(),
        generation: 0,
    };
    plugins.index();
    cx.set_global(plugins);
    contributions_changed(ws, cx);
}

/// The plugins folder, for the themes folder's watch to look at on its own tick (one timer for
/// both folders: plugins add no wake-ups of their own).
pub fn watched_dir(cx: &App) -> Option<PathBuf> {
    cx.try_global::<Plugins>().map(|p| p.dir.clone())
}

/// What a look at `dir` found, off the UI thread: its fingerprint, and whether a reload was
/// asked for.
pub fn look(dir: &std::path::Path) -> (core::Fingerprint, bool) {
    (core::fingerprint(dir), core::take_reload(dir))
}

/// Take a look (`look`): when the folder changed, or a reload was asked for, it's read again off
/// the UI thread and what's running is indexed again.
pub fn looked(print: core::Fingerprint, asked: bool, ws: &Entity<Workspace>, cx: &mut App) {
    let Some(p) = cx.try_global::<Plugins>() else {
        return;
    };
    if p.fingerprint == print && !asked {
        return;
    }
    let dir = p.dir.clone();
    let ws = ws.downgrade();
    cx.spawn(async move |cx| {
        let (scan, state) = cx
            .background_spawn(async move {
                let scan = core::scan(&dir);
                let state = checked_state(&dir, &scan);
                (scan, state)
            })
            .await;
        cx.update(|cx| {
            if let Some(ws) = ws.upgrade() {
                apply_scan(scan, state, print, true, &ws, cx);
            }
        });
    })
    .detach();
}

/// Take a read of the folder: what's new is on offer, a bad plugin is said once, and what the
/// running plugins bring is indexed again.
///
/// A plugin that wasn't there before is announced (`announce`): the user hears that it was added
/// and where to manage it, whoever wrote it (an agent, by hand, the CLI), and it's noted when it
/// appeared and which agent was at work then (`Added`).
fn apply_scan(
    scan: core::Scan,
    state: State,
    print: core::Fingerprint,
    announce: bool,
    ws: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(p) = cx.try_global::<Plugins>() else {
        return;
    };
    let dir = p.dir.clone();
    let fresh: Vec<Plugin> = scan
        .plugins
        .iter()
        .filter(|pl| {
            !p.plugins
                .iter()
                .any(|old| old.manifest.id == pl.manifest.id)
        })
        .cloned()
        .collect();
    let new_problems: Vec<Problem> = scan
        .problems
        .iter()
        .filter(|x| !p.problems.contains(x))
        .cloned()
        .collect();
    let newly_waiting: Vec<String> = scan
        .plugins
        .iter()
        .filter(|pl| {
            state.choice(&pl.manifest.id).waiting
                && !p.state.choice(&pl.manifest.id).waiting
                && !fresh.iter().any(|f| f.manifest.id == pl.manifest.id)
        })
        .map(|pl| pl.manifest.name.clone())
        .collect();
    let mut state = state;
    let mut print = print;
    let by = if fresh.is_empty() {
        None
    } else {
        at_work(ws, cx)
    };
    // Noted over the file as it is now (`State::update`, under the lock `trek plugin` takes too),
    // not over the copy the scan read off this thread: an agent turns the plugin it just wrote on
    // at once (`trek plugin enable`), which could land between that read and this write. Writing
    // the older copy back turned the plugin off again, and the fingerprint taken after the write
    // hid that anything had changed, so it stayed off.
    if fresh
        .iter()
        .any(|pl| state.choice(&pl.manifest.id).added.is_none())
    {
        let at = trek_core::store::now_ms();
        let noted = State::update(&dir, |file| {
            check(file, &scan.plugins);
            let mut noted = false;
            for pl in &fresh {
                let added = core::Added {
                    at,
                    by: by.as_ref().map(|w| w.agent.clone()),
                    thread: by.as_ref().map(|w| w.thread.clone()),
                    title: by.as_ref().map(|w| w.title.clone()),
                };
                noted |= file.note_added(&pl.manifest.id, added);
            }
            noted
        });
        match noted {
            Ok((now, noted)) => {
                state = now;
                if noted {
                    // Kept with the user's choices; what was just written isn't news to the next look.
                    print = core::fingerprint(&dir);
                }
            }
            Err(e) => tracing::warn!("plugins: couldn't note what was added: {e}"),
        }
    }
    core::write_problems(&dir, &scan.problems);
    {
        let p = cx.global_mut::<Plugins>();
        p.generation += 1;
        p.plugins = scan.plugins;
        p.problems = scan.problems;
        p.state = state.clone();
        p.fingerprint = print;
        p.index();
        if let Some((id, _)) = p.log.clone() {
            p.log = Some((id.clone(), core::log_tail(&id, 12)));
        }
    }
    let toast = |message: String, cx: &mut App| {
        ws.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        })
    };
    if let Some(problem) = new_problems.first() {
        toast(
            format!(
                "Plugin {} wasn't loaded: {}",
                problem.id,
                problem.message.lines().next().unwrap_or_default()
            ),
            cx,
        );
    }
    if let Some(name) = newly_waiting.first() {
        toast(
            format!("{name} asks for permissions: allow it in Settings › Plugins"),
            cx,
        );
    }
    if announce && !fresh.is_empty() {
        let message = added_message(&fresh, &state, by.as_ref());
        ws.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: Some(crate::workspace::UndoAction::OpenPlugins),
            })
        });
    }
    contributions_changed(ws, cx);
}

/// When a plugin was added and by whom, for its row in Settings ("Added 18 Nov 2026 by Codex
/// (“Add a marmot”)"). `at` is milliseconds since the epoch; nothing known gives an empty line.
pub fn added_text(at: Option<i64>, by: Option<&str>, title: Option<&str>) -> String {
    use chrono::TimeZone as _;
    let when = at
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|t| t.format("%-d %b %Y").to_string());
    match (when, by) {
        (None, None) => String::new(),
        (Some(w), None) => format!("Added {w}"),
        (None, Some(b)) => format!("Added by {b}"),
        (Some(w), Some(b)) => match title.filter(|t| !t.trim().is_empty()) {
            Some(t) => format!(
                "Added {w} by {b} (“{}”)",
                t.chars().take(40).collect::<String>().trim()
            ),
            None => format!("Added {w} by {b}"),
        },
    }
}

/// When the plugin's folder (or the theme's file) appeared, as the file system has it: what
/// Settings says of one Trek wasn't running when it was added.
pub fn made_at(path: &std::path::Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let t = meta.created().or_else(|_| meta.modified()).ok()?;
    let ms = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}

/// An agent at work in a thread, as Settings names who made a plugin.
pub struct AtWork {
    pub agent: String,
    pub thread: String,
    pub title: String,
}

/// The agent at work right now, when it's exactly one: a plugin that appears then was most
/// likely its doing. With none at work, or several, nobody is named.
pub fn at_work(ws: &Entity<Workspace>, cx: &App) -> Option<AtWork> {
    let ws = ws.read(cx);
    let mut working = ws.threads.iter().filter(|t| {
        t.archived_at.is_none()
            && (t.run_state == trek_core::RunState::Working || ws.turn_running(&t.id))
    });
    let t = working.next()?;
    working.next().is_none().then(|| AtWork {
        agent: t.agent.display_name(),
        thread: t.id.clone(),
        title: t.title.clone(),
    })
}

/// What the user is told when `fresh` plugins appear: what was added, by whom if known, whether
/// it runs or waits, and where to manage it.
pub fn added_message(fresh: &[Plugin], state: &State, by: Option<&AtWork>) -> String {
    let who = by.map(|w| format!(" by {}", w.agent)).unwrap_or_default();
    match fresh {
        [one] => {
            let m = &one.manifest;
            if !m.permissions.is_empty() && !state.missing(m).is_empty() {
                format!(
                    "{} was added{who} and asks for permissions. Allow it in Settings › Plugins.",
                    m.name
                )
            } else if state.running(m) {
                format!(
                    "{} was added{who} and is on. Manage it in Settings › Plugins.",
                    m.name
                )
            } else {
                format!(
                    "{} was added{who}; it's off. Turn it on in Settings › Plugins.",
                    m.name
                )
            }
        }
        many => {
            let names: Vec<&str> = many.iter().map(|p| p.manifest.name.as_str()).collect();
            format!(
                "{} plugins were added{who}: {}. Manage them in Settings › Plugins.",
                many.len(),
                names.join(", ")
            )
        }
    }
}

/// Whether a running plugin places Trek's component `c` anywhere.
fn places(c: trek_core::plugins::ui::Component, cx: &App) -> bool {
    cx.try_global::<Plugins>().is_some_and(|p| {
        p.slots
            .values()
            .flatten()
            .any(|placed| placed.tree.components().contains(&c))
            || p.plugins.iter().any(|pl| {
                p.state.running(&pl.manifest)
                    && pl
                        .manifest
                        .replacements
                        .iter()
                        .any(|r| r.tree.components().contains(&c))
            })
    })
}

/// The running plugins' contributions changed: the themes they bring, and every view.
fn contributions_changed(ws: &Entity<Workspace>, cx: &mut App) {
    // The commit trail's reader, the first time a plugin places one.
    if !cx.has_global::<trail::Trails>()
        && places(trek_core::plugins::ui::Component::CommitTrail, cx)
    {
        trail::init(cx);
    }
    // The focus timer's walk from before a restart, the first time a plugin places it.
    focus::restore(ws, cx);
    let themes: Vec<(String, PathBuf)> = cx
        .try_global::<Plugins>()
        .map(|p| {
            p.plugins
                .iter()
                .filter(|pl| p.state.running(&pl.manifest))
                .flat_map(|pl| {
                    pl.manifest
                        .themes
                        .iter()
                        .map(|t| (t.id.clone(), t.file.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    crate::themes::set_plugin_themes(themes, ws, cx);
    logic::hooks_changed(ws, cx);
    bind_keys(cx);
    ws.update(cx, |_, cx| cx.notify());
    cx.refresh_windows();
}

/// The running plugins' commands that have a key: (the key, plugin, command).
struct Keys {
    table: Vec<(Keystroke, String, String)>,
    _heard: Subscription,
}

impl Global for Keys {}

/// The keys a plugin's command has here: the user's, where they changed them on the Keyboard
/// Shortcuts page, else its manifest's (`cmd-alt-f`, with Ctrl for `cmd` off the Mac).
fn command_key(plugin: &str, command: &str, manifest: Option<&str>) -> Option<Keystroke> {
    let key = crate::keys::plugin_key(plugin, command, manifest)?;
    Keystroke::parse(&key).ok()
}

/// Trek's keys changed (the user's on the Keyboard Shortcuts page): hear the plugins' again, so
/// one that Trek's keymap now has gives way, and one it let go of is heard.
pub fn keys_changed(cx: &mut App) {
    bind_keys(cx);
}

/// The running plugins' commands, for the Keyboard Shortcuts page: each can be given keys there.
pub fn key_commands(cx: &App) -> Vec<crate::keys::PluginCommand> {
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    p.plugins
        .iter()
        .filter(|pl| p.state.running(&pl.manifest))
        .flat_map(|pl| {
            pl.manifest
                .commands
                .iter()
                .map(|c| crate::keys::PluginCommand {
                    plugin: pl.manifest.id.clone(),
                    plugin_name: pl.manifest.name.clone(),
                    command: c.id.clone(),
                    title: c.title.clone(),
                    key: c.key.clone(),
                })
        })
        .collect()
}

/// Whether a plugin command's keys are taken by one of Trek's (the user's own choice included),
/// so it's heard without them.
pub fn key_taken(plugin: &str, command: &str, manifest: Option<&str>, cx: &App) -> bool {
    command_key(plugin, command, manifest).is_some_and(|k| trek_has(&k, cx))
}

/// Whether Trek's own keymap already has `key` (on its own, in any context): a plugin's command
/// on it would run as well as Trek's, so it keeps only its palette entry.
fn trek_has(key: &Keystroke, cx: &App) -> bool {
    cx.key_bindings().borrow().bindings().any(
        |b| matches!(b.keystrokes(), [k] if k.key() == key.key && *k.modifiers() == key.modifiers),
    )
}

/// A plugin command's key as the user has it, when it's one Trek can give it: well formed, and
/// not one of Trek's.
pub fn free_key(
    plugin: &str,
    command: &str,
    manifest: Option<&str>,
    cx: &App,
) -> Option<Keystroke> {
    command_key(plugin, command, manifest).filter(|k| !trek_has(k, cx))
}

/// Hear the keys the running plugins' commands name: nothing is heard while none has a key.
fn bind_keys(cx: &mut App) {
    let wanted: Vec<(Keystroke, String, String)> = cx
        .try_global::<Plugins>()
        .map(|p| {
            p.plugins
                .iter()
                .filter(|pl| p.state.running(&pl.manifest))
                .flat_map(|pl| {
                    pl.manifest.commands.iter().filter_map(|c| {
                        Some((
                            command_key(&pl.manifest.id, &c.id, c.key.as_deref())?,
                            pl.manifest.id.clone(),
                            c.id.clone(),
                        ))
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // A key Trek has already stays Trek's: the command is still in the palette.
    let (taken, table): (Vec<_>, Vec<_>) =
        wanted.into_iter().partition(|(k, _, _)| trek_has(k, cx));
    for (key, plugin, command) in taken {
        core::log(
            &plugin,
            &format!("{command}'s key {key} is one of Trek's own: it's in the palette without it"),
        );
    }
    if table.is_empty() {
        if cx.has_global::<Keys>() {
            cx.remove_global::<Keys>();
        }
        return;
    }
    if cx.has_global::<Keys>() {
        cx.global_mut::<Keys>().table = table;
        return;
    }
    let heard = cx.observe_keystrokes(|event, window, cx| {
        let Some((plugin, command)) = cx.try_global::<Keys>().and_then(|k| {
            k.table
                .iter()
                .find(|(key, _, _)| {
                    key.key == event.keystroke.key && key.modifiers == event.keystroke.modifiers
                })
                .map(|(_, p, c)| (p.clone(), c.clone()))
        }) else {
            return;
        };
        if let Some(ws) = cx
            .try_global::<crate::workspace::GlobalWorkspace>()
            .map(|g| g.0.clone())
        {
            run_command(&plugin, &command, &ws, window, cx);
        }
    });
    cx.set_global(Keys {
        table,
        _heard: heard,
    });
}

/// What the palette offers for plugins: each running plugin's commands and pages, and a switch
/// for every plugin that can be turned on without being asked anything.
pub enum PaletteEntry {
    Command {
        plugin: String,
        command: String,
        title: String,
        key: Option<String>,
    },
    Page {
        plugin: String,
        page: String,
        title: String,
        icon: String,
    },
    Panel {
        plugin: String,
        panel: String,
        title: String,
        icon: String,
    },
    Switch {
        plugin: String,
        name: String,
        on: bool,
    },
}

pub fn palette_entries(cx: &App) -> Vec<PaletteEntry> {
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pl in &p.plugins {
        let m = &pl.manifest;
        let running = p.state.running(m);
        if running {
            for c in &m.commands {
                out.push(PaletteEntry::Command {
                    plugin: m.id.clone(),
                    command: c.id.clone(),
                    title: c.title.clone(),
                    key: crate::keys::plugin_key(&m.id, &c.id, c.key.as_deref())
                        .filter(|_| free_key(&m.id, &c.id, c.key.as_deref(), cx).is_some()),
                });
            }
            for page in &m.pages {
                out.push(PaletteEntry::Page {
                    plugin: m.id.clone(),
                    page: page.id.clone(),
                    title: page.title.clone(),
                    icon: page.icon.clone(),
                });
            }
            for panel in &m.panels {
                out.push(PaletteEntry::Panel {
                    plugin: m.id.clone(),
                    panel: panel.id.clone(),
                    title: panel.title.clone(),
                    icon: panel.icon.clone(),
                });
            }
        }
        // Turning one on from the palette allows nothing new: a plugin that asks for more than
        // it has been allowed is turned on in Settings, where what it asks is shown.
        if running || p.state.missing(m).is_empty() {
            out.push(PaletteEntry::Switch {
                plugin: m.id.clone(),
                name: m.name.clone(),
                on: running,
            });
        }
    }
    out
}

/// Change the user's choices with `f`, keep them, and take them at once.
pub fn update_state(ws: &Entity<Workspace>, cx: &mut App, f: impl FnOnce(&mut State)) {
    if !cx.has_global::<Plugins>() {
        return;
    }
    let p = cx.global_mut::<Plugins>();
    // Over the file as it is now, under the lock `trek plugin` takes too: a choice an agent made
    // from its shell since Trek last read the folder stays made.
    let mut f = Some(f);
    match State::update(&p.dir, |file| {
        check(file, &p.plugins);
        if let Some(f) = f.take() {
            f(file);
        }
    }) {
        Ok((now, ())) => p.state = now,
        Err(e) => {
            // Taken here for now, as before, even though the file couldn't keep it.
            if let Some(f) = f.take() {
                f(&mut p.state);
            }
            tracing::warn!("plugins: the choices couldn't be kept: {e}");
        }
    }
    // The state file is part of the folder's fingerprint: what was just written isn't news.
    p.fingerprint = core::fingerprint(&p.dir);
    p.index();
    contributions_changed(ws, cx);
}

/// What the user was shown when asked to allow a plugin: its files' hash then, and what it asked
/// for that it hadn't been given.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub hash: Option<String>,
    pub asks: Vec<core::Permission>,
}

impl Shown {
    /// What plugin `id` would ask the user to allow now.
    pub fn now(id: &str, cx: &App) -> Option<Shown> {
        let p = cx.try_global::<Plugins>()?;
        let pl = p.find(id)?;
        Some(Shown {
            hash: pl.hash.clone(),
            asks: p.state.missing(&pl.manifest),
        })
    }

    /// Whether allowing `now` is allowing what was shown: the same files, and nothing asked that
    /// wasn't on the card. A plugin rewritten while the dialog was open (an agent can write its
    /// folder) would otherwise be sealed with permissions the user never read.
    pub fn covers(&self, now: &Shown) -> bool {
        self.hash == now.hash && now.asks.iter().all(|p| self.asks.contains(p))
    }
}

/// Turn plugin `id` off, or on when that allows nothing new (the palette's switch).
pub fn set_on(id: &str, on: bool, ws: &Entity<Workspace>, cx: &mut App) {
    if on && Shown::now(id, cx).is_some_and(|now| !now.asks.is_empty()) {
        core::log(id, "not turned on: it asks for more than it was allowed");
        return;
    }
    turn(id, on, ws, cx);
}

/// Turn plugin `id` on, allowing what it asks for, as long as that's what the user was `shown`.
pub fn allow_shown(id: &str, shown: &Shown, ws: &Entity<Workspace>, cx: &mut App) {
    if !Shown::now(id, cx).is_some_and(|now| shown.covers(&now)) {
        core::log(id, "not turned on: it changed while it was being allowed");
        return;
    }
    turn(id, true, ws, cx);
}

fn turn(id: &str, on: bool, ws: &Entity<Workspace>, cx: &mut App) {
    let Some(plugin) = cx.try_global::<Plugins>().and_then(|p| p.find(id)).cloned() else {
        return;
    };
    // What it asks for is sealed with this install's key, for the plugin as it is now.
    let key = if on && !plugin.manifest.permissions.is_empty() {
        grant_key()
    } else {
        None
    };
    let mut allowed = true;
    let mut kept_look = None;
    update_state(ws, cx, |s| {
        if on {
            kept_look = s.plugins.get_mut(id).and_then(|c| c.look.take());
            allowed = s.turn_on_allowing(&plugin, key.as_ref());
        } else {
            s.turn_off(&plugin.manifest.id);
        }
    });
    if allowed && on && plugin.manifest.has_look() {
        // Turning a plugin on brings its look back: the one it had when another was chosen, or
        // its first.
        let themes = &plugin.manifest.themes;
        let look = kept_look
            .filter(|k| themes.iter().any(|t| &t.id == k))
            .or_else(|| themes.first().map(|t| t.id.clone()));
        if let Some(look) = look {
            crate::themes::switch_to(&look, ws, cx);
        }
    }
    if !allowed {
        let message = cant_allow(&plugin);
        ws.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message,
                undo: None,
            })
        });
        return;
    }
    core::log(
        id,
        if on {
            "turned on in Settings"
        } else {
            "turned off in Settings"
        },
    );
}

/// Why plugin `plugin`, which asks for permissions, couldn't be allowed: what the grant is
/// sealed over is the hash of its files, which a plugin past the file limit (or with a file
/// Trek can't read) doesn't have; otherwise the key couldn't be read.
pub fn cant_allow(plugin: &Plugin) -> String {
    let name = &plugin.manifest.name;
    if plugin.hash.is_none() {
        format!(
            "{name} can't be allowed: it has more than {} files, or one Trek can't read",
            core::MAX_FILES
        )
    } else {
        format!(
            "{name} can't be allowed: Trek couldn't read its key in {}",
            crate::words::words().your_credential_store
        )
    }
}

/// The user flipped plugin `id`'s switch. Off is at once, and so is on for a plugin that asks
/// for nothing it hasn't been given; one that does turns on only once the user allows what it
/// asks, read out in words in a dialog.
pub fn switch(id: &str, on: bool, ws: &Entity<Workspace>, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::{WindowExt as _, v_flex};
    let Some((name, shown)) = cx
        .try_global::<Plugins>()
        .and_then(|p| p.find(id).map(|pl| pl.manifest.name.clone()))
        .zip(Shown::now(id, cx))
    else {
        return;
    };
    if !on || shown.asks.is_empty() {
        set_on(id, on, ws, cx);
        return;
    }
    let asks = shown.asks.clone();
    let (id, ws) = (id.to_string(), ws.clone());
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let (id, ws) = (id.clone(), ws.clone());
        alert
            .title(format!("Turn on “{name}”?"))
            .description(
                v_flex()
                    .gap(px(6.))
                    .child(div().child("It may then:"))
                    .children(asks.iter().map(|p| permission_line(p, cx))),
            )
            .confirm()
            .ok_text("Allow and turn on")
            .on_ok({
                let shown = shown.clone();
                move |_, _, cx| {
                    allow_shown(&id, &shown, &ws, cx);
                    true
                }
            })
    });
}

/// What permission `p` allows, in words, with a mark for reading or acting.
pub fn permission_line(p: &core::Permission, cx: &App) -> AnyElement {
    use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
    let theme = cx.theme();
    h_flex()
        .gap(px(7.))
        .text_size(px(crate::scale::text::LABEL))
        .text_color(theme.muted_foreground)
        .child(
            Icon::new(if p.acts() {
                IconName::CircleUser
            } else {
                IconName::Eye
            })
            .xsmall()
            .text_color(theme.muted_foreground.opacity(0.8)),
        )
        .child(p.words())
        .into_any_element()
}

/// Stop plugin `id` for `why` (its logic failed, a frame of its couldn't be drawn): it's off
/// until the user turns it on again, a replacement of its gives the surface back, and the user is
/// told.
pub fn stop(id: &str, why: &str, ws: &Entity<Workspace>, cx: &mut App) {
    if !running(id, cx) {
        return;
    }
    core::log(id, &format!("stopped: {why}"));
    let name = cx
        .try_global::<Plugins>()
        .and_then(|p| p.find(id))
        .map(|p| p.manifest.name.clone())
        .unwrap_or_else(|| id.to_string());
    update_state(ws, cx, |s| s.stop(id, why));
    ws.update(cx, |_, cx| {
        cx.emit(WorkspaceEvent::Toast {
            message: format!(
                "{name} stopped: {}. It's off until you turn it on in Settings › Plugins.",
                why.lines().next().unwrap_or(why)
            ),
            undo: None,
        })
    });
}

/// Remove plugin `id`: its folder (or link), what it kept, and the user's choices for it.
pub fn remove(id: &str, ws: &Entity<Workspace>, cx: &mut App) -> Result<(), String> {
    let dir = cx
        .try_global::<Plugins>()
        .map(|p| p.dir.clone())
        .ok_or("no plugins")?;
    let mut out = Vec::new();
    let code = core::cli::run(&["remove".into(), id.to_string()], &dir, &mut out);
    if code != 0 {
        return Err(String::from_utf8_lossy(&out).trim().to_string());
    }
    reread(ws, cx);
    Ok(())
}

/// Install `src` (a folder, a git URL or `example:<name>`) off the UI thread, then read the folder.
pub fn install(src: String, ws: &Entity<Workspace>, cx: &mut App) {
    let Some(dir) = cx.try_global::<Plugins>().map(|p| p.dir.clone()) else {
        return;
    };
    let ws = ws.downgrade();
    cx.spawn(async move |cx| {
        let (code, text) = cx
            .background_spawn(async move {
                let mut out = Vec::new();
                let code = core::cli::run(&["install".into(), src], &dir, &mut out);
                (code, String::from_utf8_lossy(&out).into_owned())
            })
            .await;
        cx.update(|cx| {
            let Some(ws) = ws.upgrade() else { return };
            if code != 0 {
                ws.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message: text.trim().trim_start_matches("error: ").to_string(),
                        undo: None,
                    })
                });
            }
            reread_quiet(&ws, cx);
        });
    })
    .detach();
}

/// Settings' "Install from folder…": the user picks a plugin's folder, and it's copied in as
/// `trek plugin install <folder>` would.
pub fn install_from_folder(ws: &Entity<Workspace>, cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Install".into()),
    });
    let ws = ws.downgrade();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = rx.await else {
            return;
        };
        let Some(folder) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| {
            if let Some(ws) = ws.upgrade() {
                install(folder.to_string_lossy().into_owned(), &ws, cx);
            }
        });
    })
    .detach();
}

/// Read the folder again now, on the UI thread (after Settings changed it).
pub fn reread(ws: &Entity<Workspace>, cx: &mut App) {
    let Some(dir) = cx.try_global::<Plugins>().map(|p| p.dir.clone()) else {
        return;
    };
    let print = core::fingerprint(&dir);
    let scan = core::scan(&dir);
    let state = checked_state(&dir, &scan);
    apply_scan(scan, state, print, true, ws, cx);
}

/// The same, with nothing announced: what Settings just did itself (an install the user asked
/// for) needn't be told back to them.
fn reread_quiet(ws: &Entity<Workspace>, cx: &mut App) {
    let Some(dir) = cx.try_global::<Plugins>().map(|p| p.dir.clone()) else {
        return;
    };
    let print = core::fingerprint(&dir);
    let scan = core::scan(&dir);
    let state = checked_state(&dir, &scan);
    apply_scan(scan, state, print, false, ws, cx);
}

/// Merge `patch` (an object) into plugin `id`'s state, and draw what reads it.
pub fn set_state(id: &str, patch: Value, ws: &Entity<Workspace>, cx: &mut App) {
    if !cx.has_global::<Plugins>() {
        return;
    }
    let states = &mut cx.global_mut::<Plugins>().states;
    let state = states
        .entry(id.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if let (Value::Object(into), Value::Object(from)) = (state, patch) {
        for (k, v) in from {
            into.insert(k, v);
        }
    }
    ws.update(cx, |_, cx| cx.notify());
    cx.refresh_windows();
}

/// Run plugin `plugin`'s command `id`: its action, or its logic's function.
pub fn run_command(
    plugin: &str,
    id: &str,
    ws: &Entity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    if !running(plugin, cx) {
        return;
    }
    match render::command_action(plugin, id, cx) {
        Some(trek_core::plugins::CommandRun::Action(a)) => {
            let place = render::Place {
                thread: ws.read(cx).focused_thread().map(str::to_string),
                ..render::Place::of(ws)
            };
            render::run(plugin, &render::resolve_plain(&a), &place, window, cx)
        }
        Some(trek_core::plugins::CommandRun::Logic(f)) => {
            let thread = ws.read(cx).focused_thread().map(str::to_string);
            logic::call(
                plugin,
                trek_script::plugin::Handler::Command(f),
                serde_json::json!({ "command": id, "thread": thread }),
                ws,
                cx,
            );
        }
        None => {}
    }
}

/// Play plugin `plugin`'s sound `file` (`trek.sound`): a `.wav` inside its folder, as both
/// systems play one. Anything else is refused, in its log.
pub fn play_sound(plugin: &str, file: &str, cx: &App) {
    let Some(dir) = cx
        .try_global::<Plugins>()
        .and_then(|p| p.find(plugin))
        .map(|p| p.dir.clone())
    else {
        return;
    };
    let wav = std::path::Path::new(file)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    match core::page::resolve(&dir, file) {
        Some(path) if wav => {
            core::log(plugin, &format!("played {file}"));
            crate::system::play_sound_file(path);
        }
        Some(_) => core::log(
            plugin,
            &format!("refused trek.sound(\"{file}\"): a sound is a .wav file"),
        ),
        None => core::log(
            plugin,
            &format!("refused trek.sound(\"{file}\"): there's no such file in the plugin"),
        ),
    }
}

/// Open plugin `plugin`'s page `page` in the main pane.
pub fn open_page(plugin: &str, page: &str, ws: &Entity<Workspace>, cx: &mut App) {
    if page::page_of(plugin, page, cx).is_none() {
        return;
    }
    let route = crate::workspace::Route::Page {
        plugin: plugin.to_string(),
        page: page.to_string(),
    };
    ws.update(cx, |ws, cx| ws.navigate(route, cx));
}

/// Open plugin `plugin`'s panel `panel` beside the thread.
pub fn open_panel(plugin: &str, panel: &str, ws: &Entity<Workspace>, cx: &mut App) {
    if page::page_of(plugin, panel, cx).is_none() {
        return;
    }
    let (plugin, panel) = (plugin.to_string(), panel.to_string());
    ws.update(cx, |_, cx| {
        cx.emit(WorkspaceEvent::OpenPluginPanel { plugin, panel })
    });
}

/// Send `text` to thread `thread` as plugin `plugin` (which was given `threads:send`): to the
/// thread's agent, as if the user had typed it there. Said in the plugin's log.
pub fn send_to_thread(
    plugin: &str,
    thread: &str,
    text: &str,
    ws: &Entity<Workspace>,
    cx: &mut App,
) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("there's nothing to send".into());
    }
    if ws.read(cx).thread(thread).is_none() {
        return Err("there's no such thread".into());
    }
    core::log(
        plugin,
        &format!(
            "sent to thread {thread}: {}",
            text.chars().take(120).collect::<String>()
        ),
    );
    let (id, text) = (thread.to_string(), text.to_string());
    ws.update(cx, |ws, cx| ws.send_to(&id, text, Vec::new(), cx));
    Ok(())
}

/// The sidebar's Pages group: each running plugin's pages, a row apiece (none, and nothing drawn,
/// without them).
pub fn sidebar_pages(ws: &Entity<Workspace>, cx: &App) -> Vec<AnyElement> {
    use gpui_kit::component::Icon;
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    let pages: Vec<(String, trek_core::plugins::Page)> = p
        .plugins
        .iter()
        .filter(|pl| p.state.running(&pl.manifest))
        .flat_map(|pl| {
            pl.manifest
                .pages
                .iter()
                .map(|page| (pl.manifest.id.clone(), page.clone()))
        })
        .collect();
    if pages.is_empty() {
        return Vec::new();
    }
    let open = open_page_of(ws, cx);
    // Labelled as the sidebar labels its other groups.
    let mut out = vec![crate::sidebar::look::section_label("Pages", None, cx).into_any_element()];
    for (plugin, page) in pages {
        let selected = open.as_ref() == Some(&(plugin.clone(), page.id.clone()));
        let ws = ws.clone();
        let (pl, pg) = (plugin.clone(), page.id.clone());
        out.push(
            crate::ui::nav_row(
                SharedString::from(format!("page-{plugin}-{}", page.id)),
                Icon::empty().path(icon_path(&page.icon)),
                page.title.clone(),
                None,
                selected,
                cx,
            )
            .on_click(move |_, _, cx| open_page(&pl, &pg, &ws, cx))
            .into_any_element(),
        );
    }
    out
}

/// The page the main window shows, as (plugin, page), if it shows one.
pub fn open_page_of(ws: &Entity<Workspace>, cx: &App) -> Option<(String, String)> {
    match &ws.read(cx).route {
        crate::workspace::Route::Page { plugin, page } => Some((plugin.clone(), page.clone())),
        _ => None,
    }
}

/// Trek's own surface, for the replacement drawn in its place to embed (once: it's one element).
pub fn original(
    _c: trek_core::plugins::ui::Component,
    place: &render::Place,
    _cx: &App,
) -> AnyElement {
    place
        .original
        .borrow_mut()
        .take()
        .unwrap_or_else(|| div().into_any_element())
}

/// The asset path of Lucide icon `name` (`flame`), or the puzzle piece when Trek doesn't carry it:
/// an icon that isn't there would draw nothing and log on every frame.
pub fn icon_path(name: &str) -> SharedString {
    static HAVE: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    let have = HAVE.get_or_init(|| {
        use gpui_kit::AssetSource as _;
        crate::assets::Assets
            .list("icons/")
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.to_string())
            .collect()
    });
    let path = format!("icons/{name}.svg");
    if have.contains(&path) {
        path.into()
    } else {
        "icons/puzzle.svg".into()
    }
}

/// Show plugin `id`'s log in Settings (`None`: none), its lines read now.
pub fn show_log(id: Option<&str>, cx: &mut App) {
    if cx.has_global::<Plugins>() {
        cx.global_mut::<Plugins>().log = id.map(|id| (id.to_string(), core::log_tail(id, 12)));
    }
}

#[cfg(test)]
mod shown_tests {
    use super::Shown;
    use trek_core::plugins::Permission;

    #[test]
    fn allowing_covers_only_what_was_shown() {
        let perm = |k: &str| Permission::parse(k).unwrap();
        let shown = Shown {
            hash: Some("a".into()),
            asks: vec![perm("usage:read")],
        };
        assert!(shown.covers(&shown.clone()));
        // Less asked than shown (one was allowed meanwhile): fine.
        assert!(shown.covers(&Shown {
            hash: Some("a".into()),
            asks: vec![],
        }));
        // Rewritten while the dialog was open: more asked, or other files.
        assert!(!shown.covers(&Shown {
            hash: Some("a".into()),
            asks: vec![perm("usage:read"), perm("threads:send")],
        }));
        assert!(!shown.covers(&Shown {
            hash: Some("b".into()),
            asks: vec![perm("usage:read")],
        }));
    }
}
