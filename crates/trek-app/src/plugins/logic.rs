//! Plugins' logic and hooks: a plugin's `main.js` called when something it listens for happens
//! (`hooks` in its manifest) or one of its commands is run, in QuickJS on a background thread
//! (`trek_script::plugin`), one call at a time. A call gets the plugin's state and what its
//! permissions let it read, and hands back its state and what it wants done (a toast, a thread
//! opened, a message sent), which Trek does or refuses by its permissions.
//!
//! Nothing here exists until a plugin with logic is running: the host is made at the first call,
//! and Trek's events are only watched while some running plugin names a hook. A call that
//! throws, runs past its time or its memory stops that plugin (`super::stop`).

use super::Plugins;
use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;
use trek_core::RunState;
use trek_core::plugins::Permission;
use trek_script::plugin::{Effect, Handler, PluginCall, PluginLimits, PluginRun};

/// How long Trek's changes settle before they're read for events: a turn streaming notifies many
/// times a second, and the threads are looked over once for all of it.
const SETTLE: Duration = Duration::from_millis(250);
/// The most threads a call is handed.
const MAX_THREADS: usize = 500;

struct Job {
    plugin: String,
    handler: Handler,
    arg: Value,
}

/// The logic host: the queue of calls, and what Trek's events are told from.
pub struct Logic {
    tx: async_channel::Sender<Job>,
    /// Each thread's state as last seen: a change is an event.
    seen: HashMap<String, RunState>,
    theme: (String, bool),
    /// A look at the threads is due (`SETTLE` after the first change).
    check: Option<Task<()>>,
    /// While some running plugin has hooks: the workspace watched for what they hear.
    watching: Vec<Subscription>,
}

impl Global for Logic {}

fn ensure(ws: &Entity<Workspace>, cx: &mut App) {
    if cx.has_global::<Logic>() {
        return;
    }
    let (tx, rx) = async_channel::unbounded::<Job>();
    let seen = ws
        .read(cx)
        .threads
        .iter()
        .map(|t| (t.id.clone(), t.run_state))
        .collect();
    cx.set_global(Logic {
        tx,
        seen,
        theme: (crate::themes::shown_id(cx), cx.theme_is_dark()),
        check: None,
        watching: Vec::new(),
    });
    let ws = ws.downgrade();
    cx.spawn(async move |cx| {
        while let Ok(job) = rx.recv().await {
            let Some(ws) = ws.upgrade() else { break };
            // What the call is given, read on the UI thread; then the call, off it.
            let Some((main, call_state, data, sealed)) =
                cx.update(|cx| inputs(&job.plugin, &ws, cx))
            else {
                continue;
            };
            let plugin = job.plugin.clone();
            let run = cx
                .background_spawn(async move {
                    let source = match std::fs::read_to_string(&main) {
                        Ok(s) => s,
                        Err(e) => {
                            return PluginRun {
                                error: Some(format!("main.js couldn't be read: {e}")),
                                ..Default::default()
                            };
                        }
                    };
                    // A plugin with permissions runs only as the user allowed it: its files read
                    // after the source must hash as they did then. One edited since waits for the
                    // folder's next read, which asks the user again.
                    if let Some((dir, hash)) = &sealed
                        && trek_core::plugins::grant::content_hash(dir).as_ref() != Some(hash)
                    {
                        trek_core::plugins::log(
                            &plugin,
                            "not run: its files changed since it was allowed",
                        );
                        return PluginRun::default();
                    }
                    let call = PluginCall {
                        source,
                        handler: job.handler,
                        arg: job.arg,
                        state: call_state,
                        storage: Value::Object(trek_core::plugins::storage(&plugin)),
                        data,
                    };
                    let run = trek_script::plugin::run_plugin(&call, PluginLimits::default());
                    // What it printed and what it keeps are written here, off the UI thread.
                    for line in &run.logs {
                        trek_core::plugins::log(&plugin, line);
                    }
                    for e in &run.effects {
                        if let Effect::Storage(Value::Object(value)) = e
                            && let Err(why) = trek_core::plugins::set_storage(&plugin, value)
                        {
                            trek_core::plugins::log(&plugin, &format!("storage not kept: {why}"));
                        }
                    }
                    run
                })
                .await;
            cx.update(|cx| apply(&job.plugin, run, &ws, cx));
        }
    })
    .detach();
}

trait ThemeDark {
    fn theme_is_dark(&self) -> bool;
}

impl ThemeDark for App {
    fn theme_is_dark(&self) -> bool {
        use gpui_kit::component::ActiveTheme as _;
        self.theme().mode.is_dark()
    }
}

/// The folder and file hash a call of a plugin's must still find, when it has permissions.
type Sealed = Option<(std::path::PathBuf, String)>;

/// What a call of `plugin`'s logic is given: its `main.js`, its state, what it may read, and
/// (when it has permissions) the hash its grant was sealed over.
fn inputs(
    plugin: &str,
    ws: &Entity<Workspace>,
    cx: &App,
) -> Option<(std::path::PathBuf, Value, Value, Sealed)> {
    let p = cx.try_global::<Plugins>()?;
    let pl = p.find(plugin)?;
    if !p.state.running(&pl.manifest) {
        return None;
    }
    let main = pl.dir.join(pl.manifest.main.as_ref()?);
    let state = p.states.get(plugin).cloned().unwrap_or_else(|| json!({}));
    let read = pl.manifest.wants(&Permission::ThreadsRead);
    let usage = pl.manifest.wants(&Permission::UsageRead);
    let w = ws.read(cx);
    let mut data = serde_json::Map::new();
    if read {
        data.insert(
            "threads".into(),
            Value::Array(
                w.threads
                    .iter()
                    .filter(|t| t.archived_at.is_none() && t.side_of.is_none())
                    .take(MAX_THREADS)
                    .map(|t| super::render::thread_value(t, w, true, usage))
                    .collect(),
            ),
        );
    }
    data.insert(
        "theme".into(),
        json!({ "dark": cx.theme_is_dark(), "id": crate::themes::shown_id(cx) }),
    );
    let sealed = if pl.manifest.permissions.is_empty() {
        None
    } else {
        // Running with permissions means sealed over this hash (`State::verify`).
        Some((pl.dir.clone(), pl.hash.clone()?))
    };
    Some((main, state, Value::Object(data), sealed))
}

/// Take what a call left: its state, what it asked for (by its permissions), or its failure.
fn apply(plugin: &str, run: PluginRun, ws: &Entity<Workspace>, cx: &mut App) {
    if let Some(why) = run.error {
        super::stop(plugin, &why, ws, cx);
        return;
    }
    if let Some(state) = run.state {
        super::set_state(plugin, state, ws, cx);
    }
    let can = |perm: Permission, cx: &App| {
        cx.try_global::<Plugins>()
            .and_then(|p| p.find(plugin))
            .is_some_and(|p| p.manifest.wants(&perm))
    };
    let refuse = |what: &str, perm: Permission| {
        trek_core::plugins::log(
            plugin,
            &format!("refused {what}: it needs the {} permission", perm.key()),
        );
    };
    for effect in run.effects {
        match effect {
            Effect::Toast(text) => {
                let message: String = text.chars().take(300).collect();
                ws.update(cx, |_, cx| {
                    cx.emit(WorkspaceEvent::Toast {
                        message,
                        undo: None,
                    })
                });
            }
            Effect::Open(thread) => {
                if !can(Permission::ThreadsOpen, cx) {
                    refuse("trek.open", Permission::ThreadsOpen);
                } else if ws.read(cx).thread(&thread).is_some() {
                    ws.update(cx, |ws, cx| {
                        ws.navigate(crate::workspace::Route::Thread(thread), cx)
                    });
                }
            }
            Effect::NewThread => {
                if can(Permission::ThreadsOpen, cx) {
                    ws.update(cx, |ws, cx| {
                        ws.navigate(crate::workspace::Route::Draft { project: None }, cx)
                    });
                } else {
                    refuse("trek.newThread", Permission::ThreadsOpen);
                }
            }
            Effect::Send { thread, text } => {
                if !can(Permission::ThreadsSend, cx) {
                    refuse("trek.send", Permission::ThreadsSend);
                } else if let Err(why) = super::send_to_thread(plugin, &thread, &text, ws, cx) {
                    trek_core::plugins::log(plugin, &format!("trek.send: {why}"));
                }
            }
            Effect::Page(page) => super::open_page(plugin, &page, ws, cx),
            Effect::Sound(file) => super::play_sound(plugin, &file, cx),
            // Kept already, off the UI thread.
            Effect::Storage(_) => {}
        }
    }
}

/// Call `plugin`'s logic: `handler` with `arg`. Queued behind any call under way.
pub fn call(plugin: &str, handler: Handler, arg: Value, ws: &Entity<Workspace>, cx: &mut App) {
    ensure(ws, cx);
    let _ = cx.global::<Logic>().tx.try_send(Job {
        plugin: plugin.to_string(),
        handler,
        arg,
    });
}

/// The running plugins that hear `event`.
fn hearing(event: &str, cx: &App) -> Vec<String> {
    let Some(p) = cx.try_global::<Plugins>() else {
        return Vec::new();
    };
    p.plugins
        .iter()
        .filter(|pl| p.state.running(&pl.manifest) && pl.manifest.hooks.iter().any(|h| h == event))
        .map(|pl| pl.manifest.id.clone())
        .collect()
}

/// Tell every running plugin that hears `event`, about `thread` when it's a thread's.
fn tell(event: &str, thread: Option<&str>, ws: &Entity<Workspace>, cx: &mut App) {
    for plugin in hearing(event, cx) {
        let arg = {
            let w = ws.read(cx);
            let (read, usage) = cx
                .try_global::<Plugins>()
                .and_then(|p| p.find(&plugin))
                .map(|p| {
                    (
                        p.manifest.wants(&Permission::ThreadsRead),
                        p.manifest.wants(&Permission::UsageRead),
                    )
                })
                .unwrap_or_default();
            match thread.and_then(|id| w.thread(id)) {
                Some(t) => json!({
                    "event": event,
                    "thread": super::render::thread_value(t, w, read, usage),
                }),
                None => json!({
                    "event": event,
                    "theme": { "dark": cx.theme_is_dark(), "id": crate::themes::shown_id(cx) },
                }),
            }
        };
        call(&plugin, Handler::Event(event.to_string()), arg, ws, cx);
    }
}

/// The running plugins changed: Trek's events are watched while any of them has hooks, and not
/// otherwise. A plugin that hears `app.start` is told once it runs.
pub fn hooks_changed(ws: &Entity<Workspace>, cx: &mut App) {
    let any = cx.try_global::<Plugins>().is_some_and(|p| {
        p.plugins
            .iter()
            .any(|pl| p.state.running(&pl.manifest) && !pl.manifest.hooks.is_empty())
    });
    if !any {
        if cx.has_global::<Logic>() {
            let l = cx.global_mut::<Logic>();
            l.watching.clear();
            l.check = None;
        }
        return;
    }
    ensure(ws, cx);
    if !cx.global::<Logic>().watching.is_empty() {
        return;
    }
    let weak = ws.downgrade();
    let observed = cx.observe(ws, move |_, cx| {
        if cx.global::<Logic>().check.is_some() {
            return;
        }
        let weak = weak.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(SETTLE).await;
            cx.update(|cx| {
                cx.global_mut::<Logic>().check = None;
                if let Some(ws) = weak.upgrade() {
                    look(&ws, cx);
                }
            });
        });
        cx.global_mut::<Logic>().check = Some(task);
    });
    let heard = cx.subscribe(ws, |ws, event: &WorkspaceEvent, cx| {
        if let WorkspaceEvent::TurnChanges { id, .. } = event {
            tell("file.changed", Some(id), &ws, cx);
        }
    });
    cx.global_mut::<Logic>().watching = vec![observed, heard];
    tell("app.start", None, ws, cx);
}

/// What changed since the last look: threads that started or ended a turn or came to need the
/// user, and the theme.
fn look(ws: &Entity<Workspace>, cx: &mut App) {
    let now: Vec<(String, RunState)> = ws
        .read(cx)
        .threads
        .iter()
        .map(|t| (t.id.clone(), t.run_state))
        .collect();
    let mut events: Vec<(&'static str, String)> = Vec::new();
    {
        let l = cx.global_mut::<Logic>();
        for (id, state) in &now {
            // A thread not seen before was idle until now.
            let before = l.seen.insert(id.clone(), *state).unwrap_or(RunState::Idle);
            if before == *state {
                continue;
            }
            match (before, state) {
                // Working again after an approval is the same turn.
                (RunState::Idle | RunState::Failed, RunState::Working) => {
                    events.push(("turn.start", id.clone()))
                }
                (_, RunState::NeedsYou) => events.push(("thread.needs-you", id.clone())),
                (RunState::Working | RunState::NeedsYou, RunState::Idle | RunState::Failed) => {
                    events.push(("turn.end", id.clone()))
                }
                _ => {}
            }
        }
        l.seen.retain(|id, _| now.iter().any(|(n, _)| n == id));
    }
    for (event, id) in events {
        tell(event, Some(&id), ws, cx);
    }
    let theme = (crate::themes::shown_id(cx), cx.theme_is_dark());
    if cx.global::<Logic>().theme != theme {
        cx.global_mut::<Logic>().theme = theme;
        tell("theme.changed", None, ws, cx);
    }
}
