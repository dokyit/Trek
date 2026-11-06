//! Brand elements: the cairn mark, and the cairn stacking itself on the welcome screens.

use gpui_kit::*;
use std::time::Duration;

/// The mark's width over its height (`assets/brand/trek-mark.svg`).
const ASPECT: f32 = 580. / 540.;

/// The cairn mark. Up to [`SMALL`] points it's the small mark ([`small_mark`]), drawn for its
/// size; above that, the full mark's image (`assets/brand/trek-mark.svg`), whose detail only
/// reads large.
pub fn logo_mark(size: Pixels) -> AnyElement {
    if size <= px(SMALL) {
        small_mark(size).into_any_element()
    } else {
        img("brand/mark.png")
            .w(size)
            .h(size / ASPECT)
            .into_any_element()
    }
}

/// The largest the small mark is drawn.
const SMALL: f32 = 24.;

/// The small mark's stones in its 16 pt box, bottom first: left, top, width, height. Tops and
/// heights are whole points, so every flat edge lands on a device pixel at 1x and 2x; the
/// stones step a little left and right, as the full mark's do.
const SMALL_STONES: [(f32, f32, f32, f32); 4] = [
    (0.5, 12., 15., 4.),
    (3., 8., 11., 3.),
    (3.5, 4., 8., 3.),
    (5.5, 0., 5., 3.),
];
/// The stones' top and underside colours, bottom first: the full mark's on Night; a step deeper
/// on Paper, so the lightest stone still stands off a light title bar.
const SMALL_NIGHT: [(u32, u32); 4] = [
    (0xE8541E, 0x9E2F0C),
    (0xFF6A2B, 0xB23C12),
    (0xFF8A4C, 0xC24A18),
    (0xFFB062, 0xD5621F),
];
const SMALL_PAPER: [(u32, u32); 4] = [
    (0xC2410C, 0x8F2E08),
    (0xDB4F17, 0xA83A0E),
    (0xEC6424, 0xB84614),
    (0xF27E3E, 0xC4561C),
];

/// The cairn drawn for small sizes (the title bar's 16 pt): four flat stones on whole points,
/// with the full mark's darker underside where there are device pixels enough to show it (2x
/// and up). The full mark's image, scaled down this far, blurs into a smudge.
pub fn small_mark(side: Pixels) -> impl IntoElement {
    div().size(side).flex_none().child(
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                use gpui_kit::component::ActiveTheme as _;
                let scale = window.scale_factor();
                let k = b.size.width.as_f32().min(b.size.height.as_f32()) / 16.;
                let colors = if cx.theme().mode.is_dark() {
                    SMALL_NIGHT
                } else {
                    SMALL_PAPER
                };
                let snap = |v: f32| (v * scale).round() / scale;
                let (ox, oy) = (b.origin.x.as_f32(), snap(b.origin.y.as_f32()));
                for ((x, y, w, h), (top, side)) in SMALL_STONES.into_iter().zip(colors) {
                    let (x, w) = (ox + x * k, w * k);
                    let (y0, y1) = (snap(oy + y * k), snap(oy + (y + h) * k));
                    let stone = |y0: f32, y1: f32| {
                        Bounds::new(point(px(x), px(y0)), size(px(w), px(y1 - y0)))
                    };
                    // The underside shows as the bottom point of a stone, where a point is two
                    // device pixels or more; at 1x a one-pixel band reads as an outline.
                    let lip = if scale >= 2. {
                        snap(k.max(1. / scale))
                    } else {
                        0.
                    };
                    if lip > 0. {
                        window.paint_quad(
                            fill(stone(y0, y1), rgb(side)).corner_radii(px((y1 - y0) / 2.)),
                        );
                    }
                    window.paint_quad(
                        fill(stone(y0, y1 - lip), rgb(top)).corner_radii(px((y1 - lip - y0) / 2.)),
                    );
                }
            },
        )
        .size_full(),
    )
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
    let origin = point(
        bounds.origin.x + (bounds.size.width - w) / 2.,
        bounds.origin.y + (bounds.size.height - h) / 2.,
    );
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
        let stone = |dy: Pixels| {
            Bounds::new(
                point(centre.x - sw / 2., centre.y - face / 2. + dy),
                size(sw, face),
            )
        };
        window.paint_quad(
            fill(stone(thick / 2.), Hsla::from(rgb(side)).opacity(alpha)).corner_radii(face / 2.),
        );
        window.paint_quad(
            fill(stone(-thick / 2.), Hsla::from(rgb(top)).opacity(alpha)).corner_radii(face / 2.),
        );
    }
}

/// The welcome screens' signature: the cairn stacks itself, bottom stone first, each stone
/// settling onto the one below (an ease-out, no bounce: a stone doesn't sink into its neighbour).
pub fn cairn_draw(
    id: impl Into<ElementId>,
    size_px: Pixels,
    reduce_motion: bool,
) -> impl IntoElement {
    let total = Duration::from_millis(900);
    div()
        .w(size_px)
        .h(size_px / ASPECT)
        .with_animation(id, Animation::new(total), move |el, t| {
            let drops: [f32; 4] = std::array::from_fn(|i| {
                if reduce_motion {
                    return 1.0;
                }
                // Each stone takes 40% of the run, starting 20% after the one below.
                let local = ((t - i as f32 * 0.2) / 0.4).clamp(0.0, 1.0);
                if local == 0.0 { 0.0 } else { settle(local) }
            });
            el.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_cairn(bounds, drops, window),
                )
                .size_full(),
            )
        })
}

/// A falling stone's ease: quick, then slowing to rest at 1, never past it.
fn settle(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stone_settles_without_passing_its_place() {
        let mut last = 0.0;
        for i in 0..=100 {
            let k = super::settle(i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&k) && k >= last, "{i}: {k}");
            last = k;
        }
        assert_eq!(super::settle(1.0), 1.0);
    }

    #[test]
    fn the_small_mark_s_flat_edges_are_on_whole_points() {
        for (x, y, w, h) in super::SMALL_STONES {
            assert!(y.fract() == 0.0 && h.fract() == 0.0, "{y} {h}");
            assert!(x >= 0.0 && x + w <= 16.0 && y + h <= 16.0);
        }
        // Stacked bottom up, a point apart.
        for pair in super::SMALL_STONES.windows(2) {
            let (below, above) = (pair[0], pair[1]);
            assert_eq!(above.1 + above.3 + 1.0, below.1, "{above:?} on {below:?}");
            assert!(
                above.2 < below.2,
                "each stone smaller than the one under it"
            );
        }
    }
}

#[cfg(test)]
mod small_icon_tests;
