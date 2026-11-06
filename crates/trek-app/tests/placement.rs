//! The main window across launches and displays: where it was left, what another launch hands
//! over (Windows' single instance), and a display of another scale.

mod common;

use common::{make_repo, new_project, open, run};
use gpui_kit::{WindowBounds, px, size};
use std::time::Duration;
use trek_app::single_instance::Forwarded;
use trek_app::window_place::{Placement, initial, load};
use trek_app::workspace::Route;
use trek_core::RunState;
use trek_ipc::instance::Open;

#[test]
fn the_main_window_opens_where_it_was_left() {
    run(async |cx| {
        let trek = open(cx);
        cx.simulate_window_resize(trek.window, size(px(1000.), px(700.)));
        // Saved once it has settled, not at every step of a drag.
        cx.run_until_parked();
        let dir = trek_core::paths::data_dir();
        assert_eq!(load(&dir), None, "not yet");
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        let saved = load(&dir).expect("saved");
        assert_eq!(
            (saved.width, saved.height, saved.maximized),
            (1000., 700., false)
        );

        let min = size(
            px(trek_app::window_place::MAIN_MIN_WIDTH_PX),
            px(trek_app::window_place::MAIN_MIN_HEIGHT_PX),
        );
        let (display, bounds) = cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx));
        assert_eq!(
            display,
            cx.update(|cx| cx.displays().first().map(|d| d.id())),
            "on the display it was on"
        );
        assert_eq!(
            bounds,
            WindowBounds::Windowed(gpui_kit::Bounds::new(
                gpui_kit::point(px(saved.x), px(saved.y)),
                size(px(1000.), px(700.))
            ))
        );

        // Maximized comes back maximized, except in a launch in the background, which mustn't
        // come to the front (maximizing does on Windows).
        std::fs::write(
            dir.join("window.json"),
            serde_json::to_vec(&Placement {
                maximized: true,
                ..saved.clone()
            })
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx))
                .1,
            WindowBounds::Maximized(_)
        ));
        assert!(matches!(
            cx.update(|cx| initial(size(px(1280.), px(820.)), min, false, cx))
                .1,
            WindowBounds::Windowed(_)
        ));

        // Off every display (one since unplugged, or far off this one's edge): centred, as at first.
        for gone in [
            Placement {
                display: "unplugged".into(),
                ..saved.clone()
            },
            Placement {
                x: 50_000.,
                ..saved.clone()
            },
        ] {
            std::fs::write(dir.join("window.json"), serde_json::to_vec(&gone).unwrap()).unwrap();
            let (display, bounds) =
                cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx));
            assert_eq!(display, None);
            assert_eq!(bounds.get_bounds().size, size(px(1280.), px(820.)));
        }
    });
}

#[test]
fn another_launch_s_arguments_open_on_the_main_thread_and_are_answered() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("lib.rs");
        std::fs::write(&file, "pub fn f() {}\n").unwrap();
        let (links, heard) = async_channel::unbounded();
        let (tx, rx) = async_channel::unbounded();
        let ws = trek.ws.clone();
        cx.update(|cx| trek_app::single_instance::hear(Some(rx), vec![], ws, links, cx));

        let (done, answer) = async_channel::bounded(1);
        // What isn't a link or something that exists is left alone (and doesn't stop the rest).
        let args = [
            "trek://ask?path=%2Fx&line=1".to_string(),
            "https://example.com/".into(),
            "relative.rs".into(),
            file.display().to_string(),
        ];
        let open = Open {
            args: args.into(),
            background: false,
        };
        tx.try_send(Forwarded { open, done }).unwrap();
        cx.run_until_parked();
        assert_eq!(
            answer.try_recv(),
            Ok(Ok(())),
            "answered once the main thread took them"
        );
        // The link goes where macOS's `on_open_urls` sends links; the file opens in the editor.
        assert_eq!(
            heard.try_recv().as_deref(),
            Ok("trek://ask?path=%2Fx&line=1")
        );
        assert!(heard.try_recv().is_err());
        trek.render(cx);
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Editor { path } if path == file)
        );
    });
}

/// `trek.exe <folder>`: the folder becomes a project and a draft opens in it, as "Open Folder…"
/// does, whether it's this launch's own argument or another launch's.
#[test]
fn a_folder_argument_opens_as_a_project() {
    run(async |cx| {
        let trek = open(cx);
        let (links, heard) = async_channel::unbounded();
        let (tx, rx) = async_channel::unbounded();
        let (first, second) = (new_project("first"), new_project("second"));
        let ws = trek.ws.clone();
        let own = vec![first.display().to_string()];
        cx.update(|cx| trek_app::single_instance::hear(Some(rx), own, ws, links, cx));
        cx.run_until_parked();
        let is_project =
            |trek: &common::Trek, cx: &gpui_kit::TestAppContext, p: &std::path::Path| {
                trek.read(cx, |ws, _| {
                    ws.settings.user_projects.contains(&p.display().to_string())
                })
            };
        assert!(is_project(&trek, cx, &first), "its own argument");
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(p) } if p == first)
        );

        let (done, answer) = async_channel::bounded(1);
        tx.try_send(Forwarded {
            open: Open {
                args: vec![second.display().to_string()],
                background: false,
            },
            done,
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Ok(())));
        assert!(is_project(&trek, cx, &second), "a forwarded one");
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(p) } if p == second)
        );
        assert!(heard.try_recv().is_err(), "no link went anywhere");

        // One that isn't there is no project.
        let gone = second.join("gone");
        let (done, _answer) = async_channel::bounded(1);
        tx.try_send(Forwarded {
            open: Open {
                args: vec![gone.display().to_string()],
                background: false,
            },
            done,
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!is_project(&trek, cx, &gone));
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(p) } if p == second)
        );
    });
}

/// What the VS Code extension sends, through a second launch: the link rides the pipe, goes to
/// the open-URL handler's place, and drafts a thread in the file's project with the selection in
/// the composer.
#[test]
fn a_forwarded_ask_link_drafts_a_thread_in_the_files_project() {
    run(async |cx| {
        let trek = open(cx);
        let (links, links_rx) = async_channel::unbounded();
        let (tx, rx) = async_channel::unbounded();
        let ws = trek.ws.clone();
        cx.update(|cx| trek_app::single_instance::hear(Some(rx), vec![], ws, links, cx));
        trek.update(cx, |ws, cx| ws.hear_deep_links(links_rx, cx));
        // Somewhere else first, so arriving at the project is the link's doing.
        trek.update(cx, |ws, cx| {
            ws.navigate(
                Route::Settings(trek_app::workspace::SettingsPage::General),
                cx,
            )
        });

        let file = trek.project.join("lib.rs");
        std::fs::write(&file, "let x = 1;\n").unwrap();
        // Percent-encoded as `URLSearchParams` writes a Windows path.
        let enc = |s: &str| {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        };
        let url = format!(
            "trek://ask?path={}&line=3&end=5&selection={}",
            enc(&file.display().to_string()),
            enc("let x = 1;")
        );
        let (done, answer) = async_channel::bounded(1);
        tx.try_send(Forwarded {
            open: Open {
                args: vec![url],
                background: false,
            },
            done,
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Ok(())));
        trek.render(cx);
        let project = trek.project.clone();
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(p) } if p == project)
        );
        let text = trek.composer_text(cx);
        assert!(
            text.contains("`lib.rsL3-L5`") && text.contains("let x = 1;"),
            "{text:?}"
        );
    });
}

#[test]
fn a_launch_s_own_arguments_open_once_the_window_is_up() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("own.rs");
        std::fs::write(&file, "\n").unwrap();
        let (links, heard) = async_channel::unbounded();
        let ws = trek.ws.clone();
        cx.update(|cx| {
            trek_app::single_instance::hear(
                None,
                vec!["trek://edit?path=%2Fy".into(), file.display().to_string()],
                ws,
                links,
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(heard.try_recv().as_deref(), Ok("trek://edit?path=%2Fy"));
        trek.render(cx);
        assert!(
            matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Editor { path } if path == file)
        );
    });
}

/// Dragged to a display of another scale (125 %, 150 %…), the window lays out again on that
/// display's device pixels: the title bar's command pill (26pt, a fraction of a device pixel at
/// 125 %) lands on whole device pixels at every scale.
#[test]
fn a_window_moved_to_another_scale_lays_out_on_whole_device_pixels() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.toggle_ide(cx));
        trek.render(cx);
        for scale in [1., 1.25, 1.5, 1.75, 2., 1.25] {
            cx.simulate_window_scale_factor_change(trek.window, scale);
            trek.render(cx);
            assert_eq!(trek.window(cx, |window, _| window.scale_factor()), scale);
            let pill = trek.bounds(cx, "command-center").expect("the command pill");
            for (what, v) in [
                ("top", pill.origin.y),
                ("left", pill.origin.x),
                ("height", pill.size.height),
                ("width", pill.size.width),
            ] {
                let device = v.as_f32() * scale;
                assert!(
                    (device - device.round()).abs() < 0.01,
                    "at {scale}: the pill's {what} is {device} device pixels"
                );
            }
            assert!(
                (pill.size.height.as_f32() - 26.).abs() <= 1. / scale + 0.01,
                "at {scale}: still about 26pt tall ({:?})",
                pill.size.height
            );
        }
    });
}

/// The composer's chips ("1 file +1 −0", the PR's) float over its top, above where the composer's
/// own layout starts: a chip's text there can sit on a half device pixel below zero in that layout
/// (a 13pt line centred in a 31pt pill at 125 %) and on a half above zero in the window's. The
/// window lays the composer out from its content on frames where it changed and from its cache on
/// the others, which during a turn alternate many times a second: both have to put the text on the
/// same device pixel, or it bounces in its pill while the pill holds still.
#[test]
fn the_composers_chips_hold_still_in_and_out_of_its_cache() {
    run(async |cx| {
        let trek = open(cx);
        make_repo(&trek, cx);
        let id = trek.send(cx, "mock:write");
        trek.wait_done(cx, &id, RunState::Idle).await;
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            cx.simulate_window_scale_factor_change(trek.window, scale);
            trek.render(cx);
            // Where it's drawn, in device pixels (a cached view adds its origin in points, which
            // can leave a float's error behind).
            let device = |b: Option<gpui_kit::Bounds<gpui_kit::Pixels>>| {
                b.map(|b| {
                    let y = b.origin.y.as_f32() * scale;
                    assert!(
                        (y - y.round()).abs() < 0.01,
                        "at {scale}: {y} device pixels"
                    );
                    (
                        y.round() as i32,
                        (b.size.height.as_f32() * scale).round() as i32,
                    )
                })
            };
            let mut seen = vec![];
            for _ in 0..2 {
                // Laid out from its content: it changed, and the window draws that frame itself.
                composer.update(cx, |_, cx| cx.notify());
                cx.run_until_parked();
                let chip = device(trek.bounds(cx, "changes-chip"));
                let text = device(trek.bounds(cx, "changes-chip-files"));
                seen.push(("from its content", chip, text));
                // From its cache: nothing changed since.
                trek.render(cx);
                let chip = device(trek.bounds(cx, "changes-chip"));
                let text = device(trek.bounds(cx, "changes-chip-files"));
                seen.push(("from its cache", chip, text));
            }
            let (_, chip, text) = seen[0];
            assert!(
                chip.is_some() && text.is_some(),
                "at {scale}: the changes chip is up"
            );
            for (how, c, t) in &seen {
                assert_eq!(*c, chip, "at {scale}: the chip, laid out {how}");
                assert_eq!(*t, text, "at {scale}: its text, laid out {how}");
            }
        }
    });
}
