//! A Win32 call that has Windows call a window back at once (`WindowFromPoint`'s `WM_NCHITTEST`,
//! `ShowWindow`'s `WM_SIZE`) is made after the update that wanted it, never inside: GPUI hears
//! the callback by updating the window, which it can't while the app is being updated, and drops
//! it ("RefCell already borrowed" in the log). The test platform has no Win32 behind it, so these
//! stand in for the callback with what GPUI does on hearing it: an update of the window, made from
//! where the call is.

use super::harness::*;
use gpui_kit::{AnyWindowHandle, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;

/// What Windows does on a call about a window of this thread: GPUI's callback updates the window
/// there and then. Whether each such update ran.
fn windows_calls_back(window: AnyWindowHandle, cx: &TestAppContext) -> (Rc<dyn Fn()>, Rc<RefCell<Vec<bool>>>) {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let async_cx = cx.to_async();
    let log = heard.clone();
    (Rc::new(move || log.borrow_mut().push(window.update(&mut async_cx.clone(), |_, _, _| ()).is_ok())), heard)
}

/// `system::after_update` runs outside the update that asked, where the window hears back;
/// `cx.defer`, which `activate_window` used for `ShowWindow` and `SetForegroundWindow`, runs at
/// the end of that update, still inside it, where it can't (the restored window's new size and
/// the activation's key state were dropped).
#[test]
fn after_update_is_outside_the_update_and_defer_is_not() {
    run(async |cx| {
        let trek = open(cx);
        let (call_back, heard) = windows_calls_back(trek.window, cx);
        trek.window(cx, |_, cx| {
            let c = call_back.clone();
            crate::system::after_update(cx, move |_| c());
            let c = call_back.clone();
            cx.defer(move |_| c());
        });
        cx.run_until_parked();
        assert_eq!(*heard.borrow(), [false, true], "defer: inside the update (refused); after_update: after it (heard)");
    });
}

/// And what the log says when an update is refused: once per place, with where.
#[test]
fn a_refused_update_is_logged_once_with_where() {
    run(async |cx| {
        let trek = open(cx);
        let mut async_cx = cx.to_async();
        let window = trek.window;
        let said = crate::logging::tests::captured(|| {
            trek.window(cx, |_, _| {
                for _ in 0..3 {
                    assert!(crate::logging::ran(window.update(&mut async_cx, |_, _, _| ())).is_none());
                }
            });
        });
        assert_eq!(said.matches("didn't run").count(), 1, "{said}");
        assert!(said.contains(&format!("{}:", file!())) && said.contains("RefCell already borrowed"), "{said}");
        // Outside an update it runs, and nothing is said.
        let said = crate::logging::tests::captured(|| assert_eq!(crate::logging::ran(window.update(&mut async_cx, |_, _, _| 7)), Some(7)));
        assert_eq!(said, "");
    });
}
