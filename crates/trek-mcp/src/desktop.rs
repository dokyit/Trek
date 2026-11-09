//! The platform under computer use: mouse and keyboard input, the window list, screen capture and
//! app launch. `computer` decides what to do (and refuses what it mustn't); a `Desktop` does it.
//! macOS and Windows each have one; tests use a recording fake that never reaches the real
//! mouse, keyboard or screen.

#[cfg(target_os = "macos")]
mod ax;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod skylight;
#[cfg(target_os = "macos")]
mod spaces;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::Mac as Native;
#[cfg(windows)]
pub use windows::Win as Native;

use crate::keys::Combo;
use crate::outline::{Action, Node};

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
    Click {
        x: f64,
        y: f64,
        button: Button,
        count: u32,
    },
    Drag {
        from: (f64, f64),
        to: (f64, f64),
    },
    Scroll {
        x: f64,
        y: f64,
        dx: i32,
        dy: i32,
    },
    Type(String),
    Key(Combo),
}

/// An app that's running, as the system lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApp {
    pub name: String,
    pub bundle_id: Option<String>,
    /// Where its bundle is (`/Applications/TextEdit.app`).
    pub path: Option<String>,
    pub pid: i64,
}

/// An app on disk that could be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundle {
    pub path: String,
    pub bundle_id: Option<String>,
}

/// Where a window is, looked up by its id (`Desktop::whereabouts`). Only macOS tells the cases
/// apart; elsewhere a window is on screen or closed.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Whereabouts {
    /// On screen, on a Space that's showing (however covered).
    OnScreen(WindowInfo),
    /// On a Space that isn't showing: another desktop, or (`full_screen`) an app's own
    /// full-screen Space. It's still open, where the user left it.
    OtherSpace {
        window: WindowInfo,
        full_screen: bool,
    },
    /// Minimised.
    Minimised(WindowInfo),
    /// Its app is hidden.
    Hidden(WindowInfo),
    /// The system still lists it but it isn't on screen, for no reason it gives: a window just
    /// opened and not shown yet, or one its app has closed but kept.
    NotShowing(WindowInfo),
    /// There's no such window any more.
    Closed,
}

/// How far `Desktop::elements` reads a window's accessibility tree: it's bounded, so a huge or
/// slow one costs a known amount.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Reading {
    /// The app's menu bar as well (its menus and their items), as a second root.
    pub menu_bar: bool,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub within: std::time::Duration,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            menu_bar: false,
            max_nodes: 3000,
            max_depth: 60,
            within: std::time::Duration::from_secs(5),
        }
    }
}

/// A window's elements as read: the window (place `[0]`), then its app's menu bar (place `[1]`)
/// when asked for. Frames are in the window's own coordinates.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Elements {
    pub roots: Vec<Node>,
    /// Why the reading stopped early (a bound of `Reading` was reached); `None` when it's whole.
    pub cut: Option<&'static str>,
}

/// How an element's own action went, as far as the app said.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Performed {
    /// The app took it.
    Done,
    /// The app didn't answer in time: it may have done it or not (an action that opens a dialog
    /// often ends this way). Why, for the reply.
    Unknown(String),
}

/// Said by a platform that can't read a window's elements.
pub const NO_ELEMENTS: &str =
    "Elements aren't available here: work from screenshots, with x and y.";

/// Said by a platform that can't work behind the user's windows.
pub const NOT_BEHIND: &str = "Background use isn't available here: computer use works in front, with the real pointer and keyboard.";

pub trait Desktop {
    /// Whether this process may post mouse and keyboard events (macOS: Accessibility).
    fn may_post_input(&self) -> bool;
    /// Ask the system to let it (macOS prompts for Accessibility, once per run).
    fn ask_to_post_input(&self) {}
    /// Added to a permission error when the system has set the host app's permissions aside
    /// (its bundle was replaced while it ran); empty otherwise.
    fn permissions_note(&self) -> String {
        String::new()
    }
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
    /// Where window `id` is, on screen or not. A platform that can't tell the cases apart says
    /// on screen or closed.
    fn whereabouts(&self, id: i64) -> Result<Whereabouts, String> {
        Ok(self
            .window(id)?
            .map_or(Whereabouts::Closed, Whereabouts::OnScreen))
    }
    /// `w` by itself, as it looks under whatever covers it, as a PNG whose longest side is at
    /// most `max_side`; or `region` of it (in its own coordinates).
    fn capture_window(
        &self,
        _w: &WindowInfo,
        _region: Option<Rect>,
        _max_side: u32,
    ) -> Result<Capture, String> {
        Err(NOT_BEHIND.into())
    }
    /// Deliver `input` to `w`'s app alone: the pointer stays where the user has it, and their
    /// front app keeps the keyboard. With no window (`w.id` 0) only keys can be delivered.
    fn behind(&self, _w: &WindowInfo, _input: &Input) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }
    /// Apps running now that the user could see (they have, or may have, windows).
    fn running_apps(&self) -> Vec<RunningApp> {
        vec![]
    }
    /// Copies of an app on disk, by name (`TextEdit`), bundle id (`com.apple.TextEdit`) or path,
    /// in no particular order: `computer` ranks them.
    fn installed_apps(&self, _name: &str) -> Vec<AppBundle> {
        vec![]
    }
    /// Launch the copy at `app.path`, without bringing it forward. Never by bare name: a name
    /// can resolve to another copy (a debug build, an old one) and start a second instance.
    fn launch_behind(&self, _app: &AppBundle) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }
    /// Ask a running app for a window ("reopen", as a click on its Dock icon does), without
    /// bringing it forward or starting another process.
    fn reopen_behind(&self, _app: &RunningApp) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }
    /// How long since the user last pressed a key, moved the pointer or clicked; `None` when the
    /// platform can't say.
    fn user_idle(&self) -> Option<std::time::Duration> {
        None
    }
    /// The screen is locked (or the login window is up).
    fn screen_locked(&self) -> bool {
        false
    }
    /// The display is asleep or the screen saver is on: nobody is reading the screen.
    fn screen_dark(&self) -> bool {
        false
    }
    /// Put window `w` back in front, its app active: after another app's window came forward
    /// over it uninvited. Whether it's in front afterwards is for the caller to check.
    fn bring_forward(&self, _w: &WindowInfo) -> Result<(), String> {
        Err(NOT_BEHIND.into())
    }

    // Elements: a window's accessibility tree, and its elements' own actions. Reading and acting
    // go to the window's app alone: nothing is activated or raised, no pointer or key is posted.

    /// Whether this platform can read a window's elements.
    fn has_elements(&self) -> bool {
        false
    }
    /// The accessibility tree of `w`, read as far as `reading` allows. An app that builds its
    /// tree only when asked (Chromium, Electron) is asked, here and nowhere else.
    fn elements(&self, _w: &WindowInfo, _reading: &Reading) -> Result<Elements, String> {
        Err(NO_ELEMENTS.into())
    }
    /// The element at `place` in `w`'s tree as it is now, without its children; `None` when
    /// nothing is there any more.
    fn element_at(&self, _w: &WindowInfo, _place: &[usize]) -> Result<Option<Node>, String> {
        Err(NO_ELEMENTS.into())
    }
    /// Ask the element read with `identity` to do `action` itself.
    fn perform(
        &self,
        _w: &WindowInfo,
        _identity: u64,
        _action: Action,
    ) -> Result<Performed, String> {
        Err(NO_ELEMENTS.into())
    }
    /// Give the element read with `identity` the keyboard focus within its own app (the app
    /// isn't activated). Whether it says it has it afterwards.
    fn focus(&self, _w: &WindowInfo, _identity: u64) -> Result<bool, String> {
        Err(NO_ELEMENTS.into())
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
        Capture {
            region: Option<Rect>,
            max_side: u32,
        },
        Move(f64, f64),
        Click {
            x: f64,
            y: f64,
            button: Button,
            count: u32,
        },
        Drag {
            from: (f64, f64),
            to: (f64, f64),
        },
        Scroll {
            x: f64,
            y: f64,
            dx: i32,
            dy: i32,
        },
        Type(String),
        Key(Combo),
        OpenApp(String),
        CaptureWindow {
            id: i64,
            region: Option<Rect>,
            max_side: u32,
        },
        Behind {
            window: i64,
            input: Input,
        },
        /// `launch_behind`, by the bundle's path.
        Launch(String),
        /// `reopen_behind`, by the running app's pid.
        Reopen(i64),
        /// `bring_forward`, by window id.
        BringForward(i64),
        /// `perform`: an element's own action, by its identity.
        Perform {
            window: i64,
            identity: u64,
            action: Action,
        },
        /// `focus`, by the element's identity.
        Focus {
            window: i64,
            identity: u64,
        },
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
        /// Windows that appear when an app is launched or reopened behind (none before that).
        pub opens: Rc<RefCell<Vec<WindowInfo>>>,
        /// `opens` have appeared.
        pub opened: Rc<RefCell<bool>>,
        /// Apps running now; a launch adds one, with the pid of the first of `opens`.
        pub running: Rc<RefCell<Vec<RunningApp>>>,
        /// Copies of apps on disk.
        pub installed: Vec<AppBundle>,
        /// Since the user's last input.
        pub idle: Option<std::time::Duration>,
        /// Input posted behind counts as input, as macOS's count sometimes takes it: `user_idle`
        /// is the time since the last post.
        pub posts_count: bool,
        pub last_post: Rc<RefCell<Option<std::time::Instant>>>,
        pub locked: bool,
        /// The windows that appear come in front of everything, until `bring_forward`.
        pub jumps_forward: bool,
        /// …and `bring_forward` doesn't move them back.
        pub stubborn: bool,
        /// A launched app shows its windows only once it's reopened (TextEdit, launched behind).
        pub shy: bool,
        /// A click behind makes `opens` appear (an app that raises a window when clicked in).
        pub raises: bool,
        /// Where windows that aren't in `windows` are (on another Space, minimised…); any other
        /// is closed.
        pub elsewhere: Vec<(i64, Whereabouts)>,
        /// Windows just opened: not showing for this many more lookups, then on screen.
        pub opening: Rc<RefCell<Vec<(WindowInfo, u32)>>>,
        /// A window on another Space can't be captured (`capture_window` fails for it).
        pub blind_off_space: bool,
        pub dark: bool,
        /// What every window's elements read as now: the window, then its app's menu bar.
        /// `None`: a platform with no elements.
        pub tree: Rc<RefCell<Option<Vec<Node>>>>,
        /// Why a reading of them stopped early.
        pub cut: Option<&'static str>,
        /// How an element's own action goes. Done, a checkbox pressed is checked or unchecked.
        pub performs: Result<Performed, String>,
        /// An element asked to take the focus takes it (and says so).
        pub focuses: bool,
        /// A click behind gives the focus to the element under it.
        pub click_focuses: bool,
        /// What's typed behind changes nothing (an app that takes no keys posted to it).
        pub deaf: bool,
        /// The next this-many readings fail, as an app too busy to answer makes them.
        pub busy: Rc<RefCell<u32>>,
        /// After this many more readings, the node joins the window's children (something that
        /// shows a moment later).
        pub shows_later: Rc<RefCell<Option<(u32, Node)>>>,
    }

    impl Recording {
        /// A trusted desktop with a 1512x982 display and nothing on it.
        pub fn new() -> Self {
            Self {
                events: Rc::default(),
                trusted: true,
                display: Display {
                    width: 1512.0,
                    height: 982.0,
                    physical: None,
                },
                windows: Ok(vec![]),
                owner_at: None,
                key_window: None,
                behind: true,
                opens: Rc::default(),
                opened: Rc::default(),
                running: Rc::default(),
                installed: vec![],
                idle: None,
                posts_count: false,
                last_post: Rc::default(),
                locked: false,
                jumps_forward: false,
                stubborn: false,
                shy: false,
                raises: false,
                elsewhere: vec![],
                opening: Rc::default(),
                blind_off_space: false,
                dark: false,
                tree: Rc::default(),
                cut: None,
                performs: Ok(Performed::Done),
                focuses: true,
                click_focuses: false,
                deaf: false,
                busy: Rc::default(),
                shows_later: Rc::default(),
            }
        }

        /// Every node of the windows' elements, to change what one of them says.
        fn each_node(&self, mut change: impl FnMut(&mut Node)) {
            fn walk(nodes: &mut [Node], change: &mut dyn FnMut(&mut Node)) {
                for n in nodes {
                    change(n);
                    walk(&mut n.children, change);
                }
            }
            if let Some(roots) = self.tree.borrow_mut().as_mut() {
                walk(roots, &mut change);
            }
        }

        /// A desktop whose windows all read as `roots` (numbered here by where they sit).
        pub fn with_elements(mut self, mut roots: Vec<Node>) -> Self {
            crate::outline::number(&mut roots);
            self.tree = Rc::new(RefCell::new(Some(roots)));
            self
        }

        /// Change what the windows read as (the app redrew), keeping each node where it sits.
        pub fn redraw(&self, change: impl FnOnce(&mut Vec<Node>)) {
            let mut tree = self.tree.borrow_mut();
            let roots = tree.get_or_insert_with(Vec::new);
            change(roots);
            crate::outline::number(roots);
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
            let (w, h) = region
                .map(|r| (r.2, r.3))
                .unwrap_or((self.display.width, self.display.height));
            let s = (max_side as f64 / w.max(h)).min(1.0);
            Ok(Capture {
                png_base64: "UE5H".into(),
                width: (w * s).round() as u32,
                height: (h * s).round() as u32,
                note: "",
            })
        }
        fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
            self.record(Event::Move(x, y))
        }
        fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
            self.record(Event::Click {
                x,
                y,
                button,
                count,
            })
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
        /// The windows, with those that appeared behind the first (or in front of them all, for
        /// an app that `jumps_forward` until a window is brought forward again).
        fn windows(&self) -> Result<Vec<WindowInfo>, String> {
            let mut all = self.windows.clone()?;
            if !*self.opened.borrow() {
                return Ok(all);
            }
            let opened = self.opens.borrow().clone();
            let put_back = !self.stubborn
                && self
                    .events()
                    .iter()
                    .any(|e| matches!(e, Event::BringForward(_)));
            let at = if self.jumps_forward && !put_back {
                0
            } else {
                all.len().min(1)
            };
            all.splice(at..at, opened);
            Ok(all)
        }
        /// The window's size fitted to `max_side` (or the region's), with no pixels; at twice
        /// its size, like a Retina capture, where that fits.
        fn capture_window(
            &self,
            w: &WindowInfo,
            region: Option<Rect>,
            max_side: u32,
        ) -> Result<Capture, String> {
            if !self.behind {
                return Err(NOT_BEHIND.into());
            }
            let off_space = |(id, place): &(i64, Whereabouts)| {
                *id == w.id && matches!(place, Whereabouts::OtherSpace { .. })
            };
            if self.blind_off_space && self.elsewhere.iter().any(off_space) {
                return Err("SLSHWCaptureWindowList returned no image".into());
            }
            self.record(Event::CaptureWindow {
                id: w.id,
                region,
                max_side,
            })?;
            let (rw, rh) = region.map(|r| (r.2, r.3)).unwrap_or((w.w, w.h));
            let s = (max_side as f64 / rw.max(rh)).min(2.0);
            Ok(Capture {
                png_base64: "UE5H".into(),
                width: (rw * s).round() as u32,
                height: (rh * s).round() as u32,
                note: "",
            })
        }
        /// Keys sent to an app (no window: its New Window shortcut, say) make `opens` appear, as
        /// does a click in an app that `raises`.
        fn behind(&self, w: &WindowInfo, input: &Input) -> Result<(), String> {
            if !self.behind {
                return Err(NOT_BEHIND.into());
            }
            if (w.id == 0 && matches!(input, Input::Key(_)))
                || (self.raises && matches!(input, Input::Click { .. }))
            {
                *self.opened.borrow_mut() = true;
            }
            // What's typed lands in the element with the focus; a click can give it.
            match input {
                Input::Type(typed) if !self.deaf => self.each_node(|n| {
                    if n.focused && n.acts {
                        n.value = Some(format!("{}{typed}", n.value.as_deref().unwrap_or("")));
                    }
                }),
                Input::Click { x, y, .. } if self.click_focuses => self.each_node(|n| {
                    n.focused = n.acts && n.frame.is_some_and(|f| f.holds((*x, *y)));
                }),
                _ => {}
            }
            *self.last_post.borrow_mut() = Some(std::time::Instant::now());
            self.record(Event::Behind {
                window: w.id,
                input: input.clone(),
            })
        }
        fn running_apps(&self) -> Vec<RunningApp> {
            self.running.borrow().clone()
        }
        fn installed_apps(&self, _name: &str) -> Vec<AppBundle> {
            self.installed.clone()
        }
        fn launch_behind(&self, app: &AppBundle) -> Result<(), String> {
            self.record(Event::Launch(app.path.clone()))?;
            *self.opened.borrow_mut() = !self.shy;
            let pid = self.opens.borrow().first().map_or(500, |w| w.pid);
            let name = app.path.rsplit('/').next().unwrap_or_default();
            self.running.borrow_mut().push(RunningApp {
                name: name.trim_end_matches(".app").to_string(),
                bundle_id: app.bundle_id.clone(),
                path: Some(app.path.clone()),
                pid,
            });
            Ok(())
        }
        fn reopen_behind(&self, app: &RunningApp) -> Result<(), String> {
            *self.opened.borrow_mut() = true;
            self.record(Event::Reopen(app.pid))
        }
        fn user_idle(&self) -> Option<std::time::Duration> {
            match *self.last_post.borrow() {
                Some(at) if self.posts_count => Some(at.elapsed()),
                _ => self.idle,
            }
        }
        fn screen_locked(&self) -> bool {
            self.locked
        }
        fn screen_dark(&self) -> bool {
            self.dark
        }
        /// On screen when `windows` lists it; else where `opening` or `elsewhere` say.
        fn whereabouts(&self, id: i64) -> Result<Whereabouts, String> {
            if let Some(w) = self.windows()?.into_iter().find(|w| w.id == id) {
                return Ok(Whereabouts::OnScreen(w));
            }
            if let Some((w, lookups)) = self
                .opening
                .borrow_mut()
                .iter_mut()
                .find(|(w, _)| w.id == id)
            {
                if *lookups == 0 {
                    return Ok(Whereabouts::OnScreen(w.clone()));
                }
                *lookups -= 1;
                return Ok(Whereabouts::NotShowing(w.clone()));
            }
            Ok(self
                .elsewhere
                .iter()
                .find(|(i, _)| *i == id)
                .map_or(Whereabouts::Closed, |(_, place)| place.clone()))
        }
        fn bring_forward(&self, w: &WindowInfo) -> Result<(), String> {
            self.record(Event::BringForward(w.id))
        }
        fn has_elements(&self) -> bool {
            self.tree.borrow().is_some()
        }
        fn elements(&self, w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
            if *self.busy.borrow() > 0 {
                *self.busy.borrow_mut() -= 1;
                return Err(format!(
                    "{} didn't answer when asked for its elements.",
                    w.owner
                ));
            }
            let mut later = self.shows_later.borrow_mut();
            if matches!(*later, Some((0, _))) {
                if let Some((_, node)) = later.take() {
                    self.redraw(|roots| roots[0].children.push(node));
                }
            } else if let Some((readings, _)) = later.as_mut() {
                *readings -= 1;
            }
            drop(later);
            let roots = self.tree.borrow().clone().ok_or(NO_ELEMENTS)?;
            let take = if reading.menu_bar { roots.len() } else { 1 };
            Ok(Elements {
                roots: roots.into_iter().take(take).collect(),
                cut: self.cut,
            })
        }
        fn element_at(&self, _w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
            let tree = self.tree.borrow();
            let roots = tree.as_ref().ok_or(NO_ELEMENTS)?;
            Ok(crate::outline::at(roots, place).map(|n| Node {
                children: vec![],
                ..n.clone()
            }))
        }
        fn perform(
            &self,
            w: &WindowInfo,
            identity: u64,
            action: Action,
        ) -> Result<Performed, String> {
            self.record(Event::Perform {
                window: w.id,
                identity,
                action,
            })?;
            if self.performs == Ok(Performed::Done) && action == Action::Press {
                self.each_node(|n| {
                    if n.identity == Some(identity) {
                        n.checked = n.checked.map(|c| match c {
                            crate::outline::Checked::Yes => crate::outline::Checked::No,
                            _ => crate::outline::Checked::Yes,
                        });
                    }
                });
            }
            self.performs.clone()
        }
        /// The element takes the focus from whichever had it, when `focuses`.
        fn focus(&self, w: &WindowInfo, identity: u64) -> Result<bool, String> {
            self.record(Event::Focus {
                window: w.id,
                identity,
            })?;
            if self.focuses {
                self.each_node(|n| n.focused = n.identity == Some(identity));
            }
            Ok(self.focuses)
        }
    }
}
