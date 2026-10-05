//! Design review without Screen Recording: a build with the `shots` feature watches
//! `$TREK_SHOT_DIR/cmd` and runs the commands it finds there, one per line, against the main
//! window, then deletes the file. `shot <name>` draws the window's current frame to
//! `<name>.png` in that folder; see-through areas (liquid glass) are laid over `$TREK_SHOT_UNDER`,
//! blurred, as macOS would show the desktop. Run it with `TREK_FORCE_ACTIVE=1` so a window kept
//! behind others still draws.
//!
//! Commands: `route draft|no-project|basecamp|notes|appearance|thread:<id>|first`,
//! `send <prompt>`, `new` (⌘N), `settled` (fold or open settled history), `glass on|off`, `tint <0.3–0.9>`, `theme night|paper`, `tools`, `wait <ms>`, `shot <name>`.

use crate::workspace::{Route, SettingsPage, Workspace};
use gpui_kit::*;
use std::path::PathBuf;
use std::time::Duration;

pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let Some(dir) = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let cmd = dir.join("cmd");
            let Ok(text) = std::fs::read_to_string(&cmd) else { continue };
            let _ = std::fs::remove_file(&cmd);
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
                match verb {
                    "wait" => cx.background_executor().timer(Duration::from_millis(arg.parse().unwrap_or(300))).await,
                    "shot" => {
                        // A few frames for whatever the last command changed to be drawn.
                        cx.background_executor().timer(Duration::from_millis(400)).await;
                        let path = dir.join(format!("{arg}.png"));
                        let result = cx.update(|cx| {
                            let main = workspace.read(cx).main_window.ok_or_else(|| anyhow::anyhow!("no main window"))?;
                            main.update(cx, |_, window, _| window.render_to_image())?
                        });
                        match result {
                            Ok(image) => match save(image, &path) {
                                Ok(()) => tracing::info!("shot: {}", path.display()),
                                Err(e) => tracing::warn!("shot {arg}: {e:#}"),
                            },
                            Err(e) => tracing::warn!("shot {arg}: {e:#}"),
                        }
                    }
                    _ => {
                        let ws = workspace.clone();
                        let (verb, arg) = (verb.to_string(), arg.to_string());
                        cx.update(|cx| run(&ws, &verb, &arg, cx));
                    }
                }
            }
            let _ = std::fs::write(dir.join("done"), "");
        }
    })
    .detach();
}

fn run(ws: &Entity<Workspace>, verb: &str, arg: &str, cx: &mut App) {
    ws.update(cx, |ws, cx| match (verb, arg) {
        ("route", "draft") => ws.new_thread(cx),
        ("route", "no-project") => ws.navigate(Route::Draft { project: None }, cx),
        ("route", "basecamp") => ws.navigate(Route::Basecamp, cx),
        ("route", "notes") => ws.navigate(Route::Notes, cx),
        ("route", "appearance") => ws.navigate(Route::Settings(SettingsPage::Appearance), cx),
        ("route", "first") => {
            if let Some(id) = ws.threads.iter().find(|t| t.side_of.is_none() && t.parent_id.is_none()).map(|t| t.id.clone()) {
                ws.navigate(Route::Thread(id), cx)
            }
        }
        ("route", other) if other.starts_with("title:") => {
            let q = other["title:".len()..].to_lowercase();
            if let Some(id) = ws.threads.iter().find(|t| t.title.to_lowercase().contains(&q)).map(|t| t.id.clone()) {
                ws.navigate(Route::Thread(id), cx)
            }
        }
        ("route", other) if other.starts_with("thread:") => ws.navigate(Route::Thread(other["thread:".len()..].to_string()), cx),
        ("send", text) => ws.send(text.to_string(), vec![], cx),
        ("settled", _) => ws.settled_open = !ws.settled_open,
        ("new", _) => ws.new_thread(cx),
        ("glass", on) => {
            ws.settings.appearance.glass = on == "on";
            ws.save_settings(cx);
        }
        ("tint", t) => {
            ws.settings.appearance.glass_tint = t.parse().unwrap_or(0.6);
            ws.save_settings(cx);
        }
        ("theme", t) => {
            let choice = if t == "paper" { trek_core::settings::ThemeChoice::Paper } else { trek_core::settings::ThemeChoice::Night };
            ws.settings.appearance.theme = choice;
            ws.save_settings(cx);
            cx.defer(move |cx| crate::apply_theme(choice, None, cx));
        }
        _ => tracing::warn!("shot command not understood: {verb} {arg}"),
    });
    cx.refresh_windows();
}

/// Write `image`, with its see-through parts over the blurred `$TREK_SHOT_UNDER` (else a
/// dusky gradient), as the desktop would show through glass.
fn save(mut image: image::RgbaImage, path: &std::path::Path) -> anyhow::Result<()> {
    let (w, h) = image.dimensions();
    tracing::info!("shot: {} of {} pixels see-through", image.pixels().filter(|p| p[3] < 255).count(), w * h);
    if image.pixels().any(|p| p[3] < 255) {
        let under = std::env::var_os("TREK_SHOT_UNDER")
            .and_then(|p| image::open(p).ok())
            .map(|i| image::imageops::resize(&i.to_rgba8(), w, h, image::imageops::FilterType::Triangle))
            .unwrap_or_else(|| {
                image::RgbaImage::from_fn(w, h, |x, y| {
                    let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                    image::Rgba([(40. + 160. * fx) as u8, (70. + 60. * fy) as u8, (150. - 60. * fx) as u8, 255])
                })
            });
        let under = image::imageops::blur(&under, 30.);
        for (x, y, p) in image.enumerate_pixels_mut() {
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
