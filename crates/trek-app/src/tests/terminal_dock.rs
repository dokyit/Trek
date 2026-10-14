//! The terminal docked under the chat: the composer's Terminal and ⌃` show and hide it, its
//! shells stay while it's hidden, and it comes back as it was left.

use super::harness::{open, open_with, run};
use crate::keys::{Id, keystroke};

#[test]
fn the_composer_terminal_docks_under_the_chat_and_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        trek.send(cx, "hello");
        trek.render(cx);
        assert!(trek.visible(cx, "open-terminal") && !trek.visible(cx, "terminal-dock"));
        trek.click(cx, "open-terminal");
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock"), "the composer's Terminal shows the dock");
        assert!(trek.read(cx, |ws, _| ws.settings.layout.terminal_open), "kept in settings");
        // Under the composer, the column wide.
        let dock = trek.bounds(cx, "terminal-dock").expect("dock drawn");
        let composer = trek.bounds(cx, "open-terminal").expect("status row drawn");
        assert!(dock.origin.y >= composer.origin.y + composer.size.height, "dock {dock:?} is under the composer's row {composer:?}");
        // ⌃` hides it; its shell waits for next time rather than starting over.
        trek.press(cx, &keystroke(Id::ToggleTerminal));
        trek.render(cx);
        assert!(!trek.visible(cx, "terminal-dock"));
        assert!(!trek.read(cx, |ws, _| ws.settings.layout.terminal_open));
        let dock = trek.root.read_with(cx, |r, _| r.terminal_dock.clone());
        assert_eq!(dock.read_with(cx, |d, _| d.count()), 1);
        trek.press(cx, &keystroke(Id::ToggleTerminal));
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock"));
        assert_eq!(dock.read_with(cx, |d, _| d.count()), 1, "the shell is reused");
        // + adds a shell and the tabs appear; the bin closes them, and the dock goes with the last.
        trek.click(cx, "terminal-dock-new");
        trek.render(cx);
        assert_eq!(dock.read_with(cx, |d, _| d.count()), 2);
        assert!(trek.visible(cx, ("terminal-dock-tab", 1usize)));
        trek.click(cx, "terminal-dock-kill");
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock") && !trek.visible(cx, ("terminal-dock-tab", 1usize)));
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
        assert!((viewport.height - (dock.origin.y + dock.size.height)) < gpui_kit::px(12.), "dock {dock:?} in a window {viewport:?}");
    });
}

#[test]
fn the_terminal_and_icon_display_in_new_thread_draft() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "open-terminal"), "terminal button must be visible in draft/new thread view");
        assert!(!trek.visible(cx, "terminal-dock"));
        trek.click(cx, "open-terminal");
        trek.render(cx);
        assert!(trek.visible(cx, "terminal-dock"), "clicking terminal in new thread draft opens the terminal dock");
    });
}
