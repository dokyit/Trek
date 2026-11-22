//! A toast about something that went wrong looks different from one that didn't, and stays up
//! longer.

mod common;

use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::time::Duration;
use trek_app::workspace::WorkspaceEvent;

fn toasts(trek: &Trek, cx: &mut TestAppContext) -> usize {
    trek.window(cx, |window, cx| trek_app::toast::count(window, cx))
}

/// Let a toast's entrance (real time, 400 ms) finish.
fn settled_in(trek: &Trek, cx: &mut TestAppContext) {
    std::thread::sleep(Duration::from_millis(450));
    trek.render(cx);
}

#[test]
fn a_failure_toast_has_the_error_face_and_outlasts_an_info_one() {
    run(async |cx| {
        let trek = open(cx);
        cx.update(|cx| trek_app::root::init(trek.ws.clone(), cx));
        trek.window(cx, |window, _| window.activate_window());
        cx.run_until_parked();

        trek.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message: "Image copied".into(),
                undo: None,
            })
        });
        settled_in(&trek, cx);
        assert_eq!(toasts(&trek, cx), 1);
        assert!(!trek.visible(cx, "toast-error"), "a note has no error icon");

        trek.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Failure {
                message: "Couldn't fork: no such thread".into(),
            })
        });
        settled_in(&trek, cx);
        assert_eq!(toasts(&trek, cx), 2);
        assert!(trek.visible(cx, "toast-error"), "a failure has one");

        // The note goes after its five seconds; the failure is still there at six.
        cx.executor().advance_clock(Duration::from_secs(6));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(300));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(toasts(&trek, cx), 1, "only the failure is left");
        assert!(trek.visible(cx, "toast-error"));

        // And it goes too, once its longer time is up.
        cx.executor().advance_clock(Duration::from_secs(8));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(300));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(toasts(&trek, cx), 0);
    });
}

/// A save that fails (a full disk, a data folder Trek can't write) is told of in plain words,
/// once in a while rather than on every streamed word.
#[test]
fn a_failed_save_is_told_once_in_a_while() {
    run(async |cx| {
        let trek = open(cx);
        let said = common::toasts(&trek, cx);
        trek.update(cx, |ws, cx| {
            ws.save_failed(&"database or disk is full", cx);
            ws.save_failed(&"database or disk is full", cx);
        });
        let said = said.borrow();
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(
            said[0].0,
            "Trek couldn't save your work: the disk is full. Free some space and Trek saves again as you go."
        );
        let read_only =
            trek_app::workspace::save_failure_message("attempt to write a readonly database");
        assert!(
            read_only
                .starts_with("Trek couldn't save your work: it can't write to its data folder"),
            "{read_only}"
        );
        let other = trek_app::workspace::save_failure_message("disk I/O error\ncaused by: x");
        assert_eq!(
            other,
            "Trek couldn't save your work: disk I/O error. Until it can, what changes now may be gone after a restart."
        );
    });
}

/// The dialog for a database Trek couldn't open names the file and says what to do when it's
/// damaged, not only when another Trek holds it.
#[test]
fn a_database_that_wont_open_says_where_it_is() {
    let file = std::path::Path::new("/Volumes/data/trek.db");
    let said = trek_app::root::database_error_detail("file is not a database", file);
    assert!(said.starts_with("file is not a database\n\n"), "{said}");
    assert!(said.contains("close any other copy of Trek"), "{said}");
    assert!(said.contains("trek.db."), "{said}");
    assert!(said.contains("may be damaged"), "{said}");
}
