//! Shortcuts through the real window: the keys the table gives are the ones that work, and the
//! Keyboard Shortcuts page shows the table.

use super::harness::{open, run};
use crate::keys::{self, Id};
use crate::workspace::{Route, SettingsPage};
use gpui_kit::SharedString;

/// The platform's own spelling of ⌘K opens the palette (Ctrl+K on Windows), and again closes it.
#[test]
fn the_palette_opens_with_the_platforms_own_keys() {
    run(async |cx| {
        let trek = open(cx);
        trek.press(cx, &keys::keystroke(Id::OpenPalette));
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        trek.render(cx);
        assert!(!trek.visible(cx, "palette"));
    });
}

/// Ctrl+K itself, as a Windows keyboard sends it (no ⌘ there).
#[cfg(windows)]
#[test]
fn ctrl_k_opens_the_palette_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        trek.press(cx, "ctrl-k");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        // Ctrl+Shift+P too: the way in that the terminal panel lets through.
        trek.press(cx, "ctrl-shift-p");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
    });
}

/// On Windows, Alt+Shift+E (not Ctrl+Alt+E, which is AltGr on many layouts) switches Agents and Editor.
#[cfg(windows)]
#[test]
fn alt_shift_e_switches_to_the_editor_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        assert!(!trek.read(cx, |ws, _| ws.ide()));
        trek.press(cx, "ctrl-alt-e");
        assert!(!trek.read(cx, |ws, _| ws.ide()), "Ctrl+Alt+E is AltGr's, not Trek's");
        trek.press(cx, "alt-shift-e");
        assert!(trek.read(cx, |ws, _| ws.ide()));
    });
}

/// Windows has no Hide and no Minimize shortcut: nothing is bound to the Mac's keys for them.
#[cfg(windows)]
#[test]
fn windows_has_no_hide_or_minimize_keys() {
    assert!(keys::hint(Id::HideApp).is_empty() && keys::hint(Id::Minimize).is_empty());
}

/// The page draws the table's rows (the ones on the first screen; the rest scroll).
#[test]
fn the_shortcuts_page_shows_the_tables_rows() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Shortcuts), cx));
        trek.render(cx);
        let groups = keys::shortcut_groups(keys::is_mac(), false);
        assert_eq!(groups[0].0, "Threads");
        for (label, _) in &groups[0].1 {
            assert!(trek.visible(cx, SharedString::from(format!("shortcut-{label}"))), "{label}");
        }
    });
}
