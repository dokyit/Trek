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
        trek.press(cx, "secondary-k");
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

/// The `TrekWindow` in `window`.
fn trek_window(cx: &mut TestAppContext, window: AnyWindowHandle) -> gpui_kit::Entity<crate::root::TrekWindow> {
    cx.update_window(window, |root, _, cx| {
        let view = root.downcast::<gpui_kit::component::Root>().expect("root").read(cx).view().clone();
        view.downcast::<crate::root::TrekWindow>().expect("a Trek window")
    })
    .expect("window")
}

#[test]
fn a_reopened_main_window_takes_the_keys() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        close_main(&trek, cx);
        // From the Dock: no navigation comes with it.
        cx.update(|cx| crate::root::show_main(trek.ws.clone(), cx));
        cx.run_until_parked();
        let main = main_window(&trek, cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));
        cx.update_window(main, |_, window, cx| gpui_kit::test::TestWindowExt::input(window, "hello", cx)).expect("window");
        cx.run_until_parked();
        let view = trek_window(cx, main);
        assert_eq!(cx.read(|cx| view.read(cx).composer.read(cx).text(cx)), "hello");
        // Its shortcuts work as well.
        cx.update_window(main, |_, window, cx| gpui_kit::test::TestWindowExt::press(window, "secondary-b", cx)).expect("window");
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.sidebar_collapsed));
    });
}

#[test]
fn a_fork_from_a_thread_window_brings_its_message_to_a_reopened_main_window() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&ws.store, &t.id, transcript(2));
            ws.reload(cx);
            t.id
        });
        let own = trek.open_thread_window(cx, &id);
        close_main(&trek, cx);
        let second = trek.read(cx, |ws, _| ws.store.items_with_ids(&id).expect("items")[5].0.clone());
        let fork = trek.update(cx, |ws, cx| ws.fork_thread(&id, crate::workspace::ForkAt::Before(second), &Scope::Thread(id.clone()), cx)).expect("forked");
        let main = main_window(&trek, cx);
        assert_ne!(main, own);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(fork));
        let view = trek_window(cx, main);
        assert!(cx.read(|cx| view.read(cx).composer.read(cx).text(cx)).starts_with("Step 1:"));
    });
}

#[test]
fn the_file_picker_follows_the_folder_on_screen() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("alpha_only.rs"), "").unwrap();
        let other = new_project("beta");
        std::fs::write(other.join("beta_only.rs"), "").unwrap();
        let (a, b) = trek.update(cx, |ws, cx| {
            let a = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let b = ws.store.create_thread(Some(&other), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            (a.id, b.id)
        });
        let picks = |trek: &Trek, cx: &TestAppContext| cx.read(|cx| trek.root.read(cx).composer.read(cx).picks(cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(a), cx));
        trek.type_text(cx, "@");
        trek.wait(cx, "project A's files", |_| true).await;
        assert!(picks(&trek, cx).contains(&"alpha_only.rs".to_string()), "{:?}", picks(&trek, cx));
        // The window moves to a thread in project B: the "@" stays with A's draft, and B's own
        // picker lists B's files.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(b), cx));
        assert!(!picks(&trek, cx).contains(&"alpha_only.rs".to_string()), "{:?}", picks(&trek, cx));
        trek.type_text(cx, "@");
        trek.wait(cx, "project B's files", |_| true).await;
        assert!(picks(&trek, cx).contains(&"beta_only.rs".to_string()), "{:?}", picks(&trek, cx));
        assert!(!picks(&trek, cx).contains(&"alpha_only.rs".to_string()), "{:?}", picks(&trek, cx));
    });
}

#[test]
fn quitting_leaves_no_handle_on_the_workspace() {
    run(async |cx| {
        let trek = open(cx);
        // What `main` leaves running around the workspace: the loop that waits on the pipe for
        // other launches (open, as it is for as long as Trek is).
        let (_other_launches, rx) = async_channel::unbounded();
        let (links, _heard) = async_channel::unbounded();
        let ws = trek.ws.clone();
        cx.update(|cx| crate::single_instance::hear(Some(rx), vec![], ws, links, cx));
        cx.run_until_parked();
        let ws = trek.ws.downgrade();
        drop(trek);
        // The app quits as `Quit` does it: GPUI runs the quit handlers and closes the windows.
        // What it drops afterwards is its entities, before its globals and the tasks still
        // waiting, so a handle in a waiting task is one the leak detector reports as `run`
        // returns (the "Exited with leaked handles" panic of a `shots` build on Windows). A
        // global's handle is let go with the globals, after the detector has looked.
        cx.update(|cx| {
            cx.shutdown();
            cx.clear_globals();
        });
        cx.run_until_parked();
        ws.assert_released();
    });
}

#[test]
fn quitting_closes_the_windows_first_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        cx.update(|cx| crate::root::quit(cx));
        cx.run_until_parked();
        // Where a window's native state is let go by a task that a quitting loop never runs, the
        // windows are closed before the app ends (see `root::quit`); the Mac ends the app and its
        // windows with it.
        assert_eq!(cx.update(|cx| cx.windows().len()), if cfg!(windows) { 0 } else { 1 });
        drop(trek);
    });
}

/// The note on screen as saved on disk.
fn note_on_disk() -> String {
    trek_core::notes::list_in(&trek_core::notes::notes_dir()).first().map(|n| n.body.clone()).unwrap_or_default()
}

#[test]
fn a_note_typed_just_before_quitting_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        // Typed, then quit before the save that follows typing comes round. The Notes view is
        // the main window's, and only the window holds it (as in the app, not the harness): on
        // Windows `quit` closes the windows before GPUI's quit handlers run.
        trek.type_text(cx, "milk");
        drop(trek);
        cx.update(|cx| crate::root::quit(cx));
        cx.run_until_parked();
        cx.update(|cx| cx.shutdown());
        cx.run_until_parked();
        assert_eq!(note_on_disk(), "milk");
    });
}

#[test]
fn a_note_typed_just_before_the_main_window_closes_is_kept() {
    run(async |cx| {
        let trek = open(cx);
        trek.click(cx, "open-notes");
        trek.render(cx);
        trek.type_text(cx, "eggs");
        cx.update(|cx| crate::root::init(trek.ws.clone(), cx));
        let window = trek.window;
        drop(trek);
        // Closed as on macOS, where Trek keeps running.
        let _ = cx.update(|cx| window.update(cx, |_, window, _| window.remove_window()));
        cx.run_until_parked();
        assert_eq!(note_on_disk(), "eggs");
    });
}
