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
            texts: strings("texts"),
            buttons: strings("buttons"),
        }
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
    /// Anything else it says (connecting, first-run setup, the phone out of reach), word for word.
    Says(String),
}

impl State {
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
        if look.texts.is_empty() {
            return State::Live;
        }
        let says = look.says();
        let has = |w: &str| says.to_lowercase().contains(&w.to_lowercase());
        if has("Touch ID") || has("Mac login") || has("Mirroring Is Locked") {
            State::MacLocked
        } else if has("Lock your iPhone") || has("iPhone use") || has("no longer in use") {
            State::PhoneInUse
        } else {
            State::Says(says)
        }
    }

    /// Whether it may still connect by itself, so it's worth waiting a little.
    pub fn settling(&self) -> bool {
        matches!(self, State::Closed | State::Says(_))
    }

    /// A few words for the panel's status chip.
    pub fn chip(&self) -> &'static str {
        match self {
            State::Missing => "iPhone Mirroring isn't on this Mac", // words: ok, the iPhone tool is macOS only
            State::NotAllowed => "Allow Accessibility for Trek",
            State::Closed => "iPhone Mirroring (opens when needed)",
            State::Live => "iPhone Mirroring",
            State::MacLocked => "Unlock iPhone Mirroring",
            State::PhoneInUse => "Lock the iPhone for Mirroring",
            State::Says(_) => "iPhone Mirroring isn't connected",
        }
    }

    /// What the user has to do, said to them in the panel; empty when there's nothing to add to
    /// its standing steps (lock the iPhone, set Mirroring up once).
    pub fn note(&self) -> String {
        match self {
            State::Missing => "iPhone Mirroring comes with macOS 15 or later.".into(),
            State::NotAllowed => "Allow Trek in System Settings › Privacy & Security › Accessibility, so it can work iPhone Mirroring's window.".into(),
            State::MacLocked => "iPhone Mirroring is waiting for Touch ID or your Mac password in its window.".into(),
            State::Says(s) => format!("iPhone Mirroring says: \"{s}\""),
            State::Closed | State::Live | State::PhoneInUse => String::new(),
        }
    }

    /// What the user has to do, for an agent to pass on; empty while live.
    pub fn advice(&self) -> String {
        match self {
            State::Missing => "Taps and keys need iPhone Mirroring, which comes with macOS 15 or later.".into(),
            State::NotAllowed => "Trek needs Accessibility to work iPhone Mirroring's window: System Settings › Privacy & Security › Accessibility › Trek.".into(),
            State::Closed => "Trek opens iPhone Mirroring behind the user's windows when it needs it; the iPhone has to be locked and nearby.".into(),
            State::Live => String::new(),
            State::MacLocked => "iPhone Mirroring is waiting for Touch ID or the Mac's password: ask the user to unlock it in its window. To skip this next time, they can choose Authenticate Automatically in iPhone Mirroring › Settings.".into(),
            State::PhoneInUse => "iPhone Mirroring connects only while the iPhone is locked: ask the user to lock it (side button) and leave it nearby, then try again. While it's unlocked, Trek shows its screen over USB but can't tap.".into(),
            State::Says(s) => format!("iPhone Mirroring isn't showing the iPhone; it says: \"{s}\". It needs the iPhone locked and nearby, and its one-time setup done on the Mac."),
        }
    }
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
        assert!(State::PhoneInUse.advice().contains("lock it"));
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
        let other = State::of(&look(&["Connecting to iPhone…"]));
        assert_eq!(other, State::Says("Connecting to iPhone…".into()));
        assert!(other.settling());
    }

    #[test]
    fn nothing_to_read_is_never_taken_for_live() {
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
