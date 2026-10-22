//! Input for one window of another app while the user works in theirs: the pointer stays where
//! they have it and their front app keeps the keyboard. macOS has no public way to do this, so it
//! takes three of the window server's private calls (SkyLight), looked up when first needed;
//! without them the public per-process post is used, which fewer apps accept.
//!
//! - **Key window, in its own app.** An app that isn't active treats a click as one to activate
//!   it and drops it, and Chromium drops posted keys too. Two event records sent to the target
//!   process alone (`SLPSPostEventRecordTo`) make the window key inside its app without
//!   activating the app or raising the window: a focus record (type `0x0d`, `0x8a` = 1) and the
//!   make-key pair (types 1 and 2). Nothing goes to the user's app, so it keeps its key window.
//! - **Posted to the process** (`SLEventPostToPid`): the public `CGEventPostToPid` skips the
//!   window server's activity bookkeeping, and Chromium ignores what it delivers.
//! - **Stamped for the window**: its number in the routing fields (51, 91, 92), the target's pid
//!   (40), the window-local point (`CGEventSetWindowLocation`), a touch subtype (7) and one
//!   gesture id across a click's events (58). A move first, then a click outside every window,
//!   satisfy Chromium's gate on user activation before the real click.
//!
//! Measured on macOS 27 with TextEdit and Chrome (the caret lands where clicked, a web button
//! counts the click, keys reach a web field); the record layout and fields follow cua-driver
//! (MIT, github.com/trycua/cua), which found them.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CGMouseButton, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use foreign_types::ForeignType as _;

use super::{Button, Input, WindowInfo};
use crate::keys::KeyCombo;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
}

unsafe extern "C" {
    fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
}
const RTLD_NOW: i32 = 0x2;
const RTLD_GLOBAL: i32 = 0x8;
/// `RTLD_DEFAULT`: every image loaded so far.
const ANY: *mut c_void = -2isize as *mut c_void;

type PostToPid = unsafe extern "C" fn(i32, *mut c_void);
type SetIntegerField = unsafe extern "C" fn(*mut c_void, u32, i64);
/// A `CGPoint` passed by value is two doubles, in the same registers as two `f64` arguments.
type SetWindowLocation = unsafe extern "C" fn(*mut c_void, f64, f64);
type PostEventRecordTo = unsafe extern "C" fn(*const c_void, *const u8) -> i32;
type ProcessForPid = unsafe extern "C" fn(i32, *mut c_void) -> i32;

struct Calls {
    post: Option<PostToPid>,
    set_field: Option<SetIntegerField>,
    set_window_location: Option<SetWindowLocation>,
    post_record: Option<PostEventRecordTo>,
    process_for_pid: Option<ProcessForPid>,
}

fn calls() -> &'static Calls {
    static CALLS: OnceLock<Calls> = OnceLock::new();
    CALLS.get_or_init(|| {
        let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
        // SAFETY: loading a system framework by its absolute path; the handle is never closed.
        unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        // SAFETY: each symbol is cast to the signature the window server's own callers use.
        unsafe {
            Calls {
                post: find(c"SLEventPostToPid")
                    .map(|p| std::mem::transmute::<*mut c_void, PostToPid>(p)),
                set_field: find(c"SLEventSetIntegerValueField")
                    .map(|p| std::mem::transmute::<*mut c_void, SetIntegerField>(p)),
                set_window_location: find(c"CGEventSetWindowLocation")
                    .map(|p| std::mem::transmute::<*mut c_void, SetWindowLocation>(p)),
                post_record: find(c"SLPSPostEventRecordTo")
                    .map(|p| std::mem::transmute::<*mut c_void, PostEventRecordTo>(p)),
                process_for_pid: find(c"GetProcessForPID")
                    .map(|p| std::mem::transmute::<*mut c_void, ProcessForPid>(p)),
            }
        }
    })
}

fn find(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: a lookup by name in what's loaded; null when it isn't there.
    let p = unsafe { dlsym(ANY, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// The routing fields, numbered as the window server numbers them.
mod field {
    pub const PHASE: u32 = 0;
    pub const CLICK_STATE: u32 = 1;
    pub const BUTTON: u32 = 3;
    pub const SUBTYPE: u32 = 7;
    pub const TARGET_PID: u32 = 40;
    pub const WINDOW: u32 = 51;
    pub const GESTURE: u32 = 58;
    pub const WINDOW_UNDER_POINTER: u32 = 91;
    pub const WINDOW_THAT_CAN_HANDLE: u32 = 92;
}

/// `NSEventSubtypeTouch`.
const TOUCH: i64 = 3;
/// A point outside every window, for the primer click.
const NOWHERE: (f64, f64) = (-1.0, -1.0);

fn set(event: &CGEvent, f: u32, value: i64) {
    let ptr = event.as_ptr() as *mut c_void;
    match calls().set_field {
        // SAFETY: `ptr` is a live CGEvent.
        Some(set) => unsafe { set(ptr, f, value) },
        None => event.set_integer_value_field(f, value),
    }
}

/// Hand `event` to process `pid`.
fn post(pid: i32, event: &CGEvent) {
    match calls().post {
        // SAFETY: `event` is a live CGEvent; the call retains what it keeps.
        Some(post) => unsafe { post(pid, event.as_ptr() as *mut c_void) },
        None => event.post_to_pid(pid),
    }
}

/// Make window `wid` the key window of its app `pid`, without activating the app or raising the
/// window; then give the app a moment to act on it. Best effort: without the private call, or
/// when the window server won't take the records, events are still posted.
fn make_key(pid: i32, wid: u32) {
    let c = calls();
    let (Some(post_record), Some(process_for_pid)) = (c.post_record, c.process_for_pid) else {
        return;
    };
    let mut psn = [0u8; 8];
    // SAFETY: `psn` is the 8-byte ProcessSerialNumber the call fills in.
    if unsafe { process_for_pid(pid, psn.as_mut_ptr().cast()) } != 0 {
        return;
    }
    let mut focus = record(wid);
    focus[0x08] = 0x0d;
    focus[0x8a] = 0x01;
    // SAFETY: a 248-byte record and the PSN it goes to, as the window server expects them.
    unsafe { post_record(psn.as_ptr().cast(), focus.as_ptr()) };
    for kind in [0x01, 0x02] {
        let mut key = record(wid);
        key[0x08] = kind;
        key[0x3a] = 0x10;
        key[0x20..0x30].fill(0xff);
        // SAFETY: as above.
        unsafe { post_record(psn.as_ptr().cast(), key.as_ptr()) };
    }
    std::thread::sleep(Duration::from_millis(50));
}

/// An empty event record for window `wid`.
fn record(wid: u32) -> [u8; 0xf8] {
    let mut r = [0u8; 0xf8];
    r[0x04] = 0xf8;
    r[0x3c..0x40].copy_from_slice(&wid.to_le_bytes());
    r
}

fn sleep_ms(ms: u64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Mouse events for one window, stamped alike.
struct Pointer<'a> {
    w: &'a WindowInfo,
    pid: i32,
    source: CGEventSource,
    gesture: i64,
}

impl<'a> Pointer<'a> {
    fn new(w: &'a WindowInfo) -> Result<Self, String> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "Failed to create CGEventSource".to_string())?;
        let gesture = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i64;
        Ok(Self {
            w,
            pid: w.pid as i32,
            source,
            gesture,
        })
    }

    /// Post one event at `local` (the window's own coordinates; `NOWHERE` stays outside it).
    fn send(
        &self,
        ty: CGEventType,
        local: (f64, f64),
        button: Button,
        phase: i64,
        click_state: i64,
    ) -> Result<(), String> {
        let screen = if local == NOWHERE {
            NOWHERE
        } else {
            (self.w.x + local.0, self.w.y + local.1)
        };
        let cg_button = if button == Button::Right {
            CGMouseButton::Right
        } else {
            CGMouseButton::Left
        };
        let event = CGEvent::new_mouse_event(
            self.source.clone(),
            ty,
            CGPoint::new(screen.0, screen.1),
            cg_button,
        )
        .map_err(|_| "Failed to create mouse event".to_string())?;
        self.stamp(&event, local, phase);
        set(&event, field::CLICK_STATE, click_state);
        set(
            &event,
            field::BUTTON,
            if button == Button::Right { 1 } else { 0 },
        );
        post(self.pid, &event);
        Ok(())
    }

    fn stamp(&self, event: &CGEvent, local: (f64, f64), phase: i64) {
        // No modifier the user is holding rides along: a held Control would make it a right-click.
        event.set_flags(CGEventFlags::empty());
        let wid = self.w.id;
        set(event, field::PHASE, phase);
        set(event, field::SUBTYPE, TOUCH);
        set(event, field::TARGET_PID, self.w.pid);
        set(event, field::WINDOW, wid);
        set(event, field::WINDOW_UNDER_POINTER, wid);
        set(event, field::WINDOW_THAT_CAN_HANDLE, wid);
        set(event, field::GESTURE, self.gesture);
        if let Some(locate) = calls().set_window_location {
            // SAFETY: a live CGEvent.
            unsafe { locate(event.as_ptr() as *mut c_void, local.0, local.1) };
        }
    }

    fn hover(&self, at: (f64, f64)) -> Result<(), String> {
        self.send(CGEventType::MouseMoved, at, Button::Left, 2, 0)
    }

    /// A left click outside every window: Chromium counts it as the user's activation and
    /// does nothing with it.
    fn primer(&self) -> Result<(), String> {
        self.send(CGEventType::LeftMouseDown, NOWHERE, Button::Left, 1, 1)?;
        sleep_ms(1);
        self.send(CGEventType::LeftMouseUp, NOWHERE, Button::Left, 2, 1)?;
        sleep_ms(100);
        Ok(())
    }
}

/// Deliver `input` to window `w` of another app, in the background.
pub fn deliver(w: &WindowInfo, input: &Input) -> Result<(), String> {
    let pid = w.pid as i32;
    let wid = u32::try_from(w.id).unwrap_or(0);
    match input {
        Input::Type(_) | Input::Key(_) => {
            if wid != 0 {
                make_key(pid, wid);
            }
            match input {
                Input::Type(text) => type_text(pid, text),
                Input::Key(combo) => press(pid, *combo),
                _ => unreachable!(),
            }
        }
        _ if wid == 0 => Err("That app has no window to click in yet.".into()),
        Input::Move(x, y) => Pointer::new(w)?.hover((*x, *y)),
        Input::Click {
            x,
            y,
            button,
            count,
        } => {
            make_key(pid, wid);
            let p = Pointer::new(w)?;
            let at = (*x, *y);
            p.hover(at)?;
            sleep_ms(15);
            p.primer()?;
            let (down, up) = match button {
                Button::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp),
                Button::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp),
            };
            for n in 1..=(*count).clamp(1, 3) as i64 {
                p.send(down, at, *button, 3, n)?;
                sleep_ms(1);
                p.send(up, at, *button, 3, n)?;
                if n < *count as i64 {
                    sleep_ms(80);
                }
            }
            Ok(())
        }
        Input::Drag { from, to } => {
            make_key(pid, wid);
            let p = Pointer::new(w)?;
            p.hover(*from)?;
            sleep_ms(15);
            p.primer()?;
            p.send(CGEventType::LeftMouseDown, *from, Button::Left, 3, 1)?;
            sleep_ms(60);
            const STEPS: i32 = 24;
            for i in 1..=STEPS {
                let t = i as f64 / STEPS as f64;
                let at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                p.send(CGEventType::LeftMouseDragged, at, Button::Left, 3, 1)?;
                sleep_ms(12);
            }
            sleep_ms(40);
            p.send(CGEventType::LeftMouseUp, *to, Button::Left, 3, 1)
        }
        Input::Scroll { x, y, dx, dy } => {
            let p = Pointer::new(w)?;
            let at = (*x, *y);
            p.hover(at)?;
            sleep_ms(15);
            // CGEvent wheel deltas are positive for up/left; ours are positive for down/right.
            let event =
                CGEvent::new_scroll_event(p.source.clone(), ScrollEventUnit::LINE, 2, -dy, -dx, 0)
                    .map_err(|_| "Failed to create scroll event".to_string())?;
            // SAFETY: a live CGEvent.
            unsafe {
                CGEventSetLocation(
                    event.as_ptr() as *mut c_void,
                    CGPoint::new(w.x + at.0, w.y + at.1),
                )
            };
            p.stamp(&event, at, 0);
            post(pid, &event);
            Ok(())
        }
    }
}

fn key_source() -> Result<CGEventSource, String> {
    // Private: what the user holds on their keyboard doesn't mix into these.
    CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| "Failed to create CGEventSource".to_string())
}

fn press(pid: i32, combo: KeyCombo) -> Result<(), String> {
    let source = key_source()?;
    let flags = CGEventFlags::from_bits_truncate(combo.flags);
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), combo.keycode, down)
            .map_err(|_| "Failed to create keyboard event".to_string())?;
        event.set_flags(flags);
        event.post_to_pid(pid);
        sleep_ms(if down { 15 } else { 0 });
    }
    Ok(())
}

fn type_text(pid: i32, text: &str) -> Result<(), String> {
    let source = key_source()?;
    for segment in super::macos::split_for_typing(text) {
        match segment {
            super::macos::TypeSegment::Key(code) => press(
                pid,
                KeyCombo {
                    keycode: code,
                    flags: 0,
                },
            )?,
            super::macos::TypeSegment::Text(chunk) => {
                for down in [true, false] {
                    let event = CGEvent::new_keyboard_event(source.clone(), 0, down)
                        .map_err(|_| "Failed to create keyboard event".to_string())?;
                    event.set_flags(CGEventFlags::empty());
                    event.set_string_from_utf16_unchecked(&chunk);
                    event.post_to_pid(pid);
                }
            }
        }
        sleep_ms(8);
    }
    Ok(())
}
