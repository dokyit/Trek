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
pub(crate) const LAP: Duration = Duration::from_secs(16);
/// Waiting for others to come back, it paces the same trail on a shorter lap: restless where
/// the working walk is steady.
pub(crate) const PACE: Duration = Duration::from_secs(8);
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

/// Frames per second for the working animation. Pixel art reads fine at this rate; each frame
/// still redraws the window (the other views come from GPUI's cache), so 16 rather than 60 is
/// about a quarter of the frames — and the bar ticks at half this behind another window.
pub const FPS: u64 = 16;

/// Steps a second while walking: a pose about every other frame at [`FPS`] in front — one a
/// frame behind another window, a few through a fold's quick ones. The pose comes off the walk
/// clock rather than the frame count, so the gait stays even at every rate.
const STEPS: f32 = 8.;

/// A dotted trail the width of its parent with the hiker walking it. `phase` is how far through
/// its lap there and back it is, 0 to 1 — the caller's clock advances it a lap per [`LAP`]
/// working, [`PACE`] while it waits on others, so a change between the two moves the hiker's
/// pace, never its place; `gait` is the seconds it has walked, for its legs. `still`, it stands
/// near the left end. The caller re-renders at [`FPS`] while it wants motion (see `WorkingBar`).
pub fn trail(phase: f32, gait: f32, still: bool, cx: &App) -> AnyElement {
    let dots = cx.theme().foreground.opacity(0.16);
    let (pos, frame, right) = if still { (0.06, 1, true) } else { hike(phase, gait) };
    div().h(px(HEIGHT)).w_full().child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, pos, frame, right, dots, window)).size_full()).into_any_element()
}

/// Where a `phase` (0 to 1) of the way through a lap there and back leaves the hiker: its place
/// on the trail (0 to 1), its walk frame (`gait` seconds in), and which way it faces.
pub(crate) fn hike(phase: f32, gait: f32) -> (f32, usize, bool) {
    let t = phase % 1.;
    // Triangle wave: walk right for half the lap, back left for the other half.
    let (raw, right) = if t < 0.5 { (t * 2., true) } else { (2. - t * 2., false) };
    // Ease at the ends so the hiker slows to a stop, turns, and sets off again.
    let pos = 0.5 - 0.5 * (std::f32::consts::PI * raw).cos();
    let speed = (std::f32::consts::PI * raw).sin();
    let frame = if speed < 0.15 { 1 } else { (gait * STEPS) as usize % 4 };
    (pos, frame, right)
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
    fn the_hike_crosses_the_trail_there_and_back() {
        // Working or pacing while it waits, the hiker goes end to end and back a lap.
        for lap in [super::LAP, super::PACE] {
            let secs = lap.as_secs_f32();
            // `clock` seconds into the lap, having walked all of them.
            let at = |clock: f32| super::hike((clock / secs) % 1., clock);
            // The ends of the trail at the lap's ends: left at the start, right halfway, home at
            // a whole lap — standing at each.
            let (pos, frame, right) = at(0.);
            assert!(pos.abs() < 1e-4 && right && frame == 1, "standing at the left end, facing out");
            let (pos, frame, right) = at(secs * 0.5);
            assert!((pos - 1.).abs() < 1e-4 && !right && frame == 1, "standing at the right end, turned round");
            let (pos, frame, right) = at(secs);
            assert!(pos.abs() < 1e-4 && right && frame == 1, "back where it started");
            // In between it's on the trail in one of the four stride poses.
            for i in 0..400 {
                let (pos, frame, _) = at(i as f32 * secs / 400.);
                assert!((0. ..=1.).contains(&pos) && frame < 4, "on the trail, in stride");
            }
            // And lap after lap the same walk comes round.
            for i in 0..50 {
                let clock = i as f32 * secs / 50.;
                let (a, b) = (at(clock), at(clock + secs));
                assert!((a.0 - b.0).abs() < 1e-4 && a.2 == b.2, "lap {i}: same place, same way");
                assert!((a.1 + 4 - b.1) % 4 <= 1 || (b.1 + 4 - a.1) % 4 <= 1, "lap {i}: legs within a pose");
            }
        }
        // The wait's pace is a visibly different lap, not the working walk again.
        assert!(super::PACE < super::LAP);
    }

    #[test]
    fn sprite_rows_are_even() {
        for row in super::TOP.iter().chain(super::LEGS.iter().flatten()) {
            assert_eq!(row.len(), super::SPRITE_W);
        }
        assert_eq!(super::TOP.len() + 4, super::SPRITE_H);
    }
}
