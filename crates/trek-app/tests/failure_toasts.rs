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
