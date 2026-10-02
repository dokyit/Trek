//! `trek-mcp computer` — macOS computer use: screenshots via `screencapture`,
//! mouse/keyboard via CoreGraphics CGEvents, windows via CGWindowList.

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
use serde_json::{Value, json};

use crate::keys::{self, KeyCombo};
use crate::rpc::{self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, point_schema, text};
use crate::util::{self, MAX_IMAGE_SIDE, TempFile, fmt_num};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

const ACCESSIBILITY_ERROR: &str = "Accessibility permission is missing, so trek-mcp cannot control the mouse or keyboard. \
Grant it to the app that launched the agent (Trek, or your terminal) in System Settings ▸ Privacy & Security ▸ Accessibility, \
then restart the agent session.";

const SCREEN_RECORDING_ERROR: &str = "Screen Recording permission is missing, so trek-mcp cannot capture the screen. \
Grant it to the app that launched the agent (Trek, or your terminal) in System Settings ▸ Privacy & Security ▸ Screen & System Audio Recording, \
then restart the agent session.";

const INSTRUCTIONS: &str = "macOS computer use for the main display. Call `screenshot` first: it returns an image plus \
the coordinate scale. All x/y arguments to click, move_mouse, drag and scroll are pixel coordinates in the most recent \
full-screen screenshot (origin top-left); trek-mcp converts them to screen points. Take a new screenshot after acting \
to verify the result. Prefer `key` shortcuts and `open_app` over hunting for UI when possible.";

/// Maps full-screen screenshot pixels to logical screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    /// Logical points per screenshot pixel.
    pub scale: f64,
    pub image_w: u32,
    pub image_h: u32,
    pub logical_w: f64,
    pub logical_h: f64,
}

impl Mapping {
    /// The mapping a full-screen screenshot of a `logical_w`×`logical_h`
    /// display would get (longest side ≤ `MAX_IMAGE_SIDE`, never upscaled).
    pub fn for_display(logical_w: f64, logical_h: f64) -> Self {
        let long = logical_w.max(logical_h).max(1.0);
        let target = long.round().min(MAX_IMAGE_SIDE as f64);
        let scale = long / target;
        Self {
            scale,
            image_w: (logical_w / scale).round() as u32,
            image_h: (logical_h / scale).round() as u32,
            logical_w,
            logical_h,
        }
    }

    pub fn from_image(logical_w: f64, logical_h: f64, image_w: u32, image_h: u32) -> Self {
        Self {
            scale: logical_w / image_w.max(1) as f64,
            image_w,
            image_h,
            logical_w,
            logical_h,
        }
    }

    /// Screenshot pixel → logical point, clamped onto the display.
    /// Errors if the point is clearly outside the screenshot.
    pub fn to_points(self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let slack = 2.0;
        if !(x.is_finite() && y.is_finite())
            || x < -slack
            || y < -slack
            || x > self.image_w as f64 + slack
            || y > self.image_h as f64 + slack
        {
            return Err(format!(
                "Point ({}, {}) is outside the screenshot ({}x{}). Use pixel coordinates from the latest screenshot.",
                fmt_num(x),
                fmt_num(y),
                self.image_w,
                self.image_h
            ));
        }
        let px = (x * self.scale).clamp(0.0, (self.logical_w - 1.0).max(0.0));
        let py = (y * self.scale).clamp(0.0, (self.logical_h - 1.0).max(0.0));
        Ok((px, py))
    }
}

#[derive(Default)]
pub struct Computer {
    last: Option<Mapping>,
}

fn display_info() -> (CGRect, Option<(u64, u64)>) {
    let display = CGDisplay::main();
    let bounds = display.bounds();
    let pixels = display.display_mode().map(|m| (m.pixel_width(), m.pixel_height()));
    (bounds, pixels)
}

impl Computer {
    fn mapping(&self) -> Mapping {
        self.last.unwrap_or_else(|| {
            let (b, _) = display_info();
            Mapping::for_display(b.size.width, b.size.height)
        })
    }

    fn screenshot(&mut self, args: &Value) -> ToolResult {
        let (bounds, pixels) = display_info();
        let (lw, lh) = (bounds.size.width, bounds.size.height);
        let tmp = TempFile::new("png");

        let region = match args.get("region") {
            None | Some(Value::Null) => None,
            Some(r) => Some((arg_f64(r, "x")?, arg_f64(r, "y")?, arg_f64(r, "width")?, arg_f64(r, "height")?)),
        };

        let mut capture_args: Vec<String> = vec!["-x".into(), "-t".into(), "png".into()];
        // Region → logical points, through the full-screen mapping.
        let region_pts = match region {
            Some((x, y, w, h)) => {
                if w <= 0.0 || h <= 0.0 {
                    return Err("region width and height must be positive".into());
                }
                let m = self.mapping();
                let (x0, y0) = m.to_points(x, y)?;
                let (x1, y1) = m.to_points(x + w, y + h)?;
                let (rw, rh) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
                capture_args.push(format!("-R{},{},{},{}", x0.round(), y0.round(), rw.round(), rh.round()));
                Some((x0, y0, rw, rh))
            }
            None => {
                capture_args.push("-m".into()); // main display only
                None
            }
        };
        capture_args.push(tmp.path_str().to_string());
        let arg_refs: Vec<&str> = capture_args.iter().map(String::as_str).collect();
        let out = util::run("/usr/sbin/screencapture", &arg_refs, None, Duration::from_secs(20))?;
        let size = std::fs::metadata(&tmp.0).map(|m| m.len()).unwrap_or(0);
        if !out.success() || size == 0 {
            let detail = if out.success() { "empty capture".to_string() } else { out.reason() };
            return Err(format!("{SCREEN_RECORDING_ERROR}\n(screencapture: {detail})"));
        }
        let permission_note = if unsafe { CGPreflightScreenCaptureAccess() } {
            ""
        } else {
            "\nWarning: Screen Recording permission does not appear to be granted; other apps' windows may be missing from the image."
        };

        match region_pts {
            None => {
                let target = lw.max(lh).round().min(MAX_IMAGE_SIDE as f64) as u32;
                let (_, w0, h0) = util::load_png(&tmp.0)?;
                if w0.max(h0) > target {
                    util::sips_fit(tmp.path_str(), target)?;
                }
                let (b64, iw, ih) = util::load_png(&tmp.0)?;
                let m = Mapping::from_image(lw, lh, iw, ih);
                self.last = Some(m);
                let physical = pixels
                    .map(|(pw, ph)| format!(", {pw}x{ph} physical pixels"))
                    .unwrap_or_default();
                let info = format!(
                    "Screenshot of the main display: {iw}x{ih} px. Screen: {}x{} logical points{physical}. \
Scale: 1 screenshot px = {} points. Pass screenshot pixel coordinates (origin top-left) to click, move_mouse, drag \
and scroll; trek-mcp converts them to screen points.{permission_note}",
                    fmt_num(lw),
                    fmt_num(lh),
                    fmt_num(m.scale),
                );
                Ok(vec![rpc::image_png(b64), text(info)])
            }
            Some((x0, y0, rw, rh)) => {
                let (_, w0, h0) = util::load_png(&tmp.0)?;
                if w0.max(h0) > MAX_IMAGE_SIDE {
                    util::sips_fit(tmp.path_str(), MAX_IMAGE_SIDE)?;
                }
                let (b64, iw, ih) = util::load_png(&tmp.0)?;
                let m = self.mapping();
                let (sx, sy, sw, sh) = (x0 / m.scale, y0 / m.scale, rw / m.scale, rh / m.scale);
                let info = format!(
                    "Zoomed view of screenshot region x={} y={} width={} height={} (screen points {},{} {}x{}), \
rendered at {iw}x{ih} px. This is for reading detail only: click/drag/scroll coordinates still use the full-screen \
screenshot space. A point (u, v) in this image is at full-screenshot ({} + u*{}, {} + v*{}).{permission_note}",
                    fmt_num(sx),
                    fmt_num(sy),
                    fmt_num(sw),
                    fmt_num(sh),
                    fmt_num(x0),
                    fmt_num(y0),
                    fmt_num(rw),
                    fmt_num(rh),
                    fmt_num(sx),
                    fmt_num(sw / iw.max(1) as f64),
                    fmt_num(sy),
                    fmt_num(sh / ih.max(1) as f64),
                );
                Ok(vec![rpc::image_png(b64), text(info)])
            }
        }
    }

    fn click(&mut self, args: &Value) -> ToolResult {
        require_accessibility()?;
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
        let count = opt_f64(args, "count")?.unwrap_or(1.0) as i64;
        if !(1..=3).contains(&count) {
            return Err("count must be 1 (single), 2 (double) or 3 (triple)".into());
        }
        let (down, up, btn) = match button {
            "left" => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left),
            "right" => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right),
            other => return Err(format!("Unknown button {other:?}; use \"left\" or \"right\"")),
        };
        let (px, py) = self.mapping().to_points(x, y)?;
        let pt = CGPoint::new(px, py);
        post_mouse(CGEventType::MouseMoved, pt, CGMouseButton::Left, None)?;
        sleep_ms(30);
        for i in 1..=count {
            post_mouse(down, pt, btn, Some(i))?;
            sleep_ms(15);
            post_mouse(up, pt, btn, Some(i))?;
            if i < count {
                sleep_ms(40);
            }
        }
        let what = match count {
            2 => "Double-clicked",
            3 => "Triple-clicked",
            _ => "Clicked",
        };
        Ok(vec![text(format!(
            "{what} {button} at ({}, {}) (screen points {}, {}).",
            fmt_num(x),
            fmt_num(y),
            fmt_num(px),
            fmt_num(py)
        ))])
    }

    fn move_mouse(&mut self, args: &Value) -> ToolResult {
        require_accessibility()?;
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let (px, py) = self.mapping().to_points(x, y)?;
        post_mouse(CGEventType::MouseMoved, CGPoint::new(px, py), CGMouseButton::Left, None)?;
        Ok(vec![text(format!("Moved mouse to ({}, {}).", fmt_num(x), fmt_num(y)))])
    }

    fn drag(&mut self, args: &Value) -> ToolResult {
        require_accessibility()?;
        let (fx, fy) = arg_point(args, "from")?;
        let (tx, ty) = arg_point(args, "to")?;
        let m = self.mapping();
        let (ax, ay) = m.to_points(fx, fy)?;
        let (bx, by) = m.to_points(tx, ty)?;
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
        post_mouse(CGEventType::LeftMouseUp, CGPoint::new(bx, by), CGMouseButton::Left, Some(1))?;
        Ok(vec![text(format!(
            "Dragged from ({}, {}) to ({}, {}).",
            fmt_num(fx),
            fmt_num(fy),
            fmt_num(tx),
            fmt_num(ty)
        ))])
    }

    fn scroll(&mut self, args: &Value) -> ToolResult {
        require_accessibility()?;
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let dx = opt_f64(args, "dx")?.unwrap_or(0.0).round() as i32;
        let dy = opt_f64(args, "dy")?.unwrap_or(0.0).round() as i32;
        if dx == 0 && dy == 0 {
            return Err("scroll needs a non-zero dx or dy".into());
        }
        let (px, py) = self.mapping().to_points(x, y)?;
        post_mouse(CGEventType::MouseMoved, CGPoint::new(px, py), CGMouseButton::Left, None)?;
        sleep_ms(30);
        // CGEvent wheel deltas are positive for up/left; our API is positive for down/right.
        let ev = CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 2, -dy, -dx, 0)
            .map_err(|_| "Failed to create scroll event".to_string())?;
        ev.post(CGEventTapLocation::HID);
        Ok(vec![text(format!(
            "Scrolled dx={dx} dy={dy} lines at ({}, {}).",
            fmt_num(x),
            fmt_num(y)
        ))])
    }

    fn type_text(&mut self, args: &Value) -> ToolResult {
        require_accessibility()?;
        let s = arg_str(args, "text")?;
        if s.is_empty() {
            return Err("text is empty".into());
        }
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
        Ok(vec![text(format!("Typed {} characters.", s.chars().count()))])
    }

    fn key(&mut self, args: &Value) -> ToolResult {
        let combo = arg_str(args, "combo")?;
        let parsed = keys::parse_combo(combo)?;
        require_accessibility()?;
        press(parsed)?;
        Ok(vec![text(format!("Pressed {combo}."))])
    }

    fn open_app(&mut self, args: &Value) -> ToolResult {
        let name = arg_str(args, "name")?.trim();
        if name.is_empty() {
            return Err("name is empty".into());
        }
        util::run_ok("/usr/bin/open", &["-a", name], None, Duration::from_secs(30))
            .map_err(|e| format!("Could not open {name:?}: {e}"))?;
        Ok(vec![text(format!(
            "Opened {name}. Take a screenshot to see it (it may take a moment to appear)."
        ))])
    }

    fn list_windows(&mut self) -> ToolResult {
        let windows = on_screen_windows()?;
        let m = self.mapping();
        let list: Vec<Value> = windows
            .iter()
            .map(|w| {
                json!({
                    "app": w.owner,
                    "title": w.title,
                    "pid": w.pid,
                    "window_id": w.id,
                    "bounds_points": {"x": w.x, "y": w.y, "width": w.w, "height": w.h},
                    "bounds_screenshot": {
                        "x": (w.x / m.scale).round(),
                        "y": (w.y / m.scale).round(),
                        "width": (w.w / m.scale).round(),
                        "height": (w.h / m.scale).round(),
                    },
                })
            })
            .collect();
        let header = format!(
            "{} on-screen windows, front to back. bounds_screenshot uses screenshot pixel coordinates (scale {}). \
Window titles are empty without Screen Recording permission.",
            list.len(),
            fmt_num(m.scale)
        );
        let lines: Vec<String> = list.iter().map(Value::to_string).collect();
        Ok(vec![text(format!("{header}\n{}", lines.join("\n")))])
    }
}

impl rpc::ToolSet for Computer {
    fn family(&self) -> &'static str {
        "computer"
    }

    fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        let xy = |what: &str| {
            json!({
                "x": {"type": "number", "description": format!("{what} x, in screenshot pixels")},
                "y": {"type": "number", "description": format!("{what} y, in screenshot pixels")},
            })
        };
        let mut click_props = xy("Click").as_object().cloned().unwrap_or_default();
        click_props.insert("button".into(), json!({"type": "string", "enum": ["left", "right"], "default": "left"}));
        click_props.insert(
            "count".into(),
            json!({"type": "integer", "enum": [1, 2, 3], "default": 1, "description": "1 = click, 2 = double-click, 3 = triple-click"}),
        );
        let mut scroll_props = xy("Pointer").as_object().cloned().unwrap_or_default();
        scroll_props.insert(
            "dx".into(),
            json!({"type": "integer", "default": 0, "description": "Horizontal scroll in lines; positive scrolls right"}),
        );
        scroll_props.insert(
            "dy".into(),
            json!({"type": "integer", "default": 0, "description": "Vertical scroll in lines; positive scrolls down (reveals content below)"}),
        );
        vec![
            ToolDef {
                name: "screenshot",
                description: "Capture the main display as a PNG (longest side ≤ 1568 px) plus the screen size and \
coordinate scale. Coordinates in this image are what click/move_mouse/drag/scroll expect. Pass `region` (in \
screenshot pixels) to get a higher-detail zoom of part of the screen; zooms don't change the click coordinate space.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "region": {
                            "type": "object",
                            "description": "Optional area to zoom into, in full-screenshot pixel coordinates",
                            "properties": {
                                "x": {"type": "number"}, "y": {"type": "number"},
                                "width": {"type": "number"}, "height": {"type": "number"},
                            },
                            "required": ["x", "y", "width", "height"],
                        },
                    },
                }),
            },
            ToolDef {
                name: "click",
                description: "Move the pointer to (x, y) in screenshot pixels and click. Supports right-click and double/triple-click.",
                input_schema: json!({"type": "object", "properties": click_props, "required": ["x", "y"]}),
            },
            ToolDef {
                name: "move_mouse",
                description: "Move the pointer to (x, y) in screenshot pixels without clicking (e.g. to reveal hover UI).",
                input_schema: json!({"type": "object", "properties": xy("Target"), "required": ["x", "y"]}),
            },
            ToolDef {
                name: "drag",
                description: "Press the left button at `from`, move smoothly to `to`, and release. Coordinates in screenshot pixels.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "from": point_schema("Start point in screenshot pixels"),
                        "to": point_schema("End point in screenshot pixels"),
                    },
                    "required": ["from", "to"],
                }),
            },
            ToolDef {
                name: "scroll",
                description: "Move the pointer to (x, y) in screenshot pixels and scroll by dx/dy lines (positive dy = down, positive dx = right).",
                input_schema: json!({"type": "object", "properties": scroll_props, "required": ["x", "y"]}),
            },
            ToolDef {
                name: "type_text",
                description: "Type text into the focused element as keyboard input (Unicode-safe; newlines press Return, tabs press Tab). Click the field first to focus it.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"text": {"type": "string", "description": "Text to type"}},
                    "required": ["text"],
                }),
            },
            ToolDef {
                name: "key",
                description: "Press a key or shortcut, e.g. \"return\", \"escape\", \"tab\", \"up\", \"cmd+shift+t\", \"cmd+c\", \
\"alt+left\", \"f5\". Modifiers: cmd, shift, alt/option, ctrl, fn. Keys: letters, digits, punctuation, return, tab, space, \
delete (backspace), forwarddelete, escape, up/down/left/right, home, end, pageup, pagedown, f1-f12.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"combo": {"type": "string", "description": "Key combo joined with '+', e.g. \"cmd+shift+t\""}},
                    "required": ["combo"],
                }),
            },
            ToolDef {
                name: "open_app",
                description: "Launch or activate a macOS app by name (like `open -a`), e.g. \"Safari\", \"Notes\", \"Xcode\".",
                input_schema: json!({
                    "type": "object",
                    "properties": {"name": {"type": "string", "description": "Application name or path"}},
                    "required": ["name"],
                }),
            },
            ToolDef {
                name: "list_windows",
                description: "List on-screen app windows front to back with owner app, title, pid and bounds (in screen points and screenshot pixels).",
                input_schema: json!({"type": "object", "properties": {}}),
            },
            ToolDef {
                name: "wait",
                description: "Pause for `ms` milliseconds (max 10000), e.g. to let an app finish loading before the next screenshot.",
                input_schema: json!({
                    "type": "object",
                    "properties": {"ms": {"type": "integer", "minimum": 0, "maximum": 10000}},
                    "required": ["ms"],
                }),
            },
        ]
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        match name {
            "screenshot" => self.screenshot(args),
            "click" => self.click(args),
            "move_mouse" => self.move_mouse(args),
            "drag" => self.drag(args),
            "scroll" => self.scroll(args),
            "type_text" => self.type_text(args),
            "key" => self.key(args),
            "open_app" => self.open_app(args),
            "list_windows" => self.list_windows(),
            "wait" => {
                let ms = arg_f64(args, "ms")?;
                if !(0.0..=10_000.0).contains(&ms) {
                    return Err("ms must be between 0 and 10000".into());
                }
                std::thread::sleep(Duration::from_millis(ms as u64));
                Ok(vec![text(format!("Waited {} ms.", ms as u64))])
            }
            other => Err(format!("Unknown tool {other}")),
        }
    }
}

// ---- CoreGraphics helpers ----

fn require_accessibility() -> Result<(), String> {
    if unsafe { AXIsProcessTrusted() } != 0 {
        Ok(())
    } else {
        Err(ACCESSIBILITY_ERROR.into())
    }
}

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

struct WindowInfo {
    owner: String,
    title: String,
    pid: i64,
    id: i64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
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
    fn mapping_for_small_display_is_identity() {
        let m = Mapping::for_display(1512.0, 982.0);
        assert_eq!(m.scale, 1.0);
        assert_eq!((m.image_w, m.image_h), (1512, 982));
        assert_eq!(m.to_points(100.0, 200.0).unwrap(), (100.0, 200.0));
    }

    #[test]
    fn mapping_for_large_display_downscales() {
        let m = Mapping::for_display(2560.0, 1440.0);
        assert_eq!((m.image_w, m.image_h), (1568, 882));
        let (x, y) = m.to_points(1568.0, 882.0).unwrap();
        assert_eq!((x, y), (2559.0, 1439.0)); // clamped onto the display
        let (x, _) = m.to_points(784.0, 0.0).unwrap();
        assert!((x - 1280.0).abs() < 0.01);
        assert!(m.to_points(2000.0, 10.0).is_err());
        assert!(m.to_points(-50.0, 10.0).is_err());
    }

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
