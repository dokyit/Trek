//! The title bar's chrome per platform: where it leaves room for the window's controls, and on
//! Windows, where there's no system menu bar, Trek's menus in the window.

use super::harness::{open, run};
use crate::chrome::{LEFT_INSET, RIGHT_RESERVE};
use crate::root::SIDEBAR_WIDTH;
use gpui_kit::{AppContext as _, Pixels, TestAppContext, px};

/// The test window's width (`harness::launch`).
const WIDTH: f32 = 1280.;

fn x(trek: &super::harness::Trek, cx: &mut TestAppContext, id: &'static str) -> Pixels {
    trek.bounds(cx, id).unwrap_or_else(|| panic!("{id} is on screen")).origin.x
}

fn right(trek: &super::harness::Trek, cx: &mut TestAppContext, id: &'static str) -> Pixels {
    let b = trek.bounds(cx, id).unwrap_or_else(|| panic!("{id} is on screen"));
    b.origin.x + b.size.width
}

#[test]
fn the_title_bar_clears_the_window_controls_on_each_platform() {
    run(async |cx| {
        let trek = open(cx);
        // Left: the traffic lights (macOS) or a small margin (Windows).
        assert_eq!(x(&trek, cx, "toggle-sidebar"), px(if cfg!(target_os = "macos") { 80. } else { 12. }));
        assert_eq!(px(LEFT_INSET), x(&trek, cx, "toggle-sidebar"));
        // Right: macOS's controls are on the left; Windows's caption buttons are drawn at the right
        // end, and Trek's buttons sit just left of them, the same 8 px from the edge as on macOS.
        let reserve = if cfg!(target_os = "macos") { 0. } else { 3. * 34. };
        assert_eq!(RIGHT_RESERVE, reserve);
        near(right(&trek, cx, "toggle-tools"), px(WIDTH - 8. - reserve));
    });
}

/// Within a pixel: the buttons' own boxes are a pixel off the bar's padding.
fn near(a: Pixels, b: Pixels) {
    assert!((a - b).abs() <= px(1.), "{a:?} is not near {b:?}");
}

#[test]
fn what_follows_the_toggle_and_mark_starts_where_the_sidebar_ends() {
    run(async |cx| {
        let trek = open(cx);
        let switch = x(&trek, cx, "mode-switch");
        // The cluster is as wide as the sidebar under it (less the margin), and a gap follows.
        assert!(switch >= px(SIDEBAR_WIDTH), "{switch:?}");
        if !crate::chrome::IN_WINDOW_MENUS {
            // Within a pixel: macOS lays the gap out at 279 px.
            near(switch, px(SIDEBAR_WIDTH + 8.));
        }
        // And the sidebar itself is where it was.
        assert_eq!(trek.bounds(cx, "sidebar").map(|b| (b.origin.x, b.size.width)), Some((px(0.), px(SIDEBAR_WIDTH))));
    });
}

#[test]
fn a_thread_windows_buttons_sit_left_of_the_window_controls() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let own = trek.open_thread_window(cx, &id);
        let (width, edge) = cx
            .update_window(own, |_, window, cx| {
                use gpui_kit::test::TestWindowExt as _;
                window.render_frame(cx);
                let settle = window.try_find("settle").expect("Settle is in the title bar").bounds();
                (window.viewport_size().width, settle.origin.x + settle.size.width)
            })
            .expect("window");
        near(edge, width - px(8. + RIGHT_RESERVE));
    });
}

#[cfg(target_os = "macos")]
#[test]
fn macos_keeps_its_system_menus_and_title_bar() {
    run(async |cx| {
        let trek = open(cx);
        assert!(!trek.visible(cx, ("menu-title", 0usize)), "no menu bar in the window");
        assert_eq!(x(&trek, cx, "toggle-sidebar"), px(80.));
        near(x(&trek, cx, "mode-switch"), px(280.));
        // The system's menus are the ones main sets: the app's menu first, Hide among its items.
        let menus = crate::menus();
        assert_eq!(menus.first().map(|m| m.name.to_string()), Some("Trek".to_string()));
        assert_eq!(menus.len(), 5);
    });
}

#[cfg(windows)]
mod windows_menus {
    use super::*;
    use crate::workspace::Route;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn the_menus_are_titles_in_the_title_bar() {
        run(async |cx| {
            let trek = open(cx);
            for ix in 0..5usize {
                assert!(trek.visible(cx, ("menu-title", ix)), "menu {ix}");
            }
            assert!(!trek.visible(cx, ("menu-title", 5usize)));
            assert!(!trek.visible(cx, "menu-dropdown"));
            // After the toggle and the mark, which keep their places.
            assert!(x(&trek, cx, ("menu-title", 0usize)) > x(&trek, cx, "toggle-sidebar"));
        });
    }

    fn x(trek: &super::super::harness::Trek, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> Pixels {
        trek.bounds(cx, id).expect("on screen").origin.x
    }

    #[test]
    fn a_title_opens_its_menu_and_a_second_press_puts_it_away() {
        run(async |cx| {
            let trek = open(cx);
            trek.click(cx, ("menu-title", 2usize));
            assert!(trek.visible(cx, "menu-dropdown"));
            // View's rows (a separator is a row): Toggle Sidebar is the sixth.
            assert!(trek.visible(cx, ("menu-item", 5usize)));
            trek.click(cx, ("menu-title", 2usize));
            assert!(!trek.visible(cx, "menu-dropdown"), "the same title again closes it");
        });
    }

    #[test]
    fn choosing_an_item_dispatches_its_action_and_closes_the_menu() {
        run(async |cx| {
            let trek = open(cx);
            assert!(!trek.read(cx, |ws, _| ws.sidebar_collapsed));
            trek.click(cx, ("menu-title", 2usize));
            trek.click(cx, ("menu-item", 5usize));
            assert!(!trek.visible(cx, "menu-dropdown"));
            assert!(trek.read(cx, |ws, _| ws.sidebar_collapsed), "Toggle Sidebar ran");
        });
    }

    #[test]
    fn escape_puts_the_menu_away_and_focus_goes_back() {
        run(async |cx| {
            let trek = open(cx);
            let focused = trek.window(cx, |window, cx| window.focused(cx));
            trek.click(cx, ("menu-title", 0usize));
            assert!(trek.visible(cx, "menu-dropdown"));
            trek.press(cx, "escape");
            assert!(!trek.visible(cx, "menu-dropdown"));
            assert_eq!(trek.window(cx, |window, cx| window.focused(cx)), focused, "the composer has focus again");
            // And typing still lands in it.
            trek.type_text(cx, "hi");
            assert_eq!(trek.composer_text(cx), "hi");
        });
    }

    #[test]
    fn with_a_menu_open_the_pointer_moves_to_other_titles_and_a_press_elsewhere_closes_it() {
        use gpui_kit::test::TestWindowExt as _;
        run(async |cx| {
            let trek = open(cx);
            trek.click(cx, ("menu-title", 0usize));
            let first = x(&trek, cx, "menu-dropdown");
            trek.window(cx, |window, cx| window.hover(("menu-title", 3usize), cx));
            cx.run_until_parked();
            assert!(trek.visible(cx, "menu-dropdown"));
            assert!(x(&trek, cx, "menu-dropdown") > first, "Window's menu is under its own title");
            trek.click(cx, "mode-agents");
            assert!(!trek.visible(cx, "menu-dropdown"), "a press elsewhere closes it");
        });
    }

    #[test]
    fn the_arrows_move_between_menus_and_items_and_return_chooses() {
        run(async |cx| {
            let trek = open(cx);
            trek.click(cx, ("menu-title", 0usize));
            let file = x(&trek, cx, "menu-dropdown");
            trek.press(cx, "right");
            assert!(x(&trek, cx, "menu-dropdown") > file, "Thread's menu");
            trek.press(cx, "left");
            trek.press(cx, "left");
            assert!(x(&trek, cx, "menu-dropdown") > file, "left of File is Help, the last: its menu is at the right");
            trek.press(cx, "right");
            // File: New Thread, Open Folder…, rule, Settings…, rule, Exit. In this harness the app's
            // own handlers aren't installed, but the window handles New Thread and Settings….
            trek.press(cx, "down");
            trek.press(cx, "down");
            trek.press(cx, "down");
            trek.press(cx, "enter");
            assert!(!trek.visible(cx, "menu-dropdown"));
            assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Settings(crate::workspace::SettingsPage::General)));
        });
    }

    /// Whether the app's Quit handler ran: the harness installs none, so a test brings its own.
    fn quit_heard(cx: &mut TestAppContext) -> Arc<AtomicBool> {
        let heard = Arc::new(AtomicBool::new(false));
        let seen = heard.clone();
        cx.update(|cx| {
            cx.on_action(move |_: &crate::Quit, _| seen.store(true, Ordering::SeqCst));
        });
        heard
    }

    #[test]
    fn an_item_nothing_handles_is_off_and_does_nothing() {
        run(async |cx| {
            let trek = open(cx);
            // Exit is the app's own: no window handles Quit, and the harness has no app handler.
            trek.click(cx, ("menu-title", 0usize));
            trek.click(cx, ("menu-item", 5usize));
            assert!(trek.visible(cx, "menu-dropdown"), "a dimmed item doesn't close the menu");
            trek.press(cx, "escape");
            // With a handler it's on offer, and runs.
            let heard = quit_heard(cx);
            trek.click(cx, ("menu-title", 0usize));
            trek.click(cx, ("menu-item", 5usize));
            assert!(!trek.visible(cx, "menu-dropdown"));
            assert!(heard.load(Ordering::SeqCst), "Exit asked the app to quit");
        });
    }

    #[test]
    fn the_menu_comes_in_on_a_spring_and_just_appears_without_motion() {
        run(async |cx| {
            let trek = open(cx);
            cx.update(|cx| cx.set_reduce_motion(false));
            trek.click(cx, ("menu-title", 1usize));
            cx.executor().advance_clock(std::time::Duration::from_millis(40));
            trek.render(cx);
            let early = trek.bounds(cx, "menu-dropdown").expect("drawn, on its way in");
            cx.executor().advance_clock(std::time::Duration::from_millis(1_000));
            trek.render(cx);
            let settled = trek.bounds(cx, "menu-dropdown").expect("drawn");
            assert!(early.origin.y < settled.origin.y, "it slid down into place: {early:?} → {settled:?}");
            trek.press(cx, "escape");
            cx.executor().advance_clock(std::time::Duration::from_millis(40));
            trek.render(cx);
            assert!(trek.visible(cx, "menu-dropdown"), "still drawn while it leaves");
            cx.executor().advance_clock(std::time::Duration::from_millis(1_000));
            trek.render(cx);
            assert!(!trek.visible(cx, "menu-dropdown"), "gone once it has left");
        });
    }

    #[test]
    fn the_caption_buttons_stay_reachable_over_the_palette() {
        run(async |cx| {
            let trek = open(cx);
            assert!(!trek.visible(cx, "caption-close"), "the title bar's own are enough with nothing over them");
            trek.window(cx, |window, cx| window.dispatch_action(Box::new(crate::OpenPalette), cx));
            cx.run_until_parked();
            assert!(trek.visible(cx, "palette"));
            for id in ["caption-minimize", "caption-maximize", "caption-close"] {
                assert!(trek.visible(cx, id), "{id} is over the palette's backdrop");
            }
            let close = trek.bounds(cx, "caption-close").expect("drawn");
            near(close.origin.x + close.size.width, px(WIDTH));
            assert_eq!(close.origin.y, px(0.));
        });
    }

    #[test]
    fn closing_the_main_window_quits_but_closing_a_thread_window_does_not() {
        run(async |cx| {
            let trek = open(cx);
            let quit = quit_heard(cx);
            cx.update(|cx| {
                crate::root::init(trek.ws.clone(), cx);
                crate::root::quit_with_main_window(cx);
            });
            let id = trek.quiet_thread(cx);
            let own = trek.open_thread_window(cx, &id);
            cx.update_window(own, |_, window, _| window.remove_window()).expect("window");
            cx.run_until_parked();
            assert!(!quit.load(Ordering::SeqCst), "a thread window closing is just that");
            trek.window(cx, |window, _| window.remove_window());
            cx.run_until_parked();
            assert!(quit.load(Ordering::SeqCst), "the main window closing quits");
        });
    }
}