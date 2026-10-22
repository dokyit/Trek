//! The review card's branch diagram, drawn rather than stamped: the base's line across the top,
//! the thread's branch forking away on a curve, a dot per commit, and an arc back to the base
//! that shows how merging stands — dashed and open while it's ready or blocked, accent and
//! breathing while one runs, violet once it has landed. It draws on over about half a second so
//! the shape of the work reads at a glance, and holds still when motion is off.

use crate::palette;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use std::time::Instant;
use trek_core::worktree::MergeBlock;

/// How the merge arc ends on the base line.
#[derive(Clone, Copy, PartialEq)]
pub enum EndState {
    /// Nothing in the way: dashed arc out to a hollow green point.
    Ready,
    /// A merge, push or pull-request op is in flight: the point breathes.
    Working,
    /// `Conflicts`: amber point and count. `Other`: a greyer blocked look for the rest.
    Conflicts(usize),
    Blocked,
    /// The base has everything already (or the merge just landed): a solid violet point.
    Merged,
}

/// What the card needs from the review.
pub struct GraphArgs<'a> {
    pub base: &'a str,
    pub branch: &'a str,
    /// Commits on the branch its base doesn't have.
    pub ahead: usize,
    /// Commits the base took since the branch left it.
    pub behind: usize,
    pub end: EndState,
    /// The status line's text under the diagram.
    pub label: String,
    /// When the card came on screen (draw-in), and whether to animate at all.
    pub since: Instant,
    pub animate: bool,
    /// Seconds on the UI clock, for the breathing point while an op runs.
    pub clock: f32,
}

/// The endpoint's state from the merge block, a busy label, and whether the branch is ahead.
pub fn end_state(ahead: usize, block: Option<&MergeBlock>, busy: bool) -> EndState {
    if busy {
        return EndState::Working;
    }
    match block {
        Some(MergeBlock::Conflicts(fs)) => EndState::Conflicts(fs.len()),
        Some(MergeBlock::NothingToMerge) => EndState::Merged,
        Some(_) => EndState::Blocked,
        None if ahead == 0 => EndState::Merged,
        None => EndState::Ready,
    }
}

/// The status line under the diagram.
pub fn end_label(end: EndState, base: &str) -> String {
    match end {
        EndState::Ready => "Ready to merge".to_string(),
        EndState::Working => "Working…".to_string(),
        EndState::Conflicts(1) => format!("1 conflict with {base}"),
        EndState::Conflicts(n) => format!("{n} conflicts with {base}"),
        EndState::Blocked => format!("Not ready to merge into {base}"),
        EndState::Merged => format!("Merged into {base}"),
    }
}

/// The card's height: two lanes with room for their curves.
const CARD_H: f32 = 74.;
/// Base lane and branch lane, from the card body's top.
const Y_BASE: f32 = 16.;
const Y_BRANCH: f32 = 56.;
/// Where the branch leaves the base, and where the arc lands, as fractions of the width.
const FORK: f32 = 0.16;
const LAND: f32 = 0.92;
/// The draw-in takes about this long end to end.
const DRAW_MS: f32 = 620.;
/// The most commit dots either lane shows.
const DOTS: usize = 7;

fn clamp01(v: f32) -> f32 {
    v.clamp(0., 1.)
}

/// `t` of the way from `a` to `b` on a cubic Bézier.
fn cubic(
    p0: Point<Pixels>,
    c1: Point<Pixels>,
    c2: Point<Pixels>,
    p1: Point<Pixels>,
    t: f32,
) -> Point<Pixels> {
    let u = 1. - t;
    let (x0, y0) = (p0.x.as_f32(), p0.y.as_f32());
    let (c1x, c1y) = (c1.x.as_f32(), c1.y.as_f32());
    let (c2x, c2y) = (c2.x.as_f32(), c2.y.as_f32());
    let (x1, y1) = (p1.x.as_f32(), p1.y.as_f32());
    point(
        px(u * u * u * x0 + 3. * u * u * t * c1x + 3. * u * t * t * c2x + t * t * t * x1),
        px(u * u * u * y0 + 3. * u * u * t * c1y + 3. * u * t * t * c2y + t * t * t * y1),
    )
}

/// Stroke `points` as a polyline (nothing for one point).
fn stroke(points: &[Point<Pixels>], width: Pixels, dash: bool, color: Hsla, window: &mut Window) {
    if points.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(width);
    if dash {
        b = b.dash_array(&[px(4.), px(4.)]);
    }
    b.move_to(points[0]);
    for p in &points[1..] {
        b.line_to(*p);
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// A stroked cubic, drawn `t` of the way.
fn curve(
    p0: Point<Pixels>,
    c1: Point<Pixels>,
    c2: Point<Pixels>,
    p1: Point<Pixels>,
    t: f32,
    width: Pixels,
    dash: bool,
    color: Hsla,
    window: &mut Window,
) {
    let t = clamp01(t);
    if t <= 0. {
        return;
    }
    let n = 20;
    let points: Vec<Point<Pixels>> = (0..=n)
        .map(|i| cubic(p0, c1, c2, p1, t * i as f32 / n as f32))
        .collect();
    stroke(&points, width, dash, color, window);
}

/// A commit dot.
fn dot(at: Point<Pixels>, r: f32, color: Hsla, window: &mut Window) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - r), px(at.y.as_f32() - r)),
                size(px(2. * r), px(2. * r)),
            ),
            color,
        )
        .corner_radii(px(r)),
    );
}

/// A hollow endpoint ring.
fn ring(at: Point<Pixels>, r: f32, w: f32, color: Hsla, fill_c: Hsla, window: &mut Window) {
    window.paint_quad(
        fill(
            Bounds::new(
                point(px(at.x.as_f32() - r), px(at.y.as_f32() - r)),
                size(px(2. * r), px(2. * r)),
            ),
            fill_c,
        )
        .corner_radii(px(r))
        .border_widths(px(w))
        .border_color(color),
    );
}

/// The branch diagram for a worktree review: labels on the left, the painted graph on the right.
pub fn card(args: GraphArgs, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let base_c = theme.foreground.opacity(0.30);
    let lane = theme.foreground.opacity(0.14);
    let accent = palette::sky(cx);
    let end_color = match args.end {
        EndState::Ready => palette::emerald(cx),
        EndState::Working => accent,
        EndState::Conflicts(_) => palette::amber(cx),
        EndState::Blocked => palette::amber(cx),
        EndState::Merged => palette::indigo(cx),
    };
    let progress = if args.animate {
        (args.since.elapsed().as_millis() as f32 / DRAW_MS).min(1.)
    } else {
        1.
    };
    let end = args.end;
    let (ahead, behind, clock, bg_fill) = (args.ahead, args.behind, args.clock, theme.background);

    let graph = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let w = b.size.width.as_f32();
            let (ox, oy) = (b.origin.x.as_f32(), b.origin.y.as_f32());
            let at = |x: f32, y: f32| point(px(ox + x), px(oy + y));
            let (x0, x1) = (8., w - 8.);
            let fork_x = x0 + (x1 - x0) * FORK;
            let land_x = x0 + (x1 - x0) * LAND;
            let tip_x = land_x - (x1 - x0) * 0.10;

            // The arc only exists once the branch has somewhere to go.
            let has_arc = ahead > 0 || end == EndState::Merged;

            // Draw-in phases: base line first, the fork and branch next, the arc and point last.
            let p_base = clamp01(progress / 0.40);
            let p_branch = clamp01((progress - 0.30) / 0.45);
            let p_arc = clamp01((progress - 0.62) / 0.38);

            // Base lane: the line, a dot where the branch left it, later commits it took.
            let base_end = x0 + (x1 - x0) * p_base;
            stroke(
                &[at(x0, Y_BASE), at(base_end, Y_BASE)],
                px(1.5),
                false,
                lane,
                window,
            );
            if p_base > FORK + 0.03 {
                dot(at(fork_x, Y_BASE), 3., base_c, window);
            }
            let shown_behind = behind.min(DOTS);
            for i in 0..shown_behind {
                let dx = fork_x + (land_x - fork_x) * (i + 1) as f32 / (shown_behind + 1) as f32;
                if dx < base_end - 4. {
                    dot(at(dx, Y_BASE), 3., base_c, window);
                }
            }
            // One more for the stretch past the landing point.
            if p_base >= 1. && w - 8. > land_x + 10. {
                dot(at(x1 - 4., Y_BASE), 2.5, base_c.opacity(0.6), window);
            }

            // Branch lane: fork curve down, the line out, a dot per commit.
            if p_branch > 0. {
                let join_x = fork_x + 18.;
                curve(
                    at(fork_x, Y_BASE),
                    at(fork_x, Y_BASE + 18.),
                    at(join_x - 12., Y_BRANCH),
                    at(join_x, Y_BRANCH),
                    (p_branch / 0.25).min(1.),
                    px(1.5),
                    false,
                    accent,
                    window,
                );
                let grow = clamp01((p_branch - 0.25) / 0.75);
                let branch_end = join_x + (tip_x - join_x) * grow;
                if grow > 0. {
                    stroke(
                        &[at(join_x, Y_BRANCH), at(branch_end, Y_BRANCH)],
                        px(1.5),
                        false,
                        accent,
                        window,
                    );
                }
                let shown = ahead.min(DOTS);
                for i in 0..shown {
                    let dx = join_x + (tip_x - join_x) * (i + 1) as f32 / (shown + 1) as f32;
                    if dx < branch_end - 3. {
                        dot(at(dx, Y_BRANCH), 3., accent, window);
                    }
                }
            }

            // The arc home and its point: dashed while the merge is still a "could", solid for
            // running and landed.
            if has_arc && p_arc > 0. {
                let pending = matches!(
                    end,
                    EndState::Ready | EndState::Conflicts(_) | EndState::Blocked
                );
                curve(
                    at(tip_x, Y_BRANCH),
                    at(tip_x, Y_BRANCH - 26.),
                    at(land_x, Y_BASE + 26.),
                    at(land_x, Y_BASE),
                    p_arc,
                    px(1.5),
                    pending,
                    end_color.opacity(if pending { 0.8 } else { 1. }),
                    window,
                );
            }
            if has_arc && p_arc > 0.85 {
                let pop = clamp01((p_arc - 0.85) / 0.15);
                let breathe = if end == EndState::Working {
                    0.75 + 0.25 * (clock * 3.2).sin().abs()
                } else {
                    1.
                };
                let r = (if end == EndState::Working { 4.5 } else { 4. }) * pop * breathe;
                match end {
                    EndState::Ready | EndState::Blocked => {
                        ring(at(land_x, Y_BASE), r, 1.6, end_color, bg_fill, window)
                    }
                    _ => {
                        dot(at(land_x, Y_BASE), r, end_color, window);
                        if end == EndState::Merged {
                            dot(at(land_x, Y_BASE), 1.5, bg_fill, window);
                        }
                    }
                }
            }

            // Keep frames coming while it draws in or a live op breathes.
            if progress < 1. || end == EndState::Working {
                window.request_animation_frame();
            }
        },
    )
    .size_full();

    let label_color = match end {
        EndState::Ready => palette::emerald(cx),
        EndState::Working => accent,
        EndState::Conflicts(_) | EndState::Blocked => palette::amber(cx),
        EndState::Merged => palette::indigo(cx),
    };

    v_flex()
        .id("branch-card")
        .test_support()
        .mx_3()
        .mt_2()
        .mb_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.4))
        .child(
            h_flex()
                .px_3()
                .pt_2()
                .pb(px(2.))
                .gap_1()
                .text_xs()
                .text_color(muted)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(format!("{} → {}", args.branch, args.base)),
                )
                .child(div().flex_1())
                .child(div().text_color(label_color).child(args.label.clone())),
        )
        .child(
            h_flex()
                .px_2()
                .pb_2()
                .gap_1()
                .child(
                    v_flex()
                        .w(px(64.))
                        .flex_none()
                        .h(px(CARD_H))
                        .justify_between()
                        .items_end()
                        .child(
                            div()
                                .pt(px(8.))
                                .text_xs()
                                .text_color(muted.opacity(0.9))
                                .child(args.base.to_string()),
                        )
                        .child(
                            div()
                                .pb(px(8.))
                                .text_xs()
                                .text_color(accent)
                                .max_w_full()
                                .truncate()
                                .child(args.branch.to_string()),
                        ),
                )
                .child(div().flex_1().h(px(CARD_H)).child(graph)),
        )
        .into_any_element()
}
