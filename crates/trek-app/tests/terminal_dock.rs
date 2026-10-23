//! The terminal docked under the chat: the composer's Terminal and ⌃` show and hide it, its
//! shells stay while it's hidden, each thread has its own, it comes back as it was left, and it
//! scrolls back through its output.

mod common;

use common::{open, open_with, run};
use trek_app::keys::{Id, keystroke};
use trek_app::workspace::Route;

#[test]
fn the_composer_terminal_docks_under_the_chat_and_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        trek.send(cx, "hello");
        trek.render(cx);
        assert!(trek.visible(cx, "open-terminal") && !trek.visible(cx, "terminal-dock"));
        trek.click(cx, "open-terminal");
        trek.render(cx);
        assert!(
            trek.visible(cx, "terminal-dock"),
            "the composer's Terminal shows the dock"
        );
        assert!(
            trek.read(cx, |ws, _| ws.settings.layout.terminal_open),
            "kept in settings"
        );
        // Under the composer, the column wide.
        let dock = trek.bounds(cx, "terminal-dock").expect("dock drawn");
        let composer = trek.bounds(cx, "open-terminal").expect("status row drawn");
        assert!(
            dock.origin.y >= composer.origin.y + composer.size.height,
            "dock {dock:?} is under the composer's row {composer:?}"
        );
        // ⌃` hides it; its shell waits for next time rather than starting over.
        trek.press(cx, &keystroke(Id::ToggleTerminal));
        trek.render(cx);
        assert!(!trek.visible(cx, "terminal-dock"));
        assert!(!trek.read(cx, |ws, _| ws.settings.layout.terminal_open));
        let dock = trek.root.read_with(cx, |r, _| r.terminal_dock.clone());
        assert_eq!(dock.read_with(cx, |d, cx| d.count(cx)), 1);
        trek.press(cx, &keystroke(Id::ToggleTerminal));
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock"));
        assert_eq!(
            dock.read_with(cx, |d, cx| d.count(cx)),
            1,
            "the shell is reused"
        );
        // + adds a shell and the tabs appear; the bin closes them, and the dock goes with the last.
        trek.click(cx, "terminal-dock-new");
        trek.render(cx);
        assert_eq!(dock.read_with(cx, |d, cx| d.count(cx)), 2);
        assert!(trek.visible(cx, ("terminal-dock-tab", 1usize)));
        trek.click(cx, "terminal-dock-kill");
        trek.render(cx);
        assert!(
            trek.visible(cx, "terminal-dock") && !trek.visible(cx, ("terminal-dock-tab", 1usize))
        );
        trek.click(cx, "terminal-dock-kill");
        trek.render(cx);
        assert!(!trek.visible(cx, "terminal-dock"), "no shell, no dock");
        assert!(!trek.read(cx, |ws, _| ws.settings.layout.terminal_open));
    });
}

#[test]
fn the_dock_comes_back_as_it_was_left() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.layout.terminal_open = true;
            s.layout.terminal_height = 300.;
        });
        // On the draft too, under the centered composer.
        assert!(trek.visible(cx, "terminal-dock"));
        let dock = trek.bounds(cx, "terminal-dock").expect("dock drawn");
        assert_eq!(dock.size.height, gpui_kit::px(300.));
        let viewport = trek.window(cx, |window, _| window.viewport_size());
        // Flush with the window's bottom, but for the content's rounded frame.
        assert!(
            (viewport.height - (dock.origin.y + dock.size.height)) < gpui_kit::px(12.),
            "dock {dock:?} in a window {viewport:?}"
        );
    });
}

#[test]
fn the_terminal_and_icon_display_in_new_thread_draft() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        assert!(
            trek.visible(cx, "open-terminal"),
            "terminal button must be visible in draft/new thread view"
        );
        assert!(!trek.visible(cx, "terminal-dock"));
        trek.click(cx, "open-terminal");
        trek.render(cx);
        assert!(
            trek.visible(cx, "terminal-dock"),
            "clicking terminal in new thread draft opens the terminal dock"
        );
    });
}

/// The shells of whatever thread (or draft) is on screen.
fn shells(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext) -> Vec<gpui_kit::EntityId> {
    let dock = trek.root.read_with(cx, |r, _| r.terminal_dock.clone());
    dock.read_with(cx, |d, cx| {
        d.terminals(cx).iter().map(|t| t.entity_id()).collect()
    })
}

fn go(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext, route: Route) {
    trek.update(cx, |ws, cx| ws.navigate(route, cx));
    trek.render(cx);
}

#[test]
fn each_thread_has_its_own_dock_and_keeps_its_shells_while_away() {
    run(async |cx| {
        let trek = open(cx);
        let a = trek.send(cx, "first");
        trek.click(cx, "open-terminal");
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock"));
        let a_shells = shells(&trek, cx);
        assert_eq!(a_shells.len(), 1);

        // Another thread: none of A's dock or shells.
        let project = trek.project.clone();
        go(
            &trek,
            cx,
            Route::Draft {
                project: Some(project.clone()),
            },
        );
        assert!(
            !trek.visible(cx, "terminal-dock"),
            "the draft has no dock of its own"
        );
        let b = trek.send(cx, "second");
        assert_ne!(a, b);
        trek.render(cx);
        assert!(
            !trek.visible(cx, "terminal-dock"),
            "A's shell isn't shown under B"
        );
        assert!(shells(&trek, cx).is_empty());
        assert!(
            !trek.read(cx, |ws, _| ws.settings.layout.terminal_open),
            "kept as B has it"
        );

        // Back to A: its dock, with the same shell, never stopped.
        go(&trek, cx, Route::Thread(a.clone()));
        assert!(trek.visible(cx, "terminal-dock"), "A's dock comes back");
        assert_eq!(shells(&trek, cx), a_shells, "the same shell, still running");
        assert!(trek.read(cx, |ws, _| ws.settings.layout.terminal_open));

        // B gets a shell of its own; closing A's last one leaves B's alone.
        go(&trek, cx, Route::Thread(b.clone()));
        trek.press(cx, &keystroke(Id::ToggleTerminal));
        trek.render(cx);
        let b_shells = shells(&trek, cx);
        assert_eq!(b_shells.len(), 1);
        assert_ne!(b_shells, a_shells);
        go(&trek, cx, Route::Thread(a.clone()));
        trek.click(cx, "terminal-dock-kill");
        trek.render(cx);
        assert!(
            !trek.visible(cx, "terminal-dock") && shells(&trek, cx).is_empty(),
            "A's last shell took its dock with it"
        );
        go(&trek, cx, Route::Thread(b));
        assert!(trek.visible(cx, "terminal-dock"), "B's dock is still shown");
        assert_eq!(shells(&trek, cx), b_shells);
    });
}

#[test]
fn the_drafts_shells_go_with_it_when_it_becomes_a_thread() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-terminal");
        trek.render(cx);
        let draft_shells = shells(&trek, cx);
        assert_eq!(draft_shells.len(), 1);
        // The shell has the keys: back to the composer to send.
        trek.update(cx, |_, cx| {
            cx.emit(trek_app::workspace::WorkspaceEvent::FocusComposer)
        });
        trek.send(cx, "hello");
        trek.render(cx);
        assert!(
            trek.visible(cx, "terminal-dock"),
            "the dock stays with the thread the draft became"
        );
        assert_eq!(shells(&trek, cx), draft_shells);
        // And a new draft starts without one.
        let project = trek.project.clone();
        go(
            &trek,
            cx,
            Route::Draft {
                project: Some(project),
            },
        );
        assert!(!trek.visible(cx, "terminal-dock") && shells(&trek, cx).is_empty());
    });
}

#[test]
fn a_later_drafts_shells_stay_with_it_when_an_older_thread_is_opened() {
    run(async |cx| {
        let trek = open(cx);
        // Sent from a draft with no terminal: thread T has no dock.
        let t = trek.send(cx, "hello");
        trek.render(cx);
        assert!(shells(&trek, cx).is_empty());
        // A new draft opens a terminal of its own.
        let project = trek.project.clone();
        go(
            &trek,
            cx,
            Route::Draft {
                project: Some(project),
            },
        );
        trek.click(cx, "open-terminal");
        trek.render(cx);
        let draft_shells = shells(&trek, cx);
        assert_eq!(draft_shells.len(), 1);
        // Opening T doesn't give it the draft's shell.
        go(&trek, cx, Route::Thread(t));
        assert!(
            shells(&trek, cx).is_empty() && !trek.visible(cx, "terminal-dock"),
            "the draft's shell isn't T's"
        );
        let project = trek.project.clone();
        go(
            &trek,
            cx,
            Route::Draft {
                project: Some(project),
            },
        );
        assert_eq!(shells(&trek, cx), draft_shells, "the draft kept its shell");
    });
}

#[test]
fn the_terminal_scrolls_back_through_its_output_and_a_key_brings_it_down() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-terminal");
        trek.render(cx);
        let dock = trek.root.read_with(cx, |r, _| r.terminal_dock.clone());
        let term = dock.read_with(cx, |d, cx| d.terminals(cx)[0].clone());
        let output: String = (1..=200).map(|i| format!("line {i}\r\n")).collect();
        term.read_with(cx, |t, _| t.feed(output.as_bytes()));
        trek.render(cx);
        let bottom = term.read_with(cx, |t, _| t.visible_rows());
        let rows = bottom.len();
        assert!(rows > 3 && rows < 200, "{rows} rows on view");
        assert!(bottom.iter().any(|r| r == "line 200"), "{bottom:?}");

        // The wheel turned up five lines' worth: five lines further back.
        trek.window(cx, |window, cx| {
            use gpui_kit::test::TestWindowExt as _;
            window.scroll(
                "terminal",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(5. * 18.),
                )),
                cx,
            );
        });
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.scrolled_back()), 5);
        let back = term.read_with(cx, |t, _| t.visible_rows());
        assert_eq!(
            back[5..],
            bottom[..rows - 5],
            "the view moved five lines up the output"
        );
        assert!(!back.iter().any(|r| r == "line 200"));

        // New output while back there leaves the view where it is.
        term.read_with(cx, |t, _| t.feed(b"line 201\r\n"));
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.visible_rows()), back);

        // Shift+PageUp / PageDown page through it.
        let before = term.read_with(cx, |t, _| t.scrolled_back());
        trek.press(cx, "shift-pageup");
        assert_eq!(
            term.read_with(cx, |t, _| t.scrolled_back()),
            before + rows - 1
        );
        trek.press(cx, "shift-pagedown");
        assert_eq!(term.read_with(cx, |t, _| t.scrolled_back()), before);

        // Typing goes back to the bottom.
        trek.press(cx, "a");
        trek.render(cx);
        assert_eq!(term.read_with(cx, |t, _| t.scrolled_back()), 0);
        assert!(
            term.read_with(cx, |t, _| t.visible_rows())
                .iter()
                .any(|r| r == "line 201")
        );
    });
}
