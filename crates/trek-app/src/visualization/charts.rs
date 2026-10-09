//! Charts of values: bars, lines, a donut, stat tiles, tables, heatmaps and treemaps.

use super::layout::{Rect, Scale, squarify};
use super::{Frame, Look, format_number, format_tick, inset, readout, series_color, tone, with_unit};
use gpui_kit::component::{Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;
use trek_core::visualization::{BarMark, DonutPart, LineSeries, SemanticTone, StatTile, TableCell, TreemapItem};

use crate::assets::Lucide;

// ---------- painting ----------

pub(super) fn hline(window: &mut Window, x: Pixels, y: Pixels, w: Pixels, color: Hsla) {
    window.paint_quad(fill(Bounds::new(point(x, y), size(w, px(1.))), color));
}

pub(super) fn vline(window: &mut Window, x: Pixels, y: Pixels, h: Pixels, color: Hsla) {
    window.paint_quad(fill(Bounds::new(point(x, y), size(px(1.), h)), color));
}

pub(super) fn dot(window: &mut Window, c: Point<Pixels>, r: Pixels, color: Hsla) {
    window.paint_quad(fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)), color).corner_radii(r));
}

/// A smooth line through `pts` that never overshoots them (monotone cubic, Fritsch–Carlson):
/// curves for the eye, every point still exactly where its value is.
pub(super) fn monotone(path: &mut PathBuilder, pts: &[Point<Pixels>], start: bool) {
    let n = pts.len();
    if n == 0 {
        return;
    }
    if start {
        path.move_to(pts[0]);
    }
    if n == 1 {
        return;
    }
    let (xs, ys): (Vec<f32>, Vec<f32>) = pts.iter().map(|p| (f32::from(p.x), f32::from(p.y))).unzip();
    let slope: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]).max(1e-3)).collect();
    let mut m: Vec<f32> = (0..n)
        .map(|i| {
            if i == 0 {
                slope[0]
            } else if i == n - 1 {
                slope[n - 2]
            } else if slope[i - 1] * slope[i] <= 0. {
                0.
            } else {
                (slope[i - 1] + slope[i]) / 2.
            }
        })
        .collect();
    for i in 0..n - 1 {
        if slope[i] == 0. {
            m[i] = 0.;
            m[i + 1] = 0.;
            continue;
        }
        let (a, b) = (m[i] / slope[i], m[i + 1] / slope[i]);
        let h = a.hypot(b);
        if h > 3. {
            let t = 3. / h;
            m[i] = t * a * slope[i];
            m[i + 1] = t * b * slope[i];
        }
    }
    for i in 0..n - 1 {
        let dx = (xs[i + 1] - xs[i]) / 3.;
        path.cubic_bezier_to(pts[i + 1], point(px(xs[i] + dx), px(ys[i] + m[i] * dx)), point(px(xs[i + 1] - dx), px(ys[i + 1] - m[i + 1] * dx)));
    }
}

/// An axis label `w` wide centred on `frac` of its row.
fn centred_at(frac: f32, w: f32) -> Div {
    div().absolute().top_0().left(relative(frac)).ml(px(-w / 2.)).w(px(w)).flex().justify_center()
}

fn caption(text: String, look: &Look) -> Div {
    div().text_size(px(11.)).text_color(look.muted).child(text)
}

// ---------- bar ----------

const BAR_ROW: f32 = 26.;

pub(super) fn bars(bars: &[BarMark], x_label: Option<&str>, y_label: Option<&str>, frame: &Frame, cx: &App) -> AnyElement {
    let look = &frame.look;
    let n = bars.len();
    let lo = bars.iter().map(|b| b.value).fold(0f64, f64::min);
    let hi = bars.iter().map(|b| b.value).fold(0f64, f64::max);
    let scale = Scale::nice(lo, hi, if frame.narrow() { 3 } else { 5 });
    let zero = scale.frac(0.);
    let ticks = scale.ticks();
    let label_w = if frame.narrow() { relative(0.32) } else { relative(0.26) };
    let labels = v_flex().w(label_w).max_w(px(180.)).flex_none().gap(px(4.)).children(bars.iter().enumerate().map(|(ix, bar)| {
        div()
            .id(frame.id("barlabel", ix))
            .h(px(BAR_ROW))
            .flex()
            .items_center()
            .pr_3()
            .on_hover(frame.on_hover(ix))
            .child(div().w_full().truncate().text_size(px(11.5)).text_color(look.fg.opacity(0.86 * frame.presence(ix))).child(bar.label.clone()))
    }));
    let grid_ticks = ticks.clone();
    let (grid, hairline) = (look.grid, look.hairline);
    let tracks = div()
        .relative()
        .flex_1()
        .min_w_0()
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    for t in &grid_ticks {
                        let x = b.origin.x + b.size.width * scale.frac(*t);
                        vline(window, x, b.origin.y, b.size.height, if *t == 0. { hairline } else { grid });
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(v_flex().gap(px(4.)).children(bars.iter().enumerate().map(|(ix, bar)| {
            let color = tone(bar.tone, cx);
            let grown = frame.arrive(ix, n);
            let end = zero + (scale.frac(bar.value) - zero) * grown;
            let (left, right) = (zero.min(end), zero.max(end));
            let negative = bar.value < 0.;
            let presence = frame.presence(ix);
            let hovered = frame.hover == Some(ix);
            let value = div()
                .absolute()
                .top_0()
                .h_full()
                .flex()
                .items_center()
                .font_family(look.mono.clone())
                .text_size(px(11.))
                .text_color(look.fg.opacity(if hovered { 1. } else { 0.78 * presence }))
                .when(!negative, |el| el.left(relative(right)).pl(px(6.)))
                .when(negative, |el| el.right(relative(1. - left)).pr(px(6.)))
                .opacity(grown)
                .child(format_number(bar.value));
            div()
                .id(frame.id("bar", ix))
                .test_support()
                .relative()
                .h(px(BAR_ROW))
                .on_hover(frame.on_hover(ix))
                .child(
                    div()
                        .absolute()
                        .top(px((BAR_ROW - 14.) / 2.))
                        .h(px(14.))
                        .left(relative(left))
                        .w(relative(right - left))
                        // Square at the baseline, rounded at the value's end.
                        .when(!negative, |el| el.rounded_r(px(4.)))
                        .when(negative, |el| el.rounded_l(px(4.)))
                        .bg(color.opacity(if hovered { 1. } else { 0.86 * presence })),
                )
                .child(value)
        })));
    let axis = h_flex().child(div().w(label_w).max_w(px(180.)).flex_none()).child(div().relative().flex_1().h(px(14.)).children(ticks.iter().map(|t| {
        centred_at(scale.frac(*t), 64.).child(div().font_family(look.mono.clone()).text_size(px(10.)).text_color(look.muted).child(format_tick(*t, scale.step)))
    })));
    v_flex()
        .id(frame.id("barchart", 0))
        .test_support()
        .w_full()
        .gap(px(6.))
        .when_some(y_label.map(str::to_string), |el, label| el.child(caption(label, look)))
        .child(
            inset(look).p_3().pr(px(44.)).child(v_flex().gap(px(6.)).child(h_flex().items_start().child(labels).child(tracks)).child(axis)),
        )
        .when_some(x_label.map(str::to_string), |el, label| el.child(caption(label, look).text_center()))
        .into_any_element()
}

// ---------- line ----------

/// A row of legend chips: each switches its series on or off, and hovering one picks it out.
fn legend(labels: &[(String, Hsla)], frame: &Frame, line: bool) -> AnyElement {
    let look = &frame.look;
    let n = labels.len();
    h_flex()
        .flex_wrap()
        .gap(px(6.))
        .children(labels.iter().enumerate().map(|(ix, (label, color))| {
            let off = frame.hidden.contains(&ix);
            h_flex()
                .id(frame.id("legend", ix))
                .test_support()
                .gap(px(6.))
                .h(px(22.))
                .px(px(8.))
                .rounded_full()
                .border_1()
                .border_color(if off { look.grid } else { look.hairline })
                .cursor_pointer()
                .hover(|s| s.bg(look.fg.opacity(0.04)))
                .on_hover(frame.on_hover(ix))
                .on_click(frame.toggle(ix, n))
                .child(if line {
                    div().w(px(10.)).h(px(2.)).rounded_full().bg(if off { look.muted.opacity(0.4) } else { *color })
                } else {
                    div().size(px(8.)).rounded(px(2.)).bg(if off { look.muted.opacity(0.4) } else { *color })
                })
                .child(div().text_size(px(11.)).text_color(if off { look.muted.opacity(0.7) } else { look.fg.opacity(0.85) }).when(off, |el| el.line_through()).child(label.clone()))
        }))
        .into_any_element()
}

pub(super) fn line(x_labels: &[String], series: &[LineSeries], area: bool, y_label: Option<&str>, unit: Option<&str>, frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let narrow = frame.narrow();
    let n = x_labels.len();
    let visible: Vec<usize> = (0..series.len()).filter(|i| !frame.hidden.contains(i)).collect();
    let (min, max) = visible.iter().flat_map(|i| series[*i].values.iter()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    // Include zero unless the values sit far from it: a line's shape matters more than its base.
    let lo = if min >= 0. && min <= max * 0.5 { 0. } else { min };
    let scale = Scale::nice(lo, max, if narrow { 3 } else { 4 });
    let ticks = scale.ticks();
    let colors: Vec<Hsla> = series.iter().enumerate().map(|(i, s)| series_color(s.tone, i, cx)).collect();
    let plot_h = if narrow { 150. } else { 184. };
    let axis_w = if narrow { 38. } else { 46. };
    let reveal = frame.arrive(0, 1);
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
    let paint = {
        let bounds = bounds.clone();
        let data: Vec<(Vec<f64>, Hsla, f32)> = visible
            .iter()
            .map(|i| {
                let strength = match frame.hover {
                    Some(h) if h != *i && visible.contains(&h) => 0.22,
                    _ => 1.,
                };
                (series[*i].values.clone(), colors[*i], strength)
            })
            .collect();
        let (grid, hairline, gap, ticks, cursor) = (look.grid, look.hairline, look.gap, ticks.clone(), frame.cursor);
        let single = visible.len() == 1;
        let count = visible.len().max(1);
        move |b: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
            bounds.set(b);
            let at = |i: usize, v: f64| point(b.origin.x + b.size.width * (i as f32 / (n - 1) as f32), b.origin.y + b.size.height * (1. - scale.frac(v)));
            for t in &ticks {
                let y = b.origin.y + b.size.height * (1. - scale.frac(*t));
                hline(window, b.origin.x, y, b.size.width, if *t == 0. { hairline } else { grid });
            }
            let base = b.origin.y + b.size.height * (1. - scale.frac(0f64.clamp(scale.lo, scale.hi)));
            // The entrance draws the lines in from the left.
            let clip = Bounds::new(b.origin - point(px(8.), px(8.)), size(b.size.width * reveal + px(16.), b.size.height + px(16.)));
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                for (values, color, strength) in &data {
                    let pts: Vec<Point<Pixels>> = values.iter().enumerate().map(|(i, v)| at(i, *v)).collect();
                    if area || single {
                        let mut fillp = PathBuilder::fill();
                        fillp.move_to(point(pts[0].x, base));
                        fillp.line_to(pts[0]);
                        monotone(&mut fillp, &pts, false);
                        fillp.line_to(point(pts[pts.len() - 1].x, base));
                        fillp.close();
                        // Overlapping washes muddy each other: the more lines, the fainter each.
                        // A flat wash, not a gradient: washes blended toward transparent go grey.
                        let wash = if area { if count > 1 { 0.06 } else { 0.14 } } else { 0.06 } * strength;
                        if let Ok(p) = fillp.build() {
                            window.paint_path(p, color.opacity(wash));
                        }
                    }
                    let mut stroke = PathBuilder::stroke(px(2.));
                    monotone(&mut stroke, &pts, true);
                    if let Ok(p) = stroke.build() {
                        window.paint_path(p, color.opacity(*strength));
                    }
                }
            });
            match cursor {
                Some(ix) if ix < n => {
                    let x = b.origin.x + b.size.width * (ix as f32 / (n - 1) as f32);
                    vline(window, x, b.origin.y, b.size.height, hairline.opacity(2.2));
                    for (values, color, strength) in &data {
                        let c = at(ix, values[ix]);
                        dot(window, c, px(6.), gap);
                        dot(window, c, px(4.), color.opacity(strength.max(0.5)));
                    }
                }
                _ if reveal >= 1. => {
                    // The latest value, marked at each line's end.
                    for (values, color, strength) in &data {
                        let c = at(n - 1, values[n - 1]);
                        dot(window, c, px(5.), gap);
                        dot(window, c, px(3.), color.opacity(*strength));
                    }
                }
                _ => {}
            }
        }
    };
    let state = frame.state();
    let move_state = state.clone();
    let move_bounds = bounds.clone();
    let plot = div()
        .id(frame.id("lineplot", 0))
        .test_support()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(plot_h))
        .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
            let b = move_bounds.get();
            if b.size.width <= px(0.) || !b.contains(&e.position) {
                return;
            }
            let frac = f32::from(e.position.x - b.origin.x) / f32::from(b.size.width);
            let ix = (frac * (n - 1) as f32).round().clamp(0., (n - 1) as f32) as usize;
            Frame::set_cursor(&move_state, Some(ix), cx);
        })
        .on_hover(move |hovered, _, cx| {
            if !hovered {
                Frame::set_cursor(&state, None, cx);
            }
        })
        .child(canvas(|_, _, _| {}, paint).size_full())
        .when_some(frame.cursor.filter(|c| *c < n), |el, ix| {
            let frac = ix as f32 / (n - 1) as f32;
            let tip = readout(&look)
                .absolute()
                .top(px(6.))
                .min_w(px(120.))
                .when(frac <= 0.55, |el| el.left(relative(frac)).ml(px(14.)))
                .when(frac > 0.55, |el| el.right(relative(1. - frac)).mr(px(14.)))
                .child(div().text_size(px(11.)).text_color(look.muted).child(x_labels[ix].clone()))
                .children(visible.iter().map(|i| {
                    h_flex()
                        .gap(px(8.))
                        .child(div().w(px(10.)).h(px(2.)).rounded_full().bg(colors[*i]))
                        .child(div().font_family(look.mono.clone()).font_semibold().text_color(look.fg).child(with_unit(format_number(series[*i].values[ix]), unit)))
                        .child(div().text_color(look.muted).truncate().child(series[*i].label.clone()))
                }));
            el.child(tip)
        });
    let y_axis = div().relative().w(px(axis_w)).flex_none().h(px(plot_h)).children(ticks.iter().map(|t| {
        div()
            .absolute()
            .right(px(8.))
            .top(relative(1. - scale.frac(*t)))
            .mt(px(-7.))
            .font_family(look.mono.clone())
            .text_size(px(10.))
            .text_color(look.muted)
            .child(format_tick(*t, scale.step))
    }));
    // As many x labels as fit, evenly spread, always the first and the last.
    let room = ((frame.width - axis_w - 40.) / if narrow { 52. } else { 64. }).floor().max(2.) as usize;
    let every = n.div_ceil(room.min(n)).max(1);
    let x_axis = h_flex().child(div().w(px(axis_w)).flex_none()).child(div().relative().flex_1().h(px(14.)).children((0..n).filter(|i| i % every == 0 || *i == n - 1).filter(|i| *i == n - 1 || n - 1 - i >= every / 2 + 1 || every == 1).map(|i| {
        let frac = i as f32 / (n - 1) as f32;
        let label = div().text_size(px(10.)).text_color(if frame.cursor == Some(i) { look.fg } else { look.muted }).whitespace_nowrap().child(x_labels[i].clone());
        if i == 0 {
            div().absolute().top_0().left_0().child(label)
        } else if i == n - 1 {
            div().absolute().top_0().right_0().child(label)
        } else {
            centred_at(frac, 80.).child(label)
        }
    })));
    let header = h_flex()
        .justify_between()
        .items_end()
        .gap_3()
        .child(caption(with_unit(y_label.unwrap_or("").to_string(), None) + &unit.filter(|_| y_label.is_some()).map(|u| format!(" ({u})")).unwrap_or_default(), &look))
        .when(series.len() > 1, |el| el.child(legend(&series.iter().zip(&colors).map(|(s, c)| (s.label.clone(), *c)).collect::<Vec<_>>(), frame, true)));
    v_flex()
        .id(frame.id("line", 0))
        .test_support()
        .w_full()
        .gap(px(8.))
        .when(series.len() > 1 || y_label.is_some(), |el| el.child(header))
        .child(inset(&look).p_3().pl_1().child(v_flex().gap(px(6.)).child(h_flex().child(y_axis).child(plot)).child(x_axis)))
        .into_any_element()
}

// ---------- donut ----------

pub(super) fn donut(parts: &[DonutPart], unit: Option<&str>, frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let narrow = frame.narrow();
    let colors: Vec<Hsla> = parts.iter().enumerate().map(|(i, p)| series_color(p.tone, i, cx)).collect();
    let shown: Vec<f64> = parts.iter().enumerate().map(|(i, p)| if frame.hidden.contains(&i) { 0. } else { p.value }).collect();
    let total: f64 = shown.iter().sum::<f64>().max(f64::EPSILON);
    let d = if narrow { 148. } else { 172. };
    let thick = if narrow { 16. } else { 19. };
    let sweep = super::ease_out(frame.enter.min(1.) * 1.0);
    let hover = frame.hover.filter(|h| shown.get(*h).is_some_and(|v| *v > 0.));
    let lift = frame.lift;
    let paint = {
        let (shown, colors, track) = (shown.clone(), colors.clone(), look.grid);
        move |b: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
            let c = b.center();
            let outer = f32::from(b.size.width.min(b.size.height)) / 2. - 4.;
            let inner = outer - thick;
            // The faint full ring the parts sit on.
            ring(window, c, outer, inner, 0., TAU, track, 96);
            let pad = 1.5 / ((outer + inner) / 2.);
            let mut a = -FRAC_PI_2;
            for (i, v) in shown.iter().enumerate() {
                if *v <= 0. {
                    continue;
                }
                let span = (*v / total) as f32 * TAU * sweep;
                let (a0, a1) = (a + pad, a + span - pad);
                a += span;
                if a1 <= a0 {
                    continue;
                }
                let picked = hover == Some(i);
                let grow = if picked { 3. * lift } else { 0. };
                let alpha = match hover {
                    Some(h) if h != i => 1. - 0.6 * lift,
                    _ => 1.,
                };
                ring(window, c, outer + grow, inner, a0, a1, colors[i].opacity(alpha), 96);
            }
        }
    };
    let state = frame.state();
    let move_state = state.clone();
    let center = match hover {
        Some(h) => (with_unit(format_number(parts[h].value), unit), parts[h].label.clone(), Some(format!("{:.1}%", shown[h] / total * 100.))),
        None => (with_unit(format_number(shown.iter().sum()), unit), "Total".to_string(), None),
    };
    let chart = div()
        .id(frame.id("donutplot", 0))
        .relative()
        .flex_none()
        .size(px(d))
        .on_hover(move |hovered, _, cx| {
            if !hovered {
                Frame::set_hover(&state, None, cx);
            }
        })
        .child({
            // Which part is under the pointer: by its angle, within the ring.
            let shown = shown.clone();
            let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
            let b2 = bounds.clone();
            div()
                .absolute()
                .inset_0()
                .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
                    let b = b2.get();
                    let c = b.center();
                    let (dx, dy) = (f32::from(e.position.x - c.x), f32::from(e.position.y - c.y));
                    let r = dx.hypot(dy);
                    let outer = f32::from(b.size.width) / 2. - 4.;
                    if r > outer + 6. || r < outer - thick - 6. {
                        Frame::set_hover(&move_state, None, cx);
                        return;
                    }
                    let angle = (dy.atan2(dx) + FRAC_PI_2).rem_euclid(TAU) / TAU;
                    let mut acc = 0.;
                    let mut found = None;
                    for (i, v) in shown.iter().enumerate() {
                        acc += *v / total;
                        if *v > 0. && (angle as f64) <= acc {
                            found = Some(i);
                            break;
                        }
                    }
                    Frame::set_hover(&move_state, found, cx);
                })
                .child(canvas(move |b, _, _| bounds.set(b), paint).size_full())
        })
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .gap(px(1.))
                .child(div().text_size(px(if narrow { 18. } else { 21. })).font_semibold().text_color(look.fg).child(center.0))
                .child(div().max_w(px(d - thick * 2. - 16.)).truncate().text_size(px(11.)).text_color(look.muted).child(center.1))
                .when_some(center.2, |el, share| el.child(div().font_family(look.mono.clone()).text_size(px(10.5)).text_color(look.muted).child(share))),
        );
    let n = parts.len();
    let rows = v_flex().flex_1().min_w(px(200.)).max_w(px(460.)).gap(px(2.)).children(parts.iter().enumerate().map(|(ix, part)| {
        let off = frame.hidden.contains(&ix);
        let share = if off { "—".to_string() } else { format!("{:.0}%", shown[ix] / total * 100.) };
        h_flex()
            .id(frame.id("part", ix))
            .test_support()
            .gap(px(10.))
            .h(px(26.))
            .px(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(hover == Some(ix), |el| el.bg(look.fg.opacity(0.05)))
            .on_hover(frame.on_hover(ix))
            .on_click(frame.toggle(ix, n))
            .opacity(if off { 0.45 } else { frame.presence(ix).max(0.5) })
            .child(div().flex_none().size(px(9.)).rounded(px(2.5)).bg(if off { look.muted.opacity(0.5) } else { colors[ix] }))
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(look.fg).when(off, |el| el.line_through()).child(part.label.clone()))
            .child(div().font_family(look.mono.clone()).text_size(px(11.5)).text_color(look.fg.opacity(0.85)).child(with_unit(format_number(part.value), unit)))
            .child(div().w(px(38.)).text_right().font_family(look.mono.clone()).text_size(px(11.)).text_color(look.muted).child(share))
    }));
    inset(&look)
        .id(frame.id("donut", 0))
        .test_support()
        .w_full()
        .p_4()
        .child(h_flex().w_full().flex_wrap().items_center().gap(px(if narrow { 16. } else { 28. })).when(narrow, |el| el.justify_center()).child(chart).child(rows))
        .into_any_element()
}

/// An annular sector from angle `a0` to `a1` (radians, clockwise from three o'clock).
fn ring(window: &mut Window, c: Point<Pixels>, outer: f32, inner: f32, a0: f32, a1: f32, color: Hsla, steps: usize) {
    if color.a <= 0. {
        return;
    }
    let n = (((a1 - a0) / TAU) * steps as f32).ceil().max(2.) as usize;
    let at = |r: f32, a: f32| point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
    let mut pts: Vec<Point<Pixels>> = (0..=n).map(|i| at(outer, a0 + (a1 - a0) * i as f32 / n as f32)).collect();
    pts.extend((0..=n).rev().map(|i| at(inner, a0 + (a1 - a0) * i as f32 / n as f32)));
    let mut path = PathBuilder::fill();
    path.add_polygon(&pts, true);
    if let Ok(p) = path.build() {
        window.paint_path(p, color);
    }
}

// ---------- stats ----------

/// A delta's direction from its sign: +1 up, -1 down, 0 flat or unsigned.
fn direction(delta: &str) -> i8 {
    match delta.trim_start().chars().next() {
        Some('+' | '↑' | '▲') => 1,
        Some('-' | '−' | '–' | '↓' | '▼') => -1,
        _ => 0,
    }
}

pub(super) fn sparkline(values: Vec<f64>, line: Hsla, accent: Hsla, gap: Hsla, reveal: f32) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let n = values.len();
            let (lo, hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            let span = (hi - lo).max(f64::EPSILON);
            let inner = Bounds::new(b.origin + point(px(3.), px(3.)), size(b.size.width - px(6.), b.size.height - px(6.)));
            let at = |i: usize, v: f64| {
                let y = if hi > lo { 1. - ((v - lo) / span) as f32 } else { 0.5 };
                point(inner.origin.x + inner.size.width * (i as f32 / (n - 1).max(1) as f32), inner.origin.y + inner.size.height * y)
            };
            let pts: Vec<Point<Pixels>> = values.iter().enumerate().map(|(i, v)| at(i, *v)).collect();
            let clip = Bounds::new(b.origin, size(b.size.width * reveal, b.size.height));
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                let mut area = PathBuilder::fill();
                area.move_to(point(pts[0].x, b.origin.y + b.size.height));
                area.line_to(pts[0]);
                monotone(&mut area, &pts, false);
                area.line_to(point(pts[n - 1].x, b.origin.y + b.size.height));
                area.close();
                if let Ok(p) = area.build() {
                    window.paint_path(p, accent.opacity(0.09));
                }
                let mut stroke = PathBuilder::stroke(px(1.5));
                monotone(&mut stroke, &pts, true);
                if let Ok(p) = stroke.build() {
                    window.paint_path(p, line);
                }
            });
            if reveal >= 1. {
                dot(window, pts[n - 1], px(4.), gap);
                dot(window, pts[n - 1], px(2.5), accent);
            }
        },
    )
}

pub(super) fn stats(tiles: &[StatTile], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let n = tiles.len();
    h_flex()
        .id(frame.id("stats", 0))
        .test_support()
        .w_full()
        .flex_wrap()
        .items_stretch()
        .gap(px(8.))
        .children(tiles.iter().enumerate().map(|(ix, tile)| {
            let dir = tile.delta.as_deref().map(direction).unwrap_or(0);
            let status = match tile.tone {
                Some(t) => tone(Some(t), cx),
                None => look.muted,
            };
            let accent = match tile.tone {
                Some(SemanticTone::Neutral) | None => crate::palette::sky(cx),
                Some(t) => tone(Some(t), cx),
            };
            let arrive = frame.arrive(ix, n);
            inset(&look)
                .id(frame.id("stat", ix))
                .test_support()
                .flex_1()
                .min_w(px(if frame.narrow() { 132. } else { 156. }))
                .px(px(14.))
                .pt(px(12.))
                .pb(px(if tile.trend.is_some() { 8. } else { 12. }))
                .opacity(0.25 + 0.75 * arrive)
                .child(
                    v_flex()
                        .gap(px(4.))
                        .child(div().truncate().text_size(px(11.5)).text_color(look.muted).child(tile.label.clone()))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .items_center()
                                .gap_x(px(8.))
                                .gap_y(px(2.))
                                .child(div().text_size(px(22.)).font_semibold().line_height(relative(1.2)).text_color(look.fg).child(tile.value.clone()))
                                .when_some(tile.delta.clone(), |el, delta| {
                                    el.child(
                                        h_flex()
                                            .gap(px(2.))
                                            .h(px(18.))
                                            .px(px(6.))
                                            .rounded_full()
                                            .bg(status.opacity(0.12))
                                            .text_color(status)
                                            .when(dir != 0, |el| el.child(Icon::new(if dir > 0 { Lucide::ArrowUpRight } else { Lucide::ArrowDownRight }).size(px(11.)).text_color(status)))
                                            .child(div().text_size(px(11.)).font_medium().child(delta)),
                                    )
                                }),
                        )
                        .when_some(tile.trend.clone(), |el, trend| el.child(sparkline(trend, look.muted.opacity(0.85), accent, look.gap, arrive).w_full().h(px(30.)))),
                )
        }))
        .into_any_element()
}

// ---------- table ----------

pub(super) fn table_view(columns: &[String], rows: &[(Vec<TableCell>, Option<SemanticTone>)], frame: &Frame, part: &str, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let numeric: Vec<bool> = (0..columns.len()).map(|c| rows.iter().any(|r| matches!(r.0.get(c), Some(TableCell::Number(_)))) && rows.iter().all(|r| !matches!(r.0.get(c), Some(TableCell::Text(t)) if !t.is_empty() && t != "—"))).collect();
    // Columns share the width by what they hold (a long name column, short figures), and the
    // table scrolls sideways rather than wrapping a number.
    let widths: Vec<f32> = (0..columns.len())
        .map(|c| {
            let longest = rows
                .iter()
                .map(|r| match r.0.get(c) {
                    Some(TableCell::Number(n)) => super::format_exact(*n).chars().count(),
                    Some(TableCell::Text(t)) => t.chars().count().min(28),
                    None => 0,
                })
                .chain(std::iter::once(columns[c].chars().count().min(20)))
                .max()
                .unwrap_or(4);
            (longest as f32 * 7.2 + 22.).clamp(56., 240.)
        })
        .collect();
    let min_w = px(widths.iter().sum::<f32>() + 8.);
    let cell = |c: usize, el: Div| {
        let el = el.min_w_0().px(px(10.));
        let el = el.flex_grow(widths[c]).flex_shrink(1.).flex_basis(relative(0.));
        if numeric[c] { el.text_right() } else { el }
    };
    let header = h_flex().w_full().py(px(7.)).border_b_1().border_color(look.hairline).children(columns.iter().enumerate().map(|(c, name)| {
        cell(c, div()).truncate().text_size(px(11.)).font_medium().text_color(look.muted).child(name.clone())
    }));
    let body = rows.iter().enumerate().map(|(ix, (cells, row_tone))| {
        let tint = row_tone.map(|t| tone(Some(t), cx));
        h_flex()
            .id(frame.id(&format!("{part}row"), ix))
            .relative()
            .w_full()
            .py(px(7.))
            .items_start()
            .when(ix + 1 < rows.len(), |el| el.border_b_1().border_color(look.grid))
            .hover(|s| s.bg(look.fg.opacity(0.025)))
            .when_some(tint, |el, t| el.bg(t.opacity(0.07)).child(div().absolute().left_0().top(px(5.)).bottom(px(5.)).w(px(2.)).rounded_full().bg(t)))
            .children(cells.iter().enumerate().map(|(c, value)| {
                let text = match value {
                    // A table is where the exact figure lives: grouped, never abbreviated.
                    TableCell::Number(n) => super::format_exact(*n),
                    TableCell::Text(t) => t.clone(),
                };
                cell(c, div())
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(if c == 0 { look.fg } else { look.fg.opacity(0.86) })
                    .when(matches!(value, TableCell::Number(_)), |el| el.whitespace_nowrap().font_family(look.mono.clone()).text_size(px(11.5)))
                    .child(text)
            }))
    });
    // Wider than the column: it scrolls sideways, and a fade at the edge says so.
    let overflows = f32::from(min_w) > frame.width;
    let fade = look.gap;
    div()
        .relative()
        .w_full()
        .child(
            div()
                .id(frame.id(part, 0))
                .test_support()
                .w_full()
                .overflow_x_scroll()
                .child(inset(&look).min_w(min_w).px_1().pb_1().child(header).children(body)),
        )
        .when(overflows, |el| {
            el.child(div().absolute().top_0().bottom_0().right_0().w(px(28.)).rounded_r(px(9.)).bg(linear_gradient(90., linear_color_stop(fade.opacity(0.), 0.), linear_color_stop(fade.opacity(0.9), 1.))))
        })
        .into_any_element()
}

// ---------- heatmap ----------

pub(super) fn heatmap(x_labels: &[String], y_labels: &[String], values: &[Vec<f64>], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let (min, max) = values.iter().flatten().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    // Values on both sides of zero diverge from it (two hues, nothing at zero); otherwise one
    // hue, light to dark.
    let diverging = min < 0. && max > 0.;
    let reach = min.abs().max(max.abs()).max(f64::EPSILON);
    let span = (max - min).max(f64::EPSILON);
    let (pos, neg) = (crate::palette::sky(cx), crate::palette::red(cx));
    let paint = move |v: f64| -> (Hsla, f32) {
        if diverging {
            let s = (v.abs() / reach) as f32;
            (if v < 0. { neg } else { pos }, s)
        } else {
            (pos, ((v - min) / span) as f32)
        }
    };
    let label_w = if frame.narrow() { 64. } else { 88. };
    let cols = x_labels.len();
    let cell_w = (frame.width - 32. - label_w) / cols as f32;
    let every = (40. / cell_w.max(1.)).ceil().max(1.) as usize;
    let numbers = cell_w >= 40.;
    let hovered = frame.hover.map(|h| (h / cols, h % cols));
    let header = h_flex().gap(px(3.)).child(div().w(px(label_w)).flex_none()).children(x_labels.iter().enumerate().map(|(c, label)| {
        div()
            .flex_1()
            .min_w_0()
            .text_center()
            .truncate()
            .text_size(px(10.))
            .text_color(if hovered.is_some_and(|h| h.1 == c) { look.fg } else { look.muted })
            .child(if c % every == 0 { label.clone() } else { String::new() })
    }));
    let rows = values.iter().enumerate().map(|(r, row)| {
        let label = y_labels.get(r).cloned().unwrap_or_default();
        h_flex()
            .gap(px(3.))
            .child(div().w(px(label_w)).flex_none().pr_2().truncate().text_size(px(11.)).text_color(if hovered.is_some_and(|h| h.0 == r) { look.fg } else { look.muted }).child(label.clone()))
            .children(row.iter().enumerate().map(|(c, v)| {
                let (color, strength) = paint(*v);
                let ix = r * cols + c;
                let arrive = frame.arrive(c, cols);
                let tip = format!("{} · {}: {}", label, x_labels.get(c).map(String::as_str).unwrap_or(""), format_number(*v));
                let alpha = (0.07 + strength.clamp(0., 1.) * 0.85) * arrive;
                let on_dark = look.dark == (strength < 0.55);
                div()
                    .id(frame.id("cell", ix))
                    .test_support()
                    .flex_1()
                    .min_w(px(8.))
                    .h(px(24.))
                    .rounded(px(4.))
                    .bg(color.opacity(alpha))
                    .when(frame.hover == Some(ix), |el| el.border_1().border_color(look.fg.opacity(0.7)))
                    .when(numbers, |el| {
                        el.flex().items_center().justify_center().font_family(look.mono.clone()).text_size(px(10.)).text_color(if on_dark { gpui_kit::white().opacity(0.92) } else { gpui_kit::black().opacity(0.8) }).child(format_number(*v))
                    })
                    .on_hover(frame.on_hover(ix))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            }))
    });
    let legend_bar = if diverging {
        linear_gradient(90., linear_color_stop(neg.opacity(0.9), 0.), linear_color_stop(pos.opacity(0.9), 1.))
    } else {
        linear_gradient(90., linear_color_stop(pos.opacity(0.07), 0.), linear_color_stop(pos.opacity(0.92), 1.))
    };
    let scale = h_flex()
        .gap(px(8.))
        .justify_end()
        .font_family(look.mono.clone())
        .text_size(px(10.))
        .text_color(look.muted)
        .child(format_number(if diverging { -reach } else { min }))
        .child(div().w(px(112.)).h(px(6.)).rounded_full().bg(legend_bar))
        .child(format_number(if diverging { reach } else { max }));
    v_flex()
        .id(frame.id("heatmap", 0))
        .test_support()
        .w_full()
        .gap(px(8.))
        .child(inset(&look).p_3().child(v_flex().gap(px(3.)).child(header).children(rows)))
        .child(scale)
        .into_any_element()
}

// ---------- treemap ----------

pub(super) fn treemap(items: &[TreemapItem], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let w = (frame.width - 2.).max(120.);
    let h = if frame.narrow() { 220. } else { (w * 0.42).clamp(220., 320.) };
    let tiles = squarify(&items.iter().map(|i| i.weight).collect::<Vec<_>>(), Rect { x: 0., y: 0., w, h });
    let total: f64 = items.iter().map(|i| i.weight).sum();
    let max = items.iter().map(|i| i.weight).fold(0f64, f64::max).max(f64::EPSILON);
    let mut by_weight: Vec<usize> = (0..items.len()).collect();
    by_weight.sort_by(|a, b| items[*b].weight.total_cmp(&items[*a].weight));
    let n = items.len();
    div()
        .id(frame.id("treemap", 0))
        .test_support()
        .relative()
        .w_full()
        .h(px(h))
        .children(items.iter().enumerate().map(|(ix, item)| {
            let r = tiles[ix];
            // Untoned tiles are quiet greys stepped by weight, so a toned one stands out.
            let color = if item.tone.is_some() { tone(item.tone, cx) } else { look.fg };
            let toned = item.tone.is_some();
            let rank = by_weight.iter().position(|i| *i == ix).unwrap_or(0);
            let arrive = frame.arrive(rank, n);
            let strength = if toned { 0.16 + 0.22 * (item.weight / max).sqrt() as f32 } else { 0.03 + 0.07 * (item.weight / max).sqrt() as f32 };
            let hovered = frame.hover == Some(ix);
            let share = item.weight / total * 100.;
            let (tw, th) = (r.w - 3., r.h - 3.);
            let tip = format!("{} · {} · {:.1}%", item.label, format_number(item.weight), share);
            div()
                .absolute()
                .left(relative(r.x / w))
                .top(relative(r.y / h))
                .w(relative(r.w / w))
                .h(relative(r.h / h))
                .p(px(1.5))
                .child(
                    v_flex()
                        .id(frame.id("tile", ix))
                        .test_support()
                        .size_full()
                        .gap(px(3.))
                        .p(px(8.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(color.opacity(if hovered { 0.6 } else if toned { 0.32 } else { 0.07 }))
                        .bg(color.opacity((strength + if hovered { if toned { 0.1 } else { 0.04 } } else { 0. }) * arrive))
                        .opacity(frame.presence(ix).max(0.45))
                        .overflow_hidden()
                        .on_hover(frame.on_hover(ix))
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                        .when(tw > 54. && th > 30., |el| {
                            el.child(div().w_full().truncate().text_size(px(11.5)).font_medium().line_height(relative(1.3)).text_color(look.fg).child(item.label.clone()))
                                .when(th > 48., |el| {
                                    el.child(h_flex().gap(px(6.)).font_family(look.mono.clone()).text_size(px(10.5)).child(div().text_color(look.fg.opacity(0.8)).child(format_number(item.weight))).when(tw > 96., |el| el.child(div().text_color(look.muted).child(format!("{share:.0}%")))))
                                })
                        }),
                )
        }))
        .into_any_element()
}
