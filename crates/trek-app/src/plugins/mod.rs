//! Trek plugins in the app (`trek_core::plugins` has the format, the folder and the CLI;
//! `docs/PLUGINS.md` the design): the plugins installed and which are running, the folder read
//! again whenever it changes, and the user's choices kept.
//!
//! What a running plugin brings is indexed here once, when the folder is read, so a frame asks a
//! map and draws: with no plugins every slot's list is empty, and nothing else is made.

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;
use trek_core::plugins::{self as core, Manifest, Plugin, Problem, Slot, State};

/// How often the folder is looked at.
const WATCH_EVERY: Duration = Duration::from_millis(1500);

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
    /// The plugin whose log Settings shows, and its last lines (read when it's opened or the
    /// folder changes, never while drawing).
    pub log: Option<(String, Vec<String>)>,
}

impl Global for Plugins {}

impl Plugins {
    fn index(&mut self) {
        self.slots.clear();
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

/// Whether plugin `id` is running.
pub fn running(id: &str, cx: &App) -> bool {
    cx.try_global::<Plugins>()
        .and_then(|p| p.find(id).map(|pl| p.state.running(&pl.manifest)))
        .unwrap_or(false)
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

/// Read the folder, index what's running, and watch it. Before any window opens.
pub fn init(ws: &Entity<Workspace>, cx: &mut App) {
    let dir = core::dir();
    let fingerprint = core::fingerprint(&dir);
    let scan = core::scan(&dir);
    let state = core::State::read(&dir);
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
        log: None,
    };
    plugins.index();
    cx.set_global(plugins);
    contributions_changed(ws, cx);
    watch(ws.downgrade(), cx);
}

fn watch(ws: WeakEntity<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(WATCH_EVERY).await;
            let Some(dir) = cx.update(|cx| cx.try_global::<Plugins>().map(|p| p.dir.clone()))
            else {
                break;
            };
            // Looked at, and read when it changed, off the UI thread.
            let read = cx
                .background_spawn({
                    let dir = dir.clone();
                    async move {
                        let print = core::fingerprint(&dir);
                        let asked = core::take_reload(&dir);
                        (print, asked)
                    }
                })
                .await;
            let (print, asked) = read;
            let due = cx.update(|cx| {
                cx.try_global::<Plugins>()
                    .is_some_and(|p| p.fingerprint != print)
                    || asked
            });
            if due {
                let scan = cx
                    .background_spawn({
                        let dir = dir.clone();
                        async move { (core::scan(&dir), core::State::read(&dir)) }
                    })
                    .await;
                let gone = cx.update(|cx| match ws.upgrade() {
                    Some(ws) => {
                        apply_scan(scan.0, scan.1, print, &ws, cx);
                        false
                    }
                    None => true,
                });
                if gone {
                    break;
                }
            } else if ws.upgrade().is_none() {
                break;
            }
        }
    })
    .detach();
}

/// Take a read of the folder: what's new is on offer, a bad plugin is said once, and what the
/// running plugins bring is indexed again.
fn apply_scan(
    scan: core::Scan,
    state: State,
    print: core::Fingerprint,
    ws: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(p) = cx.try_global::<Plugins>() else {
        return;
    };
    let dir = p.dir.clone();
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
            state.choice(&pl.manifest.id).waiting && !p.state.choice(&pl.manifest.id).waiting
        })
        .map(|pl| pl.manifest.name.clone())
        .collect();
    core::write_problems(&dir, &scan.problems);
    {
        let p = cx.global_mut::<Plugins>();
        p.plugins = scan.plugins;
        p.problems = scan.problems;
        p.state = state;
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
    contributions_changed(ws, cx);
}

/// The running plugins' contributions changed: the themes they bring, and every view.
fn contributions_changed(ws: &Entity<Workspace>, cx: &mut App) {
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
    ws.update(cx, |_, cx| cx.notify());
    cx.refresh_windows();
}

/// Change the user's choices with `f`, keep them, and take them at once.
pub fn update_state(ws: &Entity<Workspace>, cx: &mut App, f: impl FnOnce(&mut State)) {
    if !cx.has_global::<Plugins>() {
        return;
    }
    let p = cx.global_mut::<Plugins>();
    f(&mut p.state);
    if let Err(e) = p.state.write(&p.dir) {
        tracing::warn!("plugins: the choices couldn't be kept: {e}");
    }
    // The state file is part of the folder's fingerprint: what was just written isn't news.
    p.fingerprint = core::fingerprint(&p.dir);
    p.index();
    contributions_changed(ws, cx);
}

/// Turn plugin `id` on (the user, in Settings: they've seen and allow what it asks for) or off.
pub fn set_on(id: &str, on: bool, ws: &Entity<Workspace>, cx: &mut App) {
    let Some(m) = cx
        .try_global::<Plugins>()
        .and_then(|p| p.find(id))
        .map(|p| p.manifest.clone())
    else {
        return;
    };
    update_state(ws, cx, |s| {
        if on {
            s.turn_on_allowing(&m);
        } else {
            s.turn_off(&m.id);
        }
    });
    core::log(
        id,
        if on {
            "turned on in Settings"
        } else {
            "turned off in Settings"
        },
    );
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
            reread(&ws, cx);
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
    apply_scan(core::scan(&dir), State::read(&dir), print, ws, cx);
}

/// Show plugin `id`'s log in Settings (`None`: none), its lines read now.
pub fn show_log(id: Option<&str>, cx: &mut App) {
    if cx.has_global::<Plugins>() {
        cx.global_mut::<Plugins>().log = id.map(|id| (id.to_string(), core::log_tail(id, 12)));
    }
}
