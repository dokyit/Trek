//! The main window across launches and displays: where it was left, what another launch hands
//! over (Windows' single instance), and a display of another scale.

use super::harness::{open, run};
use crate::single_instance::Forwarded;
use crate::window_place::{Placement, initial, load};
use crate::workspace::Route;
use gpui_kit::{WindowBounds, px, size};
use std::time::Duration;
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
        assert_eq!((saved.width, saved.height, saved.maximized), (1000., 700., false));

        let min = size(px(760.), px(520.));
        let (display, bounds) = cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx));
        assert_eq!(display, cx.update(|cx| cx.displays().first().map(|d| d.id())), "on the display it was on");
        assert_eq!(bounds, WindowBounds::Windowed(gpui_kit::Bounds::new(gpui_kit::point(px(saved.x), px(saved.y)), size(px(1000.), px(700.)))));

        // Maximized comes back maximized, except in a launch in the background, which mustn't
        // come to the front (maximizing does on Windows).
        std::fs::write(dir.join("window.json"), serde_json::to_vec(&Placement { maximized: true, ..saved.clone() }).unwrap()).unwrap();
        assert!(matches!(cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx)).1, WindowBounds::Maximized(_)));
        assert!(matches!(cx.update(|cx| initial(size(px(1280.), px(820.)), min, false, cx)).1, WindowBounds::Windowed(_)));

        // Off every display (one since unplugged, or far off this one's edge): centred, as at first.
        for gone in [Placement { display: "unplugged".into(), ..saved.clone() }, Placement { x: 50_000., ..saved.clone() }] {
            std::fs::write(dir.join("window.json"), serde_json::to_vec(&gone).unwrap()).unwrap();
            let (display, bounds) = cx.update(|cx| initial(size(px(1280.), px(820.)), min, true, cx));
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
        cx.update(|cx| crate::single_instance::hear(Some(rx), vec![], ws, links, cx));

        let (done, answer) = async_channel::bounded(1);
        let open = Open { args: vec!["trek://ask?path=%2Fx&line=1".into(), file.display().to_string(), trek.project.display().to_string()], background: false };
        tx.try_send(Forwarded { open, done }).unwrap();
        cx.run_until_parked();
        assert_eq!(answer.try_recv(), Ok(Ok(())), "answered once the main thread took them");
        // The link goes where macOS's `on_open_urls` sends links; the file opens in the editor;
        // the folder only brought Trek forward.
        assert_eq!(heard.try_recv().as_deref(), Ok("trek://ask?path=%2Fx&line=1"));
        assert!(heard.try_recv().is_err());
        trek.render(cx);
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Editor { path } if path == file));
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
        cx.update(|cx| crate::single_instance::hear(None, vec!["trek://edit?path=%2Fy".into(), file.display().to_string()], ws, links, cx));
        cx.run_until_parked();
        assert_eq!(heard.try_recv().as_deref(), Ok("trek://edit?path=%2Fy"));
        trek.render(cx);
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Editor { path } if path == file));
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
            for (what, v) in [("top", pill.origin.y), ("left", pill.origin.x), ("height", pill.size.height), ("width", pill.size.width)] {
                let device = v.as_f32() * scale;
                assert!((device - device.round()).abs() < 0.01, "at {scale}: the pill's {what} is {device} device pixels");
            }
            assert!((pill.size.height.as_f32() - 26.).abs() <= 1. / scale + 0.01, "at {scale}: still about 26pt tall ({:?})", pill.size.height);
        }
    });
}
