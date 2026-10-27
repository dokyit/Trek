//! The Mac as a Bluetooth LE keyboard and mouse for the iPhone (HID over GATT). This module is
//! the protocol: the report map the helper publishes, and taps, swipes, keys and text turned into
//! the reports it sends, with the pauses between them. The helper (helper.swift) only carries
//! them over Bluetooth.
//!
//! iOS shows a pointer for a mouse only while AssistiveTouch is on. The map has an absolute
//! pointer (X and Y from 0 to `ABS_MAX`) that iOS puts straight on the screen, so a tap lands
//! where it's aimed with no calibration; iOS ignores that pointer's buttons, so the press goes
//! through a second, relative mouse. The report map's layout follows xhoantran/iphone-use (MIT).

use std::fmt::Write as _;

/// Report IDs (they travel in the Report Reference descriptor, not the payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    Keyboard = 1,
    Absolute = 2,
    Relative = 3,
    Consumer = 4,
}

/// The absolute pointer's largest X and Y.
pub const ABS_MAX: u16 = 32767;

/// The Home button: Consumer "Menu", which iOS takes as Home with AssistiveTouch on or off.
/// (One press only: two close together open the app switcher.)
pub const HOME: u16 = 0x0040;

/// The HID report map: a boot keyboard, the absolute pointer, a relative mouse with a wheel, and
/// one consumer control.
pub fn report_map() -> Vec<u8> {
    let keyboard: &[u8] = &[
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, // keyboard, report 1
        0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, // modifiers
        0x75, 0x01, 0x95, 0x08, 0x81, 0x02, //
        0x95, 0x01, 0x75, 0x08, 0x81, 0x01, // reserved
        0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, // six keys
        0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, //
        0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, // LEDs
        0x95, 0x01, 0x75, 0x03, 0x91, 0x01, //
        0xC0,
    ];
    let absolute: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, // mouse, report 2
        0x09, 0x01, 0xA1, 0x00, //
        0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, // three buttons
        0x95, 0x03, 0x75, 0x01, 0x81, 0x02, //
        0x95, 0x01, 0x75, 0x05, 0x81, 0x01, //
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, // X, Y
        0x16, 0x00, 0x00, 0x26, 0xFF, 0x7F, // 0...32767
        0x75, 0x10, 0x95, 0x02, 0x81, 0x02, // absolute
        0xC0, 0xC0,
    ];
    let relative: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x03, // mouse, report 3
        0x09, 0x01, 0xA1, 0x00, //
        0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01, //
        0x95, 0x03, 0x75, 0x01, 0x81, 0x02, //
        0x95, 0x01, 0x75, 0x05, 0x81, 0x01, //
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, // X, Y, wheel
        0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, // relative
        0xC0, 0xC0,
    ];
    let consumer: &[u8] = &[
        0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x04, // consumer control, report 4
        0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, //
        0x75, 0x10, 0x95, 0x01, 0x81, 0x00, //
        0xC0,
    ];
    [keyboard, absolute, relative, consumer].concat()
}

/// One thing the helper does: send a report, or wait.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Send(Report, Vec<u8>),
    Wait(u32),
}

/// A BLE connection event comes every 15–30 ms: reports closer than that merge.
pub const STEP_MS: u32 = 30;

/// The line the helper reads: `hid 2:00ff7fff7f w60 3:01000000 …`.
pub fn encode(steps: &[Step]) -> String {
    let mut out = String::from("hid");
    for step in steps {
        match step {
            Step::Send(id, bytes) => {
                let _ = write!(out, " {}:", *id as u8);
                for b in bytes {
                    let _ = write!(out, "{b:02x}");
                }
            }
            Step::Wait(ms) => {
                let _ = write!(out, " w{ms}");
            }
        }
    }
    out
}

/// How long the steps take, in milliseconds.
pub fn duration_ms(steps: &[Step]) -> u32 {
    steps
        .iter()
        .map(|s| match s {
            Step::Wait(ms) => *ms,
            Step::Send(..) => 0,
        })
        .sum()
}

fn axis(fraction: f64) -> u16 {
    (fraction.clamp(0., 1.) * f64::from(ABS_MAX)).round() as u16
}

fn pointer(x: f64, y: f64, buttons: u8) -> Step {
    let (x, y) = (axis(x).to_le_bytes(), axis(y).to_le_bytes());
    Step::Send(Report::Absolute, vec![buttons, x[0], x[1], y[0], y[1]])
}

fn button(down: bool) -> Step {
    Step::Send(Report::Relative, vec![u8::from(down), 0, 0, 0])
}

/// A tap at (x, y), fractions of the screen from its top-left corner, held for `hold_ms`
/// (a long press when that's half a second or more).
pub fn tap(x: f64, y: f64, hold_ms: u32) -> Vec<Step> {
    vec![
        pointer(x, y, 0),
        Step::Wait(STEP_MS * 2),
        button(true),
        Step::Wait(hold_ms.max(STEP_MS)),
        button(false),
        Step::Wait(STEP_MS),
    ]
}

/// A drag from one point to another over `duration_ms` (a swipe; fractions as for `tap`).
pub fn swipe(from: (f64, f64), to: (f64, f64), duration_ms: u32) -> Vec<Step> {
    let steps = (duration_ms / STEP_MS).max(2);
    let mut out = vec![
        pointer(from.0, from.1, 0),
        Step::Wait(STEP_MS * 2),
        button(true),
        Step::Wait(STEP_MS),
    ];
    for i in 1..=steps {
        let t = f64::from(i) / f64::from(steps);
        out.push(pointer(
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
            1,
        ));
        out.push(Step::Wait(duration_ms / steps));
    }
    out.push(button(false));
    out.push(Step::Wait(STEP_MS));
    out
}

/// A consumer control press (Home, media keys).
pub fn consumer(usage: u16) -> Vec<Step> {
    let b = usage.to_le_bytes();
    vec![
        Step::Send(Report::Consumer, vec![b[0], b[1]]),
        Step::Wait(80),
        Step::Send(Report::Consumer, vec![0, 0]),
        Step::Wait(STEP_MS),
    ]
}

/// Home.
pub fn home() -> Vec<Step> {
    consumer(HOME)
}

/// Modifier bits of the keyboard report.
pub const CTRL: u8 = 0x01;
pub const SHIFT: u8 = 0x02;
pub const ALT: u8 = 0x04;
pub const CMD: u8 = 0x08;

/// A key's usage and the modifiers it needs on a US keyboard.
pub fn stroke(c: char) -> Option<(u8, u8)> {
    const SHIFTED: &str = "!@#$%^&*()_+{}|:\"~<>?";
    const BASES: &str = "1234567890-=[]\\;'`,./";
    if c.is_ascii_lowercase() {
        return Some((0x04 + (c as u8 - b'a'), 0));
    }
    if c.is_ascii_uppercase() {
        return Some((0x04 + (c as u8 - b'A'), SHIFT));
    }
    if let Some(i) = SHIFTED.find(c) {
        return stroke(BASES[i..].chars().next()?).map(|(k, _)| (k, SHIFT));
    }
    let code = match c {
        '1'..='9' => 0x1E + (c as u8 - b'1'),
        '0' => 0x27,
        '\n' => 0x28,
        '\t' => 0x2B,
        ' ' => 0x2C,
        '-' => 0x2D,
        '=' => 0x2E,
        '[' => 0x2F,
        ']' => 0x30,
        '\\' => 0x31,
        ';' => 0x33,
        '\'' => 0x34,
        '`' => 0x35,
        ',' => 0x36,
        '.' => 0x37,
        '/' => 0x38,
        _ => return None,
    };
    Some((code, 0))
}

fn keys(code: u8, modifiers: u8) -> Vec<Step> {
    let report = |m: u8, k: u8| Step::Send(Report::Keyboard, vec![m, 0, k, 0, 0, 0, 0, 0]);
    let mut out = vec![];
    if modifiers != 0 {
        out.extend([report(modifiers, 0), Step::Wait(STEP_MS)]);
    }
    out.extend([
        report(modifiers, code),
        Step::Wait(STEP_MS),
        report(modifiers, 0),
        Step::Wait(STEP_MS),
    ]);
    if modifiers != 0 {
        out.extend([report(0, 0), Step::Wait(STEP_MS)]);
    }
    out
}

/// Text typed key by key on a US layout. Characters it can't type (outside ASCII) are an error
/// naming the first.
pub fn type_text(text: &str) -> Result<Vec<Step>, String> {
    let mut out = vec![];
    for c in text.chars() {
        let (code, m) = stroke(c).ok_or_else(|| {
            format!(
                "Can't type {c:?}: the Bluetooth keyboard types US-ASCII only (paste or dictate the rest)"
            )
        })?;
        out.extend(keys(code, m));
    }
    Ok(out)
}

/// A named key or combination: "enter", "escape", "cmd+space", "shift+tab", "a".
pub fn key(combo: &str) -> Result<Vec<Step>, String> {
    let mut modifiers = 0u8;
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
            modifiers |= bit;
            continue;
        }
        if main.is_some() {
            return Err(format!("{combo}: one key besides the modifiers"));
        }
        let named = match lower.as_str() {
            "enter" | "return" => Some(0x28),
            "escape" | "esc" => Some(0x29),
            "backspace" | "delete" => Some(0x2A),
            "tab" => Some(0x2B),
            "space" => Some(0x2C),
            "right" | "arrowright" => Some(0x4F),
            "left" | "arrowleft" => Some(0x50),
            "down" | "arrowdown" => Some(0x51),
            "up" | "arrowup" => Some(0x52),
            "forwarddelete" => Some(0x4C),
            "home" => Some(0x4A),
            "end" => Some(0x4D),
            "pageup" => Some(0x4B),
            "pagedown" => Some(0x4E),
            _ => None,
        };
        main = Some(match named {
            Some(code) => code,
            None => {
                let mut chars = part.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        let (code, m) = stroke(c).ok_or_else(|| format!("Unknown key {part:?}"))?;
                        modifiers |= m;
                        code
                    }
                    _ => return Err(format!("Unknown key {part:?}")),
                }
            }
        });
    }
    let code = main.ok_or_else(|| format!("{combo}: which key?"))?;
    Ok(keys(code, modifiers))
}

/// A lone Control press: types nothing and keeps the phone from dimming.
pub fn nudge() -> Vec<Step> {
    vec![
        Step::Send(Report::Keyboard, vec![CTRL, 0, 0, 0, 0, 0, 0, 0]),
        Step::Wait(STEP_MS),
        Step::Send(Report::Keyboard, vec![0; 8]),
    ]
}

/// A point in a picture `w` × `h` as fractions of it (the screen it shows).
pub fn fraction(x: f64, y: f64, w: f64, h: f64) -> Result<(f64, f64), String> {
    if w <= 0. || h <= 0. {
        return Err("No screen size yet: take a phone_screenshot first".into());
    }
    if !(0. ..=w).contains(&x) || !(0. ..=h).contains(&y) {
        return Err(format!(
            "({x}, {y}) is off the screen: the screenshot is {w}×{h}"
        ));
    }
    Ok((x / w, y / h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_holds_four_reports_and_balances_its_collections() {
        let map = report_map();
        let opens = map.windows(2).filter(|w| w == &[0xA1, 0x01]).count();
        assert_eq!(opens, 4, "one application collection a report");
        for id in 1..=4u8 {
            assert!(map.windows(2).any(|w| w == [0x85, id]), "report id {id}");
        }
        assert_eq!(*map.last().unwrap(), 0xC0);
        // Keyboard first: iOS wants the keyboard as the first collection.
        assert_eq!(&map[..4], &[0x05, 0x01, 0x09, 0x06]);
    }

    #[test]
    fn taps_point_then_press_through_the_relative_mouse() {
        let steps = tap(0.5, 1.0, 80);
        assert_eq!(
            steps[0],
            Step::Send(Report::Absolute, vec![0, 0x00, 0x40, 0xFF, 0x7F])
        );
        assert_eq!(steps[2], Step::Send(Report::Relative, vec![1, 0, 0, 0]));
        assert_eq!(steps[4], Step::Send(Report::Relative, vec![0, 0, 0, 0]));
        assert_eq!(
            encode(&steps),
            "hid 2:000040ff7f w60 3:01000000 w80 3:00000000 w30"
        );
        assert_eq!(duration_ms(&steps), 170);
    }

    #[test]
    fn swipes_hold_the_button_along_the_way() {
        let steps = swipe((0.5, 0.8), (0.5, 0.2), 300);
        let moves: Vec<_> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Send(Report::Absolute, b) => Some(b.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(moves.len(), 11, "the start and ten steps");
        assert!(moves[1..].iter().all(|b| b[0] == 1), "button held");
        let last_y = u16::from_le_bytes([moves[10][3], moves[10][4]]);
        assert_eq!(last_y, axis(0.2));
        assert!(matches!(steps.last(), Some(Step::Wait(_))));
    }

    #[test]
    fn text_types_on_a_us_layout() {
        assert_eq!(stroke('a'), Some((0x04, 0)));
        assert_eq!(stroke('Z'), Some((0x1D, SHIFT)));
        assert_eq!(stroke('1'), Some((0x1E, 0)));
        assert_eq!(stroke('0'), Some((0x27, 0)));
        assert_eq!(stroke('!'), Some((0x1E, SHIFT)));
        assert_eq!(stroke('?'), Some((0x38, SHIFT)));
        assert_eq!(stroke('"'), Some((0x34, SHIFT)));
        assert_eq!(stroke('é'), None);
        let steps = type_text("Hi").unwrap();
        let sent = steps.iter().filter(|s| matches!(s, Step::Send(..))).count();
        assert_eq!(sent, 4 + 2, "H with shift down and up around it, then i");
        assert!(type_text("naïve").unwrap_err().contains("'ï'"));
    }

    #[test]
    fn combinations_name_their_keys() {
        let spotlight = key("cmd+space").unwrap();
        assert_eq!(
            spotlight[2],
            Step::Send(Report::Keyboard, vec![CMD, 0, 0x2C, 0, 0, 0, 0, 0])
        );
        assert!(key("Enter").is_ok() && key("shift+tab").is_ok() && key("A").is_ok());
        assert!(key("cmd+a+b").is_err());
        assert!(key("cmd").is_err());
        assert!(key("hyper").is_err());
    }

    #[test]
    fn home_is_one_press_of_menu() {
        let steps = home();
        assert_eq!(steps[0], Step::Send(Report::Consumer, vec![0x40, 0x00]));
        let presses = steps
            .iter()
            .filter(|s| matches!(s, Step::Send(_, b) if b != &[0, 0]))
            .count();
        assert_eq!(presses, 1, "two would open the app switcher");
    }

    #[test]
    fn screenshot_points_become_fractions() {
        assert_eq!(fraction(100., 200., 400., 800.).unwrap(), (0.25, 0.25));
        assert!(fraction(500., 10., 400., 800.).is_err());
        assert!(fraction(1., 1., 0., 0.).is_err());
    }
}
