//! `trek-mcp computer` — computer use on macOS and Windows: screenshots, the mouse and keyboard,
//! the window list and app launch. The tools, their arguments and what they answer are the same
//! on both; the platform does the work behind `desktop::Desktop`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::desktop::{
    self, AppBundle, Button, Desktop, Input, Performed, Reading, RunningApp, Whereabouts,
    WindowInfo,
};
use crate::keys;
use crate::outline::{self, Action, Checked, Frame, Node, Refs, View};
use crate::rpc::{
    self, ToolDef, ToolResult, arg_f64, arg_point, arg_str, opt_f64, opt_str, point_schema, text,
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
    "macOS computer use for the main display. Read the window in front with `elements` first: an outline whose \
controls carry refs (e12) that `press`, `click` and `type_text` act on (a ref lasts while its element stays; read \
again after the window changes); `wait` waits for text or an element. Take a `screenshot` when looks matter: it \
returns an image plus the coordinate scale. All x/y arguments to click, move_mouse, drag and scroll are pixel \
coordinates in the most recent full-screen screenshot (origin top-left), as frames are; trek-mcp converts them to \
screen points. Prefer `key` shortcuts and `open_app` over hunting for UI when possible. Trek's own windows are off \
limits: clicks, drags and scrolls in them are refused, and so are keys while Trek is in front (bring the app you mean \
forward first)."
};

/// Background use (macOS): the agent works in one window of its own, behind the user's.
const BACKGROUND_INSTRUCTIONS: &str = "macOS computer use in the background: you work in one app window of your own while the \
user keeps working in theirs. Your clicks, keys and scrolls reach only that window (the user's pointer and keyboard stay \
theirs, so never ask them to keep off the mouse). Pick the window first: open_app, or elements or screenshot with a \
window_id from list_windows. Read it with elements first: an outline whose controls carry refs (e12) that press, click \
and type_text act on (a ref lasts while its element stays; read again after the window changes); wait waits for text \
or an element. Take a screenshot when looks matter: it shows just that window, even when others cover it; x/y for \
click, move_mouse, drag and scroll are pixels in its latest screenshot (origin top-left), as frames are. Trek's own \
windows are off limits. Work only in apps the user isn't using: the app in front is theirs while they're at the Mac, \
if only reading, and acting there is refused until they switch. While they're away (screen locked or asleep, or 5 \
minutes idle) any app will do. Some apps (games, canvases) ignore input not from the real mouse: if one doesn't \
respond, say so rather than retrying; the user can switch computer use to work in front in Trek's settings.";

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
    /// When a click or drag was last posted behind: the system may count it as a mouse press of
    /// the user's (`users_doing`).
    clicked: Option<std::time::Instant>,
    /// The window in front when a click or key was last posted behind, and the pid of the app it
    /// went to: a window of that app in front of it since, while the user did nothing, came up
    /// because of that input (`put_back_in_front`).
    front: Option<(WindowInfo, i64)>,
    /// The refs `elements` has given out, and the window they're of: another window's start
    /// afresh.
    refs: Refs,
    listed: Option<i64>,
}

/// The window background use works in. An app with no window yet has `id` 0: keys reach it,
/// nothing else does.
struct Target {
    window: WindowInfo,
    /// Window points per pixel of its latest screenshot; `None` until it has one.
    shot: Option<Mapping>,
    /// Where it was when last looked up.
    place: Place,
}

/// Where the window background use works in was at its last lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Place {
    /// On screen, on a Space that's showing.
    #[default]
    Here,
    /// On a Space that isn't showing: `full_screen` for an app's own full-screen Space.
    OtherSpace { full_screen: bool },
}

impl Place {
    /// How a sentence says where it is: "is on another Space".
    fn said(self) -> &'static str {
        match self {
            Place::Here => "is on screen",
            Place::OtherSpace { full_screen: false } => "is on another Space",
            Place::OtherSpace { full_screen: true } => "is full screen, on a Space of its own",
        }
    }
}

/// The watch kept on an app the agent opens, or has just clicked or typed in, so that it doesn't
/// take the front from the user (`Computer::look`): their window, the app, and what's been seen
/// of it so far.
struct Watch {
    /// The user's window: the one in front before the app could come forward.
    front: WindowInfo,
    /// The agent's app: its pid (0 until it's known, for one just launched) and its name.
    pid: i64,
    name: String,
    /// It was the active app before the watch began: staying so takes nothing from the user.
    was_active: bool,
    /// The app is being opened (else it was clicked or typed in): the reply says which.
    opening: bool,
    started: std::time::Instant,
    /// The windows as they lay when the app was last seen behind.
    behind: Vec<WindowInfo>,
    /// In front since (as far as it was seen), and whether with a window over the user's or
    /// only as the active app.
    since: Option<std::time::Instant>,
    by_window: bool,
    /// The put-backs asked for since it came forward: how many in a row with nothing to show
    /// for them, how many in all, what was in front at the last (a window over the user's,
    /// the app active), when that was, and the looks since.
    tries: u32,
    asks: u32,
    asked: (bool, bool),
    tried: Option<std::time::Instant>,
    looks: u32,
    /// How many times it came forward, and how long it was in front in all before it was
    /// seen behind again.
    came: u32,
    in_front: Duration,
    /// When it was last seen in front, and the looks that have found it behind since.
    last: Option<std::time::Instant>,
    quiet: u32,
    /// The app switcher's keys, as the looks find them on the keyboard: Command is held and
    /// Tab or ` went down during the hold (or the hold began before the watch could see), and
    /// when such a hold was last seen.
    switching: bool,
    switched: Option<std::time::Instant>,
    /// Why the watch ended before its time.
    end: Option<End>,
}

/// How the user brought an app forward themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    /// A click on one of its windows.
    Window,
    /// A click in the Dock.
    Dock,
    /// The app switcher's keys: Command with Tab, or with ` for the next window.
    Keys,
}

/// How a `Watch` ended, when not with the app behind and quiet.
enum End {
    /// The user brought the app forward themselves: it's theirs now.
    Users(By),
    /// Put back `TRIES` times and still in front: with a window `above` the user's or only as
    /// the active app, and with the keyboard the user's app's again or not (`None`: unknown).
    Stays {
        above: bool,
        users_keyboard: Option<bool>,
    },
    /// Putting the user's window back failed.
    Failed(String),
    /// Still coming forward when the watch ran out (`WATCH_MAX`): put back each time so far.
    Keeps,
}

impl Watch {
    /// Whether `w` is a window of the agent's app (and not the user's own window, when the app
    /// is one and the same).
    fn is_apps(&self, w: &WindowInfo) -> bool {
        w.id != self.front.id
            && !is_trek(w)
            && if self.pid > 0 {
                w.pid == self.pid
            } else {
                is_app(&w.owner, &self.name)
            }
    }

    /// What the watch saw, for the reply, of the app called `app`: nothing when it never came
    /// forward.
    fn said(&self, app: &str) -> String {
        let theirs = &self.front.owner;
        let opened = if self.opening { " when it opened" } else { "" };
        match &self.end {
            Some(End::Users(by)) => {
                let how = match by {
                    By::Window => format!("clicked {app}'s window forward themselves"),
                    By::Dock => format!("clicked in the Dock and brought {app} forward themselves"),
                    By::Keys => format!("switched to {app} themselves, with Command-Tab"),
                };
                format!(" The user {how}, so it was left in front: it's theirs to use now.")
            }
            Some(End::Failed(e)) => format!(
                " Note: {app}'s window came to the front{opened}, in front of the user's {theirs} window, and couldn't be put back ({e}). Tell the user it's in their way."
            ),
            Some(End::Stays {
                above: true,
                users_keyboard,
            }) => {
                let keyboard = match users_keyboard {
                    Some(true) => {
                        format!(
                            " ({app} isn't the active app, so the keyboard is still the user's)"
                        )
                    }
                    Some(false) => {
                        format!(" ({app} is the active app too, so what the user types goes to it)")
                    }
                    None => String::new(),
                };
                format!(
                    " Note: {app}'s window came to the front{opened} and stays there, in front of the user's {theirs} window rather than behind it{keyboard}. Tell the user it's in their way."
                )
            }
            Some(End::Stays { above: false, .. }) => format!(
                " Note: {app} made itself the active app{opened} and stays so: what the user types goes to it, not to their {theirs} window. Tell the user."
            ),
            None | Some(End::Keeps) if self.came == 0 => String::new(),
            end => {
                let (did, undone, was) = if self.by_window {
                    (
                        "brought its window forward",
                        format!("it was put back behind the user's {theirs} window"),
                        "in front",
                    )
                } else {
                    (
                        "made itself the active app",
                        format!("the user's {theirs} window was made active again"),
                        "the active app",
                    )
                };
                let time = about(self.in_front);
                let mut said = match self.came {
                    1 => format!(" {app} {did}; {undone}. It was {was} for {time}."),
                    n => format!(
                        " {app} {did} {n} times; each time {undone}. It was {was} for {time} in all."
                    ),
                };
                if matches!(end, Some(End::Keeps)) {
                    said.push_str(" It may do so again: tell the user if it stays in their way.");
                }
                said
            }
        }
    }
}

/// A short time as a reply says it: "about 40 ms", "under 10 ms".
fn about(time: Duration) -> String {
    match time.as_millis() {
        0..10 => "under 10 ms".into(),
        ms => format!("about {} ms", (ms + 5) / 10 * 10),
    }
}

/// Sleep, unless for no time at all (as tests have it).
fn sleep(time: Duration) {
    if !time.is_zero() {
        std::thread::sleep(time);
    }
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
            clicked: None,
            front: None,
            refs: Refs::new("elements"),
            listed: None,
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

/// Tools that post mouse or keyboard events or read a window's elements, and so need
/// Accessibility (`wait` only when it waits on the elements). Screenshots need Screen Recording
/// instead (checked when capturing); the rest need nothing.
fn needs_accessibility(tool: &str, args: &Value) -> bool {
    matches!(
        tool,
        "click" | "move_mouse" | "drag" | "scroll" | "type_text" | "key" | "elements" | "press"
    ) || (tool == "wait" && waits_on_elements(args))
}

/// Whether a `wait` is for text or an element rather than for a time.
fn waits_on_elements(args: &Value) -> bool {
    ["text", "element"]
        .iter()
        .any(|k| args.get(k).is_some_and(|v| !v.is_null()))
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
                "{} on-screen windows, front to back. In the background you work in one of them: pass its window_id to screenshot (working_in marks the one you're in). Trek's windows are off limits.{titles}{}",
                list.len(),
                self.unlisted(&windows)
            ),
        };
        let lines: Vec<String> = list.iter().map(Value::to_string).collect();
        Ok(vec![text(format!("{header}\n{}", lines.join("\n")))])
    }

    /// For the window list's header: the window the agent works in, when it's open on another
    /// Space and so not among `listed`.
    fn unlisted(&self, listed: &[WindowInfo]) -> String {
        let Some(t) = self.target.as_ref().filter(|t| t.window.id != 0) else {
            return String::new();
        };
        if listed.iter().any(|w| w.id == t.window.id) {
            return String::new();
        }
        match self.desktop.whereabouts(t.window.id) {
            Ok(Whereabouts::OtherSpace { full_screen, .. }) => format!(
                " The window you work in, {}'s window_id {}, {}, so it isn't listed; it's still yours.",
                t.window.owner,
                t.window.id,
                Place::OtherSpace { full_screen }.said()
            ),
            _ => String::new(),
        }
    }
}

/// Background use: one window, behind the user's.
impl Computer {
    fn tell(&self, activity: Value) {
        if let Some(r) = &self.report {
            let _ = r.tx.send(activity);
        }
    }

    /// The window being worked in, as it is now: on screen, or on another Space (still the
    /// target; `place` says which). Closed, minimised or hidden: it's forgotten, and the agent is
    /// told what became of it and to pick another.
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
        let (w, place) = match self.find(id)? {
            Whereabouts::OnScreen(w) => (w, Place::Here),
            Whereabouts::OtherSpace {
                window,
                full_screen,
            } => (window, Place::OtherSpace { full_screen }),
            other => {
                self.target = None;
                return Err(gone(&owner, &title, &other));
            }
        };
        if is_trek(&w) {
            return Err(OWN_WINDOW_BEHIND.into());
        }
        if let Some(t) = &mut self.target {
            t.window = w.clone();
            t.place = place;
        }
        Ok(w)
    }

    /// Window `id`, wherever it is. One the system lists but doesn't show gets `OPENING` to come
    /// on screen before it's taken for gone: a window just opened can be off screen for a few
    /// seconds.
    fn find(&self, id: i64) -> Result<Whereabouts, String> {
        let deadline = std::time::Instant::now() + OPENING;
        loop {
            let found = self.desktop.whereabouts(id)?;
            if !matches!(found, Whereabouts::NotShowing(_)) || std::time::Instant::now() >= deadline
            {
                return Ok(found);
            }
            std::thread::sleep(OPENING_POLL);
        }
    }

    /// Where the working window is, for the end of a reply: nothing when it's on screen.
    fn elsewhere(&self) -> String {
        match self.target.as_ref() {
            Some(t) if t.place != Place::Here => format!(
                " {}'s window {}, out of the user's sight.",
                t.window.owner,
                t.place.said()
            ),
            _ => String::new(),
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

    /// Work in window `id` from now on (a `window_id` given to `screenshot` or `elements`):
    /// refused for Trek's own, and for one that's closed or put away.
    fn pick_window(&mut self, id: i64) -> Result<(), String> {
        let found = self.find(id)?;
        let w = match &found {
            Whereabouts::OnScreen(w) | Whereabouts::OtherSpace { window: w, .. } => w.clone(),
            Whereabouts::Closed => {
                return Err(format!(
                    "No window {id} is open. list_windows shows the windows there are."
                ));
            }
            Whereabouts::Minimised(w) | Whereabouts::Hidden(w) | Whereabouts::NotShowing(w) => {
                if is_trek(w) {
                    return Err(OWN_WINDOW_BEHIND.into());
                }
                return Err(gone(&w.owner, &w.title, &found));
            }
        };
        if is_trek(&w) {
            return Err(OWN_WINDOW_BEHIND.into());
        }
        if self.target.as_ref().is_none_or(|t| t.window.id != id) {
            self.target = Some(Target {
                window: w,
                shot: None,
                place: Place::Here,
            });
        }
        Ok(())
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
            self.pick_window(id as i64)?;
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
        let place = self.target.as_ref().map_or(Place::Here, |t| t.place);
        match region {
            None => {
                let shot = self
                    .desktop
                    .capture_window(&w, None, full_side(w.w, w.h))
                    .map_err(|e| unseen(&w, place, e))?;
                let m = Mapping::from_image(w.w, w.h, shot.width, shot.height);
                if let Some(t) = &mut self.target {
                    t.shot = Some(m);
                }
                let theirs = self.users_window().is_some_and(|f| f.pid == w.pid);
                let (doing, mut note) = if theirs {
                    (
                        waiting(&w.owner),
                        format!(
                            " {} is the app the user is using right now (it's in front and they're at the Mac): clicks and keys there are refused until they switch to another app or step away (ask them, or wait).",
                            w.owner
                        ),
                    )
                } else {
                    ("Looking".to_string(), String::new())
                };
                if place != Place::Here {
                    note.push_str(&format!(
                        " The window {}, out of the user's sight: this is how it was last drawn there. Clicks, keys and scrolls go to it as usual, but some apps (browsers especially) stop drawing a window that isn't showing, so a screenshot can lag behind what you did; if nothing seems to change, say so rather than repeat it.",
                        place.said()
                    ));
                }
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
                    .capture_window(&w, Some(region), MAX_IMAGE_SIDE)
                    .map_err(|e| unseen(&w, place, e))?;
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
        let raised = self.post_watched(
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
            "{what} at ({}, {}) in {}'s window (window {SCREEN_UNITS} {}, {}), in the background.{raised}{}",
            fmt_num(x),
            fmt_num(y),
            w.owner,
            fmt_num(px),
            fmt_num(py),
            self.elsewhere()
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
            "Moved the pointer to ({}, {}) in {}'s window, in the background (the user's pointer stays put).{}",
            fmt_num(x),
            fmt_num(y),
            w.owner,
            self.elsewhere()
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
            "Dragged from ({}, {}) to ({}, {}) in {}'s window, in the background.{}",
            fmt_num(fx),
            fmt_num(fy),
            fmt_num(tx),
            fmt_num(ty),
            w.owner,
            self.elsewhere()
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
            "Scrolled dx={dx} dy={dy} lines at ({}, {}) in {}'s window, in the background.{}",
            fmt_num(x),
            fmt_num(y),
            w.owner,
            self.elsewhere()
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
        let raised = self.post_watched(&w, &Input::Type(s.to_string()))?;
        self.tell(activity("type", typing(s), &w, None, None));
        Ok(vec![text(format!(
            "Typed {} characters into {}, in the background.{raised}{}",
            s.chars().count(),
            w.owner,
            self.elsewhere()
        ))])
    }

    fn bg_key(&mut self, args: &Value) -> ToolResult {
        let combo = arg_str(args, "combo")?;
        let parsed = keys::parse(combo)?;
        let w = self.keyed()?;
        let raised = self.post_watched(&w, &Input::Key(parsed))?;
        self.tell(activity("key", format!("Pressing {combo}"), &w, None, None));
        Ok(vec![text(format!(
            "Pressed {combo} in {}, in the background.{raised}{}",
            w.owner,
            self.elsewhere()
        ))])
    }

    /// The window the user is at right now: the front window (not Trek's), for as long as it's in
    /// front and they're at the Mac, whether they're typing in it or only reading it. `None`
    /// while they're away (`user_away`).
    fn users_window(&self) -> Option<WindowInfo> {
        if self.user_away() {
            return None;
        }
        self.desktop
            .key_window()
            .ok()
            .flatten()
            .filter(|w| !is_trek(w))
    }

    /// The user isn't at the Mac: the screen is locked, asleep or showing its screen saver, or
    /// they've given no input for `AWAY_AFTER`. A platform that can't say how long they've been
    /// idle counts them as there, so their front app stays theirs.
    fn user_away(&self) -> bool {
        self.desktop.screen_locked()
            || self.desktop.screen_dark()
            || self.user_idle().is_some_and(|idle| idle >= AWAY_AFTER)
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
        self.note_posted(input, before);
        Ok(())
    }

    /// Input was posted behind just now: when, for `user_idle` and `users_doing`.
    fn note_posted(&mut self, input: &Input, idle_before: Option<Duration>) {
        let now = std::time::Instant::now();
        self.posted = Some((now, idle_before));
        if matches!(input, Input::Click { .. } | Input::Drag { .. }) {
            self.clicked = Some(now);
        }
    }

    /// `post` a click, key or typing, which can make the app raise itself over the user's window:
    /// what came up since the last one, and what this one brings up, is put back behind the
    /// window the user has in front. What was put back, for the reply.
    fn post_watched(&mut self, w: &WindowInfo, input: &Input) -> Result<String, String> {
        let clicks = matches!(input, Input::Click { .. } | Input::Drag { .. });
        self.watched(w, clicks, |desktop| desktop.behind(w, input))
            .map(|((), said)| said)
    }

    /// Do `act` to `w`'s app behind (input posted to it, or an element's own action), watching
    /// for the app raising itself over the user's window. What had come up since the last
    /// input is put back first, if the user did nothing meanwhile (`put_back_in_front`). Then
    /// the app is watched from before `act` until `AFTER_INPUT` after it (an app that
    /// activates itself does within that; a slower one is caught at the next input): a window
    /// it brings forward goes back behind the user's within a moment, whether they're typing
    /// in their own app or not. What `act` answered, and what was put back. `clicks`: `act`
    /// posts a click or drag.
    fn watched<T>(
        &mut self,
        w: &WindowInfo,
        clicks: bool,
        act: impl FnOnce(&dyn Desktop) -> Result<T, String>,
    ) -> Result<(T, String), String> {
        let mut said = self.put_back_in_front();
        let front = self.desktop.key_window().ok().flatten();
        let before = self.user_idle();
        // Nothing to watch for when the window in front is the app's own.
        let theirs = front.clone().filter(|f| f.pid != w.pid);
        let mut watch = self.watch(theirs, w.pid, &w.owner, false);
        let done = act(self.desktop.as_ref())?;
        let now = std::time::Instant::now();
        self.posted = Some((now, before));
        if clicks {
            self.clicked = Some(now);
        }
        self.front = front.map(|f| (f, w.pid));
        self.settle(watch.as_mut(), AFTER_INPUT);
        said.push_str(&watch.map(|x| x.said(&w.owner)).unwrap_or_default());
        Ok((done, said))
    }

    /// When a window of the app the agent works in has come in front of another app's window
    /// that was in front at its last click or key (`self.front`), and the user has given no input
    /// since, the agent's input raised it, too late for `watched` to see: the user's window
    /// goes back in front. Nothing else is touched: apps reorder their own windows, other
    /// apps' windows come and go, and once the user has acted, what's in front is theirs.
    fn put_back_in_front(&mut self) -> String {
        let (Some((front, agents)), Some((at, _))) = (self.front.clone(), self.posted) else {
            return String::new();
        };
        if front.pid == agents {
            return String::new();
        }
        if self.user_idle().is_some_and(|idle| idle < at.elapsed()) {
            self.front = None;
            return String::new();
        }
        let order = self.desktop.windows().unwrap_or_default();
        let Some(first) = order
            .first()
            .filter(|f| f.pid == agents && f.id != front.id && !is_trek(f))
            .cloned()
        else {
            return String::new();
        };
        if !order.iter().any(|x| x.id == front.id) {
            return String::new();
        }
        let mut watch = self.watch(Some(front), agents, &first.owner, false);
        self.settle(watch.as_mut(), SETTLE);
        watch.map(|w| w.said(&first.owner)).unwrap_or_default()
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

    /// Starts a watch on the app the agent is opening (`opening`) or has just acted in, for the
    /// sake of `front`, the window the user has in front; with none there's nothing to keep in
    /// front, and no watch. What putting their window back takes is got ready now, before the
    /// app can be in its way. `pid` 0: an app just launched, known by `name` until its pid is.
    fn watch(
        &self,
        front: Option<WindowInfo>,
        pid: i64,
        name: &str,
        opening: bool,
    ) -> Option<Watch> {
        let front = front?;
        self.desktop.ready_front(&front);
        let now = std::time::Instant::now();
        // Command held already: the app switcher may be up, its Tab pressed before this began.
        let held = self
            .desktop
            .switch_keys()
            .is_some_and(|(command, _)| command);
        Some(Watch {
            switching: held,
            switched: held.then_some(now),
            // An app that's active already takes nothing by staying so.
            was_active: pid > 0 && self.desktop.is_active(pid) == Some(true),
            front,
            pid,
            name: name.to_string(),
            opening,
            started: now,
            behind: self.desktop.windows().unwrap_or_default(),
            since: None,
            by_window: false,
            tries: 0,
            asks: 0,
            asked: (false, false),
            tried: None,
            looks: 0,
            came: 0,
            in_front: Duration::ZERO,
            last: None,
            quiet: 0,
            end: None,
        })
    }

    /// One look at the windows while `watch` is kept. When a window of the agent's app has come
    /// in front of the user's window, or the app has made itself the active one in their app's
    /// place, the user's window is put back (`Desktop::bring_forward`, or `raise` when the
    /// keyboard wasn't taken): at once, then again when a moment later (`SHOWS`) that doesn't
    /// show, `TRIES` times in all before it's taken to stay. The windows as they are, front to
    /// back.
    fn look(&self, watch: Option<&mut Watch>) -> Vec<WindowInfo> {
        let order = self.desktop.windows().unwrap_or_default();
        let Some(watch) = watch.filter(|w| w.end.is_none()) else {
            return order;
        };
        let now = std::time::Instant::now();
        // The app switcher's keys, read at every look: it brings an app forward only when
        // Command is let go, which may be between two looks.
        match self.desktop.switch_keys() {
            Some((true, tab)) => {
                watch.switching |= tab;
                if watch.switching {
                    watch.switched = Some(now);
                }
            }
            Some((false, _)) => watch.switching = false,
            None => {}
        }
        let theirs = order.iter().position(|x| x.id == watch.front.id);
        let above = theirs.is_some_and(|at| order[..at].iter().any(|x| watch.is_apps(x)));
        // Whether the agent's app has the keyboard, asked at every look (`None`: it can't be
        // told). Not of an app whose own window the user's is: there the windows' order is all
        // there is to it.
        let keyboard = (watch.pid > 0 && watch.pid != watch.front.pid)
            .then(|| self.desktop.is_active(watch.pid))
            .flatten();
        let active = !watch.was_active && keyboard == Some(true);
        if !above && !active {
            if let Some(since) = watch.since.take() {
                let was = now.saturating_duration_since(since);
                watch.in_front += was;
                eprintln!(
                    "trek-mcp computer: {} came in front of the user's {} window; put back after {} ms",
                    watch.name,
                    watch.front.owner,
                    was.as_millis()
                );
            }
            watch.tries = 0;
            watch.asks = 0;
            watch.asked = (false, false);
            watch.tried = None;
            watch.quiet += 1;
            watch.behind = order.clone();
            return order;
        }
        // The user's own switch is never undone: asked at every look that finds the app in
        // front, since they may switch to it while a put-back of its own grab is under way.
        if let Some(by) = self.users_doing(watch) {
            watch.end = Some(End::Users(by));
            return order;
        }
        watch.quiet = 0;
        watch.last = Some(now);
        if watch.since.is_none() {
            watch.since = Some(now);
            watch.came += 1;
        }
        watch.by_window |= above;
        // More has come forward than when their window was last asked back: the app's window
        // gets on top a moment after the app turned active (by when their app has the keyboard
        // again), or the app turns active once more while its window is still on top. That's
        // answered at once, not waited out as a put-back that hasn't shown yet.
        let more = (above && !watch.asked.0) || (active && !watch.asked.1);
        // Where the keyboard is, when the window won't go back: the agent's app has it, or
        // not (the window server gave it back to the user's).
        let stays = End::Stays {
            above,
            users_keyboard: keyboard.map(|agents| !agents),
        };
        if let Some(tried) = watch.tried.filter(|_| !more) {
            watch.looks += 1;
            if tried.elapsed() < SHOWS || watch.looks < SHOWS_LOOKS {
                return order;
            }
            if watch.tries >= TRIES {
                watch.end = Some(stays);
                return order;
            }
        }
        if watch.asks >= ASKS {
            watch.end = Some(stays);
            return order;
        }
        // Who has the keyboard decides how it's put back. The agent's app took it (or that
        // can't be told): the user's app is made the active one again, and their window key and
        // on top. It didn't (their app is still active, or a third is: an alert, a launcher's
        // panel): their window is raised and nothing else, so whoever is active stays so.
        let took = keyboard != Some(false);
        let put_back = if took {
            self.desktop.bring_forward(&watch.front)
        } else {
            self.desktop.raise(&watch.front)
        };
        match put_back {
            Ok(()) => {
                // An asking that answers something new is the first for it; one made because
                // the last didn't show counts on towards `TRIES`.
                watch.tries = if more { 1 } else { watch.tries + 1 };
                watch.asks += 1;
                watch.asked = (above, active);
                watch.tried = Some(std::time::Instant::now());
                watch.looks = 0;
            }
            Err(e) => watch.end = Some(End::Failed(e)),
        }
        order
    }

    /// Whether the user brought the watched app forward themselves, and how. Then it's theirs,
    /// and never put back:
    /// - with the app switcher: Command was held a moment ago with Tab or ` pressed during the
    ///   hold (the app comes forward as Command is let go);
    /// - with a click, a moment ago and since the watch began, on one of the app's windows as
    ///   they lay behind theirs, or in the Dock.
    ///
    /// What they do in their own app says nothing of the kind, a Command shortcut included: an
    /// app that takes the front while they click or type there is put back all the same. A
    /// click that falls when the agent's own last click did may be that click, as the system
    /// counts it (`user_idle`), and isn't taken for theirs.
    fn users_doing(&self, watch: &Watch) -> Option<By> {
        if watch.switched.is_some_and(|at| at.elapsed() < SWITCHED) {
            return Some(By::Keys);
        }
        let (ago, (x, y)) = self.desktop.last_click()?;
        let ours = self.clicked.is_some_and(|at| {
            let since = at.elapsed();
            ago + Duration::from_millis(50) >= since && ago <= since + Duration::from_millis(500)
        });
        if ours || ago >= CLICKED || ago >= watch.started.elapsed() {
            return None;
        }
        if window_at(&watch.behind, x, y).is_some_and(|w| watch.is_apps(w)) {
            return Some(By::Window);
        }
        self.desktop.on_dock(x, y).then_some(By::Dock)
    }

    /// Keeps `watch` for at least `least`, and until the app has stayed behind for `QUIET`
    /// since it last came forward (an app can come forward more than once as it starts: Chrome
    /// does), `WATCH_MAX` at most.
    fn settle(&self, watch: Option<&mut Watch>, least: Duration) {
        let Some(watch) = watch else {
            return;
        };
        let started = std::time::Instant::now();
        loop {
            self.look(Some(watch));
            if watch.end.is_some() {
                return;
            }
            let now = std::time::Instant::now();
            let until = watch
                .last
                .map_or(started + least, |last| (last + QUIET).max(started + least));
            if watch.since.is_none() {
                if now >= until && watch.quiet >= QUIET_LOOKS {
                    return;
                }
                if now >= started + WATCH_MAX {
                    watch.end = Some(End::Keeps);
                    return;
                }
            }
            sleep(LOOK);
        }
    }

    /// Waits up to `wait` for a window of process `pid` that isn't one of `before` (`pid` 0: any
    /// window of an app called `name`), keeping `watch` all the while: the window can come up
    /// in front of the user's the moment it appears.
    fn new_window_of(
        &self,
        watch: &mut Option<Watch>,
        pid: i64,
        name: &str,
        before: &[i64],
        wait: Duration,
    ) -> Option<WindowInfo> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let found = self
                .look(watch.as_mut())
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
            sleep(LOOK);
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
            place: Place::Here,
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
            place: Place::Here,
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
        // One more window of it is no way round this: within one app, the window the agent made
        // key would take the user's typing.
        let users = self.users_window();
        if let Some(front) = users.as_ref().filter(|f| f.pid == app.pid) {
            self.tell(activity("waiting", waiting(&app.name), front, None, None));
            return Err(format!("{} {was}.", in_use(&app.name)));
        }
        if !new_window && let Some(w) = theirs.first().cloned() {
            let reply = format!(
                "{was}, so nothing was launched. Working in its window{} (window_id {}), behind the user's: read it with elements, or take a screenshot to see it.",
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
        // Watched from before it's asked: the new window can come up in front at once.
        let mut watch = self.watch(front, app.pid, &app.name, true);
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
        let found = self.new_window_of(&mut watch, app.pid, &app.name, &before, WINDOW_WAIT);
        if found.is_some() {
            self.settle(watch.as_mut(), SETTLE);
        }
        let order = watch.map(|w| w.said(&app.name)).unwrap_or_default();
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
        match found {
            Some(w) => {
                let reply = format!(
                    "{was}, so nothing was launched: {asked}. Working in its new window{} (window_id {}): read it with elements, or take a screenshot to see it.{order}{second}",
                    titled(&w),
                    w.id
                );
                self.work_in(w);
                Ok(vec![text(reply)])
            }
            None => {
                self.work_in_windowless(&app.name, app.pid);
                Ok(vec![text(format!(
                    "{was}, so nothing was launched: {asked}, but no new window has appeared. {NO_WINDOW_YET}{order}{second}"
                ))])
            }
        }
    }

    /// `open_app` for an app that isn't running: launch the chosen copy, behind.
    fn launch(&mut self, name: &str, copy: AppBundle, windows: &[WindowInfo]) -> ToolResult {
        let front = self.desktop.key_window().ok().flatten();
        let before: Vec<i64> = windows.iter().map(|w| w.id).collect();
        // Watched from before it starts: an app can take the front a tenth of a second into
        // its launch, and again as it goes on.
        let mut watch = self.watch(front, 0, name, true);
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
        if let (Some(watch), Some(app)) = (watch.as_mut(), &started) {
            watch.pid = app.pid;
            watch.name = app.name.clone();
        }
        // Launched behind, some apps open no window until asked (TextEdit skips its Open panel):
        // ask the new process once, as a click on its icon would.
        let mut found = self.new_window_of(&mut watch, pid, name, &before, WINDOW_WAIT / 2);
        if found.is_none()
            && let Some(app) = &started
            && self.desktop.reopen_behind(app).is_ok()
        {
            found = self.new_window_of(&mut watch, pid, name, &before, WINDOW_WAIT / 2);
        }
        if let Some(w) = &found {
            if let Some(watch) = watch.as_mut() {
                watch.pid = w.pid;
                watch.name = w.owner.clone();
            }
            self.settle(watch.as_mut(), LAUNCH_SETTLE);
        }
        let called = found.as_ref().map_or(name, |w| w.owner.as_str());
        let order = watch.map(|w| w.said(called)).unwrap_or_default();
        match found {
            Some(w) => {
                let reply = format!(
                    "{launched}. Working in its window{} (window_id {}): read it with elements, or take a screenshot to see it.{order}",
                    titled(&w),
                    w.id
                );
                self.work_in(w);
                Ok(vec![text(reply)])
            }
            None if pid > 0 => {
                self.work_in_windowless(name, pid);
                Ok(vec![text(format!(
                    "{launched}. It shows no window yet. {NO_WINDOW_YET}{order}"
                ))])
            }
            None => Err(format!(
                "{launched}, but no window of it has appeared yet. Call list_windows in a moment, then screenshot with its window_id.{order}"
            )),
        }
    }
}

/// The window whose elements are read and acted on, and how its frames map to the pixels the
/// x/y tools take.
struct Scope {
    w: WindowInfo,
    /// Where the window's corner is in the screenshot those pixels are of, in screen units:
    /// nowhere in the background (the screenshot is of the window), its place on screen in front.
    origin: (f64, f64),
    /// Screen units per screenshot pixel.
    scale: f64,
    /// The screenshot's size in pixels.
    image: (u32, u32),
}

/// A click's button and count from its arguments, and the word for the button.
fn click_kind(args: &Value) -> Result<(Button, i64), String> {
    let count = opt_f64(args, "count")?.unwrap_or(1.0) as i64;
    if !(1..=3).contains(&count) {
        return Err("count must be 1 (single), 2 (double) or 3 (triple)".into());
    }
    match args.get("button").and_then(Value::as_str).unwrap_or("left") {
        "left" => Ok((Button::Left, count)),
        "right" => Ok((Button::Right, count)),
        other => Err(format!(
            "Unknown button {other:?}; use \"left\" or \"right\""
        )),
    }
}

/// How a reply says a click was done.
fn clicked(count: i64, button: Button) -> &'static str {
    match (count, button) {
        (_, Button::Right) => "Right-clicked",
        (2, _) => "Double-clicked",
        (3, _) => "Triple-clicked",
        _ => "Clicked",
    }
}

/// Text as a reply quotes it: on one line, cut when long.
fn quoted(s: &str) -> String {
    Value::from(outline::clip(s, 200)).to_string()
}

/// What an element reads as after its own action, where that differs from `before`: for the
/// reply. Empty when nothing it says about itself changed.
fn after_action(before: &Node, after: Option<&Node>) -> String {
    let Some(after) = after.filter(|a| a.role == before.role) else {
        return " It's no longer where it was (the window changed): read the elements again before using a ref.".into();
    };
    if after.name != before.name {
        return format!(" It's now {}.", after.describe());
    }
    if after.checked != before.checked {
        return match after.checked {
            Some(Checked::Yes) => " It's now checked.".into(),
            Some(Checked::No) => " It's now unchecked.".into(),
            Some(Checked::Mixed) => " It's now mixed.".into(),
            // A menu item's check mark, taken off.
            None => " It's no longer checked.".into(),
        };
    }
    if after.expanded != before.expanded {
        return match after.expanded {
            Some(true) => " It's now expanded.".into(),
            Some(false) => " It's now collapsed.".into(),
            None => String::new(),
        };
    }
    if after.selected != before.selected {
        return if after.selected {
            " It's now selected.".into()
        } else {
            " It's no longer selected.".into()
        };
    }
    if after.value != before.value && !after.secure {
        return format!(
            " It now reads {}.",
            quoted(after.value.as_deref().unwrap_or_default())
        );
    }
    String::new()
}

/// How often `wait` looks again for the text or element it waits for, and how long a field
/// clicked for the focus gets to say it has it.
const WAIT_POLL: Duration = Duration::from_millis(if cfg!(test) { 1 } else { 400 });
const FOCUS_SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 200 });

/// Elements: the window read as an outline, and its controls acted on by ref.
impl Computer {
    /// The window elements are read from and acted in: in the background the one worked in, in
    /// front the one in front (never Trek's).
    fn scope(&mut self) -> Result<Scope, String> {
        if !self.desktop.has_elements() {
            return Err(desktop::NO_ELEMENTS.into());
        }
        match self.mode {
            Mode::Background => {
                let w = self.working_window()?;
                // Its latest screenshot's pixels, or those a screenshot of it would have.
                let m = self
                    .target
                    .as_ref()
                    .and_then(|t| t.shot)
                    .filter(|m| {
                        (m.logical_w - w.w).abs() <= 1.0 && (m.logical_h - w.h).abs() <= 1.0
                    })
                    .unwrap_or_else(|| Mapping::for_display(w.w, w.h));
                Ok(Scope {
                    origin: (0.0, 0.0),
                    scale: m.scale,
                    image: (m.image_w, m.image_h),
                    w,
                })
            }
            Mode::Front => {
                let w = self
                    .desktop
                    .key_window()
                    .map_err(|e| {
                        format!(
                            "Couldn't check which window is in front ({e}), so nothing was done."
                        )
                    })?
                    .ok_or("No window is in front to read.")?;
                if is_trek(&w) {
                    return Err(OWN_WINDOW.into());
                }
                let m = self.mapping();
                Ok(Scope {
                    origin: (w.x, w.y),
                    scale: m.scale,
                    image: (m.image_w, m.image_h),
                    w,
                })
            }
        }
    }

    /// Read `scope`'s elements. Refs given for another window are dropped first: a ref names an
    /// element of the window it was read from.
    fn read(&mut self, scope: &Scope, menu_bar: bool) -> Result<desktop::Elements, String> {
        if self.listed != Some(scope.w.id) {
            self.refs.clear();
            self.listed = Some(scope.w.id);
        }
        self.desktop.elements(
            &scope.w,
            &Reading {
                menu_bar,
                ..Reading::default()
            },
        )
    }

    /// How `scope`'s elements are written: frames in the pixels x/y tools take.
    fn view<'a>(
        scope: &Scope,
        filter: Option<&'a str>,
        interactive_only: bool,
        max: usize,
    ) -> View<'a> {
        View {
            filter,
            interactive_only,
            max,
            scale: scope.scale,
            origin: scope.origin,
            shows: Some(Frame {
                x: 0.0,
                y: 0.0,
                w: scope.w.w,
                h: scope.w.h,
            }),
        }
    }

    /// What `elements` answers: whose they are and how a frame reads, then the outline.
    fn outlined(&mut self, scope: &Scope, read: &desktop::Elements, view: &View) -> String {
        let out = self.refs.outline(&read.roots, view);
        let w = &scope.w;
        let mut said = match self.mode {
            Mode::Background => format!(
                "Elements of {}'s window{} (window_id {}). frame=x,y,width,height in pixels of its screenshot ({}x{}).\n",
                w.owner,
                titled(w),
                w.id,
                scope.image.0,
                scope.image.1
            ),
            Mode::Front => format!(
                "Elements of {}'s window{}, the one in front. frame=x,y,width,height in pixels of the full-screen screenshot.\n",
                w.owner,
                titled(w)
            ),
        };
        if let Some(focused) = &out.focused {
            said.push_str(&format!("Focused: {focused}\n"));
        }
        said.push('\n');
        match (out.shown + out.more, view.filter) {
            (0, Some(filter)) => said.push_str(&format!(
                "(No element's role or name contains {filter:?}.)\n"
            )),
            (0, None) => said.push_str(
                "(Nothing here to read or act on: the app may draw everything itself. A screenshot shows it.)\n",
            ),
            _ => said.push_str(&out.text),
        }
        if let Some(cut) = read.cut {
            said.push_str(&format!(
                "… (There's more: the reading stopped early because {cut}.)\n"
            ));
        }
        said
    }

    fn elements(&mut self, args: &Value) -> ToolResult {
        if self.mode == Mode::Background
            && let Some(id) = opt_f64(args, "window_id")?
        {
            self.pick_window(id as i64)?;
        }
        let flag = |key: &str| args.get(key).and_then(Value::as_bool).unwrap_or(false);
        let max = opt_f64(args, "max")?.map_or(outline::DEFAULT_MAX, |m| {
            (m.max(1.0) as usize).min(outline::MOST)
        });
        let scope = self.scope()?;
        let read = self.read(&scope, flag("menu_bar"))?;
        let view = Self::view(
            &scope,
            opt_str(args, "filter"),
            flag("interactive_only"),
            max,
        );
        let mut said = self.outlined(&scope, &read, &view);
        if self.mode == Mode::Background {
            let w = &scope.w;
            if self.users_window().is_some_and(|f| f.pid == w.pid) {
                said.push_str(&format!(
                    "{} is the app the user is using right now (it's in front and they're at the Mac): acting there is refused until they switch to another app or step away (ask them, or wait).\n",
                    w.owner
                ));
            }
            said.push_str(self.elsewhere().trim_start());
            self.tell(activity("elements", "Reading".into(), w, None, None));
        }
        Ok(vec![text(said.trim_end())])
    }

    /// The element ref `r` names, found again in the window its elements were read from and
    /// proven the same one (`Refs::prove`): nothing is done to anything else.
    fn found_again(&mut self, r: &str) -> Result<(Scope, Node), String> {
        let scope = self.scope()?;
        if self.listed.is_some_and(|id| id != scope.w.id) {
            return Err(format!(
                "Ref {r} is from another window's elements; {}'s window{} is the one {} now. Read its elements and use a ref from that. Nothing was done.",
                scope.w.owner,
                titled(&scope.w),
                match self.mode {
                    Mode::Background => "you work in",
                    Mode::Front => "in front",
                }
            ));
        }
        let place = self.refs.get(r)?.place.clone();
        let now = self.desktop.element_at(&scope.w, &place)?;
        self.refs.prove(r, now.as_ref())?;
        let now = now.ok_or("That element isn't there any more. Read the elements again.")?;
        Ok((scope, now))
    }

    /// Whether the centre of `frame` (an element's, in the window's own coordinates) is within
    /// the window: where a click on it goes.
    fn centre_in(w: &WindowInfo, frame: Option<Frame>) -> Option<(f64, f64)> {
        let window = Frame {
            x: 0.0,
            y: 0.0,
            w: w.w,
            h: w.h,
        };
        frame.map(Frame::centre).filter(|c| window.holds(*c))
    }

    /// Click at (`cx`, `cy`) in `w`'s own coordinates, the way the mode clicks: behind, or with
    /// the real pointer where no other window is over the point. What was put back, for the reply.
    fn click_in(
        &mut self,
        w: &WindowInfo,
        (cx, cy): (f64, f64),
        button: Button,
        count: i64,
        what: &str,
    ) -> Result<String, String> {
        match self.mode {
            Mode::Background => self.post_watched(
                w,
                &Input::Click {
                    x: cx,
                    y: cy,
                    button,
                    count: count as u32,
                },
            ),
            Mode::Front => {
                let (sx, sy) = (w.x + cx, w.y + cy);
                self.keep_off_trek(&[(sx, sy)])?;
                let windows = self.desktop.windows().unwrap_or_default();
                if window_at(&windows, sx, sy).is_some_and(|top| top.id != w.id) {
                    return Err(format!(
                        "Another window covers {what} now, so the click would land there. Nothing was done."
                    ));
                }
                self.desktop.click(sx, sy, button, count as u32)?;
                Ok(String::new())
            }
        }
    }

    /// `press`: an element's own accessibility action, by ref. No pointer or key event is made
    /// and nothing is given the focus.
    fn press(&mut self, args: &Value) -> ToolResult {
        let r = arg_str(args, "ref")?;
        let action = match opt_str(args, "action") {
            None => Action::Press,
            Some(a) => Action::parse(a).ok_or_else(|| {
                format!(
                    "Unknown action {a:?}; use press, show_menu, increment, decrement, confirm or cancel"
                )
            })?,
        };
        let (scope, now) = self.found_again(r)?;
        let w = scope.w.clone();
        if self.mode == Mode::Background {
            self.keep_out_of_users_app(&w)?;
        }
        let what = now.describe();
        if now.disabled {
            return Err(format!("{what} is disabled, so nothing was done."));
        }
        let identity = now.identity.filter(|_| now.actions.contains(&action));
        let Some(identity) = identity else {
            let has: Vec<&str> = now.actions.iter().map(|a| a.name()).collect();
            return Err(format!(
                "{what} has no {} action of its own ({}). Nothing was done: click it instead (click with its ref).",
                action.name(),
                if has.is_empty() {
                    "it has none".to_string()
                } else {
                    format!("it has {}", has.join(", "))
                }
            ));
        };
        let failed = |e: String| format!("{what} wasn't acted on: {e} Nothing was done.");
        let (done, raised) = match self.mode {
            Mode::Background => self
                .watched(&w, false, |desktop| desktop.perform(&w, identity, action))
                .map_err(failed)?,
            Mode::Front => (
                self.desktop.perform(&w, identity, action).map_err(failed)?,
                String::new(),
            ),
        };
        if self.mode == Mode::Background {
            let at = now.frame.map(Frame::centre);
            self.tell(activity(
                "press",
                format!("Pressing {}", outline::clip(&now.name, 32)),
                &w,
                at,
                None,
            ));
        }
        let place = self.elsewhere();
        match done {
            Performed::Done => {
                let after = self.desktop.element_at(&w, &now.place).ok().flatten();
                Ok(vec![text(format!(
                    "{} {what} by its own accessibility action: no pointer or key was sent.{}{raised}{place}",
                    action.did(),
                    after_action(&now, after.as_ref())
                ))])
            }
            // Not an error and not retried: whether it happened is for the agent to look at.
            Performed::Unknown(why) => Ok(vec![text(format!(
                "Outcome unknown: {what} was asked to {} by its own accessibility action, but {why}, so it may or may not have happened (an action that opens a dialog often ends this way). It was not tried again: read the elements, or take a screenshot, to see what happened before doing anything more.{raised}{place}",
                action.name()
            ))]),
        }
    }

    /// `click` with a ref: the element's centre, for a control that ignores `press`.
    fn click_ref(&mut self, args: &Value, r: &str) -> ToolResult {
        if ["x", "y"]
            .iter()
            .any(|k| args.get(k).is_some_and(|v| !v.is_null()))
        {
            return Err("Give ref, or x and y, not both.".into());
        }
        let (button, count) = click_kind(args)?;
        let (scope, now) = self.found_again(r)?;
        let w = scope.w.clone();
        if self.mode == Mode::Background {
            self.keep_out_of_users_app(&w)?;
        }
        let what = now.describe();
        let Some(frame) = now.frame else {
            return Err(format!(
                "{what} doesn't say where it is, so there's nowhere to click. Nothing was done: use press with its ref."
            ));
        };
        let Some((cx, cy)) = Self::centre_in(&w, Some(frame)) else {
            return Err(format!(
                "{what} is outside what the window shows (scrolled out of view), so there's nowhere to click. Nothing was done: scroll it into view and read the elements again, or use press with its ref."
            ));
        };
        let raised = self.click_in(&w, (cx, cy), button, count, &what)?;
        let (px, py) = (
            ((scope.origin.0 + cx) / scope.scale).round(),
            ((scope.origin.1 + cy) / scope.scale).round(),
        );
        let did = clicked(count, button);
        match self.mode {
            Mode::Background => {
                self.tell(activity(
                    "click",
                    "Clicking".into(),
                    &w,
                    Some((cx, cy)),
                    None,
                ));
                Ok(vec![text(format!(
                    "{did} {what} at its centre ({}, {}) in {}'s window, in the background.{raised}{}",
                    fmt_num(px),
                    fmt_num(py),
                    w.owner,
                    self.elsewhere()
                ))])
            }
            Mode::Front => Ok(vec![text(format!(
                "{did} {what} at its centre ({}, {}).",
                fmt_num(px),
                fmt_num(py)
            ))]),
        }
    }

    /// `type_text` with a ref: the field is given the focus first (asked through accessibility,
    /// else by a click in it), and only once it says it has it is anything typed.
    fn type_ref(&mut self, args: &Value, r: &str) -> ToolResult {
        let s = arg_str(args, "text")?;
        if s.is_empty() {
            return Err("text is empty".into());
        }
        let (scope, now) = self.found_again(r)?;
        let w = scope.w.clone();
        match self.mode {
            Mode::Background => self.keep_out_of_users_app(&w)?,
            Mode::Front => self.keep_keys_off_trek()?,
        }
        let what = now.describe();
        if now.disabled {
            return Err(format!("{what} is disabled, so nothing was typed."));
        }
        let mut raised = String::new();
        let mut how = "";
        let mut focused = now.focused;
        if !focused && let Some(identity) = now.identity {
            focused = self.desktop.focus(&w, identity).unwrap_or(false);
        }
        if !focused && let Some(centre) = Self::centre_in(&w, now.frame) {
            raised = self.click_in(&w, centre, Button::Left, 1, &what)?;
            std::thread::sleep(FOCUS_SETTLE);
            focused = self
                .desktop
                .element_at(&w, &now.place)?
                .is_some_and(|n| n.focused);
            how = " (it took the focus from a click in it, not when asked)";
        }
        if !focused {
            return Err(format!(
                "{what} didn't take the keyboard focus (asked through accessibility, then by a click in it), so nothing was typed: the text could have gone somewhere else. Read the elements to see what's [focused]; to type where the focus is, call type_text without ref."
            ));
        }
        match self.mode {
            Mode::Background => {
                raised.push_str(&self.post_watched(&w, &Input::Type(s.to_string()))?);
                self.tell(activity("type", typing(s), &w, None, None));
            }
            Mode::Front => {
                self.desktop.type_text(s)?;
                std::thread::sleep(AFTER_INPUT);
            }
        }
        let after = self.desktop.element_at(&w, &now.place).ok().flatten();
        let reads = match after.filter(|a| a.role == now.role && a.name == now.name) {
            _ if now.secure => ". It's a secure field, so what it holds isn't read.".to_string(),
            None => ". It's no longer where it was (the window changed), so what it holds now isn't known: read the elements again.".to_string(),
            Some(a) if a.value == now.value => format!(
                ", but it reads as it did before ({}): the text may not have reached it. It wasn't typed again: look (elements, or a screenshot) before retrying.",
                quoted(a.value.as_deref().unwrap_or_default())
            ),
            Some(a) => format!(
                "; it now reads {}.",
                quoted(a.value.as_deref().unwrap_or_default())
            ),
        };
        Ok(vec![text(format!(
            "Typed {} characters into {what}{how}{reads}{raised}{}",
            s.chars().count(),
            if self.mode == Mode::Background {
                self.elsewhere()
            } else {
                String::new()
            }
        ))])
    }

    /// `wait`: for a time, or until text or an element shows (or goes) in the window, answered
    /// with its fresh elements either way.
    fn wait(&mut self, args: &Value) -> ToolResult {
        if !waits_on_elements(args) {
            let ms = arg_f64(args, "ms")?;
            if !(0.0..=10_000.0).contains(&ms) {
                return Err("ms must be between 0 and 10000".into());
            }
            std::thread::sleep(Duration::from_millis(ms as u64));
            return Ok(vec![text(format!("Waited {} ms.", ms as u64))]);
        }
        let (needle, by_text) = match (opt_str(args, "text"), opt_str(args, "element")) {
            (Some(t), None) => (t, true),
            (None, Some(e)) => (e, false),
            (Some(_), Some(_)) => return Err("Give text or element, not both.".into()),
            (None, None) => return Err("text or element is empty".into()),
        };
        let what = if by_text {
            format!("the text {needle:?}")
        } else {
            format!("an element matching {needle:?}")
        };
        let gone = args.get("gone").and_then(Value::as_bool).unwrap_or(false);
        let within = opt_f64(args, "timeout_ms")?.unwrap_or(10_000.0);
        if !(0.0..=60_000.0).contains(&within) {
            return Err("timeout_ms must be between 0 and 60000".into());
        }
        let within = Duration::from_millis(within as u64);
        let started = std::time::Instant::now();
        loop {
            let scope = self.scope()?;
            let read = match self.read(&scope, false) {
                Ok(read) => read,
                // An app too busy to answer has said nothing either way: it's asked again
                // until the time is up.
                Err(_) if started.elapsed() < within => {
                    std::thread::sleep(WAIT_POLL);
                    continue;
                }
                Err(e) => return Err(e),
            };
            let there = if by_text {
                outline::has_text(&read.roots, needle)
            } else {
                outline::has_element(&read.roots, needle)
            };
            // Only a reading that got to the end shows that something has gone.
            let met = if gone {
                !there && read.cut.is_none()
            } else {
                there
            };
            let waited = started.elapsed();
            if met || waited >= within {
                let view = Self::view(&scope, None, false, outline::DEFAULT_MAX);
                let outlined = self.outlined(&scope, &read, &view);
                let outlined = outlined.trim_end();
                return if met {
                    Ok(vec![text(format!(
                        "{} after {:.1} s.\n\n{outlined}",
                        if gone {
                            format!("Gone: {what}")
                        } else {
                            format!("Found {what}")
                        },
                        waited.as_secs_f32()
                    ))])
                } else {
                    Err(format!(
                        "Waited {:.0} s, and {what} {}.\n\n{outlined}",
                        within.as_secs_f32(),
                        match (gone, there) {
                            (true, true) => "is still there",
                            (true, false) =>
                                "wasn't in what could be read, but the window couldn't be read to its end, so whether it has gone isn't known",
                            (false, _) => "never showed",
                        }
                    ))
                };
            }
            std::thread::sleep(WAIT_POLL);
        }
    }
}

/// How long without a key, click, scroll or pointer movement from the user before they count as
/// away from the Mac. Until then the app in front is theirs, though they may only be reading it: a
/// long page read without touching anything is still in use. The screen says so sooner when it
/// locks, sleeps or starts its screen saver, which the user's own settings time.
const AWAY_AFTER: Duration = Duration::from_secs(5 * 60);

/// How long a window that's listed but not on screen has to come on screen before it's taken
/// for gone (a window just opened can be off screen for a few seconds), and how often it's
/// looked for meanwhile.
const OPENING: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 3000 });
const OPENING_POLL: Duration = Duration::from_millis(if cfg!(test) { 1 } else { 150 });

/// How long an app is watched for bringing its window forward, once it has opened one more
/// window or a click or key has brought one up…
const SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 1500 });
/// …and once an app that was launched shows its window: a cold start goes on for a while, and
/// can take the front more than once (Chrome does).
const LAUNCH_SETTLE: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 3000 });
/// After the app last came forward it's watched this much longer, and for this many looks at
/// least (tests go by the looks: their time is no time), but never longer than `WATCH_MAX`.
const QUIET: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 1500 });
const QUIET_LOOKS: u32 = 8;
const WATCH_MAX: Duration = Duration::from_secs(10);
/// How often the windows are looked at while an app is watched. A window that comes forward is
/// seen within this, and its app is in front for about this long more than putting the user's
/// back takes (15 to 60 ms, measured on macOS 27).
const LOOK: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 6 });
/// How long the window server gets to show the user's window back in front (and how many looks
/// at least) before it's asked again; asked `TRIES` times to no effect, the app's window is
/// taken to stay.
const SHOWS: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 150 });
const SHOWS_LOOKS: u32 = 6;
const TRIES: u32 = 3;
/// The most askings one coming-forward gets, each new thing the app does answered at once: an
/// app that does something new at every look isn't asked back for ever.
const ASKS: u32 = 12;
/// How long ago a click of the user's can be and still be what brought a window forward, and
/// how long ago they can have let go of the app switcher's keys.
const CLICKED: Duration = Duration::from_millis(400);
const SWITCHED: Duration = Duration::from_millis(400);

/// How long after a click or key behind the windows are looked at for one it brought forward
/// (an app activating itself does within this; slower ones are caught at the next input).
const AFTER_INPUT: Duration = Duration::from_millis(if cfg!(test) { 0 } else { 250 });

/// Said when the agent would drive the app the user is using. Not even in a window of its own:
/// within one app, the window the agent clicks or types in takes the keyboard from the user's.
fn in_use(app: &str) -> String {
    format!(
        "{app} is the app the user is using right now (it's in front and they're at the Mac, if only reading); ask them whether to work in it (they switch to another app, then you go on), or wait and try again. Nothing was done."
    )
}

/// Said when the window the agent worked in can't be worked in any more: what became of it.
fn gone(owner: &str, title: &str, how: &Whereabouts) -> String {
    let title = if title.is_empty() {
        String::new()
    } else {
        format!(" “{title}”")
    };
    let other =
        "work in another window: call list_windows and screenshot another window_id, or open_app.";
    match how {
        Whereabouts::Minimised(_) => format!(
            "{owner}'s window{title} is minimised. Restoring it would put it in front of the user's windows, so ask them to restore it, or {other}"
        ),
        Whereabouts::Hidden(_) => format!(
            "{owner}'s window{title} is hidden, with the rest of {owner}. Showing it would put {owner} in front of the user's windows, so ask them to show it, or {other}"
        ),
        Whereabouts::NotShowing(_) => format!(
            "{owner}'s window{title} isn't on screen any more: {owner} closed it or put it away. Call list_windows and screenshot another window_id, or open_app."
        ),
        Whereabouts::Closed | Whereabouts::OnScreen(_) | Whereabouts::OtherSpace { .. } => {
            format!(
                "{owner}'s window{title} has closed. Call list_windows and screenshot another window_id, or open_app."
            )
        }
    }
}

/// Said when the window the agent works in couldn't be captured (`e`): on another Space that's
/// what's in the way, and the user can bring it back.
fn unseen(w: &WindowInfo, place: Place, e: String) -> String {
    if place == Place::Here {
        return e;
    }
    format!(
        "{}'s window{} {} and can't be seen from here ({e}). Ask the user to bring it to the desktop they're on (or out of full screen), or work in another window: call list_windows and screenshot another window_id, or open_app.",
        w.owner,
        titled(w),
        place.said()
    )
}

/// Said when the app the agent opened shows no window: keys reach it, nothing else does.
const NO_WINDOW_YET: &str = "Keys go to it now, to whatever it has focused: a document app may hold its Open dialog out of sight (it shows only in front), and typing or Return would go there. Press cmd+n first for a new window (in most apps; in Notes or Calendar it makes a new item instead), then call list_windows and screenshot with the window's window_id.";

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
        // Where windows have elements to read, `click` and `type_text` take a ref as well.
        let elements = self.desktop.has_elements();
        let mut type_props = json!({"text": {"type": "string", "description": "Text to type"}});
        let mut wait_props = json!({"ms": {"type": "integer", "minimum": 0, "maximum": 10000}});
        if elements {
            click_props.insert(
                "ref".into(),
                json!({"type": "string", "description": "An element's ref, instead of x and y"}),
            );
            type_props["ref"] = json!({"type": "string", "description": "The field's ref"});
            wait_props["text"] =
                json!({"type": "string", "description": "In an element's name or value"});
            wait_props["element"] =
                json!({"type": "string", "description": "In an element's role or name"});
            wait_props["gone"] = json!({"type": "boolean"});
            wait_props["timeout_ms"] =
                json!({"type": "integer", "description": "Default 10000, at most 60000"});
        }
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
        // What a tool says where there are elements, and where there aren't.
        let with =
            |plain: &'static str, by_ref: &'static str| if elements { by_ref } else { plain };
        let mut tools = vec![
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
                    with(
                        "Move the pointer to (x, y) in screenshot pixels and click. Supports right-click and double/triple-click.",
                        "Move the pointer to (x, y) in screenshot pixels, or to the centre of the element `ref` names, and click. Supports right-click and double/triple-click.",
                    ),
                    with(
                        "Click at (x, y) in pixels of the window's screenshot, in the background: the user's pointer doesn't move. Supports right-click and double/triple-click.",
                        "Click at (x, y) in pixels of the window's screenshot, or at the centre of the element `ref` names, in the background: the user's pointer doesn't move. Supports right-click and double/triple-click.",
                    ),
                ),
                input_schema: if elements {
                    json!({"type": "object", "properties": click_props})
                } else {
                    json!({"type": "object", "properties": click_props, "required": ["x", "y"]})
                },
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
                    with(
                        "Type text into the focused element as keyboard input (Unicode-safe; newlines press Return, tabs press Tab). Click the field first to focus it.",
                        "Type text into the focused element as keyboard input (Unicode-safe; newlines press Return, tabs press Tab). Pass a field's `ref` to focus it first, or click it first.",
                    ),
                    with(
                        "Type text into the focused element of the window you work in (Unicode-safe; newlines press Return, tabs press Tab), in the background: the user's own typing isn't affected. Click the field first to focus it.",
                        "Type text into the focused element of the window you work in (Unicode-safe; newlines press Return, tabs press Tab), in the background: the user's own typing isn't affected. Pass a field's `ref` to focus it first, or click it first.",
                    ),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": type_props,
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
                    "Work in a macOS app, by name, e.g. \"Safari\", \"Notes\", \"TextEdit\", behind the user's windows. Running already: its frontmost window is used and nothing is launched. Not running: it's launched behind (never brought forward). Refused while it's the app the user is using. Read its elements next.",
                ),
                input_schema: if behind {
                    json!({
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "Application name, bundle id or path"},
                            "new_window": {"type": "boolean", "default": false, "description": "Ask the running app for one more window to work in (it opens a window, not another copy of the app), so the user's own windows of it stay as they are. It presses cmd+n in the app, which in some (Notes, Calendar) makes a new item instead"},
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
                description: with(
                    "Pause for `ms` milliseconds (max 10000), e.g. to let an app finish loading before the next screenshot.",
                    "Wait until `text` or an `element` shows in the window (`gone`: until it goes), answered with the window's fresh elements; or pause for `ms` milliseconds (max 10000).",
                ),
                input_schema: if elements {
                    json!({"type": "object", "properties": wait_props})
                } else {
                    json!({"type": "object", "properties": wait_props, "required": ["ms"]})
                },
            },
        ];
        if elements {
            let mut props = json!({
                "filter": {"type": "string", "description": "Only elements whose role or name contains this"},
                "interactive_only": {"type": "boolean", "description": "Only those you can act on"},
                "menu_bar": {"type": "boolean", "description": "Also the app's menus and their items (press an item by ref: no menu opens)"},
                "max": {"type": "integer", "description": "Most elements shown (default 150)"},
            });
            if behind {
                props["window_id"] = json!({"type": "integer", "description": "The window to work in, from list_windows"});
            }
            tools.push(ToolDef {
                name: "elements",
                description: pick(
                    "The window in front as an outline, one element a line: role, name, value, state and frame (x,y,width,height in screenshot pixels), with a ref like [ref=e12] on each one you can act on. Read it before a screenshot: names and places are exact.",
                    "The window you work in as an outline, one element a line: role, name, value, state and frame (x,y,width,height in pixels of its screenshot), with a ref like [ref=e12] on each one you can act on. Read it before a screenshot: names and places are exact.",
                ),
                input_schema: json!({"type": "object", "properties": props}),
            });
            tools.push(ToolDef {
                name: "press",
                description: "An element's own action (a button's press), by its ref: no pointer or key is sent and nothing takes the focus. Refused if the element changed since you read it. If a control ignores it, click with its ref.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "ref": {"type": "string", "description": "From elements, e.g. \"e12\""},
                        "action": {"type": "string", "enum": ["press", "show_menu", "increment", "decrement", "confirm", "cancel"], "default": "press", "description": "show_menu: its context menu. The rest: for an element whose line lists them"},
                    },
                    "required": ["ref"],
                }),
            });
        }
        tools
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        if needs_accessibility(name, args) && !self.desktop.may_post_input() {
            self.desktop.ask_to_post_input();
            return Err(format!(
                "{ACCESSIBILITY_ERROR}{}",
                self.desktop.permissions_note()
            ));
        }
        // By ref, in either mode: the element's own window, found again first.
        let by_ref = opt_str(args, "ref");
        match (name, by_ref) {
            ("elements", _) => return self.elements(args),
            ("press", _) => return self.press(args),
            ("click", Some(r)) => return self.click_ref(args, r),
            ("type_text", Some(r)) => return self.type_ref(args, r),
            ("wait", _) => return self.wait(args),
            _ => {}
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
            clicked: None,
            front: None,
            refs: Refs::new("elements"),
            listed: None,
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
            assert!(needs_accessibility(tool, &json!({})));
            assert_eq!(
                c.call(tool, &acting_args(tool)),
                Err(ACCESSIBILITY_ERROR.to_string()),
                "{tool}"
            );
        }
        assert_eq!(fake.events(), vec![], "nothing was posted");
        // Looking, launching and waiting need no Accessibility.
        for tool in ["screenshot", "list_windows", "open_app", "wait"] {
            assert!(!needs_accessibility(tool, &json!({"ms": 0})), "{tool}");
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

    /// TextEdit on disk, to be launched behind Notes; its window (11, pid 50) is what appears.
    fn launching_textedit(fake: Recording) -> Recording {
        let fake = Recording {
            installed: vec![copy(
                "/System/Applications/TextEdit.app",
                "com.apple.TextEdit",
            )],
            ..fake
        };
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        fake
    }

    fn put_backs(fake: &Recording) -> usize {
        fake.events()
            .iter()
            .filter(|e| matches!(e, Event::BringForward(1)))
            .count()
    }

    #[test]
    fn an_app_that_comes_forward_again_is_put_back_each_time() {
        // As Chrome does while it starts: in front when its window appears, and twice more, each
        // a few looks after it was put back.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            regrab_after: 3,
            ..behind_the_users()
        });
        *fake.regrabs.borrow_mut() = 2;
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 3, "{said}");
        assert!(
            said.contains("TextEdit brought its window forward 3 times; each time it was put back behind the user's Notes window. It was in front for ")
                && !said.contains("stays there"),
            "{said}"
        );
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        // One more window of a running app that does the same.
        let fake = Recording {
            jumps_forward: true,
            regrab_after: 5,
            ..using_textedit(600)
        };
        fake.opens.borrow_mut().push(WindowInfo {
            owner: "Safari".into(),
            id: 70,
            ..textedit_window(42, 70)
        });
        *fake.regrabs.borrow_mut() = 1;
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        assert!(
            said.contains("Safari brought its window forward 2 times; each time it was put back behind the user's TextEdit window."),
            "{said}"
        );
        assert_eq!(
            fake.events()
                .iter()
                .filter(|e| matches!(e, Event::BringForward(11)))
                .count(),
            2
        );
    }

    #[test]
    fn an_app_that_takes_the_active_status_gives_it_back() {
        // Its window comes forward and its app becomes the active one, as Chrome's does: the
        // user's window goes back in front and their app is the active one again.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            takes_active: true,
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(1);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 1);
        assert_eq!(*fake.active.borrow(), Some(1), "Notes is active again");
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        assert!(
            said.contains("TextEdit brought its window forward; it was put back behind the user's Notes window."),
            "{said}"
        );
        // It takes the active status with its window left behind (the menu bar and the keys
        // are its app's, though nothing covers the user's window): given back too.
        let fake = launching_textedit(Recording {
            takes_active: true,
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(1);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 1);
        assert_eq!(*fake.active.borrow(), Some(1));
        assert!(
            said.contains("TextEdit made itself the active app; the user's Notes window was made active again. It was the active app for "),
            "{said}"
        );
        // The user's window isn't among the windows on screen (it's full screen, on a Space the
        // system left when the app took the front): the active app alone tells, and theirs is
        // made active again.
        let keynote = WindowInfo {
            owner: "Keynote".into(),
            ..textedit_window(77, 90)
        };
        let fake = launching_textedit(Recording {
            takes_active: true,
            key_window: Some(Ok(Some(keynote))),
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(77);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(
            fake.events().last(),
            Some(&Event::BringForward(90)),
            "{said}"
        );
        assert_eq!(*fake.active.borrow(), Some(77));
        // An app that was the active one already (it has no window; the user's away) takes
        // nothing by staying so when it's asked for one.
        let fake = Recording {
            takes_active: true,
            idle: Some(Duration::from_secs(600)),
            ..behind_the_users()
        };
        fake.running
            .borrow_mut()
            .push(running("TextEdit", "com.apple.TextEdit", 50));
        fake.opens.borrow_mut().push(textedit_window(50, 11));
        *fake.active.borrow_mut() = Some(50);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(fake.events(), vec![Event::Reopen(50)], "{said}");
        // A window that won't go back while the keyboard does: said as it is. (The keyboard is
        // given back once; after that the window is only asked to go behind.)
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            takes_active: true,
            stubborn: true,
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(1);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(
            fake.events()[1..],
            [Event::BringForward(1), Event::Raise(1), Event::Raise(1)]
        );
        assert_eq!(*fake.active.borrow(), Some(1));
        assert!(
            said.contains("Note: TextEdit's window came to the front when it opened and stays there, in front of the user's Notes window rather than behind it (TextEdit isn't the active app, so the keyboard is still the user's). Tell the user it's in their way."),
            "{said}"
        );
        // Its window comes forward while a third app has the keyboard (an alert, a launcher's
        // panel): the user's window is raised over it and that's all. The third app stays the
        // active one.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(99);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(
            fake.events(),
            vec![
                Event::Launch("/System/Applications/TextEdit.app".into()),
                Event::Raise(1)
            ]
        );
        assert_eq!(*fake.active.borrow(), Some(99));
        assert!(
            said.contains("TextEdit brought its window forward; it was put back behind the user's Notes window."),
            "{said}"
        );
    }

    #[test]
    fn a_window_that_lands_after_its_app_turned_active_is_put_back_at_once() {
        // Its app turns active, and its window gets on top two looks later, while the first
        // put-back hasn't shown yet (a slow window server): the user's window is asked back
        // again there and then, not once the first asking has been waited out.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            takes_active: true,
            lands_after: Some(2),
            slow: SHOWS_LOOKS - 1,
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(1);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 2, "{said}");
        assert!(
            said.contains("TextEdit brought its window forward; it was put back behind the user's Notes window."),
            "{said}"
        );
        assert_eq!(*fake.active.borrow(), Some(1));
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        // The window lands when the user's app has the keyboard again and the first put-back
        // has shown: a second coming-forward, answered by raising their window and no more.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            takes_active: true,
            lands_after: Some(3),
            ..behind_the_users()
        });
        *fake.active.borrow_mut() = Some(1);
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(
            fake.events()[1..],
            [Event::BringForward(1), Event::Raise(1)],
            "{said}"
        );
        assert!(
            said.contains("2 times; each time it was put back"),
            "{said}"
        );
        assert_eq!(*fake.active.borrow(), Some(1));
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
    }

    #[test]
    fn a_slow_window_server_isnt_taken_for_a_window_that_stays() {
        // The order changes only a few looks after the user's window is put back: it's asked
        // for once, and waited for, not asked three times in a millisecond and given up on.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            slow: SHOWS_LOOKS - 2,
            ..behind_the_users()
        });
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 1, "{said}");
        assert!(
            said.contains("it was put back behind the user's Notes window")
                && !said.contains("stays there"),
            "{said}"
        );
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        // Slower than it's waited for: asked again, and still not given up on.
        let fake = launching_textedit(Recording {
            jumps_forward: true,
            slow: SHOWS_LOOKS + 3,
            ..behind_the_users()
        });
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "TextEdit"})));
        assert_eq!(put_backs(&fake), 2, "{said}");
        assert!(
            said.contains("TextEdit brought its window forward; it was put back")
                && !said.contains("stays there"),
            "{said}"
        );
    }

    #[test]
    fn a_window_the_user_clicks_forward_themselves_is_theirs() {
        // Safari's new window opens behind, beside the user's Notes window; a moment later it's
        // in front, and the user has just clicked with the pointer on it: left there.
        let beside = || WindowInfo {
            owner: "Safari".into(),
            x: 800.0,
            y: 100.0,
            ..textedit_window(42, 70)
        };
        let clicked = |at: (f64, f64)| {
            let fake = Recording {
                jumps_forward: true,
                comes_after: Some(2),
                last_click: Some((Duration::ZERO, at)),
                ..behind_the_users()
            };
            fake.running
                .borrow_mut()
                .push(running("Safari", "com.apple.Safari", 42));
            fake.opens.borrow_mut().push(beside());
            fake
        };
        // (Its right edge, which neither Notes nor Safari's other window reaches.)
        let fake = clicked((1300.0, 200.0));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        assert!(
            said.contains(
                "The user clicked Safari's window forward themselves, so it was left in front"
            ),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 0);
        assert_eq!(c.desktop.windows().unwrap()[0].id, 70);
        // The same click in their own Notes window, as the new window comes forward by
        // itself: that's not their doing, and their window goes back in front.
        let fake = clicked((300.0, 300.0));
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        assert!(
            said.contains(
                "Safari brought its window forward; it was put back behind the user's Notes window."
            ),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 1);
        // A window that's in front the moment it appears can't have been clicked: nothing of
        // Safari's lay under the pointer before.
        let fake = Recording {
            comes_after: None,
            ..clicked((1300.0, 200.0))
        };
        let mut c = behind(&fake);
        reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        assert_eq!(put_backs(&fake), 1);
        // A click on the window Safari had already brings its app forward just as well.
        let fake = Recording {
            comes_after: None,
            ..clicked((900.0, 600.0))
        };
        let mut c = behind(&fake);
        reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        assert_eq!(put_backs(&fake), 0);
        // A press the system counted when the agent's own click went out may be that click:
        // it isn't taken for the user's. One from before or after is.
        let fake = clicked((900.0, 200.0));
        let mut c = behind(&fake);
        let notes = c.desktop.windows().unwrap().remove(0);
        let watch = c.watch(Some(notes), 42, "Safari", false).unwrap();
        assert_eq!(
            c.users_doing(&watch),
            Some(By::Window),
            "on Safari's window, behind Notes"
        );
        c.clicked = Some(std::time::Instant::now());
        assert_eq!(c.users_doing(&watch), None);
        c.clicked = Some(std::time::Instant::now() - Duration::from_secs(5));
        assert_eq!(c.users_doing(&watch), Some(By::Window));
    }

    /// Safari, running, asked for one more window: it opens behind the user's Notes window
    /// and five looks later it's in front, its app the active one. `keys`: what the keyboard
    /// says at each asking (Command down, Tab or ` down), the first as the watch begins.
    fn safari_comes_forward(keys: &[(bool, bool)], fake: Recording) -> (Recording, String) {
        let fake = Recording {
            jumps_forward: true,
            takes_active: true,
            comes_after: Some(4),
            ..fake
        };
        *fake.active.borrow_mut() = Some(1);
        fake.running
            .borrow_mut()
            .push(running("Safari", "com.apple.Safari", 42));
        fake.opens.borrow_mut().push(WindowInfo {
            owner: "Safari".into(),
            ..textedit_window(42, 70)
        });
        fake.keys.borrow_mut().extend(keys.iter().copied());
        let mut c = behind(&fake);
        let said = reply(c.call("open_app", &json!({"name": "Safari", "new_window": true})));
        (fake, said)
    }

    #[test]
    fn the_users_own_switch_to_the_app_is_never_undone() {
        const UP: (bool, bool) = (false, false);
        const COMMAND: (bool, bool) = (true, false);
        const COMMAND_TAB: (bool, bool) = (true, true);
        // Command-Tab: Command goes down, Tab is tapped, Command is let go, and Safari is in
        // front at the next look. It's left there, and it's the active app.
        let (fake, said) = safari_comes_forward(
            &[UP, COMMAND, COMMAND_TAB, COMMAND, COMMAND, UP, UP],
            behind_the_users(),
        );
        assert!(
            said.contains("The user switched to Safari themselves, with Command-Tab, so it was left in front: it's theirs to use now."),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 0);
        assert_eq!(*fake.active.borrow(), Some(42));
        // The switcher was up before the watch began (Command already held, its Tab pressed
        // unseen): theirs just the same.
        let (fake, said) = safari_comes_forward(
            &[COMMAND, COMMAND, COMMAND, COMMAND, COMMAND, UP, UP],
            behind_the_users(),
        );
        assert!(said.contains("with Command-Tab"), "{said}");
        assert_eq!(put_backs(&fake), 0);
        // A Command shortcut in their own app (Command held, no Tab) as Safari takes the front
        // is no switch: Safari is put back, and Notes has the keyboard again.
        let (fake, said) = safari_comes_forward(
            &[UP, UP, COMMAND, COMMAND, COMMAND, COMMAND, COMMAND, UP],
            behind_the_users(),
        );
        assert!(
            said.contains(
                "Safari brought its window forward; it was put back behind the user's Notes window."
            ),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 1);
        assert_eq!(*fake.active.borrow(), Some(1));
        // A click in the Dock (on Safari's icon): theirs. The pointer isn't on a window of
        // Safari's, and the Dock says it's on it.
        let on_the_dock = |on_dock| Recording {
            on_dock,
            last_click: Some((Duration::ZERO, (800.0, 950.0))),
            ..behind_the_users()
        };
        let (fake, said) = safari_comes_forward(&[], on_the_dock(true));
        assert!(
            said.contains("The user clicked in the Dock and brought Safari forward themselves, so it was left in front: it's theirs to use now."),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 0);
        assert_eq!(*fake.active.borrow(), Some(42));
        // The same click anywhere else that isn't Safari's (in Trek's window, here) as Safari
        // takes the front: put back.
        let (fake, said) = safari_comes_forward(&[], on_the_dock(false));
        assert!(said.contains("it was put back behind"), "{said}");
        assert_eq!(put_backs(&fake), 1);
        // Typing in their own app (a key 80 ms ago, no Command, no click) as Safari takes
        // the front: put back, and Notes has the keyboard again.
        let (fake, said) = safari_comes_forward(
            &[UP],
            Recording {
                idle: Some(Duration::from_millis(80)),
                last_click: Some((Duration::from_secs(40), (300.0, 300.0))),
                ..behind_the_users()
            },
        );
        assert!(
            said.contains(
                "Safari brought its window forward; it was put back behind the user's Notes window."
            ),
            "{said}"
        );
        assert_eq!(put_backs(&fake), 1);
        assert_eq!(*fake.active.borrow(), Some(1));
    }

    #[test]
    fn short_times_as_a_reply_says_them() {
        assert_eq!(about(Duration::from_millis(3)), "under 10 ms");
        assert_eq!(about(Duration::from_millis(44)), "about 40 ms");
        assert_eq!(about(Duration::from_millis(236)), "about 240 ms");
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
        // Away for ten minutes, the screen locked, or the screen asleep: it's fine.
        for fake in [
            using_textedit(600),
            Recording {
                locked: true,
                ..using_textedit(1)
            },
            Recording {
                dark: true,
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
        // Nor in a window of its own: no cmd+n reaches the app they're typing in (the new window
        // would take their keys).
        let fake = Recording {
            jumps_forward: true,
            ..using_textedit(5)
        };
        fake.opens.borrow_mut().push(textedit_window(50, 12));
        let mut c = behind(&fake);
        assert_eq!(
            c.call("open_app", &json!({"name": "TextEdit", "new_window": true})),
            Err(format!(
                "{} TextEdit was already running (pid 50, /Applications/TextEdit.app).",
                in_use("TextEdit")
            ))
        );
        assert_eq!(fake.events(), vec![]);
        // Once they're away it gets one: cmd+n to the app alone (no new process), and the
        // window in front put back over the new one.
        let fake = Recording {
            jumps_forward: true,
            ..using_textedit(600)
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
    fn a_window_a_click_brings_forward_is_put_back_behind_the_users() {
        // A click in Safari behind raises a window of it over the user's Notes.
        let mut popup = textedit_window(42, 70);
        popup.owner = "Safari".into();
        let fake = Recording {
            raises: true,
            jumps_forward: true,
            idle: Some(Duration::from_secs(300)),
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(popup.clone());
        let mut c = behind(&fake);
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let said = reply(c.call("click", &json!({"x": 10, "y": 10})));
        assert!(
            said.contains(
                "Safari brought its window forward; it was put back behind the user's Notes window."
            ),
            "{said}"
        );
        assert_eq!(fake.events().last(), Some(&Event::BringForward(1)));
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        // While the user types in their own app, too (a key 80 ms ago): their typing is no wish
        // for Safari, whose window goes back and whose app gives the keyboard back.
        let fake = Recording {
            raises: true,
            jumps_forward: true,
            takes_active: true,
            idle: Some(Duration::from_millis(80)),
            ..behind_the_users()
        };
        *fake.active.borrow_mut() = Some(1);
        fake.opens.borrow_mut().push(popup.clone());
        let mut c = behind(&fake);
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let said = reply(c.call("click", &json!({"x": 10, "y": 10})));
        assert!(
            said.contains(
                "Safari brought its window forward; it was put back behind the user's Notes window."
            ),
            "{said}"
        );
        assert_eq!(
            *fake.active.borrow(),
            Some(1),
            "Notes has the keyboard again"
        );
        assert_eq!(c.desktop.windows().unwrap()[0].owner, "Notes");
        // The user's own doing is theirs: a window in front after their input since the agent's
        // last click (10 s ago) is left there.
        let fake = Recording {
            jumps_forward: true,
            idle: Some(Duration::from_secs(1)),
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(popup);
        *fake.opened.borrow_mut() = true;
        let mut c = behind(&fake);
        c.posted = Some((
            std::time::Instant::now() - Duration::from_secs(10),
            Some(Duration::from_secs(300)),
        ));
        let notes = c.desktop.windows().unwrap().into_iter().nth(1).unwrap();
        assert_eq!(notes.id, 1, "Notes was in front");
        c.front = Some((notes.clone(), 42));
        assert_eq!(c.put_back_in_front(), "");
        assert!(c.front.is_none(), "and it's theirs from then on");
        assert!(
            !fake
                .events()
                .iter()
                .any(|e| matches!(e, Event::BringForward(_)))
        );
        // Had they done nothing since, it would have gone back.
        let mut away = fake.clone();
        away.idle = Some(Duration::from_secs(400));
        c.desktop = Box::new(away.clone());
        c.front = Some((notes.clone(), 42));
        assert!(c.put_back_in_front().contains("put back"));
        assert_eq!(fake.events(), vec![Event::BringForward(1)]);
        // Only the agent's app is ever put back: another app's window that comes forward (the
        // user's app reordering its own, a notice) is left where it is, the user idle or not.
        let mut theirs = textedit_window(1, 80);
        theirs.owner = "Notes".into();
        let fake = Recording {
            jumps_forward: true,
            idle: Some(Duration::from_secs(400)),
            ..behind_the_users()
        };
        fake.opens.borrow_mut().push(theirs);
        *fake.opened.borrow_mut() = true;
        c.desktop = Box::new(fake.clone());
        // (The agent in Safari, or in Notes itself, where there's no one else's window to keep.)
        for agents in [42, 1] {
            c.front = Some((notes.clone(), agents));
            assert_eq!(c.put_back_in_front(), "");
        }
        assert_eq!(fake.events(), vec![]);
    }

    #[test]
    fn the_agents_own_input_isnt_taken_for_the_users() {
        // The user is away, TextEdit in front; macOS counts the agent's keys as input.
        let fake = Recording {
            posts_count: true,
            ..using_textedit(600)
        };
        let mut c = behind(&fake);
        reply(c.call("open_app", &json!({"name": "TextEdit"})));
        for _ in 0..3 {
            c.call("type_text", &json!({"text": "hi"})).unwrap();
        }
        assert!(c.user_idle().unwrap() >= Duration::from_secs(600));
        assert!(c.user_away(), "its own typing doesn't bring the user back");
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
            "Touchline 27 is the app the user is using right now (it's in front and they're at the Mac, if only reading); ask them whether to work in it (they switch to another app, then you go on), or wait and try again. Nothing was done."
        );
        // No way round it is offered: a second window of the same app is still theirs.
        assert!(!in_use("Safari").contains("new_window"));
        // The rule as the agent is told it: reading counts, and away is the screen or 5 minutes.
        assert!(BACKGROUND_INSTRUCTIONS.contains(
            "the app in front is theirs while they're at the Mac, if only reading, and acting there is refused until they switch."
        ));
        assert!(BACKGROUND_INSTRUCTIONS.contains("(screen locked or asleep, or 5 minutes idle)"));
        assert_eq!(AWAY_AFTER, Duration::from_secs(5 * 60));
        assert!(!BACKGROUND_INSTRUCTIONS.contains("30 seconds"));
        assert_eq!(
            waiting("Touchline 27"),
            "Waiting: you're using Touchline 27"
        );
        assert!(BACKGROUND_INSTRUCTIONS.contains("Work only in apps the user isn't using"));
    }

    #[test]
    fn reading_the_app_in_front_counts_as_using_it() {
        // TextEdit in front, the user reading it: no key, click or movement for a while. Half a
        // minute, four minutes, just under five: still theirs.
        for idle in [31, 240, 299] {
            let fake = using_textedit(idle);
            let mut c = behind(&fake);
            c.call("screenshot", &json!({"window_id": 11})).unwrap();
            assert_eq!(
                c.call("type_text", &json!({"text": "hi"})),
                Err(in_use("TextEdit")),
                "idle {idle} s"
            );
            assert!(
                !fake
                    .events()
                    .iter()
                    .any(|e| matches!(e, Event::Behind { .. })),
                "idle {idle} s: nothing reached their app"
            );
        }
        // Five minutes without input: they're away.
        let mut c = behind(&using_textedit(300));
        c.call("screenshot", &json!({"window_id": 11})).unwrap();
        c.call("type_text", &json!({"text": "hi"})).unwrap();
        // The screen saver, or the display asleep, says so at once.
        let fake = Recording {
            dark: true,
            ..using_textedit(2)
        };
        let mut c = behind(&fake);
        c.call("screenshot", &json!({"window_id": 11})).unwrap();
        c.call("click", &json!({"x": 5, "y": 5})).unwrap();
        // A platform that can't say how long they've been idle leaves the front app theirs.
        let mut fake = using_textedit(0);
        fake.idle = None;
        let mut c = behind(&fake);
        assert_eq!(
            c.call("open_app", &json!({"name": "TextEdit"})),
            Err(format!(
                "{} TextEdit was already running (pid 50, /Applications/TextEdit.app).",
                in_use("TextEdit")
            ))
        );
        // Another app, behind, is fine while they read.
        let mut c = behind(&using_textedit(240));
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        c.call("click", &json!({"x": 5, "y": 5})).unwrap();
        // The screenshot of their app says why acting there waits.
        let mut c = behind(&using_textedit(240));
        let said = reply(c.call("screenshot", &json!({"window_id": 11})));
        assert!(
            said.contains("(it's in front and they're at the Mac)")
                && said.contains("until they switch to another app or step away"),
            "{said}"
        );
    }

    /// Safari's window 7, on another Space now (a full-screen one with `full_screen`); the user's
    /// Notes and Trek on the one showing.
    fn on_another_space(full_screen: bool) -> Recording {
        let mut fake = behind_the_users();
        let mut windows = fake.windows.clone().unwrap();
        let safari = windows.remove(1);
        fake.windows = Ok(windows);
        fake.elsewhere = vec![(
            7,
            Whereabouts::OtherSpace {
                window: safari,
                full_screen,
            },
        )];
        fake
    }

    #[test]
    fn a_window_on_another_space_is_still_the_one_worked_in() {
        let fake = on_another_space(false);
        let mut c = behind(&fake);
        // It can be picked, and it's seen and worked in where it is.
        let said = reply(c.call("screenshot", &json!({"window_id": 7})));
        assert!(
            said.contains("Safari's window “Docs” (window_id 7)")
                && said.contains("The window is on another Space, out of the user's sight: this is how it was last drawn there.")
                && said.contains("so a screenshot can lag behind what you did"),
            "{said}"
        );
        assert!(!said.contains("closed"), "{said}");
        let clicked = reply(c.call("click", &json!({"x": 100, "y": 50})));
        assert!(
            clicked.ends_with(" Safari's window is on another Space, out of the user's sight."),
            "{clicked}"
        );
        let typed = reply(c.call("type_text", &json!({"text": "hi"})));
        assert!(typed.contains("on another Space"), "{typed}");
        assert_eq!(
            fake.events()[1..],
            [
                Event::Behind {
                    window: 7,
                    input: Input::Click {
                        x: 100.0,
                        y: 50.0,
                        button: Button::Left,
                        count: 1
                    }
                },
                Event::Behind {
                    window: 7,
                    input: Input::Type("hi".into())
                },
            ]
        );
        // It isn't in the on-screen list, and the list says where it is.
        let listed = reply(c.call("list_windows", &json!({})));
        assert!(
            listed.contains("The window you work in, Safari's window_id 7, is on another Space, so it isn't listed; it's still yours."),
            "{listed}"
        );
        // A full-screen Space says so.
        let mut c = behind(&on_another_space(true));
        let said = reply(c.call("screenshot", &json!({"window_id": 7})));
        assert!(
            said.contains(
                "The window is full screen, on a Space of its own, out of the user's sight"
            ),
            "{said}"
        );
        // It was on screen when picked, then the user moved it to another Space: still the one.
        let mut c = behind(&behind_the_users());
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        let moved = on_another_space(true);
        c.desktop = Box::new(moved.clone());
        let scrolled = reply(c.call("scroll", &json!({"x": 10, "y": 10, "dy": 2})));
        assert!(
            scrolled.contains("Safari's window is full screen, on a Space of its own"),
            "{scrolled}"
        );
        assert!(matches!(
            moved.events()[..],
            [Event::Behind { window: 7, .. }]
        ));
        // Back on the Space showing: nothing more is said.
        c.desktop = Box::new(behind_the_users());
        let clicked = reply(c.call("click", &json!({"x": 10, "y": 10})));
        assert!(clicked.ends_with("in the background."), "{clicked}");
    }

    #[test]
    fn a_window_on_another_space_that_cant_be_seen_says_so() {
        let fake = Recording {
            blind_off_space: true,
            ..on_another_space(true)
        };
        let mut c = behind(&fake);
        let err = c.call("screenshot", &json!({"window_id": 7})).unwrap_err();
        assert!(
            err.starts_with("Safari's window “Docs” is full screen, on a Space of its own and can't be seen from here")
                && err.contains("Ask the user to bring it to the desktop they're on (or out of full screen), or work in another window"),
            "{err}"
        );
        assert!(!err.contains("closed"), "{err}");
        // It's still the window: what's true is said again, not "pick a window".
        let err = c.call("screenshot", &json!({})).unwrap_err();
        assert!(err.contains("can't be seen from here"), "{err}");
        assert_eq!(c.target.as_ref().map(|t| t.window.id), Some(7));
        assert!(
            !fake
                .events()
                .iter()
                .any(|e| matches!(e, Event::Behind { .. }))
        );
    }

    #[test]
    fn a_window_just_opened_gets_a_moment_to_come_on_screen() {
        // TextEdit's new window, listed but not on screen for its first few lookups.
        let fake = behind_the_users();
        fake.opening.borrow_mut().push((textedit_window(50, 11), 3));
        let mut c = behind(&fake);
        let said = reply(c.call("screenshot", &json!({"window_id": 11})));
        assert!(
            said.contains("TextEdit's window “Untitled” (window_id 11)"),
            "{said}"
        );
        assert!(!said.contains("another Space"), "{said}");
        c.call("type_text", &json!({"text": "x"})).unwrap();
        // One that never shows within the grace is let go, said as it is.
        let fake = behind_the_users();
        fake.opening
            .borrow_mut()
            .push((textedit_window(50, 11), u32::MAX));
        let mut c = behind(&fake);
        let started = std::time::Instant::now();
        let err = c.call("screenshot", &json!({"window_id": 11})).unwrap_err();
        assert!(started.elapsed() >= OPENING, "it was given the grace");
        assert_eq!(
            err,
            "TextEdit's window “Untitled” isn't on screen any more: TextEdit closed it or put it away. Call list_windows and screenshot another window_id, or open_app."
        );
    }

    #[test]
    fn a_minimised_hidden_or_closed_window_is_let_go_and_said_as_it_is() {
        let safari = behind_the_users().windows.unwrap().remove(1);
        let cases = [
            (
                Whereabouts::Minimised(safari.clone()),
                "Safari's window “Docs” is minimised. Restoring it would put it in front of the user's windows, so ask them to restore it, or work in another window: call list_windows and screenshot another window_id, or open_app.",
            ),
            (
                Whereabouts::Hidden(safari.clone()),
                "Safari's window “Docs” is hidden, with the rest of Safari. Showing it would put Safari in front of the user's windows, so ask them to show it, or work in another window: call list_windows and screenshot another window_id, or open_app.",
            ),
            (
                Whereabouts::Closed,
                "Safari's window “Docs” has closed. Call list_windows and screenshot another window_id, or open_app.",
            ),
        ];
        for (place, said) in cases {
            let mut c = behind(&behind_the_users());
            c.call("screenshot", &json!({"window_id": 7})).unwrap();
            let mut gone = behind_the_users();
            let mut windows = gone.windows.clone().unwrap();
            windows.remove(1);
            gone.windows = Ok(windows);
            gone.elsewhere = vec![(7, place.clone())];
            c.desktop = Box::new(gone.clone());
            assert_eq!(c.call("click", &json!({"x": 1, "y": 1})), Err(said.into()));
            assert!(c.target.is_none(), "{place:?}: forgotten");
            assert_eq!(gone.events(), vec![]);
            // Picked by its id, the same is said.
            if place != Whereabouts::Closed {
                assert_eq!(
                    c.call("screenshot", &json!({"window_id": 7})),
                    Err(said.into())
                );
            }
        }
        let mut c = behind(&behind_the_users());
        assert_eq!(
            c.call("screenshot", &json!({"window_id": 99})),
            Err("No window 99 is open. list_windows shows the windows there are.".into())
        );
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

    /// A control as a platform reads it: something to press, known by `identity`.
    fn control(role: &str, name: &str, identity: u64, (x, y, w, h): (f64, f64, f64, f64)) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            identity: Some(identity),
            acts: true,
            actions: vec![Action::Press],
            frame: Some(Frame { x, y, w, h }),
            ..Node::default()
        }
    }

    /// What a window 800x600 points reads as (a form), and its app's menu bar.
    fn form() -> Vec<Node> {
        let mut name = control("text field", "Name", 1, (20.0, 40.0, 200.0, 22.0));
        name.actions = vec![Action::Confirm];
        name.value = Some("Ada".into());
        let mut agree = control("checkbox", "Agree", 2, (20.0, 80.0, 100.0, 18.0));
        agree.checked = Some(Checked::No);
        let save = control("button", "Save", 3, (700.0, 560.0, 80.0, 24.0));
        let mut publish = control("button", "Publish", 4, (600.0, 560.0, 80.0, 24.0));
        publish.disabled = true;
        let mut password = control("text field", "Password", 5, (20.0, 120.0, 200.0, 22.0));
        password.secure = true;
        password.actions = vec![];
        let far = control("button", "Far down", 6, (20.0, 900.0, 80.0, 24.0));
        let status = Node {
            role: "text".into(),
            name: "Saved 2 minutes ago".into(),
            frame: Some(Frame {
                x: 20.0,
                y: 570.0,
                w: 200.0,
                h: 16.0,
            }),
            ..Node::default()
        };
        let window = Node {
            role: "window".into(),
            name: "Docs".into(),
            frame: Some(Frame {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            }),
            children: vec![name, agree, save, publish, password, far, status],
            ..Node::default()
        };
        let mut item = control("menu item", "Save…", 10, (0.0, 0.0, 1.0, 1.0));
        item.frame = None;
        let menu = Node {
            role: "menu".into(),
            name: "File".into(),
            children: vec![item],
            ..Node::default()
        };
        let bar = Node {
            role: "menu bar".into(),
            children: vec![menu],
            ..Node::default()
        };
        vec![window, bar]
    }

    /// Safari's window behind the user's, reading as the form, picked and read once.
    fn reading_the_form() -> (Recording, Computer, String) {
        let fake = behind_the_users().with_elements(form());
        let mut c = behind(&fake);
        let read = reply(c.call("elements", &json!({"window_id": 7})));
        (fake, c, read)
    }

    /// What the desktop was asked to do to an element or post to a window.
    fn acted(fake: &Recording) -> Vec<Event> {
        fake.events()
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Behind { .. }
                        | Event::Perform { .. }
                        | Event::Focus { .. }
                        | Event::Click { .. }
                        | Event::Type(_)
                )
            })
            .collect()
    }

    #[test]
    fn elements_read_the_window_worked_in_as_an_outline() {
        let fake = behind_the_users().with_elements(form());
        let mut c = behind(&fake);
        // No window picked: there's nothing to read.
        assert_eq!(c.call("elements", &json!({})), Err(NO_WINDOW.to_string()));
        // Picked by its id, as a screenshot picks it: no screenshot is needed first.
        let read = reply(c.call("elements", &json!({"window_id": 7})));
        assert_eq!(
            read,
            "Elements of Safari's window “Docs” (window_id 7). frame=x,y,width,height in pixels of its screenshot (800x600).

- window \"Docs\" [frame=0,0,800,600]:
  - text field \"Name\" [ref=e1] [actions=confirm] [frame=20,40,200,22]: Ada
  - checkbox \"Agree\" [ref=e2] [unchecked] [frame=20,80,100,18]
  - button \"Save\" [ref=e3] [frame=700,560,80,24]
  - button \"Publish\" [ref=e4] [disabled] [frame=600,560,80,24]
  - text field \"Password\" [ref=e5] [secure] [frame=20,120,200,22]
  - button \"Far down\" [ref=e6] [offscreen] [frame=20,900,80,24]
  - text \"Saved 2 minutes ago\" [frame=20,570,200,16]"
        );
        // Reading posts nothing and asks no element to do anything.
        assert_eq!(acted(&fake), vec![]);
        // Narrowed, capped, and with the app's menus when asked.
        let read = reply(c.call("elements", &json!({"filter": "SAVE"})));
        assert!(
            read.ends_with(
                "\n\n- button \"Save\" [ref=e3] [frame=700,560,80,24]\n- text \"Saved 2 minutes ago\" [frame=20,570,200,16]"
            ),
            "{read}"
        );
        let read = reply(c.call("elements", &json!({"interactive_only": true, "max": 2})));
        assert!(
            read.contains("[ref=e2]") && !read.contains("[ref=e3]"),
            "{read}"
        );
        assert!(read.contains("… (4 more not shown"), "{read}");
        let read = reply(c.call("elements", &json!({"menu_bar": true, "filter": "menu"})));
        assert!(
            read.ends_with("- menu bar\n- menu \"File\"\n- menu item \"Save…\" [ref=e7]"),
            "{read}"
        );
        assert!(
            reply(c.call("elements", &json!({"filter": "nothing like it"})))
                .ends_with("(No element's role or name contains \"nothing like it\".)")
        );
        // A reading that stopped early says so, and why.
        let mut cut = behind_the_users().with_elements(form());
        cut.cut = Some("its app was slow to answer");
        c.desktop = Box::new(cut);
        assert!(reply(c.call("elements", &json!({}))).ends_with(
            "… (There's more: the reading stopped early because its app was slow to answer.)"
        ));
        // Trek's own windows aren't read.
        assert_eq!(
            c.call("elements", &json!({"window_id": 3})),
            Err(OWN_WINDOW_BEHIND.to_string())
        );
        // A window larger than a screenshot's pixels: frames are in the screenshot's.
        let mut big = behind_the_users().with_elements(form());
        if let Ok(ws) = &mut big.windows {
            (ws[1].w, ws[1].h) = (3136.0, 2000.0);
        }
        let mut c = behind(&big);
        let read = reply(c.call("elements", &json!({"window_id": 7})));
        assert!(
            read.contains("in pixels of its screenshot (1568x1000)"),
            "{read}"
        );
        assert!(
            read.contains("button \"Save\" [ref=e3] [frame=350,280,40,12]"),
            "{read}"
        );
    }

    #[test]
    fn elements_need_accessibility_and_a_platform_that_has_them() {
        let mut blind = behind_the_users().with_elements(form());
        blind.trusted = false;
        let mut c = behind(&blind);
        for (tool, args) in [
            ("elements", json!({"window_id": 7})),
            ("press", json!({"ref": "e1"})),
            ("wait", json!({"text": "Saved"})),
        ] {
            assert_eq!(
                c.call(tool, &args),
                Err(ACCESSIBILITY_ERROR.to_string()),
                "{tool}"
            );
        }
        // Where windows have no elements to read the tools aren't offered, and a ref is refused.
        let none = behind_the_users();
        let mut c = behind(&none);
        let names: Vec<&str> = c.tools().iter().map(|t| t.name).collect();
        assert!(!names.contains(&"elements") && !names.contains(&"press"));
        c.call("screenshot", &json!({"window_id": 7})).unwrap();
        assert_eq!(
            c.call("click", &json!({"ref": "e1"})),
            Err(desktop::NO_ELEMENTS.to_string())
        );
        // Where they have, the tools are there, and click, type_text and wait take a ref or a
        // condition in place of what they needed before.
        let c = behind(&behind_the_users().with_elements(form()));
        let tools = c.tools();
        let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap();
        assert_eq!(tool("press").input_schema["required"], json!(["ref"]));
        assert!(tool("elements").input_schema["properties"]["window_id"].is_object());
        for (name, takes) in [("click", "ref"), ("type_text", "ref"), ("wait", "text")] {
            assert!(
                tool(name).input_schema["properties"][takes].is_object(),
                "{name}"
            );
        }
        assert!(tool("click").input_schema.get("required").is_none());
        assert!(tool("wait").input_schema.get("required").is_none());
        // In front there's no window to pick.
        let c = on(&behind_the_users().with_elements(form()));
        let tools = c.tools();
        let elements = tools.iter().find(|t| t.name == "elements").unwrap();
        assert!(
            elements.input_schema["properties"]
                .get("window_id")
                .is_none()
        );
        // Both modes' instructions point at elements before screenshots.
        for said in [INSTRUCTIONS, BACKGROUND_INSTRUCTIONS] {
            if cfg!(target_os = "macos") {
                assert!(
                    said.contains("elements") && said.contains("refs (e12)"),
                    "{said}"
                );
            }
        }
    }

    #[test]
    fn press_asks_the_element_itself_and_sends_no_pointer_or_key() {
        let (fake, mut c, _) = reading_the_form();
        let said = reply(c.call("press", &json!({"ref": "e2"})));
        assert_eq!(
            said,
            "Pressed checkbox \"Agree\" by its own accessibility action: no pointer or key was sent. It's now checked."
        );
        // The element's own action and nothing else: no input was posted, nothing was focused.
        assert_eq!(
            acted(&fake),
            vec![Event::Perform {
                window: 7,
                identity: 2,
                action: Action::Press
            }]
        );
        // Another of its actions, when it has it.
        let said = reply(c.call("press", &json!({"ref": "[ref=e1]", "action": "confirm"})));
        assert!(
            said.starts_with("Confirmed text field \"Name\" by its own"),
            "{said}"
        );
        assert_eq!(
            acted(&fake).last(),
            Some(&Event::Perform {
                window: 7,
                identity: 1,
                action: Action::Confirm
            })
        );
        // A menu item, read with the menu bar: pressed without a menu opening.
        reply(c.call("elements", &json!({"menu_bar": true})));
        let said = reply(c.call("press", &json!({"ref": "e7"})));
        assert!(said.starts_with("Pressed menu item \"Save…\""), "{said}");
        // What can't be done is said, and nothing is done.
        let before = acted(&fake).len();
        let refused = |c: &mut Computer, args: Value| c.call("press", &args).unwrap_err();
        assert_eq!(
            refused(&mut c, json!({"ref": "e4"})),
            "button \"Publish\" is disabled, so nothing was done."
        );
        assert_eq!(
            refused(&mut c, json!({"ref": "e3", "action": "increment"})),
            "button \"Save\" has no increment action of its own (it has press). Nothing was done: click it instead (click with its ref)."
        );
        assert!(
            refused(&mut c, json!({"ref": "e5"}))
                .contains("has no press action of its own (it has none)")
        );
        assert!(
            refused(&mut c, json!({"ref": "e3", "action": "smash"})).contains("Unknown action")
        );
        assert!(refused(&mut c, json!({"ref": "e99"})).contains("No element with ref \"e99\""));
        assert!(refused(&mut c, json!({})).contains("`ref`"));
        assert_eq!(acted(&fake).len(), before);
    }

    #[test]
    fn a_ref_is_acted_on_only_once_its_element_is_found_again_and_proven_the_same() {
        let again =
            "Nothing was done. Read the elements again (elements) and use the ref it has now.";
        // Renamed, moved, replaced by its twin, or gone since it was read: refused, each for what
        // changed, and for every tool that takes a ref.
        let changes: [(&str, fn(&mut Vec<Node>)); 4] = [
            (
                "Where button \"Save\" was there's now button \"Saved\".",
                |roots| roots[0].children[2].name = "Saved".into(),
            ),
            ("button \"Save\" has moved since you read it.", |roots| {
                roots[0].children[2].frame.as_mut().unwrap().y -= 100.0
            }),
            (
                "Another button \"Save\" is where the one you read was (things have moved).",
                |roots| roots[0].children[2].identity = Some(33),
            ),
            (
                "button \"Save\" isn't where it was any more: what's there has changed.",
                |roots| roots[0].children.truncate(2),
            ),
        ];
        for (why, change) in changes {
            let (fake, mut c, _) = reading_the_form();
            fake.redraw(change);
            for (tool, args) in [
                ("press", json!({"ref": "e3"})),
                ("click", json!({"ref": "e3"})),
                ("type_text", json!({"ref": "e3", "text": "hi"})),
            ] {
                assert_eq!(c.call(tool, &args), Err(format!("{why} {again}")), "{tool}");
            }
            assert_eq!(acted(&fake), vec![], "{why}");
            // Read again, the ref names what's there now (or is gone with it).
            reply(c.call("elements", &json!({})));
            let pressed = c.call("press", &json!({"ref": "e3"}));
            match why {
                w if w.contains("Another") => {
                    assert!(pressed.unwrap_err().contains("Another button"))
                }
                w if w.contains("isn't where it was") => {
                    assert!(pressed.unwrap_err().contains("isn't where it was"))
                }
                _ => assert!(pressed.is_ok(), "{why}"),
            }
        }
        // A ref belongs to the window it was read from.
        let (fake, mut c, _) = reading_the_form();
        c.call("screenshot", &json!({"window_id": 1})).unwrap();
        let err = c.call("press", &json!({"ref": "e3"})).unwrap_err();
        assert!(
            err.starts_with("Ref e3 is from another window's elements; Notes's window is the one you work in now."),
            "{err}"
        );
        // …and that window's elements start afresh: no old ref names one of them.
        let read = reply(c.call("elements", &json!({})));
        assert!(
            read.contains("\"Name\" [ref=e7]") && !read.contains("[ref=e1]"),
            "{read}"
        );
        assert_eq!(acted(&fake), vec![]);
    }

    #[test]
    fn an_outcome_that_cant_be_known_is_said_so_and_never_tried_again() {
        let (mut fake, mut c, _) = reading_the_form();
        fake.performs = Ok(Performed::Unknown("its app didn't answer in time".into()));
        c.desktop = Box::new(fake.clone());
        // Not an error: the press may have happened.
        let said = reply(c.call("press", &json!({"ref": "e3"})));
        assert!(
            said.starts_with("Outcome unknown: button \"Save\" was asked to press by its own accessibility action, but its app didn't answer in time, so it may or may not have happened")
                && said.contains("It was not tried again"),
            "{said}"
        );
        assert_eq!(acted(&fake).len(), 1, "asked once");
        // A refusal by the app is an error, with nothing done.
        fake.performs = Err("It's gone from its window.".into());
        c.desktop = Box::new(fake.clone());
        assert_eq!(
            c.call("press", &json!({"ref": "e3"})),
            Err(
                "button \"Save\" wasn't acted on: It's gone from its window. Nothing was done."
                    .into()
            )
        );
        assert_eq!(acted(&fake).len(), 2);
    }

    #[test]
    fn click_by_ref_clicks_the_elements_centre_behind() {
        let (fake, mut c, _) = reading_the_form();
        assert_eq!(
            reply(c.call("click", &json!({"ref": "e3", "count": 2}))),
            "Double-clicked button \"Save\" at its centre (740, 572) in Safari's window, in the background."
        );
        // Through the same path as a click at x and y: to the window, in its own points.
        assert_eq!(
            acted(&fake),
            vec![Event::Behind {
                window: 7,
                input: Input::Click {
                    x: 740.0,
                    y: 572.0,
                    button: Button::Left,
                    count: 2
                }
            }]
        );
        let refused = |c: &mut Computer, args: Value| c.call("click", &args).unwrap_err();
        assert_eq!(
            refused(&mut c, json!({"ref": "e3", "x": 1, "y": 1})),
            "Give ref, or x and y, not both."
        );
        assert!(
            refused(&mut c, json!({"ref": "e3", "button": "middle"})).contains("Unknown button")
        );
        // Scrolled out of what the window shows: there's nowhere to click.
        assert!(refused(&mut c, json!({"ref": "e6"})).starts_with(
            "button \"Far down\" is outside what the window shows (scrolled out of view)"
        ));
        assert_eq!(acted(&fake).len(), 1);
        // In a window pictured smaller than it is, the reply's point is in the picture's pixels
        // and the click in the window's points.
        let mut big = behind_the_users().with_elements(form());
        if let Ok(ws) = &mut big.windows {
            (ws[1].w, ws[1].h) = (3136.0, 2000.0);
        }
        let mut c = behind(&big);
        reply(c.call("elements", &json!({"window_id": 7})));
        assert!(reply(c.call("click", &json!({"ref": "e3"}))).contains("at its centre (370, 286)"));
        assert_eq!(
            acted(&big),
            vec![Event::Behind {
                window: 7,
                input: Input::Click {
                    x: 740.0,
                    y: 572.0,
                    button: Button::Left,
                    count: 1
                }
            }]
        );
    }

    #[test]
    fn type_text_by_ref_types_only_once_the_field_has_the_focus() {
        let (fake, mut c, _) = reading_the_form();
        assert_eq!(
            reply(c.call("type_text", &json!({"ref": "e1", "text": " L."}))),
            "Typed 3 characters into text field \"Name\"; it now reads \"Ada L.\"."
        );
        // Asked for the focus through accessibility (no click), then typed to its window.
        assert_eq!(
            acted(&fake),
            vec![
                Event::Focus {
                    window: 7,
                    identity: 1
                },
                Event::Behind {
                    window: 7,
                    input: Input::Type(" L.".into())
                },
            ]
        );
        // It has the focus already: nothing more is asked.
        reply(c.call("type_text", &json!({"ref": "e1", "text": "!"})));
        assert_eq!(acted(&fake).len(), 3);
        // A secure field's value is never read back.
        assert_eq!(
            reply(c.call("type_text", &json!({"ref": "e5", "text": "hunter2"}))),
            "Typed 7 characters into text field \"Password\". It's a secure field, so what it holds isn't read."
        );
        assert!(
            c.call("type_text", &json!({"ref": "e1", "text": ""}))
                .is_err()
        );

        // A field that doesn't take the focus when asked is clicked, as a person would.
        let (mut fake, mut c, _) = reading_the_form();
        fake.focuses = false;
        fake.click_focuses = true;
        c.desktop = Box::new(fake.clone());
        let said = reply(c.call("type_text", &json!({"ref": "e1", "text": "x"})));
        assert_eq!(
            said,
            "Typed 1 characters into text field \"Name\" (it took the focus from a click in it, not when asked); it now reads \"Adax\"."
        );
        assert_eq!(
            acted(&fake)[1],
            Event::Behind {
                window: 7,
                input: Input::Click {
                    x: 120.0,
                    y: 51.0,
                    button: Button::Left,
                    count: 1
                }
            }
        );
        // One that never says it has the focus is typed nothing: the text could land elsewhere.
        let (mut fake, mut c, _) = reading_the_form();
        fake.focuses = false;
        c.desktop = Box::new(fake.clone());
        let err = c
            .call("type_text", &json!({"ref": "e1", "text": "x"}))
            .unwrap_err();
        assert!(
            err.starts_with("text field \"Name\" didn't take the keyboard focus")
                && err.contains("nothing was typed"),
            "{err}"
        );
        assert!(!acted(&fake).iter().any(|e| matches!(
            e,
            Event::Behind {
                input: Input::Type(_),
                ..
            }
        )));
        // Typed, but the field reads as before (an app that takes no keys posted behind): said,
        // and not typed again.
        let (mut fake, mut c, _) = reading_the_form();
        fake.deaf = true;
        c.desktop = Box::new(fake.clone());
        let said = reply(c.call("type_text", &json!({"ref": "e1", "text": "x"})));
        assert!(
            said.starts_with("Typed 1 characters into text field \"Name\", but it reads as it did before (\"Ada\"): the text may not have reached it. It wasn't typed again"),
            "{said}"
        );
        assert_eq!(
            acted(&fake)
                .iter()
                .filter(|e| matches!(
                    e,
                    Event::Behind {
                        input: Input::Type(_),
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn the_users_own_app_is_read_but_not_acted_in_by_ref() {
        // TextEdit in front, typed in 5 s ago, its window picked by id.
        let fake = using_textedit(5).with_elements(form());
        let mut c = behind(&fake);
        let read = reply(c.call("elements", &json!({"window_id": 11})));
        assert!(read.contains("button \"Save\" [ref=e3]"), "{read}");
        assert!(
            read.ends_with("TextEdit is the app the user is using right now (it's in front and they're at the Mac): acting there is refused until they switch to another app or step away (ask them, or wait)."),
            "{read}"
        );
        for (tool, args) in [
            ("press", json!({"ref": "e3"})),
            ("click", json!({"ref": "e3"})),
            ("type_text", json!({"ref": "e1", "text": "hi"})),
        ] {
            assert_eq!(c.call(tool, &args), Err(in_use("TextEdit")), "{tool}");
        }
        assert_eq!(acted(&fake), vec![], "nothing reached the user's app");
        // Once they're away, it's the agent's to work in.
        let away = Recording {
            idle: Some(Duration::from_secs(600)),
            ..fake.clone()
        };
        c.desktop = Box::new(away.clone());
        assert!(c.call("press", &json!({"ref": "e3"})).is_ok());
    }

    #[test]
    fn wait_answers_with_the_fresh_elements_when_text_or_an_element_shows_or_goes() {
        let (fake, mut c, _) = reading_the_form();
        // There already.
        let said = reply(c.call("wait", &json!({"text": "saved 2 MINUTES"})));
        assert!(said.starts_with("Found the text \"saved 2 MINUTES\" after 0.0 s.\n\nElements of Safari's window “Docs” (window_id 7)."), "{said}");
        assert!(
            said.contains("- button \"Save\" [ref=e3]"),
            "the outline, with the refs as they were: {said}"
        );
        // Shows a few readings later.
        *fake.shows_later.borrow_mut() = Some((
            3,
            control("sheet", "Save as", 20, (100.0, 100.0, 400.0, 300.0)),
        ));
        let said = reply(c.call("wait", &json!({"element": "sheet"})));
        assert!(
            said.starts_with("Found an element matching \"sheet\" after"),
            "{said}"
        );
        assert!(said.contains("- sheet \"Save as\" [ref=e7]"), "{said}");
        // Goes: here it never does, and the wait gives up, with what there is instead.
        let err = c
            .call(
                "wait",
                &json!({"element": "sheet", "gone": true, "timeout_ms": 20}),
            )
            .unwrap_err();
        assert!(
            err.starts_with("Waited 0 s, and an element matching \"sheet\" is still there.\n\nElements of Safari's window"),
            "{err}"
        );
        fake.redraw(|roots| roots[0].children.truncate(7));
        assert!(
            reply(c.call("wait", &json!({"element": "sheet", "gone": true})))
                .starts_with("Gone: an element matching \"sheet\" after")
        );
        let err = c
            .call("wait", &json!({"text": "Never", "timeout_ms": 0}))
            .unwrap_err();
        assert!(
            err.starts_with("Waited 0 s, and the text \"Never\" never showed."),
            "{err}"
        );
        // A time alone is still just a pause.
        assert_eq!(
            c.call("wait", &json!({"ms": 0})),
            Ok(vec![text("Waited 0 ms.")])
        );
        let refused = |c: &mut Computer, args: Value| c.call("wait", &args).unwrap_err();
        assert_eq!(
            refused(&mut c, json!({"text": "a", "element": "b"})),
            "Give text or element, not both."
        );
        assert!(
            refused(&mut c, json!({"text": "a", "timeout_ms": 90_000}))
                .contains("between 0 and 60000")
        );
        // An app too busy to answer has said nothing either way: it's asked again, and only
        // when the time is up is its silence the answer.
        *fake.busy.borrow_mut() = 3;
        assert!(reply(c.call("wait", &json!({"text": "Saved"}))).starts_with("Found the text"));
        *fake.busy.borrow_mut() = 1000;
        assert_eq!(
            c.call("wait", &json!({"text": "Saved", "timeout_ms": 5})),
            Err("Safari didn't answer when asked for its elements.".into())
        );
        *fake.busy.borrow_mut() = 0;
        // A reading that stopped early can show that something is there, never that it's gone.
        let mut cut = fake.clone();
        cut.cut = Some("its app was slow to answer");
        c.desktop = Box::new(cut);
        assert!(reply(c.call("wait", &json!({"text": "Saved"}))).starts_with("Found the text"));
        let err = c
            .call(
                "wait",
                &json!({"element": "sheet", "gone": true, "timeout_ms": 5}),
            )
            .unwrap_err();
        assert!(
            err.starts_with("Waited 0 s, and an element matching \"sheet\" wasn't in what could be read, but the window couldn't be read to its end, so whether it has gone isn't known."),
            "{err}"
        );
        assert_eq!(acted(&fake), vec![], "waiting only reads");
    }

    #[test]
    fn in_front_elements_are_of_the_window_in_front_in_full_screen_pixels() {
        // Safari in front at (400, 100); the screenshot is of the whole screen, a point a pixel.
        let mut fake = behind_the_users().with_elements(form());
        if let Ok(ws) = &mut fake.windows {
            ws.remove(0);
        }
        let mut c = on(&fake);
        let read = reply(c.call("elements", &json!({})));
        assert!(
            read.starts_with("Elements of Safari's window “Docs”, the one in front. frame=x,y,width,height in pixels of the full-screen screenshot."),
            "{read}"
        );
        assert!(
            read.contains("button \"Save\" [ref=e3] [frame=1100,660,80,24]"),
            "{read}"
        );
        // press is the element's own action; click by ref is the real pointer at its centre;
        // type_text by ref asks for the focus, then types with the real keyboard.
        reply(c.call("press", &json!({"ref": "e2"})));
        assert_eq!(
            reply(c.call("click", &json!({"ref": "e3"}))),
            "Clicked button \"Save\" at its centre (1140, 672)."
        );
        assert_eq!(
            reply(c.call("type_text", &json!({"ref": "e1", "text": "hi"}))),
            "Typed 2 characters into text field \"Name\", but it reads as it did before (\"Ada\"): the text may not have reached it. It wasn't typed again: look (elements, or a screenshot) before retrying."
        );
        assert_eq!(
            acted(&fake),
            vec![
                Event::Perform {
                    window: 7,
                    identity: 2,
                    action: Action::Press
                },
                Event::Click {
                    x: 1140.0,
                    y: 672.0,
                    button: Button::Left,
                    count: 1
                },
                Event::Focus {
                    window: 7,
                    identity: 1
                },
                Event::Type("hi".into()),
            ]
        );
        // Another window in front since: the refs aren't its.
        let mut other = fake.clone();
        if let Ok(ws) = &mut other.windows {
            ws.insert(0, window("Notes", 0.0, 0.0, 700.0, 900.0));
        }
        c.desktop = Box::new(other);
        assert!(
            c.call("press", &json!({"ref": "e3"}))
                .unwrap_err()
                .starts_with("Ref e3 is from another window's elements; Notes's window is the one in front now.")
        );
        // Trek in front: its elements aren't read.
        let trek = Recording {
            windows: Ok(vec![window("Trek", 0.0, 0.0, 800.0, 900.0)]),
            ..Recording::new().with_elements(form())
        };
        assert_eq!(
            on(&trek).call("elements", &json!({})),
            Err(OWN_WINDOW.to_string())
        );
        // A window over the element would take the click: refused.
        let mut covered = fake.clone();
        covered.key_window = Some(Ok(covered.windows.clone().unwrap().first().cloned()));
        if let Ok(ws) = &mut covered.windows {
            ws.insert(0, window("Notes", 1000.0, 600.0, 400.0, 300.0));
        }
        let mut c = on(&covered);
        reply(c.call("elements", &json!({})));
        assert_eq!(
            c.call("click", &json!({"ref": "e3"})),
            Err("Another window covers button \"Save\" now, so the click would land there. Nothing was done.".into())
        );
    }
}
