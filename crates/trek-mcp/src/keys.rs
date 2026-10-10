//! Key-combo parsing ("cmd+shift+t", "return", "ctrl+left") into a macOS virtual keycode (ANSI
//! layout) plus CGEventFlags bits, or a Windows virtual-key code (US layout) plus the modifier
//! keys to hold. One table names every key with both codes, so the platforms accept the same
//! names. Pure — no FFI.
//!
//! Both platforms' codes are built (and tested) everywhere; each uses its own.
#![cfg_attr(not(test), allow(dead_code))]

pub const FLAG_SHIFT: u64 = 0x0002_0000; // kCGEventFlagMaskShift
pub const FLAG_CONTROL: u64 = 0x0004_0000; // kCGEventFlagMaskControl
pub const FLAG_ALTERNATE: u64 = 0x0008_0000; // kCGEventFlagMaskAlternate
pub const FLAG_COMMAND: u64 = 0x0010_0000; // kCGEventFlagMaskCommand
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000; // kCGEventFlagMaskSecondaryFn

pub const KEY_RETURN: u16 = 36;
pub const KEY_TAB: u16 = 48;

pub const VK_RETURN: u16 = 0x0D;
pub const VK_TAB: u16 = 0x09;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12; // Alt
pub const VK_LWIN: u16 = 0x5B;
const VK_OEM_PLUS: u16 = 0xBB; // the =/+ key

/// Every key name the `key` tool accepts (lowercase), with its macOS keycode and Windows
/// virtual-key code.
const KEYS: &[(&str, u16, u16)] = &[
    // letters (Windows: 'A'..'Z')
    ("a", 0, 0x41),
    ("b", 11, 0x42),
    ("c", 8, 0x43),
    ("d", 2, 0x44),
    ("e", 14, 0x45),
    ("f", 3, 0x46),
    ("g", 5, 0x47),
    ("h", 4, 0x48),
    ("i", 34, 0x49),
    ("j", 38, 0x4A),
    ("k", 40, 0x4B),
    ("l", 37, 0x4C),
    ("m", 46, 0x4D),
    ("n", 45, 0x4E),
    ("o", 31, 0x4F),
    ("p", 35, 0x50),
    ("q", 12, 0x51),
    ("r", 15, 0x52),
    ("s", 1, 0x53),
    ("t", 17, 0x54),
    ("u", 32, 0x55),
    ("v", 9, 0x56),
    ("w", 13, 0x57),
    ("x", 7, 0x58),
    ("y", 16, 0x59),
    ("z", 6, 0x5A),
    // digits (Windows: '0'..'9')
    ("0", 29, 0x30),
    ("1", 18, 0x31),
    ("2", 19, 0x32),
    ("3", 20, 0x33),
    ("4", 21, 0x34),
    ("5", 23, 0x35),
    ("6", 22, 0x36),
    ("7", 26, 0x37),
    ("8", 28, 0x38),
    ("9", 25, 0x39),
    // punctuation (Windows: the VK_OEM_* keys)
    ("=", 24, VK_OEM_PLUS),
    ("equal", 24, VK_OEM_PLUS),
    ("equals", 24, VK_OEM_PLUS),
    ("-", 27, 0xBD),
    ("minus", 27, 0xBD),
    ("]", 30, 0xDD),
    ("rightbracket", 30, 0xDD),
    ("[", 33, 0xDB),
    ("leftbracket", 33, 0xDB),
    ("'", 39, 0xDE),
    ("quote", 39, 0xDE),
    (";", 41, 0xBA),
    ("semicolon", 41, 0xBA),
    ("\\", 42, 0xDC),
    ("backslash", 42, 0xDC),
    (",", 43, 0xBC),
    ("comma", 43, 0xBC),
    ("/", 44, 0xBF),
    ("slash", 44, 0xBF),
    (".", 47, 0xBE),
    ("period", 47, 0xBE),
    ("`", 50, 0xC0),
    ("grave", 50, 0xC0),
    ("backtick", 50, 0xC0),
    // named keys
    ("return", KEY_RETURN, VK_RETURN),
    ("enter", KEY_RETURN, VK_RETURN),
    ("ret", KEY_RETURN, VK_RETURN),
    ("tab", KEY_TAB, VK_TAB),
    ("space", 49, 0x20),
    ("spacebar", 49, 0x20),
    // The Mac's Delete key is a PC's Backspace; forward delete is the PC's Delete.
    ("delete", 51, 0x08),
    ("backspace", 51, 0x08),
    ("bksp", 51, 0x08),
    ("escape", 53, 0x1B),
    ("esc", 53, 0x1B),
    ("forwarddelete", 117, 0x2E),
    ("fwddelete", 117, 0x2E),
    ("del", 117, 0x2E),
    ("home", 115, 0x24),
    ("end", 119, 0x23),
    ("pageup", 116, 0x21),
    ("pgup", 116, 0x21),
    ("pagedown", 121, 0x22),
    ("pgdn", 121, 0x22),
    ("left", 123, 0x25),
    ("arrowleft", 123, 0x25),
    ("leftarrow", 123, 0x25),
    ("right", 124, 0x27),
    ("arrowright", 124, 0x27),
    ("rightarrow", 124, 0x27),
    ("down", 125, 0x28),
    ("arrowdown", 125, 0x28),
    ("downarrow", 125, 0x28),
    ("up", 126, 0x26),
    ("arrowup", 126, 0x26),
    ("uparrow", 126, 0x26),
    ("capslock", 57, 0x14),
    // function keys (Windows: VK_F1..VK_F12)
    ("f1", 122, 0x70),
    ("f2", 120, 0x71),
    ("f3", 99, 0x72),
    ("f4", 118, 0x73),
    ("f5", 96, 0x74),
    ("f6", 97, 0x75),
    ("f7", 98, 0x76),
    ("f8", 100, 0x77),
    ("f9", 101, 0x78),
    ("f10", 109, 0x79),
    ("f11", 103, 0x7A),
    ("f12", 111, 0x7B),
];

/// A modifier as written in a combo. `Cmd` and `Super` are one key on the Mac; on Windows `cmd`
/// means Ctrl (what a Mac shortcut is there: cmd+c copies) and `win`/`super`/`meta` the
/// Windows key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Cmd,
    Super,
    Shift,
    Alt,
    Ctrl,
    Fn,
}

fn modifier(token: &str) -> Option<Modifier> {
    Some(match token {
        "cmd" | "command" | "⌘" => Modifier::Cmd,
        "meta" | "super" | "win" => Modifier::Super,
        "shift" | "⇧" => Modifier::Shift,
        "alt" | "option" | "opt" | "⌥" => Modifier::Alt,
        "ctrl" | "control" | "ctl" | "⌃" => Modifier::Ctrl,
        "fn" => Modifier::Fn,
        _ => return None,
    })
}

/// A combo before it becomes one platform's codes: the key's row and the modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// (name, macOS keycode, Windows virtual-key code)
    pub key: (&'static str, u16, u16),
    pub mods: Vec<Modifier>,
}

impl Chord {
    pub fn has(&self, m: Modifier) -> bool {
        self.mods.contains(&m)
    }
}

/// macOS virtual keycode for a (lowercased) key name, ANSI layout.
pub fn keycode(name: &str) -> Option<u16> {
    KEYS.iter().find(|k| k.0 == name).map(|k| k.1)
}

/// Windows virtual-key code for a (lowercased) key name, US layout.
pub fn vk(name: &str) -> Option<u16> {
    KEYS.iter().find(|k| k.0 == name).map(|k| k.2)
}

/// Parse "cmd+shift+t", "Return", "ctrl+alt+delete", "cmd+plus", "cmd++".
pub fn parse_chord(combo: &str) -> Result<Chord, String> {
    let lowered = combo.trim().to_lowercase();
    if lowered.is_empty() {
        return Err("Empty key combo".into());
    }
    // A trailing "++" means the key itself is '+', i.e. shift+= on ANSI.
    let (body, plus_key) = match lowered.strip_suffix("++") {
        Some(rest) => (rest.to_string(), true),
        None if lowered == "+" => (String::new(), true),
        None => (lowered.clone(), false),
    };
    let mut tokens: Vec<&str> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('+').map(str::trim).collect()
    };
    if plus_key {
        tokens.push("plus");
    }
    if tokens.iter().any(|t| t.is_empty()) {
        return Err(format!("Malformed key combo {combo:?}"));
    }
    let (key, mod_tokens) = tokens.split_last().expect("at least one token");
    let mut mods = Vec::new();
    for m in mod_tokens {
        let m = modifier(m).ok_or_else(|| {
            format!("Unknown modifier {m:?} in {combo:?} (use {})", if cfg!(windows) { "ctrl, shift, alt, win" } else { "cmd, shift, alt/option, ctrl, fn" })
        })?;
        if !mods.contains(&m) {
            mods.push(m);
        }
    }
    let key = match *key {
        "plus" | "+" => {
            if !mods.contains(&Modifier::Shift) {
                mods.push(Modifier::Shift);
            }
            *KEYS.iter().find(|k| k.0 == "=").expect("= is in the table")
        }
        k => match KEYS.iter().find(|row| row.0 == k) {
            Some(row) => *row,
            None if modifier(k).is_some() => {
                return Err(format!("Key combo {combo:?} has only modifiers; add a key, e.g. \"{}\"", if cfg!(windows) { "ctrl+c" } else { "cmd+c" }));
            }
            None => {
                return Err(format!(
                    "Unknown key {k:?} in {combo:?}. Use a letter/digit/punctuation or one of: return, tab, space, delete, forwarddelete, escape, up, down, left, right, home, end, pageup, pagedown, f1-f12"
                ));
            }
        },
    };
    Ok(Chord { key, mods })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub keycode: u16,
    pub flags: u64,
}

/// A combo as macOS posts it: a keycode with CGEventFlags.
pub fn parse_combo(combo: &str) -> Result<KeyCombo, String> {
    let chord = parse_chord(combo)?;
    let flags = chord
        .mods
        .iter()
        .map(|m| match m {
            Modifier::Cmd | Modifier::Super => FLAG_COMMAND,
            Modifier::Shift => FLAG_SHIFT,
            Modifier::Alt => FLAG_ALTERNATE,
            Modifier::Ctrl => FLAG_CONTROL,
            Modifier::Fn => FLAG_SECONDARY_FN,
        })
        .fold(0, |a, b| a | b);
    Ok(KeyCombo { keycode: chord.key.1, flags })
}

/// A combo as Windows sends it: the modifier keys to hold (in the order they go down) and the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VkCombo {
    pub held: Vec<u16>,
    pub vk: u16,
}

pub fn parse_vk(combo: &str) -> Result<VkCombo, String> {
    let chord = parse_chord(combo)?;
    if chord.has(Modifier::Fn) {
        return Err(format!("{combo:?} needs fn, which only the keyboard itself can press on Windows. Name the key fn would give instead (e.g. f5, delete, pageup)."));
    }
    // Ctrl, Alt, Shift, then Win: the order a person presses them, and Ctrl once even when
    // both cmd and ctrl are named.
    let mut held = Vec::new();
    for (wanted, vk) in [
        (chord.has(Modifier::Ctrl) || chord.has(Modifier::Cmd), VK_CONTROL),
        (chord.has(Modifier::Alt), VK_MENU),
        (chord.has(Modifier::Shift), VK_SHIFT),
        (chord.has(Modifier::Super), VK_LWIN),
    ] {
        if wanted {
            held.push(vk);
        }
    }
    Ok(VkCombo { held, vk: chord.key.2 })
}

/// The combo this platform's `Desktop` presses.
#[cfg(target_os = "macos")]
pub type Combo = KeyCombo;
#[cfg(windows)]
pub type Combo = VkCombo;

#[cfg(target_os = "macos")]
pub fn parse(combo: &str) -> Result<Combo, String> {
    parse_combo(combo)
}
#[cfg(windows)]
pub fn parse(combo: &str) -> Result<Combo, String> {
    parse_vk(combo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_named_keys() {
        assert_eq!(parse_combo("return").unwrap(), KeyCombo { keycode: 36, flags: 0 });
        assert_eq!(parse_combo("Enter").unwrap().keycode, 36);
        assert_eq!(parse_combo("tab").unwrap().keycode, 48);
        assert_eq!(parse_combo("space").unwrap().keycode, 49);
        assert_eq!(parse_combo("delete").unwrap().keycode, 51);
        assert_eq!(parse_combo("escape").unwrap().keycode, 53);
        assert_eq!(parse_combo("esc").unwrap().keycode, 53);
        assert_eq!(parse_combo("left").unwrap().keycode, 123);
        assert_eq!(parse_combo("right").unwrap().keycode, 124);
        assert_eq!(parse_combo("down").unwrap().keycode, 125);
        assert_eq!(parse_combo("up").unwrap().keycode, 126);
        assert_eq!(parse_combo("f1").unwrap().keycode, 122);
        assert_eq!(parse_combo("F12").unwrap().keycode, 111);
        assert_eq!(parse_combo("home").unwrap().keycode, 115);
        assert_eq!(parse_combo("end").unwrap().keycode, 119);
        assert_eq!(parse_combo("pageup").unwrap().keycode, 116);
        assert_eq!(parse_combo("pagedown").unwrap().keycode, 121);
    }

    #[test]
    fn letters_and_digits() {
        assert_eq!(parse_combo("a").unwrap().keycode, 0);
        assert_eq!(parse_combo("t").unwrap().keycode, 17);
        assert_eq!(parse_combo("0").unwrap().keycode, 29);
        assert_eq!(parse_combo("9").unwrap().keycode, 25);
        // every letter maps, and to a distinct code, on both platforms; on Windows a letter's
        // code is its capital and a digit's is itself
        let mut seen = std::collections::HashSet::new();
        let mut seen_vk = std::collections::HashSet::new();
        for c in ('a'..='z').chain('0'..='9') {
            assert!(seen.insert(keycode(&c.to_string()).unwrap()), "dup for {c}");
            assert!(seen_vk.insert(vk(&c.to_string()).unwrap()), "dup vk for {c}");
            assert_eq!(vk(&c.to_string()), Some(c.to_ascii_uppercase() as u16), "{c}");
        }
    }

    #[test]
    fn modifiers() {
        let k = parse_combo("cmd+shift+t").unwrap();
        assert_eq!(k.keycode, 17);
        assert_eq!(k.flags, FLAG_COMMAND | FLAG_SHIFT);
        let k = parse_combo("Ctrl + Option + Delete").unwrap();
        assert_eq!(k.keycode, 51);
        assert_eq!(k.flags, FLAG_CONTROL | FLAG_ALTERNATE);
        assert_eq!(parse_combo("alt+left").unwrap().flags, FLAG_ALTERNATE);
        assert_eq!(parse_combo("command+q").unwrap().flags, FLAG_COMMAND);
        assert_eq!(parse_combo("fn+f1").unwrap().flags, FLAG_SECONDARY_FN);
        assert_eq!(parse_combo("win+q").unwrap().flags, FLAG_COMMAND);
    }

    #[test]
    fn plus_key() {
        let k = parse_combo("cmd++").unwrap();
        assert_eq!(k.keycode, 24);
        assert_eq!(k.flags, FLAG_COMMAND | FLAG_SHIFT);
        assert_eq!(parse_combo("cmd+plus").unwrap(), k);
        assert_eq!(parse_combo("cmd+=").unwrap(), KeyCombo { keycode: 24, flags: FLAG_COMMAND });
        assert_eq!(parse_combo("cmd+-").unwrap().keycode, 27);
        assert_eq!(parse_vk("ctrl++").unwrap(), VkCombo { held: vec![VK_CONTROL, VK_SHIFT], vk: VK_OEM_PLUS });
        assert_eq!(parse_vk("shift++").unwrap(), VkCombo { held: vec![VK_SHIFT], vk: VK_OEM_PLUS }, "shift is held once");
    }

    #[test]
    fn errors() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("cmd").unwrap_err().contains("only modifiers"));
        assert!(parse_combo("hyper+a").unwrap_err().contains("Unknown modifier"));
        assert!(parse_combo("cmd+banana").unwrap_err().contains("Unknown key"));
        assert!(parse_combo("cmd++a").is_err());
        assert!(parse_vk("ctrl").unwrap_err().contains("only modifiers"));
        assert!(parse_vk("ctrl+banana").unwrap_err().contains("Unknown key"));
    }

    #[test]
    fn every_key_name_has_a_windows_virtual_key() {
        // One table holds both codes, so a name the Mac accepts can't be missing on Windows;
        // check the table itself: names unique and lowercase, every code a real key.
        let mut names = std::collections::HashSet::new();
        for (name, _, code) in KEYS {
            assert!(names.insert(*name), "{name} is listed twice");
            assert_eq!(name.to_lowercase(), *name, "names are matched lowercased");
            assert!((0x08..=0xFE).contains(code), "{name}: {code:#x} isn't a virtual-key code");
            assert_eq!(parse_vk(name).unwrap().vk, *code, "{name}");
            assert_eq!(parse_combo(name).unwrap().keycode, keycode(name).unwrap(), "{name}");
        }
        // Aliases agree on both platforms: a name and its alias are one key everywhere.
        for (a, b) in [("delete", "backspace"), ("forwarddelete", "del"), ("return", "enter"), ("=", "equals"), ("`", "backtick"), ("pgup", "pageup")] {
            assert_eq!((keycode(a), vk(a)), (keycode(b), vk(b)), "{a} = {b}");
        }
        // The Mac's delete is a PC's Backspace; its forward delete is Delete.
        assert_eq!(vk("delete"), Some(0x08));
        assert_eq!(vk("forwarddelete"), Some(0x2E));
        assert_eq!(vk("f12"), Some(0x7B));
        assert_eq!(vk("up"), Some(0x26));
    }

    #[test]
    fn windows_modifiers_follow_the_pc() {
        // cmd means Ctrl on a PC (cmd+c copies); win/super/meta are the Windows key.
        assert_eq!(parse_vk("cmd+c").unwrap(), VkCombo { held: vec![VK_CONTROL], vk: 0x43 });
        assert_eq!(parse_vk("cmd+ctrl+c").unwrap().held, vec![VK_CONTROL], "Ctrl once");
        assert_eq!(parse_vk("win+r").unwrap(), VkCombo { held: vec![VK_LWIN], vk: 0x52 });
        assert_eq!(parse_vk("super+e").unwrap().held, vec![VK_LWIN]);
        assert_eq!(parse_vk("meta+e").unwrap().held, vec![VK_LWIN]);
        // Held in the order a person presses them, whatever order they're written in.
        assert_eq!(parse_vk("shift+win+alt+ctrl+t").unwrap().held, vec![VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN]);
        assert_eq!(parse_vk("option+left").unwrap(), VkCombo { held: vec![VK_MENU], vk: 0x25 });
        assert_eq!(parse_vk("Return").unwrap(), VkCombo { held: vec![], vk: VK_RETURN });
        // fn never reaches Windows: it's the keyboard's own key.
        assert!(parse_vk("fn+f1").unwrap_err().contains("fn"));
    }
}
