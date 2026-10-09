//! Brand elements: the cairn mark, and the cairn stacking itself on the welcome screens.

use gpui_kit::*;
use std::time::Duration;

/// The mark's width over its height (`assets/brand/trek-mark.svg`).
const ASPECT: f32 = 580. / 540.;

/// The cairn mark (static image; `assets/brand/trek-mark.svg`).
pub fn logo_mark(size: Pixels) -> impl IntoElement {
    img("brand/mark.png").w(size).h(size / ASPECT)
}

/// The mark's stones, bottom first, in its own box (0..1 across, 0..1 down): centre, width,
/// height, then top and side colours. From `assets/brand/trek_icon.py`.
const STONES: [(f32, f32, f32, f32, u32, u32); 4] = [
    (0.4828, 0.7889, 0.8621, 0.3111, 0xE8541E, 0x9E2F0C),
    (0.5241, 0.5333, 0.6138, 0.2556, 0xFF6A2B, 0xB23C12),
    (0.4621, 0.3111, 0.4000, 0.2148, 0xFF8A4C, 0xC24A18),
    (0.5034, 0.1333, 0.2069, 0.1519, 0xFFB062, 0xD5621F),
];

/// Paint the stones into `bounds` (the mark's proportions, centred). Stone `i` is drawn
/// `drops[i]` of the way down to its place (0 = above the box, 1 = resting), and not at all at 0.
fn paint_cairn(bounds: Bounds<Pixels>, drops: [f32; 4], window: &mut Window) {
    let w = bounds.size.width.min(bounds.size.height * ASPECT);
    let h = w / ASPECT;
    let origin = point(bounds.origin.x + (bounds.size.width - w) / 2., bounds.origin.y + (bounds.size.height - h) / 2.);
    for ((cx, cy, sw, sh, top, side), k) in STONES.into_iter().zip(drops) {
        if k <= 0.0 {
            continue;
        }
        let (sw, sh) = (w * sw, h * sh);
        let thick = sh * 0.26;
        let face = sh - thick;
        // Falls from a stone's height above its place.
        let lift = h * 0.25 * (1.0 - k);
        let centre = point(origin.x + w * cx, origin.y + h * cy - lift);
        let alpha = k.clamp(0.0, 1.0);
        let stone = |dy: Pixels| Bounds::new(point(centre.x - sw / 2., centre.y - face / 2. + dy), size(sw, face));
        window.paint_quad(fill(stone(thick / 2.), Hsla::from(rgb(side)).opacity(alpha)).corner_radii(face / 2.));
        window.paint_quad(fill(stone(-thick / 2.), Hsla::from(rgb(top)).opacity(alpha)).corner_radii(face / 2.));
    }
}

/// The welcome screens' signature: the cairn stacks itself, bottom stone first.
pub fn cairn_draw(id: impl Into<ElementId>, size_px: Pixels, reduce_motion: bool) -> impl IntoElement {
    let total = Duration::from_millis(1100);
    div().w(size_px).h(size_px / ASPECT).with_animation(id, Animation::new(total), move |el, t| {
        let drops: [f32; 4] = std::array::from_fn(|i| {
            if reduce_motion {
                return 1.0;
            }
            // Each stone takes 40% of the run, starting 20% after the one below.
            let local = ((t - i as f32 * 0.2) / 0.4).clamp(0.0, 1.0);
            if local == 0.0 { 0.0 } else { back_out(local) }
        });
        el.child(canvas(|_, _, _| {}, move |bounds, _, window, _| paint_cairn(bounds, drops, window)).size_full())
    })
}

/// Ease-out with a small overshoot, for the beacon "ignite".
fn back_out(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}
