//! Trek: a second after a window's last frame, its Metal layer goes back to two drawables and lets
//! its spare ones go (see `BUSY_FRAMES` in metal_renderer.rs), through libdispatch's main queue.
//! A lower `maximumDrawableCount` alone keeps the drawables already made; a change of
//! `drawableSize` drops them, all but the one on screen, and the next frame makes what it needs.
//! At 1600 x 1000 points: 97 MB at rest and 122 MB after a stream, against 135 and 171 before.
//! Kept out of metal_renderer.rs, whose items cbindgen reads into the shaders' header.

use objc::{msg_send, sel, sel_impl};
use std::{cell::Cell, ffi::c_void};

/// Trek: how long after its last frame a window that drew with three drawables goes back to two.
pub(crate) const IDLE_TRIM_NS: i64 = 1_000_000_000;

/// Trek: what the renderer and its idle check share (on the main thread, which draws).
#[derive(Default)]
pub(crate) struct Drawables {
    /// Frames drawn so far.
    pub(crate) frames: Cell<u64>,
    /// How many drawables the layer may keep now.
    pub(crate) count: Cell<u64>,
    /// An idle check is waiting to run.
    pub(crate) pending: Cell<bool>,
}

/// Trek: one idle check: the layer (retained) and the frame it was set after.
pub(crate) struct IdleCheck {
    pub(crate) shared: std::rc::Rc<Drawables>,
    pub(crate) layer: *mut objc::runtime::Object,
    pub(crate) frame: u64,
}

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// Trek: check again in `IDLE_TRIM_NS` whether the window drew since `check.frame`.
pub(crate) fn schedule_idle_check(check: IdleCheck) {
    let context = Box::into_raw(Box::new(check)) as *mut c_void;
    unsafe {
        dispatch_after_f(
            dispatch_time(0, IDLE_TRIM_NS),
            &raw const _dispatch_main_q,
            context,
            idle_check,
        );
    }
}

/// Trek: on the main queue. No frame since the check was set: two drawables, which lets the layer
/// free its third. Still drawing: look again later.
extern "C" fn idle_check(context: *mut c_void) {
    let check = unsafe { Box::from_raw(context as *mut IdleCheck) };
    let frames = check.shared.frames.get();
    if frames == check.frame {
        unsafe {
            let _: () = msg_send![check.layer, setMaximumDrawableCount: 2u64];
            // A lower count alone keeps the drawables the layer has: a new size lets them go (the
            // next frame makes what it needs at the real size).
            #[allow(deprecated)]
            let size: cocoa::foundation::NSSize = msg_send![check.layer, drawableSize];
            #[allow(deprecated)]
            let other = cocoa::foundation::NSSize::new(size.width, size.height + 1.0);
            let _: () = msg_send![check.layer, setDrawableSize: other];
            let _: () = msg_send![check.layer, setDrawableSize: size];
            let _: () = msg_send![check.layer, release];
        }
        check.shared.count.set(2);
        check.shared.pending.set(false);
    } else {
        schedule_idle_check(IdleCheck { frame: frames, ..*check });
    }
}
