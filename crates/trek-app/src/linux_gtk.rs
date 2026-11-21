//! GTK on Linux. The tray icon (libayatana-appindicator, through tray-icon) and the web views
//! (WebKitGTK, through wry) are GTK's, and GTK wants `gtk::init` on the main thread before either
//! is made, then its main loop run. GPUI runs its own loop (calloop), so GTK's is run from it:
//! every few milliseconds, whatever GTK has pending, never blocking.
//!
//! Without a display GTK can open (no `DISPLAY`/`WAYLAND_DISPLAY`, or a server that refuses it)
//! `ready` says no, and what needs GTK isn't offered: no tray icon, and the web views say why.

use gpui_kit::App;
use std::sync::OnceLock;
use std::time::Duration;

/// How often GTK's pending work is run: often enough for a web view's frames and input.
const PUMP_EVERY: Duration = Duration::from_millis(8);

static READY: OnceLock<bool> = OnceLock::new();

/// Start GTK if it isn't yet, and run its main loop from GPUI's from now on. Whether GTK is up.
/// Main thread only (GPUI's foreground).
pub fn ready(cx: &mut App) -> bool {
    *READY.get_or_init(|| match gtk::init() {
        Ok(()) => {
            // GDK ends the process at the first X error on its connection. A web view's window
            // lives inside windows Trek and GPUI make and remove on connections of their own, so
            // GTK can act on one that has just gone (a card scrolled away while GTK still had its
            // resize queued): a trap held for good makes such errors GDK's to ignore.
            #[allow(deprecated)]
            gtk::gdk::error_trap_push();
            pump(cx);
            true
        }
        Err(e) => {
            tracing::warn!("GTK couldn't start ({e}): no tray icon or web views");
            false
        }
    })
}

/// Whether GTK is up already (`ready` was called and it started).
pub fn is_ready() -> bool {
    READY.get().copied().unwrap_or(false)
}

fn pump(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(PUMP_EVERY).await;
            // On the main thread, between GPUI's own turns and outside any of them, as AppKit's
            // run loop calls a WKWebView's handlers: a handler that updates the app finds it
            // free. A bounded number of rounds, so a flood of GTK events can't starve GPUI's.
            let mut n = 0;
            while gtk::events_pending() && n < 64 {
                gtk::main_iteration_do(false);
                n += 1;
            }
        }
    })
    .detach();
}

/// The parent wry takes for a web view inside the GPUI window `raw`, or `None` where one can't go.
/// wry puts a web view inside an X11 window only, named by Xlib (GPUI's is named by XCB: the same
/// window), and WebKitGTK has no way into another client's Wayland surface; and it needs GTK up.
pub fn web_view_parent(
    raw: raw_window_handle::RawWindowHandle,
) -> Option<raw_window_handle::RawWindowHandle> {
    use raw_window_handle::{RawWindowHandle, XlibWindowHandle};
    if !is_ready() {
        return None;
    }
    match raw {
        RawWindowHandle::Xcb(h) => Some(RawWindowHandle::Xlib(XlibWindowHandle::new(
            std::ffi::c_ulong::from(h.window.get()),
        ))),
        RawWindowHandle::Xlib(_) => Some(raw),
        _ => None,
    }
}

/// In a Wayland session that also offers X11 (Xwayland: GNOME, KDE and the rest do), Trek's windows
/// are X11 ones, unless `TREK_WAYLAND=1`. Two things need it: WebKitGTK goes inside an X11 window
/// only (the Browser, interactive artifacts, plugin pages), and GNOME draws no title bar or
/// borders around a Wayland window (it leaves them to the app), which would leave Trek's without a
/// close button or an edge to resize by. Under Xwayland the desktop frames the window as any
/// other. First thing at launch, before any thread starts: it changes this process's environment,
/// which Trek's agents and terminals inherit (they still reach X11 through Xwayland).
pub fn prefer_x11() {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    let x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    let native = std::env::var("TREK_WAYLAND").is_ok_and(|v| v == "1");
    if wayland && x11 && !native {
        // SAFETY: called first thing in `run`, while this is the only thread.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
}

/// Whether a library tray-icon draws the tray icon with is installed: libappindicator-sys loads
/// one of these at its first use, and panics, ending Trek, when none is there.
pub fn indicator_library() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        [
            c"libayatana-appindicator3.so.1",
            c"libappindicator3.so.1",
            c"libayatana-appindicator3.so",
            c"libappindicator3.so",
        ]
        .iter()
        .any(|name| {
            // SAFETY: a NUL-terminated name; the handle, if any, is closed straight away
            // (tray-icon opens the library again itself).
            unsafe {
                let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
                if handle.is_null() {
                    false
                } else {
                    libc::dlclose(handle);
                    true
                }
            }
        })
    })
}

/// Why a web view can't open here, in a sentence for the panel that would show it.
pub const NO_WEB_VIEWS: &str = "Web pages open inside Trek on X11. In a Wayland session, start Trek with WAYLAND_DISPLAY unset to use them, or open the page in your browser.";
