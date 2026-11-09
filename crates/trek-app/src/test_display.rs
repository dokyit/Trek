//! The "Trek Test Display": an invisible extra display the isolated test app's windows live on.
//!
//! An agent's test run (`trek-dev`) shows no Dock icon, takes no focus and is clicked from
//! inside (`shots.rs`). Its window still sat on the owner's screen, behind their windows, where
//! macOS stops drawing it while it's covered and a capture of real pixels means showing it to
//! them. With `TREK_TEST_DISPLAY=1` a `shots` build on macOS makes a display of its own before it
//! opens any window, and every window opens there: nothing of the run is on the owner's screens,
//! and macOS still counts the windows as visible.
//!
//! The display is a `CGVirtualDisplay`: private CoreGraphics classes, reached by name, as
//! Chromium's tests reach them (`ui/display/mac/test/virtual_display_util_mac.mm`). It exists
//! while this process holds the object, so it goes when the process exits, crashes or is killed.
//! The shipped app has none of this compiled in.
//!
//! It must not get in the owner's way:
//! - It sits diagonally off one corner of the desktop, touching it at that one point at most
//!   (CoreGraphics closes any gap in a layout, so no contact at all isn't on offer). A pointer
//!   leaves a display along a shared edge; here there is none. `plan::place` picks the corner:
//!   not one of their hot corners, far from the main display, and for a second test app the far
//!   corner of the first one's display, off the real desktop altogether.
//! - macOS puts a new display beside the main one, along its edge; it's moved to its corner at
//!   once, before anything else is asked of it (a third of a second, while CoreGraphics
//!   applies the change).
//! - It's placed for this app only (`kCGConfigureForAppOnly`): Trek saves nothing to their
//!   display settings, their displays keep their arrangement, mode and main display, and
//!   nothing is mirrored. macOS itself notes each set of displays it has seen, as it found
//!   them, in `/Library/Preferences/com.apple.windowserver.displays.plist` (as it does a
//!   monitor plugged in once); a few fixed serial numbers keep that to a few entries. A
//!   placement "for the session" (`kCGConfigureForSession`) is written there too on macOS 27,
//!   whatever its name says, so it isn't used.
//! - On a quit, Trek's windows close before the display goes (`end`), so none lands on the
//!   owner's screen on its way out. Killed, the windows and the display go together.
//! - Any change to the displays (another test app's display coming or going, a monitor
//!   plugged in) has macOS drop the app's placement and put the display back beside the main
//!   one. `keep` looks whenever the displays change, and once a second: a display that shares
//!   an edge with the desktop is placed again (within a quarter of a second), and a window
//!   that turned up on a real display (a file panel, one macOS moved) is put back.
//!
//! Each test app has a display of its own, since a display dies with the process that made it
//! and windows sharing one would cover each other: "Trek Test Display", "Trek Test Display 2",
//! and so on, in the first free slot.
//!
//! If the classes are missing or anything fails, the reason is logged (and written to
//! `$TREK_SHOT_DIR/display.json`, where `trek-dev status` reads it) and the run carries on as it
//! always has, on the owner's screen behind their windows.

use gpui_kit::{App, Bounds, DisplayId, Pixels, PlatformDisplay, Size, WindowBounds};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Whether this build can make a display at all: `shots` builds on macOS only.
pub const BUILT_IN: bool = cfg!(all(feature = "shots", target_os = "macos"));

/// The test display's id while this process holds one; 0 otherwise (and always, in a build
/// that can't make one).
static HELD: AtomicU32 = AtomicU32::new(0);

/// The display Trek's windows open on in this run: the test display, when there is one. `None`
/// everywhere else, where windows choose as they always have.
pub fn id() -> Option<DisplayId> {
    match HELD.load(Ordering::Relaxed) {
        0 => None,
        id => Some(DisplayId::new(id as u64)),
    }
}

/// Where a window of `size` opens: centred on the test display, or on the primary one as before.
pub fn centered(size: Size<Pixels>, cx: &App) -> (Option<DisplayId>, WindowBounds) {
    let display = id();
    (
        display,
        WindowBounds::Windowed(Bounds::centered(display, size, cx)),
    )
}

/// The display a small floating window places itself on: the test display, else the primary.
pub fn display(cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
    id().and_then(|id| cx.find_display(id))
        .or_else(|| cx.primary_display())
}

#[cfg(all(feature = "shots", target_os = "macos"))]
pub use mac::{end, start, watch, windows, windows_up};

/// Let the display go, before the app ends; whether there was one. Nothing to do in a build
/// that can't make one.
#[cfg(not(all(feature = "shots", target_os = "macos")))]
pub fn end() -> bool {
    false
}

/// Whether a window of the app is still on the test display, closing. Never, in a build that
/// can't make one.
#[cfg(not(all(feature = "shots", target_os = "macos")))]
pub fn windows_up() -> bool {
    false
}

/// Where the display goes and whether there is one: no system calls, so it can be tested.
#[cfg(any(test, all(feature = "shots", target_os = "macos")))]
#[cfg_attr(not(all(feature = "shots", target_os = "macos")), allow(dead_code))]
pub(crate) mod plan {
    use std::cmp::Reverse;

    /// A display's rectangle in the global display space: points, y down, the main display's
    /// top-left corner at the origin.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Rect {
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
    }

    impl Rect {
        pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
            Self { x, y, w, h }
        }

        fn right(&self) -> i32 {
            self.x + self.w
        }

        fn bottom(&self) -> i32 {
            self.y + self.h
        }

        /// The two cover some of the same area.
        pub fn overlaps(&self, other: &Rect) -> bool {
            self.x < other.right()
                && other.x < self.right()
                && self.y < other.bottom()
                && other.y < self.bottom()
        }

        /// The two share a stretch of edge: a pointer crosses from one to the other anywhere
        /// along it. Meeting at a corner alone isn't one.
        pub fn shares_edge(&self, other: &Rect) -> bool {
            let beside = self.right() == other.x || other.right() == self.x;
            let stacked = self.bottom() == other.y || other.bottom() == self.y;
            let rows = self.y.max(other.y) < self.bottom().min(other.bottom());
            let columns = self.x.max(other.x) < self.right().min(other.right());
            (beside && rows) || (stacked && columns)
        }

        /// The corner of `other` this one sits diagonally outside of, touching only that point.
        pub fn corner_met(&self, other: &Rect) -> Option<Corner> {
            let left = self.right() == other.x;
            let right = self.x == other.right();
            let above = self.bottom() == other.y;
            let below = self.y == other.bottom();
            match (left, right, above, below) {
                (true, _, _, true) => Some(Corner::BottomLeft),
                (_, true, _, true) => Some(Corner::BottomRight),
                (_, true, true, _) => Some(Corner::TopRight),
                (true, _, true, _) => Some(Corner::TopLeft),
                _ => None,
            }
        }

        /// Twice the centre (to stay in whole numbers).
        fn centre2(&self) -> (i64, i64) {
            (
                2 * self.x as i64 + self.w as i64,
                2 * self.y as i64 + self.h as i64,
            )
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Corner {
        BottomLeft,
        BottomRight,
        TopRight,
        TopLeft,
    }

    impl Corner {
        /// Most wanted first, other things equal: with the Dock along the bottom nothing is
        /// aimed at the bottom-left corner; the Apple menu is in the top-left one.
        pub const ALL: [Corner; 4] = [
            Corner::BottomLeft,
            Corner::BottomRight,
            Corner::TopRight,
            Corner::TopLeft,
        ];

        /// The origin that puts a display of `size` diagonally outside this corner of `of`.
        pub fn outside(self, of: &Rect, (w, h): (i32, i32)) -> (i32, i32) {
            match self {
                Corner::BottomLeft => (of.x - w, of.bottom()),
                Corner::BottomRight => (of.right(), of.bottom()),
                Corner::TopRight => (of.right(), of.y - h),
                Corner::TopLeft => (of.x - w, of.y - h),
            }
        }

        pub fn name(self) -> &'static str {
            match self {
                Corner::BottomLeft => "bottom-left",
                Corner::BottomRight => "bottom-right",
                Corner::TopRight => "top-right",
                Corner::TopLeft => "top-left",
            }
        }
    }

    /// The corners the owner throws the pointer into: their hot corners.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Hot {
        pub bottom_left: bool,
        pub bottom_right: bool,
        pub top_right: bool,
        pub top_left: bool,
    }

    impl Hot {
        /// From the Dock's `wvous-*-corner` settings, `None` where a key isn't set. 0 and 1 are
        /// "no action". The bottom-right corner opens a Quick Note until it's given something
        /// else to do, so there an unset key counts as hot.
        pub fn from_settings(
            bottom_left: Option<i64>,
            bottom_right: Option<i64>,
            top_right: Option<i64>,
            top_left: Option<i64>,
        ) -> Self {
            let acts = |v: Option<i64>| v.is_some_and(|v| v > 1);
            Self {
                bottom_left: acts(bottom_left),
                bottom_right: bottom_right.is_none_or(|v| v > 1),
                top_right: acts(top_right),
                top_left: acts(top_left),
            }
        }

        pub fn is(&self, corner: Corner) -> bool {
            match corner {
                Corner::BottomLeft => self.bottom_left,
                Corner::BottomRight => self.bottom_right,
                Corner::TopRight => self.top_right,
                Corner::TopLeft => self.top_left,
            }
        }
    }

    /// `display` keeps off the real desktop: it covers none of it and shares no edge with it (a
    /// corner at most), so the owner's pointer can't wander onto it.
    pub fn apart(display: &Rect, real: &[Rect]) -> bool {
        real.iter()
            .all(|r| !display.overlaps(r) && !display.shares_edge(r))
    }

    /// Where a test display of `size` goes: its origin, or `None` when every place is taken.
    ///
    /// `real` are the owner's displays (`main` the one with the menu bar) and `tests` the other
    /// test apps' displays. It sits diagonally outside a corner of one of them, covering no
    /// display and sharing no edge with a real one. Of those places, in order: one that doesn't
    /// touch the real desktop at all (the far corner of another test display), one whose corner
    /// isn't a hot corner, the one farthest from the main display, then `Corner::ALL`'s order.
    pub fn place(
        size: (i32, i32),
        main: &Rect,
        real: &[Rect],
        tests: &[Rect],
        hot: Hot,
    ) -> Option<(i32, i32)> {
        let from = main.centre2();
        let mut places = Vec::new();
        for anchor in tests.iter().chain(real) {
            for (order, corner) in Corner::ALL.into_iter().enumerate() {
                let (x, y) = corner.outside(anchor, size);
                let rect = Rect::new(x, y, size.0, size.1);
                if real.iter().chain(tests).any(|d| rect.overlaps(d)) || !apart(&rect, real) {
                    continue;
                }
                let met: Vec<Corner> = real.iter().filter_map(|d| rect.corner_met(d)).collect();
                let (cx, cy) = rect.centre2();
                let far = (cx - from.0).pow(2) + (cy - from.1).pow(2);
                let key = (
                    !met.is_empty(),
                    met.iter().any(|c| hot.is(*c)),
                    Reverse(far),
                    order,
                );
                places.push((key, (x, y)));
            }
        }
        places.into_iter().min_by_key(|(key, _)| *key).map(|p| p.1)
    }

    /// The smallest display made: room for the default window (1280x820) and the usual larger
    /// ones, at 2x.
    pub const BASE: (u32, u32) = (1920, 1200);
    /// The largest: 7680x4320 pixels at 2x. A window asked for larger than that is cut to the
    /// display, as one larger than the owner's screen always was.
    pub const MOST: (u32, u32) = (3840, 2160);
    /// Room beside the window: the display's own menu bar, and enough around the window that a
    /// capture of the whole display shows its edges and shadow.
    const AROUND: (u32, u32) = (160, 200);

    /// The display's size in points for a main window of `window` (`TREK_WINDOW_SIZE`).
    pub fn display_size(window: Option<(u32, u32)>) -> (u32, u32) {
        let (w, h) = window.unwrap_or((0, 0));
        let fit = |asked: u32, around: u32, base: u32, most: u32| {
            let wanted = asked.saturating_add(around).clamp(base, most);
            wanted + wanted % 2
        };
        (
            fit(w, AROUND.0, BASE.0, MOST.0),
            fit(h, AROUND.1, BASE.1, MOST.1),
        )
    }

    /// `1280x820`, as `TREK_WINDOW_SIZE` gives it.
    pub fn window_size(var: Option<&str>) -> Option<(u32, u32)> {
        let (w, h) = var?.trim().split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }

    /// What the test displays say they are, so each test app can tell them from the owner's.
    pub const VENDOR: u32 = 0x7E4B;
    pub const PRODUCT: u32 = 0x7E57;
    const SERIAL: u32 = 0x7E57_0000;
    /// How many test apps can have a display at once. macOS wants each display's serial number
    /// to differ and remembers every one it has seen, so they come from this short list.
    pub const SLOTS: u32 = 8;

    pub fn serial(slot: u32) -> u32 {
        SERIAL + slot
    }

    /// The first slot none of `used` (the serial numbers of the test displays there now) has.
    pub fn free_slot(used: &[u32]) -> Option<u32> {
        (1..=SLOTS).find(|slot| !used.contains(&serial(*slot)))
    }

    /// "Trek Test Display", then "Trek Test Display 2" and so on, to tell them apart.
    pub fn name(slot: u32) -> String {
        match slot {
            0 | 1 => "Trek Test Display".to_string(),
            n => format!("Trek Test Display {n}"),
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Decision {
        /// The build has no code to make a display (anything but a `shots` build on macOS).
        NotBuiltIn,
        /// `TREK_TEST_DISPLAY` isn't `1`.
        NotAsked,
        Make,
    }

    pub fn decide(built_in: bool, var: Option<&str>) -> Decision {
        match (built_in, var.map(str::trim)) {
            (false, _) => Decision::NotBuiltIn,
            (true, Some("1")) => Decision::Make,
            (true, _) => Decision::NotAsked,
        }
    }

    /// The display this process made.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Info {
        pub id: u32,
        pub slot: u32,
        pub name: String,
        /// Its size in points; twice that in pixels.
        pub points: (u32, u32),
        pub bounds: Rect,
        /// Where that is, in words.
        pub sits: String,
    }

    /// How the run stands.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Outcome {
        /// No display was asked for, or this build can't make one.
        Off,
        /// One was asked for and couldn't be made: the run carries on as it always has.
        AsBefore {
            why: String,
        },
        On(Info),
    }

    impl Outcome {
        /// `make` runs only when a display is to be made; its failure is a reason, not an error.
        pub fn settle(decision: Decision, make: impl FnOnce() -> Result<Info, String>) -> Self {
            match decision {
                Decision::NotBuiltIn | Decision::NotAsked => Outcome::Off,
                Decision::Make => match make() {
                    Ok(info) => Outcome::On(info),
                    Err(why) => Outcome::AsBefore { why },
                },
            }
        }

        /// The display windows are told to open on: none unless one was made.
        pub fn target(&self) -> Option<u32> {
            match self {
                Outcome::On(info) => Some(info.id),
                Outcome::Off | Outcome::AsBefore { .. } => None,
            }
        }

        /// What `trek-dev status` reads (`display.json`).
        pub fn report(&self) -> serde_json::Value {
            match self {
                Outcome::Off => serde_json::json!({ "asked": false, "created": false }),
                Outcome::AsBefore { why } => {
                    serde_json::json!({ "asked": true, "created": false, "why": why })
                }
                Outcome::On(info) => serde_json::json!({
                    "asked": true,
                    "created": true,
                    "id": info.id,
                    "name": info.name,
                    "slot": info.slot,
                    "scale": 2,
                    "points": [info.points.0, info.points.1],
                    "pixels": [info.points.0 * 2, info.points.1 * 2],
                    "bounds": {
                        "x": info.bounds.x, "y": info.bounds.y,
                        "w": info.bounds.w, "h": info.bounds.h,
                    },
                    "sits": info.sits,
                }),
            }
        }
    }

    /// Where `display` is, for the report: the corner it touches, or what's wrong.
    /// `real` pairs each of the owner's displays with its id.
    pub fn sits(display: &Rect, real: &[(u32, Rect)], main: u32) -> String {
        let which = |id: u32| {
            if id == main {
                format!("display {id} (the main display)")
            } else {
                format!("display {id}")
            }
        };
        if let Some((id, _)) = real.iter().find(|(_, r)| display.overlaps(r)) {
            return format!("over {}", which(*id));
        }
        if let Some((id, _)) = real.iter().find(|(_, r)| display.shares_edge(r)) {
            return format!("along an edge of {}", which(*id));
        }
        match real
            .iter()
            .find_map(|(id, r)| display.corner_met(r).map(|corner| (*id, corner)))
        {
            Some((id, corner)) => format!(
                "diagonally off the {} corner of {}, touching only that point",
                corner.name(),
                which(id)
            ),
            None => "off the far corner of another test display, touching no real display".into(),
        }
    }
}

#[cfg(all(feature = "shots", target_os = "macos"))]
mod mac {
    use super::plan::{self, Hot, Info, Outcome, Rect};
    use core_graphics::display::{
        CGConfigureOption, CGDisplay, CGDisplayRegisterReconfigurationCallback,
    };
    use gpui_kit::App;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{ClassType as _, MainThreadMarker, msg_send};
    use objc2_app_kit::{NSApplication, NSWindow};
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::os::fd::AsRawFd as _;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, after_one_source: u8) -> i32;
    }

    unsafe extern "C" {
        /// libdispatch: the queue the display's termination handler would run on.
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
    }

    /// The display, for as long as this is kept: letting go of the object removes it.
    struct Held {
        /// The `CGVirtualDisplay`, never used again: holding it is what keeps the display.
        _display: Retained<AnyObject>,
        info: Info,
        /// Looks in a row that didn't find the display in the list (see `keep`).
        missing: u8,
    }

    thread_local! {
        /// Made and let go of on the main thread only.
        static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
    }

    /// With `TREK_TEST_DISPLAY=1`, make the test display, before any window opens (and before
    /// the app's run loop: AppKit has to hear of the display first, see `await_screen`). A
    /// failure is logged and the run carries on without one.
    pub fn start() {
        let var = std::env::var("TREK_TEST_DISPLAY").ok();
        let outcome = Outcome::settle(plan::decide(super::BUILT_IN, var.as_deref()), || {
            let window = plan::window_size(std::env::var("TREK_WINDOW_SIZE").ok().as_deref());
            let held = make(window)?;
            let info = held.info.clone();
            HELD.with(|h| *h.borrow_mut() = Some(held));
            Ok(info)
        });
        super::HELD.store(outcome.target().unwrap_or(0), Ordering::Relaxed);
        match &outcome {
            Outcome::On(info) => tracing::info!(
                "test display: {} (display {}), {}x{} points at 2x, at {},{}: {}",
                info.name,
                info.id,
                info.points.0,
                info.points.1,
                info.bounds.x,
                info.bounds.y,
                info.sits
            ),
            Outcome::AsBefore { why } => tracing::warn!(
                "test display: not made ({why}); windows open on the main display as before"
            ),
            Outcome::Off => {}
        }
        report(&outcome);
    }

    /// Keep the display off the desktop's edges and Trek's windows on it, for as long as the app
    /// runs (see `keep`): at once when the displays change (another test app's display went and
    /// macOS put this one back beside the main display), and once a second for a window that
    /// turned up elsewhere.
    pub fn watch(cx: &mut App) {
        if super::id().is_none() {
            return;
        }
        // One waiting is enough: several displays report one change.
        let (tx, rx) = async_channel::bounded(1);
        if CHANGED.set(tx).is_ok() {
            // SAFETY: `displays_changed` is a plain function that lives as long as the process.
            unsafe { CGDisplayRegisterReconfigurationCallback(displays_changed, std::ptr::null()) };
        }
        cx.spawn(async move |cx| {
            while rx.recv().await.is_ok() && super::id().is_some() {
                cx.update(|_| keep());
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            while super::id().is_some() {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                cx.update(|_| keep());
            }
        })
        .detach();
    }

    /// Where `displays_changed` says so.
    static CHANGED: OnceLock<async_channel::Sender<()>> = OnceLock::new();

    /// CoreGraphics calls this for each display before and after any change to the displays.
    unsafe extern "C" fn displays_changed(_display: u32, flags: u32, _: *const c_void) {
        /// `kCGDisplayBeginConfigurationFlag`: about to change; the call after has the change.
        const BEGINS: u32 = 1;
        if flags & BEGINS == 0
            && let Some(changed) = CHANGED.get()
        {
            let _ = changed.try_send(());
        }
    }

    /// Whether a window of the app is still on the test display, as the app quits: the display
    /// stays until none is (`end`). A closed window is the window server's for a moment more
    /// while macOS draws it shrinking away, after AppKit has let go of it; let go of then, the
    /// display would leave that window on the owner's screen for the rest of the moment. What
    /// AppKit still lists (a panel, nothing GPUI closed) is taken off screen here.
    pub fn windows_up() -> bool {
        /// `NSWindowNumberListAllSpaces`: this app's windows on screen, on every Space.
        const ALL_SPACES: usize = 1 << 4;
        let (true, Some(mtm)) = (super::id().is_some(), MainThreadMarker::new()) else {
            return false;
        };
        for window in NSApplication::sharedApplication(mtm).windows().iter() {
            window.orderOut(None);
        }
        // SAFETY: `+[NSWindow windowNumbersWithOptions:]`, on the main thread: the numbers of
        // this app's windows the window server has on screen.
        let numbers: Option<Retained<NSArray<AnyObject>>> =
            unsafe { msg_send![NSWindow::class(), windowNumbersWithOptions: ALL_SPACES] };
        numbers.is_some_and(|n| n.count() > 0)
    }

    /// Before the app ends, once its windows are closed (`windows_up`): let the display go, and
    /// say whether there was one. A display that went with windows on it would have macOS move
    /// them to the owner's screen.
    ///
    /// Let go of, the display leaves at once from where it is. The app shouldn't end in that
    /// same instant: macOS then drops the app's placement first, and the display spends its
    /// last tenth of a second beside the main display (`root::quit` waits a moment).
    pub fn end() -> bool {
        let Some(held) = HELD.with(|h| h.borrow_mut().take()) else {
            return false;
        };
        super::HELD.store(0, Ordering::Relaxed);
        if let Some(mtm) = MainThreadMarker::new() {
            for window in NSApplication::sharedApplication(mtm).windows().iter() {
                window.orderOut(None);
            }
        }
        drop(held);
        true
    }

    /// Every window of the app as the window server has it, for `trek-dev windows` and
    /// `screenshot --real`: its number (`screencapture -l`), the display it's on, its frame in
    /// the global display space, and whether macOS counts it as visible (so draws it). `main`
    /// is the main window's `NSWindow`.
    pub fn windows(main: Option<*const c_void>) -> serde_json::Value {
        let Some(mtm) = MainThreadMarker::new() else {
            return serde_json::json!([]);
        };
        /// `NSWindowOcclusionStateVisible`.
        const VISIBLE: usize = 1 << 1;
        let test = super::HELD.load(Ordering::Relaxed);
        let main_height = CGDisplay::main().bounds().size.height;
        let mut out = Vec::new();
        for window in NSApplication::sharedApplication(mtm).windows().iter() {
            let frame = window.frame();
            let display = screen_number(&window);
            // SAFETY: plain AppKit getters on a live window, on the main thread.
            let (occlusion, sheet, scale): (usize, bool, f64) = unsafe {
                let parent: Option<Retained<AnyObject>> = msg_send![&*window, sheetParent];
                (
                    msg_send![&*window, occlusionState],
                    parent.is_some(),
                    msg_send![&*window, backingScaleFactor],
                )
            };
            out.push(serde_json::json!({
                "id": window.windowNumber(),
                "title": window.title().to_string(),
                "main": main.is_some_and(|m| std::ptr::eq(m, Retained::as_ptr(&window).cast())),
                "shown": window.isVisible(),
                "drawn": occlusion & VISIBLE != 0,
                "sheet": sheet,
                "scale": scale,
                "display": display,
                "on_test_display": test != 0 && display == Some(test),
                // AppKit measures up from the main display's bottom edge.
                "frame": {
                    "x": frame.origin.x,
                    "y": main_height - frame.origin.y - frame.size.height,
                    "w": frame.size.width,
                    "h": frame.size.height,
                },
            }));
        }
        serde_json::Value::Array(out)
    }

    /// One look: the display is still there and still off the desktop's edges, and no window
    /// of the app has turned up on another display.
    fn keep() {
        let gone = HELD.with(|held| {
            let mut held = held.borrow_mut();
            let Some(h) = held.as_mut() else { return false };
            let id = h.info.id;
            let all = seen();
            let Some(me) = all.iter().find(|d| d.id == id) else {
                // Not on one look alone: letting go of a display that's there would put its
                // windows on the owner's screen.
                h.missing += 1;
                return h.missing >= 3;
            };
            h.missing = 0;
            let real: Vec<Rect> = all.iter().filter(|d| !d.test).map(|d| d.rect).collect();
            let was = h.info.clone();
            if plan::apart(&me.rect, &real) {
                h.info.bounds = me.rect;
            } else {
                tracing::warn!(
                    "test display: now at {},{}, along the desktop's edge; placing it again",
                    me.rect.x,
                    me.rect.y
                );
                let _turn = Turn::take();
                match place(id, h.info.points) {
                    Ok(rect) => h.info.bounds = rect,
                    Err(why) => tracing::warn!("test display: still along the edge: {why}"),
                }
            }
            h.info.sits = sits(id, &h.info.bounds);
            if h.info != was {
                report(&Outcome::On(h.info.clone()));
            }
            gather(id, &h.info.bounds);
            false
        });
        if gone {
            // macOS took it away (it isn't in the list any more): what was on it is on another
            // display now, and new windows choose for themselves.
            tracing::warn!("test display: macOS removed it; windows are on the main display");
            super::HELD.store(0, Ordering::Relaxed);
            HELD.with(|h| h.borrow_mut().take());
            report(&Outcome::AsBefore {
                why: "macOS removed the display while the app ran".into(),
            });
        }
    }

    /// Make the display, place it, and wait until AppKit knows it. On any failure the object is
    /// let go of, which removes the display again.
    fn make(window: Option<(u32, u32)>) -> Result<Held, String> {
        MainThreadMarker::new().ok_or("not on the main thread")?;
        let (Some(descriptor), Some(settings), Some(mode), Some(virtual_display)) = (
            AnyClass::get(c"CGVirtualDisplayDescriptor"),
            AnyClass::get(c"CGVirtualDisplaySettings"),
            AnyClass::get(c"CGVirtualDisplayMode"),
            AnyClass::get(c"CGVirtualDisplay"),
        ) else {
            return Err("this macOS has no CGVirtualDisplay".into());
        };
        let points = plan::display_size(window);
        // One test app at a time picks a slot, makes its display and places it.
        let _turn = Turn::take();
        let used: Vec<u32> = seen().iter().filter(|d| d.test).map(|d| d.serial).collect();
        let slot = plan::free_slot(&used)
            .ok_or_else(|| format!("all {} test displays are in use", plan::SLOTS))?;
        let name = plan::name(slot);

        // SAFETY: CoreGraphics' CGVirtualDisplay classes, with the selectors and argument types
        // Chromium's tests use (`virtual_display_util_mac.mm`): `unsigned int` ids and sizes,
        // CGPoint chromaticities, a CGSize in millimetres, a dispatch queue, on the main thread.
        let display: Retained<AnyObject> = unsafe {
            let d: Retained<AnyObject> = msg_send![descriptor, new];
            let _: () = msg_send![&*d, setQueue: dispatch_get_global_queue(0, 0)];
            let _: () = msg_send![&*d, setName: &*NSString::from_str(&name)];
            // sRGB's primaries and white point.
            let _: () = msg_send![&*d, setWhitePoint: NSPoint::new(0.3125, 0.3291)];
            let _: () = msg_send![&*d, setRedPrimary: NSPoint::new(0.6797, 0.3203)];
            let _: () = msg_send![&*d, setGreenPrimary: NSPoint::new(0.2559, 0.6983)];
            let _: () = msg_send![&*d, setBluePrimary: NSPoint::new(0.1494, 0.0557)];
            let _: () = msg_send![&*d, setMaxPixelsWide: points.0 * 2];
            let _: () = msg_send![&*d, setMaxPixelsHigh: points.1 * 2];
            // 220 pixels to the inch, as a Retina display has.
            let mm = |px: u32| 25.4 * px as f64 / 220.;
            let _: () = msg_send![&*d, setSizeInMillimeters: NSSize::new(mm(points.0 * 2), mm(points.1 * 2))];
            // macOS 14 and later want a vendor that isn't 0 and a serial number of its own for
            // each display.
            let _: () = msg_send![&*d, setVendorID: plan::VENDOR];
            let _: () = msg_send![&*d, setProductID: plan::PRODUCT];
            let _: () = msg_send![&*d, setSerialNum: plan::serial(slot)];
            let made: Allocated<AnyObject> = msg_send![virtual_display, alloc];
            let made: Option<Retained<AnyObject>> = msg_send![made, initWithDescriptor: &*d];
            let made = made.ok_or("CGVirtualDisplay wouldn't make one")?;
            // One mode, in points: with `hiDPI` its pixels are twice that each way.
            let s: Retained<AnyObject> = msg_send![settings, new];
            let _: () = msg_send![&*s, setHiDPI: 1u32];
            let m: Allocated<AnyObject> = msg_send![mode, alloc];
            let m: Option<Retained<AnyObject>> =
                msg_send![m, initWithWidth: points.0, height: points.1, refreshRate: 60f64];
            let m = m.ok_or("CGVirtualDisplayMode wouldn't make the mode")?;
            let _: () = msg_send![&*s, setModes: &*NSArray::from_retained_slice(&[m])];
            let applied: bool = msg_send![&*made, applySettings: &*s];
            if !applied {
                return Err("the display refused its settings".into());
            }
            made
        };
        // SAFETY: a plain getter, `unsigned int`.
        let id: u32 = unsafe { msg_send![&*display, displayID] };
        if id == 0 {
            return Err("the display has no id".into());
        }
        // It's in the list a moment after it's made, beside the main display, where macOS puts
        // a new one: off the desktop's edge first, then the rest.
        let listed = || CGDisplay::active_displays().is_ok_and(|all| all.contains(&id));
        if !wait_turning(Duration::from_secs(3), listed) {
            return Err(format!("display {id} never appeared in the display list"));
        }
        let bounds = place(id, points)?;
        let has_mode = || CGDisplay::new(id).display_mode().is_some();
        if !wait_turning(Duration::from_secs(3), has_mode) {
            return Err(format!("display {id} never had a mode"));
        }
        check(id, points)?;
        await_screen(id, &bounds)?;
        let info = Info {
            id,
            slot,
            name,
            points,
            bounds,
            sits: sits(id, &bounds),
        };
        Ok(Held {
            _display: display,
            info,
            missing: 0,
        })
    }

    /// The display is what was asked for and has changed nothing of the owner's: 2x at the
    /// size wanted (so screenshots are as sharp as on a Retina screen), not the main display,
    /// and no display mirroring another.
    fn check(id: u32, points: (u32, u32)) -> Result<(), String> {
        let display = CGDisplay::new(id);
        if display.is_main() {
            return Err("macOS made it the main display".into());
        }
        let all = CGDisplay::active_displays().unwrap_or_default();
        if all.iter().any(|d| CGDisplay::new(*d).is_in_mirror_set()) {
            return Err("macOS mirrored a display".into());
        }
        let mode = display.display_mode().ok_or("the display has no mode")?;
        let (w, h) = (mode.width(), mode.height());
        let (pw, ph) = (mode.pixel_width(), mode.pixel_height());
        if (w, h) != (points.0 as u64, points.1 as u64) || (pw, ph) != (2 * w, 2 * h) {
            return Err(format!(
                "it came up at {w}x{h} points and {pw}x{ph} pixels, not {}x{} at 2x",
                points.0, points.1
            ));
        }
        Ok(())
    }

    /// Move the display to where `plan::place` puts it, for this app only, and say where it
    /// ended up. An error if macOS won't keep it off the desktop's edges.
    fn place(id: u32, points: (u32, u32)) -> Result<Rect, String> {
        let size = (points.0 as i32, points.1 as i32);
        let mut why = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(150));
            }
            let all = seen();
            let others = || all.iter().filter(|d| d.id != id);
            let real: Vec<Rect> = others().filter(|d| !d.test).map(|d| d.rect).collect();
            let tests: Vec<Rect> = others().filter(|d| d.test).map(|d| d.rect).collect();
            let main = rect(CGDisplay::main().bounds());
            let Some((x, y)) = plan::place(size, &main, &real, &tests, hot()) else {
                return Err("no corner of the desktop is free".into());
            };
            let display = CGDisplay::new(id);
            let moved = display.begin_configuration().and_then(|config| {
                // No `CGConfigureDisplayFadeEffect` here: macOS 27 refuses a whole change that
                // sets one (error 1006), and fades nothing without it (the main display's
                // gamma table holds still through the change).
                if let Err(e) = display.configure_display_origin(&config, x, y) {
                    let _ = display.cancel_configuration(&config);
                    return Err(e);
                }
                // For this app only: never saved to the owner's display settings.
                display.complete_configuration(&config, CGConfigureOption::ConfigureForAppOnly)
            });
            if let Err(e) = moved {
                why = format!("CoreGraphics error {e} moving it to {x},{y}");
                continue;
            }
            let now = rect(display.bounds());
            if plan::apart(&now, &real) {
                return Ok(now);
            }
            why = format!(
                "asked for {x},{y}, macOS put it at {},{} along an edge",
                now.x, now.y
            );
        }
        Err(why)
    }

    /// Wait until AppKit lists the display where CoreGraphics has it: GPUI finds a window's
    /// screen through AppKit, which hears of a new display on the main run loop.
    fn await_screen(id: u32, bounds: &Rect) -> Result<(), String> {
        if wait_turning(Duration::from_secs(3), || {
            appkit_frame(id, bounds).is_some()
        }) {
            Ok(())
        } else {
            Err(format!("AppKit never listed display {id} where it is"))
        }
    }

    /// The display's frame as AppKit has it (measured up from the main display's bottom
    /// edge), once that agrees with where CoreGraphics has the display (`bounds`) at 2x.
    /// `None` while AppKit is still a turn of the run loop behind a change.
    fn appkit_frame(id: u32, bounds: &Rect) -> Option<NSRect> {
        let main_height = CGDisplay::main().bounds().size.height;
        let wanted = NSRect::new(
            NSPoint::new(bounds.x as f64, main_height - (bounds.y + bounds.h) as f64),
            NSSize::new(bounds.w as f64, bounds.h as f64),
        );
        let screen = screen(id)?;
        // SAFETY: `-[NSScreen frame]` and `backingScaleFactor`, on the main thread.
        let (frame, scale): (NSRect, f64) = unsafe {
            (
                msg_send![&*screen, frame],
                msg_send![&*screen, backingScaleFactor],
            )
        };
        (frame == wanted && scale == 2.).then_some(frame)
    }

    /// Put a window of the app that's on another display onto the test display: a file panel,
    /// or one macOS moved. Sheets and child windows (popovers) follow the window they belong
    /// to, and menus and the like (above the level panels float at) are left alone. Not while
    /// AppKit is behind a change to the displays (`bounds` is where the display is now): what
    /// it says of a window's screen is stale then, and macOS moves a display's windows with it.
    fn gather(id: u32, bounds: &Rect) {
        /// `NSModalPanelWindowLevel`.
        const PANELS: isize = 8;
        let (Some(mtm), Some(on)) = (MainThreadMarker::new(), appkit_frame(id, bounds)) else {
            return;
        };
        for window in NSApplication::sharedApplication(mtm).windows().iter() {
            // SAFETY: plain AppKit getters on a live window, on the main thread.
            let attached = unsafe {
                let parent: Option<Retained<AnyObject>> = msg_send![&*window, parentWindow];
                let sheet: Option<Retained<AnyObject>> = msg_send![&*window, sheetParent];
                parent.is_some() || sheet.is_some()
            };
            if !window.isVisible()
                || attached
                || window.level() > PANELS
                || screen_number(&window) == Some(id)
            {
                continue;
            }
            let size = window.frame().size;
            let middle = |from: f64, room: f64, taken: f64| from + ((room - taken) / 2.).max(0.);
            window.setFrameOrigin(NSPoint::new(
                middle(on.origin.x, on.size.width, size.width),
                middle(on.origin.y, on.size.height, size.height),
            ));
            tracing::info!(
                "test display: window {} was on another display; moved onto it",
                window.windowNumber()
            );
        }
    }

    /// A display as CoreGraphics lists it.
    struct Seen {
        id: u32,
        rect: Rect,
        /// One of the test displays (this app's or another's), not one of the owner's.
        test: bool,
        serial: u32,
    }

    fn seen() -> Vec<Seen> {
        CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                let d = CGDisplay::new(id);
                Seen {
                    id,
                    rect: rect(d.bounds()),
                    test: d.vendor_number() == plan::VENDOR && d.model_number() == plan::PRODUCT,
                    serial: d.serial_number(),
                }
            })
            .collect()
    }

    fn rect(r: core_graphics::display::CGRect) -> Rect {
        Rect::new(
            r.origin.x.round() as i32,
            r.origin.y.round() as i32,
            r.size.width.round() as i32,
            r.size.height.round() as i32,
        )
    }

    fn sits(id: u32, bounds: &Rect) -> String {
        let real: Vec<(u32, Rect)> = seen()
            .iter()
            .filter(|d| !d.test && d.id != id)
            .map(|d| (d.id, d.rect))
            .collect();
        plan::sits(bounds, &real, CGDisplay::main().id)
    }

    /// The owner's hot corners, from the Dock's settings.
    fn hot() -> Hot {
        let Some(defaults) = AnyClass::get(c"NSUserDefaults") else {
            return Hot::default();
        };
        // SAFETY: `-[NSUserDefaults initWithSuiteName:]`, `objectForKey:` and `integerForKey:`,
        // reading another app's settings; nothing is written.
        unsafe {
            let dock: Allocated<AnyObject> = msg_send![defaults, alloc];
            let dock: Option<Retained<AnyObject>> =
                msg_send![dock, initWithSuiteName: &*NSString::from_str("com.apple.dock")];
            let Some(dock) = dock else {
                return Hot::default();
            };
            let read = |key: &str| -> Option<i64> {
                let key = NSString::from_str(key);
                let set: Option<Retained<AnyObject>> = msg_send![&*dock, objectForKey: &*key];
                set?;
                let value: isize = msg_send![&*dock, integerForKey: &*key];
                Some(value as i64)
            };
            Hot::from_settings(
                read("wvous-bl-corner"),
                read("wvous-br-corner"),
                read("wvous-tr-corner"),
                read("wvous-tl-corner"),
            )
        }
    }

    /// The `NSScreen` of display `id`, once AppKit has it.
    fn screen(id: u32) -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"NSScreen")?;
        // SAFETY: `+[NSScreen screens]` and each screen's `deviceDescription`, whose
        // `NSScreenNumber` is the display's id as an NSNumber; on the main thread.
        unsafe {
            let screens: Retained<NSArray<AnyObject>> = msg_send![class, screens];
            screens.iter().find(|s| number_of(s) == Some(id))
        }
    }

    /// The display a window is on, as AppKit has it.
    fn screen_number(window: &NSWindow) -> Option<u32> {
        // SAFETY: `-[NSWindow screen]`, on the main thread; nil while it's on none.
        unsafe {
            let screen: Option<Retained<AnyObject>> = msg_send![window, screen];
            let screen = screen?;
            number_of(&screen)
        }
    }

    /// # Safety
    /// `screen` is an `NSScreen`, and this is the main thread.
    unsafe fn number_of(screen: &AnyObject) -> Option<u32> {
        // SAFETY: as the caller promises; the value under `NSScreenNumber` is an NSNumber.
        unsafe {
            let description: Retained<AnyObject> = msg_send![screen, deviceDescription];
            let number: Option<Retained<AnyObject>> =
                msg_send![&*description, objectForKey: &*NSString::from_str("NSScreenNumber")];
            let id: u32 = msg_send![&*number?, unsignedIntValue];
            Some(id)
        }
    }

    /// Ask `done` every 10 ms until it says yes or `limit` passes.
    fn wait(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + limit;
        loop {
            if done() {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// As `wait`, turning the main run loop between asks: what CoreGraphics and AppKit know of
    /// the displays in this process is brought up to date there (a new display's mode, its
    /// `NSScreen`). Only for `make`, before the app's own loop runs: later, a turn here would run
    /// GPUI's work in the middle of GPUI's work.
    fn wait_turning(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + limit;
        loop {
            if done() {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            // SAFETY: the main thread's run loop, in its default mode, for a moment.
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 0) };
        }
    }

    /// One test app at a time picks a slot and places its display: a lock file they all share,
    /// held until this is dropped. After ten seconds of waiting it goes ahead without (a stuck
    /// test app mustn't stop the others).
    struct Turn {
        /// The locked file: the lock goes when it's closed.
        _file: Option<std::fs::File>,
    }

    impl Turn {
        fn take() -> Self {
            // SAFETY: `getuid` has no preconditions.
            let uid = unsafe { libc::getuid() };
            // Not the temp folder: sandboxed agents each have their own.
            let path = PathBuf::from(format!("/tmp/trek-test-display-{uid}.lock"));
            let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)
            else {
                return Turn { _file: None };
            };
            let fd = file.as_raw_fd();
            // SAFETY: `flock` on a descriptor this function owns.
            let locked = wait(Duration::from_secs(10), || unsafe {
                libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0
            });
            if !locked {
                tracing::warn!("test display: another test app has held the lock for 10 s");
            }
            Turn { _file: Some(file) }
        }
    }

    /// Say how the run stands where `trek-dev status` reads it.
    fn report(outcome: &Outcome) {
        let Some(dir) = std::env::var_os("TREK_SHOT_DIR").map(PathBuf::from) else {
            return;
        };
        let mut value = outcome.report();
        value["pid"] = std::process::id().into();
        let _ = std::fs::create_dir_all(&dir);
        let tmp = dir.join(".display.json.tmp");
        if std::fs::write(&tmp, format!("{value:#}\n")).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("display.json"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plan::*;

    /// A 13-inch MacBook's screen at "More Space".
    const MAIN: Rect = Rect::new(0, 0, 1710, 1107);
    const SIZE: (i32, i32) = (1920, 1200);
    /// Only the bottom-right corner does something (a Quick Note), as macOS ships.
    const USUAL: Hot = Hot {
        bottom_left: false,
        bottom_right: true,
        top_right: false,
        top_left: false,
    };

    fn at(origin: (i32, i32)) -> Rect {
        Rect::new(origin.0, origin.1, SIZE.0, SIZE.1)
    }

    #[test]
    fn a_corner_is_not_an_edge() {
        let below_left = Rect::new(-1920, 1107, 1920, 1200);
        assert!(!below_left.overlaps(&MAIN));
        assert!(!below_left.shares_edge(&MAIN));
        assert_eq!(below_left.corner_met(&MAIN), Some(Corner::BottomLeft));
        assert!(apart(&below_left, &[MAIN]));

        // Where macOS puts a new display: beside the main one, all down its right edge.
        let beside = Rect::new(1710, 0, 1920, 1200);
        assert!(beside.shares_edge(&MAIN));
        assert!(!apart(&beside, &[MAIN]));
        // One point of overlap along the edge is enough for a pointer to cross.
        assert!(Rect::new(1710, 1106, 1920, 1200).shares_edge(&MAIN));
        // Below it, stacked.
        assert!(Rect::new(-100, 1107, 1920, 1200).shares_edge(&MAIN));
        // Over it.
        assert!(Rect::new(100, 100, 1920, 1200).overlaps(&MAIN));
        assert!(!apart(&Rect::new(100, 100, 1920, 1200), &[MAIN]));
        // Away from it altogether.
        let far = Rect::new(-5000, 4000, 1920, 1200);
        assert!(apart(&far, &[MAIN]) && far.corner_met(&MAIN).is_none());
    }

    #[test]
    fn every_corner_touches_at_one_point() {
        for corner in Corner::ALL {
            let rect = at(corner.outside(&MAIN, SIZE));
            assert!(apart(&rect, &[MAIN]), "{corner:?}");
            assert_eq!(rect.corner_met(&MAIN), Some(corner));
        }
    }

    #[test]
    fn the_first_display_goes_below_and_left_of_the_only_screen() {
        let origin = place(SIZE, &MAIN, &[MAIN], &[], USUAL).unwrap();
        assert_eq!(origin, (-1920, 1107));
        assert_eq!(at(origin).corner_met(&MAIN), Some(Corner::BottomLeft));
        // The main display stays the one at the origin: nothing is ever placed there.
        assert_ne!(origin, (0, 0));
    }

    #[test]
    fn hot_corners_are_left_alone() {
        let corner_for = |hot: Hot| {
            at(place(SIZE, &MAIN, &[MAIN], &[], hot).unwrap())
                .corner_met(&MAIN)
                .unwrap()
        };
        // None hot: the first choice. The usual one (bottom-right) hot: the same.
        assert_eq!(corner_for(Hot::default()), Corner::BottomLeft);
        assert_eq!(corner_for(USUAL), Corner::BottomLeft);
        let bottom = Hot {
            bottom_left: true,
            bottom_right: true,
            ..Hot::default()
        };
        assert_eq!(corner_for(bottom), Corner::TopRight);
        let all_but_top_left = Hot {
            top_right: true,
            ..bottom
        };
        assert_eq!(corner_for(all_but_top_left), Corner::TopLeft);
        // Bottom-left hot alone: the bottom-right one is next.
        let bottom_left = Hot {
            bottom_left: true,
            ..Hot::default()
        };
        assert_eq!(corner_for(bottom_left), Corner::BottomRight);
        // With all four in use there's still a display: the bottom-left one.
        let all = Hot {
            top_left: true,
            ..all_but_top_left
        };
        assert_eq!(corner_for(all), Corner::BottomLeft);
    }

    #[test]
    fn hot_corners_read_from_the_docks_settings() {
        // As on the owner's Mac: only `wvous-br-corner` is set, to 14 (Quick Note).
        assert_eq!(Hot::from_settings(None, Some(14), None, None), USUAL);
        // Never set: the bottom-right corner still opens a Quick Note.
        assert_eq!(Hot::from_settings(None, None, None, None), USUAL);
        // 1 is "no action" in System Settings; 0 the same.
        assert_eq!(
            Hot::from_settings(Some(1), Some(1), Some(0), Some(1)),
            Hot::default()
        );
        assert!(Hot::from_settings(Some(2), Some(1), None, None).bottom_left);
    }

    #[test]
    fn a_second_test_app_hangs_off_the_first_ones_display() {
        let first = at(place(SIZE, &MAIN, &[MAIN], &[], USUAL).unwrap());
        let second = at(place(SIZE, &MAIN, &[MAIN], &[first], USUAL).unwrap());
        // Diagonally on from the first, touching no real display at all.
        assert_eq!((second.x, second.y), (-3840, 2307));
        assert!(apart(&second, &[MAIN]));
        assert!(second.corner_met(&MAIN).is_none());
        assert!(!second.overlaps(&first));
        let third = at(place(SIZE, &MAIN, &[MAIN], &[first, second], USUAL).unwrap());
        assert!(third.corner_met(&MAIN).is_none());
        assert!(!third.overlaps(&first) && !third.overlaps(&second));
    }

    #[test]
    fn with_the_first_gone_the_second_takes_a_real_corner() {
        // The display it hung off went: placed again, it's the only test display.
        let alone = at(place(SIZE, &MAIN, &[MAIN], &[], USUAL).unwrap());
        assert_eq!(alone.corner_met(&MAIN), Some(Corner::BottomLeft));
    }

    #[test]
    fn two_real_displays_keep_theirs_edge_free() {
        // A monitor to the right of the laptop, a little shorter.
        let monitor = Rect::new(1710, 0, 1920, 1080);
        let real = [MAIN, monitor];
        let rect = at(place(SIZE, &MAIN, &real, &[], USUAL).unwrap());
        assert!(apart(&rect, &real));
        // Off the far display's far corner, not between the two.
        assert_eq!(rect.corner_met(&monitor), Some(Corner::TopRight));
        // Every hot corner taken or not, nothing ever lands along an edge of either.
        for hot in [Hot::default(), USUAL] {
            let rect = at(place(SIZE, &MAIN, &real, &[], hot).unwrap());
            assert!(apart(&rect, &real));
        }
        // A monitor above, centred: its bottom corners are along the laptop's top edge.
        let above = Rect::new(-105, -1080, 1920, 1080);
        let real = [MAIN, above];
        let rect = at(place(SIZE, &MAIN, &real, &[], Hot::default()).unwrap());
        assert!(apart(&rect, &real));
    }

    #[test]
    fn a_main_display_ringed_by_others_is_never_touched() {
        // The owner's displays all round the main one: every place off one of its corners
        // would lie along one of them.
        let ring = [
            MAIN,
            Rect::new(-1920, -1200, 1920 + 1710 + 1920, 1200),
            Rect::new(-1920, 1107, 1920 + 1710 + 1920, 1200),
            Rect::new(-1920, 0, 1920, 1107),
            Rect::new(1710, 0, 1920, 1107),
        ];
        // It goes off an outer corner of the ring instead.
        let rect = at(place(SIZE, &MAIN, &ring, &[], Hot::default()).unwrap());
        assert!(apart(&rect, &ring));
        assert!(rect.corner_met(&MAIN).is_none());
        // And a desktop with no displays to hang off has no place at all.
        assert_eq!(place(SIZE, &MAIN, &[], &[], Hot::default()), None);
    }

    #[test]
    fn the_display_fits_the_window_asked_for() {
        // No size asked, and trek-dev's default: the smallest display.
        assert_eq!(display_size(None), BASE);
        assert_eq!(display_size(Some((1280, 820))), BASE);
        // Larger windows get a larger display, with room round them.
        assert_eq!(display_size(Some((1920, 1200))), (2080, 1400));
        assert_eq!(display_size(Some((2560, 1440))), (2720, 1640));
        // Odd sizes are made even, and nothing is larger than the most CoreGraphics is asked for.
        assert_eq!(display_size(Some((1801, 1001))), (1962, 1202));
        assert_eq!(display_size(Some((9000, 9000))), MOST);
        assert_eq!(display_size(Some((u32::MAX, 1))), (MOST.0, BASE.1));
        for (w, h) in [(1280, 820), (1920, 1200), (2560, 1440), (3000, 1900)] {
            let (dw, dh) = display_size(Some((w, h)));
            assert!(dw >= w && dh >= h, "{w}x{h} fits in {dw}x{dh}");
        }
        assert_eq!(window_size(Some("1280x820")), Some((1280, 820)));
        assert_eq!(window_size(Some(" 2560x1440 ")), Some((2560, 1440)));
        assert_eq!(window_size(Some("large")), None);
        assert_eq!(window_size(None), None);
    }

    #[test]
    fn each_test_app_has_a_slot_and_a_name() {
        assert_eq!(free_slot(&[]), Some(1));
        assert_eq!(free_slot(&[serial(1)]), Some(2));
        // A slot freed by an app that quit is taken again.
        assert_eq!(free_slot(&[serial(2), serial(3)]), Some(1));
        // Someone else's display with another serial number takes no slot.
        assert_eq!(free_slot(&[42]), Some(1));
        let all: Vec<u32> = (1..=SLOTS).map(serial).collect();
        assert_eq!(free_slot(&all), None);
        assert_eq!(name(1), "Trek Test Display");
        assert_eq!(name(2), "Trek Test Display 2");
        assert_ne!(serial(1), serial(2));
    }

    #[test]
    fn only_a_shots_build_on_a_mac_asked_to_makes_a_display() {
        assert_eq!(
            super::BUILT_IN,
            cfg!(all(feature = "shots", target_os = "macos"))
        );
        // The shipped app, whatever its environment says.
        assert_eq!(decide(false, Some("1")), Decision::NotBuiltIn);
        assert_eq!(decide(false, None), Decision::NotBuiltIn);
        // A test build that wasn't asked, or was told not to.
        assert_eq!(decide(true, None), Decision::NotAsked);
        assert_eq!(decide(true, Some("0")), Decision::NotAsked);
        assert_eq!(decide(true, Some("")), Decision::NotAsked);
        assert_eq!(decide(true, Some("yes")), Decision::NotAsked);
        assert_eq!(decide(true, Some("1")), Decision::Make);
        assert_eq!(decide(true, Some(" 1 ")), Decision::Make);
    }

    fn info() -> Info {
        Info {
            id: 7,
            slot: 1,
            name: name(1),
            points: (1920, 1200),
            bounds: Rect::new(-1920, 1107, 1920, 1200),
            sits: sits(&Rect::new(-1920, 1107, 1920, 1200), &[(1, MAIN)], 1),
        }
    }

    #[test]
    fn nothing_is_made_unless_asked_and_built_in() {
        let never = || -> Result<Info, String> { panic!("a display was made") };
        assert_eq!(Outcome::settle(Decision::NotBuiltIn, never), Outcome::Off);
        assert_eq!(Outcome::settle(Decision::NotAsked, never), Outcome::Off);
        assert_eq!(Outcome::Off.target(), None);
        assert_eq!(
            Outcome::Off.report(),
            serde_json::json!({ "asked": false, "created": false })
        );
    }

    #[test]
    fn a_display_that_cant_be_made_leaves_the_run_as_it_was() {
        let outcome = Outcome::settle(Decision::Make, || {
            Err("this macOS has no CGVirtualDisplay".into())
        });
        // No display for windows to open on: they choose as they always have.
        assert_eq!(outcome.target(), None);
        let report = outcome.report();
        assert_eq!(report["asked"], true);
        assert_eq!(report["created"], false);
        assert_eq!(report["why"], "this macOS has no CGVirtualDisplay");
    }

    #[test]
    fn a_made_display_is_where_windows_open_and_what_status_reports() {
        let outcome = Outcome::settle(Decision::Make, || Ok(info()));
        assert_eq!(outcome.target(), Some(7));
        let report = outcome.report();
        assert_eq!(report["created"], true);
        assert_eq!(report["id"], 7);
        assert_eq!(report["name"], "Trek Test Display");
        assert_eq!(report["scale"], 2);
        assert_eq!(report["points"], serde_json::json!([1920, 1200]));
        assert_eq!(report["pixels"], serde_json::json!([3840, 2400]));
        assert_eq!(
            report["bounds"],
            serde_json::json!({ "x": -1920, "y": 1107, "w": 1920, "h": 1200 })
        );
        assert_eq!(
            report["sits"],
            "diagonally off the bottom-left corner of display 1 (the main display), touching only that point"
        );
    }

    #[test]
    fn where_it_sits_is_said_plainly() {
        let real = [(1, MAIN), (2, Rect::new(1710, 0, 1920, 1080))];
        assert_eq!(
            sits(&Rect::new(3630, -1200, 1920, 1200), &real, 1),
            "diagonally off the top-right corner of display 2, touching only that point"
        );
        assert_eq!(
            sits(&Rect::new(3630, 0, 1920, 1200), &real, 1),
            "along an edge of display 2"
        );
        assert_eq!(
            sits(&Rect::new(100, 100, 1920, 1200), &real, 1),
            "over display 1 (the main display)"
        );
        assert_eq!(
            sits(&Rect::new(-3840, 2307, 1920, 1200), &real, 1),
            "off the far corner of another test display, touching no real display"
        );
    }

    /// Without a display held (every build but a test run that made one), windows are given
    /// no display to open on.
    #[test]
    fn no_display_held_means_no_display_chosen() {
        assert_eq!(super::HELD.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(super::id().is_none());
    }
}
