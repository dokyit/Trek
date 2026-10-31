//! `trek-mcp computer` — computer use on macOS and Windows: screenshots, the mouse and keyboard,
//! the window list and app launch. The tools, their arguments and what they answer are the same
//! on both; the platform does the work behind `desktop::Desktop`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::desktop::{self, AppBundle, Button, Desktop, Input, RunningApp, WindowInfo};
use crate::keys;
use crate::rpc::{
    self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, point_schema, text,
};
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

/// Background use (macOS): the agent works in one window of its own, behind the user's.
const BACKGROUND_INSTRUCTIONS: &str = "macOS computer use in the background: you work in one app window of your own while the \
user keeps working in theirs. Your clicks, keys and scrolls reach only that window (the user's pointer and keyboard stay \
theirs, so never ask them to keep off the mouse). Pick the window first: open_app, or screenshot with a window_id from \
list_windows. A screenshot shows just that window, even when others cover it; x/y for click, move_mouse, drag and scroll \
are pixels in its latest screenshot (origin top-left). Take a new screenshot after acting. Trek's own windows are off \
limits. Work only in apps the user isn't using: one in front that they typed or pointed in within 30 seconds is theirs, \
and acting there waits for them. While they're away (idle, or the screen locked) any app will do. Some apps (games, \
canvases) ignore input not from the real mouse: if one doesn't respond, say so rather than retrying; the user can switch \
computer use to work in front in Trek's settings.";

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

const BACKGROUND_KEY_DESCRIPTION: &str = "Press a key or shortcut in the window you work in, in the background (the user's \
front app keeps the keyboard), e.g. \"return\", \"escape\", \"tab\", \"up\", \"cmd+shift+t\", \"cmd+c\", \"alt+left\", \"f5\". \
Modifiers: cmd, shift, alt/option, ctrl, fn. Keys: letters, digits, punctuation, return, tab, space, delete (backspace), \
forwarddelete, escape, up/down/left/right, home, end, pageup, pagedown, f1-f12.";

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

/// How the agent works the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The real pointer and keyboard, on the whole screen.
    Front,
    /// In one window of its own, behind the user's (macOS): see `BACKGROUND_INSTRUCTIONS`.
    Background,
}

pub struct Computer {
    last: Option<Mapping>,
    /// Does the work: the real screen, mouse and keyboard, or in tests a fake that only records.
    desktop: Box<dyn Desktop>,
    mode: Mode,
    /// In the background: the window the agent works in, and how its latest screenshot maps to it.
    target: Option<Target>,
    /// Tells Trek what it's doing in the background, for its picture-in-picture window.
    report: Option<Reporter>,
    /// When input was last posted behind, and how idle the user was just before (`user_idle`).
    posted: Option<(std::time::Instant, Option<Duration>)>,
}

/// The window background use works in. An app with no window yet has `id` 0: keys reach it,
/// nothing else does.
struct Target {
    window: WindowInfo,
    /// Window points per pixel of its latest screenshot; `None` until it has one.
    shot: Option<Mapping>,
}

impl Default for Computer {
    fn default() -> Self {
        let desktop: Box<dyn Desktop> = Box::new(desktop::Native::default());
        let asked = std::env::var(trek_ipc::ENV_COMPUTER_MODE)
            .is_ok_and(|m| m == trek_ipc::COMPUTER_BACKGROUND);
        let mode = if asked && desktop.works_behind() {
            Mode::Background
        } else {
            Mode::Front
        };
        let report = (mode == Mode::Background)
            .then(trek_ipc::Client::from_env)
            .flatten()
            .map(Reporter::start);
        Self {
            last: None,
            desktop,
            mode,
            target: None,
            report,
            posted: None,
        }
    }
}

impl Computer {
    /// Computer use as a session given it in the background sees it (`prompt_budget`).
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn in_background() -> Self {
        Self {
            mode: Mode::Background,
            ..Self::default()
        }
    }
}

/// Tells Trek what background use is doing (`trek_ipc::COMPUTER_ACTIVITY`), on a thread of its
/// own and best effort: a Trek that's slow or gone never holds up or fails a tool call. When
/// reports pile up only the latest is sent.
struct Reporter {
    tx: std::sync::mpsc::Sender<Value>,
}

impl Reporter {
    fn start(client: trek_ipc::Client) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let mut conn: Option<trek_ipc::Connection> = None;
            while let Ok(mut activity) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    activity = newer;
                }
                if conn.is_none() {
                    conn = client.connect().ok();
                }
                if conn
                    .as_mut()
                    .is_some_and(|c| c.call(trek_ipc::COMPUTER_ACTIVITY, &activity).is_err())
                {
                    conn = None;
                }
            }
        });
        Self { tx }
    }
}

/// A report of one step in window `w`: `point` (and a drag's `from`) in its own coordinates.
fn activity(
    action: &str,
    detail: String,
    w: &WindowInfo,
    point: Option<(f64, f64)>,
    from: Option<(f64, f64)>,
) -> Value {
    let at = |p: Option<(f64, f64)>| p.map(|(x, y)| json!({"x": x, "y": y}));
    json!({
        "action": action,
        "detail": detail,
        "window": {"id": w.id, "pid": w.pid, "app": w.owner, "title": w.title, "x": w.x, "y": w.y, "width": w.w, "height": w.h},
        "point": at(point),
        "from": at(from),
    })
}

/// What's typed, as the picture-in-picture window says it: the start of it, on one line.
fn typing(text: &str) -> String {
    const SHOWN: usize = 32;
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one_line = one_line.trim();
    let mut shown: String = one_line.chars().take(SHOWN).collect();
    if one_line.chars().count() > SHOWN {
        shown.push('…');
    }
    format!("Typing “{shown}”")
}

/// Whether window owner `owner` is the app the agent named (`Safari`, `/Applications/Safari.app`,
/// `chrome` for Google Chrome).
fn is_app(owner: &str, name: &str) -> bool {
    let name = name.trim().trim_end_matches('/');
    let name = name.rsplit('/').next().unwrap_or(name);
    let name = name.strip_suffix(".app").unwrap_or(name).to_lowercase();
    let owner = owner.to_lowercase();
    !name.is_empty() && (owner == name || (name.chars().count() >= 4 && owner.contains(&name)))
}

/// How long `open_app` waits in the background for the app's window to appear.
const WINDOW_WAIT: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 8000 });

/// Said when an agent points at Trek itself.
const OWN_WINDOW: &str = "That's in Trek's own window. Agents don't operate Trek itself: its approvals, questions and settings are the user's to answer. Ask the user, or work in another app's window.";

/// Said when the agent picks one of Trek's windows to work in, in the background.
const OWN_WINDOW_BEHIND: &str = "That's one of Trek's own windows. Agents don't operate Trek itself: its approvals, questions and settings are the user's to answer. Ask the user, or work in another app's window.";

/// Said when an action in the background has no window to go to.
const NO_WINDOW: &str = "No window picked yet. In the background you work in one window of your own: call open_app, or list_windows and then screenshot with a window_id.";

/// Said when the window background use works in has changed size since its screenshot.
const RESIZED: &str = "The window changed size since your last screenshot of it, so those coordinates may be off. Take a new screenshot first.";

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
    windows
        .iter()
        .find(|w| x >= w.x && x < w.x + w.w && y >= w.y && y < w.y + w.h)
}

impl Computer {
    /// Refuses a mouse action at any of `points` (screen points) that would land in a window
    /// of Trek's. An agent with the mouse could otherwise approve its own requests there, or
    /// give itself Full access in Settings. When the windows can't be listed, nothing is done:
    /// better no click than one nobody checked.
    fn keep_off_trek(&self, points: &[(f64, f64)]) -> Result<(), String> {
        let windows = self.desktop.windows().map_err(|e| {
            format!("Couldn't check which window that is ({e}), so nothing was done.")
        })?;
        let in_trek = |&(x, y): &(f64, f64)| {
            window_at(&windows, x, y).is_some_and(is_trek)
                || self.desktop.owner_at(x, y).is_some_and(|o| is_trek_app(&o))
        };
        if points.iter().any(in_trek) {
            return Err(OWN_WINDOW.into());
        }
        Ok(())
    }

    /// Refuses keys while Trek's own window is the one in front (they'd be typed into Trek).
    fn keep_keys_off_trek(&self) -> Result<(), String> {
        let front = self.desktop.key_window().map_err(|e| {
            format!("Couldn't check which window is in front ({e}), so nothing was typed.")
        })?;
        if front.as_ref().is_some_and(is_trek) {
            return Err(OWN_KEYS.into());
        }
        Ok(())
    }
}

/// Tools that post mouse or keyboard events, and so need Accessibility. Screenshots need Screen
/// Recording instead (checked when capturing); the rest need nothing.
fn needs_accessibility(tool: &str) -> bool {
    matches!(
        tool,
        "click" | "move_mouse" | "drag" | "scroll" | "type_text" | "key"
    )
}

/// Longest side of a full-screen screenshot: the display's size in points (so one screenshot
/// pixel is one point where it fits), capped at `MAX_IMAGE_SIDE`. A Retina capture comes back at
/// twice that and is scaled down to it.
fn full_side(logical_w: f64, logical_h: f64) -> u32 {
    logical_w.max(logical_h).round().min(MAX_IMAGE_SIDE as f64) as u32
}

/// What the model is told with a full-screen screenshot.
fn full_info(
    iw: u32,
    ih: u32,
    lw: f64,
    lh: f64,
    physical: Option<(u64, u64)>,
    m: Mapping,
    note: &str,
) -> String {
    let physical = physical
        .map(|(pw, ph)| format!(", {pw}x{ph} physical pixels"))
        .unwrap_or_default();
    let units = if cfg!(windows) {
        "pixels"
    } else {
        "logical points"
    };
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
fn region_info(
    region_pts: (f64, f64, f64, f64),
    iw: u32,
    ih: u32,
    m: Mapping,
    note: &str,
) -> String {
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
fn region_points(
    m: Mapping,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(f64, f64, f64, f64), String> {
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
            Some(r) => Some((
                arg_f64(r, "x")?,
                arg_f64(r, "y")?,
                arg_f64(r, "width")?,
                arg_f64(r, "height")?,
            )),
        };

        match region {
            None => {
                let shot = self.desktop.capture(None, full_side(lw, lh))?;
                let m = Mapping::from_image(lw, lh, shot.width, shot.height);
                self.last = Some(m);
                let info = full_info(
                    shot.width,
                    shot.height,
                    lw,
                    lh,
                    display.physical,
                    m,
                    shot.note,
                );
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
            other => {
                return Err(format!(
                    "Unknown button {other:?}; use \"left\" or \"right\""
                ));
            }
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
        Ok(vec![text(format!(
            "Moved mouse to ({}, {}).",
            fmt_num(x),
            fmt_num(y)
        ))])
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
        Ok(vec![text(format!(
            "Typed {} characters.",
            s.chars().count()
        ))])
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
        self.desktop
            .open_app(name)
            .map_err(|e| format!("Could not open {name:?}: {e}"))?;
        Ok(vec![text(format!(
            "Opened {name}. Take a screenshot to see it (it may take a moment to appear)."
        ))])
    }

    fn list_windows(&mut self) -> ToolResult {
        let windows = self.desktop.windows()?;
        let m = self.mapping();
        let working_in = self
            .target
            .as_ref()
            .map(|t| t.window.id)
            .filter(|_| self.mode == Mode::Background);
        let list: Vec<Value> = windows
            .iter()
            .map(|w| {
                let mut v = json!({
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
                });
                if working_in == Some(w.id) {
                    v["working_in"] = json!(true);
                }
                v
            })
            .collect();
        let titles = if cfg!(windows) {
            ""
        } else {
            " Window titles are empty without Screen Recording permission."
        };
        let units = if cfg!(windows) {
            " bounds_points is in screen pixels."
        } else {
            ""
        };
        let header = match self.mode {
            Mode::Front => format!(
                "{} on-screen windows, front to back. bounds_screenshot uses screenshot pixel coordinates (scale {}).{units}{titles}",
                list.len(),
                fmt_num(m.scale)
            ),
            Mode::Background => format!(
                "{} on-screen windows, front to back. In the background you work in one of them: pass its window_id to screenshot (working_in marks the one you're in). Trek's windows are off limits.{titles}",
                list.len()
            ),
        };
        let lines: Vec<String> = list.iter().map(Value::to_string).collect();
        Ok(vec![text(format!("{header}\n{}", lines.join("\n")))])
    }
}

/// Background use: one window, behind the user's.
impl Computer {
    fn tell(&self, activity: Value) {
        if let Some(r) = &self.report {
            let _ = r.tx.send(activity);
        }
    }

    /// The window being worked in, as it is now. Gone (closed, minimised, on another Space):
    /// it's forgotten, and the agent is told to pick another.
    fn working_window(&mut self) -> Result<WindowInfo, String> {
        let Some(t) = &self.target else {
            return Err(NO_WINDOW.into());
        };
        if t.window.id == 0 {
            return Err(format!(
                "{} has no window yet, so there's nothing to point at: keys reach it (cmd+n opens a window in most apps), then list_windows and screenshot with its window_id.",
                t.window.owner
            ));
        }
        let (id, owner, title) = (t.window.id, t.window.owner.clone(), t.window.title.clone());
        match self.desktop.window(id)? {
            Some(w) if is_trek(&w) => Err(OWN_WINDOW_BEHIND.into()),
            Some(w) => {
                if let Some(t) = &mut self.target {
                    t.window = w.clone();
                }
                Ok(w)
            }
            None => {
                self.target = None;
                let title = if title.is_empty() {
                    String::new()
                } else {
                    format!(" “{title}”")
                };
                Err(format!(
                    "{owner}'s window{title} has closed or left the screen (minimised, or on another Space). Call list_windows and screenshot another window_id, or open_app."
                ))
            }
        }
    }

    /// The working window and the mapping of its latest screenshot, for a pointer action.
    fn pointing(&mut self) -> Result<(WindowInfo, Mapping), String> {
        let w = self.working_window()?;
        let m = self.target.as_ref().and_then(|t| t.shot).ok_or(
            "Take a screenshot of the window first: x/y are pixels in its latest screenshot.",
        )?;
        if (m.logical_w - w.w).abs() > 1.0 || (m.logical_h - w.h).abs() > 1.0 {
            return Err(RESIZED.into());
        }
        self.keep_out_of_users_app(&w)?;
        Ok((w, m))
    }

    fn bg_screenshot(&mut self, args: &Value) -> ToolResult {
        let region = match args.get("region") {
            None | Some(Value::Null) => None,
            Some(r) => Some((
                arg_f64(r, "x")?,
                arg_f64(r, "y")?,
                arg_f64(r, "width")?,
                arg_f64(r, "height")?,
            )),
        };
        if let Some(id) = opt_f64(args, "window_id")? {
            let id = id as i64;
            let w = self.desktop.window(id)?.ok_or_else(|| format!("No window {id} is on screen. list_windows shows the windows there are (a minimised window, or one on another Space, isn't)."))?;
            if is_trek(&w) {
                return Err(OWN_WINDOW_BEHIND.into());
            }
            if self.target.as_ref().is_none_or(|t| t.window.id != id) {
                self.target = Some(Target {
                    window: w,
                    shot: None,
                });
            }
        }
        // Nothing picked yet: the whole screen, for finding the way.
        if self.target.as_ref().is_none_or(|t| t.window.id == 0) {
            if region.is_some() {
                return Err(NO_WINDOW.into());
            }
            let mut shot = self.screenshot(args)?;
            shot.push(text("Background use: this is the whole screen, to find your way. To act, work in one window: open_app, or list_windows and then screenshot with its window_id."));
            return Ok(shot);
        }
        let w = self.working_window()?;
        let title = if w.title.is_empty() {
            String::new()
        } else {
            format!(" “{}”", w.title)
        };
        match region {
            None => {
                let shot = self.desktop.capture_window(&w, None, full_side(w.w, w.h))?;
                let m = Mapping::from_image(w.w, w.h, shot.width, shot.height);
                if let Some(t) = &mut self.target {
                    t.shot = Some(m);
                }
                let theirs = self.users_window().is_some_and(|f| f.pid == w.pid);
                let (doing, note) = if theirs {
                    (
                        waiting(&w.owner),
                        format!(
                            " {} is the app the user is using right now: clicks and keys there wait until they switch to another app (ask them, or open_app with new_window: true).",
                            w.owner
                        ),
                    )
                } else {
                    ("Looking".to_string(), String::new())
                };
                self.tell(activity("screenshot", doing, &w, None, None));
                let info = format!(
                    "Screenshot of {}'s window{title} (window_id {}), worked in the background: {}x{} px. The window is {}x{} points. \
Scale: 1 screenshot px = {} points. Pass pixel coordinates in this screenshot (origin at the window's top-left) to click, \
move_mouse, drag and scroll: they go to this window alone, behind the user's, without moving their pointer.{}{note}",
                    w.owner,
                    w.id,
                    shot.width,
                    shot.height,
                    fmt_num(w.w),
                    fmt_num(w.h),
                    fmt_num(m.scale),
                    shot.note
                );
                Ok(vec![rpc::image_png(shot.png_base64), text(info)])
            }
            Some((x, y, rw, rh)) => {
                let (_, m) = self.pointing()?;
                let region = region_points(m, x, y, rw, rh)?;
                let shot = self
                    .desktop
                    .capture_window(&w, Some(region), MAX_IMAGE_SIDE)?;
                self.tell(activity(
                    "screenshot",
                    "Looking closer".into(),
                    &w,
                    Some((region.0 + region.2 / 2.0, region.1 + region.3 / 2.0)),
                    None,
                ));
                let info = region_info(region, shot.width, shot.height, m, shot.note)
                    .replace("full-screen screenshot", "window's screenshot")
                    .replace("full-screenshot", "window-screenshot")
                    .replace(
                        &format!("(screen {SCREEN_UNITS}"),
                        &format!("(window {SCREEN_UNITS}"),
                    );
                Ok(vec![rpc::image_png(shot.png_base64), text(info)])
            }
        }
    }

    fn bg_click(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let button = args.get("button").and_then(Value::as_str).unwrap_or("left");
        let count = opt_f64(args, "count")?.unwrap_or(1.0) as i64;
        if !(1..=3).contains(&count) {
            return Err("count must be 1 (single), 2 (double) or 3 (triple)".into());
        }
        let btn = match button {
            "left" => Button::Left,
            "right" => Button::Right,
            other => {
                return Err(format!(
                    "Unknown button {other:?}; use \"left\" or \"right\""
                ));
            }
        };
        let (w, m) = self.pointing()?;
        let (px, py) = m.to_points(x, y)?;
        self.post(
            &w,
            &Input::Click {
                x: px,
                y: py,
                button: btn,
                count: count as u32,
            },
        )?;
        let what = match (count, btn) {
            (_, Button::Right) => "Right-clicked",
            (2, _) => "Double-clicked",
            (3, _) => "Triple-clicked",
            _ => "Clicked",
        };
        let doing = match (count, btn) {
            (_, Button::Right) => "Right-clicking",
            (2, _) => "Double-clicking",
            (3, _) => "Triple-clicking",
            _ => "Clicking",
        };
        self.tell(activity("click", doing.into(), &w, Some((px, py)), None));
        Ok(vec![text(format!(
            "{what} at ({}, {}) in {}'s window (window {SCREEN_UNITS} {}, {}), in the background.",
            fmt_num(x),
            fmt_num(y),
            w.owner,
            fmt_num(px),
            fmt_num(py)
        ))])
    }

    fn bg_move(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let (w, m) = self.pointing()?;
        let (px, py) = m.to_points(x, y)?;
        self.post(&w, &Input::Move(px, py))?;
        self.tell(activity(
            "move",
            "Pointing".into(),
            &w,
            Some((px, py)),
            None,
        ));
        Ok(vec![text(format!(
            "Moved the pointer to ({}, {}) in {}'s window, in the background (the user's pointer stays put).",
            fmt_num(x),
            fmt_num(y),
            w.owner
        ))])
    }

    fn bg_drag(&mut self, args: &Value) -> ToolResult {
        let (fx, fy) = arg_point(args, "from")?;
        let (tx, ty) = arg_point(args, "to")?;
        let (w, m) = self.pointing()?;
        let a = m.to_points(fx, fy)?;
        let b = m.to_points(tx, ty)?;
        self.post(&w, &Input::Drag { from: a, to: b })?;
        self.tell(activity("drag", "Dragging".into(), &w, Some(b), Some(a)));
        Ok(vec![text(format!(
            "Dragged from ({}, {}) to ({}, {}) in {}'s window, in the background.",
            fmt_num(fx),
            fmt_num(fy),
            fmt_num(tx),
            fmt_num(ty),
            w.owner
        ))])
    }

    fn bg_scroll(&mut self, args: &Value) -> ToolResult {
        let (x, y) = (arg_f64(args, "x")?, arg_f64(args, "y")?);
        let dx = opt_f64(args, "dx")?.unwrap_or(0.0).round() as i32;
        let dy = opt_f64(args, "dy")?.unwrap_or(0.0).round() as i32;
        if dx == 0 && dy == 0 {
            return Err("scroll needs a non-zero dx or dy".into());
        }
        let (w, m) = self.pointing()?;
        let (px, py) = m.to_points(x, y)?;
        self.post(
            &w,
            &Input::Scroll {
                x: px,
                y: py,
                dx,
                dy,
            },
        )?;
        let way = match (dy.signum(), dx.signum()) {
            (1, _) => "down",
            (-1, _) => "up",
            (_, 1) => "right",
            _ => "left",
        };
        self.tell(activity(
            "scroll",
            format!("Scrolling {way}"),
            &w,
            Some((px, py)),
            None,
        ));
        Ok(vec![text(format!(
            "Scrolled dx={dx} dy={dy} lines at ({}, {}) in {}'s window, in the background.",
            fmt_num(x),
            fmt_num(y),
            w.owner
        ))])
    }

    /// The window (or windowless app) keys go to.
    fn keyed(&mut self) -> Result<WindowInfo, String> {
        let w = match &self.target {
            None => Err(NO_WINDOW.into()),
            Some(t) if t.window.id == 0 => Ok(t.window.clone()),
            Some(_) => self.working_window(),
        }?;
        self.keep_out_of_users_app(&w)?;
        Ok(w)
    }

    fn bg_type(&mut self, args: &Value) -> ToolResult {
        let s = arg_str(args, "text")?;
        if s.is_empty() {
            return Err("text is empty".into());
        }
        let w = self.keyed()?;
        self.post(&w, &Input::Type(s.to_string()))?;
        self.tell(activity("type", typing(s), &w, None, None));
        Ok(vec![text(format!(
            "Typed {} characters into {}, in the background.",
            s.chars().count(),
            w.owner
        ))])
    }

    fn bg_key(&mut self, args: &Value) -> ToolResult {
        let combo = arg_str(args, "combo")?;
        let parsed = keys::parse(combo)?;
        let w = self.keyed()?;
        self.post(&w, &Input::Key(parsed))?;
        self.tell(activity("key", format!("Pressing {combo}"), &w, None, None));
        Ok(vec![text(format!(
            "Pressed {combo} in {}, in the background.",
            w.owner
        ))])
    }

    /// The window the user is at right now, when they are: the front window (not Trek's), with
    /// input from their keyboard, pointer or trackpad in the last `IN_USE`, the screen unlocked.
    /// `None` when they're away, or the platform can't tell.
    fn users_window(&self) -> Option<WindowInfo> {
        if self.desktop.screen_locked() || self.user_idle()? >= IN_USE {
            return None;
        }
        self.desktop
            .key_window()
            .ok()
            .flatten()
            .filter(|w| !is_trek(w))
    }

    /// How long since the user's own last input. The system's count can include the events this
    /// agent posts (measured on macOS 27: a key sent behind sometimes resets it), so a last event
    /// that falls when ours did counts as ours, and the user's idle time is what it was then
    /// plus the time since.
    fn user_idle(&self) -> Option<Duration> {
        let raw = self.desktop.user_idle()?;
        match self.posted {
            Some((at, before)) => {
                let since = at.elapsed();
                let ours = raw + Duration::from_millis(50) >= since
                    && raw <= since + Duration::from_millis(500);
                if ours {
                    before.map(|b| b + since)
                } else {
                    Some(raw)
                }
            }
            None => Some(raw),
        }
    }

    /// Deliver `input` behind, noting when (and how idle the user was just before) for
    /// `user_idle`.
    fn post(&mut self, w: &WindowInfo, input: &Input) -> Result<(), String> {
        let before = self.user_idle();
        self.desktop.behind(w, input)?;
        self.posted = Some((std::time::Instant::now(), before));
        Ok(())
    }

    /// Refuses to drive `w` while it belongs to the app the user is using right now: their
    /// typing and the agent's would land in the same app. Tells Trek it's waiting.
    fn keep_out_of_users_app(&self, w: &WindowInfo) -> Result<(), String> {
        let Some(front) = self.users_window().filter(|f| f.pid == w.pid) else {
            return Ok(());
        };
        let shown = if w.id == 0 { &front } else { w };
        self.tell(activity("waiting", waiting(&w.owner), shown, None, None));
        Err(in_use(&w.owner))
    }

    /// Keeps `w` behind `front`, the window that was in front before `w` opened: apps often bring
    /// a new window forward themselves, the moment it appears or a little after. Up to three
    /// times it's put back. What happened, for the reply: `None` when nothing needed doing.
    fn keep_behind(&self, w: &WindowInfo, front: Option<&WindowInfo>) -> Option<String> {
        let front = front.filter(|f| f.id != w.id)?;
        let deadline = std::time::Instant::now() + SETTLE;
        let mut put_back = 0;
        loop {
            let order = self.desktop.windows().unwrap_or_default();
            let at = |id: i64| order.iter().position(|x| x.id == id);
            let (Some(theirs), Some(ours)) = (at(front.id), at(w.id)) else {
                return None;
            };
            if ours < theirs {
                if put_back == 3 {
                    return Some(format!(
                        " Note: {}'s window came to the front when it opened and stays there, in front of the user's {} window rather than behind it. Tell the user it's in their way.",
                        w.owner, front.owner
                    ));
                }
                if let Err(e) = self.desktop.bring_forward(front) {
                    return Some(format!(
                        " Note: {}'s window came to the front when it opened, in front of the user's {} window, and couldn't be put back ({e}). Tell the user it's in their way.",
                        w.owner, front.owner
                    ));
                }
                put_back += 1;
                continue;
            }
            if std::time::Instant::now() >= deadline {
                return (put_back > 0).then(|| {
                    format!(
                        " {} brought its window forward; it was put back behind the user's {} window.",
                        w.owner, front.owner
                    )
                });
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Waits up to `wait` for a window of process `pid` that isn't one of `before`. `pid` 0: any
    /// window of an app called `name`.
    fn new_window_of(
        &self,
        pid: i64,
        name: &str,
        before: &[i64],
        wait: Duration,
    ) -> Option<WindowInfo> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let found = self
                .desktop
                .windows()
                .unwrap_or_default()
                .into_iter()
                .filter(|w| !is_trek(w) && !before.contains(&w.id))
                .find(|w| {
                    if pid > 0 {
                        w.pid == pid
                    } else {
                        is_app(&w.owner, name)
                    }
                });
            if found.is_some() || std::time::Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Work in `w` from now on.
    fn work_in(&mut self, w: WindowInfo) {
        self.tell(activity(
            "open_app",
            format!("Opening {}", w.owner),
            &w,
            None,
            None,
        ));
        self.target = Some(Target {
            window: w,
            shot: None,
        });
    }

    /// An app that's open with no window: keys reach it, nothing else does.
    fn work_in_windowless(&mut self, owner: &str, pid: i64) {
        self.target = Some(Target {
            window: WindowInfo {
                owner: owner.to_string(),
                title: String::new(),
                pid,
                id: 0,
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            shot: None,
        });
    }

    /// Open the app behind the user's windows and work in a window of it. Never a second
    /// instance: a running app's own window is used (or it's asked for one); only an app that
    /// isn't running is launched, from the copy `pick` chooses, by its path.
    fn bg_open_app(&mut self, args: &Value) -> ToolResult {
        let name = arg_str(args, "name")?.trim();
        if name.is_empty() {
            return Err("name is empty".into());
        }
        if is_trek_app(name) || name.trim_end_matches('/').ends_with("/Trek.app") {
            return Err(OWN_WINDOW_BEHIND.into());
        }
        let new_window = args
            .get("new_window")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let windows = self.desktop.windows().unwrap_or_default();
        let running = self.desktop.running_apps();
        match pick(name, &running, &windows, || {
            self.desktop.installed_apps(name)
        }) {
            Pick::Running(app) => self.open_running(app, new_window, &windows, &running),
            Pick::Launch(copy) => self.launch(name, copy, &windows),
            Pick::Missing => Err(format!(
                "No app called {name:?} is running or installed. list_windows shows what's open; pass an app's name as the Dock shows it, its bundle id or its path."
            )),
        }
    }

    /// `open_app` for an app that's already running: its window, or one more of it.
    fn open_running(
        &mut self,
        app: RunningApp,
        new_window: bool,
        windows: &[WindowInfo],
        running: &[RunningApp],
    ) -> ToolResult {
        let theirs: Vec<WindowInfo> = windows
            .iter()
            .filter(|w| w.pid == app.pid && !is_trek(w))
            .cloned()
            .collect();
        let from = app
            .path
            .as_deref()
            .map(|p| format!(", {p}"))
            .unwrap_or_default();
        let was = format!("{} was already running (pid {}{from})", app.name, app.pid);
        let users = self.users_window();
        if !new_window && let Some(front) = users.as_ref().filter(|f| f.pid == app.pid) {
            self.tell(activity("waiting", waiting(&app.name), front, None, None));
            return Err(format!("{} {was}.", in_use(&app.name)));
        }
        if !new_window && let Some(w) = theirs.first().cloned() {
            let reply = format!(
                "{was}, so nothing was launched. Working in its window{} (window_id {}), behind the user's: take a screenshot to see it.",
                titled(&w),
                w.id
            );
            self.work_in(w);
            return Ok(vec![text(reply)]);
        }
        // Ask the running app for a window: a reopen when it has none (as a click on its Dock
        // icon does), else its own New Window (cmd+n), sent to it alone.
        let front = self.desktop.key_window().ok().flatten();
        let before: Vec<i64> = theirs.iter().map(|w| w.id).collect();
        let asked = if theirs.is_empty() {
            self.desktop
                .reopen_behind(&app)
                .map(|()| "asked it for a window")
        } else {
            let keys = WindowInfo {
                owner: app.name.clone(),
                title: String::new(),
                pid: app.pid,
                id: 0,
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            };
            let cmd_n = keys::parse("cmd+n")?;
            self.post(&keys, &Input::Key(cmd_n))
                .map(|()| "pressed cmd+n in it for one more window")
        }
        .map_err(|e| format!("{was}, but asking it for a window failed: {e}"))?;
        // Never a second instance: say so if the system started one anyway.
        let others = |apps: &[RunningApp]| -> Vec<i64> {
            apps.iter()
                .filter(|a| a.pid != app.pid && same_app(a, &app))
                .map(|a| a.pid)
                .collect()
        };
        let (had, has) = (others(running), others(&self.desktop.running_apps()));
        let second = has
            .iter()
            .find(|p| !had.contains(p))
            .map(|p| {
                format!(
                    " Warning: macOS started a second copy of {} (pid {p}); tell the user.",
                    app.name
                )
            })
            .unwrap_or_default();
        match self.new_window_of(app.pid, &app.name, &before, WINDOW_WAIT) {
            Some(w) => {
                let order = self.keep_behind(&w, front.as_ref()).unwrap_or_default();
                let reply = format!(
                    "{was}, so nothing was launched: {asked}. Working in its new window{} (window_id {}): take a screenshot to see it.{order}{second}",
                    titled(&w),
                    w.id
                );
                self.work_in(w);
                Ok(vec![text(reply)])
            }
            None => {
                self.work_in_windowless(&app.name, app.pid);
                Ok(vec![text(format!(
                    "{was}, so nothing was launched: {asked}, but no new window has appeared. Keys go to it now (cmd+n opens a window in most apps); then call list_windows and screenshot with the window's window_id.{second}"
                ))])
            }
        }
    }

    /// `open_app` for an app that isn't running: launch the chosen copy, behind.
    fn launch(&mut self, name: &str, copy: AppBundle, windows: &[WindowInfo]) -> ToolResult {
        let front = self.desktop.key_window().ok().flatten();
        let before: Vec<i64> = windows.iter().map(|w| w.id).collect();
        self.desktop
            .launch_behind(&copy)
            .map_err(|e| format!("Could not launch {}: {e}", copy.path))?;
        let id = copy
            .bundle_id
            .as_deref()
            .map(|b| format!(", {b}"))
            .unwrap_or_default();
        let launched = format!(
            "{name} wasn't running: launched {}{id} behind the user's windows",
            copy.path
        );
        let started = self
            .desktop
            .running_apps()
            .into_iter()
            .find(|a| a.path.as_deref() == Some(copy.path.as_str()));
        let pid = started.as_ref().map_or(0, |a| a.pid);
        // Launched behind, some apps open no window until asked (TextEdit skips its Open panel):
        // ask the new process once, as a click on its icon would.
        let mut found = self.new_window_of(pid, name, &before, WINDOW_WAIT / 2);
        if found.is_none()
            && let Some(app) = &started
            && self.desktop.reopen_behind(app).is_ok()
        {
            found = self.new_window_of(pid, name, &before, WINDOW_WAIT / 2);
        }
        match found {
            Some(w) => {
                let order = self.keep_behind(&w, front.as_ref()).unwrap_or_default();
                let reply = format!(
                    "{launched}. Working in its window{} (window_id {}): take a screenshot to see it.{order}",
                    titled(&w),
                    w.id
                );
                self.work_in(w);
                Ok(vec![text(reply)])
            }
            None if pid > 0 => {
                self.work_in_windowless(name, pid);
                Ok(vec![text(format!(
                    "{launched}. It shows no window yet: keys go to it now (cmd+n opens a window in most apps); then call list_windows and screenshot with the window's window_id."
                ))])
            }
            None => Err(format!(
                "{launched}, but no window of it has appeared yet. Call list_windows in a moment, then screenshot with its window_id."
            )),
        }
    }
}

/// How long the user counts as using the app in front after their last key, click or pointer
/// movement.
const IN_USE: Duration = Duration::from_secs(30);

/// How long an app that was opened is watched for bringing its window forward.
const SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 1500 });

/// Said when the agent would drive the app the user is using.
fn in_use(app: &str) -> String {
    format!(
        "{app} is the app the user is using right now; ask them whether to work in it (they switch to another app, then you go on), wait, or open a second window (open_app with new_window: true). Nothing was done."
    )
}

/// What Trek shows while the agent waits for the user's app.
fn waiting(app: &str) -> String {
    format!("Waiting: you're using {app}")
}

/// ` “Title”`, or nothing for an untitled window.
fn titled(w: &WindowInfo) -> String {
    if w.title.is_empty() {
        String::new()
    } else {
        format!(" “{}”", w.title)
    }
}

/// The app `open_app` works in.
#[derive(Debug, PartialEq)]
enum Pick {
    /// Running: its own windows are used, it isn't launched again.
    Running(RunningApp),
    /// Not running: this copy is launched.
    Launch(AppBundle),
    Missing,
}

/// Whether running app `a` is the one `name` (a name, bundle id or path) means, exactly.
fn names(a: &RunningApp, name: &str) -> bool {
    let name = name.trim().trim_end_matches('/');
    if name.contains('/') {
        return a.path.as_deref().map(|p| p.trim_end_matches('/')) == Some(name);
    }
    let stem = name.strip_suffix(".app").unwrap_or(name);
    a.name.eq_ignore_ascii_case(stem)
        || a.bundle_id
            .as_deref()
            .is_some_and(|b| b.eq_ignore_ascii_case(name))
        || a.path.as_deref().is_some_and(|p| {
            let file = p
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default();
            file.strip_suffix(".app")
                .unwrap_or(file)
                .eq_ignore_ascii_case(stem)
        })
}

/// Two running processes of one app (by bundle id, else by name).
fn same_app(a: &RunningApp, b: &RunningApp) -> bool {
    match (&a.bundle_id, &b.bundle_id) {
        (Some(x), Some(y)) => x == y,
        _ => a.name == b.name,
    }
}

/// How good a place a copy of an app is to launch it from: /Applications first, then the
/// system's, the user's own Applications folder, anywhere else; build output, caches and the
/// Trash last (a debug build there shares the name and bundle id of the real app).
fn rank(path: &str) -> u8 {
    const BUILDS: [&str; 9] = [
        "/DerivedData/",
        "/.build/",
        "/build/",
        "/target/",
        "/.Trash/",
        "/Caches/",
        "/tmp/",
        "/private/",
        "/Volumes/",
    ];
    if BUILDS.iter().any(|b| path.contains(b)) {
        4
    } else if path.starts_with("/Applications/") {
        0
    } else if path.starts_with("/System/") {
        1
    } else if path.starts_with("/Users/") && path.contains("/Applications/") {
        2
    } else {
        3
    }
}

/// Which app `open_app` means by `name`: one that's running first (two copies running: the one
/// with a window in front), and only when none is, the best-placed copy on disk (`installed`,
/// asked for only then).
fn pick(
    name: &str,
    running: &[RunningApp],
    windows: &[WindowInfo],
    installed: impl FnOnce() -> Vec<AppBundle>,
) -> Pick {
    let mut apps: Vec<&RunningApp> = running
        .iter()
        .filter(|a| !is_trek_app(&a.name) && names(a, name))
        .collect();
    if apps.is_empty() {
        apps = running
            .iter()
            .filter(|a| !is_trek_app(&a.name) && is_app(&a.name, name))
            .collect();
    }
    let front = windows
        .iter()
        .find_map(|w| apps.iter().find(|a| a.pid == w.pid));
    if let Some(app) = front.or(apps.first()) {
        return Pick::Running((*app).clone());
    }
    let mut copies = installed();
    copies.sort_by(|a, b| (rank(&a.path), &a.path).cmp(&(rank(&b.path), &b.path)));
    copies
        .into_iter()
        .next()
        .map_or(Pick::Missing, Pick::Launch)
}

impl rpc::ToolSet for Computer {
    fn family(&self) -> &'static str {
        "computer"
    }

    fn instructions(&self) -> &'static str {
        match self.mode {
            Mode::Front => INSTRUCTIONS,
            Mode::Background => BACKGROUND_INSTRUCTIONS,
        }
    }

    fn tools(&self) -> Vec<ToolDef> {
        let xy = |what: &str| {
            json!({
                "x": {"type": "number", "description": format!("{what} x, in screenshot pixels")},
                "y": {"type": "number", "description": format!("{what} y, in screenshot pixels")},
            })
        };
        let mut click_props = xy("Click").as_object().cloned().unwrap_or_default();
        click_props.insert(
            "button".into(),
            json!({"type": "string", "enum": ["left", "right"], "default": "left"}),
        );
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
        let behind = self.mode == Mode::Background;
        let mut shot_props = json!({
            "region": {
                "type": "object",
                "description": if behind { "Optional area to zoom into, in pixel coordinates of the window's latest screenshot" } else { "Optional area to zoom into, in full-screenshot pixel coordinates" },
                "properties": {
                    "x": {"type": "number"}, "y": {"type": "number"},
                    "width": {"type": "number"}, "height": {"type": "number"},
                },
                "required": ["x", "y", "width", "height"],
            },
        });
        if behind {
            shot_props["window_id"] = json!({"type": "integer", "description": "The window to work in, from list_windows; it stays the one you work in until you pick another"});
        }
        let pick =
            |front: &'static str, background: &'static str| if behind { background } else { front };
        vec![
            ToolDef {
                name: "screenshot",
                description: pick(
                    "Capture the main display as a PNG (longest side ≤ 1568 px) plus the screen size and \
coordinate scale. Coordinates in this image are what click/move_mouse/drag/scroll expect. Pass `region` (in \
screenshot pixels) to get a higher-detail zoom of part of the screen; zooms don't change the click coordinate space.",
                    "Capture the window you work in as a PNG (longest side ≤ 1568 px), as it looks under whatever covers \
it, plus its size and coordinate scale. Pass `window_id` (from list_windows) to work in that window from now on. \
Coordinates in this image are what click/move_mouse/drag/scroll expect. Pass `region` (in screenshot pixels) for a \
higher-detail zoom; zooms don't change the click coordinate space. With no window picked yet it shows the whole screen, \
for finding your way.",
                ),
                input_schema: json!({"type": "object", "properties": shot_props}),
            },
            ToolDef {
                name: "click",
                description: pick(
                    "Move the pointer to (x, y) in screenshot pixels and click. Supports right-click and double/triple-click.",
                    "Click at (x, y) in pixels of the window's screenshot, in the background: the user's pointer doesn't move. Supports right-click and double/triple-click.",
                ),
                input_schema: json!({"type": "object", "properties": click_props, "required": ["x", "y"]}),
            },
            ToolDef {
                name: "move_mouse",
                description: pick(
                    "Move the pointer to (x, y) in screenshot pixels without clicking (e.g. to reveal hover UI).",
                    "Point at (x, y) in pixels of the window's screenshot without clicking (e.g. to reveal hover UI), in the background.",
                ),
                input_schema: json!({"type": "object", "properties": xy("Target"), "required": ["x", "y"]}),
            },
            ToolDef {
                name: "drag",
                description: pick(
                    "Press the left button at `from`, move smoothly to `to`, and release. Coordinates in screenshot pixels.",
                    "Press the left button at `from`, move smoothly to `to`, and release, in the window, in the background. Coordinates in screenshot pixels.",
                ),
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
                description: pick(
                    "Move the pointer to (x, y) in screenshot pixels and scroll by dx/dy lines (positive dy = down, positive dx = right).",
                    "Scroll by dx/dy lines at (x, y) in pixels of the window's screenshot (positive dy = down, positive dx = right), in the background.",
                ),
                input_schema: json!({"type": "object", "properties": scroll_props, "required": ["x", "y"]}),
            },
            ToolDef {
                name: "type_text",
                description: pick(
                    "Type text into the focused element as keyboard input (Unicode-safe; newlines press Return, tabs press Tab). Click the field first to focus it.",
                    "Type text into the focused element of the window you work in (Unicode-safe; newlines press Return, tabs press Tab), in the background: the user's own typing isn't affected. Click the field first to focus it.",
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {"text": {"type": "string", "description": "Text to type"}},
                    "required": ["text"],
                }),
            },
            ToolDef {
                name: "key",
                description: pick(KEY_DESCRIPTION, BACKGROUND_KEY_DESCRIPTION),
                input_schema: json!({
                    "type": "object",
                    "properties": {"combo": {"type": "string", "description": if cfg!(windows) { "Key combo joined with '+', e.g. \"ctrl+shift+t\"" } else { "Key combo joined with '+', e.g. \"cmd+shift+t\"" }}},
                    "required": ["combo"],
                }),
            },
            ToolDef {
                name: "open_app",
                description: pick(
                    OPEN_APP_DESCRIPTION,
                    "Work in a macOS app, by name, e.g. \"Safari\", \"Notes\", \"TextEdit\", behind the user's windows. Running already: its frontmost window is used and nothing is launched. Not running: it's launched behind (never brought forward). Refused while it's the app the user is using. Take a screenshot next.",
                ),
                input_schema: if behind {
                    json!({
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "Application name, bundle id or path"},
                            "new_window": {"type": "boolean", "default": false, "description": "Ask the running app for one more window to work in (it opens a window, not another copy of the app), e.g. when the user is using its main one"},
                        },
                        "required": ["name"],
                    })
                } else {
                    json!({
                        "type": "object",
                        "properties": {"name": {"type": "string", "description": "Application name or path"}},
                        "required": ["name"],
                    })
                },
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
            return Err(format!(
                "{ACCESSIBILITY_ERROR}{}",
                self.desktop.permissions_note()
            ));
        }
        if self.mode == Mode::Background {
            match name {
                "screenshot" => return self.bg_screenshot(args),
                "click" => return self.bg_click(args),
                "move_mouse" => return self.bg_move(args),
                "drag" => return self.bg_drag(args),
                "scroll" => return self.bg_scroll(args),
                "type_text" => return self.bg_type(args),
                "key" => return self.bg_key(args),
                "open_app" => return self.bg_open_app(args),
                _ => {}
            }
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
        Computer {
            last: Some(Mapping::for_display(1512.0, 982.0)),
            desktop: Box::new(fake.clone()),
            mode: Mode::Front,
            target: None,
            report: None,
            posted: None,
        }
    }

    /// A computer on `fake` working in the background, with nothing picked yet.
    fn behind(fake: &Recording) -> Computer {
        Computer {
            mode: Mode::Background,
            ..on(fake)
        }
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
        let fake = Recording {
            trusted: false,
            ..Recording::new()
        };
        let mut c = on(&fake);
        for tool in ["click", "move_mouse", "drag", "scroll", "type_text", "key"] {
            assert!(needs_accessibility(tool));
            assert_eq!(
                c.call(tool, &acting_args(tool)),
                Err(ACCESSIBILITY_ERROR.to_string()),
                "{tool}"
            );
        }
        assert_eq!(fake.events(), vec![], "nothing was posted");
        // Looking, launching and waiting need no Accessibility.
        for tool in ["screenshot", "list_windows", "open_app", "wait"] {
            assert!(!needs_accessibility(tool), "{tool}");
        }
        assert_eq!(
            c.call("wait", &json!({"ms": 0})),
            Ok(vec![text("Waited 0 ms.")])
        );
    }

    /// A window as `Desktop::windows` lists one.
    fn window(owner: &str, x: f64, y: f64, w: f64, h: f64) -> WindowInfo {
        WindowInfo {
            owner: owner.into(),
            title: String::new(),
            pid: 1,
            id: 1,
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn the_mouse_and_keys_stay_out_of_treks_own_windows() {
        // Trek's window in front, covering the left of the screen; a browser behind it.
        let trek_in_front = vec![
            window("Trek", 0.0, 0.0, 800.0, 900.0),
            window("Safari", 0.0, 0.0, 1512.0, 982.0),
        ];
        let fake = Recording {
            windows: Ok(trek_in_front.clone()),
            ..Recording::new()
        };
        let mut c = on(&fake);
        // Every action that would land in it (or type into it) is refused before any event.
        assert_eq!(
            c.call("click", &json!({"x": 10, "y": 10})),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(
            c.call("scroll", &json!({"x": 400, "y": 300, "dy": 3})),
            Err(OWN_WINDOW.to_string())
        );
        // A drag that starts outside and ends inside is refused too.
        assert_eq!(
            c.call(
                "drag",
                &json!({"from": {"x": 1200, "y": 500}, "to": {"x": 100, "y": 100}})
            ),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(
            c.call("type_text", &json!({"text": "y"})),
            Err(OWN_KEYS.to_string())
        );
        assert_eq!(
            c.call("key", &json!({"combo": "return"})),
            Err(OWN_KEYS.to_string())
        );
        assert_eq!(fake.events(), vec![], "nothing reached the desktop");
        // Outside Trek's window the click goes through, to the point it was given.
        assert!(c.call("click", &json!({"x": 1200, "y": 500})).is_ok());
        assert_eq!(
            fake.events(),
            vec![Event::Click {
                x: 1200.0,
                y: 500.0,
                button: Button::Left,
                count: 1
            }]
        );
        // The window a point is in is the frontmost one holding it; a dev build counts as Trek.
        assert_eq!(
            window_at(&trek_in_front, 10.0, 10.0).map(|w| w.owner.as_str()),
            Some("Trek")
        );
        assert_eq!(
            window_at(&trek_in_front, 1200.0, 500.0).map(|w| w.owner.as_str()),
            Some("Safari")
        );
        assert_eq!(
            window_at(&trek_in_front, 5000.0, 5000.0).map(|w| w.owner.as_str()),
            None
        );
        assert!(
            is_trek(&window("trek", 0.0, 0.0, 1.0, 1.0))
                && !is_trek(&window("Trekking Maps", 0.0, 0.0, 1.0, 1.0))
        );
        // The windows can't be listed: nothing is done unchecked.
        let blind = Recording {
            windows: Err("no window list".into()),
            ..Recording::new()
        };
        let mut c = on(&blind);
        assert!(
            c.call("click", &json!({"x": 10, "y": 10}))
                .is_err_and(|e| e.contains("nothing was done"))
        );
        assert!(
            c.call("key", &json!({"combo": "return"}))
                .is_err_and(|e| e.contains("nothing was typed"))
        );
        assert_eq!(blind.events(), vec![]);
    }

    #[test]
    fn the_system_saying_a_point_is_treks_is_enough_to_refuse() {
        // The list's frontmost window there is an overlay covering the whole screen (NVIDIA's is,
        // on Windows, without being marked click-through), but the system's own hit test says the
        // click would reach Trek beneath it: refused.
        let fake = Recording {
            windows: Ok(vec![
                window("NVIDIA Overlay", 0.0, 0.0, 1512.0, 982.0),
                window("trek", 0.0, 0.0, 800.0, 900.0),
            ]),
            owner_at: Some("trek".into()),
            ..Recording::new()
        };
        let mut c = on(&fake);
        assert_eq!(
            c.call("click", &json!({"x": 10, "y": 10})),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(fake.events(), vec![]);
        // Keys go by the window in front, whatever is under the mouse.
        assert!(c.call("key", &json!({"combo": "escape"})).is_ok());
    }

    #[test]
    fn keys_go_by_the_window_that_takes_them_not_the_front_of_the_list() {
        // On Windows an always-on-top window (the taskbar, an overlay) leads the list whichever
        // app has the focus; the desktop says who does.
        let taskbar = window("Explorer", 0.0, 940.0, 1512.0, 42.0);
        let on_top = vec![
            taskbar.clone(),
            window("Notepad", 0.0, 0.0, 800.0, 900.0),
            window("trek", 0.0, 0.0, 1512.0, 982.0),
        ];
        // Trek has the focus: refused, though the list leads with the taskbar.
        let fake = Recording {
            windows: Ok(on_top),
            key_window: Some(Ok(Some(window("trek", 0.0, 0.0, 1512.0, 982.0)))),
            ..Recording::new()
        };
        let mut c = on(&fake);
        assert_eq!(
            c.call("type_text", &json!({"text": "y"})),
            Err(OWN_KEYS.to_string())
        );
        assert_eq!(
            c.call("key", &json!({"combo": "return"})),
            Err(OWN_KEYS.to_string())
        );
        assert_eq!(fake.events(), vec![]);
        // Trek is high in the list but another app has the focus: the keys go to that app.
        let fake = Recording {
            windows: Ok(vec![window("trek", 0.0, 0.0, 1512.0, 982.0), taskbar]),
            key_window: Some(Ok(Some(window("Notepad", 0.0, 0.0, 800.0, 900.0)))),
            ..Recording::new()
        };
        let mut c = on(&fake);
        assert!(c.call("key", &json!({"combo": "ctrl+s"})).is_ok());
        assert_eq!(
            fake.events(),
            vec![Event::Key(keys::parse("ctrl+s").unwrap())]
        );
        // The focus can't be read: nothing is typed unchecked.
        let blind = Recording {
            key_window: Some(Err("no foreground".into())),
            ..Recording::new()
        };
        let mut c = on(&blind);
        assert!(
            c.call("type_text", &json!({"text": "y"}))
                .is_err_and(|e| e.contains("nothing was typed"))
        );
        assert!(
            c.call("key", &json!({"combo": "return"}))
                .is_err_and(|e| e.contains("nothing was typed"))
        );
        assert_eq!(blind.events(), vec![]);
    }

    #[test]
    fn the_systems_hit_test_guards_every_pointer_action() {
        // Whatever the window list says (nothing here), the system's own answer that the point
        // is Trek's is enough to refuse a click, a scroll, or a drag starting or ending there.
        let fake = Recording {
            owner_at: Some("Trek".into()),
            ..Recording::new()
        };
        let mut c = on(&fake);
        assert_eq!(
            c.call("click", &json!({"x": 100, "y": 100, "button": "right"})),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(
            c.call("scroll", &json!({"x": 100, "y": 100, "dy": 3})),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(
            c.call(
                "drag",
                &json!({"from": {"x": 1, "y": 1}, "to": {"x": 100, "y": 100}})
            ),
            Err(OWN_WINDOW.to_string())
        );
        assert_eq!(fake.events(), vec![]);
        // Another app's answer lets them through.
        let fake = Recording {
            owner_at: Some("Notepad".into()),
            ..Recording::new()
        };
        assert!(
            on(&fake)
                .call("click", &json!({"x": 100, "y": 100}))
                .is_ok()
        );
    }

    #[test]
    fn bad_arguments_are_refused_before_any_event() {
        // Trusted, but every call here fails its checks before posting anything.
        let fake = Recording::new();
        let mut c = on(&fake);
        let refused =
            |c: &mut Computer, tool: &str, args: Value| c.call(tool, &args).expect_err(tool);
        assert!(refused(&mut c, "click", json!({})).contains("`x`"));
        assert!(
            refused(&mut c, "click", json!({"x": 10, "y": 10, "count": 4}))
                .contains("count must be")
        );
        assert!(
            refused(
                &mut c,
                "click",
                json!({"x": 10, "y": 10, "button": "middle"})
            )
            .contains("Unknown button")
        );
        assert!(
            refused(&mut c, "click", json!({"x": 5000, "y": 10}))
                .contains("outside the screenshot (1512x982)")
        );
        assert!(
            refused(&mut c, "move_mouse", json!({"x": 10, "y": -40}))
                .contains("outside the screenshot")
        );
        assert!(refused(&mut c, "drag", json!({"from": {"x": 10, "y": 10}})).contains("`to`"));
        assert!(
            refused(
                &mut c,
                "drag",
                json!({"from": {"x": 10, "y": 10}, "to": {"x": 10, "y": 9000}})
            )
            .contains("outside the screenshot")
        );
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
        let fake = Recording {
            trusted: false,
            ..Recording::new()
        };
        let mut c = on(&fake);
        let tools = c.tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "names are unique");
        // The same tools on every platform.
        assert_eq!(
            names,
            vec![
                "click",
                "drag",
                "key",
                "list_windows",
                "move_mouse",
                "open_app",
                "screenshot",
                "scroll",
                "type_text",
                "wait"
            ]
        );
        for t in &tools {
            let schema = t.to_json()["inputSchema"].clone();
            assert_eq!(schema["type"], "object", "{}", t.name);
            let props = schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{} has properties", t.name));
            for req in schema["required"].as_array().into_iter().flatten() {
                assert!(
                    props.contains_key(req.as_str().unwrap()),
                    "{} requires {req}, which it doesn't describe",
                    t.name
                );
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
            assert!(
                t.input_schema["properties"]["x"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("screenshot pixels")
            );
        }
        // Each platform's text names only its own keys and apps.
        let key = tools.iter().find(|t| t.name == "key").unwrap().description;
        let open = tools
            .iter()
            .find(|t| t.name == "open_app")
            .unwrap()
            .description;
        if cfg!(windows) {
            assert!(
                key.contains("ctrl+c") && key.contains("win") && !key.contains("fn"),
                "{key}"
            );
            assert!(
                !open.contains("macOS") && !INSTRUCTIONS.contains("macOS"),
                "{open}"
            );
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
        assert_eq!(
            (m.image_w, m.image_h),
            (
                Mapping::for_display(1728.0, 1117.0).image_w,
                Mapping::for_display(1728.0, 1117.0).image_h
            )
        );
        let (x, y) = m.to_points(784.0, 507.0).unwrap();
        assert!(
            (x - 864.0).abs() < 0.5 && (y - 558.5).abs() < 1.0,
            "the centre maps to the centre: {x}, {y}"
        );
        assert_eq!(m.to_points(0.0, 0.0), Ok((0.0, 0.0)));
        assert_eq!(
            m.to_points(1568.0, 1014.0),
            Ok((1727.0, 1116.0)),
            "far corner stays on the display"
        );
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
        assert!(
            (region.0 - 100.0 * m.scale).abs() < 1e-9 && (region.2 - 300.0 * m.scale).abs() < 1e-9,
            "{region:?}"
        );
        // Captured on a Retina display: twice the points, in pixels.
        let (iw, ih) = (
            (region.2 * 2.0).round() as u32,
            (region.3 * 2.0).round() as u32,
        );
        let info = region_info(region, iw, ih, m, "");
        assert!(info.contains("x=100 y=200 width=300 height=150"), "{info}");
        assert!(
            info.contains(&format!(
                "(100 + u*{}, 200 + v*{})",
                fmt_num(300.0 / iw as f64),
                fmt_num(150.0 / ih as f64)
            )),
            "{info}"
        );
        assert!(
            region_points(m, 10.0, 10.0, 0.0, 5.0)
                .unwrap_err()
                .contains("positive")
        );
        assert!(
            region_points(m, 1500.0, 10.0, 400.0, 5.0)
                .unwrap_err()
                .contains("outside")
        );
        let full = full_info(1568, 1014, 1728.0, 1117.0, Some((3456, 2234)), m, "");
        assert!(
            full.contains("1568x1014 px") && full.contains("3456x2234 physical pixels"),
            "{full}"
        );
        assert!(full.contains(&format!(
            "1 screenshot px = {} {SCREEN_UNITS}",
            fmt_num(m.scale)
        )));
    }

    #[test]
    fn a_1080p_pc_screen_maps_screenshot_pixels_to_screen_pixels() {
        // Windows reports the primary display in physical pixels (trek-mcp is per-monitor DPI
        // aware): a 1920x1080 screen at any scaling is shot at 1568x882.
        let fake = Recording {
            display: Display {
                width: 1920.0,
                height: 1080.0,
                physical: None,
            },
            ..Recording::new()
        };
        let mut c = Computer {
            last: None,
            ..on(&fake)
        };
        let shot = c.call("screenshot", &json!({})).unwrap();
        assert_eq!(
            fake.events(),
            vec![Event::Capture {
                region: None,
                max_side: 1568
            }]
        );
        assert_eq!(
            shot[0],
            json!({"type": "image", "data": "UE5H", "mimeType": "image/png"})
        );
        let info = shot[1]["text"].as_str().unwrap();
        assert!(
            info.starts_with("Screenshot of the main display: 1568x882 px."),
            "{info}"
        );
        // The screenshot's centre is the screen's; its far corner stays on it.
        c.call("click", &json!({"x": 784, "y": 441})).unwrap();
        c.call("move_mouse", &json!({"x": 1568, "y": 882})).unwrap();
        let events = fake.events();
        let Event::Click { x, y, .. } = events[1] else {
            panic!("{events:?}")
        };
        assert!(
            (x - 960.0).abs() <= 1.0 && (y - 540.0).abs() <= 1.0,
            "{x}, {y}"
        );
        let Event::Move(x, y) = events[2] else {
            panic!("{events:?}")
        };
        assert!(
            (x - 1919.0).abs() <= 1.0 && (y - 1079.0).abs() <= 1.0,
            "{x}, {y}"
        );
    }

    #[test]
    fn tools_answer_in_the_same_shapes_everywhere() {
        let fake = Recording {
            windows: Ok(vec![WindowInfo {
                owner: "Notepad".into(),
                title: "notes.txt".into(),
                pid: 42,
                id: 7,
                x: 100.0,
                y: 50.0,
                w: 800.0,
                h: 600.0,
            }]),
            ..Recording::new()
        };
        let mut c = on(&fake);
        let points = SCREEN_UNITS;
        assert_eq!(
            Value::Array(
                c.call(
                    "click",
                    &json!({"x": 10, "y": 20, "button": "right", "count": 2})
                )
                .unwrap()
            ),
            json!([{"type": "text", "text": format!("Double-clicked right at (10, 20) (screen {points} 10, 20).")}])
        );
        assert_eq!(
            Value::Array(
                c.call(
                    "drag",
                    &json!({"from": {"x": 1000, "y": 10}, "to": {"x": 1100, "y": 20}})
                )
                .unwrap()
            ),
            json!([{"type": "text", "text": "Dragged from (1000, 10) to (1100, 20)."}])
        );
        assert_eq!(
            Value::Array(
                c.call("scroll", &json!({"x": 1000, "y": 10, "dy": -3}))
                    .unwrap()
            ),
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
                Event::Click {
                    x: 10.0,
                    y: 20.0,
                    button: Button::Right,
                    count: 2
                },
                Event::Drag {
                    from: (1000.0, 10.0),
                    to: (1100.0, 20.0)
                },
                Event::Scroll {
                    x: 1000.0,
                    y: 10.0,
                    dx: 0,
                    dy: -3
                },
                Event::Type("héllo\n".into()),
                Event::Key(keys::parse("ctrl+shift+t").unwrap()),
                Event::OpenApp("Notepad".into()),
            ]
        );
        // The window list: a header, then one JSON object per window.
        let listed = c.call("list_windows", &json!({})).unwrap();
        let body = listed[0]["text"].as_str().unwrap();
        let mut lines = body.lines();
        assert!(
            lines
                .next()
                .unwrap()
                .starts_with("1 on-screen windows, front to back.")
        );
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
        let zoom = c
            .call(
                "screenshot",
                &json!({"region": {"x": 100, "y": 200, "width": 300, "height": 150}}),
            )
            .unwrap();
        assert_eq!(zoom[0]["type"], "image");
        assert!(
            zoom[1]["text"]
                .as_str()
                .unwrap()
                .starts_with("Zoomed view of screenshot region x=100 y=200 width=300 height=150")
        );
        assert_eq!(
            fake.events().last(),
            Some(&Event::Capture {
                region: Some((100.0, 200.0, 300.0, 150.0)),
                max_side: 1568
            })
        );
    }

    /// Safari's window, half covered by the user's own; Trek's beside it.
    fn behind_the_users() -> Recording {
        Recording {
            windows: Ok(vec![
                window("Notes", 0.0, 0.0, 700.0, 900.0),
                WindowInfo {
                    owner: "Safari".into(),
                    title: "Docs".into(),
                    pid: 42,
                    id: 7,
                    x: 400.0,
                    y: 100.0,
                    w: 800.0,
                    h: 600.0,
                },
                WindowInfo {
                    owner: "Trek".into(),
                    title: "Trek".into(),
                    pid: 9,
                    id: 3,
                    x: 0.0,
                    y: 0.0,
                    w: 1512.0,
                    h: 982.0,
                },
            ]),
            ..Recording::new()
        }
    }

    #[test]
    fn in_the_background_it_works_in_one_window_in_its_own_coordinates() {
        let fake = behind_the_users();
        let mut c = behind(&fake);
        // Nothing picked: actions are refused, a screenshot is of the whole screen and says how to pick.
        assert_eq!(
            c.call("click", &json!({"x": 10, "y": 10})),
            Err(NO_WINDOW.to_string())
        );
        assert_eq!(
            c.call("type_text", &json!({"text": "hi"})),
            Err(NO_WINDOW.to_string())
        );
        let whole = c.call("screenshot", &json!({})).unwrap();
        assert!(
            whole.last().unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("window_id")
        );
        assert_eq!(
            fake.events(),
            vec![Event::Capture {
                region: None,
                max_side: 1512
            }]
        );
        // Picked: the window alone, at its own size (twice that in pixels, fitted to it).
        let shot = c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let info = shot[1]["text"].as_str().unwrap();
        assert!(
            info.contains("Safari's window “Docs” (window_id 7)") && info.contains("800x600 px"),
            "{info}"
        );
        assert_eq!(
            fake.events()[1],
            Event::CaptureWindow {
                id: 7,
                region: None,
                max_side: 800
            }
        );
        // Clicks, keys and scrolls go to it, in its own points, however it's covered.
        c.call("click", &json!({"x": 100, "y": 50, "count": 2}))
            .unwrap();
        c.call("scroll", &json!({"x": 400, "y": 300, "dy": 3}))
            .unwrap();
        c.call(
            "drag",
            &json!({"from": {"x": 10, "y": 10}, "to": {"x": 20, "y": 30}}),
        )
        .unwrap();
        c.call("move_mouse", &json!({"x": 5, "y": 6})).unwrap();
        c.call("type_text", &json!({"text": "héllo"})).unwrap();
        c.call("key", &json!({"combo": "cmd+l"})).unwrap();
        let to_safari = |input| Event::Behind { window: 7, input };
        assert_eq!(
            fake.events()[2..],
            [
                to_safari(Input::Click {
                    x: 100.0,
                    y: 50.0,
                    button: Button::Left,
                    count: 2
                }),
                to_safari(Input::Scroll {
                    x: 400.0,
                    y: 300.0,
                    dx: 0,
                    dy: 3
                }),
                to_safari(Input::Drag {
                    from: (10.0, 10.0),
                    to: (20.0, 30.0)
                }),
                to_safari(Input::Move(5.0, 6.0)),
                to_safari(Input::Type("héllo".into())),
                to_safari(Input::Key(keys::parse("cmd+l").unwrap())),
            ]
        );
        // Points past the window's screenshot are refused; a zoom is cut from the window.
        assert!(
            c.call("click", &json!({"x": 900, "y": 10}))
                .unwrap_err()
                .contains("outside the screenshot (800x600)")
        );
        let zoom = c
            .call(
                "screenshot",
                &json!({"region": {"x": 0, "y": 0, "width": 200, "height": 100}}),
            )
            .unwrap();
        assert!(
            zoom[1]["text"]
                .as_str()
                .unwrap()
                .contains(&format!("window {SCREEN_UNITS}")),
            "{}",
            zoom[1]
        );
        assert_eq!(
            fake.events().last(),
            Some(&Event::CaptureWindow {
                id: 7,
                region: Some((0.0, 0.0, 200.0, 100.0)),
                max_side: 1568
            })
        );
        // The list says which window it works in.
        let listed = c.call("list_windows", &json!({})).unwrap();
        let body = listed[0]["text"].as_str().unwrap();
        assert!(
            body.lines().nth(2).unwrap().contains("\"working_in\":true")
                && !body.lines().nth(1).unwrap().contains("working_in"),
            "{body}"
        );
        assert_eq!(c.instructions(), BACKGROUND_INSTRUCTIONS);
        assert!(
            c.tools()[0].input_schema["properties"]
                .get("window_id")
                .is_some()
        );
        assert!(
            on(&fake).tools()[0].input_schema["properties"]
                .get("window_id")
                .is_none(),
            "in front, there's no window to pick"
        );
    }

    #[test]
    fn in_the_background_trek_stays_off_limits_and_a_closed_window_is_let_go() {
        let fake = behind_the_users();
        let mut c = behind(&fake);
        assert_eq!(
            c.call("screenshot", &json!({"window_id": 3})),
            Err(OWN_WINDOW_BEHIND.to_string())
        );
        assert_eq!(
            c.call("open_app", &json!({"name": "Trek"})),
            Err(OWN_WINDOW_BEHIND.to_string())
        );
        assert!(
            c.call("screenshot", &json!({"window_id": 99}))
                .unwrap_err()
                .contains("No window 99")
        );
        // A pick needs a screenshot before anything is pointed at.
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let mut gone = behind_the_users();
        gone.windows = Ok(vec![]);
        c.desktop = Box::new(gone.clone());
        let err = c.call("click", &json!({"x": 1, "y": 1})).unwrap_err();
        assert!(err.contains("Safari's window “Docs” has closed"), "{err}");
        assert_eq!(
            c.call("key", &json!({"combo": "return"})),
            Err(NO_WINDOW.to_string()),
            "and it's forgotten"
        );
        assert!(
            gone.events()
                .iter()
                .all(|e| !matches!(e, Event::Behind { .. }))
        );
        // Resized since its screenshot: the coordinates are stale.
        let mut c = behind(&fake);
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let mut bigger = behind_the_users();
        if let Ok(ws) = &mut bigger.windows {
            ws[1].w = 1000.0;
        }
        c.desktop = Box::new(bigger);
        assert_eq!(
            c.call("click", &json!({"x": 1, "y": 1})),
            Err(RESIZED.to_string())
        );
    }

    #[test]
    fn open_app_in_the_background_works_in_the_window_that_appears() {
        let fake = Recording {
            installed: vec![copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit",
            )],
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        let mut c = behind(&fake);
        let opened = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert!(
            opened.contains("window “Untitled” (window_id 11)"),
            "{opened}"
        );
        assert_eq!(
            fake.events(),
            vec![Event::Launch("/System/Applications/TextEdit.app".into())]
        );
        c.call("type_text", &json!({"text": "x"})).unwrap();
        assert_eq!(
            fake.events().last(),
            Some(&Event::Behind {
                window: 11,
                input: Input::Type("x".into())
            })
        );
        // Launched behind, it shows a window only when asked: it's asked, once.
        let fake = Recording {
            installed: vec![copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit",
            )],
            shy: true,
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        let mut c = behind(&fake);
        assert!(reply(c.call("open_app", &json!({"name": "TextEdit"}))).contains("(window_id 11)"));
        assert_eq!(
            fake.events(),
            vec![
                Event::Launch("/System/Applications/TextEdit.app".into()),
                Event::Reopen(50)
            ]
        );
        // An app with no window, that shows none when asked: keys go to it, pointing doesn't.
        let fake = behind_the_users();
        fake.running
            .borrow_mut()
            .push(running("Mail", "com.apple.mail", 77));
        let mut c = behind(&fake);
        assert!(
            reply(c.call("open_app", &json!({"name": "Mail"})))
                .contains("no new window has appeared")
        );
        assert_eq!(fake.events(), vec![Event::Reopen(77)]);
        c.call("key", &json!({"combo": "cmd+n"})).unwrap();
        assert_eq!(
            fake.events().last(),
            Some(&Event::Behind {
                window: 0,
                input: Input::Key(keys::parse("cmd+n").unwrap())
            })
        );
        assert!(
            c.call("click", &json!({"x": 1, "y": 1}))
                .unwrap_err()
                .contains("no window yet")
        );
        // Nothing to find at all.
        let mut c = behind(&behind_the_users());
        assert!(
            c.call("open_app", &json!({"name": "Nope"}))
                .unwrap_err()
                .contains("is running or installed")
        );
        assert!(
            is_app("Google Chrome", "chrome")
                && is_app("Safari", "safari.app")
                && !is_app("Safari", "")
                && !is_app("Notes", "no")
        );
    }

    fn reply(r: ToolResult) -> String {
        match r {
            Ok(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => panic!("refused: {e}"),
        }
    }

    fn copy(path: &str, id: &str) -> AppBundle {
        AppBundle {
            path: path.into(),
            bundle_id: Some(id.into()),
        }
    }

    fn running(name: &str, id: &str, pid: i64) -> RunningApp {
        RunningApp {
            name: name.into(),
            bundle_id: Some(id.into()),
            path: Some(format!("/Applications/{name}.app")),
            pid,
        }
    }

    fn textedit_window(pid: i64, id: i64) -> WindowInfo {
        WindowInfo {
            owner: "TextEdit".into(),
            title: "Untitled".into(),
            pid,
            id,
            x: 0.0,
            y: 0.0,
            w: 600.0,
            h: 400.0,
        }
    }

    #[test]
    fn which_app_open_app_works_in() {
        let none = || -> Vec<AppBundle> { panic!("a running app isn't looked for on disk") };
        let textedit = running("TextEdit", "com.apple.TextEdit", 50);
        // Running: that one, by name, bundle id, path or a loose name; nothing on disk is asked.
        for name in [
            "TextEdit",
            "textedit.app",
            "com.apple.TextEdit",
            "/Applications/TextEdit.app",
        ] {
            assert_eq!(
                pick(name, std::slice::from_ref(&textedit), &[], none),
                Pick::Running(textedit.clone()),
                "{name}"
            );
        }
        let chrome = running("Google Chrome", "com.google.Chrome", 60);
        assert_eq!(
            pick("chrome", std::slice::from_ref(&chrome), &[], none),
            Pick::Running(chrome)
        );
        // Two running copies (a debug build beside the real one): the one with a window in front.
        let debug = RunningApp {
            path: Some(
                "/Users/me/Library/Developer/Xcode/DerivedData/T/Build/Touchline 27.app".into(),
            ),
            pid: 71,
            ..running("Touchline 27", "local.touchline", 0)
        };
        let real = running("Touchline 27", "local.touchline", 70);
        let mut w = textedit_window(70, 5);
        w.owner = "Touchline 27".into();
        assert_eq!(
            pick("Touchline 27", &[debug.clone(), real.clone()], &[w], none),
            Pick::Running(real)
        );
        assert_eq!(
            pick("Touchline 27", std::slice::from_ref(&debug), &[], none),
            Pick::Running(debug),
            "a running copy is used, wherever it's from"
        );
        // Not running: the best-placed copy on disk, never a build's.
        let copies = || {
            vec![
                copy(
                    "/Users/me/Library/Developer/Xcode/DerivedData/T/Build/Products/Debug/TextEdit.app",
                    "com.apple.TextEdit",
                ),
                copy("/Users/me/Applications/TextEdit.app", "com.apple.TextEdit"),
                copy("/System/Applications/TextEdit.app", "com.apple.TextEdit"),
            ]
        };
        assert_eq!(
            pick("TextEdit", &[], &[], copies),
            Pick::Launch(copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit"
            ))
        );
        assert_eq!(pick("TextEdit", &[], &[], Vec::new), Pick::Missing);
        // Trek is never the app.
        let trek = running("Trek", "dev.trek.Trek", 9);
        assert_eq!(pick("Trek", &[trek], &[], Vec::new), Pick::Missing);
        assert!(
            rank("/Applications/Safari.app") < rank("/System/Applications/Notes.app")
                && rank("/System/Applications/Notes.app") < rank("/Users/me/Applications/X.app")
                && rank("/Users/me/Applications/X.app") < rank("/Users/me/Code/X.app")
                && rank("/Users/me/Code/X.app") < rank("/Users/me/Code/.build/X.app")
        );
    }

    #[test]
    fn a_running_app_is_never_launched_again() {
        // Running with a window: that window, and no launch, reopen or keys.
        let fake = behind_the_users();
        fake.running
            .borrow_mut()
            .push(running("Safari", "com.apple.Safari", 42));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "Safari"})));
        assert!(
            said.contains("Safari was already running (pid 42, /Applications/Safari.app), so nothing was launched")
                && said.contains("window “Docs” (window_id 7)"),
            "{said}"
        );
        assert_eq!(fake.events(), vec![]);
        c.call("screenshot", &json!({})).unwrap();
        c.call("click", &json!({"x": 10, "y": 10})).unwrap();
        // Running with no window: it's asked for one (a reopen, not a launch).
        let fake = behind_the_users();
        fake.running
            .borrow_mut()
            .push(running("TextEdit", "com.apple.TextEdit", 50));
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert!(
            said.contains("nothing was launched: asked it for a window")
                && said.contains("(window_id 11)"),
            "{said}"
        );
        assert_eq!(fake.events(), vec![Event::Reopen(50)]);
    }

    #[test]
    fn an_app_that_comes_forward_is_put_back_behind() {
        let fake = Recording {
            installed: vec![copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit",
            )],
            jumps_forward: true,
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        // Notes (window 1) was in front: it's brought back.
        assert_eq!(
            fake.events(),
            vec![
                Event::Launch("/System/Applications/TextEdit.app".into()),
                Event::BringForward(1)
            ]
        );
        assert!(
            said.contains("TextEdit brought its window forward; it was put back behind the user's Notes window."),
            "{said}"
        );
        // One that won't go back: said so, not passed off as behind.
        let fake = Recording {
            installed: vec![copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit",
            )],
            jumps_forward: true,
            stubborn: true,
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert!(
            said.contains("Note: TextEdit's window came to the front when it opened and stays there, in front of the user's Notes window rather than behind it."),
            "{said}"
        );
        assert_eq!(
            fake.events()
                .iter()
                .filter(|e| matches!(e, Event::BringForward(1)))
                .count(),
            3
        );
    }

    /// TextEdit in front, being typed in; Safari behind it.
    fn using_textedit(idle_secs: u64) -> Recording {
        let mut fake = behind_the_users();
        let mut windows = fake.windows.clone().unwrap();
        windows.insert(0, textedit_window(50, 11));
        fake.windows = Ok(windows);
        fake.idle = Some(Duration::from_secs(idle_secs));
        fake.running.borrow_mut().extend([
            running("TextEdit", "com.apple.TextEdit", 50),
            running("Safari", "com.apple.Safari", 42),
        ]);
        fake
    }

    #[test]
    fn the_app_the_user_is_using_is_left_to_them() {
        // In front and typed in 5 s ago: refused, and Trek is told it's waiting.
        let fake = using_textedit(5);
        let mut c = behind(&fake);
        assert_eq!(
            c.call("open_app", &json!({"name": "TextEdit"})),
            Err(format!(
                "{} TextEdit was already running (pid 50, /Applications/TextEdit.app).",
                in_use("TextEdit")
            ))
        );
        assert_eq!(fake.events(), vec![]);
        assert!(c.target.is_none());
        // Another app is fine meanwhile.
        assert!(reply(c.call("open_app", &json!({"name": "Safari"}))).contains("window_id 7"));
        // Picked by its window_id: looking is fine, acting waits.
        let said = reply(c.call("screenshot", &json!({"window_id": 11})));
        assert!(
            said.contains("TextEdit is the app the user is using right now"),
            "{said}"
        );
        assert_eq!(
            c.call("click", &json!({"x": 5, "y": 5})),
            Err(in_use("TextEdit"))
        );
        assert_eq!(
            c.call("type_text", &json!({"text": "hi"})),
            Err(in_use("TextEdit"))
        );
        assert!(
            !fake
                .events()
                .iter()
                .any(|e| matches!(e, Event::Behind { .. })),
            "nothing reached the user's app"
        );
        // Away for minutes, or the screen locked: it's fine.
        for fake in [
            using_textedit(300),
            Recording {
                locked: true,
                ..using_textedit(1)
            },
        ] {
            let mut c = behind(&fake);
            let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
            assert!(
                said.contains("Working in its window “Untitled” (window_id 11)"),
                "{said}"
            );
            c.call("type_text", &json!({"text": "hi"})).unwrap();
        }
        // Or a window of its own: one more window of the running app (cmd+n to it, no new
        // process), and the user's window brought back to the front.
        let fake = Recording {
            jumps_forward: true,
            ..using_textedit(5)
        };
        fake.opens.borrow_mut().push(textedit_window(50, 12));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit", "new_window": true})));
        assert!(
            said.contains("nothing was launched: pressed cmd+n in it for one more window")
                && said.contains("(window_id 12)")
                && said.contains("it was put back behind the user's TextEdit window"),
            "{said}"
        );
        assert_eq!(
            fake.events(),
            vec![
                Event::Behind {
                    window: 0,
                    input: Input::Key(keys::parse("cmd+n").unwrap())
                },
                Event::BringForward(11)
            ]
        );
    }

    #[test]
    fn the_agents_own_input_isnt_taken_for_the_users() {
        // The user is away, TextEdit in front; macOS counts the agent's keys as input.
        let fake = Recording {
            posts_count: true,
            ..using_textedit(300)
        };
        let mut c = behind(&fake);
        reply(c.call("open_app", &json!({"name": "TextEdit"})));
        for _ in 0..3 {
            c.call("type_text", &json!({"text": "hi"})).unwrap();
        }
        assert!(c.user_idle().unwrap() >= Duration::from_secs(300));
        // Input that came after the agent's is the user's.
        let mut c = behind(&using_textedit(300));
        c.posted = Some((
            std::time::Instant::now() - Duration::from_secs(10),
            Some(Duration::from_secs(300)),
        ));
        assert_eq!(
            c.user_idle(),
            Some(Duration::from_secs(300)),
            "nothing since: as the system says"
        );
        let mut fake = using_textedit(2);
        fake.idle = Some(Duration::from_secs(2));
        let mut c = behind(&fake);
        c.posted = Some((
            std::time::Instant::now() - Duration::from_secs(10),
            Some(Duration::from_secs(300)),
        ));
        assert_eq!(c.user_idle(), Some(Duration::from_secs(2)));
    }

    #[test]
    fn what_the_agent_is_told_when_the_user_is_using_the_app() {
        assert_eq!(
            in_use("Touchline 27"),
            "Touchline 27 is the app the user is using right now; ask them whether to work in it (they switch to another app, then you go on), wait, or open a second window (open_app with new_window: true). Nothing was done."
        );
        assert_eq!(
            waiting("Touchline 27"),
            "Waiting: you're using Touchline 27"
        );
        assert!(BACKGROUND_INSTRUCTIONS.contains("Work only in apps the user isn't using"));
    }

    #[test]
    fn what_the_picture_in_picture_window_is_told() {
        let w = WindowInfo {
            owner: "Safari".into(),
            title: "Docs".into(),
            pid: 42,
            id: 7,
            x: 400.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let a = activity(
            "drag",
            "Dragging".into(),
            &w,
            Some((20.0, 30.0)),
            Some((1.0, 2.0)),
        );
        assert_eq!(
            a["window"],
            json!({"id": 7, "pid": 42, "app": "Safari", "title": "Docs", "x": 400.0, "y": 100.0, "width": 800.0, "height": 600.0})
        );
        assert_eq!(
            (a["point"].clone(), a["from"].clone()),
            (json!({"x": 20.0, "y": 30.0}), json!({"x": 1.0, "y": 2.0}))
        );
        assert_eq!(
            activity("key", "Pressing return".into(), &w, None, None)["point"],
            Value::Null
        );
        assert_eq!(typing("hi\nthere"), "Typing “hi there”");
        assert_eq!(
            typing(&"a".repeat(40)),
            format!("Typing “{}…”", "a".repeat(32))
        );
    }

    #[test]
    fn each_step_in_the_background_is_told_to_trek() {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
        // A listener like Trek's: the hello, then the reports, answered with `{}`.
        #[cfg(unix)]
        let path =
            std::env::temp_dir().join(format!("trek-mcp-behind-{}.sock", std::process::id()));
        #[cfg(windows)]
        let path = trek_ipc::pipe_name(1).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut listener = {
            let _enter = rt.enter();
            trek_ipc::server::Listener::bind(&path).unwrap()
        };
        let trek = std::thread::spawn(move || {
            rt.block_on(async {
                let (rd, mut w) = tokio::io::split(listener.accept().await.unwrap());
                let mut r = tokio::io::BufReader::new(rd);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                assert!(line.contains("\"hello\""), "{line}");
                w.write_all(trek_ipc::encode(&json!({"ok": true})).as_bytes())
                    .await
                    .unwrap();
                let mut told = vec![];
                while told.len() < 2 {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    let req: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(req["method"], trek_ipc::COMPUTER_ACTIVITY);
                    w.write_all(
                        trek_ipc::encode(&trek_ipc::reply(&req["id"], Ok(json!({})))).as_bytes(),
                    )
                    .await
                    .unwrap();
                    told.push(req["params"].clone());
                }
                told
            })
        });
        let fake = behind_the_users();
        let mut c = behind(&fake);
        c.report = Some(Reporter::start(trek_ipc::Client {
            socket: path.clone(),
            token: "t".into(),
            session: "s".into(),
        }));
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        // Let the first go before the next, so neither is folded into the other.
        std::thread::sleep(Duration::from_millis(300));
        c.call("click", &json!({"x": 100, "y": 50})).unwrap();
        let told = trek.join().unwrap();
        assert_eq!(told[0]["action"], "screenshot");
        assert_eq!(
            (
                told[1]["action"].clone(),
                told[1]["detail"].clone(),
                told[1]["point"].clone()
            ),
            (
                json!("click"),
                json!("Clicking"),
                json!({"x": 100.0, "y": 50.0})
            )
        );
        assert_eq!(told[1]["window"]["app"], "Safari");
        #[cfg(unix)]
        let _ = std::fs::remove_file(path);
    }
}
