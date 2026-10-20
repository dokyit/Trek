//! The platform under computer use: mouse and keyboard input, the window list, screen capture and
//! app launch. `computer` decides what to do (and refuses what it mustn't); a `Desktop` does it.
//! macOS and Windows each have one; tests use a recording fake that never reaches the real
//! mouse, keyboard or screen.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod skylight;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::Mac as Native;
#[cfg(windows)]
pub use windows::Win as Native;

use crate::keys::Combo;

/// One window on screen. Bounds are in screen coordinates: points on macOS, pixels on Windows.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// The app that owns it: its name on macOS, its executable's name on Windows.
    pub owner: String,
    pub title: String,
    pub pid: i64,
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The main display, in the units the mouse moves in, and in physical pixels where those differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub width: f64,
    pub height: f64,
    pub physical: Option<(u64, u64)>,
}

/// A PNG of the screen or part of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub png_base64: String,
    pub width: u32,
    pub height: u32,
    /// Said after the screenshot's description (a missing permission, say); empty when all's well.
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// A rectangle in screen coordinates: x, y, width, height.
pub type Rect = (f64, f64, f64, f64);

/// Input for one window in the background (`Desktop::behind`). Points are in the window's own
/// coordinates: screen units, origin at its top-left corner.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Move(f64, f64),
    Click { x: f64, y: f64, button: Button, count: u32 },
    Drag { from: (f64, f64), to: (f64, f64) },
    Scroll { x: f64, y: f64, dx: i32, dy: i32 },
    Type(String),
    Key(Combo),
}

/// Said by a platform that can't work behind the user's windows.
pub const NOT_BEHIND: &str = "Background use isn't available here: computer use works in front, with the real pointer and keyboard.";

pub trait Desktop {
    /// Whether this process may post mouse and keyboard events (macOS: Accessibility).
    fn may_post_input(&self) -> bool;
    /// Ask the system to let it (macOS prompts for Accessibility, once per run).
    fn ask_to_post_input(&self) {}
    fn display(&self) -> Display;
    /// Windows on screen, front to back.
    fn windows(&self) -> Result<Vec<WindowInfo>, String>;
    /// The window keys would go to now.
    fn key_window(&self) -> Result<Option<WindowInfo>, String> {
        Ok(self.windows()?.into_iter().next())
    }
    /// The app owning the window the system itself would hand a click at (`x`, `y`) to, where the
    /// platform can say. Checked as well as the window list, never instead of it.
    fn owner_at(&self, _x: f64, _y: f64) -> Option<String> {
        None
    }
    /// The main display, or `region` of the screen, as a PNG whose longest side is at most
    /// `max_side` (never upscaled).
    fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String>;
    fn move_to(&self, x: f64, y: f64) -> Result<(), String>;
    /// Move there and click `count` times (2 = double-click, 3 = triple-click).
    fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String>;
    /// Press the left button at `from`, move smoothly to `to`, release.
    fn drag(&self, from: (f64, f64), to: (f64, f64)) -> Result<(), String>;
    /// Move there and scroll by lines; positive `dy` scrolls down, positive `dx` right.
    fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String>;
    /// Type text into the focused element; newlines press Return and tabs Tab.
    fn type_text(&self, text: &str) -> Result<(), String>;
    fn key(&self, combo: &Combo) -> Result<(), String>;
    /// Launch or bring forward an app by name (or a path).
    fn open_app(&self, name: &str) -> Result<(), String>;

    // Background use: one app's window, worked in behind the user's, with neither the pointer
    // nor the keyboard focus. Platforms that can't say `NOT_BEHIND`.

    /// Whether this platform can work behind the user's windows.
    fn works_behind(&self) -> bool {
        false
    }
    /// Window `id` as it is now: `None` when it has closed, or isn't on screen (minimised, or on
    /// another Space).
    fn window(&self, id: i64) -> Result<Option<WindowInfo>, String> {
        Ok(self.windows()?.into_iter().find(|w| w.id == id))
    }
    /// `w` by itself, as it looks under whatever covers it, as a PNG whose longest side is at
    /// most `max_side`; or `region` of it (in its own coordinates).
    fn capture_window(&self, _w: &WindowInfo, _region: Option<Rect>, _max_side: u32) -> Result<Capture, String> {
        Err(NOT_BEHIND.into())
    }
    /// Deliver `input` to `w`'s app alone: the pointer stays where the user has it, and their
    /// front app keeps the keyboard. With no window (`w.id` 0) only keys can be delivered.
    fn behind(&self, _w: &WindowInfo, _input: &Input) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }
    /// Launch an app, or open it if it's running, without bringing it forward.
    fn open_app_behind(&self, _name: &str) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }
    /// The process id of a running app, by name, when it has no window to find it by.
    fn app_pid(&self, _name: &str) -> Option<i64> {
        None
    }
}

#[cfg(test)]
pub mod fake {
    //! A `Desktop` that records what it's asked to do and does none of it.

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Capture { region: Option<Rect>, max_side: u32 },
        Move(f64, f64),
        Click { x: f64, y: f64, button: Button, count: u32 },
        Drag { from: (f64, f64), to: (f64, f64) },
        Scroll { x: f64, y: f64, dx: i32, dy: i32 },
        Type(String),
        Key(Combo),
        OpenApp(String),
        CaptureWindow { id: i64, region: Option<Rect>, max_side: u32 },
        Behind { window: i64, input: Input },
        OpenAppBehind(String),
    }

    #[derive(Clone)]
    pub struct Recording {
        pub events: Rc<RefCell<Vec<Event>>>,
        pub trusted: bool,
        pub display: Display,
        pub windows: Result<Vec<WindowInfo>, String>,
        /// What `owner_at` says for every point.
        pub owner_at: Option<String>,
        /// The window that takes the keys, where that isn't the front of the list (Windows: the
        /// foreground window, which the always-on-top taskbar and overlays don't change).
        pub key_window: Option<Result<Option<WindowInfo>, String>>,
        /// It can work behind the user's windows (macOS).
        pub behind: bool,
        /// Windows that open when an app is opened behind (`open_app_behind`).
        pub opens: Rc<RefCell<Vec<WindowInfo>>>,
        /// Running apps with no window, by name, and their process ids.
        pub pids: Vec<(String, i64)>,
    }

    impl Recording {
        /// A trusted desktop with a 1512x982 display and nothing on it.
        pub fn new() -> Self {
            Self {
                events: Rc::default(),
                trusted: true,
                display: Display { width: 1512.0, height: 982.0, physical: None },
                windows: Ok(vec![]),
                owner_at: None,
                key_window: None,
                behind: true,
                opens: Rc::default(),
                pids: vec![],
            }
        }

        pub fn events(&self) -> Vec<Event> {
            self.events.borrow().clone()
        }

        fn record(&self, e: Event) -> Result<(), String> {
            self.events.borrow_mut().push(e);
            Ok(())
        }
    }

    impl Desktop for Recording {
        fn may_post_input(&self) -> bool {
            self.trusted
        }
        fn display(&self) -> Display {
            self.display
        }
        fn key_window(&self) -> Result<Option<WindowInfo>, String> {
            match &self.key_window {
                Some(front) => front.clone(),
                None => Ok(self.windows()?.into_iter().next()),
            }
        }
        fn owner_at(&self, _x: f64, _y: f64) -> Option<String> {
            self.owner_at.clone()
        }
        /// A capture the display's size fitted to `max_side` (or the region's), with no pixels.
        fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
            self.record(Event::Capture { region, max_side })?;
            let (w, h) = region.map(|r| (r.2, r.3)).unwrap_or((self.display.width, self.display.height));
            let s = (max_side as f64 / w.max(h)).min(1.0);
            Ok(Capture { png_base64: "UE5H".into(), width: (w * s).round() as u32, height: (h * s).round() as u32, note: "" })
        }
        fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
            self.record(Event::Move(x, y))
        }
        fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
            self.record(Event::Click { x, y, button, count })
        }
        fn drag(&self, from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
            self.record(Event::Drag { from, to })
        }
        fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String> {
            self.record(Event::Scroll { x, y, dx, dy })
        }
        fn type_text(&self, text: &str) -> Result<(), String> {
            self.record(Event::Type(text.into()))
        }
        fn key(&self, combo: &Combo) -> Result<(), String> {
            self.record(Event::Key(combo.clone()))
        }
        fn open_app(&self, name: &str) -> Result<(), String> {
            self.record(Event::OpenApp(name.into()))
        }
        fn works_behind(&self) -> bool {
            self.behind
        }
        fn windows(&self) -> Result<Vec<WindowInfo>, String> {
            let mut all = self.windows.clone()?;
            all.extend(self.opens.borrow().iter().cloned());
            Ok(all)
        }
        /// The window's size fitted to `max_side` (or the region's), with no pixels; at twice
        /// its size, like a Retina capture, where that fits.
        fn capture_window(&self, w: &WindowInfo, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
            if !self.behind {
                return Err(NOT_BEHIND.into());
            }
            self.record(Event::CaptureWindow { id: w.id, region, max_side })?;
            let (rw, rh) = region.map(|r| (r.2, r.3)).unwrap_or((w.w, w.h));
            let s = (max_side as f64 / rw.max(rh)).min(2.0);
            Ok(Capture { png_base64: "UE5H".into(), width: (rw * s).round() as u32, height: (rh * s).round() as u32, note: "" })
        }
        fn behind(&self, w: &WindowInfo, input: &Input) -> Result<(), String> {
            if !self.behind {
                return Err(NOT_BEHIND.into());
            }
            self.record(Event::Behind { window: w.id, input: input.clone() })
        }
        fn open_app_behind(&self, name: &str) -> Result<(), String> {
            self.record(Event::OpenAppBehind(name.into()))
        }
        fn app_pid(&self, name: &str) -> Option<i64> {
            self.pids.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, p)| *p)
        }
    }
}
