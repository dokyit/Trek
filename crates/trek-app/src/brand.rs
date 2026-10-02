//! Brand elements: the logo mark, the animated trail draw, and the status beacon.

use crate::palette;
use crate::trail_path::{BEACON, TRAIL};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use std::time::Duration;

/// The gradient switchback mark (static image).
pub fn logo_mark(size: Pixels) -> impl IntoElement {
    img("brand/mark.png").w(size).h(size * (110. / 128.))
}

/// Paint the switchback ribbon into `bounds`, drawn up to `progress` (0..1), plus the summit
/// beacon scaled by `beacon` (0..1, may overshoot for the ignite bounce).
pub fn paint_trail(bounds: Bounds<Pixels>, progress: f32, beacon: f32, dark: bool, window: &mut Window) {
    let side = bounds.size.width.min(bounds.size.height);
    let origin = point(
        bounds.origin.x + (bounds.size.width - side) / 2.,
        bounds.origin.y + (bounds.size.height - side) / 2.,
    );
    let at = |x: f32, y: f32| point(origin.x + side * x, origin.y + side * y);
    let n = TRAIL.len() - 1;
    let visible = (progress.clamp(0.0, 1.0) * n as f32).ceil() as usize;
    for i in 0..visible.min(n) {
        let (x0, y0, w0) = TRAIL[i];
        let (x1, y1, _) = TRAIL[i + 1];
        // Partial last segment so the head of the trail moves smoothly.
        let seg_t = if i + 1 == visible { (progress * n as f32 - i as f32).clamp(0.0, 1.0) } else { 1.0 };
        let (ex, ey) = (x0 + (x1 - x0) * seg_t, y0 + (y1 - y0) * seg_t);
        let color = palette::sunrise_at(i as f32 / n as f32);
        let mut path = PathBuilder::stroke(side * w0);
        path.move_to(at(x0, y0));
        path.line_to(at(ex, ey));
        if let Ok(p) = path.build() {
            window.paint_path(p, color);
        }
        // Round joins and caps.
        let r = side * w0 / 2.;
        let c = at(x0, y0);
        window.paint_quad(fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)), color).corner_radii(r));
        if i + 1 == visible {
            let c = at(ex, ey);
            window.paint_quad(fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)), color).corner_radii(r));
        }
    }
    if beacon > 0.0 {
        let c = at(BEACON.0, BEACON.1);
        let glow: Hsla = rgb(0xFFC56B).into();
        // Soft bloom: many faint rings read as a smooth glow.
        for i in 0..14 {
            let k = 1.4 + i as f32 * 0.22;
            let alpha = 0.075 * (1.0 - i as f32 / 14.0);
            let r = side * 0.053 * k * beacon.min(1.0);
            window.paint_quad(
                fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)), glow.opacity(alpha * beacon.min(1.0)))
                    .corner_radii(r),
            );
        }
        let r = side * 0.053 * beacon;
        // Cream core reads on dark; on light backgrounds use the ember core.
        let core = if dark { rgb(0xFFF1D6) } else { rgb(0xFF6A2B) };
        window.paint_quad(fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)), core).corner_radii(r));
    }
}

/// The signature "trail draw": the ribbon draws itself, then the beacon ignites.
/// `generation` replays it when changed.
pub fn trail_draw(id: impl Into<ElementId>, size_px: Pixels, reduce_motion: bool) -> impl IntoElement {
    let draw = Duration::from_millis(900);
    let ignite = Duration::from_millis(420);
    div().size(size_px).with_animations(
        id,
        vec![
            Animation::new(draw).with_easing(ease_in_out),
            Animation::new(ignite).with_easing(back_out),
        ],
        move |el, step, t| {
            let (progress, beacon) = if reduce_motion {
                (1.0, 1.0)
            } else if step == 0 {
                (t, 0.0)
            } else {
                (1.0, t)
            };
            el.child(canvas(|_, _, _| {}, move |bounds, _, window, cx| paint_trail(bounds, progress, beacon, cx.theme().mode.is_dark(), window)).size_full())
        },
    )
}

/// Ease-out with a small overshoot, for the beacon "ignite".
fn back_out(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}
