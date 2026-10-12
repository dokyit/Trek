//! A Win32 call that has Windows call a window back at once (`WindowFromPoint`'s `WM_NCHITTEST`,
//! `ShowWindow`'s `WM_SIZE`) is made after the update that wanted it, never inside: GPUI hears
//! the callback by updating the window, which it can't while the app is being updated, and drops
//! it ("RefCell already borrowed" in the log). The test platform has no Win32 behind it, so these
//! stand in for the callback with what GPUI does on hearing it: an update of the window, made from
//! where the call is.

use super::harness::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, PlatformInput, TestAppContext, point};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What Windows does on a call about a window of this thread: GPUI's callback updates the window
/// there and then. Whether each such update ran.
fn windows_calls_back(window: AnyWindowHandle, cx: &TestAppContext) -> (Rc<dyn Fn()>, Rc<RefCell<Vec<bool>>>) {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let async_cx = cx.to_async();
    let log = heard.clone();
    (Rc::new(move || log.borrow_mut().push(window.update(&mut async_cx.clone(), |_, _, _| ()).is_ok())), heard)
}

/// The tests' answer to "where is the cursor", until dropped.
struct Cursor;

impl Cursor {
    fn answers(ask: impl Fn() -> Option<bool> + 'static) -> Cursor {
        crate::chrome::CURSOR_IN_TESTS.with(|c| *c.borrow_mut() = Some(Rc::new(ask)));
        Cursor
    }
}

impl Drop for Cursor {
    fn drop(&mut self) {
        crate::chrome::CURSOR_IN_TESTS.with(|c| *c.borrow_mut() = None);
    }
}

/// The pointer in the middle of the window, which hasn't been told it's over it (the test
/// platform never says so): as after Windows' `WM_MOUSELEAVE`, or a window that came up under a
/// cursor that hasn't moved since.
fn pointer_in_the_middle(trek: &Trek, cx: &mut TestAppContext) {
    trek.window(cx, |window, cx| {
        let at = point(window.viewport_size().width / 2., window.viewport_size().height / 2.);
        window.dispatch_event(PlatformInput::MouseMove(gpui_kit::MouseMoveEvent { position: at, pressed_button: None, modifiers: Default::default() }), cx)
    });
    cx.run_until_parked();
}

/// Beta 2 and 3 logged "RefCell already borrowed" by the burst: the window asked where the cursor
/// was (`WindowFromPoint`) as it was drawn, and Windows asked the window under it, Trek's own,
/// which couldn't answer mid-draw. Asked after the draw, the window hears it, and a cursor over
/// another window still lets the pointer go.
#[test]
fn the_cursor_is_asked_where_after_the_draw() {
    run(async |cx| {
        let trek = open(cx);
        // What opening asked is answered first (by no one: the test platform has no cursor).
        cx.run_until_parked();
        let (call_back, heard) = windows_calls_back(trek.window, cx);
        let asked = Rc::new(Cell::new(0));
        let a = asked.clone();
        let _cursor = Cursor::answers(move || {
            a.set(a.get() + 1);
            call_back();
            Some(false)
        });
        pointer_in_the_middle(&trek, cx);
        trek.render(cx);
        cx.run_until_parked();
        assert!(!heard.borrow().is_empty() && heard.borrow().iter().all(|ran| *ran), "every callback reached the window: {:?}", heard.borrow());
        assert_eq!(asked.get(), 1, "the system was asked once");
        assert!(!trek.window(cx, |window, _| crate::chrome::pointer_inside(window)), "the cursor is over another window: the pointer was let go");
    });
}

/// A cursor over the window itself (its frame, or a web view in it) keeps the pointer, and the
/// window is asked once per draw at most, never twice at once.
#[test]
fn a_cursor_over_the_window_keeps_the_pointer() {
    run(async |cx| {
        let trek = open(cx);
        cx.run_until_parked();
        let (call_back, heard) = windows_calls_back(trek.window, cx);
        let asked = Rc::new(Cell::new(0));
        let a = asked.clone();
        let _cursor = Cursor::answers(move || {
            a.set(a.get() + 1);
            call_back();
            Some(true)
        });
        pointer_in_the_middle(&trek, cx);
        // Two draws before the answer comes: one question.
        trek.window(cx, |window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        cx.run_until_parked();
        assert!(heard.borrow().iter().all(|ran| *ran), "every callback reached the window: {:?}", heard.borrow());
        assert_eq!(asked.get(), 1, "one question out at a time");
        assert!(trek.window(cx, |window, _| crate::chrome::pointer_inside(window)), "the pointer stays: the cursor is over this window");
        // The next draw asks again.
        trek.render(cx);
        cx.run_until_parked();
        assert_eq!(asked.get(), 2);
    });
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
