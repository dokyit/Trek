//! Brand elements: the mountain mark, and the mark drawing itself on the welcome screens.
//!
//! The mark is one mountain with a lit face and a shaded face, a dotted trail up the lit face
//! and a small flag planted behind the summit. The numbers here are those of
//! `assets/brand/trek_icon.py`, which writes the app icons and the mark's SVGs from them;
//! `small_icon_tests` reads its SVGs and holds the two together.

use gpui_kit::*;
use std::time::Duration;

/// A point of a drawing, in its own units.
type P = (f32, f32);

/// A drawing of the mountain: the mark on its 96-unit square, or one fitted to a few pixels.
struct Art {
    /// The outline, clockwise from the lit face's foot: the summit is its last corner but one,
    /// the shaded face's foot its last.
    outline: &'static [P],
    /// The outline is grown by half of this with round joins (the SVG fills it and strokes it
    /// this wide), which is what rounds the summit and the feet.
    edge: f32,
    /// The ridge, summit to base. The shaded face is the silhouette right of it, so the two
    /// faces share one shape and meet exactly at the summit.
    ridge: &'static [P],
    /// The trail up the lit face, from its foot: centre x, y, radius. Cut through the mountain,
    /// so they're the colour of whatever it stands on.
    dots: &'static [(f32, f32, f32)],
    /// The flag's pole, behind the mountain: centre x, foot, top, width.
    pole: (f32, f32, f32, f32),
    /// The flag's cloth: where it starts, then lines (one point) and curves (two control points
    /// and the end).
    cloth: &'static [&'static [P]],
    /// Drawn for a few pixels: the flag is two plain rectangles on whole device pixels (the
    /// cloth's first and third points are its corners). The mark's own has a round-ended pole
    /// and a waving cloth.
    fitted: bool,
}

/// The mark, on its 96-unit square.
const MARK: Art = Art {
    outline: &[
        (9.5, 78.5),
        (32.8, 46.6),
        (43., 43.4),
        (56., 21.),
        (86.5, 78.5),
    ],
    edge: 4.5,
    ridge: &[
        (56., 21.),
        (60.6, 39.6),
        (55., 51.6),
        (62.6, 64.8),
        (58.8, 78.5),
    ],
    dots: &[
        (29., 74.2, 2.4),
        (36., 69.2, 2.4),
        (43., 66., 2.4),
        (47.2, 60., 2.4),
        (44., 53.2, 2.4),
        (48., 47., 2.4),
        (51.6, 40.4, 2.4),
        (53.6, 34., 2.4),
        (54.6, 28.2, 2.),
    ],
    pole: (56., 24., 8.6, 2.),
    cloth: &[
        &[(56.4, 8.6)],
        &[(59.3, 7.2), (61.5, 10.2), (65.7, 8.8)],
        &[(65.7, 15.2)],
        &[(61.5, 16.6), (59.3, 13.6), (56.4, 15.)],
    ],
    fitted: false,
};

/// The mark's own box on that square: left, top, width, height (`trek-mark.svg`'s view box).
const BOX: (f32, f32, f32, f32) = (7., 7., 82., 74.);
/// The mark's width over its height.
const ASPECT: f32 = BOX.2 / BOX.3;
/// How many dots the trail has.
const DOTS: usize = MARK.dots.len();

/// The mark's colours on a theme: the lit face, the shaded face, the flag. Night's are the
/// app icon's; Paper's are a step deeper, as the Paper icon's are, and its flag is dark where
/// Night's is light.
const NIGHT: [u32; 3] = [0xFF7A3D, 0xC9551F, 0xFFC7A0];
const PAPER: [u32; 3] = [0xE85D1F, 0xB8461A, 0xB8461A];

/// The largest the small mark is drawn.
const SMALL: f32 = 24.;

/// The small mark in its 16 pt box, a device pixel a point: no trail (its dots would be a third
/// of a pixel across), a straight ridge on a pixel's edge, the base on the box's bottom edge
/// and the flag on whole points, larger than its share so it's still a flag.
const SMALL_1X: Art = Art {
    outline: &[(0., 16.), (5., 9.6), (7., 9.), (9.5, 4.), (16., 16.)],
    edge: 0.,
    ridge: &[(9.5, 4.), (10., 5.), (10., 16.)],
    dots: &[],
    pole: (9.5, 5., 1., 1.),
    cloth: &[&[(10., 1.)], &[(12., 1.)], &[(12., 3.)], &[(10., 3.)]],
    fitted: true,
};

/// The small mark where a point is two device pixels or more: on half points, with the ridge's
/// turns, round joins a pixel across and a shorter trail of larger dots, each centred on a
/// pixel.
const SMALL_2X: Art = Art {
    outline: &[(0.5, 15.5), (5., 9.3), (7., 8.7), (9.5, 4.5), (15.5, 15.5)],
    edge: 1.,
    ridge: &[
        (9.5, 4.5),
        (10.5, 8.),
        (9.5, 10.25),
        (11., 12.75),
        (10.25, 15.5),
    ],
    dots: &[
        (4.75, 14.25, 0.575),
        (6.75, 13.25, 0.575),
        (7.75, 11.25, 0.575),
        (8.25, 9.25, 0.575),
        (8.75, 7.25, 0.575),
    ],
    pole: (9.5, 5., 1., 1.),
    cloth: &[&[(10., 1.)], &[(12., 1.)], &[(12., 2.5)], &[(10., 2.5)]],
    fitted: true,
};

/// A step along a grown outline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Line(P),
    /// A clockwise arc of the edge's radius to here: a round join.
    Arc(P),
}

impl Step {
    fn point(self) -> P {
        match self {
            Step::Line(p) | Step::Arc(p) => p,
        }
    }
}

/// The direction from `a` to `b`.
fn unit(a: P, b: P) -> P {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx.hypot(dy);
    (dx / length, dy / length)
}

impl Art {
    fn radius(&self) -> f32 {
        self.edge / 2.
    }

    fn summit(&self) -> P {
        self.outline[self.outline.len() - 2]
    }

    /// The whole silhouette: the outline grown by the edge's radius, with round joins.
    fn silhouette(&self) -> Vec<Step> {
        let (corners, r) = (self.outline, self.radius());
        let mut steps = Vec::with_capacity(corners.len() * 2);
        for (i, &corner) in corners.iter().enumerate() {
            let before = corners[(i + corners.len() - 1) % corners.len()];
            let after = corners[(i + 1) % corners.len()];
            let (d0, d1) = (unit(before, corner), unit(corner, after));
            // Where the grown edges before and after this corner end and start.
            let a = (corner.0 + r * d0.1, corner.1 - r * d0.0);
            let b = (corner.0 + r * d1.1, corner.1 - r * d1.0);
            let turn = d0.0 * d1.1 - d0.1 * d1.0;
            if turn > 0. {
                // A corner that points out: the round join.
                steps.push(Step::Line(a));
                steps.push(Step::Arc(b));
            } else {
                // The shoulder's inside corner: the two grown edges meet.
                let t = ((b.0 - a.0) * d1.1 - (b.1 - a.1) * d1.0) / turn;
                steps.push(Step::Line((a.0 + t * d0.0, a.1 + t * d0.1)));
            }
        }
        steps
    }

    /// The silhouette right of the ridge: from the top of the summit's round join, round the
    /// shaded face's foot, along the base to under the ridge's end, and back up the ridge.
    fn shade(&self) -> Vec<Step> {
        let (summit, foot, first) = (
            self.summit(),
            self.outline[self.outline.len() - 1],
            self.outline[0],
        );
        let r = self.radius();
        let (down, base) = (unit(summit, foot), unit(foot, first));
        let end = self.ridge[self.ridge.len() - 1];
        let mut steps = vec![
            Step::Line((summit.0, summit.1 - r)),
            Step::Arc((summit.0 + r * down.1, summit.1 - r * down.0)),
            Step::Line((foot.0 + r * down.1, foot.1 - r * down.0)),
            Step::Arc((foot.0 + r * base.1, foot.1 - r * base.0)),
            Step::Line((end.0, end.1 + r)),
        ];
        steps.extend(self.ridge.iter().rev().map(|&p| Step::Line(p)));
        steps
    }
}

/// How far the mark has drawn itself. Finished, for everything but the welcome screens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// Each dot of the trail, from the foot: 0 not there, 1 its full size.
    dots: [f32; DOTS],
    /// The flag: 0 behind the summit, 1 risen.
    flag: f32,
}

/// The welcome screens' drawing, as shares of its length: when the first dot starts, how long
/// after one the next starts, how long a dot takes to grow, and when the flag starts to rise
/// (it's up at the end). The last dot is nearly out when the flag starts.
const FIRST_DOT: f32 = 0.06;
const NEXT_DOT: f32 = 0.07;
const DOT_GROWS: f32 = 0.14;
const FLAG_RISES: f32 = 0.7;

impl Drawn {
    const DONE: Self = Self {
        dots: [1.; DOTS],
        flag: 1.,
    };

    /// `t` of the way through the welcome screens' drawing, 0 to 1. The mountain is there from
    /// the start; the trail's dots appear one at a time from its foot, then the flag rises.
    fn at(t: f32) -> Self {
        Self {
            dots: std::array::from_fn(|i| {
                settle((t - FIRST_DOT - i as f32 * NEXT_DOT) / DOT_GROWS)
            }),
            flag: settle((t - FLAG_RISES) / (1. - FLAG_RISES)),
        }
    }
}

/// An ease for something arriving: quick, then slowing to rest at 1, never past it (a dot
/// doesn't swell and shrink back, a flag doesn't overshoot its pole).
fn settle(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Follow `steps` as one closed shape. `r` is the round joins' radius, `to` where a point of
/// the drawing is in the window.
fn trace(path: &mut PathBuilder, steps: &[Step], r: Pixels, to: &impl Fn(P) -> Point<Pixels>) {
    for (i, step) in steps.iter().enumerate() {
        let at = to(step.point());
        match step {
            _ if i == 0 => path.move_to(at),
            Step::Arc(_) if r > px(0.) => path.arc_to(point(r, r), px(0.), false, true, at),
            // No radius, no join: the line before it ended here.
            Step::Arc(_) => {}
            Step::Line(_) => path.line_to(at),
        }
    }
    path.close();
}

/// Paint `art`: `to` places a point of it in the window, `k` is window pixels per unit of it
/// and `scale` device pixels per window pixel. Back to front: the flag, the mountain in the
/// lit face's colour with the trail cut through it, the shaded face over its right. (As the SVG
/// has it. Two faces painted side by side leave a seam of the ground's colour down the ridge,
/// each path being antialiased on its own; one over the other doesn't. It needs the colours
/// opaque: see [`logo_mark_muted`].)
fn paint(
    art: &Art,
    to: impl Fn(P) -> Point<Pixels>,
    k: f32,
    scale: f32,
    [lit, shade, flag]: [Hsla; 3],
    drawn: Drawn,
    window: &mut Window,
) {
    let (x, foot, top, wide) = art.pole;
    if art.fitted {
        let snap = |p: Point<Pixels>| {
            point(
                px((p.x.as_f32() * scale).round() / scale),
                px((p.y.as_f32() * scale).round() / scale),
            )
        };
        let rect = |a: P, b: P| Bounds::from_corners(snap(to(a)), snap(to(b)));
        window.paint_quad(fill(
            rect((x - wide / 2., top), (x + wide / 2., foot)),
            flag,
        ));
        window.paint_quad(fill(rect(art.cloth[0][0], art.cloth[2][0]), flag));
    } else if drawn.flag > 0. {
        // Rising, the flag starts with its top at the summit, behind the mountain, and its
        // cloth unfurls from the pole on the way up.
        let sunk = (1. - drawn.flag) * (art.summit().1 - top);
        let flag = flag.opacity(drawn.flag);
        let (a, b) = (to((x - wide / 2., top + sunk)), to((x + wide / 2., foot)));
        if b.y > a.y {
            window
                .paint_quad(fill(Bounds::from_corners(a, b), flag).corner_radii(px(wide * k / 2.)));
        }
        let place = |p: P| to((x + (p.0 - x) * drawn.flag, p.1 + sunk));
        let mut cloth = PathBuilder::fill();
        cloth.move_to(place(art.cloth[0][0]));
        for part in &art.cloth[1..] {
            match **part {
                [end] => cloth.line_to(place(end)),
                [a, b, end] => cloth.cubic_bezier_to(place(end), place(a), place(b)),
                _ => {}
            }
        }
        cloth.close();
        if let Ok(cloth) = cloth.build() {
            window.paint_path(cloth, flag);
        }
    }

    let r = px(art.radius() * k);
    let mut body = PathBuilder::fill();
    trace(&mut body, &art.silhouette(), r, &to);
    // The trail: holes (the fill is even-odd), so the ground shows through them.
    for (&(x, y, dot), grown) in art.dots.iter().zip(drawn.dots) {
        let dot = dot * grown;
        if dot <= 0. {
            continue;
        }
        let (left, right) = (to((x - dot, y)), to((x + dot, y)));
        let radii = point(px(dot * k), px(dot * k));
        body.move_to(left);
        body.arc_to(radii, px(0.), true, false, right);
        body.arc_to(radii, px(0.), true, false, left);
        body.close();
    }
    if let Ok(body) = body.build() {
        window.paint_path(body, lit);
    }
    let mut face = PathBuilder::fill();
    trace(&mut face, &art.shade(), r, &to);
    if let Ok(face) = face.build() {
        window.paint_path(face, shade);
    }
}

/// The mark's colours on the theme in use.
fn colours(cx: &App) -> [Hsla; 3] {
    use gpui_kit::component::ActiveTheme as _;
    let colours = if cx.theme().mode.is_dark() {
        NIGHT
    } else {
        PAPER
    };
    colours.map(|c| Hsla::from(rgb(c)))
}

/// `colours`, `strength` of the way from the theme's background to themselves, and opaque.
fn muted(colours: [Hsla; 3], strength: f32, cx: &App) -> [Hsla; 3] {
    use gpui_kit::component::ActiveTheme as _;
    if strength >= 1. {
        return colours;
    }
    let ground = cx.theme().background.to_rgb();
    colours.map(|c| {
        let c = c.to_rgb();
        let mix = |c: f32, ground: f32| ground + (c - ground) * strength;
        Hsla::from(Rgba {
            r: mix(c.r, ground.r),
            g: mix(c.g, ground.g),
            b: mix(c.b, ground.b),
            a: 1.,
        })
    })
}

/// Paint the mark into `bounds`, in its own proportions, centred.
fn paint_mark(bounds: Bounds<Pixels>, colours: [Hsla; 3], drawn: Drawn, window: &mut Window) {
    let w = bounds.size.width.min(bounds.size.height * ASPECT);
    let k = w.as_f32() / BOX.2;
    let ox = bounds.origin.x.as_f32() + (bounds.size.width - w).as_f32() / 2. - BOX.0 * k;
    let oy = bounds.origin.y.as_f32() + (bounds.size.height - w / ASPECT).as_f32() / 2. - BOX.1 * k;
    paint(
        &MARK,
        |p| point(px(ox + p.0 * k), px(oy + p.1 * k)),
        k,
        window.scale_factor(),
        colours,
        drawn,
        window,
    );
}

/// The mark, `size` wide, in the theme's colours. Up to [`SMALL`] points it's the small mark
/// ([`small_mark`]), drawn for its size; above that the mark itself, trail and all.
pub fn logo_mark(size: Pixels) -> AnyElement {
    mark(size, 1.)
}

/// The mark at half strength, for an empty state that shouldn't call for attention: its colours
/// half way to the theme's background. Use this rather than making the mark see-through
/// (`.opacity(0.5)` round [`logo_mark`]): the shaded face is painted over the lit one, which
/// would show through it, brighter than the lit face itself on a dark theme.
pub fn logo_mark_muted(size: Pixels) -> AnyElement {
    mark(size, 0.5)
}

fn mark(size: Pixels, strength: f32) -> AnyElement {
    if size <= px(SMALL) {
        small(size, strength).into_any_element()
    } else {
        div()
            .w(size)
            .h(size / ASPECT)
            .flex_none()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let colours = muted(colours(cx), strength, cx);
                        paint_mark(bounds, colours, Drawn::DONE, window)
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

/// The mountain drawn for small sizes (the title bar's 16 pt), where the mark itself, scaled
/// down, loses its trail and its flag to blur: its box is put on whole device pixels, the base
/// on the box's bottom edge and the flag on whole points. A point a pixel it has no trail; from
/// two pixels a point, a short one.
pub fn small_mark(side: Pixels) -> impl IntoElement {
    small(side, 1.)
}

fn small(side: Pixels, strength: f32) -> impl IntoElement {
    div().size(side).flex_none().child(
        canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let scale = window.scale_factor();
                let snap = |v: f32| (v * scale).round() / scale;
                let side = snap(b.size.width.as_f32().min(b.size.height.as_f32()));
                let (ox, oy) = (snap(b.origin.x.as_f32()), snap(b.origin.y.as_f32()));
                let k = side / 16.;
                let art = small_art(side * scale);
                paint(
                    art,
                    |p| point(px(ox + p.0 * k), px(oy + p.1 * k)),
                    k,
                    scale,
                    muted(colours(cx), strength, cx),
                    Drawn::DONE,
                    window,
                );
            },
        )
        .size_full(),
    )
}

/// The small mark's drawing for a box `pixels` device pixels across.
fn small_art(pixels: f32) -> &'static Art {
    if pixels >= 28. { &SMALL_2X } else { &SMALL_1X }
}

/// The welcome screens' signature: the mark draws itself. The mountain comes up, the trail's
/// dots appear one at a time from its foot, then the flag rises behind the summit. With reduce
/// motion it's simply there, finished.
pub fn mountain_draw(
    id: impl Into<ElementId>,
    size_px: Pixels,
    reduce_motion: bool,
) -> impl IntoElement {
    let total = Duration::from_millis(900);
    div()
        .w(size_px)
        .h(size_px / ASPECT)
        .with_animation(id, Animation::new(total), move |el, t| {
            let drawn = if reduce_motion {
                Drawn::DONE
            } else {
                Drawn::at(t)
            };
            el.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| paint_mark(bounds, colours(cx), drawn, window),
                )
                .size_full(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{
        DOT_GROWS, Drawn, FIRST_DOT, FLAG_RISES, MARK, NEXT_DOT, SMALL_1X, SMALL_2X, Step, settle,
        small_art,
    };

    #[test]
    fn nothing_arrives_past_its_place() {
        let mut last = 0.0;
        for i in 0..=100 {
            let k = settle(i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&k) && k >= last, "{i}: {k}");
            last = k;
        }
        assert_eq!(settle(1.0), 1.0);
    }

    #[test]
    fn the_trail_appears_from_the_foot_and_then_the_flag_rises() {
        // It starts as the bare mountain and ends as the mark.
        assert_eq!(Drawn::at(1.0), Drawn::DONE);
        let start = Drawn::at(0.0);
        assert_eq!((start.dots, start.flag), ([0.; 9], 0.));
        let mut last = start;
        for i in 1..=200 {
            let now = Drawn::at(i as f32 / 200.);
            // Nothing goes back.
            assert!(now.flag >= last.flag);
            for (a, b) in now.dots.iter().zip(last.dots) {
                assert!(*a >= b);
            }
            // One at a time from the foot: a dot is never further out than the one below it.
            for pair in now.dots.windows(2) {
                assert!(pair[0] >= pair[1], "{i}: {:?}", now.dots);
            }
            // The flag waits until the last dot is nearly out.
            if now.flag > 0. {
                assert!(now.dots[8] > 0.8, "{i}: {now:?}");
            }
            last = now;
        }
        // The dots come one after another, not together: when the fifth starts the first is
        // out and the last hasn't begun.
        let mid = Drawn::at(FIRST_DOT + 4. * NEXT_DOT + 0.01);
        assert!(mid.dots[0] == 1. && mid.dots[4] > 0. && mid.dots[8] == 0.);
        // And the whole trail is out before the end, with the flag still rising.
        let late = Drawn::at(FIRST_DOT + 8. * NEXT_DOT + DOT_GROWS);
        assert!(late.dots == [1.; 9] && late.flag > 0. && late.flag < 1.);
        assert!(FLAG_RISES < 1.);
    }

    #[test]
    fn both_faces_share_the_silhouette_and_meet_at_the_summit() {
        let (whole, shade) = (MARK.silhouette(), MARK.shade());
        let summit = MARK.summit();
        // The shaded face starts at the top of the summit's round join, straight above it.
        assert_eq!(shade[0], Step::Line((summit.0, summit.1 - MARK.radius())));
        // Its outer edge is the silhouette's own: the same join's end, then the far foot's.
        for step in &shade[1..4] {
            assert!(whole.contains(step), "{step:?} isn't on the silhouette");
        }
        // And it comes back up the ridge to the summit itself.
        assert_eq!(shade.last(), Some(&Step::Line(summit)));
        // The trail is on the lit face: left of the ridge at its height.
        for &(x, y, r) in MARK.dots {
            let ridge = MARK
                .ridge
                .windows(2)
                .find(|w| w[0].1 <= y && y <= w[1].1)
                .map(|w| w[0].0 + (w[1].0 - w[0].0) * (y - w[0].1) / (w[1].1 - w[0].1))
                .unwrap();
            assert!(
                x + r < ridge,
                "the dot at {x} {y} crosses the ridge at {ridge}"
            );
        }
    }

    /// Whether `v` is on a `1/per`-point grid: whole points for 1, half points for 2.
    fn on_grid(v: f32, per: f32) -> bool {
        (v * per).fract() == 0.0
    }

    #[test]
    fn the_small_mark_is_on_whole_pixels() {
        assert!(std::ptr::eq(small_art(16.), &SMALL_1X));
        assert!(std::ptr::eq(small_art(24.), &SMALL_1X));
        assert!(std::ptr::eq(small_art(32.), &SMALL_2X));
        for (art, per) in [(&SMALL_1X, 1.), (&SMALL_2X, 2.)] {
            let r = art.radius();
            // The base is the box's bottom edge, the feet its bottom corners.
            let (first, foot) = (art.outline[0], art.outline[art.outline.len() - 1]);
            assert_eq!((first.1 + r, foot.1 + r), (16., 16.));
            assert_eq!((first.0 - r, foot.0 + r), (0., 16.));
            // The pole is a column of pixels, the summit in the middle of it, and the cloth
            // a rectangle of them beside it.
            let (x, pole_foot, top, wide) = art.pole;
            assert!(on_grid(x - wide / 2., 1.) && wide == 1. && on_grid(top, per));
            assert_eq!(art.summit().0, x);
            assert!(
                pole_foot > art.summit().1 - r,
                "the pole stands behind the summit"
            );
            let (a, b) = (art.cloth[0][0], art.cloth[2][0]);
            assert_eq!(
                a,
                (x + wide / 2., top),
                "the cloth hangs from the pole's top"
            );
            assert!(on_grid(b.0, 1.) && on_grid(b.1, per) && b.0 <= 16. && b.1 < art.summit().1);
            assert!(
                top >= 0. && art.summit().1 - r >= b.1,
                "the flag is above the summit"
            );
            // Each dot is centred on a pixel.
            for &(x, y, _) in art.dots {
                assert!(
                    on_grid(x * per - 0.5, 1.) && on_grid(y * per - 0.5, 1.),
                    "{x} {y}"
                );
            }
        }
        // A pixel a point, the shaded face starts on a pixel's edge under the summit's pixel.
        assert!(SMALL_1X.ridge[1..].iter().all(|p| p.0 == 10.));
        // No trail a pixel a point; a shorter one of larger dots from two.
        assert!(SMALL_1X.dots.is_empty());
        assert!(!SMALL_2X.dots.is_empty() && SMALL_2X.dots.len() < MARK.dots.len());
        let share = |art: &super::Art| {
            art.dots[0].2 / (art.outline[art.outline.len() - 1].0 - art.outline[0].0)
        };
        assert!(share(&SMALL_2X) > share(&MARK));
    }
}

#[cfg(test)]
mod small_icon_tests;
