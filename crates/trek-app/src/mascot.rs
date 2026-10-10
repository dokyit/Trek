//! While an agent works: a plain "Working…" and a little hiker walking back and forth along a
//! dotted trail above the composer (`working_bar`).

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::time::Duration;

/// What Trek says an agent is doing while it works. Plain status text: the hiker beside it is
/// the brand, the words aren't.
pub const WORDS: &[&str] = &["Working"];

/// The word for a turn in thread `seed` that has run `secs` seconds. One word for now; the
/// arguments keep callers ready should it ever say more.
pub fn word(_seed: &str, _secs: u64) -> &'static str {
    WORDS[0]
}

/// The hiker's walk: one lap there and back in `LAP`.
pub(crate) const LAP: Duration = Duration::from_secs(16);
/// Waiting for others to come back, it paces the same trail on a shorter lap: restless where
/// the working walk is steady.
pub(crate) const PACE: Duration = Duration::from_secs(8);
/// One sprite pixel, in points, as designed: 3 device pixels on Retina. At other scales it's
/// whatever whole number of device pixels comes nearest (`cell_device_px`).
const PX: f32 = 1.5;
/// The most a sprite pixel may grow to, in points, rounding up to whole device pixels: the hiker
/// then still fits the trail's height.
const PX_MAX: f32 = 1.6;
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

/// How many device pixels one sprite pixel takes at `scale`: the whole number nearest `PX`
/// points' worth, so every sprite pixel is the same size (GPUI snaps each quad's edges to device
/// pixels: at 125 % a 1.5pt pixel would come out 2, 2, 2, 2, 1 device pixels wide). Rounding up
/// is held to `PX_MAX`, so at 100 % it's 1 device pixel rather than a hiker a third taller.
fn cell_device_px(scale: f32) -> f32 {
    let n = (PX * scale).round().max(1.);
    if n > 1. && n / scale > PX_MAX { n - 1. } else { n }
}

/// `v` points, moved to the nearest device pixel.
fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

fn paint(b: Bounds<Pixels>, pos: f32, frame: usize, right: bool, dots: Hsla, window: &mut Window) {
    let scale = window.scale_factor();
    let w = b.size.width.as_f32();
    let ground = snap(b.origin.y.as_f32() + HEIGHT - 2., scale);
    // Dotted trail: dots of a whole number of device pixels, on device pixels, all alike.
    let dot = (2. * scale).round().max(1.) / scale;
    let mut x = b.origin.x.as_f32() + 2.;
    while x < b.origin.x.as_f32() + w - 2. {
        window.paint_quad(fill(Bounds::new(point(px(snap(x, scale)), px(ground - 1.)), size(px(dot), px(dot))), dots).corner_radii(px(dot / 2.)));
        x += 7.;
    }
    let n = cell_device_px(scale);
    let cell = n / scale;
    let travel = (w - SPRITE_W as f32 * cell - 8.).max(0.);
    // Snap to the sprite's pixel grid, in device pixels, so it never blurs mid-pixel.
    let left = (((b.origin.x.as_f32() + 4. + travel * pos) * scale) / n).round() * n / scale;
    // Its feet on the trail; a hiker rounded up a little is kept inside the trail's height.
    let top = snap(ground - 1. - SPRITE_H as f32 * cell, scale).max(snap(b.origin.y.as_f32(), scale));
    sprite(left, top, cell, frame, right, window);
}

fn sprite(left: f32, top: f32, cell_px: f32, frame: usize, right: bool, window: &mut Window) {
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
    use super::{WORDS, word};

    #[test]
    fn the_working_word_is_plain() {
        assert_eq!(word("thread-a", 0), "Working");
        assert_eq!(word("thread-b", 123), word("thread-a", 0), "the same in every thread, all turn long");
        assert!(WORDS.iter().all(|w| !w.to_lowercase().contains("trail") && !w.to_lowercase().contains("cairn")));
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
    fn sprite_pixels_are_whole_device_pixels_at_every_scale() {
        // Retina is as designed: 3 device pixels, 1.5pt.
        assert_eq!(super::cell_device_px(2.), 3.);
        for (scale, want) in [(1., 1.), (1.25, 2.), (1.5, 2.), (1.75, 2.), (2., 3.), (2.25, 3.), (2.5, 4.), (3., 4.)] {
            let n = super::cell_device_px(scale);
            assert_eq!(n, want, "at {scale}");
            assert!(n / scale <= super::PX_MAX + 1e-6, "at {scale} the hiker still fits the trail");
            // The sprite's top, from a ground on a device pixel, lands on one too.
            let ground = super::snap(100.3, scale);
            let top = super::snap(ground - 1. - super::SPRITE_H as f32 * n / scale, scale);
            assert!(((top * scale) - (top * scale).round()).abs() < 1e-3 && ((ground * scale) - (ground * scale).round()).abs() < 1e-3);
        }
    }

    #[test]
    fn sprite_rows_are_even() {
        for row in super::TOP.iter().chain(super::LEGS.iter().flatten()) {
            assert_eq!(row.len(), super::SPRITE_W);
        }
        assert_eq!(super::TOP.len() + 4, super::SPRITE_H);
    }
}
