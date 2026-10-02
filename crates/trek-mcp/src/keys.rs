//! Key-combo parsing ("cmd+shift+t", "return", "ctrl+left") into a macOS
//! virtual keycode (ANSI layout) plus CGEventFlags bits. Pure — no FFI.

pub const FLAG_SHIFT: u64 = 0x0002_0000; // kCGEventFlagMaskShift
pub const FLAG_CONTROL: u64 = 0x0004_0000; // kCGEventFlagMaskControl
pub const FLAG_ALTERNATE: u64 = 0x0008_0000; // kCGEventFlagMaskAlternate
pub const FLAG_COMMAND: u64 = 0x0010_0000; // kCGEventFlagMaskCommand
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000; // kCGEventFlagMaskSecondaryFn

pub const KEY_RETURN: u16 = 36;
pub const KEY_TAB: u16 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub keycode: u16,
    pub flags: u64,
}

fn modifier_flag(token: &str) -> Option<u64> {
    Some(match token {
        "cmd" | "command" | "meta" | "super" | "win" | "⌘" => FLAG_COMMAND,
        "shift" | "⇧" => FLAG_SHIFT,
        "alt" | "option" | "opt" | "⌥" => FLAG_ALTERNATE,
        "ctrl" | "control" | "ctl" | "⌃" => FLAG_CONTROL,
        "fn" => FLAG_SECONDARY_FN,
        _ => return None,
    })
}

/// macOS virtual keycode for a (lowercased) key name, ANSI layout.
pub fn keycode(name: &str) -> Option<u16> {
    Some(match name {
        // letters
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "o" => 31,
        "u" => 32,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "k" => 40,
        "n" => 45,
        "m" => 46,
        // digits
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "9" => 25,
        "7" => 26,
        "8" => 28,
        "0" => 29,
        // punctuation
        "=" | "equal" | "equals" => 24,
        "-" | "minus" => 27,
        "]" | "rightbracket" => 30,
        "[" | "leftbracket" => 33,
        "'" | "quote" => 39,
        ";" | "semicolon" => 41,
        "\\" | "backslash" => 42,
        "," | "comma" => 43,
        "/" | "slash" => 44,
        "." | "period" => 47,
        "`" | "grave" | "backtick" => 50,
        // named keys
        "return" | "enter" | "ret" => KEY_RETURN,
        "tab" => KEY_TAB,
        "space" | "spacebar" => 49,
        "delete" | "backspace" | "bksp" => 51,
        "escape" | "esc" => 53,
        "forwarddelete" | "fwddelete" | "del" => 117,
        "home" => 115,
        "end" => 119,
        "pageup" | "pgup" => 116,
        "pagedown" | "pgdn" => 121,
        "left" | "arrowleft" | "leftarrow" => 123,
        "right" | "arrowright" | "rightarrow" => 124,
        "down" | "arrowdown" | "downarrow" => 125,
        "up" | "arrowup" | "uparrow" => 126,
        "capslock" => 57,
        // function keys
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        _ => return None,
    })
}

/// Parse "cmd+shift+t", "Return", "ctrl+alt+delete", "cmd+plus", "cmd++".
pub fn parse_combo(combo: &str) -> Result<KeyCombo, String> {
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
    let (key, mods) = tokens.split_last().expect("at least one token");
    let mut flags = 0;
    for m in mods {
        flags |= modifier_flag(m).ok_or_else(|| {
            format!("Unknown modifier {m:?} in {combo:?} (use cmd, shift, alt/option, ctrl, fn)")
        })?;
    }
    let keycode = match *key {
        "plus" | "+" => {
            flags |= FLAG_SHIFT;
            24
        }
        k => match keycode(k) {
            Some(code) => code,
            None if modifier_flag(k).is_some() => {
                return Err(format!("Key combo {combo:?} has only modifiers; add a key, e.g. \"cmd+c\""));
            }
            None => {
                return Err(format!(
                    "Unknown key {k:?} in {combo:?}. Use a letter/digit/punctuation or one of: return, tab, space, delete, forwarddelete, escape, up, down, left, right, home, end, pageup, pagedown, f1-f12"
                ));
            }
        },
    };
    Ok(KeyCombo { keycode, flags })
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
        // every letter maps, and to a distinct code
        let mut seen = std::collections::HashSet::new();
        for c in 'a'..='z' {
            assert!(seen.insert(keycode(&c.to_string()).unwrap()), "dup for {c}");
        }
        for c in '0'..='9' {
            assert!(seen.insert(keycode(&c.to_string()).unwrap()), "dup for {c}");
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
    }

    #[test]
    fn plus_key() {
        let k = parse_combo("cmd++").unwrap();
        assert_eq!(k.keycode, 24);
        assert_eq!(k.flags, FLAG_COMMAND | FLAG_SHIFT);
        assert_eq!(parse_combo("cmd+plus").unwrap(), k);
        assert_eq!(parse_combo("cmd+=").unwrap(), KeyCombo { keycode: 24, flags: FLAG_COMMAND });
        assert_eq!(parse_combo("cmd+-").unwrap().keycode, 27);
    }

    #[test]
    fn errors() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("cmd").unwrap_err().contains("only modifiers"));
        assert!(parse_combo("hyper+a").unwrap_err().contains("Unknown modifier"));
        assert!(parse_combo("cmd+banana").unwrap_err().contains("Unknown key"));
        assert!(parse_combo("cmd++a").is_err());
    }
}
