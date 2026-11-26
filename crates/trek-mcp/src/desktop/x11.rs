//! Linux' desktop through X11, the part that decides: what a window's properties read as, which
//! window is in front, which keys type a text on the keyboard's layout, what a screenshot's
//! bytes are as a picture, and which app a name means. All of it is plain data, built and tested
//! on every platform; `server` (Linux only) asks the X server and does what's decided here.
//!
//! - **The screen** is the X server's root window, in pixels: every monitor, as the pointer
//!   moves over them. Screenshots are `GetImage` of it (`rgb_from_zpixmap`), scaled down here.
//! - **Windows** are the window manager's own list (EWMH's `_NET_CLIENT_LIST_STACKING`, front to
//!   back; `front_to_back`), each with its title, its app (`owner`) and where it shows
//!   (`visible_rect`: without a GTK window's shadow, with the window manager's frame).
//! - **The pointer and keys** are the XTEST extension's. A text is typed with the keys the layout
//!   has for it (`plan_text`); a character it has no key for is lent a key the keyboard doesn't
//!   use, for the length of the typing, as `xdotool` does.
//! - **Apps** are desktop entries (`parse_entry`, `entry_score`, `exec_argv`), else a program on
//!   PATH.
//! - **Wayland**: an X11 client reaches only the apps running through Xwayland there, and can't
//!   read the screen. A Wayland session is told so in plain words (`WAYLAND`) instead of being
//!   handed black screenshots and clicks that land nowhere.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
pub mod server;

use crate::keys::{KeysymCombo, XK_RETURN, XK_TAB};

/// Said by every tool that needs the screen, the pointer or the keyboard, in a Wayland session.
pub const WAYLAND: &str = "This is a Wayland session. Trek's computer use on Linux works through X11 (the X server's screen, pointer and keyboard), which under Wayland reaches only apps running through Xwayland and can't read the screen, so nothing was done. Log in to an X11 session to use it (\"on Xorg\" at the login screen); Wayland, through the desktop portal's RemoteDesktop, isn't built yet.";

/// Said when there's no X server to talk to.
pub const NO_DISPLAY: &str = "There's no X11 display to work on (DISPLAY isn't set, or its server didn't answer). Computer use on Linux needs an X11 session.";

/// Said when the X server can't make pointer and key events.
pub const NO_XTEST: &str = "This X server has no XTEST extension, which is how Trek moves the pointer and presses keys, so nothing was done.";

/// Whether the desktop session is Wayland's, from `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY` (Trek
/// takes the second out of its own environment to run as an X11 client, so the first decides
/// under Trek).
pub fn wayland_session(session_type: Option<&str>, wayland_display: Option<&str>) -> bool {
    session_type.is_some_and(|t| t.trim().eq_ignore_ascii_case("wayland"))
        || wayland_display.is_some_and(|d| !d.trim().is_empty())
}

// ---- properties ----

/// A property of 32-bit items (`CARDINAL`, `WINDOW`, `ATOM`), as the connection hands its bytes
/// over: in this computer's byte order.
pub fn cardinals(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// A text property: `UTF8_STRING` (`_NET_WM_NAME`), or the older `STRING` (`WM_NAME`), which is
/// Latin-1. Up to its first NUL.
pub fn text(bytes: &[u8], utf8: bool) -> String {
    let bytes = bytes.split(|b| *b == 0).next().unwrap_or_default();
    if utf8 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        bytes.iter().map(|b| char::from(*b)).collect()
    }
}

/// `WM_CLASS`: the instance and the class, each ended by a NUL (`"zenity\0Zenity\0"`).
pub fn wm_class(bytes: &[u8]) -> Option<(String, String)> {
    let mut parts = bytes.split(|b| *b == 0);
    let instance = text(parts.next()?, false);
    let class = parts.next().map(|c| text(c, false)).unwrap_or_default();
    (!instance.is_empty() || !class.is_empty()).then_some((instance, class))
}

/// The program's name from where `/proc/<pid>/exe` points: `/usr/bin/zenity` is `zenity`. A
/// program replaced while it runs reads as `… (deleted)`.
pub fn exe_name(path: &str) -> String {
    let path = path.strip_suffix(" (deleted)").unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Whether a window is one of Trek's own: its program is `trek`, or its class is Trek's app id
/// (`dev.trek.Trek`, as GPUI names its windows) or `trek`.
pub fn is_trek(class: Option<&(String, String)>, exe: Option<&str>) -> bool {
    let named = |s: &str| {
        let s = s.to_lowercase();
        s == "trek" || s.starts_with("dev.trek.trek")
    };
    exe.is_some_and(|e| e.eq_ignore_ascii_case("trek"))
        || class.is_some_and(|(instance, class)| named(instance) || named(class))
}

/// The app a window is of, as the window list names it: its class (`Zenity`, `firefox`: what
/// desktops know an app's windows by), else its program's name. One of Trek's own is `Trek`,
/// whichever of the two says so.
pub fn owner(class: Option<&(String, String)>, exe: Option<&str>) -> String {
    if is_trek(class, exe) {
        return "Trek".into();
    }
    match class {
        Some((_, class)) if !class.is_empty() => class.clone(),
        Some((instance, _)) if !instance.is_empty() => instance.clone(),
        _ => exe.unwrap_or_default().to_string(),
    }
}

/// Whether a window (its class and its program) is of the app `name` the agent asked `open_app`
/// for: by its class or instance (`firefox`, `Gnome-terminal`), its program (`gedit`,
/// `/usr/bin/gedit`), or a word of a longer name ("Google Chrome" is chrome's).
pub fn runs_as(class: Option<&(String, String)>, exe: Option<&str>, name: &str) -> bool {
    let name = name.trim().trim_end_matches('/');
    let name = name.rsplit('/').next().unwrap_or(name).to_lowercase();
    let name = name.strip_suffix(".desktop").unwrap_or(&name);
    if name.is_empty() {
        return false;
    }
    let mut known: Vec<String> = vec![];
    if let Some((instance, class)) = class {
        known.push(instance.to_lowercase());
        known.push(class.to_lowercase());
    }
    if let Some(exe) = exe {
        known.push(exe.to_lowercase());
    }
    known.iter().filter(|k| !k.is_empty()).any(|k| {
        k == name
            // `org.gnome.Calculator`'s windows are of "calculator".
            || k.rsplit('.').next() == Some(name)
            || (k.len() >= 4 && name.split_whitespace().any(|word| word == k))
    })
}

// ---- windows ----

/// Where a window shows, in screen pixels, from its own (client) window's place and size: without
/// the see-through margin a GTK window draws its shadow in (`_GTK_FRAME_EXTENTS`; a click there
/// goes to whatever is under it), and with the frame the window manager put around it
/// (`_NET_FRAME_EXTENTS`: its title bar is the window's too). Both are left, right, top, bottom.
pub fn visible_rect(
    (x, y, w, h): (i32, i32, u32, u32),
    gtk: Option<[u32; 4]>,
    frame: Option<[u32; 4]>,
) -> (f64, f64, f64, f64) {
    let (mut x, mut y, mut w, mut h) = (x as f64, y as f64, w as f64, h as f64);
    if let Some([left, right, top, bottom]) = gtk.map(|e| e.map(f64::from))
        && left + right < w
        && top + bottom < h
    {
        x += left;
        y += top;
        w -= left + right;
        h -= top + bottom;
    }
    if let Some([left, right, top, bottom]) = frame.map(|e| e.map(f64::from)) {
        x -= left;
        y -= top;
        w += left + right;
        h += top + bottom;
    }
    (x, y, w, h)
}

/// Four 32-bit items of a property, as both frame-extents properties are.
pub fn extents(bytes: &[u8]) -> Option<[u32; 4]> {
    <[u32; 4]>::try_from(cardinals(bytes).get(..4)?).ok()
}

/// The windows, front to back. `stacking` is the window manager's `_NET_CLIENT_LIST_STACKING`
/// (back to front); without it, `clients` is its `_NET_CLIENT_LIST` (oldest first, so the newest
/// is taken for the front, after the `active` one); with no window manager at all, `children`
/// are the root window's own, which the server keeps back to front.
pub fn front_to_back(
    stacking: Option<Vec<u32>>,
    clients: Option<Vec<u32>>,
    children: &[u32],
    active: Option<u32>,
) -> Vec<u32> {
    let has = |list: &Option<Vec<u32>>| list.as_ref().is_some_and(|l| !l.is_empty());
    let mut order: Vec<u32> = if has(&stacking) {
        stacking.unwrap_or_default()
    } else if has(&clients) {
        let mut clients = clients.unwrap_or_default();
        if let Some(at) = active.and_then(|a| clients.iter().position(|c| *c == a)) {
            let active = clients.remove(at);
            clients.push(active);
        }
        clients
    } else {
        children.to_vec()
    };
    order.reverse();
    order.dedup();
    order
}

/// The window keys go to, among those `listed`: the window manager's active one, else the one
/// that has the input focus or holds the window that has (`focus`: that window, then each window
/// it's inside, outwards).
pub fn key_window(active: Option<u32>, focus: &[u32], listed: &[u32]) -> Option<u32> {
    active
        .filter(|a| listed.contains(a))
        .or_else(|| focus.iter().copied().find(|f| listed.contains(f)))
}

// ---- the pointer ----

/// A screen point as the X protocol takes it: whole pixels, on the screen.
pub fn screen_point(x: f64, y: f64, width: u16, height: u16) -> (i16, i16) {
    let on = |v: f64, size: u16| {
        let most = i64::from(size.max(1)) - 1;
        (v.round() as i64).clamp(0, most.min(i64::from(i16::MAX))) as i16
    };
    (on(x, width), on(y, height))
}

/// The wheel as X has it: a click of button 4 scrolls up a notch, 5 down, 6 left, 7 right.
/// Positive `dy` scrolls down and positive `dx` right, a notch a line; never more than
/// `MOST_NOTCHES` at once.
pub fn scroll_clicks(dx: i32, dy: i32) -> Vec<(u8, u32)> {
    const MOST_NOTCHES: u32 = 200;
    let mut clicks = vec![];
    for (lines, back, forth) in [(dy, 4u8, 5u8), (dx, 6, 7)] {
        if lines != 0 {
            let button = if lines > 0 { forth } else { back };
            clicks.push((button, lines.unsigned_abs().min(MOST_NOTCHES)));
        }
    }
    clicks
}

/// The points a drag moves through between its ends, the last of them `to`: a step every dozen
/// pixels or so, as a hand moves, so the app sees a drag and not a jump.
pub fn drag_path(from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
    let distance = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let steps = ((distance / 12.0).ceil() as usize).clamp(8, 60);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
        })
        .collect()
}

// ---- keys ----

/// The keyboard's layout as the X server has it (`GetKeyboardMapping`): for each keycode from
/// `min` up, `per` keysyms. The first two are what the key types alone and with Shift in the
/// first layout group; the next two, in the second.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keymap {
    pub min: u8,
    pub per: usize,
    pub syms: Vec<u32>,
}

impl Keymap {
    fn codes(&self) -> impl DoubleEndedIterator<Item = (u8, &[u32])> {
        self.syms
            .chunks(self.per.max(1))
            .enumerate()
            .filter_map(|(i, row)| Some((u8::try_from(usize::from(self.min) + i).ok()?, row)))
    }

    /// The key that types `sym` in layout group `group`: its keycode, and whether with Shift.
    /// One that types it alone is taken before one that needs Shift.
    pub fn find(&self, sym: u32, group: usize) -> Option<(u8, bool)> {
        if sym == 0 {
            return None;
        }
        let at = |column: usize| {
            self.codes()
                .find(|(_, row)| row.get(column) == Some(&sym))
                .map(|(code, _)| code)
        };
        at(group * 2)
            .map(|code| (code, false))
            .or_else(|| at(group * 2 + 1).map(|code| (code, true)))
    }

    /// A key that gives `sym` at any level: how a modifier's own key is found.
    pub fn any(&self, sym: u32) -> Option<u8> {
        self.codes()
            .find(|(_, row)| row.contains(&sym))
            .map(|(code, _)| code)
    }

    /// The keycodes no key gives anything for, highest first: lent to a keysym the layout has no
    /// key for, while it's typed.
    pub fn spares(&self) -> Vec<u8> {
        self.codes()
            .rev()
            .filter(|(_, row)| row.iter().all(|s| *s == 0))
            .map(|(code, _)| code)
            .collect()
    }
}

/// The keysym that types `c`: Latin-1's characters are their own, Return and Tab are keys, and
/// any other is its Unicode code point with X's mark on it. `None` for a control character,
/// which no key types.
pub fn keysym_of(c: char) -> Option<u32> {
    Some(match c {
        '\n' | '\r' => XK_RETURN,
        '\t' => XK_TAB,
        ' '..='~' | '\u{a0}'..='\u{ff}' => c as u32,
        c if c.is_control() => return None,
        c => 0x0100_0000 | c as u32,
    })
}

/// One key pressed and let go, with Shift held or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u8,
    pub shift: bool,
}

/// A stretch of a text as it's typed: the spare keys lent to keysyms the layout has no key for
/// (keycode, keysym), then the strokes. The lent keys are given back after the run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub lend: Vec<(u8, u32)>,
    pub strokes: Vec<Stroke>,
}

/// How `text` is typed on the layout `map` has, in layout group `group`, with Caps Lock on or
/// not (`caps`: a letter's key then types its capital, and with Shift its small letter). A
/// character the layout has no key for is lent a spare key; when a text has more such characters
/// than there are spare keys, it's typed in runs, each lending them afresh. `\r\n` is one Return.
pub fn plan_text(text: &str, map: &Keymap, group: usize, caps: bool) -> Result<Vec<Run>, String> {
    let spares = map.spares();
    let mut runs = vec![];
    let mut run = Run::default();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue;
        }
        let Some(sym) = keysym_of(c) else {
            continue;
        };
        if let Some((code, shift)) = map.find(sym, group) {
            run.strokes.push(Stroke {
                code,
                shift: shift != (caps && c.is_alphabetic()),
            });
            continue;
        }
        let lent = run
            .lend
            .iter()
            .find(|(_, s)| *s == sym)
            .map(|(code, _)| *code);
        let code = match lent {
            Some(code) => code,
            None => {
                if run.lend.len() == spares.len() {
                    if spares.is_empty() {
                        return Err(format!(
                            "This keyboard's layout has no key for {c:?}, and no spare key to lend it, so the text wasn't typed."
                        ));
                    }
                    runs.push(std::mem::take(&mut run));
                }
                let code = spares[run.lend.len()];
                run.lend.push((code, sym));
                code
            }
        };
        // A lent key gives the one keysym at every level: no Shift, and Caps Lock changes nothing.
        run.strokes.push(Stroke { code, shift: false });
    }
    if !run.strokes.is_empty() {
        runs.push(run);
    }
    Ok(runs)
}

/// A key combo as it's pressed: the modifiers' keycodes to hold, in order, then the key's; and
/// the spare key lent to the key's keysym when the layout gives it only with Shift, or not at
/// all (the `/` of a German keyboard, the keypad's `0` with Num Lock either way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub lend: Option<(u8, u32)>,
    pub held: Vec<u8>,
    pub code: u8,
}

pub fn plan_key(combo: &KeysymCombo, map: &Keymap, group: usize) -> Result<Chord, String> {
    let held = combo
        .held
        .iter()
        .map(|sym| {
            map.any(*sym).ok_or_else(|| {
                format!(
                    "This keyboard has no {} key, so the combo wasn't pressed.",
                    modifier_name(*sym)
                )
            })
        })
        .collect::<Result<Vec<u8>, String>>()?;
    if let Some((code, false)) = map.find(combo.keysym, group) {
        return Ok(Chord {
            lend: None,
            held,
            code,
        });
    }
    let spare = map.spares().into_iter().next().ok_or(
        "This keyboard's layout has no key of its own for that, and no spare key to lend it, so the combo wasn't pressed.",
    )?;
    Ok(Chord {
        lend: Some((spare, combo.keysym)),
        held,
        code: spare,
    })
}

fn modifier_name(sym: u32) -> &'static str {
    match sym {
        crate::keys::XK_SHIFT => "Shift",
        crate::keys::XK_CONTROL => "Ctrl",
        crate::keys::XK_ALT => "Alt",
        crate::keys::XK_SUPER => "Super",
        _ => "such",
    }
}

/// The layout group in effect and whether Caps Lock is on, from the modifier state the server
/// gives with the pointer (`QueryPointer`'s mask): XKB keeps the group in bits 13 and 14, and
/// Lock is bit 1.
pub fn group_and_caps(mask: u16) -> (usize, bool) {
    (usize::from((mask >> 13) & 3), mask & 2 != 0)
}

// ---- pictures ----

/// Width and height fitted inside `max_side` on the long side, aspect kept, never upscaled.
pub fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    if long <= max_side {
        return (w.max(1), h.max(1));
    }
    let s = max_side as f64 / long as f64;
    (
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
    )
}

/// A region of the screen (x, y, width, height, in pixels) kept on a `width`×`height` screen, as
/// `GetImage` needs it: the server refuses a rectangle that leaves the screen. `None` when
/// nothing of it is on the screen.
pub fn on_screen(
    (x, y, w, h): (f64, f64, f64, f64),
    width: u16,
    height: u16,
) -> Option<(i16, i16, u16, u16)> {
    let (sw, sh) = (i64::from(width), i64::from(height));
    let x0 = (x.round() as i64).clamp(0, sw);
    let y0 = (y.round() as i64).clamp(0, sh);
    let x1 = ((x + w.max(1.0)).round() as i64).clamp(0, sw);
    let y1 = ((y + h.max(1.0)).round() as i64).clamp(0, sh);
    (x1 > x0 && y1 > y0 && x0 <= i64::from(i16::MAX) && y0 <= i64::from(i16::MAX))
        .then(|| (x0 as i16, y0 as i16, (x1 - x0) as u16, (y1 - y0) as u16))
}

/// How the X server lays out the pixels of an image (`GetImage` in `ZPixmap` format): from its
/// setup and the screen's visual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    /// Each row is padded to a multiple of this many bits.
    pub scanline_pad: u8,
    /// The least significant byte of a pixel comes first.
    pub lsb_first: bool,
    pub red: u32,
    pub green: u32,
    pub blue: u32,
}

/// An image's bytes as RGB, three bytes a pixel, row after row.
pub fn rgb_from_zpixmap(
    data: &[u8],
    width: usize,
    height: usize,
    f: &PixelFormat,
) -> Result<Vec<u8>, String> {
    let bytes = usize::from(f.bits_per_pixel) / 8;
    if !(2..=4).contains(&bytes) || f.bits_per_pixel % 8 != 0 {
        return Err(format!(
            "The X server's screen has {}-bit pixels, which Trek doesn't read.",
            f.bits_per_pixel
        ));
    }
    let pad = usize::from(f.scanline_pad.max(8));
    let stride = (width * usize::from(f.bits_per_pixel)).div_ceil(pad) * pad / 8;
    if width == 0 || height == 0 || data.len() < stride * (height - 1) + width * bytes {
        return Err("The X server sent less of the screen than was asked for.".into());
    }
    // A channel's place in the pixel, and what its largest value is.
    let channel = |mask: u32| {
        (
            mask.trailing_zeros().min(31),
            mask >> mask.trailing_zeros().min(31),
        )
    };
    let channels = [channel(f.red), channel(f.green), channel(f.blue)];
    let mut rgb = Vec::with_capacity(width * height * 3);
    for row in data.chunks(stride).take(height) {
        for px in row.chunks_exact(bytes).take(width) {
            let mut pixel = 0u32;
            for (i, b) in px.iter().enumerate() {
                let shift = if f.lsb_first { i } else { bytes - 1 - i };
                pixel |= u32::from(*b) << (8 * shift);
            }
            for (shift, most) in channels {
                rgb.push(match most {
                    0 => 0,
                    255 => ((pixel >> shift) & 255) as u8,
                    most => (((pixel >> shift) & most) * 255 / most) as u8,
                });
            }
        }
    }
    Ok(rgb)
}

/// `rgb` (`w`×`h`, three bytes a pixel) made `tw`×`th`, each new pixel the average of the old
/// ones it covers: text stays readable where dropping pixels would break its strokes.
pub fn downscale(rgb: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    if (w, h) == (tw, th) || w == 0 || h == 0 || tw == 0 || th == 0 {
        return rgb.to_vec();
    }
    // For each new column or row: the old ones it covers, and how much of it each is.
    let cover = |old: usize, new: usize| -> Vec<Vec<(usize, f32)>> {
        let ratio = old as f64 / new as f64;
        (0..new)
            .map(|i| {
                let (start, end) = (i as f64 * ratio, (i + 1) as f64 * ratio);
                let mut parts = vec![];
                let mut at = start.floor() as usize;
                while (at as f64) < end && at < old {
                    let share = end.min(at as f64 + 1.0) - start.max(at as f64);
                    if share > 0.0 {
                        parts.push((at, (share / ratio) as f32));
                    }
                    at += 1;
                }
                if parts.is_empty() {
                    parts.push((start.floor().min(old as f64 - 1.0) as usize, 1.0));
                }
                parts
            })
            .collect()
    };
    let (columns, rows) = (cover(w, tw), cover(h, th));
    let mut out = Vec::with_capacity(tw * th * 3);
    for row in &rows {
        for column in &columns {
            let mut sum = [0f32; 3];
            for (y, wy) in row {
                for (x, wx) in column {
                    let at = (y * w + x) * 3;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += f32::from(rgb[at + c]) * wy * wx;
                    }
                }
            }
            out.extend(sum.map(|s| s.round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

// ---- apps ----

/// An app as its desktop entry (`/usr/share/applications/org.gnome.Calculator.desktop`) has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entry {
    /// The file's name without `.desktop`.
    pub id: String,
    pub name: String,
    pub exec: String,
    /// `StartupWMClass`: the class its windows have, where the entry says.
    pub wm_class: String,
}

/// A desktop entry's file as an app that can be launched: an `Application` with a command, not
/// hidden, and not one that needs a terminal to run in. `None` otherwise.
pub fn parse_entry(id: &str, contents: &str) -> Option<Entry> {
    let mut entry = Entry {
        id: id.strip_suffix(".desktop").unwrap_or(id).to_string(),
        ..Entry::default()
    };
    let (mut in_group, mut is_app, mut off) = (false, false, false);
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the entry's own group: its actions' groups have commands of their own.
            in_group = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_group) else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "Type" => is_app = value == "Application",
            "Name" => entry.name = value.to_string(),
            "Exec" => entry.exec = value.to_string(),
            "StartupWMClass" => entry.wm_class = value.to_string(),
            "Hidden" | "Terminal" => off |= value == "true",
            _ => {}
        }
    }
    (is_app && !off && !entry.exec.is_empty()).then_some(entry)
}

/// How well a desktop entry answers to the `name` an agent gave `open_app`; 0 when it doesn't.
/// Its own name comes first ("Text Editor"), then its id (`org.gnome.gedit`, or its last part
/// `gedit`), then its program or its windows' class, then a word of its name ("chrome" for
/// Google Chrome).
pub fn entry_score(e: &Entry, name: &str) -> u8 {
    let name = name.trim().to_lowercase();
    let name = name.strip_suffix(".desktop").unwrap_or(&name);
    if name.is_empty() {
        return 0;
    }
    let (id, entry_name) = (e.id.to_lowercase(), e.name.to_lowercase());
    let program = exec_argv(&e.exec)
        .first()
        .map(|p| exe_name(p).to_lowercase())
        .unwrap_or_default();
    if entry_name == name {
        5
    } else if id == name || id.rsplit('.').next() == Some(name) {
        4
    } else if program == name || e.wm_class.to_lowercase() == name {
        3
    } else if name.len() >= 4 && entry_name.split_whitespace().any(|word| word == name) {
        2
    } else {
        0
    }
}

/// A desktop entry's `Exec` line as the program and its arguments: quoted arguments unquoted,
/// and the field codes a launcher fills in (`%U`: the files to open, `%i`, `%c`, `%k`) left out,
/// as with nothing to open (`%%` is a percent sign).
pub fn exec_argv(exec: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![];
    let mut arg = String::new();
    let (mut quoted, mut any) = (false, false);
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            // Within quotes, a backslash keeps the next of " ` $ \ as itself.
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    arg.push(next);
                }
            }
            '%' => match chars.next() {
                Some('%') => arg.push('%'),
                // A field code: nothing to fill it with.
                Some(_) | None => {}
            },
            c if c.is_whitespace() && !quoted => {
                if any || !arg.is_empty() {
                    args.push(std::mem::take(&mut arg));
                }
                any = false;
            }
            c => arg.push(c),
        }
    }
    if any || !arg.is_empty() {
        args.push(arg);
    }
    // A field code that stood alone left an empty argument only when it was quoted.
    args.retain(|a| !a.is_empty());
    args
}

/// The folders desktop entries are in, the user's own first: `$XDG_DATA_HOME` (else
/// `~/.local/share`), then each of `$XDG_DATA_DIRS` (else `/usr/local/share` and `/usr/share`),
/// then Flatpak's and Snap's, each with `applications` under it.
pub fn entry_dirs(
    home: Option<&str>,
    data_home: Option<&str>,
    data_dirs: Option<&str>,
) -> Vec<String> {
    let mut dirs: Vec<String> = vec![];
    let set = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty());
    match (set(data_home), set(home)) {
        (Some(data), _) => dirs.push(data.to_string()),
        (None, Some(home)) => dirs.push(format!("{home}/.local/share")),
        (None, None) => {}
    }
    match set(data_dirs) {
        Some(list) => dirs.extend(list.split(':').filter(|d| !d.is_empty()).map(String::from)),
        None => dirs.extend(["/usr/local/share".to_string(), "/usr/share".to_string()]),
    }
    if let Some(home) = set(home) {
        dirs.push(format!("{home}/.local/share/flatpak/exports/share"));
    }
    dirs.push("/var/lib/flatpak/exports/share".into());
    dirs.push("/var/lib/snapd/desktop".into());
    let mut seen = std::collections::HashSet::new();
    dirs.into_iter()
        .map(|d| format!("{}/applications", d.trim_end_matches('/')))
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{XK_CONTROL, XK_SHIFT, parse_x11};

    #[test]
    fn a_wayland_session_is_known_by_its_type_even_under_trek() {
        assert!(wayland_session(Some("wayland"), None));
        assert!(wayland_session(Some("Wayland"), Some("wayland-0")));
        // Trek takes WAYLAND_DISPLAY out of its own environment; the session's type still says.
        assert!(wayland_session(Some("wayland"), Some("")));
        assert!(wayland_session(None, Some("wayland-0")));
        assert!(!wayland_session(Some("x11"), None));
        assert!(!wayland_session(None, None));
        assert!(!wayland_session(Some("tty"), Some("  ")));
        // What a Wayland session is told names the way out, in plain words.
        assert!(WAYLAND.contains("Wayland session") && WAYLAND.contains("X11 session"));
        assert!(WAYLAND.contains("nothing was done"));
    }

    #[test]
    fn properties_read_as_numbers_and_text() {
        let ids = [0x0040_0001u32, 0x0060_0007, 9];
        let bytes: Vec<u8> = ids.iter().flat_map(|i| i.to_ne_bytes()).collect();
        assert_eq!(cardinals(&bytes), ids);
        // A short tail isn't an item.
        assert_eq!(cardinals(&bytes[..10]), ids[..2]);
        assert_eq!(cardinals(&[]), Vec::<u32>::new());
        assert_eq!(
            text("Übersicht — Notes".as_bytes(), true),
            "Übersicht — Notes"
        );
        // WM_NAME is Latin-1: 0xDC is Ü.
        assert_eq!(text(&[0xDC, b'b', b'e', b'r'], false), "Über");
        assert_eq!(text(b"xterm\0junk", true), "xterm");
        assert_eq!(
            wm_class(b"zenity\0Zenity\0"),
            Some(("zenity".into(), "Zenity".into()))
        );
        assert_eq!(wm_class(b"xev\0"), Some(("xev".into(), String::new())));
        assert_eq!(wm_class(b""), None);
        assert_eq!(wm_class(b"\0\0"), None);
        assert_eq!(extents(&bytes), None);
        let four: Vec<u8> = [1u32, 2, 30, 4]
            .iter()
            .flat_map(|i| i.to_ne_bytes())
            .collect();
        assert_eq!(extents(&four), Some([1, 2, 30, 4]));
    }

    fn class(instance: &str, class: &str) -> Option<(String, String)> {
        Some((instance.into(), class.into()))
    }

    #[test]
    fn a_window_is_named_for_its_app_and_treks_own_are_treks() {
        assert_eq!(exe_name("/usr/bin/zenity"), "zenity");
        assert_eq!(exe_name("/opt/Trek/trek (deleted)"), "trek");
        assert_eq!(exe_name("gedit"), "gedit");
        assert_eq!(
            owner(class("zenity", "Zenity").as_ref(), Some("zenity")),
            "Zenity"
        );
        assert_eq!(owner(class("xev", "").as_ref(), None), "xev");
        assert_eq!(owner(None, Some("python3")), "python3");
        assert_eq!(owner(None, None), "");
        // Trek by its program, by GPUI's app id as its class, or by a plain class: `computer`
        // keeps agents out of any window named so.
        for (c, exe) in [
            (class("dev.trek.Trek", "dev.trek.Trek"), None),
            (
                class("dev.trek.Trek.dev", "dev.trek.Trek.dev"),
                Some("trek-dev"),
            ),
            (class("trek", "Trek"), None),
            (class("something", "Else"), Some("trek")),
            (None, Some("Trek")),
        ] {
            assert!(is_trek(c.as_ref(), exe), "{c:?} {exe:?}");
            assert_eq!(owner(c.as_ref(), exe), "Trek");
        }
        for (c, exe) in [
            (class("trekking", "Trekking"), Some("trekking")),
            (class("zenity", "Zenity"), Some("zenity")),
            (None, Some("trek-mcp")),
        ] {
            assert!(!is_trek(c.as_ref(), exe), "{c:?} {exe:?}");
        }
    }

    #[test]
    fn a_running_app_is_known_by_its_names() {
        let firefox = class("Navigator", "firefox");
        assert!(runs_as(firefox.as_ref(), Some("firefox-bin"), "firefox"));
        assert!(runs_as(firefox.as_ref(), Some("firefox-bin"), "Firefox"));
        assert!(runs_as(
            firefox.as_ref(),
            Some("firefox-bin"),
            "/usr/bin/firefox-bin"
        ));
        assert!(runs_as(firefox.as_ref(), None, "firefox.desktop"));
        let chrome = class("google-chrome", "Google-chrome");
        assert!(runs_as(chrome.as_ref(), Some("chrome"), "Google Chrome"));
        let calc = class("org.gnome.Calculator", "org.gnome.Calculator");
        assert!(runs_as(
            calc.as_ref(),
            Some("gnome-calculator"),
            "calculator"
        ));
        assert!(runs_as(
            calc.as_ref(),
            Some("gnome-calculator"),
            "gnome-calculator"
        ));
        assert!(!runs_as(
            firefox.as_ref(),
            Some("firefox-bin"),
            "thunderbird"
        ));
        assert!(!runs_as(firefox.as_ref(), Some("firefox-bin"), ""));
        assert!(!runs_as(None, None, "firefox"));
        // A short program name isn't a word of every longer one.
        assert!(!runs_as(
            class("vi", "Vi").as_ref(),
            Some("vi"),
            "Vi IMproved"
        ));
    }

    #[test]
    fn a_window_shows_without_its_shadow_and_with_its_frame() {
        // A plain window: where its own window is.
        assert_eq!(
            visible_rect((100, 50, 800, 600), None, None),
            (100.0, 50.0, 800.0, 600.0)
        );
        // A GTK window draws its shadow inside its own window: 26 px each side, 23 above, 29
        // below. A click there lands on what's under it.
        assert_eq!(
            visible_rect((74, 27, 852, 652), Some([26, 26, 23, 29]), None),
            (100.0, 50.0, 800.0, 600.0)
        );
        // A window manager's frame is around it: a 28 px title bar and 1 px borders.
        assert_eq!(
            visible_rect((101, 78, 798, 571), None, Some([1, 1, 28, 1])),
            (100.0, 50.0, 800.0, 600.0)
        );
        // Margins larger than the window are nonsense: left alone.
        assert_eq!(
            visible_rect((0, 0, 40, 40), Some([26, 26, 23, 29]), None),
            (0.0, 0.0, 40.0, 40.0)
        );
    }

    #[test]
    fn windows_are_listed_front_to_back() {
        // The window manager's stacking list is back to front.
        assert_eq!(
            front_to_back(Some(vec![1, 2, 3]), Some(vec![3, 1, 2]), &[9], Some(2)),
            [3, 2, 1]
        );
        // Without it: its client list, newest first, the active one before them all.
        assert_eq!(
            front_to_back(None, Some(vec![1, 2, 3]), &[9], Some(1)),
            [1, 3, 2]
        );
        assert_eq!(
            front_to_back(Some(vec![]), Some(vec![1, 2]), &[], None),
            [2, 1]
        );
        // With no window manager: the root window's children, which the server keeps back to
        // front.
        assert_eq!(front_to_back(None, None, &[7, 8, 9], None), [9, 8, 7]);
        assert_eq!(front_to_back(None, None, &[], Some(4)), Vec::<u32>::new());
    }

    #[test]
    fn keys_go_to_the_active_window_or_the_one_holding_the_focus() {
        assert_eq!(key_window(Some(2), &[5], &[1, 2, 3]), Some(2));
        // No active window from a window manager: the focus is in a window inside window 3.
        assert_eq!(key_window(None, &[31, 30, 3], &[1, 2, 3]), Some(3));
        // An active window that isn't listed (closed since) doesn't count.
        assert_eq!(key_window(Some(8), &[1], &[1, 2, 3]), Some(1));
        assert_eq!(key_window(None, &[], &[1, 2, 3]), None);
        assert_eq!(key_window(Some(0), &[40], &[1, 2, 3]), None);
    }

    #[test]
    fn the_pointer_stays_on_the_screen_and_the_wheel_is_buttons() {
        assert_eq!(screen_point(100.4, 200.6, 1920, 1200), (100, 201));
        assert_eq!(screen_point(-5.0, 5000.0, 1920, 1200), (0, 1199));
        assert_eq!(screen_point(1920.0, 0.0, 1920, 1200), (1919, 0));
        assert_eq!(scroll_clicks(0, 3), [(5, 3)]);
        assert_eq!(scroll_clicks(0, -2), [(4, 2)]);
        assert_eq!(scroll_clicks(4, 0), [(7, 4)]);
        assert_eq!(scroll_clicks(-1, 1), [(5, 1), (6, 1)]);
        assert_eq!(scroll_clicks(0, 0), []);
        assert_eq!(scroll_clicks(0, 100_000), [(5, 200)]);
        // A drag moves through the points between, and ends where it's going.
        let path = drag_path((10.0, 10.0), (250.0, 10.0));
        assert_eq!(path.len(), 20);
        assert_eq!(path.last(), Some(&(250.0, 10.0)));
        assert!(path.windows(2).all(|p| p[1].0 > p[0].0 && p[1].1 == 10.0));
        assert_eq!(drag_path((0.0, 0.0), (3.0, 4.0)).len(), 8);
        assert_eq!(drag_path((0.0, 0.0), (5000.0, 0.0)).len(), 60);
    }

    /// A US keyboard's part: keycodes from 8, four keysyms each (two layout groups), as XKB
    /// hands a layout to the core protocol. Keycodes 8 and 9 give nothing; the second group is
    /// Greek on the letters.
    fn us() -> Keymap {
        let rows: &[[u32; 4]] = &[
            [0, 0, 0, 0],                     // 8: spare
            [0, 0, 0, 0],                     // 9: spare
            [0x31, 0x21, 0x31, 0x21],         // 10: 1 !
            [0x61, 0x41, 0x7e1, 0x7c1],       // 11: a A / α Α
            [0x68, 0x48, 0x7e7, 0x7c7],       // 12: h H
            [0x2c, 0x3c, 0x2c, 0x3c],         // 13: , <
            [0x20, 0x20, 0x20, 0x20],         // 14: space
            [XK_RETURN, 0, XK_RETURN, 0],     // 15
            [XK_TAB, 0xfe20, XK_TAB, 0xfe20], // 16: Tab, ISO_Left_Tab
            [XK_SHIFT, 0, XK_SHIFT, 0],       // 17
            [XK_CONTROL, 0, XK_CONTROL, 0],   // 18
            [0xff9e, 0xffb0, 0xff9e, 0xffb0], // 19: KP_Insert, KP_0
            [0x37, 0x2f, 0x37, 0x2f],         // 20: 7 / (a German keyboard's slash)
        ];
        Keymap {
            min: 8,
            per: 4,
            syms: rows.iter().flatten().copied().collect(),
        }
    }

    fn stroke(code: u8, shift: bool) -> Stroke {
        Stroke { code, shift }
    }

    #[test]
    fn a_layout_says_which_key_types_what() {
        let map = us();
        assert_eq!(map.find(0x61, 0), Some((11, false)));
        assert_eq!(map.find(0x41, 0), Some((11, true)));
        assert_eq!(map.find(0x21, 0), Some((10, true)));
        // In the second layout group, the letter keys type Greek.
        assert_eq!(map.find(0x7e1, 1), Some((11, false)));
        assert_eq!(map.find(0x61, 1), None);
        assert_eq!(map.find(0x7e1, 0), None);
        // A third group isn't in the core protocol's part at all.
        assert_eq!(map.find(0x61, 2), None);
        assert_eq!(map.find(0, 0), None);
        assert_eq!(map.any(XK_SHIFT), Some(17));
        assert_eq!(map.any(0xffb0), Some(19));
        assert_eq!(map.any(0xffeb), None);
        assert_eq!(map.spares(), [9, 8]);
        assert_eq!(Keymap::default().spares(), Vec::<u8>::new());
        assert_eq!(group_and_caps(0), (0, false));
        assert_eq!(group_and_caps(0x2000 | 2), (1, true));
        assert_eq!(group_and_caps(0x4001), (2, false));
    }

    #[test]
    fn text_is_typed_with_the_layouts_own_keys() {
        let map = us();
        assert_eq!(
            plan_text("Ha, 1!\n", &map, 0, false).unwrap(),
            [Run {
                lend: vec![],
                strokes: vec![
                    stroke(12, true),
                    stroke(11, false),
                    stroke(13, false),
                    stroke(14, false),
                    stroke(10, false),
                    stroke(10, true),
                    stroke(15, false),
                ],
            }]
        );
        // With Caps Lock on, a letter's key types its capital, and with Shift its small letter;
        // a digit's key is as it was.
        assert_eq!(
            plan_text("Ha1", &map, 0, true).unwrap()[0].strokes,
            [stroke(12, false), stroke(11, true), stroke(10, false)]
        );
        // A Windows line end is one Return, a Tab is the Tab key, and a control character is no
        // key at all.
        assert_eq!(
            plan_text("a\r\n\ta\u{7}", &map, 0, false).unwrap()[0].strokes,
            [
                stroke(11, false),
                stroke(15, false),
                stroke(16, false),
                stroke(11, false)
            ]
        );
        assert_eq!(plan_text("", &map, 0, false).unwrap(), []);
    }

    #[test]
    fn a_character_with_no_key_is_lent_a_spare_one() {
        let map = us();
        // é and → have no key here: each is lent a spare keycode, once, and typed without Shift.
        let runs = plan_text("é→é a", &map, 0, false).unwrap();
        assert_eq!(
            runs,
            [Run {
                lend: vec![(9, 0xe9), (8, 0x0100_2192)],
                strokes: vec![
                    stroke(9, false),
                    stroke(8, false),
                    stroke(9, false),
                    stroke(14, false),
                    stroke(11, false),
                ],
            }]
        );
        // More such characters than spare keys: typed in runs, the keys lent afresh in each.
        let runs = plan_text("éüö", &map, 0, false).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].lend, [(9, 0xe9), (8, 0xfc)]);
        assert_eq!(runs[0].strokes, [stroke(9, false), stroke(8, false)]);
        assert_eq!(runs[1].lend, [(9, 0xf6)]);
        assert_eq!(runs[1].strokes, [stroke(9, false)]);
        // In the second layout group the Latin letters have no key: lent, as any other.
        let greek = plan_text("a", &map, 1, false).unwrap();
        assert_eq!(greek[0].lend, [(9, 0x61)]);
        // Caps Lock doesn't touch a lent key.
        assert_eq!(
            plan_text("é", &map, 0, true).unwrap()[0].strokes,
            [stroke(9, false)]
        );
        // With no spare key, a character the layout lacks can't be typed, and nothing is.
        let mut full = us();
        full.syms[..8].fill(0x62);
        let why = plan_text("aé", &full, 0, false).unwrap_err();
        assert!(
            why.contains("no key for 'é'") && why.contains("wasn't typed"),
            "{why}"
        );
        assert!(plan_text("a1", &full, 0, false).is_ok());
    }

    #[test]
    fn a_combo_holds_its_modifiers_and_presses_the_key_itself() {
        let map = us();
        assert_eq!(
            plan_key(&parse_x11("ctrl+shift+a").unwrap(), &map, 0).unwrap(),
            Chord {
                lend: None,
                held: vec![18, 17],
                code: 11
            }
        );
        assert_eq!(
            plan_key(&parse_x11("return").unwrap(), &map, 0)
                .unwrap()
                .code,
            15
        );
        // A key the layout gives only with Shift (a German keyboard's slash, the keypad's 0) is
        // lent a spare key, so ctrl+/ is Ctrl with a slash and not Ctrl+Shift+7.
        assert_eq!(
            plan_key(&parse_x11("ctrl+/").unwrap(), &map, 0).unwrap(),
            Chord {
                lend: Some((9, 0x2f)),
                held: vec![18],
                code: 9
            }
        );
        assert_eq!(
            plan_key(&parse_x11("numpad0").unwrap(), &map, 0)
                .unwrap()
                .lend,
            Some((9, 0xffb0))
        );
        // One it doesn't have at all (F5 here) too.
        assert_eq!(
            plan_key(&parse_x11("f5").unwrap(), &map, 0).unwrap().lend,
            Some((9, 0xffc2))
        );
        // A modifier the keyboard has no key for is said by name.
        let why = plan_key(&parse_x11("win+a").unwrap(), &map, 0).unwrap_err();
        assert!(why.contains("no Super key"), "{why}");
        let mut full = us();
        full.syms[..8].fill(0x62);
        assert!(plan_key(&parse_x11("f5").unwrap(), &full, 0).is_err());
    }

    #[test]
    fn captures_fit_the_long_side_and_stay_on_the_screen() {
        assert_eq!(fit(1920, 1200, 1568), (1568, 980));
        assert_eq!(fit(1280, 800, 1568), (1280, 800));
        assert_eq!(fit(1080, 1920, 1568), (882, 1568));
        assert_eq!(fit(0, 0, 1568), (1, 1));
        assert_eq!(
            on_screen((10.2, 20.0, 300.0, 200.0), 1920, 1200),
            Some((10, 20, 300, 200))
        );
        // What hangs off the screen is cut to it.
        assert_eq!(
            on_screen((-50.0, 1100.0, 200.0, 300.0), 1920, 1200),
            Some((0, 1100, 150, 100))
        );
        assert_eq!(on_screen((2000.0, 0.0, 100.0, 100.0), 1920, 1200), None);
        assert_eq!(
            on_screen((0.0, 0.0, 0.0, 0.0), 1920, 1200),
            Some((0, 0, 1, 1))
        );
    }

    const BGRX: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        scanline_pad: 32,
        lsb_first: true,
        red: 0xff_0000,
        green: 0x00_ff00,
        blue: 0x00_00ff,
    };

    #[test]
    fn the_servers_pixels_become_rgb() {
        // The usual screen: 32 bits a pixel, least significant byte first: blue, green, red, pad.
        let data = [3, 2, 1, 0, 30, 20, 10, 0, 6, 5, 4, 0, 60, 50, 40, 0];
        assert_eq!(
            rgb_from_zpixmap(&data, 2, 2, &BGRX).unwrap(),
            [1, 2, 3, 10, 20, 30, 4, 5, 6, 40, 50, 60]
        );
        // The other byte order: pad, red, green, blue.
        let msb = PixelFormat {
            lsb_first: false,
            ..BGRX
        };
        assert_eq!(
            rgb_from_zpixmap(&[0, 1, 2, 3], 1, 1, &msb).unwrap(),
            [1, 2, 3]
        );
        // 24 bits a pixel, each row padded to 32 bits: three pixels are 9 bytes in a 12-byte row.
        let packed = PixelFormat {
            bits_per_pixel: 24,
            ..BGRX
        };
        let rows = [
            3, 2, 1, 6, 5, 4, 9, 8, 7, 0, 0, 0, //
            13, 12, 11, 16, 15, 14, 19, 18, 17, 0, 0, 0,
        ];
        assert_eq!(
            rgb_from_zpixmap(&rows, 3, 2, &packed).unwrap(),
            [
                1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19
            ]
        );
        // 16 bits a pixel, 5-6-5: full red is 255, not 248.
        let high = PixelFormat {
            bits_per_pixel: 16,
            scanline_pad: 32,
            lsb_first: true,
            red: 0xf800,
            green: 0x07e0,
            blue: 0x001f,
        };
        assert_eq!(
            rgb_from_zpixmap(&[0x00, 0xf8, 0xe0, 0x07], 2, 1, &high).unwrap(),
            [255, 0, 0, 0, 255, 0]
        );
        // A short answer, or pixels Trek doesn't read, is said and not drawn.
        assert!(rgb_from_zpixmap(&data[..12], 2, 2, &BGRX).is_err());
        let palette = PixelFormat {
            bits_per_pixel: 8,
            ..BGRX
        };
        assert!(
            rgb_from_zpixmap(&[1], 1, 1, &palette)
                .unwrap_err()
                .contains("8-bit")
        );
    }

    #[test]
    fn a_picture_is_scaled_down_by_averaging() {
        // 4x2 → 2x1: each new pixel is the average of the four it covers.
        let rgb = [
            0, 0, 0, 100, 0, 0, 200, 10, 0, 200, 30, 0, //
            100, 0, 0, 200, 0, 0, 200, 20, 0, 200, 20, 0,
        ];
        assert_eq!(downscale(&rgb, 4, 2, 2, 1), [100, 0, 0, 200, 20, 0]);
        // The same size is the same picture.
        assert_eq!(downscale(&rgb, 4, 2, 4, 2), rgb);
        // 3 → 2 columns: the middle one is shared between the two.
        let row = [0, 0, 0, 90, 90, 90, 180, 180, 180];
        assert_eq!(downscale(&row, 3, 1, 2, 1), [30, 30, 30, 150, 150, 150]);
        // A flat colour stays flat at any size.
        let flat = vec![77u8; 1920 * 3 * 4];
        let small = downscale(&flat, 1920, 4, 1568, 3);
        assert_eq!(small.len(), 1568 * 3 * 3);
        assert!(small.iter().all(|b| *b == 77));
    }

    const CALCULATOR: &str = "[Desktop Entry]\nName=Calculator\nName[de]=Rechner\nExec=gnome-calculator %U\nType=Application\nStartupWMClass=org.gnome.Calculator\n\n[Desktop Action new]\nName=New\nExec=gnome-calculator --new\n";

    #[test]
    fn desktop_entries_are_apps_to_launch() {
        let calc = parse_entry("org.gnome.Calculator.desktop", CALCULATOR).unwrap();
        assert_eq!(
            calc,
            Entry {
                id: "org.gnome.Calculator".into(),
                name: "Calculator".into(),
                // Its own command, not its "New" action's.
                exec: "gnome-calculator %U".into(),
                wm_class: "org.gnome.Calculator".into(),
            }
        );
        // A link, a hidden entry, one for a terminal and one with no command aren't.
        assert_eq!(
            parse_entry("a", "[Desktop Entry]\nType=Link\nName=A\nExec=a\n"),
            None
        );
        for off in ["Hidden=true", "Terminal=true"] {
            let text = format!("[Desktop Entry]\nType=Application\nName=A\nExec=a\n{off}\n");
            assert_eq!(parse_entry("a", &text), None, "{off}");
        }
        assert_eq!(
            parse_entry("a", "[Desktop Entry]\nType=Application\nName=A\n"),
            None
        );
        assert!(
            parse_entry(
                "a",
                "[Desktop Entry]\nType = Application\nExec = a\nTerminal=false\n"
            )
            .is_some()
        );
    }

    #[test]
    fn an_app_answers_to_its_name_its_id_and_its_program() {
        let calc = parse_entry("org.gnome.Calculator.desktop", CALCULATOR).unwrap();
        assert_eq!(entry_score(&calc, "Calculator"), 5);
        assert_eq!(entry_score(&calc, " calculator "), 5);
        assert_eq!(entry_score(&calc, "org.gnome.Calculator"), 4);
        assert_eq!(entry_score(&calc, "org.gnome.Calculator.desktop"), 4);
        assert_eq!(entry_score(&calc, "gnome-calculator"), 3);
        assert_eq!(entry_score(&calc, "editor"), 0);
        assert_eq!(entry_score(&calc, ""), 0);
        let chrome = Entry {
            id: "google-chrome".into(),
            name: "Google Chrome".into(),
            exec: "/usr/bin/google-chrome-stable %U".into(),
            wm_class: "Google-chrome".into(),
        };
        assert_eq!(entry_score(&chrome, "Google Chrome"), 5);
        assert_eq!(entry_score(&chrome, "google-chrome"), 4);
        assert_eq!(entry_score(&chrome, "google-chrome-stable"), 3);
        assert_eq!(entry_score(&chrome, "chrome"), 2);
        // A short word isn't enough to pick an app by.
        assert_eq!(entry_score(&chrome, "go"), 0);
    }

    #[test]
    fn an_exec_line_is_a_program_and_its_arguments() {
        assert_eq!(exec_argv("gnome-calculator %U"), ["gnome-calculator"]);
        assert_eq!(
            exec_argv("env GDK_BACKEND=x11 /opt/App/app --new-window %f"),
            ["env", "GDK_BACKEND=x11", "/opt/App/app", "--new-window"]
        );
        assert_eq!(
            exec_argv(r#""/opt/My App/run" --title "a \"b\" c" --icon=%i 100%%"#),
            [
                "/opt/My App/run",
                "--title",
                r#"a "b" c"#,
                "--icon=",
                "100%"
            ]
        );
        assert_eq!(exec_argv("  "), Vec::<String>::new());
        assert_eq!(exec_argv("app %"), ["app"]);
    }

    #[test]
    fn desktop_entries_are_looked_for_in_the_users_folders_first() {
        assert_eq!(
            entry_dirs(Some("/home/ada"), None, None),
            [
                "/home/ada/.local/share/applications",
                "/usr/local/share/applications",
                "/usr/share/applications",
                "/home/ada/.local/share/flatpak/exports/share/applications",
                "/var/lib/flatpak/exports/share/applications",
                "/var/lib/snapd/desktop/applications",
            ]
        );
        assert_eq!(
            entry_dirs(
                Some("/home/ada"),
                Some("/data/ada/"),
                Some("/usr/share/gnome:/usr/share::/var/lib/snapd/desktop")
            ),
            [
                "/data/ada/applications",
                "/usr/share/gnome/applications",
                "/usr/share/applications",
                "/var/lib/snapd/desktop/applications",
                "/home/ada/.local/share/flatpak/exports/share/applications",
                "/var/lib/flatpak/exports/share/applications",
            ]
        );
        assert_eq!(entry_dirs(None, Some(" "), None).len(), 4);
    }
}
