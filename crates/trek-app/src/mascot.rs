//! While an agent works: a trail word that changes every few seconds ("Switchbacking…") and a
//! little hiker walking back and forth along a dotted trail above the composer.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::time::Duration;

const WORDS: &[&str] = &[
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

/// The word for a turn that has run `secs` seconds; `seed` keeps threads from moving in lockstep.
pub fn word(seed: &str, secs: u64) -> &'static str {
    let h = seed.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let step = secs / 4;
    // A cheap scramble so consecutive steps don't walk the list in order.
    let i = (h.wrapping_add(step.wrapping_mul(0x9E3779B97F4A7C15)) >> 29) as usize % WORDS.len();
    WORDS[i]
}

/// The hiker's walk: one lap there and back in `LAP`.
const LAP: Duration = Duration::from_secs(16);
/// One sprite pixel, in points. 1.5pt is 3 device pixels on Retina, so edges stay crisp.
const PX: f32 = 1.5;
const SPRITE_W: usize = 14;
const SPRITE_H: usize = 16;
const HEIGHT: f32 = SPRITE_H as f32 * PX + 4.;

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

/// A dotted trail the width of its parent with the hiker walking it.
pub fn trail(id: impl Into<ElementId>, reduce_motion: bool, cx: &App) -> AnyElement {
    let dots = cx.theme().foreground.opacity(0.16);
    if reduce_motion {
        return div()
            .h(px(HEIGHT))
            .w_full()
            .child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, 0.06, 1, true, dots, window)).size_full())
            .into_any_element();
    }
    div()
        .h(px(HEIGHT))
        .w_full()
        .with_animation(id, Animation::new(LAP).repeat(), move |el, t| {
            // Triangle wave: walk right for half the lap, back left for the other half.
            let (raw, right) = if t < 0.5 { (t * 2., true) } else { (2. - t * 2., false) };
            // Ease at the ends so the hiker slows to a stop, turns, and sets off again.
            let pos = 0.5 - 0.5 * (std::f32::consts::PI * raw).cos();
            let speed = (std::f32::consts::PI * raw).sin();
            let frame = if speed < 0.15 { 1 } else { (t * LAP.as_secs_f32() * 7.) as usize % 4 };
            el.child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, pos, frame, right, dots, window)).size_full())
        })
        .into_any_element()
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
    let top = ground - 1. - SPRITE_H as f32 * PX;
    let cell = |x: f32, y: f32, rgb: u32, window: &mut Window| {
        let col = if right { x } else { SPRITE_W as f32 - 1. - x };
        window.paint_quad(fill(Bounds::new(point(px(left + col * PX), px(top + y * PX)), size(px(PX), px(PX))), gpui_kit::rgb(rgb)));
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
    #[test]
    fn words_change_over_time() {
        let a: std::collections::HashSet<_> = (0..60).map(|s| super::word("thread", s)).collect();
        assert!(a.len() >= 5);
        assert_eq!(super::word("x", 1), super::word("x", 3), "stable within a step");
    }

    #[test]
    fn sprite_rows_are_even() {
        for row in super::TOP.iter().chain(super::LEGS.iter().flatten()) {
            assert_eq!(row.len(), super::SPRITE_W);
        }
        assert_eq!(super::TOP.len() + 4, super::SPRITE_H);
    }
}
