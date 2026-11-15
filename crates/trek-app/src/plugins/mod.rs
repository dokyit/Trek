//! Trek plugins in the app (`trek_core::plugins` has the format, the folder and the CLI;
//! `docs/PLUGINS.md` the design): the plugins installed and which are running, the folder read
//! again whenever it changes, and the user's choices kept.
//!
//! What a running plugin brings is indexed here once, when the folder is read, so a frame asks a
//! map and draws: with no plugins every slot's list is empty, and nothing else is made.

pub mod data;
pub mod focus;
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
            .background_spawn(async move { (core::scan(&dir), core::State::read(&dir)) })
            .await;
        cx.update(|cx| {
            if let Some(ws) = ws.upgrade() {
                apply_scan(scan, state, print, &ws, cx);
            }
        });
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
        p.generation += 1;
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
                ws: ws.clone(),
                thread: ws.read(cx).focused_thread().map(str::to_string),
                project: None,
                threads: None,
            };
            render::run(plugin, &render::resolve_plain(&a), &place, window, cx)
        }
        Some(trek_core::plugins::CommandRun::Logic(f)) => {
            core::log(plugin, &format!("{f}(): plugins' logic isn't run yet"));
        }
        None => {}
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
    use gpui_kit::component::{ActiveTheme as _, Icon};
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
    let theme = cx.theme();
    let mut out = vec![
        div()
            .pt(px(10.))
            .pb(px(2.))
            .px(px(10.))
            .text_size(crate::scale::to_rems(crate::scale::text::META))
            .text_color(theme.muted_foreground)
            .child("Pages")
            .into_any_element(),
    ];
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

/// Trek's own surface for a replacement to embed.
pub fn original(
    _c: trek_core::plugins::ui::Component,
    _place: &render::Place,
    _cx: &App,
) -> AnyElement {
    div().into_any_element()
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
