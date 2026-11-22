//! Brand elements: the signpost mark, and the signpost putting itself up on the welcome screens.
//!
//! The mark is a trail signpost: a thick post with a chamfered cap and two ember boards, the
//! upper pointing right with a trail blaze cut through it, the lower shorter, ending in a
//! swallowtail. Its numbers (`signpost.rs`) are written by `assets/brand/trek_icon.py`, which
//! writes the app icons and the mark's SVGs from the same drawing; `small_icon_tests` reads its
//! SVGs and holds the two together.

use gpui_kit::*;
use std::time::Duration;

mod signpost;
use signpost::{BOARDS, BOX, POST, RADIUS, SMALL_1X, SMALL_2X};

/// A point of the drawing, on its 96-unit square.
type P = (f32, f32);

/// A board of the signpost, as it stands.
struct Board {
    /// The outline, clockwise from its top left. It's grown by [`RADIUS`] with round joins (the
    /// SVG fills it and strokes it twice that wide), which is what rounds its corners.
    outline: &'static [P],
    /// The trail blaze cut through it (the ground shows), or nothing.
    blaze: &'static [P],
    /// Where it's fixed to the post, which it swings about on the welcome screens.
    pivot: P,
    /// How far it's turned from level, clockwise in degrees (negative raises its right end).
    /// The outline is already turned; the tests undo it to check the drawing is as written.
    #[allow(dead_code)]
    angle: f32,
}

/// The mark's width over its height.
const ASPECT: f32 = BOX.2 / BOX.3;

/// The mark's colours on a theme: the boards, the post. Night's are the Night icon's (a cream
/// post for a dark ground); Paper's the Paper icon's (a near-black post, the boards a step
/// deeper).
const NIGHT: [u32; 2] = [0xFF7A3D, 0xF6F1EA];
const PAPER: [u32; 2] = [0xEE5A24, 0x1F1714];

/// The largest the small mark is drawn.
const SMALL: f32 = 24.;

/// A step along a grown outline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Line(P),
    /// A clockwise arc of the joins' radius to here: a round join.
    Arc(P),
}

impl Step {
    fn point(self) -> P {
        match self {
            Step::Line(p) | Step::Arc(p) => p,
        }
    }

    fn map(self, f: impl Fn(P) -> P) -> Self {
        match self {
            Step::Line(p) => Step::Line(f(p)),
            Step::Arc(p) => Step::Arc(f(p)),
        }
    }
}

/// The direction from `a` to `b`.
fn unit(a: P, b: P) -> P {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx.hypot(dy);
    (dx / length, dy / length)
}

/// A clockwise outline grown by `r`, with round joins where it turns out and the grown edges
/// meeting where it turns in (the swallowtail's notch).
fn grown(corners: &[P], r: f32) -> Vec<Step> {
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
            steps.push(Step::Line(a));
            steps.push(Step::Arc(b));
        } else {
            let t = ((b.0 - a.0) * d1.1 - (b.1 - a.1) * d1.0) / turn;
            steps.push(Step::Line((a.0 + t * d0.0, a.1 + t * d0.1)));
        }
    }
    steps
}

/// `p` turned `degrees` about `pivot`, clockwise on screen.
fn turned(p: P, pivot: P, degrees: f32) -> P {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (x, y) = (p.0 - pivot.0, p.1 - pivot.1);
    (pivot.0 + x * cos - y * sin, pivot.1 + x * sin + y * cos)
}

/// How far the signpost has put itself up. Finished, for everything but the welcome screens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    /// The post: 0 in the ground, 1 standing.
    post: f32,
    /// Each board's swing, upper then lower: 0 not hung yet, 1 at rest.
    boards: [f32; 2],
}

/// The welcome screens' drawing, as shares of its length: the post rises over the first part,
/// then each board swings on, the upper first; the lower is at rest at the end.
const POST_RISES: f32 = 0.38;
const BOARD_SWINGS: [(f32, f32); 2] = [(0.28, 0.78), (0.46, 1.0)];
/// Where each board swings in from, in degrees from where it rests: the upper one tipped down
/// at its tip, the lower one at its tail end, so they swing up into place.
const SWING_FROM: [f32; 2] = [32., -28.];

impl Drawn {
    const DONE: Self = Self {
        post: 1.,
        boards: [1.; 2],
    };

    /// `t` of the way through the welcome screens' drawing, 0 to 1.
    fn at(t: f32) -> Self {
        let span = |(from, to): (f32, f32)| ((t - from) / (to - from)).clamp(0., 1.);
        Self {
            post: settle(span((0., POST_RISES))),
            boards: BOARD_SWINGS.map(span),
        }
    }
}

/// An ease for something arriving: quick, then slowing to rest at 1, never past it (the post
/// doesn't bounce out of the ground).
fn settle(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// What's left of a board's swing `t` of the way through it, as a share of where it swung from:
/// 1 at the start, then past its place a little (a fifth at most) and back, 0 at the end.
fn swing(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    (1. - t).powi(2) * (1.5 * std::f32::consts::PI * t).cos()
}

/// How opaque a board is `t` of the way through its swing: it's there by a third of the way.
fn shown(t: f32) -> f32 {
    settle(t * 3.)
}

/// Follow `steps` as one closed shape. `r` is the joins' radius, `to` where a point of the
/// drawing is in the window.
fn trace(path: &mut PathBuilder, steps: &[Step], r: Pixels, to: &impl Fn(P) -> Point<Pixels>) {
    for (i, step) in steps.iter().enumerate() {
        let at = to(step.point());
        match step {
            _ if i == 0 => path.move_to(at),
            Step::Arc(_) if r > px(0.) => path.arc_to(point(r, r), px(0.), false, true, at),
            Step::Arc(_) => {}
            Step::Line(_) => path.line_to(at),
        }
    }
    path.close();
}

/// Paint the signpost: `to` places a point of it in the window and `k` is window pixels per
/// unit. Back to front, as the SVG has it: the post, then the boards over it, each with its
/// blaze cut through (the fill is even-odd).
fn paint(
    to: impl Fn(P) -> Point<Pixels>,
    k: f32,
    [board, post]: [Hsla; 2],
    drawn: Drawn,
    window: &mut Window,
) {
    let r = px(RADIUS * k);
    if drawn.post > 0. {
        // Rising, the post comes up out of the ground: drawn lower by what's still to come,
        // and cut off at its foot.
        let foot = POST.iter().map(|p| p.1).fold(f32::MIN, f32::max) + RADIUS;
        let top = POST.iter().map(|p| p.1).fold(f32::MAX, f32::min) - RADIUS;
        let sunk = (1. - drawn.post) * (foot - top);
        let mut path = PathBuilder::fill();
        trace(&mut path, &grown(POST, RADIUS), r, &|p| {
            to((p.0, p.1 + sunk))
        });
        if let Ok(path) = path.build() {
            let (a, b) = (to((0., 0.)), to((96., foot)));
            let ground = Bounds::from_corners(a, b);
            window.with_content_mask(Some(ContentMask { bounds: ground }), |window| {
                window.paint_path(path, post)
            });
        }
    }
    for ((b, swung), from) in BOARDS.iter().zip(drawn.boards).zip(SWING_FROM) {
        if swung <= 0. {
            continue;
        }
        let extra = from * swing(swung);
        let turn = |p: P| turned(p, b.pivot, extra);
        let mut path = PathBuilder::fill();
        let steps: Vec<Step> = grown(b.outline, RADIUS)
            .into_iter()
            .map(|s| s.map(turn))
            .collect();
        trace(&mut path, &steps, r, &to);
        if !b.blaze.is_empty() {
            let blaze: Vec<Step> = b.blaze.iter().map(|&p| Step::Line(turn(p))).collect();
            trace(&mut path, &blaze, px(0.), &to);
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, board.opacity(shown(swung)));
        }
    }
}

/// The mark's colours on the theme in use: the boards, the post.
fn colours(cx: &App) -> [Hsla; 2] {
    use gpui_kit::component::ActiveTheme as _;
    let colours = if cx.theme().mode.is_dark() {
        NIGHT
    } else {
        PAPER
    };
    colours.map(|c| Hsla::from(rgb(c)))
}

/// `colours`, `strength` of the way from the theme's background to themselves, and opaque.
fn muted(colours: [Hsla; 2], strength: f32, cx: &App) -> [Hsla; 2] {
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
fn paint_mark(bounds: Bounds<Pixels>, colours: [Hsla; 2], drawn: Drawn, window: &mut Window) {
    let w = bounds.size.width.min(bounds.size.height * ASPECT);
    let k = w.as_f32() / BOX.2;
    let ox = bounds.origin.x.as_f32() + (bounds.size.width - w).as_f32() / 2. - BOX.0 * k;
    let oy = bounds.origin.y.as_f32() + (bounds.size.height - w / ASPECT).as_f32() / 2. - BOX.1 * k;
    paint(
        |p| point(px(ox + p.0 * k), px(oy + p.1 * k)),
        k,
        colours,
        drawn,
        window,
    );
}

/// The mark, `size` wide, in the theme's colours. Up to [`SMALL`] points it's the small mark
/// ([`small_mark`]), drawn pixel by pixel; above that the signpost itself.
pub fn logo_mark(size: Pixels) -> AnyElement {
    mark(size, 1.)
}

/// The mark at half strength, for an empty state that shouldn't call for attention: its colours
/// half way to the theme's background. Use this rather than making the mark see-through
/// (`.opacity(0.5)` round [`logo_mark`]): the post would show through the boards.
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

/// The signpost drawn for small sizes (the title bar's 16 pt), where the full drawing, scaled
/// down, blurs: pixel by pixel, from a grid of 16 a side a point a pixel and of 32 from two
/// pixels a point. Its box is put on whole device pixels.
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
                let grid = small_grid(side * scale);
                let k = side / grid.len() as f32;
                let [board, post] = muted(colours(cx), strength, cx);
                for (ch, colour) in [('#', post), ('o', board)] {
                    for (x, y, w) in runs(grid, ch) {
                        let at = |x: usize, y: usize| {
                            point(px(snap(ox + x as f32 * k)), px(snap(oy + y as f32 * k)))
                        };
                        let cell = Bounds::from_corners(at(x, y), at(x + w, y + 1));
                        window.paint_quad(fill(cell, colour));
                    }
                }
            },
        )
        .size_full(),
    )
}

/// The small mark's grid for a box `pixels` device pixels across.
fn small_grid(pixels: f32) -> &'static [&'static str] {
    if pixels >= 28. { &SMALL_2X } else { &SMALL_1X }
}

/// The cells of `ch` in `grid`, as runs along each row: column, row, length.
fn runs(grid: &[&str], ch: char) -> Vec<(usize, usize, usize)> {
    let mut out = vec![];
    for (y, row) in grid.iter().enumerate() {
        let mut start = None;
        for (x, c) in row.chars().chain(['.']).enumerate() {
            match (c == ch, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    out.push((s, y, x - s));
                    start = None;
                }
                _ => {}
            }
        }
    }
    out
}

/// The welcome screens' signature: the signpost puts itself up. The post rises out of the
/// ground, then the boards swing on, the upper first, and settle. With reduce motion it's
/// simply there, finished.
pub fn signpost_draw(
    id: impl Into<ElementId>,
    size_px: Pixels,
    reduce_motion: bool,
) -> impl IntoElement {
    let total = Duration::from_millis(1100);
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
        BOARDS, Drawn, POST, POST_RISES, SMALL_1X, SMALL_2X, SWING_FROM, runs, settle, shown,
        small_grid, swing, turned,
    };

    #[test]
    fn the_post_never_bounces_and_the_boards_settle_where_they_rest() {
        let mut last = 0.0;
        for i in 0..=100 {
            let k = settle(i as f32 / 100.0);
            assert!((0.0..=1.0).contains(&k) && k >= last, "{i}: {k}");
            last = k;
        }
        assert_eq!(settle(1.0), 1.0);
        // A swing starts from where it swung in from and ends at rest, going past its place a
        // little on the way, never far.
        assert_eq!(swing(0.), 1.);
        assert!(swing(1.).abs() < 1e-6);
        let least = (0..=100)
            .map(|i| swing(i as f32 / 100.))
            .fold(f32::MAX, f32::min);
        assert!(least < 0. && least > -1. / 5., "{least}");
        assert_eq!(shown(0.34), 1.);
    }

    #[test]
    fn the_post_rises_first_then_the_boards_swing_on_upper_first() {
        assert_eq!(Drawn::at(1.0), Drawn::DONE);
        let start = Drawn::at(0.0);
        assert_eq!((start.post, start.boards), (0., [0., 0.]));
        let mut last = start;
        for i in 1..=200 {
            let now = Drawn::at(i as f32 / 200.);
            assert!(now.post >= last.post);
            assert!(now.boards[0] >= last.boards[0] && now.boards[1] >= last.boards[1]);
            // The upper board is always further on than the lower.
            assert!(now.boards[0] >= now.boards[1], "{i}: {now:?}");
            // No board is hung before the post is mostly up.
            if now.boards[0] > 0. {
                assert!(now.post > 0.9, "{i}: {now:?}");
            }
            last = now;
        }
        assert_eq!(Drawn::at(POST_RISES).post, 1.);
        // They swing in from opposite sides: the upper tipped down at its tip, the lower at its
        // tail end.
        const { assert!(SWING_FROM[0] > 0. && SWING_FROM[1] < 0.) };
    }

    #[test]
    fn the_boards_hang_on_the_post_and_the_mirror_is_broken() {
        let (left, right) = (POST[0].0, POST[2].0);
        let [upper, lower] = &BOARDS;
        for b in [upper, lower] {
            // Fixed to the post, crossing it.
            assert!(b.pivot.0 > left && b.pivot.0 < right);
            let xs = b.outline.iter().map(|p| p.0);
            assert!(xs.clone().fold(f32::MAX, f32::min) < left);
            assert!(xs.fold(f32::MIN, f32::max) > right);
            // Turned as the drawing says: undo the turn and its top edge is level.
            let level: Vec<_> = b
                .outline
                .iter()
                .map(|&p| turned(p, b.pivot, -b.angle))
                .collect();
            assert!((level[0].1 - level[1].1).abs() < 0.02, "{level:?}");
        }
        // The upper board points right and carries the blaze; the lower is shorter, reaching
        // further left, and ends in a notch (an inside corner) rather than a second point.
        let width = |b: &super::Board| {
            let xs = b.outline.iter().map(|p| p.0);
            xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
        };
        assert!(width(lower) < width(upper));
        assert!(!upper.blaze.is_empty() && lower.blaze.is_empty());
        assert_eq!(lower.outline.len(), 5, "the swallowtail's notch");
        // The post stands above the upper board and below the lower: a signpost, its cap one
        // chamfer (the left corner lower).
        assert!(POST[0].1 > POST[1].1);
        let top = upper.outline.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let bottom = lower.outline.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(POST[1].1 < top && POST[2].1 > bottom);
    }

    #[test]
    fn the_small_mark_is_drawn_on_whole_pixels() {
        // By value: each use of a `const` may be a copy of its own, so their addresses can differ.
        assert_eq!(small_grid(16.), &SMALL_1X[..]);
        assert_eq!(small_grid(24.), &SMALL_1X[..]);
        assert_eq!(small_grid(32.), &SMALL_2X[..]);
        for grid in [&SMALL_1X[..], &SMALL_2X[..]] {
            let n = grid.len();
            assert!(grid.iter().all(|row| row.len() == n));
            // One post, a column at least two pixels wide all the way down from its cap, and
            // two boards: the post shows between them.
            let post = runs(grid, '#');
            let rows: Vec<usize> = post.iter().map(|r| r.1).collect();
            assert!(rows.windows(2).all(|w| w[1] >= w[0]));
            assert!(post.iter().filter(|r| r.2 >= 2).count() >= n / 3);
            let board_rows: Vec<usize> = runs(grid, 'o').iter().map(|r| r.1).collect();
            let gaps = board_rows.windows(2).filter(|w| w[1] > w[0] + 1).count();
            assert_eq!(gaps, 1, "two boards, apart");
        }
    }
}

#[cfg(test)]
mod small_icon_tests;
