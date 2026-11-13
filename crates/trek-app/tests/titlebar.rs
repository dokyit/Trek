//! The title bar's chrome per platform: where it leaves room for the window's controls, that no
//! menus are drawn in the window (macOS has its system menu bar, and Windows has none: the
//! palette, Settings, the context menus and the shortcuts reach every command), and which parts
//! of the row are the pointer's and which are the window's handle.

mod common;

use common::{Trek, open, run};
use gpui_kit::{AppContext as _, Pixels, TestAppContext, px};
use trek_app::chrome::{LEFT_INSET, RIGHT_RESERVE};
use trek_app::root::SIDEBAR_WIDTH;

/// The test window's width (`harness::launch`).
const WIDTH: f32 = 1280.;

fn x(trek: &Trek, cx: &mut TestAppContext, id: &'static str) -> Pixels {
    trek.bounds(cx, id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .origin
        .x
}

fn right(trek: &Trek, cx: &mut TestAppContext, id: &'static str) -> Pixels {
    let b = trek
        .bounds(cx, id)
        .unwrap_or_else(|| panic!("{id} is on screen"));
    b.origin.x + b.size.width
}

#[test]
fn the_title_bar_clears_the_window_controls_on_each_platform() {
    run(async |cx| {
        let trek = open(cx);
        // Left: the traffic lights (macOS) or a small margin (Windows).
        assert_eq!(
            x(&trek, cx, "toggle-sidebar"),
            px(if cfg!(target_os = "macos") { 80. } else { 12. })
        );
        assert_eq!(px(LEFT_INSET), x(&trek, cx, "toggle-sidebar"));
        // Right: macOS's controls are on the left; Windows's caption buttons are drawn at the right
        // end, and Trek's buttons sit just left of them, the same 8 px from the edge as on macOS.
        let reserve = if cfg!(target_os = "macos") {
            0.
        } else {
            3. * 34.
        };
        assert_eq!(RIGHT_RESERVE, reserve);
        // On Windows a line sets them apart from the caption buttons, the row's 5 px gap before it.
        let line = if cfg!(windows) {
            5. + 1. + 2. * trek_app::chrome::CAPTION_DIVIDER_MARGIN
        } else {
            0.
        };
        near(
            right(&trek, cx, "toggle-tools"),
            px(WIDTH - 8. - reserve - line),
        );
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
        // The cluster is as wide as the sidebar under it (less the margin), and a gap follows: the
        // same on both platforms, as nothing but the toggle and the mark are in it.
        assert!(switch >= px(SIDEBAR_WIDTH), "{switch:?}");
        near(switch, px(SIDEBAR_WIDTH + 8.));
        // And the sidebar itself is where it was.
        assert_eq!(
            trek.bounds(cx, "sidebar")
                .map(|b| (b.origin.x, b.size.width)),
            Some((px(0.), px(SIDEBAR_WIDTH)))
        );
    });
}

#[test]
fn no_menus_are_drawn_in_the_window_on_any_platform() {
    run(async |cx| {
        let trek = open(cx);
        for id in ["menu-bar", "menu-dropdown"] {
            assert!(!trek.visible(cx, id), "{id}");
        }
        for ix in 0..5usize {
            assert!(!trek.visible(cx, ("menu-title", ix)), "menu {ix}");
        }
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
                let settle = window
                    .try_find("settle")
                    .expect("Settle is in the title bar")
                    .bounds();
                (
                    window.viewport_size().width,
                    settle.origin.x + settle.size.width,
                )
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
        assert_eq!(x(&trek, cx, "toggle-sidebar"), px(80.));
        near(x(&trek, cx, "mode-switch"), px(280.));
        // The system's menus are the ones main sets: the app's menu first, Hide among its items.
        let menus = trek_app::menus();
        assert_eq!(
            menus.first().map(|m| m.name.to_string()),
            Some("Trek".to_string())
        );
        // And Help last, where macOS puts it (Send feedback… lives there).
        assert_eq!(menus.len(), 6);
        assert_eq!(
            menus.last().map(|m| m.name.to_string()),
            Some("Help".to_string())
        );
    });
}

/// The controls in the title bar's row, each in the area it keeps the window's drag from (`chrome::no_drag`).
const AGENTS_CONTROLS: [&str; 6] = [
    "toggle-sidebar",
    "mode-switch",
    "thread-menu",
    "hub",
    "toggle-tools",
    "settle",
];
/// The same in the Editor's.
const EDITOR_CONTROLS: [&str; 3] = ["mode-switch", "command-center", "layout-toggles"];

/// Where `id`'s no-drag area is, and its control inside it.
fn area_and_control(
    trek: &Trek,
    cx: &mut TestAppContext,
    id: &'static str,
) -> (gpui_kit::Bounds<Pixels>, gpui_kit::Bounds<Pixels>) {
    let area = trek
        .bounds(cx, format!("no-drag-{id}"))
        .unwrap_or_else(|| panic!("{id} has a no-drag area"));
    let control = trek
        .bounds(cx, id)
        .unwrap_or_else(|| panic!("{id} is on screen"));
    (area, control)
}

/// `inner` lies inside `outer`, give or take a pixel.
fn holds(outer: gpui_kit::Bounds<Pixels>, inner: gpui_kit::Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(1.)
        && inner.right() <= outer.right() + px(1.)
        && inner.top() >= outer.top() - px(1.)
        && inner.bottom() <= outer.bottom() + px(1.)
}

/// Whether `point` is inside any no-drag area of the title bar.
fn in_a_control(
    trek: &Trek,
    cx: &mut TestAppContext,
    ids: &[&'static str],
    point: gpui_kit::Point<Pixels>,
) -> bool {
    ids.iter().any(|id| {
        trek.bounds(cx, format!("no-drag-{id}"))
            .is_some_and(|b| b.contains(&point))
    })
}

/// A thread on screen, so the bar has what a thread gives it (the hub, Settle).
fn thread_on_screen(trek: &Trek, cx: &mut TestAppContext) {
    let id = trek.quiet_thread(cx);
    trek.update(cx, |ws, cx| {
        ws.navigate(trek_app::workspace::Route::Thread(id), cx)
    });
    trek.render(cx);
}

#[test]
fn every_control_in_the_agents_title_bar_is_in_a_no_drag_area_that_fits_it() {
    run(async |cx| {
        let trek = open(cx);
        thread_on_screen(&trek, cx);
        for id in AGENTS_CONTROLS {
            let (area, control) = area_and_control(&trek, cx, id);
            // The area is the control and nothing more: a wider one would take the handle's room.
            assert!(holds(area, control), "{id}: {area:?} holds {control:?}");
            assert!(
                (area.size.width - control.size.width).abs() <= px(1.)
                    && (area.size.height - control.size.height).abs() <= px(1.),
                "{id}: {area:?} is the size of {control:?}"
            );
        }
    });
}

#[test]
fn every_control_in_the_editors_title_bar_is_in_a_no_drag_area() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.set_mode(trek_app::workspace::Mode::Editor, cx)
        });
        cx.run_until_parked();
        trek.render(cx);
        for id in EDITOR_CONTROLS {
            let (area, control) = area_and_control(&trek, cx, id);
            assert!(holds(area, control), "{id}: {area:?} holds {control:?}");
        }
        // The layout toggles and Settings are one area: each of them is inside it.
        for id in [
            "toggle-primary",
            "toggle-panel",
            "toggle-ai",
            "ide-settings",
        ] {
            let b = trek
                .bounds(cx, id)
                .unwrap_or_else(|| panic!("{id} is on screen"));
            assert!(
                in_a_control(&trek, cx, &EDITOR_CONTROLS, b.center()),
                "{id}"
            );
        }
    });
}

#[test]
fn a_narrow_editor_window_narrows_the_command_center_and_keeps_its_toggles() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            ws.set_mode(trek_app::workspace::Mode::Editor, cx)
        });
        cx.run_until_parked();
        let width = 760.;
        cx.simulate_window_resize(trek.window, gpui_kit::size(px(width), px(640.)));
        trek.render(cx);
        // The pill gives way: the layout toggles and Settings stay on screen, left of where
        // Windows's caption buttons are (which a full-width pill pushed off the window).
        let (area, pill) = area_and_control(&trek, cx, "command-center");
        assert!(
            area.size.width < px(420.) && area.size.width > px(40.),
            "the Command Center narrows: {area:?}"
        );
        assert!(holds(area, pill), "{area:?} holds {pill:?}");
        for id in [
            "toggle-primary",
            "toggle-panel",
            "toggle-ai",
            "ide-settings",
        ] {
            assert!(trek.visible(cx, id), "{id} is on screen");
            assert!(
                right(&trek, cx, id) <= px(width - RIGHT_RESERVE),
                "{id} is left of the caption buttons"
            );
        }
        assert!(
            right(&trek, cx, "command-center") <= x(&trek, cx, "toggle-primary"),
            "the pill ends before the toggles"
        );
    });
}

#[test]
fn the_empty_strip_of_the_title_bar_is_still_the_windows_handle() {
    run(async |cx| {
        let trek = open(cx);
        thread_on_screen(&trek, cx);
        let mid = px(17.);
        // Between the mode switch and the thread menu (then the hub) is the thread's title and then
        // nothing: both are the handle, so a press there moves the window and a double click
        // maximises it.
        let switch_end = right(&trek, cx, "mode-switch");
        let hub_start = x(
            &trek,
            cx,
            if cfg!(windows) {
                "settle"
            } else {
                "thread-menu"
            },
        );
        assert!(
            hub_start - switch_end > px(200.),
            "there's a strip to drag by: {switch_end:?} to {hub_start:?}"
        );
        for at in [
            switch_end + px(40.),
            (switch_end + hub_start) / 2.,
            hub_start - px(20.),
        ] {
            assert!(
                !in_a_control(&trek, cx, &AGENTS_CONTROLS, gpui_kit::point(at, mid)),
                "{at:?} is the handle's"
            );
        }
        // The Mac's and Windows's alike: what sits between the controls (the gaps) is the handle's too.
        // Settle leads Trek's buttons on Windows, away from the window's Minimize, and ends them
        // on the Mac; the gap checked is the one between it and the next button.
        let (gap_start, gap_end) = if cfg!(windows) {
            (right(&trek, cx, "settle"), x(&trek, cx, "thread-menu"))
        } else {
            (right(&trek, cx, "toggle-tools"), x(&trek, cx, "settle"))
        };
        assert!(gap_end > gap_start, "the buttons have a gap between them");
        assert!(!in_a_control(
            &trek,
            cx,
            &AGENTS_CONTROLS,
            gpui_kit::point((gap_start + gap_end) / 2., mid)
        ));
    });
}

#[test]
fn a_press_on_the_handle_is_never_swallowed_so_the_window_can_move_and_maximise() {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, point};
    run(async |cx| {
        let trek = open(cx);
        thread_on_screen(&trek, cx);
        // Windows hands a press on the handle to the window first, and only does its own thing
        // (a move; a double click maximises) with one the window left alone.
        let at = point(
            (right(&trek, cx, "mode-switch") + x(&trek, cx, "hub")) / 2.,
            px(17.),
        );
        let press = |click_count| {
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
                click_count,
                first_mouse: false,
            })
        };
        let (first, second) = trek.window(cx, |window, cx| {
            let first = window.dispatch_event(press(1), cx).propagate;
            let second = window.dispatch_event(press(2), cx).propagate;
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::default(),
                    click_count: 2,
                }),
                cx,
            );
            (first, second)
        });
        assert!(
            first && second,
            "a press and a double click on the empty strip reach Windows"
        );
    });
}

#[cfg(windows)]
mod windows_chrome {
    use super::*;
    use gpui_kit::{Capslock, InputEvent as _, Modifiers, ModifiersChangedEvent};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Holds Alt down or lets it go, as the platform reports it: a modifiers change.
    fn alt(trek: &Trek, cx: &mut TestAppContext, held: bool) {
        let modifiers = if held {
            Modifiers::alt()
        } else {
            Modifiers::none()
        };
        trek.window(cx, |window, cx| {
            window.dispatch_event(
                ModifiersChangedEvent {
                    modifiers,
                    capslock: Capslock { on: false },
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.run_until_parked();
        trek.render(cx);
    }

    #[test]
    fn a_tap_of_alt_takes_nothing_with_no_menus_to_take_it_for() {
        run(async |cx| {
            let trek = open(cx);
            trek.type_text(cx, "hi");
            let focused = trek.window(cx, |window, cx| window.focused(cx));
            alt(&trek, cx, true);
            alt(&trek, cx, false);
            assert_eq!(
                trek.window(cx, |window, cx| window.focused(cx)),
                focused,
                "the composer keeps the keys"
            );
            trek.type_text(cx, " there");
            assert_eq!(trek.composer_text(cx), "hi there");
        });
    }

    /// Whether the app's Quit handler ran: the harness installs none, so a test brings its own.
    fn quit_heard(cx: &mut TestAppContext) -> Arc<AtomicBool> {
        let heard = Arc::new(AtomicBool::new(false));
        let seen = heard.clone();
        cx.update(|cx| {
            cx.on_action(move |_: &trek_app::Quit, _| seen.store(true, Ordering::SeqCst));
        });
        heard
    }

    #[test]
    fn the_caption_buttons_stay_reachable_over_the_palette() {
        run(async |cx| {
            let trek = open(cx);
            assert!(
                !trek.visible(cx, "caption-close"),
                "the title bar's own are enough with nothing over them"
            );
            trek.window(cx, |window, cx| {
                window.dispatch_action(Box::new(trek_app::OpenPalette), cx)
            });
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
                trek_app::root::init(trek.ws.clone(), cx);
                trek_app::root::quit_with_main_window(cx);
            });
            let id = trek.quiet_thread(cx);
            let own = trek.open_thread_window(cx, &id);
            cx.update_window(own, |_, window, _| window.remove_window())
                .expect("window");
            cx.run_until_parked();
            assert!(
                !quit.load(Ordering::SeqCst),
                "a thread window closing is just that"
            );
            trek.window(cx, |window, _| window.remove_window());
            cx.run_until_parked();
            assert!(quit.load(Ordering::SeqCst), "the main window closing quits");
        });
    }
}

/// The main window's close button where the app ends with it (Windows'). Trek's own logic, given
/// that setting: tested wherever the tests run, not only on Windows.
mod ends_with_its_main_window {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Whether the app's Quit handler ran: the harness installs none, so a test brings its own.
    fn quit_heard(cx: &mut TestAppContext) -> Arc<AtomicBool> {
        let heard = Arc::new(AtomicBool::new(false));
        let seen = heard.clone();
        cx.update(|cx| {
            cx.on_action(move |_: &trek_app::Quit, _| seen.store(true, Ordering::SeqCst));
        });
        heard
    }

    fn ending_with_main_window(trek: &Trek, cx: &mut TestAppContext) -> Arc<AtomicBool> {
        let quit = quit_heard(cx);
        cx.update(|cx| {
            trek_app::root::init(trek.ws.clone(), cx);
            trek_app::root::quit_with_main_window(cx);
        });
        quit
    }

    #[test]
    fn closing_it_with_an_agent_at_work_asks_and_can_keep_the_work_going() {
        run(async |cx| {
            let trek = open(cx);
            let quit = ending_with_main_window(&trek, cx);
            // Nothing at work: the close button is the Quit command, with nothing asked.
            trek.window(cx, |window, cx| trek_app::root::close_main(window, cx));
            cx.run_until_parked();
            assert!(!cx.has_pending_prompt());
            assert!(quit.swap(false, Ordering::SeqCst), "it quits");
            // A turn running: it asks, and Cancel leaves it all be.
            let id = trek.send(cx, "mock:long 8s");
            let tid = id.clone();
            trek.wait(cx, "the turn to start", |ws| ws.turn_running(&tid))
                .await;
            trek.window(cx, |window, cx| trek_app::root::close_main(window, cx));
            cx.run_until_parked();
            assert!(cx.has_pending_prompt(), "it asks first");
            cx.simulate_prompt_answer("Cancel");
            cx.run_until_parked();
            assert!(!quit.load(Ordering::SeqCst));
            assert!(!cx.windows().is_empty());
            // Keep Working: Trek's windows close, and the turn carries on without a quit.
            trek.window(cx, |window, cx| trek_app::root::close_main(window, cx));
            cx.run_until_parked();
            cx.simulate_prompt_answer("Keep Working");
            cx.run_until_parked();
            assert!(cx.windows().is_empty(), "its windows are closed");
            assert!(
                !quit.load(Ordering::SeqCst),
                "the main window closing isn't the Quit command meanwhile"
            );
            assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
        });
    }

    /// Kept in the tray (Settings › General, Windows'), the close button closes the windows and
    /// Trek stays up, as a Mac's does in the Dock: nothing asked even with a turn running, and no
    /// quitting once it's done. Without the tray icon to come back through, it quits as before.
    #[test]
    fn kept_in_the_tray_the_close_button_leaves_trek_running() {
        run(async |cx| {
            let trek = open(cx);
            let quit = ending_with_main_window(&trek, cx);
            trek.update(cx, |ws, _| {
                ws.settings.general.stay_in_tray = true;
                ws.settings.notifications.menu_bar_icon = true;
            });
            let id = trek.send(cx, "mock:long 2s");
            let tid = id.clone();
            trek.wait(cx, "the turn to start", |ws| ws.turn_running(&tid))
                .await;
            trek.window(cx, |window, cx| trek_app::root::close_main(window, cx));
            cx.run_until_parked();
            assert!(!cx.has_pending_prompt(), "nothing to ask");
            assert!(cx.windows().is_empty(), "its windows are closed");
            let tid = id.clone();
            trek.wait(cx, "the turn to end", |ws| !ws.turn_running(&tid))
                .await;
            cx.run_until_parked();
            assert!(!quit.load(Ordering::SeqCst), "it stays in the tray");
            // Back on screen, the tray icon off: the close button is the Quit command again.
            cx.update(|cx| trek_app::root::show_main(trek.ws.clone(), cx));
            cx.run_until_parked();
            trek.update(cx, |ws, _| ws.settings.notifications.menu_bar_icon = false);
            let main = trek
                .read(cx, |ws, _| ws.main_window)
                .expect("the main window again");
            main.update(cx, |_, window, cx| trek_app::root::close_main(window, cx))
                .expect("the window");
            cx.run_until_parked();
            assert!(
                quit.load(Ordering::SeqCst),
                "nothing to come back through: it quits"
            );
        });
    }

    #[test]
    fn working_in_the_background_quits_once_nothing_is_at_work() {
        run(async |cx| {
            let trek = open(cx);
            let quit = ending_with_main_window(&trek, cx);
            let id = trek.send(cx, "mock:long 3s");
            let tid = id.clone();
            trek.wait(cx, "the turn to start", |ws| ws.turn_running(&tid))
                .await;
            cx.update(|cx| trek_app::root::keep_working(Duration::from_millis(300), cx));
            cx.run_until_parked();
            assert!(cx.windows().is_empty());
            let tid = id.clone();
            trek.wait(cx, "the turn to end", |ws| !ws.turn_running(&tid))
                .await;
            let heard = quit.clone();
            trek.wait(cx, "Trek to quit once it's done", move |_| {
                heard.load(Ordering::SeqCst)
            })
            .await;
        });
    }

    #[test]
    fn what_closing_would_stop_is_said_in_words() {
        let t = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(trek_app::root::still_working(&[], false), None);
        let (title, one) =
            trek_app::root::still_working(&t(&["Fix the login bug"]), false).unwrap();
        assert_eq!(title, "An agent is still working");
        assert_eq!(
            one,
            "An agent is working on “Fix the login bug”. Trek can keep it going with its windows closed, and quit once it's done; opening Trek meanwhile brings the window back. Quitting now stops it."
        );
        let (title, many) = trek_app::root::still_working(&t(&["A", "B", "C", "D"]), true).unwrap();
        assert_eq!(title, "Agents are still working");
        assert!(
            many.starts_with(
                "Agents are working on “A”, “B” and 2 more. Trek can keep them going with its windows closed, and quit once they're done;"
            ),
            "{many}"
        );
        let (title, update) = trek_app::root::still_working(&[], true).unwrap();
        assert_eq!(title, "An agent is being updated");
        assert!(update.starts_with("Trek is updating an agent."), "{update}");
    }
}
