//! Where a window that isn't on screen is, and its picture while it isn't. The window list keeps
//! an entry for such a window without saying why it's off screen (`kCGWindowIsOnscreen` is just
//! missing), so each case is asked of whoever knows it:
//!
//! - **Minimised, or its app hidden**: Accessibility (`AXMinimized` on the window, matched by its
//!   number through `_AXUIElementGetWindow`; `AXHidden` on its app).
//! - **On another Space**: the window server's private calls (SkyLight) say which Spaces a window
//!   is on (`SLSCopySpacesForWindows`), which one its display shows
//!   (`SLSManagedDisplayGetCurrentSpace`), and whether a Space is an app's full-screen one
//!   (`SLSSpaceGetType`). A window on another Space stays ordered in, as AppKit has it (visible,
//!   but not on the active Space); one its app closed but kept is ordered out
//!   (`SLSWindowIsOrderedIn`), whatever Space it was on.
//! - **Its picture**: `screencapture -l` and ScreenCaptureKit give nothing for a window on another
//!   Space; `SLSHWCaptureWindowList` gives the window's own buffer as it was last drawn, as window
//!   switchers use it (cua-driver, CUA-1351). It orders, activates and switches nothing.
//!
//! All of it reads only, and is best effort: a call that's missing or an app that doesn't answer
//! leaves the window "not showing", which `computer` gives a few seconds and then lets go.
//!
//! Measured on macOS 27.2: a hidden app's window is off screen, ordered out, on the Space showing
//! and `AXHidden`; a panel its app closed is off screen, ordered out, on a Space, and not in the
//! app's `AXWindows`; `SLSHWCaptureWindowList` gives a picture of both, and `screencapture -l` does
//! too. A window on another Space wasn't there to measure (Spaces aren't made or switched to test).

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::{CFURL, CFURLRef};
use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType as _;

use super::{Whereabouts, WindowInfo};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetTypeID() -> core_foundation::base::CFTypeID;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithURL(
        url: CFURLRef,
        kind: CFStringRef,
        count: usize,
        options: *const c_void,
    ) -> CFTypeRef;
    fn CGImageDestinationAddImage(
        destination: CFTypeRef,
        image: *mut c_void,
        options: *const c_void,
    );
    fn CGImageDestinationFinalize(destination: CFTypeRef) -> bool;
    fn CGImageSourceCreateWithURL(url: CFURLRef, options: *const c_void) -> CFTypeRef;
    fn CGImageSourceCreateImageAtIndex(
        source: CFTypeRef,
        index: usize,
        options: *const c_void,
    ) -> *mut c_void;
}

type Connection = unsafe extern "C" fn() -> i32;
type SpacesForWindows = unsafe extern "C" fn(i32, i32, CFArrayRef) -> CFArrayRef;
type DisplayForWindow = unsafe extern "C" fn(i32, u32) -> CFStringRef;
type DisplaySpace = unsafe extern "C" fn(i32, CFStringRef) -> u64;
type ActiveSpace = unsafe extern "C" fn(i32) -> u64;
type SpaceType = unsafe extern "C" fn(i32, u64) -> i32;
type OrderedIn = unsafe extern "C" fn(i32, u32, *mut u8) -> i32;
type CaptureWindows = unsafe extern "C" fn(i32, *mut u32, i32, u32) -> CFArrayRef;
/// `_AXUIElementGetWindow(element, &number)`: the window list's number for an AX window.
type AxWindowNumber = unsafe extern "C" fn(CFTypeRef, *mut u32) -> i32;

struct Calls {
    connection: Connection,
    spaces_for_windows: Option<SpacesForWindows>,
    display_for_window: Option<DisplayForWindow>,
    display_space: Option<DisplaySpace>,
    active_space: Option<ActiveSpace>,
    space_type: Option<SpaceType>,
    ordered_in: Option<OrderedIn>,
    capture: Option<CaptureWindows>,
    ax_window_number: Option<AxWindowNumber>,
}

fn calls() -> Option<&'static Calls> {
    static CALLS: OnceLock<Option<Calls>> = OnceLock::new();
    CALLS
        .get_or_init(|| {
            let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
            // SAFETY: loading a system framework by its absolute path; the handle is never
            // closed. Each symbol is cast to the signature the window server's own callers use.
            unsafe {
                let sky = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
                if sky.is_null() {
                    return None;
                }
                let find = |handle: *mut c_void, name: &CStr| {
                    let p = libc::dlsym(handle, name.as_ptr());
                    (!p.is_null()).then_some(p)
                };
                let connection = find(sky, c"SLSMainConnectionID")?;
                Some(Calls {
                    connection: std::mem::transmute::<*mut c_void, Connection>(connection),
                    spaces_for_windows: find(sky, c"SLSCopySpacesForWindows")
                        .map(|p| std::mem::transmute::<*mut c_void, SpacesForWindows>(p)),
                    display_for_window: find(sky, c"SLSCopyManagedDisplayForWindow")
                        .map(|p| std::mem::transmute::<*mut c_void, DisplayForWindow>(p)),
                    display_space: find(sky, c"SLSManagedDisplayGetCurrentSpace")
                        .map(|p| std::mem::transmute::<*mut c_void, DisplaySpace>(p)),
                    active_space: find(sky, c"SLSGetActiveSpace")
                        .map(|p| std::mem::transmute::<*mut c_void, ActiveSpace>(p)),
                    space_type: find(sky, c"SLSSpaceGetType")
                        .map(|p| std::mem::transmute::<*mut c_void, SpaceType>(p)),
                    ordered_in: find(sky, c"SLSWindowIsOrderedIn")
                        .map(|p| std::mem::transmute::<*mut c_void, OrderedIn>(p)),
                    capture: find(sky, c"SLSHWCaptureWindowList")
                        .map(|p| std::mem::transmute::<*mut c_void, CaptureWindows>(p)),
                    ax_window_number: find(libc::RTLD_DEFAULT, c"_AXUIElementGetWindow")
                        .map(|p| std::mem::transmute::<*mut c_void, AxWindowNumber>(p)),
                })
            }
        })
        .as_ref()
}

/// `SLSCopySpacesForWindows`' selector for every Space a window is on.
const ALL_SPACES: i32 = 0x7;
/// `SLSSpaceGetType` for an app's full-screen Space (0 is a desktop).
const FULL_SCREEN: i32 = 4;
/// `SLSHWCaptureWindowList` options: through the window's own clip shape, at its backing store's
/// resolution (Retina pixels, as `screencapture -l` gives).
const IGNORE_GLOBAL_CLIP_SHAPE: u32 = 1 << 11;
const BEST_RESOLUTION: u32 = 1 << 8;
/// How long an app has to answer Accessibility before it's taken to know nothing.
const AX_TIMEOUT: f32 = 0.5;

/// Where `w` is, a window the list has but not on screen.
pub fn off_screen(w: WindowInfo) -> Whereabouts {
    match put_away(&w) {
        Some(PutAway::Hidden) => return Whereabouts::Hidden(w),
        Some(PutAway::Minimised) => return Whereabouts::Minimised(w),
        None => {}
    }
    match other_space(&w) {
        Some(full_screen) => Whereabouts::OtherSpace {
            window: w,
            full_screen,
        },
        None => Whereabouts::NotShowing(w),
    }
}

enum PutAway {
    Hidden,
    Minimised,
}

/// Whether Accessibility says `w`'s app is hidden, or `w` minimised. `None` when it says neither,
/// or can't (no permission, an app that doesn't answer).
fn put_away(w: &WindowInfo) -> Option<PutAway> {
    let pid = i32::try_from(w.pid).ok().filter(|p| *p > 0)?;
    // SAFETY: a +1 AX element for the process, released by the wrapper; null when there's none.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app) };
    // SAFETY: a live AX element.
    unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), AX_TIMEOUT) };
    if flag(&app, "AXHidden") == Some(true) {
        return Some(PutAway::Hidden);
    }
    let number = calls()?.ax_window_number?;
    let windows = attribute(&app, "AXWindows")?.downcast::<CFArray>()?;
    for item in windows.iter() {
        let element = *item as CFTypeRef;
        let mut id = 0u32;
        // SAFETY: an element of the array just copied, alive while it is.
        if unsafe { number(element, &mut id) } == 0 && i64::from(id) == w.id {
            let element = unsafe { CFType::wrap_under_get_rule(element) };
            return (flag(&element, "AXMinimized") == Some(true)).then_some(PutAway::Minimised);
        }
    }
    None
}

/// The window list's number for an AX window element (`_AXUIElementGetWindow`); `None` when this
/// macOS has no such call or the element is no window.
pub(super) fn ax_window_number(element: CFTypeRef) -> Option<u32> {
    let number = calls()?.ax_window_number?;
    let mut id = 0u32;
    // SAFETY: a live AX element and a number to write the answer to.
    (unsafe { number(element, &mut id) } == 0).then_some(id)
}

/// An AX element's attribute, when it has it.
fn attribute(element: &CFType, name: &'static str) -> Option<CFType> {
    let mut value: CFTypeRef = std::ptr::null();
    let name = CFString::from_static_string(name);
    // SAFETY: a live element; on success `value` holds a +1 reference, released by the wrapper.
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            &mut value,
        )
    };
    (err == 0 && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
}

fn flag(element: &CFType, name: &'static str) -> Option<bool> {
    attribute(element, name)?
        .downcast::<CFBoolean>()
        .map(bool::from)
}

/// `Some(full_screen)` when `w` is still ordered in, on Spaces its display isn't showing.
fn other_space(w: &WindowInfo) -> Option<bool> {
    let calls = calls()?;
    let wid = u32::try_from(w.id).ok()?;
    // SAFETY: the process's own connection to the window server.
    let cid = unsafe { (calls.connection)() };
    let spaces = spaces_of(calls, cid, wid)?;
    let showing = showing(calls, cid, wid)?;
    if spaces.contains(&showing) {
        return None;
    }
    if let Some(ordered_in) = calls.ordered_in {
        let mut yes = 0u8;
        // SAFETY: a window number and a byte to write the answer to.
        if unsafe { ordered_in(cid, wid, &mut yes) } == 0 && yes == 0 {
            return None;
        }
    }
    let full_screen = calls.space_type.is_some_and(|kind| {
        spaces
            .iter()
            // SAFETY: a Space id the window server just gave.
            .any(|s| unsafe { kind(cid, *s) } == FULL_SCREEN)
    });
    Some(full_screen)
}

/// The Spaces window `wid` is on; `None` for none (a window not placed yet).
fn spaces_of(calls: &Calls, cid: i32, wid: u32) -> Option<Vec<u64>> {
    let copy = calls.spaces_for_windows?;
    let ids = CFArray::from_CFTypes(&[CFNumber::from(i64::from(wid))]);
    // SAFETY: an array of one window number; the answer is a +1 array, released by the wrapper.
    let found = unsafe { copy(cid, ALL_SPACES, ids.as_concrete_TypeRef()) };
    if found.is_null() {
        return None;
    }
    let found: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(found) };
    let spaces: Vec<u64> = found
        .iter()
        .filter_map(|s| s.downcast::<CFNumber>()?.to_i64())
        .filter_map(|s| u64::try_from(s).ok())
        .filter(|s| *s != 0)
        .collect();
    (!spaces.is_empty()).then_some(spaces)
}

/// The Space the display window `wid` is on shows now (each display has its own when Spaces are
/// per display); else the one showing.
fn showing(calls: &Calls, cid: i32, wid: u32) -> Option<u64> {
    let on_its_display = calls
        .display_for_window
        .zip(calls.display_space)
        .and_then(|(display_for, space_of)| {
            // SAFETY: a +1 display name or null, released by the wrapper.
            let display = unsafe { display_for(cid, wid) };
            if display.is_null() {
                return None;
            }
            let display = unsafe { CFString::wrap_under_create_rule(display) };
            Some(unsafe { space_of(cid, display.as_concrete_TypeRef()) })
        })
        .filter(|s| *s != 0);
    on_its_display.or_else(|| {
        calls
            .active_space
            // SAFETY: the process's own connection.
            .map(|active| unsafe { active(cid) })
            .filter(|s| *s != 0)
    })
}

/// Window `id` as the window server last drew it, as a PNG at `path`, wherever it is. Fails when
/// there's no picture, or one with nothing in it.
pub fn capture(id: i64, path: &str) -> Result<(), String> {
    let calls = calls().ok_or("this macOS has no SkyLight")?;
    let capture = calls
        .capture
        .ok_or("this macOS has no SLSHWCaptureWindowList")?;
    let mut wid = u32::try_from(id).map_err(|_| format!("no window {id}"))?;
    // SAFETY: the process's own connection, and one window number to capture; the answer is a +1
    // array of images, released by the wrapper.
    let images = unsafe {
        capture(
            (calls.connection)(),
            &mut wid,
            1,
            IGNORE_GLOBAL_CLIP_SHAPE | BEST_RESOLUTION,
        )
    };
    if images.is_null() {
        return Err("SLSHWCaptureWindowList returned nothing".into());
    }
    let images: CFArray = unsafe { CFArray::wrap_under_create_rule(images) };
    let first = images
        .get(0)
        .map(|i| *i as CFTypeRef)
        .filter(|i| !i.is_null())
        // SAFETY: a live CF object from the array.
        .filter(|i| unsafe { core_foundation::base::CFGetTypeID(*i) == CGImageGetTypeID() })
        .ok_or("SLSHWCaptureWindowList returned no image")?;
    // SAFETY: a CGImage the array holds; the extra reference is the wrapper's to release.
    let image = unsafe {
        core_foundation::base::CFRetain(first);
        CGImage::from_ptr(first as *mut _)
    };
    if !visible(&image) {
        return Err("the window's picture is empty".into());
    }
    write_png(&image, path)
}

/// Whether the PNG at `path` has any pixel that isn't fully transparent.
pub fn has_content(path: &str) -> bool {
    let Some(url) = CFURL::from_path(path, false) else {
        return false;
    };
    // SAFETY: a +1 image source and image (or null), each released by its wrapper.
    unsafe {
        let source = CGImageSourceCreateWithURL(url.as_concrete_TypeRef(), std::ptr::null());
        if source.is_null() {
            return false;
        }
        let source = CFType::wrap_under_create_rule(source);
        let image = CGImageSourceCreateImageAtIndex(source.as_CFTypeRef(), 0, std::ptr::null());
        !image.is_null() && visible(&CGImage::from_ptr(image.cast()))
    }
}

/// Whether any pixel of `image` shows: the window server hands back a fully transparent picture
/// for a window with nothing drawn.
fn visible(image: &CGImage) -> bool {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return false;
    }
    let mut context = CGContext::create_bitmap_context(
        None,
        w,
        h,
        8,
        w * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    );
    context.draw_image(
        CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64)),
        image,
    );
    context
        .data()
        .chunks_exact(4)
        .step_by(97)
        .any(|pixel| pixel[3] != 0)
}

fn write_png(image: &CGImage, path: &str) -> Result<(), String> {
    let url = CFURL::from_path(path, false).ok_or("bad capture path")?;
    let png = CFString::from_static_string("public.png");
    // SAFETY: a +1 destination (or null), released by the wrapper, given one live image.
    unsafe {
        let destination = CGImageDestinationCreateWithURL(
            url.as_concrete_TypeRef(),
            png.as_concrete_TypeRef(),
            1,
            std::ptr::null(),
        );
        if destination.is_null() {
            return Err("couldn't write the window's picture".into());
        }
        let destination = CFType::wrap_under_create_rule(destination);
        CGImageDestinationAddImage(
            destination.as_CFTypeRef(),
            image.as_ptr().cast(),
            std::ptr::null(),
        );
        if !CGImageDestinationFinalize(destination.as_CFTypeRef()) {
            return Err("couldn't write the window's picture".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only, on the real desktop: says where each app window the system lists off screen
    /// is, and that asking never checked this process in as an app. Run by hand
    /// (`cargo test -p trek-mcp spaces -- --ignored --nocapture`); it needs Accessibility to tell
    /// minimised and hidden windows apart.
    #[test]
    #[ignore]
    fn the_real_windows_off_screen_are_placed() {
        use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
        use core_graphics::window::{
            copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
            kCGWindowListOptionAll,
        };
        let all = copy_window_info(
            kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
        .unwrap();
        let num = |d: &CFDictionary<CFString, CFType>, k: &'static str| {
            d.find(CFString::from_static_string(k))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
                .unwrap_or(-1)
        };
        for item in all.iter() {
            let d: CFDictionary<CFString, CFType> =
                unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
            if num(&d, "kCGWindowLayer") != 0 {
                continue;
            }
            let on = d
                .find(CFString::from_static_string("kCGWindowIsOnscreen"))
                .and_then(|v| v.downcast::<CFBoolean>())
                .is_some_and(bool::from);
            let w = WindowInfo {
                owner: d
                    .find(CFString::from_static_string("kCGWindowOwnerName"))
                    .and_then(|v| v.downcast::<CFString>())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                title: String::new(),
                pid: num(&d, "kCGWindowOwnerPID"),
                id: num(&d, "kCGWindowNumber"),
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            };
            if on {
                continue;
            }
            let place = off_screen(w.clone());
            // A window that's out of sight but kept (another Space, a hidden app's) is pictured
            // from the window server's copy; the PNG is only measured, then removed.
            if matches!(
                place,
                Whereabouts::OtherSpace { .. } | Whereabouts::Hidden(_)
            ) {
                let png = crate::util::TempFile::new("png");
                match capture(w.id, png.path_str()) {
                    Ok(()) => {
                        assert!(has_content(png.path_str()));
                        println!(
                            "{} {}: pictured, {:?}",
                            w.id,
                            w.owner,
                            crate::util::png_size(&std::fs::read(&png.0).unwrap())
                        );
                    }
                    // A helper window with nothing drawn in it.
                    Err(e) => println!("{} {}: no picture ({e})", w.id, w.owner),
                }
            }
            let placed = match place {
                Whereabouts::OtherSpace { full_screen, .. } => {
                    format!("on another Space (full screen: {full_screen})")
                }
                Whereabouts::Minimised(_) => "minimised".into(),
                Whereabouts::Hidden(_) => "hidden".into(),
                Whereabouts::NotShowing(_) => "not showing".into(),
                other => format!("{other:?}"),
            };
            println!("{} {}: {placed}", w.id, w.owner);
        }
        // This process never checked itself in as an app (a Dock icon) by asking.
        let pid = std::process::id().to_string();
        let listed = std::process::Command::new("/usr/bin/lsappinfo")
            .args(["find", &format!("pid={pid}")])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
            "{}",
            String::from_utf8_lossy(&listed.stdout)
        );
    }
}
