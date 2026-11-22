//! Settings › General › "Delete all Trek data…": the confirmation asks for a typed word before
//! Delete does anything, then this profile's folders go but what stays (Trek's worktrees, an
//! agent's second sign-in, threads' folders without a project); a profile beside it and a folder
//! a link inside led to survive. And nothing but that page can start it.

mod common;

use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::path::{Path, PathBuf};
use trek_app::workspace::{Route, SettingsPage};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Scroll Settings' page down by `by` points, as a wheel would.
fn scroll_settings(trek: &Trek, cx: &mut TestAppContext, by: f32) {
    let page = trek.bounds(cx, "settings-content").expect("the page");
    let wheel = gpui_kit::ScrollWheelEvent {
        position: page.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-by))),
        ..Default::default()
    };
    trek.window(cx, |window, cx| {
        _ = window.dispatch_event(gpui_kit::InputEvent::to_platform_input(wheel), cx)
    });
    trek.render(cx);
}

#[cfg(unix)]
fn link(to: &Path, at: &Path) {
    std::os::unix::fs::symlink(to, at).unwrap();
}

#[cfg(windows)]
fn link(to: &Path, at: &Path) {
    if std::os::windows::fs::symlink_dir(to, at).is_err() {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(at)
            .arg(to)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[test]
fn delete_all_data_asks_for_the_word_then_deletes_this_profile_alone() {
    run(async |cx| {
        let trek = open(cx);
        let data = trek_core::paths::data_dir();
        // This test's profile, as Trek writes one.
        write(&data.join("notes/groceries.md"), "eggs");
        write(&data.join("transcripts/t1.log"), "hello");
        write(&data.join("mobile/devices.json"), "{\"devices\":[]}");
        write(&data.join("telemetry/state.json"), "{}");
        write(&data.join("logs/trek-2026-11-22.log"), "a line");
        write(&data.join("cache/videos/v1/frame.jpg"), "jpg");
        write(
            &data.join("worktrees/repo/brave-fox/src/main.rs"),
            "fn main() {}",
        );
        write(&data.join("accounts/work/.credentials.json"), "login");
        write(
            &data.join("chats/2026-11-22-120000-000/notes.md"),
            "made here",
        );
        // Outside it: the profile beside it, and a folder a link inside points at.
        let parent = data.parent().unwrap().to_path_buf();
        let sibling: PathBuf = parent.join(format!(
            "{}-sibling",
            data.file_name().unwrap().to_string_lossy()
        ));
        write(&sibling.join("settings.toml"), "the other profile");
        let decoy = parent.join(format!(
            "{}-decoy",
            data.file_name().unwrap().to_string_lossy()
        ));
        write(&decoy.join("keep-me.txt"), "outside");
        link(&decoy, &data.join("linked-out"));
        trek.update(cx, |ws, cx| {
            ws.save_settings(cx);
            ws.navigate(Route::Settings(SettingsPage::General), cx)
        });
        trek.render(cx);
        scroll_settings(&trek, cx, 4000.);
        assert!(trek.visible(cx, "delete-all-data"));
        trek.click(cx, "delete-all-data");
        trek.render(cx);
        assert!(
            trek.visible(cx, "wipe-confirm"),
            "the confirmation lists it all"
        );
        assert!(trek.visible(cx, "wipe-cancel"));

        // Not without the word.
        trek.click(cx, "wipe-delete");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.wiped.is_none()));
        assert!(data.join("notes/groceries.md").exists());
        trek.type_text(cx, "nope");
        trek.click(cx, "wipe-delete");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.wiped.is_none()));

        trek.press(cx, "secondary-a");
        trek.type_text(cx, "Delete");
        trek.render(cx);
        trek.click(cx, "wipe-delete");
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        trek.wait(cx, "the deleting", |ws| ws.wiped.is_some()).await;
        let (plan, outcome) = trek.read(cx, |ws, _| ws.wiped.clone().unwrap());
        assert!(outcome.left.is_empty(), "{:?}", outcome.left);
        assert!(plan.refused.is_empty(), "{:?}", plan.refused);

        for gone in [
            "notes",
            "transcripts",
            "mobile",
            "telemetry",
            "logs",
            "cache",
            "settings.toml",
            "linked-out",
        ] {
            assert!(
                std::fs::symlink_metadata(data.join(gone)).is_err(),
                "{gone} is deleted"
            );
        }
        for kept in [
            "worktrees/repo/brave-fox/src/main.rs",
            "accounts/work/.credentials.json",
            "chats/2026-11-22-120000-000/notes.md",
        ] {
            assert!(data.join(kept).exists(), "{kept} stays");
        }
        assert_eq!(
            std::fs::read_to_string(decoy.join("keep-me.txt")).unwrap(),
            "outside",
            "a link was deleted as a link"
        );
        assert_eq!(
            std::fs::read_to_string(sibling.join("settings.toml")).unwrap(),
            "the other profile"
        );
        let _ = std::fs::remove_dir_all(&sibling);
        let _ = std::fs::remove_dir_all(&decoy);
    });
}

/// Cancel is the first button, and closes it with nothing deleted.
#[test]
fn cancel_leaves_everything() {
    run(async |cx| {
        let trek = open(cx);
        let data = trek_core::paths::data_dir();
        write(&data.join("notes/a.md"), "x");
        let ws = trek.ws.clone();
        trek.window(cx, |window, cx| trek_app::wipe::confirm(ws, window, cx));
        trek.render(cx);
        let cancel = trek.bounds(cx, "wipe-cancel").expect("Cancel");
        let delete = trek.bounds(cx, "wipe-delete").expect("Delete");
        assert!(cancel.left() < delete.left(), "Cancel comes first");
        trek.click(cx, "wipe-cancel");
        trek.render(cx);
        assert!(!trek.visible(cx, "wipe-confirm"));
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.wiped.is_none()));
        assert!(data.join("notes/a.md").exists());
    });
}

/// Only Settings starts it: nothing an agent or a phone can reach (an MCP tool, a request over
/// the phone or machine link, a `trek://` link, a palette command) calls into it. Every file
/// that names the app's entry points is listed here.
#[test]
fn only_the_settings_page_reaches_delete_all_data() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let entry_points = [
        "wipe::confirm",
        "wipe::run",
        "wind_down_for_wipe",
        "leave_profile_for_wipe",
        "wipe::execute",
        "paths::leave(",
    ];
    let mut found = vec![];
    let mut stack = vec![crates.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" && name != "tests" {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for entry in entry_points {
                if text.contains(entry) {
                    let rel = path
                        .strip_prefix(crates)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    found.push(format!("{rel}: {entry}"));
                }
            }
        }
    }
    found.sort();
    assert_eq!(
        found,
        [
            "trek-app/src/settings_view/pages.rs: wipe::confirm",
            "trek-app/src/wipe.rs: leave_profile_for_wipe",
            "trek-app/src/wipe.rs: paths::leave(",
            "trek-app/src/wipe.rs: wind_down_for_wipe",
            "trek-app/src/wipe.rs: wipe::execute",
        ],
        "a new caller of Delete all Trek data: make sure no agent, phone or link can reach it"
    );
}
