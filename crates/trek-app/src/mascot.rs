//! While an agent works: a trail word that changes every few seconds ("Breaking trail…") and a
//! little hiker walking back and forth along a dotted trail above the composer (`working_bar`).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::time::Duration;

/// What Trek says an agent is doing while it works, instead of a plain "Working…".
pub const WORDS: &[&str] = &[
    "Trailblazing",
    "Switchbacking",
    "Summiting",
    "Scrambling",
    "Bushwhacking",
    "Route-finding",
    "Stacking cairns",
    "Reading the map",
    "Checking the compass",
    "Fording the creek",
    "Gaining elevation",
    "Traversing",
    "Acclimatizing",
    "Scouting ahead",
    "Marking the trail",
    "Crossing the ridge",
    "Breaking trail",
    "Boulder-hopping",
    "Topping out",
    "Following the cairns",
    "Charting a course",
    "Wayfinding",
    "Lighting the beacon",
    "Refilling canteens",
    "Taking the scenic route",
    "Contouring",
    "Peak-bagging",
    "Setting up base camp",
    "Lacing boots",
    "Checking the forecast",
    "Glissading",
    "Hiking it out",
];

/// How long each trail word stays before the next.
pub const WORD_EVERY: u64 = 4;

/// The trail word for a turn that has run `secs` seconds ("Breaking trail"): it changes every
/// [`WORD_EVERY`] seconds, and `seed` (the thread) keeps threads from moving in lockstep.
pub fn word(seed: &str, secs: u64) -> &'static str {
    let n = WORDS.len() as u64;
    let h = seed.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    // Each thread strides through the list from its own start, by a stride that shares no factor
    // with its length: every word comes round once a lap, never twice in a row.
    let gcd = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let stride = (1..n).map(|k| (h >> 7).wrapping_add(k) % n).find(|&s| s > 1 && gcd(s, n) == 1).unwrap_or(1);
    let i = (h % n + (secs / WORD_EVERY) % n * stride) % n;
    WORDS[i as usize]
}

/// The hiker's walk: one lap there and back in `LAP`.
const LAP: Duration = Duration::from_secs(16);
/// One sprite pixel, in points. 1.5pt is 3 device pixels on Retina, so edges stay crisp.
const PX: f32 = 1.5;
const SPRITE_W: usize = 14;
const SPRITE_H: usize = 16;
/// Height of the trail with the hiker on it.
pub const HEIGHT: f32 = SPRITE_H as f32 * PX + 4.;

/// Pixel hiker facing right: hat, ember backpack with a bedroll, walking pole.
const TOP: [&str; 12] = [
    "......hhh.....",
    ".....hhhhh....",
    "....HHHHHHHH..",
    ".....ssses....",
    ".....sssss....",
    "..rr..ss......",
    ".bbbbcccc.....",
    ".bBbbccccC....",
    ".bBbbcccCsL...",
    ".bBbbcccC.....",
    "..bbbcccC.....",
    "....pppp......",
];
/// Four-frame walk: near leg forward, passing, far leg forward, passing.
const LEGS: [[&str; 4]; 4] = [
    ["....PP.pp.....", "...PP...pp....", "...P.....p....", "..kk.....kk..."],
    ["....Pppp......", ".....Pp.......", ".....Pp.......", ".....kkk......"],
    ["....pp.PP.....", "...pp...PP....", "...p.....P....", "..kk.....kk..."],
    ["....pPPP......", ".....pP.......", ".....pP.......", ".....kkk......"],
];
/// Pole from under the grip to the ground: planted ahead on contact frames.
const POLE: [(f32, f32, f32, f32); 4] = [(10., 9., 13., 15.), (10., 9., 11., 15.), (10., 9., 13., 15.), (10., 9., 11., 15.)];

fn color(c: char) -> Option<u32> {
    Some(match c {
        'h' => 0xB07A45,
        'H' => 0x8A5A2E,
        's' => 0xE9C39A,
        'e' => 0x2A2A2E,
        'c' => 0x5E8C7B,
        'C' => 0x4A7262,
        'b' => 0xFF7A3D,
        'B' => 0xC9551F,
        'r' => 0xF2E3C6,
        'p' => 0x55606F,
        'P' => 0x3A424E,
        'k' => 0x2B2B30,
        'l' => 0xA8A29E,
        'L' => 0x6B6763,
        _ => return None,
    })
}

/// `TREK_FORCE_ACTIVE=1` runs the working animation as if the window were frontmost, and keeps
/// drawing the window while macOS hides it (covered, or on another Space), so the animation's full
/// cost, drawing included, can be measured while Trek stays in the background. A measurement aid,
/// not a setting.
pub fn force_active() -> bool {
    static FORCE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("TREK_FORCE_ACTIVE").is_ok_and(|v| v == "1"));
    *FORCE
}

/// Frames per second for the working animation. Pixel art reads fine at this rate, and each frame
/// still redraws the window (the other views come from GPUI's cache), so 15 instead of 60 is a
/// quarter of the CPU.
pub const FPS: u64 = 15;

/// A dotted trail the width of its parent with the hiker walking it. `clock` is seconds since the
/// turn started; the caller re-renders at [`FPS`] while it wants motion (see `WorkingBar`).
pub fn trail(clock: f32, still: bool, cx: &App) -> AnyElement {
    let dots = cx.theme().foreground.opacity(0.16);
    let (pos, frame, right) = if still {
        (0.06, 1, true)
    } else {
        let lap = LAP.as_secs_f32();
        let t = (clock % lap) / lap;
        // Triangle wave: walk right for half the lap, back left for the other half.
        let (raw, right) = if t < 0.5 { (t * 2., true) } else { (2. - t * 2., false) };
        // Ease at the ends so the hiker slows to a stop, turns, and sets off again.
        let pos = 0.5 - 0.5 * (std::f32::consts::PI * raw).cos();
        let speed = (std::f32::consts::PI * raw).sin();
        let frame = if speed < 0.15 { 1 } else { (clock * 7.) as usize % 4 };
        (pos, frame, right)
    };
    div().h(px(HEIGHT)).w_full().child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, pos, frame, right, dots, window)).size_full()).into_any_element()
}

/// How often the hiker, waiting, looks the other way.
const LOOK_EVERY: f32 = 3.;

/// The trail with the hiker standing at a fork, waiting for others to come back: it looks one way
/// down the trail, then the other, every few seconds. Drawn at a second's pace (`clock` in
/// seconds since the wait began); `still`, it faces ahead.
pub fn waiting(clock: f32, still: bool, cx: &App) -> AnyElement {
    let dots = cx.theme().foreground.opacity(0.16);
    let right = still || (clock / LOOK_EVERY) as u64 % 2 == 0;
    div().h(px(HEIGHT)).w_full().child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, 0.12, 1, right, dots, window)).size_full()).into_any_element()
}

fn paint(b: Bounds<Pixels>, pos: f32, frame: usize, right: bool, dots: Hsla, window: &mut Window) {
    let w = b.size.width.as_f32();
    let ground = (b.origin.y.as_f32() + HEIGHT - 2.).round();
    // Dotted trail.
    let mut x = b.origin.x.as_f32() + 2.;
    while x < b.origin.x.as_f32() + w - 2. {
        window.paint_quad(fill(Bounds::new(point(px(x), px(ground - 1.)), size(px(2.), px(2.))), dots).corner_radii(px(1.)));
        x += 7.;
    }
    let sprite_w = SPRITE_W as f32 * PX;
    let travel = (w - sprite_w - 8.).max(0.);
    // Snap to the sprite's pixel grid so it never blurs mid-pixel.
    let left = ((b.origin.x.as_f32() + 4. + travel * pos) / PX).round() * PX;
    sprite(left, ground - 1., PX, frame, right, window);
}

/// The hiker standing still with its feet on `ground` and its left edge at `left`, drawn with
/// `cell`-point pixels (Basecamp marks "now" on its profile with it).
pub fn stand(left: f32, ground: f32, cell: f32, window: &mut Window) {
    sprite((left / cell).round() * cell, ground, cell, 1, true, window);
}

/// Width and height of the hiker drawn with `cell`-point pixels.
pub fn size_at(cell: f32) -> (f32, f32) {
    (SPRITE_W as f32 * cell, SPRITE_H as f32 * cell)
}

fn sprite(left: f32, ground: f32, cell_px: f32, frame: usize, right: bool, window: &mut Window) {
    let top = ground - SPRITE_H as f32 * cell_px;
    let cell = |x: f32, y: f32, rgb: u32, window: &mut Window| {
        let col = if right { x } else { SPRITE_W as f32 - 1. - x };
        window.paint_quad(fill(Bounds::new(point(px(left + col * cell_px), px(top + y * cell_px)), size(px(cell_px), px(cell_px))), gpui_kit::rgb(rgb)));
    };
    for (y, row) in TOP.iter().chain(LEGS[frame].iter()).enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if let Some(rgb) = color(ch) {
                cell(x as f32, y as f32, rgb, window);
            }
        }
    }
    let (x0, y0, x1, y1) = POLE[frame];
    let n = (y1 - y0) as usize;
    for i in 0..=n {
        let f = i as f32 / n as f32;
        cell((x0 + (x1 - x0) * f).round(), y0 + i as f32, 0xA8A29E, window);
    }
}

#[cfg(test)]
mod tests {
    use super::{WORD_EVERY, WORDS, word};

    #[test]
    fn trail_words_hold_then_move_on() {
        // The same word for the whole step, from its first second to its last.
        assert_eq!(word("thread-a", 0), word("thread-a", WORD_EVERY - 1));
        assert_eq!(word("thread-a", 8), word("thread-a", 8 + WORD_EVERY - 1));
        // Over a few minutes it changes often, and never stays on one word for long.
        let steps: Vec<&str> = (0..60).map(|i| word("thread-a", i * WORD_EVERY)).collect();
        let changes = steps.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(changes, 59, "a new word every step: {steps:?}");
        // The whole list comes round in a lap, and threads don't move in lockstep.
        let mut lap: Vec<&str> = steps[..WORDS.len()].to_vec();
        lap.sort();
        lap.dedup();
        assert_eq!(lap.len(), WORDS.len());
        let other: Vec<&str> = (0..60).map(|i| word("thread-b", i * WORD_EVERY)).collect();
        assert_ne!(steps, other);
        // Deterministic: a redraw shows the same word.
        assert_eq!(word("thread-a", 123), word("thread-a", 123));
    }
    #[test]
    fn sprite_rows_are_even() {
        for row in super::TOP.iter().chain(super::LEGS.iter().flatten()) {
            assert_eq!(row.len(), super::SPRITE_W);
        }
        assert_eq!(super::TOP.len() + 4, super::SPRITE_H);
    }
}
