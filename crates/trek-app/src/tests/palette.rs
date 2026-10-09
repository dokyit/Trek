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
        trek.press(cx, "cmd-k");
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
        trek.press(cx, "cmd-k");
        trek.render(cx);
        trek.press(cx, "cmd-3");
        trek.render(cx);
        assert!(!trek.visible(cx, "palette"), "jumping closes the palette");
        let opened = trek.read(cx, |ws, _| ws.current_thread().map(|t| t.title.clone()));
        assert_eq!(opened.as_deref(), Some(newest[2].as_str()));

        // While searching there are no numbers: ⌘2 leaves the palette as it is.
        trek.press(cx, "cmd-k");
        trek.type_text(cx, "Thread 1");
        trek.render(cx);
        trek.press(cx, "cmd-2");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        assert_eq!(trek.read(cx, |ws, _| ws.current_thread().map(|t| t.title.clone())).as_deref(), Some(newest[2].as_str()));
    });
}
