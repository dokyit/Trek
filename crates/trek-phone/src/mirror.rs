//! Taps and keys through macOS's iPhone Mirroring app, for when the Mac can't be the phone's
//! Bluetooth keyboard and mouse (a phone on the Mac's own Apple Account never takes it as one).
//! The helper drives the app's window in the background (clicks and drags in it become touches,
//! keys go to its process alone) and reads what it shows through Accessibility; this module is
//! what that means: the state the window is in, what to tell the user about it, and keys.
//!
//! iPhone Mirroring connects only while the iPhone is locked and nearby, after the user's
//! one-time setup on the Mac; while the phone is in use it says so ("Lock your iPhone…"), and
//! with "Ask Every Time" set it wants Touch ID or the Mac's password first. Its View menu's
//! Home Screen (⌘1), App Switcher (⌘2) and Spotlight (⌘3) are chosen through Accessibility,
//! so no key is pressed at all.

use serde_json::Value;

/// What the helper read off iPhone Mirroring (`mirror` answers).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    pub installed: bool,
    pub running: bool,
    /// Whether Trek may read other apps' windows (Accessibility); without it nothing shows.
    pub trusted: bool,
    /// Its main window's number and size in points, when it has one.
    pub window: Option<u32>,
    pub width: f64,
    pub height: f64,
    /// Whether Accessibility gave its window: when it doesn't (a busy Mac, for a moment), no
    /// texts says nothing.
    pub read: bool,
    /// The messages it shows ("Timed Out", "Lock your iPhone…"): none while mirroring.
    pub texts: Vec<String>,
    pub buttons: Vec<String>,
}

impl Look {
    pub fn parse(v: &Value) -> Self {
        let strings = |k: &str| -> Vec<String> {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        Look {
            installed: v["installed"].as_bool().unwrap_or(false),
            running: v["running"].as_bool().unwrap_or(false),
            trusted: v["trusted"].as_bool().unwrap_or(false),
            window: v["window"].as_u64().map(|n| n as u32),
            width: v["width"].as_f64().unwrap_or(0.),
            height: v["height"].as_f64().unwrap_or(0.),
            read: v["read"].as_bool().unwrap_or(true),
            texts: strings("texts"),
            buttons: strings("buttons"),
        }
    }

    /// The iPhone it's connecting to, as it names it ("Connecting to", "iPhone 16 Pro Max").
    pub fn device(&self) -> Option<String> {
        let at = self
            .texts
            .iter()
            .position(|t| t.trim() == "Connecting to")?;
        self.texts
            .get(at + 1)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// What it shows, as one line.
    pub fn says(&self) -> String {
        self.texts
            .iter()
            .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// No iPhone Mirroring on this Mac (it came with macOS 15).
    Missing,
    /// Trek can't read its window: Accessibility is off for Trek.
    NotAllowed,
    /// Not running, or no window yet.
    Closed,
    /// Showing the iPhone: taps and keys reach it.
    Live,
    /// Waiting for Touch ID or the Mac's password.
    MacLocked,
    /// The iPhone is unlocked and in use, so it won't connect.
    PhoneInUse,
    /// "Connecting to iPhone 16 Pro Max".
    Connecting,
    /// It gave up connecting ("timed out connecting", Connect offered): the iPhone wasn't locked,
    /// or wasn't near enough.
    TimedOut,
    /// Anything else it says (first-run setup, the phone out of reach), word for word.
    Says(String),
    /// Its window was there but Accessibility didn't give it this time.
    Unread,
}

impl State {
    /// What the window shows. Nothing to read is taken for the phone's screen, but iPhone
    /// Mirroring also shows nothing for a moment between its screens: `Settle` waits that out.
    pub fn of(look: &Look) -> State {
        if !look.installed {
            return State::Missing;
        }
        if !look.running || look.window.is_none() {
            return State::Closed;
        }
        if !look.trusted {
            return State::NotAllowed;
        }
        if !look.read {
            return State::Unread;
        }
        if look.texts.is_empty() {
            return State::Live;
        }
        let says = look.says();
        let has = |w: &str| says.to_lowercase().contains(&w.to_lowercase());
        if has("Touch ID") || has("Mac login") || has("Mirroring Is Locked") {
            State::MacLocked
        } else if has("Lock your iPhone") || has("iPhone use") || has("no longer in use") {
            State::PhoneInUse
        } else if has("Connecting to") {
            State::Connecting
        } else if has("timed out") {
            State::TimedOut
        } else {
            State::Says(says)
        }
    }

    /// Whether it shows that it's set up with an iPhone: one on this Mac's own Apple Account,
    /// the only kind it mirrors.
    pub fn set_up(&self) -> bool {
        matches!(
            self,
            State::Live
                | State::MacLocked
                | State::PhoneInUse
                | State::Connecting
                | State::TimedOut
        )
    }

    /// What the user has to do, for an agent to pass on, the action first; empty while live.
    pub fn advice(&self) -> String {
        match self {
            State::Missing => "Taps and keys can't reach this iPhone: they need iPhone Mirroring, which comes with macOS 15 or later.".into(),
            State::NotAllowed => "Ask the user to allow Trek in System Settings › Privacy & Security › Accessibility: Trek works iPhone Mirroring's window through it.".into(),
            State::Closed => "Trek opens iPhone Mirroring behind the user's windows when it needs it; the iPhone has to be locked and nearby.".into(),
            State::Live => String::new(),
            State::MacLocked => "Ask the user to unlock iPhone Mirroring with Touch ID or the Mac's password, in its window. To skip this next time: iPhone Mirroring › Settings › Authenticate Automatically.".into(),
            State::PhoneInUse | State::TimedOut => "Ask the user to lock the iPhone (side button) and leave it nearby, then try again: taps go through iPhone Mirroring, which connects only while the iPhone is locked. Screenshots still come over USB while it's unlocked.".into(),
            State::Connecting => "iPhone Mirroring is still connecting: try again in a few seconds.".into(),
            State::Says(s) => format!("Ask the user to check iPhone Mirroring; it says: \"{s}\"."),
            State::Unread => "iPhone Mirroring's window couldn't be read just now: try again in a moment.".into(),
        }
    }
}

impl State {
    /// Timing out with the iPhone locked: iPhone Mirroring can't find it on the network.
    pub fn unreachable_advice() -> &'static str {
        "iPhone Mirroring can't reach the iPhone although it's locked: ask the user to check that Wi-Fi and Bluetooth are on for both the iPhone and the Mac, on the same network, with the iPhone nearby, then try again."
    }
}

/// iPhone Mirroring's state as it settles: nothing to read is taken for the phone's screen only
/// once it lasts, since the window is also blank for a moment between its own screens (seen on
/// macOS 27.2: Timed Out, blank for ~1 s, then Connecting to…).
#[derive(Debug, Clone, Default)]
pub struct Settle {
    blank: Option<std::time::Instant>,
}

/// How long a blank window must last to count as the phone's screen.
pub const BLANK_SETTLES: std::time::Duration = std::time::Duration::from_millis(1500);

impl Settle {
    /// `seen` as read just now; Live only when it has been for `BLANK_SETTLES`, else `None`
    /// (keep what was known before).
    pub fn see(&mut self, seen: State, now: std::time::Instant) -> Option<State> {
        if seen == State::Unread {
            return None;
        }
        if seen != State::Live {
            self.blank = None;
            return Some(seen);
        }
        let since = *self.blank.get_or_insert(now);
        (now.duration_since(since) >= BLANK_SETTLES).then_some(State::Live)
    }
}

/// Trek's own Connect after iPhone Mirroring times out. Pressing again at once only times out
/// again the same way (seen ten times in a row on macOS 27.2), so: press again only once the
/// phone shows on the Mac's network (or when that can't be told), a bounded number of times, and
/// relaunch iPhone Mirroring once when it's stuck (connecting for ever, or a window that can't be
/// read).
#[derive(Debug, Clone, Default)]
pub struct Retry {
    pub presses: u32,
    pub relaunched: bool,
}

/// At most this many presses of Connect for one call, the first included.
pub const PRESSES: u32 = 3;
/// How long to wait for the phone to show on the network before giving up on it.
pub const PHONE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// Connecting for longer than this is stuck (an attempt gives up after about 10 s by itself).
pub const STUCK: std::time::Duration = std::time::Duration::from_secs(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Press,
    /// Wait for the phone to show on the network, then ask again.
    Wait,
    Relaunch,
    /// Done trying: say what's in the way.
    Stop,
}

impl Retry {
    /// It timed out (again); `seen`: whether the phone shows on the Mac's network now (`None`
    /// when that can't be told), `waited`: whether Trek already waited for it this round.
    pub fn timed_out(&mut self, seen: Option<bool>, waited: bool) -> Next {
        if self.presses >= PRESSES {
            return Next::Stop;
        }
        match seen {
            Some(false) if waited => Next::Stop,
            Some(false) => Next::Wait,
            _ => {
                self.presses += 1;
                Next::Press
            }
        }
    }

    /// It's stuck: relaunch it once, then stop.
    pub fn stuck(&mut self) -> Next {
        if std::mem::replace(&mut self.relaunched, true) {
            Next::Stop
        } else {
            Next::Relaunch
        }
    }
}

/// Quit iPhone Mirroring (for a relaunch: the helper's `mirror open` brings it back behind the
/// user's windows). A plain SIGTERM, which an app takes as Quit; no Apple Events, so no prompt.
pub fn quit() {
    let _ = std::process::Command::new("/usr/bin/pkill") // spawn: macOS only, and trek-phone has no trek-core
        .args(["-x", "iPhone Mirroring"])
        .status();
}

/// The View menu's shortcuts (chosen through Accessibility, not pressed).
pub const HOME: &str = "1";
pub const APP_SWITCHER: &str = "2";
pub const SPOTLIGHT: &str = "3";

/// CGEventFlags for the modifiers.
const CMD: u64 = 0x10_0000;
const SHIFT: u64 = 0x2_0000;
const ALT: u64 = 0x8_0000;
const CTRL: u64 = 0x4_0000;

/// A key's macOS virtual keycode on a US (ANSI) layout, and whether it needs Shift.
fn keycode(c: char) -> Option<(u16, bool)> {
    const LETTERS: &str = "asdfhgzxcv\0bqweryt";
    const ROW: [(char, u16); 21] = [
        ('1', 18),
        ('2', 19),
        ('3', 20),
        ('4', 21),
        ('6', 22),
        ('5', 23),
        ('=', 24),
        ('9', 25),
        ('7', 26),
        ('-', 27),
        ('8', 28),
        ('0', 29),
        (']', 30),
        ('[', 33),
        ('\'', 39),
        (';', 41),
        ('\\', 42),
        (',', 43),
        ('/', 44),
        ('.', 47),
        ('`', 50),
    ];
    const MORE: [(char, u16); 7] = [
        ('o', 31),
        ('u', 32),
        ('i', 34),
        ('p', 35),
        ('l', 37),
        ('j', 38),
        ('k', 40),
    ];
    const SHIFTED: &str = "!@#$%^&*()_+{}|:\"~<>?";
    const BASES: &str = "1234567890-=[]\\;'`,./";
    let lower = c.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        let code = LETTERS
            .find(lower)
            .map(|i| i as u16)
            .or_else(|| MORE.iter().find(|(k, _)| *k == lower).map(|(_, v)| *v))
            .or(match lower {
                'n' => Some(45),
                'm' => Some(46),
                _ => None,
            })?;
        return Some((code, c.is_ascii_uppercase()));
    }
    if let Some(i) = SHIFTED.find(c) {
        let base = BASES[i..].chars().next()?;
        return keycode(base).map(|(k, _)| (k, true));
    }
    ROW.iter().find(|(k, _)| *k == c).map(|(_, v)| (*v, false))
}

/// A named key or combination ("enter", "cmd+a", "shift+tab") as a keycode and CGEventFlags.
pub fn key(combo: &str) -> Result<(u16, u64), String> {
    let mut flags = 0u64;
    let mut main = None;
    for part in combo.split('+').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        let bit = match lower.as_str() {
            "cmd" | "command" | "meta" => Some(CMD),
            "shift" => Some(SHIFT),
            "alt" | "option" | "opt" => Some(ALT),
            "ctrl" | "control" => Some(CTRL),
            _ => None,
        };
        if let Some(bit) = bit {
            flags |= bit;
            continue;
        }
        if main.is_some() {
            return Err(format!("{combo}: one key besides the modifiers"));
        }
        let named = match lower.as_str() {
            "enter" | "return" => Some(36),
            "tab" => Some(48),
            "space" => Some(49),
            "backspace" | "delete" => Some(51),
            "escape" | "esc" => Some(53),
            "forwarddelete" => Some(117),
            "home" => Some(115),
            "end" => Some(119),
            "pageup" => Some(116),
            "pagedown" => Some(121),
            "left" | "arrowleft" => Some(123),
            "right" | "arrowright" => Some(124),
            "down" | "arrowdown" => Some(125),
            "up" | "arrowup" => Some(126),
            _ => None,
        };
        main = Some(match named {
            Some(code) => code,
            None => {
                let mut chars = part.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        let (code, shift) =
                            keycode(c).ok_or_else(|| format!("Unknown key {part:?}"))?;
                        if shift {
                            flags |= SHIFT;
                        }
                        code
                    }
                    _ => return Err(format!("Unknown key {part:?}")),
                }
            }
        });
    }
    let code = main.ok_or_else(|| format!("{combo}: which key?"))?;
    Ok((code, flags))
}

/// Text for the `mirror text` command: UTF-8 as hex, so spaces and newlines survive the line.
pub fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn look(texts: &[&str]) -> Look {
        Look::parse(&json!({
            "installed": true, "running": true, "trusted": true, "window": 3655,
            "width": 355.0, "height": 782.0, "texts": texts, "buttons": ["Connect"],
        }))
    }

    #[test]
    fn the_window_says_what_state_it_is_in() {
        // Seen on macOS 27.2 while the phone was unlocked, and before Touch ID.
        let in_use = look(&[
            "Timed Out",
            "iPhone Mirroring timed out due to iPhone use while connecting.\n\nLock your iPhone before connecting.",
        ]);
        assert_eq!(in_use.window, Some(3655));
        assert_eq!(in_use.buttons, ["Connect"]);
        assert_eq!(State::of(&in_use), State::PhoneInUse);
        assert!(
            State::PhoneInUse
                .advice()
                .starts_with("Ask the user to lock the iPhone")
        );
        let locked = look(&[
            "iPhone Mirroring Is Locked",
            "Touch ID or enter the Mac login for “Toby Dokyi” to continue.",
        ]);
        assert_eq!(State::of(&locked), State::MacLocked);
        assert!(
            State::MacLocked
                .advice()
                .contains("Authenticate Automatically")
        );
        assert_eq!(State::of(&look(&[])), State::Live);
        assert_eq!(State::Live.advice(), "");
        // Seen on macOS 27.2 with the phone on the cable, pressing Connect.
        let connecting = look(&["Connecting to", "iPhone 16 Pro Max", "iPhone 16 Pro Max"]);
        assert_eq!(State::of(&connecting), State::Connecting);
        assert_eq!(connecting.device().as_deref(), Some("iPhone 16 Pro Max"));
        assert_eq!(in_use.device(), None);
        let timed_out = look(&["Timed Out", "iPhone Mirroring timed out connecting."]);
        assert_eq!(State::of(&timed_out), State::TimedOut);
        assert!(
            State::TimedOut
                .advice()
                .starts_with("Ask the user to lock the iPhone")
        );
        for s in [
            State::Live,
            State::TimedOut,
            State::Connecting,
            State::MacLocked,
        ] {
            assert!(s.set_up(), "{s:?}");
        }
        for s in [
            State::Closed,
            State::Missing,
            State::NotAllowed,
            State::Says("Continue".into()),
        ] {
            assert!(!s.set_up(), "{s:?}");
        }
        let other = State::of(&look(&["Welcome to iPhone Mirroring"]));
        assert_eq!(other, State::Says("Welcome to iPhone Mirroring".into()));
    }

    #[test]
    fn a_blank_moment_between_screens_is_not_taken_for_live() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + std::time::Duration::from_millis(n);
        let mut settle = Settle::default();
        assert_eq!(settle.see(State::TimedOut, t0), Some(State::TimedOut));
        // Blank for a second after Connect is pressed, then "Connecting to…".
        assert_eq!(settle.see(State::Live, ms(500)), None);
        assert_eq!(settle.see(State::Live, ms(1200)), None);
        assert_eq!(
            settle.see(State::Connecting, ms(1800)),
            Some(State::Connecting)
        );
        // Blank and staying blank: the phone's screen.
        assert_eq!(settle.see(State::Live, ms(5000)), None);
        assert_eq!(settle.see(State::Live, ms(6600)), Some(State::Live));
        assert_eq!(settle.see(State::Live, ms(7000)), Some(State::Live));
    }

    #[test]
    fn nothing_to_read_is_never_taken_for_live() {
        // Seen live on a busy Mac: the window on screen showed Timed Out, but Accessibility gave
        // no window for that read.
        let mut unread = look(&[]);
        unread.read = false;
        assert_eq!(State::of(&unread), State::Unread);
        let mut settle = Settle::default();
        assert_eq!(settle.see(State::Unread, std::time::Instant::now()), None);
        let mut l = look(&[]);
        l.trusted = false;
        assert_eq!(State::of(&l), State::NotAllowed);
        l.window = None;
        assert_eq!(State::of(&l), State::Closed);
        assert_eq!(
            State::of(&Look::parse(&json!({"installed": true, "running": false}))),
            State::Closed
        );
        assert_eq!(State::of(&Look::default()), State::Missing);
    }

    #[test]
    fn connect_is_pressed_again_only_when_it_can_help() {
        let mut r = Retry::default();
        // The phone shows on the network (or that can't be told): press, a bounded number of times.
        assert_eq!(r.timed_out(Some(true), false), Next::Press);
        assert_eq!(r.timed_out(None, false), Next::Press);
        // Not on the network: wait for it once, then give up rather than press into nothing.
        assert_eq!(r.timed_out(Some(false), false), Next::Wait);
        assert_eq!(r.timed_out(Some(false), true), Next::Stop);
        assert_eq!(r.timed_out(Some(true), true), Next::Press);
        assert_eq!(r.presses, PRESSES);
        assert_eq!(r.timed_out(Some(true), false), Next::Stop);
        // Stuck: one relaunch, then stop.
        let mut r = Retry::default();
        assert_eq!(r.stuck(), Next::Relaunch);
        assert_eq!(r.stuck(), Next::Stop);
    }

    #[test]
    fn keys_are_us_keycodes_with_their_modifiers() {
        assert_eq!(key("enter").unwrap(), (36, 0));
        assert_eq!(key("a").unwrap(), (0, 0));
        assert_eq!(key("A").unwrap(), (0, SHIFT));
        assert_eq!(key("cmd+a").unwrap(), (0, CMD));
        assert_eq!(key("cmd+space").unwrap(), (49, CMD));
        assert_eq!(key("shift+tab").unwrap(), (48, SHIFT));
        assert_eq!(key("?").unwrap(), (44, SHIFT));
        assert_eq!(key("m").unwrap(), (46, 0));
        assert_eq!(key("k").unwrap(), (40, 0));
        assert_eq!(key("0").unwrap(), (29, 0));
        assert!(key("cmd+a+b").is_err() && key("cmd").is_err() && key("hyper").is_err());
    }

    #[test]
    fn text_travels_as_hex() {
        assert_eq!(hex("a b\n"), "6120620a");
        assert_eq!(hex("é"), "c3a9");
    }
}
