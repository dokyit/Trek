//! `trek-mcp computer` — computer use on macOS and Windows: screenshots, the mouse and keyboard,
//! the window list and app launch. The tools, their arguments and what they answer are the same
//! on both; the platform does the work behind `desktop::Desktop`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::desktop::{self, Button, Desktop, WindowInfo};
use crate::keys;
use crate::rpc::{self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, point_schema, text};
use crate::util::{MAX_IMAGE_SIDE, fmt_num};

const ACCESSIBILITY_ERROR: &str = "Accessibility permission is missing, so trek-mcp cannot control the mouse or keyboard. \
macOS may be showing a permission prompt for trek-mcp now — allow it in System Settings ▸ Privacy & Security ▸ Accessibility and try again \
(no restart needed). If nothing asked, grant it to the app that launched the agent (Trek, or your terminal) in that same list.";

const INSTRUCTIONS: &str = if cfg!(windows) {
    "Windows computer use for the primary display. Call `screenshot` first: it returns an image plus \
the coordinate scale. All x/y arguments to click, move_mouse, drag and scroll are pixel coordinates in the most recent \
full-screen screenshot (origin top-left); trek-mcp converts them to screen pixels. Take a new screenshot after acting \
to verify the result. Prefer `key` shortcuts (ctrl+c, alt+tab, win+r) and `open_app` over hunting for UI when possible. \
Trek's own windows are off limits: clicks, drags and scrolls in them are refused, and so are keys while Trek is in front \
(bring the app you mean forward first)."
} else {
    "macOS computer use for the main display. Call `screenshot` first: it returns an image plus \
the coordinate scale. All x/y arguments to click, move_mouse, drag and scroll are pixel coordinates in the most recent \
full-screen screenshot (origin top-left); trek-mcp converts them to screen points. Take a new screenshot after acting \
to verify the result. Prefer `key` shortcuts and `open_app` over hunting for UI when possible. Trek's own windows \
are off limits: clicks, drags and scrolls in them are refused, and so are keys while Trek is in front (bring the app \
you mean forward first)."
};

/// What a screen coordinate is: macOS moves the mouse in points, Windows (per-monitor DPI aware)
/// in pixels.
const SCREEN_UNITS: &str = if cfg!(windows) { "pixels" } else { "points" };

const KEY_DESCRIPTION: &str = if cfg!(windows) {
    "Press a key or shortcut, e.g. \"return\", \"escape\", \"tab\", \"up\", \"ctrl+shift+t\", \"ctrl+c\", \
\"alt+tab\", \"win+r\", \"f5\". Modifiers: ctrl, shift, alt, win (cmd is taken as ctrl). Keys: letters, digits, punctuation, \
return, tab, space, delete (backspace), forwarddelete, escape, up/down/left/right, home, end, pageup, pagedown, f1-f12."
} else {
    "Press a key or shortcut, e.g. \"return\", \"escape\", \"tab\", \"up\", \"cmd+shift+t\", \"cmd+c\", \
\"alt+left\", \"f5\". Modifiers: cmd, shift, alt/option, ctrl, fn. Keys: letters, digits, punctuation, return, tab, space, \
delete (backspace), forwarddelete, escape, up/down/left/right, home, end, pageup, pagedown, f1-f12."
};

const OPEN_APP_DESCRIPTION: &str = if cfg!(windows) {
    "Launch a Windows app by name, e.g. \"Notepad\", \"Microsoft Edge\", \"Visual Studio Code\" (as named in the \
Start menu), or by program name or path, e.g. \"calc\", \"msedge\"."
} else {
    "Launch or activate a macOS app by name (like `open -a`), e.g. \"Safari\", \"Notes\", \"Xcode\"."
};

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

pub struct Computer {
    last: Option<Mapping>,
    /// Does the work: the real screen, mouse and keyboard, or in tests a fake that only records.
    desktop: Box<dyn Desktop>,
}

impl Default for Computer {
    fn default() -> Self {
        Self { last: None, desktop: Box::new(desktop::Native::default()) }
    }
}

/// Said when an agent points at Trek itself.
const OWN_WINDOW: &str = "That's in Trek's own window. Agents don't operate Trek itself: its approvals, questions and settings are the user's to answer. Ask the user, or work in another app's window.";

/// Said when the keys would go to Trek itself.
const OWN_KEYS: &str = "Trek's own window is in front, so the keys would go to Trek. Agents don't operate Trek itself. Bring the app you mean forward first (open_app, or a click in its window).";

/// Whether an app is Trek: the app's name on macOS, `trek.exe` on Windows, or a build run from
/// the repository.
fn is_trek_app(owner: &str) -> bool {
    owner.eq_ignore_ascii_case("trek")
}

fn is_trek(w: &WindowInfo) -> bool {
    is_trek_app(&w.owner)
}

/// The window a click at (`x`, `y`) in screen points would land in: the frontmost holding it.
fn window_at(windows: &[WindowInfo], x: f64, y: f64) -> Option<&WindowInfo> {
    windows.iter().find(|w| x >= w.x && x < w.x + w.w && y >= w.y && y < w.y + w.h)
}

impl Computer {
    /// Refuses a mouse action at any of `points` (screen points) that would land in a window
    /// of Trek's. An agent with the mouse could otherwise approve its own requests there, or
    /// give itself Full access in Settings. When the windows can't be listed, nothing is done:
    /// better no click than one nobody checked.
    fn keep_off_trek(&self, points: &[(f64, f64)]) -> Result<(), String> {
        let windows = self
            .desktop
            .windows()
            .map_err(|e| format!("Couldn't check which window that is ({e}), so nothing was done."))?;
        let in_trek = |&(x, y): &(f64, f64)| {
            window_at(&windows, x, y).is_some_and(is_trek) || self.desktop.owner_at(x, y).is_some_and(|o| is_trek_app(&o))
        };
        if points.iter().any(in_trek) {
            return Err(OWN_WINDOW.into());
        }
        Ok(())
    }

    /// Refuses keys while Trek's own window is the one in front (they'd be typed into Trek).
    fn keep_keys_off_trek(&self) -> Result<(), String> {
        let front = self
            .desktop
            .key_window()
            .map_err(|e| format!("Couldn't check which window is in front ({e}), so nothing was typed."))?;
        if front.as_ref().is_some_and(is_trek) {
            return Err(OWN_KEYS.into());
        }
        Ok(())
    }
}

/// Tools that post mouse or keyboard events, and so need Accessibility. Screenshots need Screen
/// Recording instead (checked when capturing); the rest need nothing.
fn needs_accessibility(tool: &str) -> bool {
    matches!(tool, "click" | "move_mouse" | "drag" | "scroll" | "type_text" | "key")
}

/// Longest side of a full-screen screenshot: the display's size in points (so one screenshot
/// pixel is one point where it fits), capped at `MAX_IMAGE_SIDE`. A Retina capture comes back at
/// twice that and is scaled down to it.
fn full_side(logical_w: f64, logical_h: f64) -> u32 {
    logical_w.max(logical_h).round().min(MAX_IMAGE_SIDE as f64) as u32
}

/// What the model is told with a full-screen screenshot.
fn full_info(iw: u32, ih: u32, lw: f64, lh: f64, physical: Option<(u64, u64)>, m: Mapping, note: &str) -> String {
    let physical = physical.map(|(pw, ph)| format!(", {pw}x{ph} physical pixels")).unwrap_or_default();
    let units = if cfg!(windows) { "pixels" } else { "logical points" };
    format!(
        "Screenshot of the main display: {iw}x{ih} px. Screen: {}x{} {units}{physical}. \
Scale: 1 screenshot px = {} {SCREEN_UNITS}. Pass screenshot pixel coordinates (origin top-left) to click, move_mouse, drag \
and scroll; trek-mcp converts them to screen {SCREEN_UNITS}.{note}",
        fmt_num(lw),
        fmt_num(lh),
        fmt_num(m.scale),
    )
}

/// What the model is told with a zoom: the region `(x0, y0, rw, rh)` in screen points, rendered
/// at `iw`×`ih`, and how to map a point in the zoom back to full-screenshot pixels.
fn region_info(region_pts: (f64, f64, f64, f64), iw: u32, ih: u32, m: Mapping, note: &str) -> String {
    let (x0, y0, rw, rh) = region_pts;
    let (sx, sy, sw, sh) = (x0 / m.scale, y0 / m.scale, rw / m.scale, rh / m.scale);
    format!(
        "Zoomed view of screenshot region x={} y={} width={} height={} (screen {SCREEN_UNITS} {},{} {}x{}), \
rendered at {iw}x{ih} px. This is for reading detail only: click/drag/scroll coordinates still use the full-screen \
screenshot space. A point (u, v) in this image is at full-screenshot ({} + u*{}, {} + v*{}).{note}",
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
    )
}

/// A zoom's area in screen points from a region in full-screenshot pixels.
fn region_points(m: Mapping, x: f64, y: f64, w: f64, h: f64) -> Result<(f64, f64, f64, f64), String> {
    if w <= 0.0 || h <= 0.0 {
        return Err("region width and height must be positive".into());
    }
    let (x0, y0) = m.to_points(x, y)?;
    let (x1, y1) = m.to_points(x + w, y + h)?;
    Ok((x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)))
}

impl Computer {
    fn mapping(&self) -> Mapping {
        self.last.unwrap_or_else(|| {
            let d = self.desktop.display();
            Mapping::for_display(d.width, d.height)
        })
    }

    fn screenshot(&mut self, args: &Value) -> ToolResult {
        let display = self.desktop.display();
        let (lw, lh) = (display.width, display.height);

        let region = match args.get("region") {
            None | Some(Value::Null) => None,
            Some(r) => Some((arg_f64(r, "x")?, arg_f64(r, "y")?, arg_f64(r, "width")?, arg_f64(r, "height")?)),
        };

        match region {
            None => {
                let shot = self.desktop.capture(None, full_side(lw, lh))?;
                let m = Mapping::from_image(lw, lh, shot.width, shot.height);
                self.last = Some(m);
                let info = full_info(shot.width, shot.height, lw, lh, display.physical, m, shot.note);
                Ok(vec![rpc::image_png(shot.png_base64), text(info)])
            }
            // Region → screen points, through the full-screen mapping.
            Some((x, y, w, h)) => {
                let region = region_points(self.mapping(), x, y, w, h)?;
                let shot = self.desktop.capture(Some(region), MAX_IMAGE_SIDE)?;
                let info = region_info(region, shot.width, shot.height, self.mapping(), shot.note);
                Ok(vec![rpc::image_png(shot.png_base64), text(info)])
            }
        }
    }

    fn click(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
        let count = opt_f64(args, "count")?.unwrap_or(1.0) as i64;
        if !(1..=3).contains(&count) {
            return Err("count must be 1 (single), 2 (double) or 3 (triple)".into());
        }
        let btn = match button {
            "left" => Button::Left,
            "right" => Button::Right,
            other => return Err(format!("Unknown button {other:?}; use \"left\" or \"right\"")),
        };
        let (px, py) = self.mapping().to_points(x, y)?;
        self.keep_off_trek(&[(px, py)])?;
        self.desktop.click(px, py, btn, count as u32)?;
        let what = match count {
            2 => "Double-clicked",
            3 => "Triple-clicked",
            _ => "Clicked",
        };
        Ok(vec![text(format!(
            "{what} {button} at ({}, {}) (screen {SCREEN_UNITS} {}, {}).",
            fmt_num(x),
            fmt_num(y),
            fmt_num(px),
            fmt_num(py)
        ))])
    }

    fn move_mouse(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let (px, py) = self.mapping().to_points(x, y)?;
        self.desktop.move_to(px, py)?;
        Ok(vec![text(format!("Moved mouse to ({}, {}).", fmt_num(x), fmt_num(y)))])
    }

    fn drag(&mut self, args: &Value) -> ToolResult {
        let (fx, fy) = arg_point(args, "from")?;
        let (tx, ty) = arg_point(args, "to")?;
        let m = self.mapping();
        let a = m.to_points(fx, fy)?;
        let b = m.to_points(tx, ty)?;
        self.keep_off_trek(&[a, b])?;
        self.desktop.drag(a, b)?;
        Ok(vec![text(format!(
            "Dragged from ({}, {}) to ({}, {}).",
            fmt_num(fx),
            fmt_num(fy),
            fmt_num(tx),
            fmt_num(ty)
        ))])
    }

    fn scroll(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let dx = opt_f64(args, "dx")?.unwrap_or(0.0).round() as i32;
        let dy = opt_f64(args, "dy")?.unwrap_or(0.0).round() as i32;
        if dx == 0 && dy == 0 {
            return Err("scroll needs a non-zero dx or dy".into());
        }
        let (px, py) = self.mapping().to_points(x, y)?;
        self.keep_off_trek(&[(px, py)])?;
        self.desktop.scroll(px, py, dx, dy)?;
        Ok(vec![text(format!(
            "Scrolled dx={dx} dy={dy} lines at ({}, {}).",
            fmt_num(x),
            fmt_num(y)
        ))])
    }

    fn type_text(&mut self, args: &Value) -> ToolResult {
        let s = arg_str(args, "text")?;
        if s.is_empty() {
            return Err("text is empty".into());
        }
        self.keep_keys_off_trek()?;
        self.desktop.type_text(s)?;
        Ok(vec![text(format!("Typed {} characters.", s.chars().count()))])
    }

    fn key(&mut self, args: &Value) -> ToolResult {
        let combo = arg_str(args, "combo")?;
        let parsed = keys::parse(combo)?;
        self.keep_keys_off_trek()?;
        self.desktop.key(&parsed)?;
        Ok(vec![text(format!("Pressed {combo}."))])
    }

    fn open_app(&mut self, args: &Value) -> ToolResult {
        let name = arg_str(args, "name")?.trim();
        if name.is_empty() {
            return Err("name is empty".into());
        }
        self.desktop.open_app(name).map_err(|e| format!("Could not open {name:?}: {e}"))?;
        Ok(vec![text(format!(
            "Opened {name}. Take a screenshot to see it (it may take a moment to appear)."
        ))])
    }

    fn list_windows(&mut self) -> ToolResult {
        let windows = self.desktop.windows()?;
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
        let titles = if cfg!(windows) { "" } else { " Window titles are empty without Screen Recording permission." };
        let units = if cfg!(windows) { " bounds_points is in screen pixels." } else { "" };
        let header = format!(
            "{} on-screen windows, front to back. bounds_screenshot uses screenshot pixel coordinates (scale {}).{units}{titles}",
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
                description: KEY_DESCRIPTION,
                input_schema: json!({
                    "type": "object",
                    "properties": {"combo": {"type": "string", "description": if cfg!(windows) { "Key combo joined with '+', e.g. \"ctrl+shift+t\"" } else { "Key combo joined with '+', e.g. \"cmd+shift+t\"" }}},
                    "required": ["combo"],
                }),
            },
            ToolDef {
                name: "open_app",
                description: OPEN_APP_DESCRIPTION,
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
        if needs_accessibility(name) && !self.desktop.may_post_input() {
            self.desktop.ask_to_post_input();
            return Err(ACCESSIBILITY_ERROR.into());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::Display;
    use crate::desktop::fake::{Event, Recording};

    /// A computer on `fake`, as if its last screenshot was of a 1512x982 display.
    fn on(fake: &Recording) -> Computer {
        Computer { last: Some(Mapping::for_display(1512.0, 982.0)), desktop: Box::new(fake.clone()) }
    }

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

    use crate::rpc::ToolSet as _;

    /// Arguments each input tool would act on. Only ever passed to a `Computer` that isn't
    /// trusted, so nothing reaches the real mouse or keyboard.
    fn acting_args(tool: &str) -> Value {
        match tool {
            "click" | "move_mouse" => json!({"x": 10, "y": 10}),
            "drag" => json!({"from": {"x": 10, "y": 10}, "to": {"x": 20, "y": 20}}),
            "scroll" => json!({"x": 10, "y": 10, "dy": 3}),
            "type_text" => json!({"text": "hi"}),
            "key" => json!({"combo": "cmd+c"}),
            other => panic!("{other} posts no events"),
        }
    }

    #[test]
    fn input_tools_need_accessibility_and_do_nothing_without_it() {
        let fake = Recording { trusted: false, ..Recording::new() };
        let mut c = on(&fake);
        for tool in ["click", "move_mouse", "drag", "scroll", "type_text", "key"] {
            assert!(needs_accessibility(tool));
            assert_eq!(c.call(tool, &acting_args(tool)), Err(ACCESSIBILITY_ERROR.to_string()), "{tool}");
        }
        assert_eq!(fake.events(), vec![], "nothing was posted");
        // Looking, launching and waiting need no Accessibility.
        for tool in ["screenshot", "list_windows", "open_app", "wait"] {
            assert!(!needs_accessibility(tool), "{tool}");
        }
        assert_eq!(c.call("wait", &json!({"ms": 0})), Ok(vec![text("Waited 0 ms.")]));
    }

    /// A window as `Desktop::windows` lists one.
    fn window(owner: &str, x: f64, y: f64, w: f64, h: f64) -> WindowInfo {
        WindowInfo { owner: owner.into(), title: String::new(), pid: 1, id: 1, x, y, w, h }
    }

    #[test]
    fn the_mouse_and_keys_stay_out_of_treks_own_windows() {
        // Trek's window in front, covering the left of the screen; a browser behind it.
        let trek_in_front = vec![window("Trek", 0.0, 0.0, 800.0, 900.0), window("Safari", 0.0, 0.0, 1512.0, 982.0)];
        let fake = Recording { windows: Ok(trek_in_front.clone()), ..Recording::new() };
        let mut c = on(&fake);
        // Every action that would land in it (or type into it) is refused before any event.
        assert_eq!(c.call("click", &json!({"x": 10, "y": 10})), Err(OWN_WINDOW.to_string()));
        assert_eq!(c.call("scroll", &json!({"x": 400, "y": 300, "dy": 3})), Err(OWN_WINDOW.to_string()));
        // A drag that starts outside and ends inside is refused too.
        assert_eq!(c.call("drag", &json!({"from": {"x": 1200, "y": 500}, "to": {"x": 100, "y": 100}})), Err(OWN_WINDOW.to_string()));
        assert_eq!(c.call("type_text", &json!({"text": "y"})), Err(OWN_KEYS.to_string()));
        assert_eq!(c.call("key", &json!({"combo": "return"})), Err(OWN_KEYS.to_string()));
        assert_eq!(fake.events(), vec![], "nothing reached the desktop");
        // Outside Trek's window the click goes through, to the point it was given.
        assert!(c.call("click", &json!({"x": 1200, "y": 500})).is_ok());
        assert_eq!(fake.events(), vec![Event::Click { x: 1200.0, y: 500.0, button: Button::Left, count: 1 }]);
        // The window a point is in is the frontmost one holding it; a dev build counts as Trek.
        assert_eq!(window_at(&trek_in_front, 10.0, 10.0).map(|w| w.owner.as_str()), Some("Trek"));
        assert_eq!(window_at(&trek_in_front, 1200.0, 500.0).map(|w| w.owner.as_str()), Some("Safari"));
        assert_eq!(window_at(&trek_in_front, 5000.0, 5000.0).map(|w| w.owner.as_str()), None);
        assert!(is_trek(&window("trek", 0.0, 0.0, 1.0, 1.0)) && !is_trek(&window("Trekking Maps", 0.0, 0.0, 1.0, 1.0)));
        // The windows can't be listed: nothing is done unchecked.
        let blind = Recording { windows: Err("no window list".into()), ..Recording::new() };
        let mut c = on(&blind);
        assert!(c.call("click", &json!({"x": 10, "y": 10})).is_err_and(|e| e.contains("nothing was done")));
        assert!(c.call("key", &json!({"combo": "return"})).is_err_and(|e| e.contains("nothing was typed")));
        assert_eq!(blind.events(), vec![]);
    }

    #[test]
    fn the_system_saying_a_point_is_treks_is_enough_to_refuse() {
        // The list's frontmost window there is an overlay covering the whole screen (NVIDIA's is,
        // on Windows, without being marked click-through), but the system's own hit test says the
        // click would reach Trek beneath it: refused.
        let fake = Recording {
            windows: Ok(vec![window("NVIDIA Overlay", 0.0, 0.0, 1512.0, 982.0), window("trek", 0.0, 0.0, 800.0, 900.0)]),
            owner_at: Some("trek".into()),
            ..Recording::new()
        };
        let mut c = on(&fake);
        assert_eq!(c.call("click", &json!({"x": 10, "y": 10})), Err(OWN_WINDOW.to_string()));
        assert_eq!(fake.events(), vec![]);
        // Keys go by the window in front, whatever is under the mouse.
        assert!(c.call("key", &json!({"combo": "escape"})).is_ok());
    }

    #[test]
    fn bad_arguments_are_refused_before_any_event() {
        // Trusted, but every call here fails its checks before posting anything.
        let fake = Recording::new();
        let mut c = on(&fake);
        let refused = |c: &mut Computer, tool: &str, args: Value| c.call(tool, &args).expect_err(tool);
        assert!(refused(&mut c, "click", json!({})).contains("`x`"));
        assert!(refused(&mut c, "click", json!({"x": 10, "y": 10, "count": 4})).contains("count must be"));
        assert!(refused(&mut c, "click", json!({"x": 10, "y": 10, "button": "middle"})).contains("Unknown button"));
        assert!(refused(&mut c, "click", json!({"x": 5000, "y": 10})).contains("outside the screenshot (1512x982)"));
        assert!(refused(&mut c, "move_mouse", json!({"x": 10, "y": -40})).contains("outside the screenshot"));
        assert!(refused(&mut c, "drag", json!({"from": {"x": 10, "y": 10}})).contains("`to`"));
        assert!(refused(&mut c, "drag", json!({"from": {"x": 10, "y": 10}, "to": {"x": 10, "y": 9000}})).contains("outside the screenshot"));
        assert!(refused(&mut c, "scroll", json!({"x": 10, "y": 10})).contains("non-zero dx or dy"));
        assert!(refused(&mut c, "type_text", json!({"text": ""})).contains("empty"));
        assert!(refused(&mut c, "key", json!({"combo": "cmd+nope"})).contains("nope"));
        assert!(refused(&mut c, "open_app", json!({"name": "  "})).contains("empty"));
        assert!(refused(&mut c, "wait", json!({"ms": 20_000})).contains("between 0 and 10000"));
        assert!(refused(&mut c, "teleport", json!({})).contains("Unknown tool"));
        assert_eq!(fake.events(), vec![]);
    }

    #[test]
    fn every_tool_has_an_object_schema_and_a_handler() {
        let fake = Recording { trusted: false, ..Recording::new() };
        let mut c = on(&fake);
        let tools = c.tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "names are unique");
        // The same tools on every platform.
        assert_eq!(
            names,
            vec!["click", "drag", "key", "list_windows", "move_mouse", "open_app", "screenshot", "scroll", "type_text", "wait"]
        );
        for t in &tools {
            let schema = t.to_json()["inputSchema"].clone();
            assert_eq!(schema["type"], "object", "{}", t.name);
            let props = schema["properties"].as_object().unwrap_or_else(|| panic!("{} has properties", t.name));
            for req in schema["required"].as_array().into_iter().flatten() {
                assert!(props.contains_key(req.as_str().unwrap()), "{} requires {req}, which it doesn't describe", t.name);
            }
            assert!(!t.description.is_empty());
            // Every listed tool is handled (screenshots and the window list are answered below).
            if !matches!(t.name, "screenshot" | "list_windows") {
                let err = c.call(t.name, &json!({})).expect_err(t.name);
                assert!(!err.contains("Unknown tool"), "{}: {err}", t.name);
            }
        }
        // The coordinates every pointer tool takes are screenshot pixels.
        for name in ["click", "move_mouse", "scroll"] {
            let t = tools.iter().find(|t| t.name == name).unwrap();
            assert_eq!(t.input_schema["required"], json!(["x", "y"]));
            assert!(t.input_schema["properties"]["x"]["description"].as_str().unwrap().contains("screenshot pixels"));
        }
        // Each platform's text names only its own keys and apps.
        let key = tools.iter().find(|t| t.name == "key").unwrap().description;
        let open = tools.iter().find(|t| t.name == "open_app").unwrap().description;
        if cfg!(windows) {
            assert!(key.contains("ctrl+c") && key.contains("win") && !key.contains("fn"), "{key}");
            assert!(!open.contains("macOS") && !INSTRUCTIONS.contains("macOS"), "{open}");
        } else {
            assert!(key.contains("cmd+c") && open.contains("macOS"));
        }
    }

    #[test]
    fn retina_screenshots_map_back_to_screen_points() {
        // 14" MacBook Pro at its default size: 1512x982 points, captured at 3024x1964 pixels and
        // scaled to 1512 wide, so a screenshot pixel is a point.
        assert_eq!(full_side(1512.0, 982.0), 1512);
        let m = Mapping::from_image(1512.0, 982.0, 1512, 982);
        assert_eq!(m.scale, 1.0);
        assert_eq!(m.to_points(756.0, 491.0), Ok((756.0, 491.0)));
        // 16" at 1728x1117 points (3456x2234 pixels): capped at 1568, aspect kept by `sips`.
        assert_eq!(full_side(1728.0, 1117.0), 1568);
        let m = Mapping::from_image(1728.0, 1117.0, 1568, 1014);
        assert_eq!((m.image_w, m.image_h), (Mapping::for_display(1728.0, 1117.0).image_w, Mapping::for_display(1728.0, 1117.0).image_h));
        let (x, y) = m.to_points(784.0, 507.0).unwrap();
        assert!((x - 864.0).abs() < 0.5 && (y - 558.5).abs() < 1.0, "the centre maps to the centre: {x}, {y}");
        assert_eq!(m.to_points(0.0, 0.0), Ok((0.0, 0.0)));
        assert_eq!(m.to_points(1568.0, 1014.0), Ok((1727.0, 1116.0)), "far corner stays on the display");
        // A portrait display: its height is the long side.
        assert_eq!(full_side(1080.0, 1920.0), 1568);
        let m = Mapping::for_display(1080.0, 1920.0);
        assert_eq!((m.image_w, m.image_h), (882, 1568));
        // A display smaller than the cap is never upscaled.
        assert_eq!(full_side(1024.0, 768.0), 1024);
    }

    #[test]
    fn zooms_say_how_to_map_back_and_reject_bad_regions() {
        let m = Mapping::from_image(1728.0, 1117.0, 1568, 1014);
        let region = region_points(m, 100.0, 200.0, 300.0, 150.0).unwrap();
        assert!((region.0 - 100.0 * m.scale).abs() < 1e-9 && (region.2 - 300.0 * m.scale).abs() < 1e-9, "{region:?}");
        // Captured on a Retina display: twice the points, in pixels.
        let (iw, ih) = ((region.2 * 2.0).round() as u32, (region.3 * 2.0).round() as u32);
        let info = region_info(region, iw, ih, m, "");
        assert!(info.contains("x=100 y=200 width=300 height=150"), "{info}");
        assert!(info.contains(&format!("(100 + u*{}, 200 + v*{})", fmt_num(300.0 / iw as f64), fmt_num(150.0 / ih as f64))), "{info}");
        assert!(region_points(m, 10.0, 10.0, 0.0, 5.0).unwrap_err().contains("positive"));
        assert!(region_points(m, 1500.0, 10.0, 400.0, 5.0).unwrap_err().contains("outside"));
        let full = full_info(1568, 1014, 1728.0, 1117.0, Some((3456, 2234)), m, "");
        assert!(full.contains("1568x1014 px") && full.contains("3456x2234 physical pixels"), "{full}");
        assert!(full.contains(&format!("1 screenshot px = {} {SCREEN_UNITS}", fmt_num(m.scale))));
    }

    #[test]
    fn a_1080p_pc_screen_maps_screenshot_pixels_to_screen_pixels() {
        // Windows reports the primary display in physical pixels (trek-mcp is per-monitor DPI
        // aware): a 1920x1080 screen at any scaling is shot at 1568x882.
        let fake = Recording { display: Display { width: 1920.0, height: 1080.0, physical: None }, ..Recording::new() };
        let mut c = Computer { last: None, desktop: Box::new(fake.clone()) };
        let shot = c.call("screenshot", &json!({})).unwrap();
        assert_eq!(fake.events(), vec![Event::Capture { region: None, max_side: 1568 }]);
        assert_eq!(shot[0], json!({"type": "image", "data": "UE5H", "mimeType": "image/png"}));
        let info = shot[1]["text"].as_str().unwrap();
        assert!(info.starts_with("Screenshot of the main display: 1568x882 px."), "{info}");
        // The screenshot's centre is the screen's; its far corner stays on it.
        c.call("click", &json!({"x": 784, "y": 441})).unwrap();
        c.call("move_mouse", &json!({"x": 1568, "y": 882})).unwrap();
        let events = fake.events();
        let Event::Click { x, y, .. } = events[1] else { panic!("{events:?}") };
        assert!((x - 960.0).abs() <= 1.0 && (y - 540.0).abs() <= 1.0, "{x}, {y}");
        let Event::Move(x, y) = events[2] else { panic!("{events:?}") };
        assert!((x - 1919.0).abs() <= 1.0 && (y - 1079.0).abs() <= 1.0, "{x}, {y}");
    }

    #[test]
    fn tools_answer_in_the_same_shapes_everywhere() {
        let fake = Recording {
            windows: Ok(vec![WindowInfo { owner: "Notepad".into(), title: "notes.txt".into(), pid: 42, id: 7, x: 100.0, y: 50.0, w: 800.0, h: 600.0 }]),
            ..Recording::new()
        };
        let mut c = on(&fake);
        let points = SCREEN_UNITS;
        assert_eq!(
            Value::Array(c.call("click", &json!({"x": 10, "y": 20, "button": "right", "count": 2})).unwrap()),
            json!([{"type": "text", "text": format!("Double-clicked right at (10, 20) (screen {points} 10, 20).")}])
        );
        assert_eq!(
            Value::Array(c.call("drag", &json!({"from": {"x": 1000, "y": 10}, "to": {"x": 1100, "y": 20}})).unwrap()),
            json!([{"type": "text", "text": "Dragged from (1000, 10) to (1100, 20)."}])
        );
        assert_eq!(
            Value::Array(c.call("scroll", &json!({"x": 1000, "y": 10, "dy": -3})).unwrap()),
            json!([{"type": "text", "text": "Scrolled dx=0 dy=-3 lines at (1000, 10)."}])
        );
        assert_eq!(
            Value::Array(c.call("type_text", &json!({"text": "héllo\n"})).unwrap()),
            json!([{"type": "text", "text": "Typed 6 characters."}])
        );
        assert_eq!(
            Value::Array(c.call("key", &json!({"combo": "ctrl+shift+t"})).unwrap()),
            json!([{"type": "text", "text": "Pressed ctrl+shift+t."}])
        );
        assert_eq!(
            Value::Array(c.call("open_app", &json!({"name": " Notepad "})).unwrap()),
            json!([{"type": "text", "text": "Opened Notepad. Take a screenshot to see it (it may take a moment to appear)."}])
        );
        assert_eq!(
            fake.events(),
            vec![
                Event::Click { x: 10.0, y: 20.0, button: Button::Right, count: 2 },
                Event::Drag { from: (1000.0, 10.0), to: (1100.0, 20.0) },
                Event::Scroll { x: 1000.0, y: 10.0, dx: 0, dy: -3 },
                Event::Type("héllo\n".into()),
                Event::Key(keys::parse("ctrl+shift+t").unwrap()),
                Event::OpenApp("Notepad".into()),
            ]
        );
        // The window list: a header, then one JSON object per window.
        let listed = c.call("list_windows", &json!({})).unwrap();
        let body = listed[0]["text"].as_str().unwrap();
        let mut lines = body.lines();
        assert!(lines.next().unwrap().starts_with("1 on-screen windows, front to back."));
        let w: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(
            w,
            json!({
                "app": "Notepad", "title": "notes.txt", "pid": 42, "window_id": 7,
                "bounds_points": {"x": 100.0, "y": 50.0, "width": 800.0, "height": 600.0},
                "bounds_screenshot": {"x": 100.0, "y": 50.0, "width": 800.0, "height": 600.0},
            })
        );
        // A zoom: the region in screen units, at most 1568 on its long side.
        let zoom = c.call("screenshot", &json!({"region": {"x": 100, "y": 200, "width": 300, "height": 150}})).unwrap();
        assert_eq!(zoom[0]["type"], "image");
        assert!(zoom[1]["text"].as_str().unwrap().starts_with("Zoomed view of screenshot region x=100 y=200 width=300 height=150"));
        assert_eq!(fake.events().last(), Some(&Event::Capture { region: Some((100.0, 200.0, 300.0, 150.0)), max_side: 1568 }));
    }
}
