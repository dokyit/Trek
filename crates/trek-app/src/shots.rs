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
//! `send <prompt>`, `project <folder>` (added, and a draft in it), `diff` (the latest turn's changes in the Git tool),
//! `pair` (a pairing code, its link written to `pair.txt`), `push on|test|alert`, `new` (⌘N), `settled [on|off]`,
//! `glass on|off`, `tint <0.2–0.95>`, `theme night|paper`, `tools on|off|git|explorer|terminal|browser|sidechat|simulator`,
//! `range today|week|all` (Basecamp's), `pace <f>` (the mock agent's speed: 1.0 demo, 0 instant),
//! `wait <ms>` or `wait idle|permission|question|plan [cap ms]`, `record <name> <ms> [fps]` (frames
//! to `<name>.frames/` plus `<name>.ffconcat` for ffmpeg's concat demuxer; `record stop`, `record wait`),
//! `shot <name>`, `quit`. Lines starting with `#` are comments.

use crate::workspace::{PanelTool, Route, SettingsPage, Workspace};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let Some(dir) = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    cx.spawn(async move |cx| {
        let mut recording: Option<Recording> = None;
        loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let cmd = dir.join("cmd");
            let Ok(text) = std::fs::read_to_string(&cmd) else { continue };
            let _ = std::fs::remove_file(&cmd);
            let mut errors = Vec::new();
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
                let result = match verb {
                    "wait" => wait(&workspace, arg, cx).await,
                    "shot" => shot(&workspace, &dir, arg, cx).await,
                    "record" => record(&workspace, &dir, arg, &mut recording, cx).await,
                    "quit" => quit(&workspace, cx),
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

async fn shot(ws: &Entity<Workspace>, dir: &Path, arg: &str, cx: &mut AsyncApp) -> anyhow::Result<()> {
    if arg.is_empty() {
        anyhow::bail!("shot needs a name");
    }
    // A few frames for whatever the last command changed to be drawn.
    cx.background_executor().timer(Duration::from_millis(400)).await;
    let path = dir.join(format!("{arg}.png"));
    let image = cx.update(|cx| -> anyhow::Result<image::RgbaImage> {
        let main = ws.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
        main.update(cx, |_, window, _| window.render_to_image())?
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
                main.update(cx, |_, window, _| window.render_to_image())?
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

fn quit(ws: &Entity<Workspace>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| {
        ws.update(cx, |ws, _| ws.shutdown_sessions());
        cx.quit();
    });
    Ok(())
}

fn run(ws: &Entity<Workspace>, verb: &str, arg: &str, cx: &mut App) -> anyhow::Result<()> {
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
    ws.update(cx, |ws, cx| -> anyhow::Result<()> {
        match (verb, arg) {
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
