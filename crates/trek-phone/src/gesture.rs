//! The user's own mouse, trackpad and keys on the iPhone tool's picture, as touches and keys for
//! the phone.
//!
//! A press is recorded from the button going down to it coming up ([`Stroke`]): one that stayed
//! put is a tap held as long as the button was (so press-and-hold opens a long-press menu), one
//! that moved is its whole path with its timing, a rest before the first move included (that's
//! how an icon is picked up and moved, or a row of a list dragged). Scrolling over the picture
//! becomes swipes that move the content as far as the trackpad or wheel did ([`scroll`]). Keys
//! become text and the keys the phone's keyboard has ([`key`]).
//!
//! Each route then carries an [`Input`] as well as it can: Trek Driver follows a path point by
//! point (`/path`; a driver from before it gets one straight `/swipe`), the Bluetooth mouse does
//! too, and iPhone Mirroring gets one straight drag.
//!
//! Points are fractions of the screen (0…1 from its top-left corner); times are milliseconds
//! from the press.

use serde_json::{Value, json};

use crate::driver::{self, Step};
use crate::helper::MirrorCmd;
use crate::{hid, mirror};

/// The home bar the panel draws where the phone's home indicator is (fractions of the screen:
/// x from, x to, y from): a click on it goes Home, as the Home button does. A drag from it is the
/// phone's own swipe up from the bottom edge (Home, or the App Switcher when it rests partway).
pub const HOME_BAR: (f64, f64, f64) = (0.3, 0.7, 0.965);

/// `at` is on the home bar.
pub fn on_home_bar(at: (f64, f64)) -> bool {
    (HOME_BAR.0..=HOME_BAR.1).contains(&at.0) && at.1 >= HOME_BAR.2
}

/// Pointer travel, in pixels of the picture as drawn, under which a press stays a tap (or a hold).
pub const SLOP: f64 = 6.;
/// The shortest and longest press sent as a tap.
pub const MIN_HOLD_MS: u32 = 50;
pub const MAX_HOLD_MS: u32 = 10_000;
/// A press held still this long is a long press (the panel's dot shows it).
pub const LONG_PRESS_MS: u32 = 500;
/// A path runs this long at most: a drag held longer lifts there.
pub const MAX_PATH_MS: u32 = 20_000;
/// Points closer together in time merge (the earlier stays; the last one always goes).
const MIN_STEP_MS: u32 = 8;
/// A pause longer than this before a move is kept as a rest: the finger stays where it was until
/// just before the move. Without it the phone would see one slow slide across the pause, and a
/// long press before a drag would never be one.
const REST_MS: u32 = 40;

/// Swipes from scrolling stay inside this band of the screen: one up from the bottom edge goes
/// Home, one down from the top opens Notification Center, one in from a side goes back.
const BAND_X: (f64, f64) = (0.08, 0.92);
const BAND_Y: (f64, f64) = (0.14, 0.86);
/// Less scrolling than this (pixels of the picture) isn't sent.
const MIN_SCROLL: f64 = 4.;
/// One scroll is at most this many swipes (the rest of a long fling is dropped).
const MAX_SWIPES: usize = 3;
/// A key is repeated at most this often in one go (a held Delete).
const MAX_REPEAT: u32 = 50;

/// Where the finger is, and when.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub x: f64,
    pub y: f64,
    pub ms: u32,
}

impl Spot {
    fn at(&self) -> (f64, f64) {
        (self.x, self.y)
    }
}

/// A press being recorded, from the button going down.
#[derive(Debug, Clone)]
pub struct Stroke {
    spots: Vec<Spot>,
    /// The picture's size as drawn (window pixels), for how far the pointer went.
    size: (f64, f64),
    /// It went further than [`SLOP`] from the press once: a drag from then on, even if it came
    /// back.
    moved: bool,
}

impl Stroke {
    /// Pressed at `at` on a picture drawn `size` (window pixels).
    pub fn new(at: (f64, f64), size: (f64, f64)) -> Self {
        let (x, y) = clamp(at);
        Self {
            spots: vec![Spot { x, y, ms: 0 }],
            size,
            moved: false,
        }
    }

    pub fn spots(&self) -> &[Spot] {
        &self.spots
    }

    /// It's a drag, not a tap.
    pub fn moved(&self) -> bool {
        self.moved
    }

    /// Held still `ms` after the press: a long press by now.
    pub fn long_press(&self, ms: u32) -> bool {
        !self.moved && ms >= LONG_PRESS_MS
    }

    /// The pointer is at `at`, `ms` after the press.
    pub fn push(&mut self, at: (f64, f64), ms: u32) {
        self.add(at, ms, false);
    }

    fn add(&mut self, at: (f64, f64), ms: u32, end: bool) {
        let (x, y) = clamp(at);
        let first = self.spots[0];
        let travel = ((x - first.x) * self.size.0).hypot((y - first.y) * self.size.1);
        if travel >= SLOP {
            self.moved = true;
        }
        let last = *self.spots.last().expect("a stroke starts with its press");
        let ms = ms.max(last.ms);
        if (x, y) == last.at() || (!end && ms < last.ms + MIN_STEP_MS) {
            return;
        }
        if ms > last.ms + REST_MS {
            self.spots.push(Spot {
                ms: ms - MIN_STEP_MS,
                ..last
            });
        }
        self.spots.push(Spot {
            x,
            y,
            ms: ms.max(last.ms + 1),
        });
    }

    /// The button came up at `at`, `ms` after the press: what the phone gets.
    pub fn finish(mut self, at: (f64, f64), ms: u32) -> Input {
        if !self.moved {
            // Within the slop the pointer's wobble isn't the finger's: it stays on the press.
            self.add(at, ms, true);
        }
        if !self.moved {
            let first = self.spots[0];
            return Input::Tap {
                at: first.at(),
                hold_ms: ms.clamp(MIN_HOLD_MS, MAX_HOLD_MS),
            };
        }
        self.add(at, ms, true);
        let last = *self.spots.last().expect("a stroke starts with its press");
        // Held where it stopped until the button came up (a drop target wants the rest).
        if ms > last.ms {
            self.spots.push(Spot { ms, ..last });
        }
        if let Some(cut) = self.spots.iter().position(|s| s.ms > MAX_PATH_MS) {
            self.spots.truncate(cut.max(2));
        }
        Input::Path(finger(self.spots))
    }
}

/// A flick's move, once the rest before it is gone, plays in this long at most (a mouse drags
/// slower than a finger flicks, and a slow lift doesn't turn a page or throw a list).
const FLICK_MS: (u32, u32) = (80, 220);
/// A move that stopped before the lift plays in this long (enough for the phone to follow it).
const PLACE_MS: (u32, u32) = (150, 450);
/// A stop this long before the lift makes a drag one that was put down where it ended, not flung.
const PLACED_MS: u32 = 100;
/// Such a drag rests this long where it ended before the lift (no fling), or, from the bottom
/// edge, up to the longer one (resting partway up opens the App Switcher).
const END_REST_MS: u32 = 100;
const SWITCHER_REST_MS: u32 = 700;
/// A press this close to an edge (fractions of the screen) that moves away from it starts on
/// the edge itself: a swipe up from the bottom goes Home, one in from the left goes back, one
/// down from the top opens Notification Center or Control Center. The phone takes these only
/// from the very edge, which a mouse on a small picture rarely hits.
const EDGE_BOTTOM: f64 = 0.05;
const EDGE_SIDE: f64 = 0.04;
const EDGE_TOP: f64 = 0.03;

/// The path as a finger would draw it: the mouse's shape, but from the edge when it started near
/// one and moved away, without a short pause before the move (a long press stays: it picks an
/// icon up), quicker (a mouse drag is slow, and played as slowly it neither turns a page nor
/// shows for long after the button comes up), still moving at the lift when the mouse was
/// (flung) and resting briefly when it had stopped (put down where it ended).
fn finger(mut spots: Vec<Spot>) -> Vec<Spot> {
    let (Some(&first), Some(&last)) = (spots.first(), spots.last()) else {
        return spots;
    };
    if spots.len() < 2 {
        return spots;
    }
    // From the edge it was pressed by, when it moved away from it.
    let (dx, dy) = (last.x - first.x, last.y - first.y);
    let mut edge = None;
    if first.y >= 1. - EDGE_BOTTOM && dy < -0.04 && dy.abs() > dx.abs() {
        edge = Some((first.x, 0.997));
    } else if first.y <= EDGE_TOP && dy > 0.04 && dy.abs() > dx.abs() {
        edge = Some((first.x, 0.003));
    } else if first.x <= EDGE_SIDE && dx > 0.04 && dx.abs() > dy.abs() {
        edge = Some((0.003, first.y));
    } else if first.x >= 1. - EDGE_SIDE && dx < -0.04 && dx.abs() > dy.abs() {
        edge = Some((0.997, first.y));
    }
    let from_bottom = edge.is_some_and(|(_, y)| y > 0.5);
    if let Some((x, y)) = edge {
        for s in spots.iter_mut().take_while(|s| s.at() == first.at()) {
            (s.x, s.y) = (x, y);
        }
    }
    // The parts: the rest on the press, the move, the rest where it ended.
    let rest_end = spots
        .iter()
        .take_while(|s| s.at() == spots[0].at())
        .count()
        .saturating_sub(1);
    let rest = spots[rest_end].ms;
    let n = spots.len();
    let end_rest = if n >= 2 && spots[n - 1].at() == spots[n - 2].at() {
        spots[n - 1].ms - spots[n - 2].ms
    } else {
        0
    };
    let move_end = if end_rest > 0 { n - 2 } else { n - 1 };
    if move_end <= rest_end {
        return spots;
    }
    let moving = spots[move_end].ms - rest;
    let placed = end_rest >= PLACED_MS;
    let (lo, hi) = if placed { PLACE_MS } else { FLICK_MS };
    let target = moving.clamp(lo, hi.max(lo));
    let scale = f64::from(target) / f64::from(moving.max(1));
    // A long press before the move stays (it picks up what's under it); a short pause goes.
    let start = if rest >= LONG_PRESS_MS { rest } else { 0 };
    let mut out: Vec<Spot> = Vec::with_capacity(n);
    out.push(Spot { ms: 0, ..spots[0] });
    if start > 0 {
        out.push(Spot {
            ms: start,
            ..spots[rest_end]
        });
    }
    for s in &spots[rest_end + 1..=move_end] {
        let ms = start + (f64::from(s.ms - rest) * scale).round() as u32;
        let ms = ms.max(out.last().map_or(0, |l| l.ms + 1));
        out.push(Spot { ms, ..*s });
    }
    if placed {
        let hold = end_rest.min(if from_bottom {
            SWITCHER_REST_MS
        } else {
            END_REST_MS
        });
        let end = *out.last().expect("a move has an end");
        out.push(Spot {
            ms: end.ms + hold,
            ..end
        });
    }
    out
}

fn clamp((x, y): (f64, f64)) -> (f64, f64) {
    (x.clamp(0., 1.), y.clamp(0., 1.))
}

/// One thing for the phone from the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A touch at one spot, held `hold_ms` (half a second or more is a long press).
    Tap { at: (f64, f64), hold_ms: u32 },
    /// A touch that follows the spots (times rising from 0, the touch-down) and lifts at the last.
    Path(Vec<Spot>),
    /// A straight swipe over `ms`: scrolling.
    Swipe {
        from: (f64, f64),
        to: (f64, f64),
        ms: u32,
    },
    /// Text, typed where the phone's keyboard is.
    Text(String),
    /// A key pressed `times` times, named as [`hid::key`] names it ("enter", "backspace",
    /// "forwarddelete", "tab", "shift+tab", "escape", "up", "alt+left"…).
    Key { name: String, times: u32 },
}

impl Input {
    /// It touches the screen (the Bluetooth route needs AssistiveTouch's pointer for it).
    pub fn touches(&self) -> bool {
        matches!(
            self,
            Input::Tap { .. } | Input::Path(_) | Input::Swipe { .. }
        )
    }

    /// Take `next` into this one when they go as one: text typed on, a key pressed again.
    pub fn merge(&mut self, next: &Input) -> bool {
        match (self, next) {
            (Input::Text(a), Input::Text(b)) => {
                a.push_str(b);
                true
            }
            (Input::Key { name, times }, Input::Key { name: n, times: t })
                if name == n && *times + t <= MAX_REPEAT =>
            {
                *times += t;
                true
            }
            _ => false,
        }
    }

    /// Trek Driver's requests for it, `screen` in points. `takes` says what the driver on the
    /// phone does (its `/status` features): `path` for `/path`, `keys` for the arrows and
    /// repeats in `/key`. What it can't do is left out.
    pub fn driver(&self, screen: (f64, f64), takes: &dyn Fn(&str) -> bool) -> Vec<Step> {
        let pt = |at: (f64, f64)| driver::to_points(at, screen);
        let swipe = |from: (f64, f64), to: (f64, f64), ms: u32| {
            let ((x1, y1), (x2, y2)) = (pt(from), pt(to));
            Step::Post(
                "/swipe",
                json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2, "duration_ms": ms.clamp(60, 5000) }),
            )
        };
        match self {
            Input::Tap { at, hold_ms } => {
                let (x, y) = pt(*at);
                vec![Step::Post(
                    "/tap",
                    json!({ "x": x, "y": y, "hold_ms": hold_ms }),
                )]
            }
            Input::Path(spots) if takes("path") => {
                let points: Vec<Value> = spots
                    .iter()
                    .map(|s| {
                        let (x, y) = pt(s.at());
                        json!({ "x": x, "y": y, "t_ms": s.ms })
                    })
                    .collect();
                vec![Step::Post("/path", json!({ "points": points }))]
            }
            // An older driver: one straight swipe over the part that moved.
            Input::Path(spots) => match (spots.first(), spots.last()) {
                (Some(a), Some(b)) => vec![swipe(a.at(), b.at(), b.ms - rest(spots))],
                _ => vec![],
            },
            Input::Swipe { from, to, ms } => vec![swipe(*from, *to, *ms)],
            Input::Text(text) if text.is_empty() => vec![],
            Input::Text(text) => vec![Step::Post("/type", json!({ "text": text }))],
            Input::Key { name, times } => {
                let keys = takes("keys");
                let Some(key) = driver_key(name, keys) else {
                    return vec![];
                };
                if keys {
                    vec![Step::Post("/key", json!({ "key": key, "count": times }))]
                } else {
                    (0..*times)
                        .map(|_| Step::Post("/key", json!({ "key": key })))
                        .collect()
                }
            }
        }
    }

    /// The Bluetooth keyboard and mouse's reports for it.
    pub fn hid(&self) -> Result<Vec<hid::Step>, String> {
        Ok(match self {
            Input::Tap { at, hold_ms } => hid::tap(at.0, at.1, *hold_ms),
            Input::Path(spots) => {
                let points: Vec<_> = spots.iter().map(|s| (s.x, s.y, s.ms)).collect();
                hid::path(&points)
            }
            Input::Swipe { from, to, ms } => hid::swipe(*from, *to, *ms),
            Input::Text(text) => hid::type_text(text)?,
            Input::Key { name, times } => {
                let once = hid::key(name)?;
                (0..*times).flat_map(|_| once.clone()).collect()
            }
        })
    }

    /// iPhone Mirroring's commands for it (points are fractions of its window, which its picture
    /// is): a path goes as one straight drag over the part that moved.
    pub fn mirror(&self) -> Result<Vec<MirrorCmd>, String> {
        Ok(match self {
            Input::Tap { at, hold_ms } => vec![MirrorCmd::Tap {
                x: at.0,
                y: at.1,
                hold_ms: *hold_ms,
            }],
            Input::Path(spots) => match (spots.first(), spots.last()) {
                (Some(a), Some(b)) => vec![MirrorCmd::Drag {
                    from: a.at(),
                    to: b.at(),
                    ms: (b.ms - rest(spots)).clamp(60, 5000),
                }],
                _ => vec![],
            },
            Input::Swipe { from, to, ms } => vec![MirrorCmd::Drag {
                from: *from,
                to: *to,
                ms: *ms,
            }],
            Input::Text(text) => vec![MirrorCmd::Text(text.clone())],
            Input::Key { name, times } => {
                let (code, flags) = mirror::key(name)?;
                vec![MirrorCmd::Key(code, flags); *times as usize]
            }
        })
    }
}

/// How long a path rests where it was pressed before it first moves.
fn rest(spots: &[Spot]) -> u32 {
    let Some(first) = spots.first() else {
        return 0;
    };
    spots
        .iter()
        .take_while(|s| s.at() == first.at())
        .last()
        .map_or(0, |s| s.ms)
}

/// The name Trek Driver's `/key` takes for `name`, if it has the key: Return, Delete, Tab and
/// Escape always; the arrows from the driver with `keys` (forward delete does nothing on the
/// phone through it, so it isn't sent). Home, End and keys
/// with modifiers it doesn't have (its "home" is the Home Screen).
fn driver_key(name: &str, keys: bool) -> Option<String> {
    match name {
        "enter" | "backspace" | "tab" | "escape" => Some(name.into()),
        "up" | "down" | "left" | "right" if keys => Some(name.into()),
        _ => None,
    }
}

/// A scroll of `delta` (pixels of the picture as drawn `size`, as GPUI reports it: positive y
/// moves the content down, the Mac's natural scrolling already applied) over `at`: swipes that
/// move the content as far, as a finger would, kept inside [`BAND_X`] × [`BAND_Y`] and split
/// when longer than that.
pub fn scroll(at: (f64, f64), delta: (f64, f64), size: (f64, f64)) -> Vec<Input> {
    if delta.0.hypot(delta.1) < MIN_SCROLL || size.0 < 1. || size.1 < 1. {
        return vec![];
    }
    let (mut dx, mut dy) = (delta.0 / size.0, delta.1 / size.1);
    let (span_x, span_y) = (BAND_X.1 - BAND_X.0, BAND_Y.1 - BAND_Y.0);
    let mut out = vec![];
    while out.len() < MAX_SWIPES && (dx * size.0).hypot(dy * size.1) >= MIN_SCROLL {
        let fit = 1f64
            .min(span_x / dx.abs().max(1e-9))
            .min(span_y / dy.abs().max(1e-9));
        let (sx, sy) = (dx * fit, dy * fit);
        // From the pointer, moved in so the whole swipe stays in the band.
        let x = at.0.clamp(BAND_X.0 - sx.min(0.), BAND_X.1 - sx.max(0.));
        let y = at.1.clamp(BAND_Y.0 - sy.min(0.), BAND_Y.1 - sy.max(0.));
        let length = (sx * size.0).hypot(sy * size.1);
        out.push(Input::Swipe {
            from: (x, y),
            to: (x + sx, y + sy),
            ms: (length * 0.75).clamp(100., 450.) as u32,
        });
        dx -= sx;
        dy -= sy;
    }
    out
}

/// What a key pressed on the Mac does while the picture has the keys.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    /// Trek's own: a shortcut (⌘ or Ctrl held), or a key the phone has no use for.
    Trek,
    /// ⌘V: the Mac's clipboard is typed on the phone.
    Paste,
    /// Escape: to the phone, and the keys go back to Trek.
    Escape,
    /// This goes to the phone.
    Phone(Input),
}

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    /// ⌘ on a Mac.
    pub platform: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

/// What the key GPUI names `name` (with `typed`, the character it types, if any) does.
pub fn key(name: &str, typed: Option<&str>, mods: Mods) -> Key {
    if mods.platform && !mods.control && !mods.alt && name == "v" {
        return Key::Paste;
    }
    if mods.platform || mods.control {
        return Key::Trek;
    }
    let press = |key: &str| {
        let mut name = String::new();
        if mods.alt {
            name += "alt+";
        }
        if mods.shift {
            name += "shift+";
        }
        Key::Phone(Input::Key {
            name: name + key,
            times: 1,
        })
    };
    match name {
        "escape" => Key::Escape,
        "enter" | "backspace" | "tab" | "up" | "down" | "left" | "right" | "home" | "end"
        | "pageup" | "pagedown" => press(name),
        // GPUI's delete is the forward one.
        "delete" => press("forwarddelete"),
        "space" => Key::Phone(Input::Text(" ".into())),
        _ => match typed {
            Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                Key::Phone(Input::Text(t.into()))
            }
            _ => Key::Trek,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture drawn 300×652 window pixels (a 440×956-point phone).
    const SIZE: (f64, f64) = (300., 652.);

    fn px(x: f64, y: f64) -> (f64, f64) {
        (x / SIZE.0, y / SIZE.1)
    }

    #[test]
    fn a_click_is_a_tap_held_as_long_as_the_button_was() {
        let mut s = Stroke::new(px(150., 300.), SIZE);
        // The pointer's wobble within the slop isn't a drag.
        s.push(px(152., 301.), 40);
        assert!(!s.moved());
        assert_eq!(
            s.finish(px(153., 302.), 90),
            Input::Tap {
                at: px(150., 300.),
                hold_ms: 90
            }
        );
        // A very quick click still reaches the phone as a touch.
        let s = Stroke::new(px(10., 10.), SIZE);
        assert!(matches!(
            s.finish(px(10., 10.), 3),
            Input::Tap { hold_ms: 50, .. }
        ));
    }

    #[test]
    fn a_press_held_still_is_a_long_press() {
        let s = Stroke::new(px(150., 300.), SIZE);
        assert!(!s.long_press(300) && s.long_press(500));
        let Input::Tap { hold_ms, .. } = s.finish(px(150., 300.), 820) else {
            panic!("a hold is a tap held")
        };
        assert_eq!(hold_ms, 820);
        assert!(hold_ms >= LONG_PRESS_MS);
        // Held for ever: as long as a tap goes.
        let s = Stroke::new(px(150., 300.), SIZE);
        assert!(matches!(
            s.finish(px(150., 300.), 60_000),
            Input::Tap {
                hold_ms: MAX_HOLD_MS,
                ..
            }
        ));
    }

    #[test]
    fn a_drag_keeps_its_rest_before_the_move_and_at_the_end() {
        // Press on an icon, hold 700 ms (the phone's long press), drag it 90 px right, rest on
        // the drop spot 300 ms, let go.
        let mut s = Stroke::new(px(60., 200.), SIZE);
        let mut ms = 700;
        for step in 1..=9 {
            s.push(px(60. + 10. * f64::from(step), 200.), ms);
            ms += 16;
        }
        let Input::Path(spots) = s.finish(px(150., 200.), ms + 300) else {
            panic!("a drag is a path")
        };
        assert_eq!(
            spots[0],
            Spot {
                x: 0.2,
                y: 200. / 652.,
                ms: 0
            }
        );
        // Still on the press until just before the first move: a long press picks it up.
        assert_eq!((spots[1].at(), spots[1].ms), (spots[0].at(), 692));
        assert_eq!(rest(&spots), 692);
        assert!(spots[2].ms > 692 && spots[2].at() != spots[0].at());
        assert!(spots.windows(2).all(|w| w[1].ms > w[0].ms), "times rise");
        // Put down on the drop spot: it rests there a moment (no fling), not the whole 300 ms.
        let (last, before) = (spots[spots.len() - 1], spots[spots.len() - 2]);
        assert_eq!((last.at(), before.at()), (px(150., 200.), px(150., 200.)));
        assert_eq!(last.ms - before.ms, END_REST_MS);
    }

    /// The mouse's slow drag across a page, still moving when the button came up: played as a
    /// finger's flick, quick and moving at the lift, so the page turns and the phone answers
    /// right after the button comes up.
    #[test]
    fn a_slow_drag_let_go_while_moving_is_played_as_a_flick() {
        let mut s = Stroke::new(px(250., 300.), SIZE);
        // 200 px left over 900 ms: a mouse's pace.
        for step in 1..=45 {
            s.push(px(250. - 4.4 * f64::from(step), 300.), 20 * step);
        }
        let Input::Path(spots) = s.finish(px(50., 300.), 905) else {
            panic!("a drag is a path")
        };
        let (first, last) = (spots[0], *spots.last().unwrap());
        assert_eq!((first.ms, last.at()), (0, px(50., 300.)));
        assert!(last.ms <= FLICK_MS.1, "played in {} ms", last.ms);
        assert_ne!(
            spots[spots.len() - 2].at(),
            last.at(),
            "still moving at the lift"
        );
    }

    /// A short pause before the move isn't the user's: the move starts at once.
    #[test]
    fn a_short_pause_before_the_move_goes() {
        let mut s = Stroke::new(px(100., 300.), SIZE);
        s.push(px(140., 300.), 250);
        s.push(px(180., 300.), 280);
        let Input::Path(spots) = s.finish(px(200., 300.), 300) else {
            panic!()
        };
        assert_eq!(rest(&spots), 0);
        assert_ne!(spots[1].at(), spots[0].at());
        assert!(spots.last().unwrap().ms <= FLICK_MS.1);
    }

    /// Exiting an app: a swipe up from near the bottom starts on the very edge, as the phone
    /// needs; let go while moving it goes Home, rested partway up it keeps the rest (the App
    /// Switcher). The same for the other edges.
    #[test]
    fn a_swipe_from_near_an_edge_starts_on_it() {
        let up = |end_rest: u32| {
            let mut s = Stroke::new(px(150., 640.), SIZE);
            s.push(px(150., 500.), 120);
            s.push(px(150., 350.), 240);
            match s.finish(px(150., 350.), 240 + end_rest) {
                Input::Path(spots) => spots,
                _ => panic!("a drag is a path"),
            }
        };
        let home = up(0);
        assert_eq!(home[0].y, 0.997, "from the bottom edge");
        assert!(home.last().unwrap().ms <= FLICK_MS.1);
        let switcher = up(450);
        assert_eq!(switcher[0].y, 0.997);
        let n = switcher.len();
        assert_eq!(
            switcher[n - 1].ms - switcher[n - 2].ms,
            450,
            "rested partway up"
        );
        // In from the left edge (back), down from the top: on the edge too.
        let mut s = Stroke::new(px(8., 300.), SIZE);
        s.push(px(120., 300.), 100);
        let Input::Path(back) = s.finish(px(160., 300.), 140) else {
            panic!()
        };
        assert_eq!(back[0].x, 0.003);
        let mut s = Stroke::new(px(150., 10.), SIZE);
        s.push(px(150., 200.), 100);
        let Input::Path(down) = s.finish(px(150., 260.), 140) else {
            panic!()
        };
        assert_eq!(down[0].y, 0.003);
        // Along the bottom (a tab bar's swipe), or from the middle: where it was pressed.
        let mut s = Stroke::new(px(50., 640.), SIZE);
        s.push(px(250., 635.), 150);
        let Input::Path(along) = s.finish(px(250., 635.), 160) else {
            panic!()
        };
        assert_eq!(along[0].at(), px(50., 640.));
    }

    #[test]
    fn moves_closer_than_a_step_merge_but_the_release_always_counts() {
        let mut s = Stroke::new(px(100., 100.), SIZE);
        // A 1000 Hz mouse.
        for ms in 1..=40 {
            s.push(px(100. + f64::from(ms), 100.), ms);
        }
        assert!(s.moved());
        assert!(s.spots().len() <= 6, "{:?}", s.spots());
        assert!(
            s.spots()
                .windows(2)
                .all(|w| w[1].ms >= w[0].ms + MIN_STEP_MS)
        );
        let Input::Path(spots) = s.finish(px(141., 100.), 41) else {
            panic!()
        };
        assert_eq!(spots.last().unwrap().at(), px(141., 100.));
        // Off the picture: held to its edge.
        let mut s = Stroke::new(px(290., 100.), SIZE);
        s.push((1.3, -0.2), 30);
        assert_eq!(s.spots().last().unwrap().at(), (1., 0.));
    }

    #[test]
    fn a_drag_held_past_the_limit_lifts_there() {
        let mut s = Stroke::new(px(100., 100.), SIZE);
        s.push(px(140., 100.), 100);
        s.push(px(150., 100.), MAX_PATH_MS + 5000);
        let Input::Path(spots) = s.finish(px(150., 100.), MAX_PATH_MS + 9000) else {
            panic!()
        };
        assert!(spots.last().unwrap().ms <= MAX_PATH_MS);
        assert!(spots.len() >= 2);
    }

    #[test]
    fn the_driver_follows_a_path_or_an_old_one_swipes() {
        let screen = (440., 956.);
        let path = Input::Path(vec![
            Spot {
                x: 0.5,
                y: 0.5,
                ms: 0,
            },
            Spot {
                x: 0.5,
                y: 0.5,
                ms: 592,
            },
            Spot {
                x: 0.6,
                y: 0.5,
                ms: 600,
            },
            Spot {
                x: 0.75,
                y: 0.5,
                ms: 900,
            },
        ]);
        let new = path.driver(screen, &|f| f == "path");
        assert_eq!(new.len(), 1);
        let Step::Post(endpoint, body) = &new[0] else {
            panic!()
        };
        assert_eq!(*endpoint, "/path");
        assert_eq!(
            body["points"][0],
            json!({"x": 220.0, "y": 478.0, "t_ms": 0})
        );
        assert_eq!(body["points"][1]["t_ms"], 592);
        assert_eq!(
            body["points"][3],
            json!({"x": 330.0, "y": 478.0, "t_ms": 900})
        );
        // Before /path: one swipe from the press to the lift over the part that moved.
        let old = path.driver(screen, &|_| false);
        assert_eq!(
            old,
            vec![Step::Post(
                "/swipe",
                json!({"x1": 220.0, "y1": 478.0, "x2": 330.0, "y2": 478.0, "duration_ms": 308})
            )]
        );
        let tap = Input::Tap {
            at: (0.25, 0.5),
            hold_ms: 700,
        };
        assert_eq!(
            tap.driver(screen, &|_| false),
            vec![Step::Post(
                "/tap",
                json!({"x": 110.0, "y": 478.0, "hold_ms": 700})
            )]
        );
    }

    #[test]
    fn keys_the_driver_lacks_are_left_out_and_repeats_go_as_it_can() {
        let screen = (440., 956.);
        let up = Input::Key {
            name: "up".into(),
            times: 1,
        };
        assert!(
            up.driver(screen, &|_| false).is_empty(),
            "no arrows before keys"
        );
        assert_eq!(
            up.driver(screen, &|f| f == "keys"),
            vec![Step::Post("/key", json!({"key": "up", "count": 1}))]
        );
        let delete = Input::Key {
            name: "backspace".into(),
            times: 3,
        };
        assert_eq!(delete.driver(screen, &|_| false).len(), 3);
        assert_eq!(
            delete.driver(screen, &|f| f == "keys"),
            vec![Step::Post("/key", json!({"key": "backspace", "count": 3}))]
        );
        // The driver's "home" is the Home Screen, not the Home key.
        for name in ["home", "shift+tab", "alt+left", "forwarddelete"] {
            let key = Input::Key {
                name: name.into(),
                times: 1,
            };
            assert!(key.driver(screen, &|_| true).is_empty(), "{name}");
        }
        assert_eq!(
            Input::Text("hi".into()).driver(screen, &|_| false),
            vec![Step::Post("/type", json!({"text": "hi"}))]
        );
    }

    #[test]
    fn the_other_routes_get_holds_and_paths_as_they_can() {
        let path = Input::Path(vec![
            Spot {
                x: 0.5,
                y: 0.5,
                ms: 0,
            },
            Spot {
                x: 0.5,
                y: 0.5,
                ms: 600,
            },
            Spot {
                x: 0.5,
                y: 0.3,
                ms: 800,
            },
        ]);
        // Bluetooth follows it, the hold included.
        let steps = path.hid().unwrap();
        assert!(steps.contains(&hid::Step::Wait(600)), "{steps:?}");
        // iPhone Mirroring drags straight over the moving part.
        assert_eq!(
            path.mirror().unwrap(),
            vec![MirrorCmd::Drag {
                from: (0.5, 0.5),
                to: (0.5, 0.3),
                ms: 200
            }]
        );
        let hold = Input::Tap {
            at: (0.1, 0.2),
            hold_ms: 900,
        };
        assert_eq!(
            hold.mirror().unwrap(),
            vec![MirrorCmd::Tap {
                x: 0.1,
                y: 0.2,
                hold_ms: 900
            }]
        );
        assert!(hold.hid().unwrap().contains(&hid::Step::Wait(900)));
        let keys = Input::Key {
            name: "enter".into(),
            times: 2,
        };
        assert_eq!(keys.mirror().unwrap().len(), 2);
        assert!(Input::Text("é".into()).hid().is_err(), "US-ASCII only");
    }

    #[test]
    fn scrolling_moves_the_content_as_far_and_the_same_way() {
        // Two fingers up 120 px on the trackpad (natural): the content goes up, so does the
        // finger on the phone.
        let swipes = scroll(px(150., 400.), (0., -120.), SIZE);
        let [Input::Swipe { from, to, ms }] = swipes.as_slice() else {
            panic!("{swipes:?}")
        };
        assert_eq!(*from, px(150., 400.));
        assert!((to.1 - from.1 - -120. / SIZE.1).abs() < 1e-9);
        assert_eq!(to.0, from.0);
        assert_eq!(*ms, 100);
        // Sideways: the Home Screen's next page.
        let [Input::Swipe { from, to, .. }] = scroll(px(150., 400.), (-200., 0.), SIZE)[..] else {
            panic!()
        };
        assert!(to.0 < from.0 && to.1 == from.1);
        // A nudge isn't sent.
        assert!(scroll(px(150., 400.), (1., 2.), SIZE).is_empty());
    }

    #[test]
    fn scroll_swipes_stay_clear_of_the_edges_and_split_when_long() {
        // Down from near the top: started inside the band, never from the top edge.
        let [Input::Swipe { from, to, .. }] = scroll(px(150., 10.), (0., 100.), SIZE)[..] else {
            panic!()
        };
        assert!(from.1 >= BAND_Y.0 && to.1 <= BAND_Y.1);
        // Up from the bottom edge (which would go Home): moved in.
        let [Input::Swipe { from, to, .. }] = scroll(px(150., 650.), (0., -100.), SIZE)[..] else {
            panic!()
        };
        assert!(from.1 <= BAND_Y.1 && to.1 >= BAND_Y.0);
        // A fling of two screens: in band-long pieces, at most a few.
        let swipes = scroll(px(150., 326.), (0., -1300.), SIZE);
        assert_eq!(swipes.len(), MAX_SWIPES);
        for s in &swipes {
            let Input::Swipe { from, to, ms } = s else {
                panic!()
            };
            assert!(
                from.1 <= BAND_Y.1 + 1e-9 && to.1 >= BAND_Y.0 - 1e-9,
                "{s:?}"
            );
            assert!(*ms <= 450);
        }
    }

    #[test]
    fn keys_go_to_the_phone_and_shortcuts_stay_treks() {
        let none = Mods::default();
        let key_named = |n: &str| {
            Key::Phone(Input::Key {
                name: n.into(),
                times: 1,
            })
        };
        assert_eq!(
            key("a", Some("a"), none),
            Key::Phone(Input::Text("a".into()))
        );
        let shift = Mods {
            shift: true,
            ..none
        };
        assert_eq!(
            key("a", Some("A"), shift),
            Key::Phone(Input::Text("A".into()))
        );
        assert_eq!(
            key("space", Some(" "), none),
            Key::Phone(Input::Text(" ".into()))
        );
        assert_eq!(key("enter", None, none), key_named("enter"));
        assert_eq!(key("backspace", None, none), key_named("backspace"));
        assert_eq!(key("delete", None, none), key_named("forwarddelete"));
        assert_eq!(key("tab", None, shift), key_named("shift+tab"));
        assert_eq!(key("up", None, none), key_named("up"));
        assert_eq!(key("escape", None, none), Key::Escape);
        let cmd = Mods {
            platform: true,
            ..none
        };
        assert_eq!(key("v", Some("v"), cmd), Key::Paste);
        assert_eq!(key("j", Some("j"), cmd), Key::Trek, "⌘J is Trek's");
        let ctrl = Mods {
            control: true,
            ..none
        };
        assert_eq!(key("c", Some("c"), ctrl), Key::Trek);
        assert_eq!(key("f5", None, none), Key::Trek);
        // Each route then sends the key as it can: hid and iPhone Mirroring know the names.
        for name in [
            "enter",
            "backspace",
            "forwarddelete",
            "shift+tab",
            "up",
            "alt+left",
        ] {
            assert!(hid::key(name).is_ok(), "{name}");
            assert!(mirror::key(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn typing_merges_while_it_waits() {
        let mut a = Input::Text("he".into());
        assert!(a.merge(&Input::Text("llo".into())));
        assert_eq!(a, Input::Text("hello".into()));
        let mut d = Input::Key {
            name: "backspace".into(),
            times: 1,
        };
        assert!(d.merge(&d.clone()));
        assert!(!d.merge(&Input::Key {
            name: "enter".into(),
            times: 1
        }));
        assert!(!a.merge(&d), "text and keys keep their order");
        assert_eq!(
            d,
            Input::Key {
                name: "backspace".into(),
                times: 2
            }
        );
        assert!(
            !Input::Tap {
                at: (0., 0.),
                hold_ms: 50
            }
            .merge(&Input::Tap {
                at: (0., 0.),
                hold_ms: 50
            })
        );
    }
}
