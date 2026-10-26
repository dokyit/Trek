//! "More…": the system's share menu (NSSharingServicePicker) with the thread as text, where the
//! pointer is (over the button that asked for it).

use gpui_kit::Window;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

thread_local! {
    /// The picker on screen: kept until the next one, as AppKit doesn't promise to.
    static PICKER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// NSRectEdge's NSMinYEdge: the menu opens below the point.
const BELOW: usize = 1;

/// Show the share menu for `text` at the pointer in `window`. False when the window has no
/// AppKit view to show it from.
pub fn share_text(window: &Window, text: &str) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return false;
    };
    let at = window.mouse_position();
    // SAFETY: `ns_view` is the live NSView GPUI made for this window, used on the main thread;
    // the picker and its items are ordinary AppKit objects made here and kept (`PICKER`).
    unsafe {
        let view = appkit.ns_view.as_ptr().cast::<AnyObject>();
        let text = NSString::from_str(text);
        let items: Retained<AnyObject> = msg_send![class!(NSArray), arrayWithObject: &*text];
        let picker: Allocated<AnyObject> = msg_send![class!(NSSharingServicePicker), alloc];
        let picker: Retained<AnyObject> = msg_send![picker, initWithItems: &*items];
        let flipped: bool = msg_send![view, isFlipped];
        let bounds: NSRect = msg_send![view, bounds];
        let y = f64::from(at.y.as_f32());
        let y = if flipped { y } else { bounds.size.height - y };
        let rect = NSRect::new(
            NSPoint::new(f64::from(at.x.as_f32()), y),
            NSSize::new(1., 1.),
        );
        let _: () =
            msg_send![&*picker, showRelativeToRect: rect, ofView: view, preferredEdge: BELOW];
        PICKER.with(|p| p.replace(Some(picker)));
    }
    true
}
