//! macOS: screenshots via `screencapture`, mouse/keyboard via CoreGraphics CGEvents, windows via
//! CGWindowList, apps via `open -a`.

use std::time::Duration;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
};

use super::{Button, Capture, Desktop, Display, Rect, WindowInfo};
use crate::keys::{self, KeyCombo};
use crate::util::{self, TempFile};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

const SCREEN_RECORDING_ERROR: &str = "Screen Recording permission is missing, so trek-mcp cannot capture the screen. \
Grant it to the app that launched the agent (Trek, or your terminal) in System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording, \
then restart the agent session.";

#[derive(Default)]
pub struct Mac;

impl Desktop for Mac {
    fn may_post_input(&self) -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    /// Once per run, ask macOS to prompt for Accessibility: the silent check alone never adds
    /// trek-mcp to the list, and a bundled helper can't be picked in the pane by hand.
    fn ask_to_post_input(&self) {
        use core_foundation::boolean::CFBoolean;
        static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if ASKED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::from_static_string("AXTrustedCheckOptionPrompt"),
            CFBoolean::true_value(),
        )]);
        unsafe {
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
        }
    }

    fn display(&self) -> Display {
        let display = CGDisplay::main();
        let bounds = display.bounds();
        let physical = display.display_mode().map(|m| (m.pixel_width(), m.pixel_height()));
        Display { width: bounds.size.width, height: bounds.size.height, physical }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        on_screen_windows()
    }

    fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
        let tmp = TempFile::new("png");
        let mut capture_args: Vec<String> = vec!["-x".into(), "-t".into(), "png".into()];
        match region {
            Some((x0, y0, rw, rh)) => {
                capture_args.push(format!("-R{},{},{},{}", x0.round(), y0.round(), rw.round(), rh.round()))
            }
            None => capture_args.push("-m".into()), // main display only
        }
        capture_args.push(tmp.path_str().to_string());
        let arg_refs: Vec<&str> = capture_args.iter().map(String::as_str).collect();
        let out = util::run("/usr/sbin/screencapture", &arg_refs, None, Duration::from_secs(20))?;
        let size = std::fs::metadata(&tmp.0).map(|m| m.len()).unwrap_or(0);
        if !out.success() || size == 0 {
            let detail = if out.success() { "empty capture".to_string() } else { out.reason() };
            return Err(format!("{SCREEN_RECORDING_ERROR}\n(screencapture: {detail})"));
        }
        let note = if unsafe { CGPreflightScreenCaptureAccess() } {
            ""
        } else {
            "\nWarning: Screen Recording permission does not appear to be granted; other apps' windows may be missing from the image."
        };
        let (_, w0, h0) = util::load_png(&tmp.0)?;
        if w0.max(h0) > max_side {
            util::sips_fit(tmp.path_str(), max_side)?;
        }
        let (png_base64, width, height) = util::load_png(&tmp.0)?;
        Ok(Capture { png_base64, width, height, note })
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        post_mouse(CGEventType::MouseMoved, CGPoint::new(x, y), CGMouseButton::Left, None)
    }

    fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let (down, up, btn) = match button {
            Button::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left),
            Button::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right),
        };
        let pt = CGPoint::new(x, y);
        post_mouse(CGEventType::MouseMoved, pt, CGMouseButton::Left, None)?;
        sleep_ms(30);
        for i in 1..=count as i64 {
            post_mouse(down, pt, btn, Some(i))?;
            sleep_ms(15);
            post_mouse(up, pt, btn, Some(i))?;
            if i < count as i64 {
                sleep_ms(40);
            }
        }
        Ok(())
    }

    fn drag(&self, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> Result<(), String> {
        let start = CGPoint::new(ax, ay);
        post_mouse(CGEventType::MouseMoved, start, CGMouseButton::Left, None)?;
        sleep_ms(30);
        post_mouse(CGEventType::LeftMouseDown, start, CGMouseButton::Left, Some(1))?;
        sleep_ms(60);
        const STEPS: i32 = 24;
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let p = CGPoint::new(ax + (bx - ax) * t, ay + (by - ay) * t);
            post_mouse(CGEventType::LeftMouseDragged, p, CGMouseButton::Left, Some(1))?;
            sleep_ms(12);
        }
        sleep_ms(40);
        post_mouse(CGEventType::LeftMouseUp, CGPoint::new(bx, by), CGMouseButton::Left, Some(1))
    }

    fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String> {
        post_mouse(CGEventType::MouseMoved, CGPoint::new(x, y), CGMouseButton::Left, None)?;
        sleep_ms(30);
        // CGEvent wheel deltas are positive for up/left; our API is positive for down/right.
        let ev = CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 2, -dy, -dx, 0)
            .map_err(|_| "Failed to create scroll event".to_string())?;
        ev.post(CGEventTapLocation::HID);
        Ok(())
    }

    fn type_text(&self, s: &str) -> Result<(), String> {
        for segment in split_for_typing(s) {
            match segment {
                TypeSegment::Key(code) => press(KeyCombo { keycode: code, flags: 0 })?,
                TypeSegment::Text(chunk) => {
                    for down in [true, false] {
                        let ev = CGEvent::new_keyboard_event(source()?, 0, down)
                            .map_err(|_| "Failed to create keyboard event".to_string())?;
                        ev.set_flags(CGEventFlags::empty());
                        ev.set_string_from_utf16_unchecked(&chunk);
                        ev.post(CGEventTapLocation::HID);
                    }
                }
            }
            sleep_ms(8);
        }
        Ok(())
    }

    fn key(&self, combo: &KeyCombo) -> Result<(), String> {
        press(*combo)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        util::run_ok("/usr/bin/open", &["-a", name], None, Duration::from_secs(30)).map(|_| ())
    }
}

// ---- CoreGraphics helpers ----

fn source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "Failed to create CGEventSource".to_string())
}

fn post_mouse(ty: CGEventType, pt: CGPoint, button: CGMouseButton, click_state: Option<i64>) -> Result<(), String> {
    let ev = CGEvent::new_mouse_event(source()?, ty, pt, button).map_err(|_| "Failed to create mouse event".to_string())?;
    // Don't inherit modifier keys the user may be holding.
    ev.set_flags(CGEventFlags::empty());
    if let Some(n) = click_state {
        ev.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, n);
    }
    ev.post(CGEventTapLocation::HID);
    Ok(())
}

fn press(combo: KeyCombo) -> Result<(), String> {
    let flags = CGEventFlags::from_bits_truncate(combo.flags);
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(source()?, combo.keycode, down)
            .map_err(|_| "Failed to create keyboard event".to_string())?;
        ev.set_flags(flags);
        ev.post(CGEventTapLocation::HID);
        if down {
            sleep_ms(15);
        }
    }
    Ok(())
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[derive(Debug, PartialEq)]
enum TypeSegment {
    Key(u16),
    /// UTF-16 chunk (≤ 20 units, the CGEventKeyboardSetUnicodeString limit).
    Text(Vec<u16>),
}

fn split_for_typing(s: &str) -> Vec<TypeSegment> {
    const MAX_UNITS: usize = 20;
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let s = s.replace("\r\n", "\n");
    for ch in s.chars() {
        let key = match ch {
            '\n' | '\r' => Some(keys::KEY_RETURN),
            '\t' => Some(keys::KEY_TAB),
            _ => None,
        };
        if let Some(code) = key {
            if !cur.is_empty() {
                out.push(TypeSegment::Text(std::mem::take(&mut cur)));
            }
            out.push(TypeSegment::Key(code));
            continue;
        }
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        if cur.len() + units.len() > MAX_UNITS {
            out.push(TypeSegment::Text(std::mem::take(&mut cur)));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(TypeSegment::Text(cur));
    }
    out
}

fn on_screen_windows() -> Result<Vec<WindowInfo>, String> {
    let arr = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )
    .ok_or("CGWindowListCopyWindowInfo returned nothing")?;
    let key = CFString::from_static_string;
    let mut out = Vec::new();
    for item in arr.iter() {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
        let get_str = |k: &'static str| {
            dict.find(key(k))
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let get_num = |k: &'static str| {
            dict.find(key(k))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
                .unwrap_or(-1)
        };
        if get_num("kCGWindowLayer") != 0 {
            continue;
        }
        let Some(bounds) = dict
            .find(key("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))
        else {
            continue;
        };
        if bounds.size.width < 2.0 || bounds.size.height < 2.0 {
            continue;
        }
        out.push(WindowInfo {
            owner: get_str("kCGWindowOwnerName"),
            title: get_str("kCGWindowName"),
            pid: get_num("kCGWindowOwnerPID"),
            id: get_num("kCGWindowNumber"),
            x: bounds.origin.x,
            y: bounds.origin.y,
            w: bounds.size.width,
            h: bounds.size.height,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_segments() {
        let segs = split_for_typing("hi\r\nyou\tthere");
        assert_eq!(
            segs,
            vec![
                TypeSegment::Text("hi".encode_utf16().collect()),
                TypeSegment::Key(36),
                TypeSegment::Text("you".encode_utf16().collect()),
                TypeSegment::Key(48),
                TypeSegment::Text("there".encode_utf16().collect()),
            ]
        );
        // long text is chunked at 20 UTF-16 units without splitting surrogate pairs
        let s = "a".repeat(19) + "😀" + "b";
        let segs = split_for_typing(&s);
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], TypeSegment::Text(u) if u.len() == 19));
        assert!(matches!(&segs[1], TypeSegment::Text(u) if u.len() == 3));
    }
}
