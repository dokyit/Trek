//! ⌘K with an empty field is a thread switcher: the latest threads first, numbered for ⌘1–⌘9,
//! with their project and age, and the keys in a footer.

use super::harness::{Trek, mock, open, run};
use gpui_kit::TestAppContext;
use trek_core::HandHolding;
use trek_core::store::now_ms;

/// Twelve threads in the project, saved out of order: thread `i` was last touched `ages[i]`
/// minutes ago. Returns their titles newest first.
fn threads(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    let ages = [50, 3, 700, 20, 1, 90, 400, 8, 2_000, 30, 5_000, 12];
    let project = trek.project.clone();
    trek.update(cx, |ws, cx| {
        let now = now_ms();
        for (i, minutes) in ages.iter().enumerate() {
            let mut t = ws.store.create_thread(Some(&project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = format!("Thread {i}");
            t.updated_at = now - minutes * 60_000;
            ws.store.save_thread(&t).expect("save");
        }
        ws.reload(cx);
    });
    let mut order: Vec<usize> = (0..ages.len()).collect();
    order.sort_by_key(|&i| ages[i]);
    order.into_iter().map(|i| format!("Thread {i}")).collect()
}

fn rows(trek: &Trek, cx: &mut TestAppContext) -> Vec<(String, Option<usize>)> {
    let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
    palette.read_with(cx, |p, cx| p.entries(cx).iter().map(|e| (e.label.to_string(), e.jump)).collect())
}

#[test]
fn an_empty_palette_lists_the_latest_threads_first_numbered_for_jumping() {
    run(async |cx| {
        let trek = open(cx);
        let newest = threads(&trek, cx);
        trek.press(cx, "secondary-k");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        let listed = rows(&trek, cx);
        // The nine latest, newest first, numbered 1–9; nothing else carries a number.
        let expected: Vec<(String, Option<usize>)> = newest.iter().take(9).enumerate().map(|(i, t)| (t.clone(), Some(i + 1))).collect();
        assert_eq!(listed[..9], expected[..]);
        assert!(listed[9..].iter().all(|(_, n)| n.is_none()), "{listed:?}");
        assert!(trek.visible(cx, ("palette-row", 8usize)), "the ninth recent thread is on screen");
        // The keys, in a footer.
        assert!(trek.visible(cx, "palette-footer"));

        // A query lists matches without numbers.
        trek.type_text(cx, "Thread");
        trek.render(cx);
        assert!(rows(&trek, cx).iter().all(|(_, n)| n.is_none()));
    });
}

#[test]
fn command_and_a_digit_open_that_recent_thread() {
    run(async |cx| {
        let trek = open(cx);
        let newest = threads(&trek, cx);
        trek.press(cx, "secondary-k");
        trek.render(cx);
        trek.press(cx, "secondary-3");
        trek.render(cx);
        assert!(!trek.visible(cx, "palette"), "jumping closes the palette");
        let opened = trek.read(cx, |ws, _| ws.current_thread().map(|t| t.title.clone()));
        assert_eq!(opened.as_deref(), Some(newest[2].as_str()));

        // While searching there are no numbers: ⌘2 leaves the palette as it is.
        trek.press(cx, "secondary-k");
        trek.type_text(cx, "Thread 1");
        trek.render(cx);
        trek.press(cx, "secondary-2");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        assert_eq!(trek.read(cx, |ws, _| ws.current_thread().map(|t| t.title.clone())).as_deref(), Some(newest[2].as_str()));
    });
}

/// What a query lists, in the palette's order.
fn listed_for(trek: &Trek, cx: &mut TestAppContext, query: &str) -> Vec<String> {
    trek.press(cx, "secondary-k");
    trek.type_text(cx, query);
    trek.render(cx);
    let labels = rows(trek, cx).into_iter().map(|(label, _)| label).collect();
    trek.press(cx, "escape");
    trek.render(cx);
    labels
}

/// Chooses the row `label` among what `query` lists (arrowing down to it, as a person does).
fn choose(trek: &Trek, cx: &mut TestAppContext, query: &str, label: &str) {
    trek.press(cx, "secondary-k");
    trek.type_text(cx, query);
    trek.render(cx);
    let at = rows(trek, cx).iter().position(|(l, _)| l == label).unwrap_or_else(|| panic!("{query:?} lists {label:?}"));
    for _ in 0..at {
        trek.press(cx, "down");
    }
    trek.press(cx, "enter");
    cx.run_until_parked();
}

/// A thread on screen, for the commands that are about one.
fn thread_on_screen(trek: &Trek, cx: &mut TestAppContext) -> String {
    let id = trek.quiet_thread(cx);
    trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Thread(id.clone()), cx));
    trek.render(cx);
    id
}

/// The commands a Mac's menus have and Windows, with no menu bar, finds here.
#[test]
fn the_palette_has_the_window_commands_a_menu_bar_would() {
    run(async |cx| {
        let trek = open(cx);
        assert!(listed_for(&trek, cx, "minimize").contains(&"Minimize window".to_string()));
        assert!(listed_for(&trek, cx, "close window").contains(&"Close window".to_string()));
        let quit = if cfg!(windows) { "Exit Trek" } else { "Quit Trek" };
        assert!(listed_for(&trek, cx, "quit").contains(&quit.to_string()));
        assert!(listed_for(&trek, cx, "exit").contains(&quit.to_string()));
        // About, Settings and Check for Updates were in the menu too: they were already here.
        assert!(listed_for(&trek, cx, "about").contains(&"Settings: About".to_string()));
        assert!(listed_for(&trek, cx, "settings general").contains(&"Settings: General".to_string()));
        assert!(listed_for(&trek, cx, "check for updates").contains(&"Check for updates".to_string()));
    });
}

#[test]
fn quitting_and_closing_the_window_run_from_the_palette() {
    run(async |cx| {
        let trek = open(cx);
        let heard = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = heard.clone();
        cx.update(|cx| {
            cx.on_action(move |_: &crate::Quit, _| seen.store(true, std::sync::atomic::Ordering::SeqCst));
        });
        choose(&trek, cx, "quit", if cfg!(windows) { "Exit Trek" } else { "Quit Trek" });
        assert!(heard.load(std::sync::atomic::Ordering::SeqCst), "the palette's Quit asked the app to quit");
        choose(&trek, cx, "close window", "Close window");
        assert!(cx.update(|cx| cx.windows().is_empty()), "the window closed");
    });
}

#[test]
fn a_command_about_a_thread_is_offered_with_a_thread_on_screen() {
    run(async |cx| {
        let trek = open(cx);
        assert!(!listed_for(&trek, cx, "new window").contains(&"Open thread in new window".to_string()), "no thread, nothing to open");
        trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Basecamp, cx));
        assert!(!listed_for(&trek, cx, "plan mode").contains(&"Plan mode".to_string()), "plan mode is the composer's: Basecamp has none");
        thread_on_screen(&trek, cx);
        assert!(listed_for(&trek, cx, "new window").contains(&"Open thread in new window".to_string()));
        assert!(listed_for(&trek, cx, "plan").contains(&"Plan mode".to_string()));
        let windows = cx.update(|cx| cx.windows().len());
        choose(&trek, cx, "new window", "Open thread in new window");
        assert_eq!(cx.update(|cx| cx.windows().len()), windows + 1, "the thread opened in a window of its own");
    });
}

#[test]
fn plan_mode_toggles_from_the_palette() {
    run(async |cx| {
        let trek = open(cx);
        thread_on_screen(&trek, cx);
        let plan = |trek: &Trek, cx: &TestAppContext| trek.read(cx, |ws, _| ws.prefs_in(&ws.focused_scope()).plan);
        let before = plan(&trek, cx);
        choose(&trek, cx, "plan mode", "Plan mode");
        assert_eq!(plan(&trek, cx), !before, "the composer's plan switch flipped");
        choose(&trek, cx, "plan mode", "Plan mode");
        assert_eq!(plan(&trek, cx), before, "and flips back");
    });
}

#[test]
fn stop_agent_is_offered_while_a_turn_runs_and_stops_it() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.navigate(crate::workspace::Route::Thread(id), cx));
        assert!(!listed_for(&trek, cx, "stop agent").contains(&"Stop agent".to_string()), "nothing is running");
        let id = trek.send(cx, "permission");
        trek.wait_needs_you(cx, &id).await;
        assert!(listed_for(&trek, cx, "stop agent").contains(&"Stop agent".to_string()), "a turn is running");
        choose(&trek, cx, "stop agent", "Stop agent");
        trek.wait_done(cx, &id, trek_core::RunState::Idle).await;
        assert!(matches!(trek.items(cx, &id).last(), Some(trek_core::store::Item::Notice { text }) if text == "Interrupted"));
    });
}
