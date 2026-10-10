//! Design review without Screen Recording: a build with the `shots` feature watches
//! `$TREK_SHOT_DIR/cmd` and runs the commands it finds there, one per line, against the main
//! window, then deletes the file. `shot <name>` draws the window's current frame to
//! `<name>.png` in that folder; see-through areas (liquid glass) are laid over `$TREK_SHOT_UNDER`,
//! blurred, as macOS would show the desktop. Run it with `TREK_FORCE_ACTIVE=1` so a window kept
//! behind others still draws. When `TREK_SHOT_DIR` is set the process is isolated (see
//! `trek_core::paths::isolate`): the Keychain stays untouched and only the mock agent runs, so a
//! manifest can never reach a real agent or a real account.
//!
//! A batch ends by writing `done` in the same folder: `ok`, or one `err <line>: <message>` per
//! command that failed. Drivers must remove a stale `done` before writing `cmd`, and write `cmd`
//! atomically (a temp file renamed into place) so a half-written batch is never run.
//!
//! Commands: `route draft|no-project|basecamp|notes|appearance|settings:<page>|thread:<id>|title:<q>|project:<path>|first`,
//! `send <prompt>`, `attach <image>` (into the main composer's outbox), `project <folder>` (added, and a draft in it), `diff` (the latest turn's changes in the Git tool),
//! `pair` (a pairing code, its link written to `pair.txt`), `push on|test|alert`, `new` (⌘N), `settled [on|off]`,
//! `glass on|off`, `tint <0.2–0.95>`, `theme night|paper`, `tools on|off|git|explorer|terminal|browser|sidechat|simulator`,
//! `range today|week|all` (Basecamp's), `usage-demo` (plans and limits for the Usage card, no real agent asked), `pace <f>` (the mock agent's speed: 1.0 demo, 0 instant),
//! `wait <ms>` or `wait idle|permission|question|plan [cap ms]`, `record <name> <ms> [fps]` (frames
//! to `<name>.frames/` plus `<name>.ffconcat` for ffmpeg's concat demuxer; `record stop`, `record wait`),
//! `editor on|off|root|view|panel|chat|ai|open …` (Trek IDE; see `run`), `agent-install <id> <percent>|fail <message>|clear`
//! (an ACP Registry install's row, without downloading), `toast [undo|error] <message>` (a toast, with
//! an Undo or the error icon), `shot <name>`, `quit`.
//!
//! The Browser tool (open it with `tools browser`; its page is a native view, so `shot` doesn't
//! show it): `browser go <address>|back|forward|reload`, `browser state` (the page, the native
//! view's rectangle beside the panel's and which window has the keys, to `browser.json`), and
//! `browser snap <name>` (the page as its screenshot button captures it, to `<name>.png`; Windows).
//!
//! Input without a pointer or a keyboard (events dispatched to the window, never real OS input):
//! `click|rclick|hover <element id>` (`name#3` for a row's id; `elements` writes the ids on screen
//! to `elements.txt`), `type <text>` (into the focused field, else the main composer),
//! `key <keystroke>…` (`secondary-k`: ⌘K on a Mac, Ctrl+K on Windows; `escape`, `shift-tab`,
//! `down down enter`; on Windows `alt` alone takes the keyboard for the menu bar and `alt-f` opens
//! the menu its F marks, then `down`, `enter` and `escape` work it; `cmd`/`win`/`super` modifiers
//! are read as `secondary`, so one manifest is right on both platforms), `scroll <element id> <px>`
//! (a wheel over that element, positive down the page), `resize <w> <h>` (the main window, logical
//! px). The thread on screen: `approve` / `deny` its waiting permission, plan
//! or question card, `answer <n>|<text>` (option n of each question, or a typed answer), and
//! `rewind [n] [keep]` (undo the n-th turn from the end, 1 = the latest, as its Undo does; `keep`
//! leaves the files).
//! Lines starting with `#` are comments.

use crate::workspace::{PanelTool, Route, SettingsPage, Workspace};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let Some(dir) = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    // Weak between batches: this loop outlives the app's own wind-down, and a handle it held
    // would be one left when GPUI drops its entities (the leak detector panics on that).
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let mut recording: Option<Recording> = None;
        loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let cmd = dir.join("cmd");
            let Ok(text) = std::fs::read_to_string(&cmd) else { continue };
            let _ = std::fs::remove_file(&cmd);
            let Some(workspace) = workspace.upgrade() else { break };
            let mut errors = Vec::new();
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
                let result = match verb {
                    "wait" => wait(&workspace, arg, cx).await,
                    "shot" => shot(&workspace, &dir, arg, cx).await,
                    "browser" => browser(&workspace, &dir, arg, cx).await,
                    "record" => record(&workspace, &dir, arg, &mut recording, cx).await,
                    "quit" => quit(cx),
                    _ => cx.update(|cx| run(&workspace, verb, arg, cx)),
                };
                if let Err(e) = result {
                    tracing::warn!("shot command failed: {line}: {e:#}");
                    errors.push(format!("err {line}: {e:#}"));
                }
            }
            let _ = std::fs::write(dir.join("done"), if errors.is_empty() { "ok".to_string() } else { errors.join("\n") });
        }
    })
    .detach();
}

/// A frame capture running beside the command loop: `record` starts one, `record stop` asks it to
/// finish early and `record wait` joins it (the ffconcat list is written when it ends either way).
struct Recording {
    stop: Arc<AtomicBool>,
    task: Task<anyhow::Result<u32>>,
}

/// `wait <ms>`, or `wait <condition> [cap ms]` — `idle` until no thread has a turn running,
/// `permission`/`question`/`plan` until such a request is showing (for cards on film).
async fn wait(ws: &Entity<Workspace>, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let mut it = arg.split_whitespace();
    let Some(kind) = it.next() else { anyhow::bail!("wait needs <ms> or idle|permission|question|plan [cap ms]") };
    if let Ok(ms) = kind.parse::<u64>() {
        cx.background_executor().timer(Duration::from_millis(ms)).await;
        return Ok(());
    }
    if !matches!(kind, "idle" | "permission" | "question" | "plan") {
        anyhow::bail!("wait: unknown condition {kind:?} (idle|permission|question|plan or milliseconds)");
    }
    let cap = match it.next() {
        Some(v) => v.parse::<u64>().map_err(|_| anyhow::anyhow!("wait {kind}: bad cap {v:?}"))?,
        None => 60_000,
    };
    let deadline = Instant::now() + Duration::from_millis(cap);
    // A turn needs a beat to register as started after `send`; without it `wait idle` could pass
    // before the session had a chance to begin.
    cx.background_executor().timer(Duration::from_millis(250)).await;
    loop {
        let hit = cx.update(|cx| {
            let ws = ws.read(cx);
            match kind {
                "idle" => ws.live.values().all(|l| l.turn_started.is_none()),
                "permission" => ws.live.values().any(|l| l.permissions.iter().any(|p| p.prompt.is_none())),
                "question" => ws.live.values().any(|l| l.permissions.iter().any(|p| p.prompt.as_ref().is_some_and(|p| matches!(p, trek_agents::Prompt::Questions(_))))),
                _ => ws.live.values().any(|l| l.permissions.iter().any(|p| p.prompt.as_ref().is_some_and(|p| matches!(p, trek_agents::Prompt::Plan(_))))),
            }
        });
        if hit {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("wait {kind}: still not met after {cap} ms");
        }
        cx.background_executor().timer(Duration::from_millis(150)).await;
    }
}

/// Set once a manifest has put the pointer somewhere (`click`, `hover`, `scroll` …): from then on
/// it's where it was put.
static POINTER_SCRIPTED: AtomicBool = AtomicBool::new(false);

/// Until then, a frame is drawn with the pointer outside the window. The window isn't in front, but
/// a real mouse resting over it still hovers what's under it (p08 of the parity set caught a
/// Basecamp bar's tooltip that way), and a capture shouldn't depend on where the mouse is.
fn park_pointer(window: &mut Window, cx: &mut App) {
    if !POINTER_SCRIPTED.load(Ordering::Relaxed) {
        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: point(px(-100.), px(-100.)), pressed_button: None, modifiers: Default::default() }), cx);
    }
}

async fn shot(ws: &Entity<Workspace>, dir: &Path, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    if arg.is_empty() {
        anyhow::bail!("shot needs a name");
    }
    // A few frames for whatever the last command changed to be drawn.
    cx.background_executor().timer(Duration::from_millis(400)).await;
    // And for the git reads the command set going (the composer's branch chip): a process spawn
    // costs a hundred milliseconds on Windows, so they can land after those few frames.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut waited = false;
    while cx.update(|cx| ws.read(cx).git_inflight) > 0 && Instant::now() < deadline {
        cx.background_executor().timer(Duration::from_millis(50)).await;
        waited = true;
    }
    if waited {
        cx.background_executor().timer(Duration::from_millis(200)).await;
    }
    let path = dir.join(format!("{arg}.png"));
    let image = cx.update(|cx| -> anyhow::Result<image::RgbaImage> {
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |_, window, cx| {
            park_pointer(window, cx);
            window.render_to_image()
        })?
    })?;
    let path2 = path.clone();
    cx.background_executor()
        .spawn(async move {
            if let Some(parent) = path2.parent() {
                std::fs::create_dir_all(parent)?;
            }
            save(image, &path2)
        })
        .await?;
    tracing::info!("shot: {}", path.display());
    Ok(())
}

async fn browser(ws: &Entity<Workspace>, dir: &Path, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let (cmd, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    let main = cx.update(|cx| ws.read(cx).main_window).ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let panel = main.update(cx, |root, _, cx| {
        let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
        view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()).and_then(|trek| trek.read(cx).right_panel.read(cx).browser())
    })?;
    let panel = panel.ok_or_else(|| anyhow::anyhow!("browser: the Browser tool isn't open (tools browser)"))?;
    match cmd {
        "state" => {
            // A beat for the last command's layout to reach the native view.
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let state = main.update(cx, |_, window, cx| panel.read(cx).shots_state(window, cx))?;
            std::fs::write(dir.join("browser.json"), serde_json::to_string_pretty(&state)?)?;
            Ok(())
        }
        "snap" => {
            anyhow::ensure!(!rest.is_empty(), "browser snap needs a name");
            #[cfg(windows)]
            {
                let png = cx.update(|cx| panel.read(cx).shots_capture(cx)).ok_or_else(|| anyhow::anyhow!("browser snap: no page"))?;
                let bytes = png.recv().await.map_err(|_| anyhow::anyhow!("browser snap: the page closed"))?.map_err(|e| anyhow::anyhow!("browser snap: {e}"))?;
                std::fs::write(dir.join(format!("{rest}.png")), bytes)?;
                Ok(())
            }
            #[cfg(not(windows))]
            anyhow::bail!("browser snap: Windows only (WebView2's CapturePreview)")
        }
        _ => {
            cx.update(|cx| panel.update(cx, |p, cx| p.shots(cmd, rest, cx)))?;
            cx.update(|cx| cx.refresh_windows());
            Ok(())
        }
    }
}

async fn record(ws: &Entity<Workspace>, dir: &Path, arg: &str, slot: &mut Option<Recording>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let mut it = arg.split_whitespace();
    match it.next() {
        Some("wait") => match slot.take() {
            Some(rec) => {
                let frames = rec.task.await?;
                tracing::info!("record: {frames} frames");
                Ok(())
            }
            None => anyhow::bail!("record wait: nothing is recording"),
        },
        Some("stop") => match slot.take() {
            Some(rec) => {
                rec.stop.store(true, Ordering::Relaxed);
                let frames = rec.task.await?;
                tracing::info!("record: {frames} frames (stopped early)");
                Ok(())
            }
            None => anyhow::bail!("record stop: nothing is recording"),
        },
        Some(name) => {
            if slot.is_some() {
                anyhow::bail!("record {name}: already recording (record wait or record stop first)");
            }
            let ms = it
                .next()
                .map(str::parse::<u64>)
                .transpose()
                .map_err(|_| anyhow::anyhow!("record {name}: bad duration"))?
                .ok_or_else(|| anyhow::anyhow!("record {name}: needs <ms>"))?;
            let fps = match it.next() {
                Some(v) => v.parse::<u32>().map_err(|_| anyhow::anyhow!("record {name}: bad fps {v:?}"))?,
                None => 15,
            };
            anyhow::ensure!(fps > 0 && fps <= 60, "record {name}: fps {fps} out of range 1–60");
            *slot = Some(start_recording(ws.clone(), dir.to_path_buf(), name.to_string(), ms, fps, cx));
            Ok(())
        }
        None => anyhow::bail!("record needs <name> <ms> [fps] | stop | wait"),
    }
}

/// Render a frame every `1000/fps` ms for `ms` ms, writing `f%05d.png` files under
/// `<name>.frames/`. The real capture instant goes into `<name>.ffconcat` as each frame's
/// duration, so a frame that took long still plays for as long as it was on screen — dropped
/// frames don't speed the clip up. Runs beside the command loop: the `send`/`route`/`wait` lines
/// after `record` are what gets filmed.
fn start_recording(ws: Entity<Workspace>, dir: PathBuf, name: String, ms: u64, fps: u32, cx: &mut AsyncApp) -> Recording {
    let frames_dir = dir.join(format!("{name}.frames"));
    let concat_path = dir.join(format!("{name}.ffconcat"));
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let task = cx.spawn(async move |cx| {
        std::fs::create_dir_all(&frames_dir)?;
        let started = Instant::now();
        let interval = Duration::from_secs_f64(1. / fps as f64);
        let mut times = Vec::new();
        let mut index: u32 = 0;
        while started.elapsed() < Duration::from_millis(ms) && !flag.load(Ordering::Relaxed) {
            let image = cx.update(|cx| -> anyhow::Result<image::RgbaImage> {
                let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
                main.update(cx, |_, window, cx| {
                    park_pointer(window, cx);
                    window.render_to_image()
                })?
            })?;
            times.push(started.elapsed().as_secs_f64());
            let path = frames_dir.join(format!("f{index:05}.png"));
            index += 1;
            cx.background_executor().spawn(async move { save(image, &path) }).await?;
            let spent = started.elapsed().as_secs_f64() - times.last().copied().unwrap_or(0.);
            let left = interval.as_secs_f64() - spent;
            if left > 0. {
                cx.background_executor().timer(Duration::from_secs_f64(left)).await;
            }
        }
        if let Some(parent) = concat_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut concat = String::from("ffconcat version 1.0\n");
        for (i, &t) in times.iter().enumerate() {
            let till = times.get(i + 1).copied().unwrap_or(t + interval.as_secs_f64());
            concat.push_str(&format!("file '{name}.frames/f{i:05}.png'\nduration {:.4}\n", till - t));
        }
        std::fs::write(&concat_path, concat)?;
        Ok(times.len() as u32)
    });
    Recording { stop, task }
}

fn quit(cx: &mut AsyncApp) -> anyhow::Result<()> {
    // As the Quit menu item does.
    cx.update(crate::root::quit);
    Ok(())
}

fn run(ws: &Entity<Workspace>, verb: &str, arg: &str, cx: &mut App) -> anyhow::Result<()> {
    if verb == "usage-demo" {
        usage_demo(ws, cx);
        return Ok(());
    }
    if verb == "toast" {
        anyhow::ensure!(!arg.is_empty(), "toast [undo|error] <message>");
        let (kind, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        match kind {
            "undo" => ws.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: rest.into(), undo: Some(crate::workspace::UndoAction::Unsettle(String::new())) })),
            "error" => {
                let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
                main.update(cx, |_, window, cx| crate::toast::push(window, crate::toast::Toast::error(rest.to_string()), cx))?;
            }
            _ => ws.update(cx, |_, cx| cx.emit(crate::workspace::WorkspaceEvent::Toast { message: arg.into(), undo: None })),
        }
        return Ok(());
    }
    if verb == "range" {
        use trek_core::basecamp::Range;
        let range = match arg {
            "today" => Range::Today,
            "week" => Range::Week,
            "all" => Range::All,
            _ => anyhow::bail!("range: unknown range {arg:?} (today|week|all)"),
        };
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, _, cx| {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            if let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) {
                let basecamp = trek.read(cx).basecamp.clone();
                basecamp.update(cx, |b, cx| b.set_range(range, cx));
            }
        })?;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "tools" {
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("tools: no Trek window");
            };
            let panel = trek.read(cx).right_panel.clone();
            match arg {
                "on" => panel.update(cx, |p, cx| {
                    if !p.open {
                        p.toggle(cx);
                    }
                }),
                "off" => panel.update(cx, |p, cx| {
                    if p.open {
                        p.toggle(cx);
                    }
                }),
                "git" | "explorer" | "terminal" | "browser" | "sidechat" | "simulator" => {
                    let tool = match arg {
                        "git" => PanelTool::Git,
                        "explorer" => PanelTool::Explorer,
                        "terminal" => PanelTool::Terminal,
                        "browser" => PanelTool::Browser,
                        "sidechat" => PanelTool::SideChat,
                        _ => PanelTool::Simulator,
                    };
                    panel.update(cx, |p, cx| p.open_tool(tool, window, cx));
                }
                _ => anyhow::bail!("tools: unknown argument {arg:?} (on|off|git|explorer|terminal|browser|sidechat|simulator)"),
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "dictate" || verb == "dictate-file" {
        // Voice input without a pointer: `dictate` runs the mic button's real path (permission
        // prompts included); `dictate-file <path>` skips the mic and transcribes the file.
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("{verb}: no Trek window");
            };
            let composer = trek.read(cx).composer.clone();
            if verb == "dictate" {
                composer.update(cx, |c, cx| c.dictate(window, cx));
            } else {
                composer.update(cx, |c, cx| c.dictate_file(PathBuf::from(arg), window, cx));
            }
            Ok(())
        })??;
        return Ok(());
    }
    if verb == "editor" {
        // The editor without a pointer: `editor on|off` (Editor mode), `editor root <folder>`,
        // `editor view explorer|search|scm|agents`, `editor panel terminal|problems|output|tasks|off`,
        // `editor chat new|send <prompt>|<n>`, `editor ai on|off`, and `editor [open] <abs-path> [line]`
        // (a tab in Editor mode, the center surface in the harness). The AI side bar:
        // `editor send <prompt>` (through its input, chips and mode included), `editor type <text>`
        // (typed, not sent), `editor select <first> <last>` (lines in the editor), `editor add`
        // (⌘⇧L), `editor mode agent|plan|ask`, `editor review`, `editor keep|undo all|<path>`.
        // The editor and the chat together: `editor inline [text]` (⌘K's card, typed into),
        // `editor inline-run <text>` (and run), `editor hunk keep|undo|next|prev`,
        // `editor diag <abs-path> <line> <message>` (a problem, as a language server would say),
        // `editor fix` (Fix with Agent on the first problem), `editor term-chat`, and
        // `editor rclick|hover <element id>` (a context menu, a hover, without a pointer; `name#3`
        // for a row's id).
        let (sub, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
            let Some(trek) = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()) else {
                anyhow::bail!("editor: no Trek window");
            };
            let ide = trek.read(cx).ide.clone();
            let ws = ws.clone();
            match (sub, rest) {
                ("on", _) => ws.update(cx, |ws, cx| ws.set_mode(crate::workspace::Mode::Editor, cx)),
                ("off", _) => ws.update(cx, |ws, cx| ws.set_mode(crate::workspace::Mode::Agents, cx)),
                ("root", path) => ws.update(cx, |ws, cx| ws.set_ide_root(PathBuf::from(path), cx)),
                ("view", v) => {
                    let v = match v {
                        "explorer" => crate::ide::SideView::Explorer,
                        "search" => crate::ide::SideView::Search,
                        "scm" => crate::ide::SideView::Scm,
                        "agents" => crate::ide::SideView::Agents,
                        other => anyhow::bail!("editor view: unknown view {other:?} (explorer|search|scm|agents)"),
                    };
                    ide.update(cx, |ide, cx| ide.show_view(v, window, cx));
                }
                ("panel", "off") => ide.update(cx, |ide, cx| {
                    if ide.layout.panel_open {
                        ide.toggle_panel(window, cx);
                    }
                }),
                ("panel", p) => {
                    let tab = match p {
                        "terminal" => crate::ide::PanelTab::Terminal,
                        "problems" => crate::ide::PanelTab::Problems,
                        "output" => crate::ide::PanelTab::Output,
                        "tasks" => crate::ide::PanelTab::Tasks,
                        other => anyhow::bail!("editor panel: unknown tab {other:?} (terminal|problems|output|tasks|off)"),
                    };
                    ide.update(cx, |ide, cx| ide.show_panel(tab, window, cx));
                }
                ("ai", on) => ide.update(cx, |ide, cx| {
                    if ide.layout.ai_open != (on == "on") {
                        ide.toggle_ai(window, cx);
                    }
                }),
                ("send", text) => {
                    let input = ide.read(cx).ai.read(cx).input.clone();
                    input.update(cx, |i, cx| i.send_text(text, window, cx));
                }
                ("type", text) => {
                    let input = ide.read(cx).ai.read(cx).input.clone();
                    input.update(cx, |i, cx| i.insert_text(text, window, cx));
                }
                ("select", lines) => {
                    let (a, b) = lines.split_once(' ').ok_or_else(|| anyhow::anyhow!("editor select <first> <last>"))?;
                    let (a, b): (usize, usize) = (a.parse()?, b.parse()?);
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor select: no file open"))?;
                    let state = editor.read(cx).text_state();
                    state.update(cx, |s, cx| {
                        use gpui_kit::base::input::{Point, RopeExt};
                        let text = s.text().clone();
                        let from = text.point_to_offset(Point::new(a.saturating_sub(1), 0));
                        let to = text.point_to_offset(Point::new(b, 0)).saturating_sub(1).max(from);
                        s.set_selected_range(from..to, cx);
                    });
                }
                ("add", _) => ide.update(cx, |ide, cx| ide.add_selection(false, window, cx)),
                (verb @ ("inline" | "inline-run"), text) => {
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor inline: no file open"))?;
                    editor.update(cx, |e, cx| e.type_inline(text, verb == "inline-run", window, cx));
                }
                ("hunk", what) => {
                    let editor = ide.read(cx).active_editor().ok_or_else(|| anyhow::anyhow!("editor hunk: no file open"))?;
                    editor.update(cx, |e, cx| match what {
                        "keep" => e.act_on_hunk(true, cx),
                        "undo" => e.act_on_hunk(false, cx),
                        "prev" => e.step_hunk(-1, window, cx),
                        _ => e.step_hunk(1, window, cx),
                    });
                }
                ("diag", rest) => {
                    let mut it = rest.splitn(3, ' ');
                    let (Some(path), Some(line), Some(message)) = (it.next(), it.next(), it.next()) else { anyhow::bail!("editor diag <abs-path> <line> <message>") };
                    let line: u32 = line.parse::<u32>()?.saturating_sub(1);
                    let at = lsp_types::Position { line, character: 0 };
                    let d = lsp_types::Diagnostic { range: lsp_types::Range { start: at, end: at }, severity: Some(lsp_types::DiagnosticSeverity::ERROR), message: message.to_string(), source: Some("rustc".into()), ..Default::default() };
                    let path = PathBuf::from(path);
                    ws.update(cx, |ws, cx| {
                        let mut all = ws.diagnostics.get(&path).cloned().unwrap_or_default();
                        all.push(d);
                        ws.set_diagnostics(path, all, cx);
                    });
                }
                ("fix", _) => {
                    let first = ws.read(cx).diagnostics.iter().next().map(|(p, d)| (p.clone(), d[0].clone()));
                    let (path, d) = first.ok_or_else(|| anyhow::anyhow!("editor fix: no problems"))?;
                    ide.update(cx, |ide, cx| ide.fix_problem(path, d, window, cx));
                }
                ("term-chat", _) => ide.update(cx, |ide, cx| ide.terminal_to_chat(window, cx)),
                (verb @ ("rclick" | "hover"), id) => pointer(window, verb, id, cx)?,
                ("mode", m) => {
                    let mode = match m {
                        "agent" => crate::workspace::ChatMode::Agent,
                        "plan" => crate::workspace::ChatMode::Plan,
                        "ask" => crate::workspace::ChatMode::Ask,
                        other => anyhow::bail!("editor mode: unknown mode {other:?} (agent|plan|ask)"),
                    };
                    ws.update(cx, |ws, cx| ws.set_chat_mode_in(&crate::workspace::Scope::Ide, mode, cx));
                }
                ("review", _) => ws.update(cx, |ws, cx| {
                    if let Some(id) = ws.ide_chat.active_thread().map(str::to_string) {
                        ws.open_review(&id, cx);
                    }
                }),
                (verb @ ("keep" | "undo"), which) => ws.update(cx, |ws, cx| {
                    let Some(id) = ws.ide_chat.active_thread().map(str::to_string) else { return };
                    let paths = (which != "all").then(|| vec![which.to_string()]);
                    if verb == "keep" { ws.keep_files(&id, paths, cx) } else { ws.undo_files(&id, paths, cx) }
                }),
                // `editor diff <rel> [staged]`: a file's changes in a diff tab, as Source Control
                // opens them; `editor split on|off`: the diff in front side by side.
                ("diff", rest) => {
                    let (rel, staged) = match rest.strip_suffix(" staged") {
                        Some(r) => (r, true),
                        None => (rest, false),
                    };
                    let root = ws.read(cx).ide_root.clone().ok_or_else(|| anyhow::anyhow!("editor diff: no folder"))?;
                    let top = crate::ide::git::top(&root).ok_or_else(|| anyhow::anyhow!("editor diff: not a repository"))?;
                    let source = crate::ide::diff_view::DiffSource::Git { top, rel: rel.to_string(), staged };
                    ide.update(cx, |ide, cx| _ = ide.open_diff_view(source, false, window, cx));
                }
                ("split", on) => {
                    let diff = ide.read(cx).tabs.get(ide.read(cx).active).and_then(|t| t.diff().cloned()).ok_or_else(|| anyhow::anyhow!("editor split: no diff in front"))?;
                    diff.update(cx, |d, cx| d.set_split(on == "on", cx));
                }
                // `editor new-file`: the Explorer's name input for a new file; `editor rename-chat`:
                // the chat tab's; `editor edit-last`: the chat's last message back to edit;
                // `editor undo-turn`: the last turn's Undo, asking.
                ("new-file", _) => {
                    let explorer = ide.read(cx).explorer.clone();
                    explorer.update(cx, |e, cx| {
                        if let Some(dir) = e.target_dir() {
                            e.begin_new(dir, false, window, cx);
                        }
                    });
                }
                ("rename-chat", _) => {
                    let active = ws.read(cx).ide_chat.active;
                    let ai = ide.read(cx).ai.clone();
                    ai.update(cx, |a, cx| a.begin_rename(active, window, cx));
                }
                ("edit-last", _) => ws.update(cx, |ws, cx| {
                    let Some(id) = ws.ide_chat.active_thread().map(str::to_string) else { return };
                    let Some(live) = ws.live.get(&id) else { return };
                    let last = live.items.iter().enumerate().rev().find_map(|(i, it)| match it {
                        trek_core::store::Item::User { text, aside: false, .. } => Some((live.items.id_at(i).map(str::to_string), text.clone())),
                        _ => None,
                    });
                    if let Some((Some(item), text)) = last {
                        cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: crate::workspace::Scope::Ide, thread: id, text, images: vec![], edit: Some(item) });
                    }
                }),
                ("undo-turn", _) => {
                    let transcript = ide.read(cx).ai.read(cx).transcript.clone();
                    transcript.update(cx, |t, cx| t.confirm_last_undo(cx));
                }
                ("chat", "new") => ws.update(cx, |ws, cx| ws.ide_new_chat(cx)),
                ("chat", r) if r.starts_with("send ") => ws.update(cx, |ws, cx| ws.send_in(&crate::workspace::Scope::Ide, r["send ".len()..].to_string(), vec![], cx)),
                ("chat", n) => {
                    let n: usize = n.parse().map_err(|_| anyhow::anyhow!("editor chat: new|send <prompt>|<tab number>"))?;
                    ws.update(cx, |ws, cx| ws.ide_select_chat(n, cx));
                }
                _ => {
                    // `editor [open] <abs-path> [line]`.
                    let target = if sub == "open" { rest } else { arg };
                    let (path, line) = match target.rsplit_once(' ') {
                        Some((p, n)) if n.parse::<u32>().is_ok() => (p, n.parse().ok()),
                        _ => (target, None),
                    };
                    let path = PathBuf::from(path);
                    trek.update(cx, |t, cx| t.open_editor(path, line, window, cx));
                }
            }
            Ok(())
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if matches!(verb, "click" | "rclick" | "hover" | "scroll" | "type" | "key" | "resize" | "elements") {
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |root, window, cx| -> anyhow::Result<()> {
            match verb {
                "click" | "rclick" | "hover" => pointer(window, verb, arg, cx),
                "type" => {
                    anyhow::ensure!(!arg.is_empty(), "type needs text");
                    type_text(root, window, arg, cx)
                }
                "key" => {
                    use gpui_kit::test::TestWindowExt as _;
                    anyhow::ensure!(!arg.is_empty(), "key needs a keystroke (secondary-k, escape, shift-tab …)");
                    // Manifests write the Mac's keys (`cmd-k`); Trek's bindings read a Mac ⌘ as
                    // Ctrl on Windows, so the harness does too — `secondary` is gpui's spelling
                    // of exactly that, keeping one manifest right on both platforms.
                    let keys: Vec<String> = arg.split_whitespace().map(manifest_key).collect();
                    // All of them parse before any is pressed: a typo doesn't leave half a sequence done.
                    for k in &keys {
                        Keystroke::parse(k).map_err(|e| anyhow::anyhow!("key: bad keystroke {k:?}: {e}"))?;
                    }
                    for k in &keys {
                        window.press(k, cx);
                    }
                    Ok(())
                }
                "scroll" => {
                    use gpui_kit::test::TestWindowExt as _;
                    let (id, dy) = arg.rsplit_once(' ').ok_or_else(|| anyhow::anyhow!("scroll <element id> <px>"))?;
                    let dy: f32 = dy.trim().parse()?;
                    pointer(window, "hover", id, cx)?;
                    // Positive is down the page, as a wheel turned towards you.
                    window.scroll(element_id(id), ScrollDelta::Pixels(point(px(0.), px(-dy))), cx);
                    Ok(())
                }
                "resize" => {
                    let (w, h) = arg.split_once(' ').ok_or_else(|| anyhow::anyhow!("resize <width> <height>"))?;
                    let (w, h): (f32, f32) = (w.trim().parse()?, h.trim().parse()?);
                    anyhow::ensure!(w >= 100. && h >= 100., "resize: {w}×{h} is too small");
                    window.resize(size(px(w), px(h)));
                    Ok(())
                }
                _ => elements(window, cx),
            }
        })??;
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "agent-install" {
        // How an install from the ACP Registry looks under way or failed, without downloading:
        // `agent-install <id> <percent>|fail <message>|clear`.
        use trek_core::registry::Progress;
        let (id, what) = arg.split_once(' ').ok_or_else(|| anyhow::anyhow!("agent-install <id> <percent>|fail <message>|clear"))?;
        let state = match what.split_once(' ').map_or((what, ""), |(a, b)| (a, b)) {
            ("clear", _) => None,
            ("fail", why) => Some(crate::workspace::AgentInstall::Failed(why.to_string())),
            (pct, _) => {
                let pct: u64 = pct.parse().map_err(|_| anyhow::anyhow!("agent-install: bad percent {pct:?}"))?;
                Some(crate::workspace::AgentInstall::Running(Some(Progress::Downloading { done: pct * 1_000_000, total: Some(100_000_000) })))
            }
        };
        ws.update(cx, |ws, cx| {
            match state {
                Some(s) => ws.added_agents.installs.insert(id.to_string(), s),
                None => ws.added_agents.installs.remove(id),
            };
            cx.notify();
        });
        cx.refresh_windows();
        return Ok(());
    }
    if verb == "rewind" {
        // The n-th turn from the end (1 = the latest), undone as its Undo button does: the files
        // too when there's a checkpoint for them (the confirmation's default), and the message
        // back in the composer. `rewind <n> keep` leaves the files alone.
        let (n, files) = arg.split_once(' ').unwrap_or((arg, ""));
        let n: usize = if n.is_empty() { 1 } else { n.parse().map_err(|_| anyhow::anyhow!("rewind [n] [keep]: bad n {n:?}"))? };
        anyhow::ensure!(n >= 1, "rewind: n starts at 1 (the latest turn)");
        ws.update(cx, |ws, cx| -> anyhow::Result<()> {
            let id = ws.focused_thread().map(str::to_string).ok_or_else(|| anyhow::anyhow!("rewind: no thread on screen"))?;
            let live = ws.live.get(&id).ok_or_else(|| anyhow::anyhow!("rewind: the thread isn't loaded"))?;
            let ends: Vec<usize> = live.items.iter().enumerate().filter(|(_, i)| matches!(i, trek_core::store::Item::TurnEnd { .. })).map(|(ix, _)| ix).collect();
            let ix = *ends.iter().rev().nth(n - 1).ok_or_else(|| anyhow::anyhow!("rewind {n}: the thread has {} finished turns", ends.len()))?;
            let end = live.items.id_at(ix).ok_or_else(|| anyhow::anyhow!("rewind {n}: the turn has no id"))?.to_string();
            let start = ws.turn_start_item(&id, &end).ok_or_else(|| anyhow::anyhow!("rewind {n}: no message starts that turn"))?;
            let restore = files != "keep" && ws.restorable_checkpoint(&id, &start).is_some();
            let (text, images) = ws.undo_turn(&id, &end, restore, cx).ok_or_else(|| anyhow::anyhow!("rewind {n}: refused (see the toast)"))?;
            cx.emit(crate::workspace::WorkspaceEvent::ComposeIn { scope: ws.focused_scope(), thread: id, text, images, edit: None });
            Ok(())
        })?;
        cx.refresh_windows();
        return Ok(());
    }
    if matches!(verb, "approve" | "deny" | "answer") {
        ws.update(cx, |ws, cx| card(ws, verb, arg, cx))?;
        cx.refresh_windows();
        return Ok(());
    }
    ws.update(cx, |ws, cx| -> anyhow::Result<()> {
        match (verb, arg) {
            ("ide", _) => ws.toggle_ide(cx),
            ("route", "draft") => ws.new_thread(cx),
            ("route", "no-project") => ws.navigate(Route::Draft { project: None }, cx),
            ("route", "basecamp") => ws.navigate(Route::Basecamp, cx),
            ("route", "notes") => ws.navigate(Route::Notes, cx),
            ("route", "appearance") => ws.navigate(Route::Settings(SettingsPage::Appearance), cx),
            ("route", "first") => {
                let Some(id) = ws.threads.iter().find(|t| t.side_of.is_none() && t.parent_id.is_none()).map(|t| t.id.clone()) else {
                    anyhow::bail!("route first: no threads");
                };
                ws.navigate(Route::Thread(id), cx)
            }
            ("route", other) if other.starts_with("settings:") => {
                let Some(page) = crate::settings_view::page_named(&other["settings:".len()..]) else {
                    anyhow::bail!("route {other}: no such settings page");
                };
                ws.navigate(Route::Settings(page), cx)
            }
            ("route", other) if other.starts_with("title:") => {
                let q = other["title:".len()..].to_lowercase();
                let Some(id) = ws.threads.iter().find(|t| t.title.to_lowercase().contains(&q)).map(|t| t.id.clone()) else {
                    anyhow::bail!("route {other}: no thread title matches");
                };
                ws.navigate(Route::Thread(id), cx)
            }
            ("route", other) if other.starts_with("project:") => ws.navigate(Route::Draft { project: Some(PathBuf::from(&other["project:".len()..])) }, cx),
            ("route", other) if other.starts_with("thread:") => ws.navigate(Route::Thread(other["thread:".len()..].to_string()), cx),
            ("route", other) => anyhow::bail!("route: unknown route {other:?}"),
            // An image in the composer's outbox, as a drop or paste attaches it.
            ("attach", "") => anyhow::bail!("attach needs an image path"),
            ("attach", path) => cx.emit(crate::workspace::WorkspaceEvent::AttachImage(PathBuf::from(path))),
            ("send", "") => anyhow::bail!("send needs a prompt"),
            ("send", text) => ws.send(text.to_string(), vec![], cx),
            ("project", "") => anyhow::bail!("project needs a folder"),
            ("project", path) => {
                let path = std::path::PathBuf::from(path);
                ws.add_project(path.clone(), cx);
                ws.navigate(Route::Draft { project: Some(path) }, cx);
            }
            ("push", "on") => ws.set_push(true, cx),
            ("push", "test") => ws.test_push(cx),
            ("push", "alert") => {
                let Some(id) = ws.threads.first().map(|t| t.id.clone()) else {
                    anyhow::bail!("push alert: no threads to alert on");
                };
                ws.settings.mobile.push_when = trek_core::settings::PushWhen::Always;
                ws.push_alert("Needs your approval: Run ./scripts/migrate.sh --apply", &id);
            }
            ("push", other) => anyhow::bail!("push: unknown argument {other:?} (on|test|alert)"),
            ("pair", _) => {
                ws.offer_pairing(cx);
                let Some(offer) = ws.remote.as_ref().and_then(|r| r.offer.clone()) else {
                    anyhow::bail!("pair: no pairing offer (isolated processes don't run the remote)");
                };
                if let Some(dir) = std::env::var_os("TREK_SHOT_DIR") {
                    let _ = std::fs::write(Path::new(&dir).join("pair.txt"), offer.url);
                }
            }
            ("settled", "") => ws.settled_open = !ws.settled_open,
            ("settled", "on") => ws.settled_open = true,
            ("settled", "off") => ws.settled_open = false,
            ("settled", other) => anyhow::bail!("settled: unknown argument {other:?} (on|off)"),
            ("diff", _) => {
                // The latest turn's changes on screen, in the Git tool.
                let end = ws.current_thread().map(|t| t.id.clone()).and_then(|id| {
                    let items = &ws.live.get(&id)?.items;
                    let ix = items.iter().rposition(|i| matches!(i, trek_core::store::Item::TurnEnd { .. }))?;
                    Some((id, items.id_at(ix)?.to_string()))
                });
                let Some((thread, end)) = end else {
                    anyhow::bail!("diff: the current thread has no finished turn");
                };
                cx.emit(crate::workspace::WorkspaceEvent::ShowTurnDiff { thread, end, path: None });
            }
            ("new", _) => ws.new_thread(cx),
            ("glass", "on") => {
                ws.settings.appearance.glass = true;
                ws.save_settings(cx);
            }
            ("glass", "off") => {
                ws.settings.appearance.glass = false;
                ws.save_settings(cx);
            }
            ("glass", other) => anyhow::bail!("glass: unknown argument {other:?} (on|off)"),
            ("tint", t) => {
                ws.settings.appearance.glass_tint = t.parse().map_err(|_| anyhow::anyhow!("tint: bad value {t:?} (0.2–0.95)"))?;
                ws.save_settings(cx);
            }
            ("theme", t @ ("night" | "paper")) => {
                let choice = if t == "paper" { trek_core::settings::ThemeChoice::Paper } else { trek_core::settings::ThemeChoice::Night };
                ws.settings.appearance.theme = choice;
                ws.save_settings(cx);
                cx.defer(move |cx| crate::apply_theme(choice, None, cx));
            }
            ("theme", other) => anyhow::bail!("theme: unknown theme {other:?} (night|paper)"),
            ("pace", v) => {
                let pace: f32 = v.parse().map_err(|_| anyhow::anyhow!("pace: bad value {v:?} (0 = instant, 1 = demo speed)"))?;
                trek_agents::mock::set_pace(pace);
            }
            _ => anyhow::bail!("command not understood: {verb} {arg}"),
        }
        Ok(())
    })?;
    cx.refresh_windows();
    Ok(())
}

/// An element id as written in a command: `name#3` is a row, ("name", 3).
fn element_id(id: &str) -> ElementId {
    match id.rsplit_once('#').and_then(|(n, i)| Some((n, i.parse::<u64>().ok()?))) {
        Some((name, ix)) => ElementId::NamedInteger(name.to_string().into(), ix),
        None => ElementId::Name(id.to_string().into()),
    }
}

/// How `elements` writes an id, the way `click` reads it back.
fn id_text(id: &ElementId) -> String {
    match id {
        ElementId::Name(n) => n.to_string(),
        ElementId::NamedInteger(n, i) => format!("{n}#{i}"),
        other => format!("{other:?}"),
    }
}

/// `usage-demo`: plans and limits for the Usage card as Claude Code, Codex and Devin report them
/// (Devin's as kept from an earlier run), without asking any real agent.
fn usage_demo(ws: &Entity<Workspace>, cx: &mut App) {
    use trek_agents::{AgentStatus, UsageLimit};
    use trek_core::AgentId;
    ws.update(cx, |ws, cx| {
        let now = ws.now();
        let limit = |label: &str, window: &str, percent: f32, hours: i64| UsageLimit { label: label.into(), percent, resets_at: Some(now + hours * 3_600_000 + 1_380_000), window: window.into() };
        for (agent, name) in [(AgentId::ClaudeCode, "Claude Code"), (AgentId::Codex, "Codex")] {
            if !ws.agents.iter().any(|a| a.agent == agent) {
                let availability = trek_core::detect::Availability::Ready;
                ws.agents.push(trek_core::detect::DetectedAgent { agent, name: name.into(), path: None, version: None, availability, models: vec![], install_hint: None });
            }
        }
        let claude = vec![limit("5-hour limit", "5h", 42., 2), limit("Weekly limit", "7d", 68., 50), limit("Weekly · Opus", "7d", 91., 50)];
        ws.agent_status.insert(AgentId::ClaudeCode.key(), AgentStatus { plan: Some("Claude Max".into()), logged_in: true, limits: claude, ..Default::default() });
        let codex = vec![limit("5-hour limit", "5h", 12., 3), limit("Weekly limit", "7d", 35., 120)];
        ws.agent_status.insert(AgentId::Codex.key(), AgentStatus { plan: Some("ChatGPT Plus".into()), logged_in: true, limits: codex, ..Default::default() });
        let devin = crate::workspace::usage::Snapshot { read_at: now - 25 * 60_000, plan: Some("Devin Pro".into()), limits: vec![limit("Daily quota", "24h", 55., 6)], note: None };
        ws.usage_cached.insert(crate::workspace::devin_agent().key(), devin);
        cx.notify();
    });
}

/// A manifest's `key` keystroke as this platform means it: `cmd`/`win`/`super` name the Mac's ⌘,
/// which Trek's bindings read as Ctrl off the Mac — gpui's `secondary` is exactly that mapping.
/// Anything else (a bare `ctrl-`, `shift-`, a key) passes through.
fn manifest_key(key: &str) -> String {
    let mut parts: Vec<&str> = key.split('-').collect();
    let modifiers = parts.len().saturating_sub(1);
    for part in parts.iter_mut().take(modifiers) {
        if matches!(part.to_ascii_lowercase().as_str(), "cmd" | "win" | "super") {
            *part = "secondary";
        }
    }
    parts.join("-")
}

/// `click|rclick|hover <id>`: the pointer on the one element on screen with that id, as events
/// dispatched to the window (no real input). The kit's lookup panics on a missing or ambiguous id,
/// so both are checked first.
fn pointer(window: &mut Window, verb: &str, id: &str, cx: &mut App) -> anyhow::Result<()> {
    use gpui_kit::test::TestWindowExt as _;
    anyhow::ensure!(!id.is_empty(), "{verb} needs an element id (see `elements`)");
    POINTER_SCRIPTED.store(true, Ordering::Relaxed);
    window.render_frame(cx);
    let id = element_id(id);
    let found: Vec<_> = gpui_kit::base::test_support::snapshots(window).into_iter().filter(|s| s.path().last() == Some(&id)).collect();
    match found.as_slice() {
        [] => anyhow::bail!("{verb}: nothing on screen is {}", id_text(&id)),
        [one] if !one.visible() => anyhow::bail!("{verb}: {} is hidden or scrolled away", id_text(&id)),
        [_] => {}
        many => anyhow::bail!("{verb}: {} elements are {}: {:?}", many.len(), id_text(&id), many.iter().map(|s| s.path().to_vec()).collect::<Vec<_>>()),
    }
    match verb {
        "rclick" => window.right_click(id, cx),
        "hover" => window.hover(id, cx),
        _ => window.click(id, cx),
    }
    Ok(())
}

/// `type <text>`: into the focused field as typed text, or into the main composer when no field
/// has the keyboard.
fn type_text(root: AnyView, window: &mut Window, text: &str, cx: &mut App) -> anyhow::Result<()> {
    let typed = window.focused(cx).is_some() && window.dispatch_keystroke(Keystroke { modifiers: Modifiers::default(), key: String::new(), key_char: Some(text.to_string()) }, cx);
    if typed {
        return Ok(());
    }
    let view = root.downcast::<gpui_kit::component::Root>().ok().map(|r| r.read(cx).view().clone());
    let trek = view.and_then(|v| v.downcast::<crate::root::TrekWindow>().ok()).ok_or_else(|| anyhow::anyhow!("type: no Trek window"))?;
    let composer = trek.read(cx).composer.clone();
    composer.update(cx, |c, cx| {
        c.focus(window, cx);
        c.insert_text(text, window, cx);
    });
    Ok(())
}

/// `elements`: every visible element `click` can name, one per line, to `elements.txt` — its id,
/// then its role and label when it has them, then where it is.
fn elements(window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    use gpui_kit::test::TestWindowExt as _;
    window.render_frame(cx);
    let mut lines: Vec<String> = gpui_kit::base::test_support::snapshots(window)
        .into_iter()
        .filter(|s| s.visible())
        .filter_map(|s| {
            let id = id_text(s.path().last()?);
            let b = s.bounds();
            let role = s.role().map(|r| format!(" {r:?}")).unwrap_or_default();
            let label = s.label().map(|l| format!(" {l:?}")).unwrap_or_default();
            Some(format!("{id}{role}{label} @ {:.0},{:.0} {:.0}×{:.0}", f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)))
        })
        .collect();
    lines.sort();
    let dir = std::env::var_os("TREK_SHOT_DIR").ok_or_else(|| anyhow::anyhow!("elements: no TREK_SHOT_DIR"))?;
    std::fs::write(Path::new(&dir).join("elements.txt"), lines.join("\n") + "\n")?;
    Ok(())
}

/// `approve`, `deny`, `answer <text|n>`: the thread on screen's waiting card, through what its
/// buttons call. `approve` allows a permission or starts a plan; `deny` is Deny, Keep planning or
/// Skip; `answer <n>` picks option n (from 1) of every question and sends, `answer <text>` is the
/// answer typed in the composer.
fn card(ws: &mut Workspace, verb: &str, arg: &str, cx: &mut Context<Workspace>) -> anyhow::Result<()> {
    use trek_agents::{Decision, Prompt};
    let id = ws.focused_thread().map(str::to_string).ok_or_else(|| anyhow::anyhow!("{verb}: no thread on screen"))?;
    let p = ws.live.get(&id).and_then(|l| l.permissions.first()).cloned().ok_or_else(|| anyhow::anyhow!("{verb}: nothing is waiting on the thread"))?;
    match (verb, &p.prompt) {
        ("approve", None) => ws.respond(&id, &p.request_id, Decision::Allow, cx),
        ("approve", Some(Prompt::Plan(_))) => ws.approve_plan(&id, &p.request_id, cx),
        ("approve", Some(Prompt::Questions(_))) => anyhow::bail!("approve: it's a question (answer <text|n>, or deny to skip)"),
        ("deny", _) => ws.respond(&id, &p.request_id, Decision::Deny, cx),
        ("answer", Some(Prompt::Questions(questions))) => {
            anyhow::ensure!(!arg.is_empty(), "answer needs <text> or an option number");
            match arg.parse::<usize>() {
                Ok(n) => {
                    let mut answers = vec![];
                    for (qi, q) in questions.iter().enumerate() {
                        let (label, _) = q.options.get(n.wrapping_sub(1)).ok_or_else(|| anyhow::anyhow!("answer {n}: “{}” has {} options", q.question, q.options.len()))?;
                        if let Some(live) = ws.live.get_mut(&id) {
                            live.picks.insert((p.request_id.clone(), qi), vec![label.clone()]);
                        }
                        answers.push((q.question.clone(), label.clone()));
                    }
                    ws.answer(&id, &p.request_id, answers, cx);
                }
                Err(_) => {
                    let scope = ws.focused_scope();
                    ws.send_in(&scope, arg.to_string(), vec![], cx);
                }
            }
        }
        ("answer", _) => anyhow::bail!("answer: the thread is waiting on a {}, not a question (approve or deny)", if p.prompt.is_some() { "plan" } else { "permission" }),
        _ => unreachable!(),
    }
    Ok(())
}

/// The see-through part of a frame is laid over `$TREK_SHOT_UNDER` (else a dusky gradient),
/// blurred once per (source, size) — recording calls [`save`] for every frame, so the 30-sigma
/// blur must not run per frame.
static UNDERLAY: LazyLock<Mutex<Option<Underlay>>> = LazyLock::new(|| Mutex::new(None));

struct Underlay {
    source: Option<PathBuf>,
    w: u32,
    h: u32,
    image: image::RgbaImage,
}

fn underlay(w: u32, h: u32) -> image::RgbaImage {
    let source = std::env::var_os("TREK_SHOT_UNDER").map(PathBuf::from);
    let mut cached = UNDERLAY.lock().unwrap();
    if let Some(u) = cached.as_ref() {
        if u.w == w && u.h == h && u.source == source {
            return u.image.clone();
        }
    }
    let raw = source
        .as_ref()
        .and_then(|p| image::open(p).ok())
        .map(|i| image::imageops::resize(&i.to_rgba8(), w, h, image::imageops::FilterType::Triangle))
        .unwrap_or_else(|| {
            image::RgbaImage::from_fn(w, h, |x, y| {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                image::Rgba([(40. + 160. * fx) as u8, (70. + 60. * fy) as u8, (150. - 60. * fx) as u8, 255])
            })
        });
    let image = image::imageops::blur(&raw, 30.);
    *cached = Some(Underlay { source, w, h, image: image.clone() });
    image
}

/// Write `image`, with its see-through parts over the blurred `$TREK_SHOT_UNDER` (else a
/// dusky gradient), as the desktop would show through glass.
fn save(mut image: image::RgbaImage, path: &Path) -> anyhow::Result<()> {
    let (w, h) = image.dimensions();
    if image.pixels().any(|p| p[3] < 255) {
        let under = underlay(w, h);
        for (x, y, p) in image.enumerate_pixels_mut() {
            if p[3] == 255 {
                continue;
            }
            let a = p[3] as f32 / 255.;
            let u = under.get_pixel(x, y);
            for c in 0..3 {
                // The frame is premultiplied by alpha.
                p[c] = (p[c] as f32 + u[c] as f32 * (1. - a)).min(255.) as u8;
            }
            p[3] = 255;
        }
    }
    image.save(path)?;
    Ok(())
}
