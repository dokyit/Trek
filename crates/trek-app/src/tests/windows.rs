//! The main window closing and coming back, and thread windows that outlive it.

use super::harness::{Trek, mock, new_project, open, run, store_items, transcript};
use crate::workspace::{ItemRef, Route, Scope, SettingsPage};
use gpui_kit::{AnyWindowHandle, AppContext as _, TestAppContext};
use trek_agents::{AgentEvent, CommandKind, SlashCommand};
use trek_core::{Effort, HandHolding};

/// Close the main window as its close button would, with the app-wide handlers the app installs
/// at launch (toasts, bringing the main window back, the palette).
fn close_main(trek: &Trek, cx: &mut TestAppContext) {
    cx.update(|cx| crate::root::init(trek.ws.clone(), cx));
    trek.window(cx, |window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(trek.read(cx, |ws, _| ws.main_window), None);
}

fn main_window(trek: &Trek, cx: &TestAppContext) -> AnyWindowHandle {
    trek.read(cx, |ws, _| ws.main_window).expect("the main window is open")
}

#[test]
fn command_k_in_a_thread_window_reopens_the_main_window_with_the_palette() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let own = trek.open_thread_window(cx, &id);
        close_main(&trek, cx);
        cx.update_window(own, |_, window, cx| window.dispatch_action(Box::new(crate::OpenPalette), cx)).expect("thread window");
        cx.run_until_parked();
        let main = main_window(&trek, cx);
        assert_ne!(main, trek.window);
        assert!(trek.visible_in(cx, main, "palette"), "the palette opened in the new main window");
        assert!(!trek.visible_in(cx, own, "palette"));
    });
}

#[test]
fn the_palette_is_on_offer_with_no_window_open() {
    run(async |cx| {
        let trek = open(cx);
        close_main(&trek, cx);
        // The menu item and ⌘K stay enabled: the app handles them with no window to.
        cx.update(|cx| crate::app_actions(cx));
        assert!(cx.update(|cx| cx.is_action_available(&crate::OpenPalette)));
        // What that handler does.
        cx.update(|cx| crate::root::show_palette(trek.ws.clone(), cx));
        cx.run_until_parked();
        assert!(trek.visible_in(cx, main_window(&trek, cx), "palette"));
    });
}

#[test]
fn a_reopened_main_window_starts_afresh() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&ws.store, &t.id, transcript(400));
            ws.reload(cx);
            t.id
        });
        // A search hit deep in a long thread: it opens there.
        let hit = trek.read(cx, |ws, _| ws.store.items_with_ids(&id).expect("items")[503].0.clone());
        trek.update(cx, |ws, cx| ws.open_thread_at(&id, ItemRef::Id(hit), cx));
        assert!(trek.visible(cx, ("answer", 503usize)));
        // The palette is up (the browser hides under it) when the window closes.
        trek.press(cx, "cmd-k");
        assert!(trek.read(cx, |ws, _| ws.overlay_open));
        close_main(&trek, cx);
        assert!(!trek.read(cx, |ws, _| ws.overlay_open), "nothing covers the tools panel any more");

        cx.update(|cx| crate::root::show_main(trek.ws.clone(), cx));
        cx.run_until_parked();
        let main = main_window(&trek, cx);
        // The same thread, at its end, not back at the old hit.
        assert!(trek.visible_in(cx, main, ("copy-turn", 1999usize)));
        assert!(!trek.visible_in(cx, main, ("answer", 503usize)));
        assert!(!trek.visible_in(cx, main, "palette"));
    });
}

#[test]
fn slash_commands_follow_the_folder_of_the_thread_shown() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let _own = trek.open_thread_window(cx, &id);
        let review = SlashCommand { name: "review-only".into(), description: "Review this project".into(), kind: CommandKind::Command };
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::Commands(vec![review])], cx));
        let offers = |trek: &Trek, cx: &TestAppContext, scope: Scope| {
            trek.read(cx, |ws, _| ws.slash_commands(&scope, &mock()).iter().any(|c| c.name == "review-only"))
        };
        assert!(offers(&trek, cx, Scope::Main), "the main window shows the thread");
        // The main window moves to another project, then to Settings; the thread's own window
        // keeps offering what the agent offered in its folder.
        let other = new_project("other");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(other) }, cx));
        assert!(!offers(&trek, cx, Scope::Main));
        assert!(offers(&trek, cx, Scope::Thread(id.clone())));
        trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx));
        assert!(offers(&trek, cx, Scope::Thread(id.clone())));
        // Trek's own commands come first wherever it is.
        assert_eq!(trek.read(cx, |ws, _| ws.slash_commands(&Scope::Thread(id.clone()), &mock()).first().map(|c| c.name.clone())), Some("new".into()));
    });
}
