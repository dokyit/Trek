//! The platform under computer use: mouse and keyboard input, the window list, screen capture and
//! app launch. `computer` decides what to do (and refuses what it mustn't); a `Desktop` does it.
//! macOS and Windows each have one; tests use a recording fake that never reaches the real
//! mouse, keyboard or screen.

#[cfg(target_os = "macos")]
mod macos;
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
        fn windows(&self) -> Result<Vec<WindowInfo>, String> {
            self.windows.clone()
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
    }
}
