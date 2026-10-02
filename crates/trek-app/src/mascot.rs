//! While an agent works: a trail word that changes every few seconds ("Switchbacking…") and a
//! little hiker walking back and forth along a dotted trail above the composer.

use crate::palette;
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
const LAP: Duration = Duration::from_secs(14);
const HEIGHT: f32 = 26.;

/// A dotted trail the width of its parent with the hiker walking it.
pub fn trail(id: impl Into<ElementId>, reduce_motion: bool, cx: &App) -> AnyElement {
    let ink = cx.theme().foreground.opacity(0.78);
    let dots = cx.theme().foreground.opacity(0.16);
    let pack = palette::ember(cx);
    if reduce_motion {
        return div()
            .h(px(HEIGHT))
            .w_full()
            .child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, 0.08, 0.0, false, ink, dots, pack, window)).size_full())
            .into_any_element();
    }
    div()
        .h(px(HEIGHT))
        .w_full()
        .with_animation(id, Animation::new(LAP).repeat(), move |el, t| {
            // Triangle wave: walk right for half the lap, back left for the other half.
            let (pos, right) = if t < 0.5 { (t * 2., true) } else { (2. - t * 2., false) };
            // Ease at the turnarounds so the hiker slows, turns, and sets off again.
            let pos = 0.5 - 0.5 * (std::f32::consts::PI * pos).cos();
            let stride = t * LAP.as_secs_f32() * 1.9;
            el.child(canvas(|_, _, _| {}, move |b, _, window, _| paint(b, pos, stride, right, ink, dots, pack, window)).size_full())
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn paint(b: Bounds<Pixels>, pos: f32, stride: f32, right: bool, ink: Hsla, dots: Hsla, pack: Hsla, window: &mut Window) {
    let w = b.size.width.as_f32();
    let ground = b.origin.y.as_f32() + HEIGHT - 3.;
    // Dotted trail.
    let mut x = b.origin.x.as_f32() + 2.;
    while x < b.origin.x.as_f32() + w - 2. {
        window.paint_quad(fill(Bounds::new(point(px(x), px(ground)), size(px(2.), px(2.))), dots).corner_radii(px(1.)));
        x += 7.;
    }
    // Hiker, about 18px tall, facing the way it walks.
    let margin = 12.;
    let cx = b.origin.x.as_f32() + margin + (w - margin * 2.) * pos;
    let dir = if right { 1. } else { -1. };
    let phase = stride * std::f32::consts::TAU;
    let swing = phase.sin();
    let bob = phase.sin().abs() * 0.8;
    let hip = point(cx, ground - 8. - bob);
    let neck = point(cx + dir * 0.6, ground - 14. - bob);
    let line = |a: (f32, f32), c: (f32, f32), width: f32, color: Hsla, window: &mut Window| {
        let mut p = PathBuilder::stroke(px(width));
        p.move_to(point(px(a.0), px(a.1)));
        p.line_to(point(px(c.0), px(c.1)));
        if let Ok(path) = p.build() {
            window.paint_path(path, color);
        }
    };
    let dot = |c: (f32, f32), r: f32, color: Hsla, window: &mut Window| {
        window.paint_quad(fill(Bounds::new(point(px(c.0 - r), px(c.1 - r)), size(px(r * 2.), px(r * 2.))), color).corner_radii(px(r)));
    };
    // Legs: swing opposite each other from the hip; a knee keeps them from looking like stilts.
    for s in [swing, -swing] {
        let knee = (hip.x + dir * s * 2.2, hip.y + 4.);
        let foot = (hip.x + dir * s * 3.6, ground - 0.5);
        line((hip.x, hip.y), knee, 1.6, ink, window);
        line(knee, foot, 1.6, ink, window);
    }
    // Torso and backpack (the one spot of colour).
    line((hip.x, hip.y), (neck.x, neck.y), 1.8, ink, window);
    let pack_x = neck.x - dir * 2.6;
    window.paint_quad(fill(Bounds::new(point(px(pack_x - 1.8), px(neck.y + 0.5)), size(px(3.6), px(5.2))), pack).corner_radii(px(1.2)));
    // Arm and trekking pole, planted ahead in rhythm with the stride.
    let hand = (neck.x + dir * (2.4 + swing * 0.8), neck.y + 4.);
    line((neck.x, neck.y + 1.), hand, 1.4, ink, window);
    line((hand.0 - dir * 0.6, hand.1 - 2.5), (hand.0 + dir * (1.6 + swing * 1.2), ground - 0.5), 1.0, ink.opacity(0.7), window);
    // Head.
    dot((neck.x + dir * 0.4, neck.y - 2.6), 2.2, ink, window);
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_change_over_time() {
        let a: std::collections::HashSet<_> = (0..60).map(|s| super::word("thread", s)).collect();
        assert!(a.len() >= 5);
        assert_eq!(super::word("x", 1), super::word("x", 3), "stable within a step");
    }
}
