//! Diagrams of structure rather than values: timelines, flows and layer stacks.

use super::charts::{dot, hline, vline};
use super::layout::{Scale, flow_layout, pack};
use super::{Frame, format_tick, inset, tone, with_unit};
use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::rc::Rc;
use trek_core::visualization::{FlowEdge, FlowNode, Layer, TimePoint, TimelineEvent};

// ---------- timeline ----------

/// Where an event starts and ends on the axis, and whether it is a milestone (no end).
fn span(event: &TimelineEvent, ticks: Option<&[String]>) -> (f64, f64, bool) {
    let at = |p: &TimePoint| match p {
        TimePoint::Number(n) => *n,
        TimePoint::Tick(t) => ticks.and_then(|ts| ts.iter().position(|x| x == t)).unwrap_or(0) as f64,
    };
    let start = at(&event.start);
    match (&event.end, ticks) {
        (None, Some(_)) => (start + 0.5, start + 0.5, true),
        (None, None) => (start, start, true),
        // A tick names a whole slot: Oct to Nov runs to the end of November.
        (Some(end), Some(_)) => (start, at(end) + 1., false),
        (Some(end), None) => (start, at(end), false),
    }
}

const EVENT_ROW: f32 = 28.;

pub(super) fn timeline(events: &[TimelineEvent], ticks: Option<&[String]>, unit: Option<&str>, frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let narrow = frame.narrow();
    let spans: Vec<(f64, f64, bool)> = events.iter().map(|e| span(e, ticks)).collect();
    // The axis: the ticks' slots, or a nice scale over the numbers.
    let (lo, hi, marks): (f64, f64, Vec<(f64, String, f64)>) = match ticks {
        Some(ts) => (0., ts.len() as f64, ts.iter().enumerate().map(|(i, t)| (i as f64 + 0.5, t.clone(), i as f64)).collect()),
        None => {
            let min = spans.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
            let max = spans.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
            let scale = Scale::nice(min, max, if narrow { 4 } else { 6 });
            (scale.lo, scale.hi, scale.ticks().into_iter().map(|t| (t, with_unit(format_tick(t, scale.step), unit), t)).collect())
        }
    };
    let frac = move |v: f64| ((v - lo) / (hi - lo).max(f64::EPSILON)) as f32;
    let lanes: Vec<String> = events.iter().filter_map(|e| e.lane.clone()).fold(Vec::new(), |mut acc, l| {
        if !acc.contains(&l) {
            acc.push(l);
        }
        acc
    });
    let laned = !lanes.is_empty();
    // Rows: one per event, or per lane with its events packed into as few rows as fit.
    let mut rows: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    if laned {
        let mut lane_names = lanes.clone();
        if events.iter().any(|e| e.lane.is_none()) {
            lane_names.push(String::new());
        }
        for lane in lane_names {
            let members: Vec<usize> = (0..events.len()).filter(|i| events[*i].lane.clone().unwrap_or_default() == lane).collect();
            // Touching spans share a row; a milestone keeps room for its label.
            let packed = pack(&members.iter().map(|i| (spans[*i].0, if spans[*i].2 { spans[*i].0 + (hi - lo) * 0.12 } else { spans[*i].1 })).collect::<Vec<_>>(), 0.);
            let count = packed.iter().max().map_or(0, |m| m + 1);
            for r in 0..count {
                let row: Vec<usize> = members.iter().zip(&packed).filter(|(_, p)| **p == r).map(|(i, _)| *i).collect();
                rows.push((if r == 0 { Some(if lane.is_empty() { "Other".into() } else { lane.clone() }) } else { None }, row));
            }
        }
    } else {
        rows = (0..events.len()).map(|i| (Some(events[i].label.clone()), vec![i])).collect();
    }
    let n = events.len();
    let label_w = if narrow { relative(0.34) } else { relative(0.24) };
    let grid_at: Vec<f32> = match ticks {
        Some(ts) => (0..=ts.len()).map(|i| i as f32 / ts.len() as f32).collect(),
        None => marks.iter().map(|m| frac(m.2)).collect(),
    };
    let (grid, hairline) = (look.grid, look.hairline);
    let first_of_lane: Vec<bool> = rows.iter().map(|r| r.0.is_some()).collect();
    let row_h = EVENT_ROW;
    let gridlines = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            for g in &grid_at {
                vline(window, b.origin.x + b.size.width * *g, b.origin.y, b.size.height, grid);
            }
            if laned {
                for (r, first) in first_of_lane.iter().enumerate() {
                    if *first && r > 0 {
                        hline(window, b.origin.x, b.origin.y + px(r as f32 * row_h), b.size.width, hairline);
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0();
    let axis = h_flex().h(px(16.)).child(div().w(label_w).max_w(px(180.)).flex_none()).child(div().relative().flex_1().children(marks.iter().map(|(at, label, _)| {
        // Numbers in mono, so they line up; named ticks (months, phases) as words.
        div().absolute().top_0().left(relative(frac(*at))).ml(px(-40.)).w(px(80.)).flex().justify_center().child(div().whitespace_nowrap().when(ticks.is_none(), |el| el.font_family(look.mono.clone())).text_size(if ticks.is_some() { px(11.) } else { px(10.) }).text_color(look.muted).child(label.clone()))
    })));
    let labels = v_flex().w(label_w).max_w(px(180.)).flex_none().children(rows.iter().enumerate().map(|(r, (label, members))| {
        let solo = !laned && members.len() == 1;
        div()
            .h(px(row_h))
            .pr_3()
            .flex()
            .items_center()
            .when(laned && label.is_some() && r > 0, |el| el.border_t_1().border_color(look.hairline))
            .when_some(label.clone(), |el, label| {
                el.child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(11.5))
                        .when(laned, |el| el.font_medium().text_color(look.fg))
                        .when(solo, |el| el.text_color(look.fg.opacity(0.86 * frame.presence(members[0]))))
                        .child(label),
                )
            })
    }));
    let track_rows = v_flex().children(rows.iter().map(|(_, members)| {
        div().relative().h(px(row_h)).children(members.iter().map(|ix| {
            let ix = *ix;
            let event = &events[ix];
            let (start, end, milestone) = spans[ix];
            let color = tone(event.tone, cx);
            let grown = frame.arrive(ix, n);
            let presence = frame.presence(ix);
            let hovered = frame.hover == Some(ix);
            let when = |p: &TimePoint| match p {
                TimePoint::Number(v) => with_unit(format_tick(*v, 1.), unit),
                TimePoint::Tick(t) => t.clone(),
            };
            let tip = match &event.end {
                Some(e) => format!("{} · {} – {}", event.label, when(&event.start), when(e)),
                None => format!("{} · {}", event.label, when(&event.start)),
            };
            if milestone {
                let diamond = canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        let c = b.center();
                        let r = px(6.);
                        let mut p = PathBuilder::fill();
                        p.add_polygon(&[point(c.x, c.y - r), point(c.x + r, c.y), point(c.x, c.y + r), point(c.x - r, c.y)], true);
                        if let Ok(p) = p.build() {
                            window.paint_path(p, color.opacity(presence));
                        }
                    },
                )
                .size(px(14.));
                h_flex()
                    .id(frame.id("event", ix))
                    .test_support()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(frac(start)))
                    .ml(px(-7.))
                    .gap(px(5.))
                    .opacity(grown)
                    .on_hover(frame.on_hover(ix))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .child(diamond)
                    .when(laned, |el| el.child(div().whitespace_nowrap().text_size(px(11.)).text_color(look.fg.opacity(0.86 * presence)).child(event.label.clone())))
                    .into_any_element()
            } else {
                let (l, r) = (frac(start), frac(start) + (frac(end) - frac(start)) * grown);
                div()
                    .id(frame.id("event", ix))
                    .test_support()
                    .absolute()
                    .top(px((row_h - 18.) / 2.))
                    .h(px(18.))
                    .left(relative(l))
                    .w(relative((r - l).max(0.)))
                    .min_w(px(4.))
                    .px(px(1.))
                    .on_hover(frame.on_hover(ix))
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .child(
                        div()
                            .size_full()
                            .rounded(px(4.))
                            .when(!laned, |el| el.bg(color.opacity(if hovered { 1. } else { 0.85 * presence })))
                            .when(laned, |el| {
                                el.bg(color.opacity(if hovered { 0.34 } else { 0.2 * presence }))
                                    .border_1()
                                    .border_color(color.opacity(0.55 * presence))
                                    .flex()
                                    .items_center()
                                    .px(px(6.))
                                    .overflow_hidden()
                                    .child(div().truncate().text_size(px(10.5)).font_medium().text_color(look.fg.opacity(presence.max(0.4))).child(event.label.clone()))
                            }),
                    )
                    .into_any_element()
            }
        }))
    }));
    v_flex()
        .id(frame.id("timeline", 0))
        .test_support()
        .w_full()
        .child(
            inset(&look).p_3().pr(px(20.)).child(
                v_flex().gap(px(6.)).child(axis).child(h_flex().items_start().child(labels).child(div().relative().flex_1().min_w_0().child(gridlines).child(track_rows))),
            ),
        )
        .into_any_element()
}

// ---------- flow ----------

pub(super) fn flow(nodes: &[FlowNode], edges: &[FlowEdge], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let narrow = frame.narrow();
    let index = |id: &str| nodes.iter().position(|n| n.id == id).unwrap_or(0);
    let pairs: Vec<(usize, usize)> = edges.iter().map(|e| (index(&e.from), index(&e.to))).collect();
    let group_names: Vec<String> = nodes.iter().filter_map(|n| n.group.clone()).fold(Vec::new(), |mut acc, g| {
        if !acc.contains(&g) {
            acc.push(g);
        }
        acc
    });
    let groups: Vec<Option<usize>> = nodes.iter().map(|n| n.group.as_ref().and_then(|g| group_names.iter().position(|x| x == g))).collect();
    let layout = flow_layout(nodes.len(), &pairs, &groups);
    let grouped = !group_names.is_empty();
    let details = !narrow && nodes.iter().any(|n| n.detail.is_some());
    let node_h = if details { 46. } else { 32. };
    let gap = if grouped { 52. } else { 38. };
    let top = if grouped { 26. } else { 6. };
    let slots = layout.slots as f32;
    let rank_top = move |r: usize| top + r as f32 * (node_h + gap);
    let height = rank_top(layout.ranks - 1) + node_h + if grouped { 14. } else { 8. } + if layout.back.iter().any(|b| *b) { 4. } else { 0. };
    let max_node = 220.;
    // A group gets a container when one fits around its nodes and nobody else's; groups that
    // interleave with others are named on their nodes instead.
    let boxed: Vec<bool> = (0..group_names.len())
        .map(|g| {
            let members: Vec<usize> = (0..nodes.len()).filter(|v| groups[*v] == Some(g)).collect();
            let (lo, hi) = members.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(layout.x[*v]), hi.max(layout.x[*v])));
            let (r0, r1) = members.iter().fold((usize::MAX, 0), |(lo, hi), v| (lo.min(layout.rank[*v]), hi.max(layout.rank[*v])));
            !(0..nodes.len()).any(|v| groups[v] != Some(g) && (r0..=r1).contains(&layout.rank[v]) && layout.x[v] > lo - 0.99 && layout.x[v] < hi + 0.99)
        })
        .collect();
    let pad = 6.;
    let colors: Vec<Hsla> = nodes.iter().map(|n| if n.tone.is_some() { tone(n.tone, cx) } else { look.muted }).collect();
    let hover = frame.hover;
    let linked = |e: usize| hover.is_some_and(|h| pairs[e].0 == h || pairs[e].1 == h);
    let near = |v: usize| hover.is_none_or(|h| h == v || pairs.iter().any(|(a, b)| (*a == h && *b == v) || (*b == h && *a == v)));
    let enter = frame.arrive(0, 1);
    let ranks = layout.ranks;
    let node_alpha: Vec<f32> = (0..nodes.len()).map(|v| frame.arrive(layout.rank[v], ranks)).collect();
    let paint = {
        let (layout, pairs, groups, colors) = (layout.clone(), pairs.clone(), groups.clone(), colors.clone());
        let edge_color = look.fg.opacity(if look.dark { 0.26 } else { 0.24 });
        let (group_fill, group_edge) = (look.fg.opacity(if look.dark { 0.022 } else { 0.03 }), look.hairline);
        let lit: Vec<bool> = (0..pairs.len()).map(linked).collect();
        let lit_color = match hover {
            Some(h) if nodes[h].tone.is_some() => colors[h],
            _ => look.fg.opacity(0.75),
        };
        let dimmed = hover.is_some();
        let group_count = group_names.len();
        let boxed = boxed.clone();
        move |b: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
            let slot_w = b.size.width / slots;
            let node_w = (slot_w - px(pad * 2.)).min(px(max_node));
            let cx_of = |v: usize| b.origin.x + slot_w * (layout.x[v] + 0.5);
            let top_of = |v: usize| b.origin.y + px(rank_top(layout.rank[v]));
            for g in (0..group_count).filter(|g| boxed[*g]) {
                let members: Vec<usize> = (0..groups.len()).filter(|v| groups[*v] == Some(g)).collect();
                if members.is_empty() {
                    continue;
                }
                let (lo, hi) = members.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(layout.x[*v]), hi.max(layout.x[*v])));
                let (r0, r1) = members.iter().fold((usize::MAX, 0), |(lo, hi), v| (lo.min(layout.rank[*v]), hi.max(layout.rank[*v])));
                let left = b.origin.x + slot_w * lo + px(2.);
                let right = b.origin.x + slot_w * (hi + 1.) - px(2.);
                let rect = Bounds::new(point(left, b.origin.y + px(rank_top(r0) - 22.)), size(right - left, px(rank_top(r1) - rank_top(r0) + node_h + 30.)));
                window.paint_quad(fill(rect, group_fill).corner_radii(px(10.)).border_widths(px(1.)).border_color(group_edge));
            }
            for (e, (from, to)) in pairs.iter().enumerate() {
                // The hovered node's edges light up in its tone (or ink, untoned).
                let color = if lit[e] { lit_color } else if dimmed { edge_color.opacity(0.45) } else { edge_color };
                let color = color.opacity(enter);
                let width = if lit[e] { px(1.75) } else { px(1.25) };
                let mut path = PathBuilder::stroke(width);
                let (tip, dir);
                if layout.back[e] || layout.rank[*to] <= layout.rank[*from] {
                    // Back up the ranks: out of the source's right side and round into the target's.
                    let s = point(cx_of(*from) + node_w / 2., top_of(*from) + px(node_h / 2.));
                    let t = point(cx_of(*to) + node_w / 2. + px(2.), top_of(*to) + px(node_h / 2.));
                    let bulge = px(26.);
                    path.move_to(s);
                    path.cubic_bezier_to(t, point(s.x + bulge, s.y), point(t.x + bulge, t.y));
                    tip = t;
                    dir = (-1., 0.);
                } else {
                    // Down through each rank it crosses, in its own slot there.
                    let slot_x = |x: f32| b.origin.x + slot_w * (x + 0.5);
                    let mut way = vec![point(cx_of(*from), top_of(*from) + px(node_h))];
                    for (r, x) in &layout.routes[e] {
                        way.push(point(slot_x(*x), b.origin.y + px(rank_top(*r))));
                        way.push(point(slot_x(*x), b.origin.y + px(rank_top(*r) + node_h)));
                    }
                    let t = point(cx_of(*to), top_of(*to) - px(2.));
                    way.push(t);
                    path.move_to(way[0]);
                    for i in 0..way.len() - 1 {
                        let (s, t) = (way[i], way[i + 1]);
                        if i % 2 == 1 {
                            path.line_to(t);
                        } else {
                            let dy = (t.y - s.y) * 0.5;
                            path.cubic_bezier_to(t, point(s.x, s.y + dy), point(t.x, t.y - dy));
                        }
                    }
                    tip = t;
                    dir = (0., 1.);
                }
                if let Ok(p) = path.build() {
                    window.paint_path(p, color);
                }
                // The arrowhead, pointing along the curve's end.
                let (dx, dy) = dir;
                let (l, w) = (6.5, 3.6);
                let base = point(tip.x - px(dx * l), tip.y - px(dy * l));
                let mut head = PathBuilder::fill();
                head.add_polygon(&[tip, point(base.x + px(-dy * w), base.y + px(dx * w)), point(base.x - px(-dy * w), base.y - px(dx * w))], true);
                if let Ok(p) = head.build() {
                    window.paint_path(p, color);
                }
            }
        }
    };
    let node_els = nodes.iter().enumerate().map(|(v, node)| {
        let hovered = hover == Some(v);
        let presence = if near(v) { 1. } else { 0.4 };
        let color = colors[v];
        let toned = node.tone.is_some();
        let tag = (!narrow).then_some(()).and(groups[v].filter(|g| !boxed[*g]).map(|g| group_names[g].clone()));
        div()
            .absolute()
            .left(relative(layout.x[v] / slots))
            .w(relative(1. / slots))
            .top(px(rank_top(layout.rank[v])))
            .h(px(node_h))
            .px(px(pad))
            .flex()
            .justify_center()
            .opacity(node_alpha[v] * presence)
            .child(
                h_flex()
                    .id(frame.id("node", v))
                    .test_support()
                    .relative()
                    .w_full()
                    .max_w(px(max_node))
                    .h_full()
                    .pl(px(if toned { 12. } else { 10. }))
                    .pr(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if hovered { color.opacity(0.8) } else { look.fg.opacity(if look.dark { 0.13 } else { 0.12 }) })
                    .bg(look.popover)
                    .shadow(vec![BoxShadow { color: gpui_kit::black().opacity(if look.dark { 0.35 } else { 0.06 }), offset: point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false }])
                    .overflow_hidden()
                    .on_hover(frame.on_hover(v))
                    .when(toned, |el| el.child(div().absolute().left(px(4.)).top(px(7.)).bottom(px(7.)).w(px(3.)).rounded_full().bg(color)))
                    .child(
                        v_flex()
                            .min_w_0()
                            .w_full()
                            .gap(px(1.))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap(px(6.))
                                    .child(div().flex_1().min_w_0().truncate().text_size(px(if narrow { 11. } else { 12. })).font_medium().text_color(look.fg).child(node.label.clone()))
                                    .when_some(tag.clone(), |el, tag| el.child(div().flex_none().text_size(px(9.5)).text_color(look.muted).child(tag))),
                            )
                            .when(details, |el| el.child(div().w_full().truncate().text_size(px(10.5)).text_color(look.muted).child(node.detail.clone().unwrap_or_default()))),
                    ),
            )
    });
    let group_labels = group_names.iter().enumerate().filter(|(g, _)| boxed[*g]).filter_map(|(g, name)| {
        let members: Vec<usize> = (0..nodes.len()).filter(|v| groups[*v] == Some(g)).collect();
        let lo = members.iter().map(|v| layout.x[*v]).fold(f32::MAX, f32::min);
        let r0 = members.iter().map(|v| layout.rank[*v]).min()?;
        Some(
            div()
                .absolute()
                .left(relative(lo / slots))
                .ml(px(12.))
                .top(px(rank_top(r0) - 18.))
                .text_size(px(10.5))
                .font_medium()
                .text_color(look.muted)
                .child(name.clone()),
        )
    });
    let edge_labels = edges.iter().enumerate().filter_map(|(e, edge)| {
        let label = edge.label.clone()?;
        let (from, to) = pairs[e];
        if layout.rank[to] <= layout.rank[from] {
            return None;
        }
        // Halfway down its first stretch, just under the source.
        let next = layout.routes[e].first().map_or(layout.x[to], |(_, x)| *x);
        let mid_x = (layout.x[from] + next) / 2. + 0.5;
        let y = rank_top(layout.rank[from]) + node_h + gap / 2. - 9.;
        Some(
            div().absolute().left(relative(mid_x / slots)).ml(px(-70.)).w(px(140.)).top(px(y)).flex().justify_center().opacity(enter).child(
                div()
                    .px(px(6.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(look.hairline)
                    .bg(look.popover)
                    .text_size(px(10.5))
                    .text_color(if linked(e) { look.fg } else { look.muted })
                    .whitespace_nowrap()
                    .child(label),
            ),
        )
    });
    inset(&look)
        .id(frame.id("flow", 0))
        .test_support()
        .w_full()
        .p_3()
        .when(layout.back.iter().any(|b| *b), |el| el.pr(px(36.)))
        .child(
            div()
                .relative()
                .w_full()
                .h(px(height))
                .child(canvas(|_, _, _| {}, paint).absolute().inset_0())
                .children(group_labels)
                .children(node_els)
                .children(edge_labels),
        )
        .into_any_element()
}

// ---------- layers ----------

pub(super) fn layers(layers: &[Layer], frame: &Frame, cx: &App) -> AnyElement {
    let look = frame.look.clone();
    let narrow = frame.narrow();
    let n = layers.len();
    let rows: Rc<RefCell<Vec<Option<Bounds<Pixels>>>>> = Rc::new(RefCell::new(vec![None; n]));
    let colors: Vec<Hsla> = layers.iter().enumerate().map(|(i, l)| if l.tone.is_some() { tone(l.tone, cx) } else { crate::palette::series(i, cx) }).collect();
    let hover = frame.hover;
    let lift = frame.lift;
    // The stack builds itself bottom layer first, the way a cairn does.
    let settle: Vec<f32> = (0..n).map(|i| frame.arrive(n - 1 - i, n)).collect();
    let paint = {
        let (rows, colors, base, edge, settle) = (rows.clone(), colors.clone(), look.gap, look.fg, settle.clone());
        let items: Vec<usize> = layers.iter().map(|l| l.items.len()).collect();
        let dark = look.dark;
        let leader = look.fg.opacity(0.18);
        move |b: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
            let rows = rows.borrow();
            let pw = f32::from(b.size.width) * 0.8;
            let ph = pw * 0.44;
            let cx = f32::from(b.origin.x) + f32::from(b.size.width) * 0.45;
            let thick = 6.;
            // Back to front: the bottom layer first, so the ones above sit over it.
            for i in (0..rows.len()).rev() {
                let Some(row) = rows[i] else { continue };
                let picked = hover == Some(i);
                let k = settle[i];
                let cy = f32::from(row.center().y) - (1. - k) * 28. - if picked { 9. * lift } else { 0. };
                let alpha = k * match hover {
                    Some(h) if h != i => 1. - 0.6 * lift,
                    _ => 1.,
                };
                if alpha <= 0. {
                    continue;
                }
                let c = colors[i];
                let at = |x: f32, y: f32| point(px(x), px(y));
                let (l, t, r, bot) = (at(cx - pw / 2., cy), at(cx, cy - ph / 2.), at(cx + pw / 2., cy), at(cx, cy + ph / 2.));
                let down = |p: Point<Pixels>| point(p.x, p.y + px(thick));
                // The slab's two visible sides, then its face.
                let side = base.blend(c.opacity(if dark { 0.42 } else { 0.5 }));
                let side2 = base.blend(c.opacity(if dark { 0.3 } else { 0.38 }));
                let face = base.blend(c.opacity(if picked { 0.34 } else if dark { 0.2 } else { 0.16 }));
                let poly = |window: &mut Window, pts: &[Point<Pixels>], color: Hsla| {
                    let mut p = PathBuilder::fill();
                    p.add_polygon(pts, true);
                    if let Ok(p) = p.build() {
                        window.paint_path(p, color.opacity(alpha));
                    }
                };
                poly(window, &[l, bot, down(bot), down(l)], side);
                poly(window, &[bot, r, down(r), down(bot)], side2);
                poly(window, &[l, t, r, bot], face);
                // Its rim: the back edges catch the light, the front ones the tone.
                let mut rim = PathBuilder::stroke(px(1.));
                rim.add_polygon(&[l, t, r], false);
                if let Ok(p) = rim.build() {
                    window.paint_path(p, edge.opacity(if dark { 0.22 } else { 0.18 } * alpha));
                }
                let mut front = PathBuilder::stroke(px(if picked { 1.5 } else { 1. }));
                front.add_polygon(&[l, bot, r], false);
                if let Ok(p) = front.build() {
                    window.paint_path(p, c.opacity((if picked { 1. } else { 0.75 }) * alpha));
                }
                // What it holds, as blocks on its face.
                let count = items[i].min(8);
                if count > 0 {
                    let cols = (count as f32).sqrt().ceil() as usize;
                    let lines = count.div_ceil(cols);
                    let on = |u: f32, v: f32| at(f32::from(l.x) + (f32::from(t.x) - f32::from(l.x)) * u + (f32::from(bot.x) - f32::from(l.x)) * v, f32::from(l.y) + (f32::from(t.y) - f32::from(l.y)) * u + (f32::from(bot.y) - f32::from(l.y)) * v);
                    for j in 0..count {
                        let (col, line) = ((j % cols) as f32, (j / cols) as f32);
                        let (du, dv) = (0.64 / cols as f32, 0.64 / lines as f32);
                        let (u0, v0) = (0.18 + col * du, 0.18 + line * dv);
                        let (u1, v1) = (u0 + du * 0.7, v0 + dv * 0.7);
                        let quad = [on(u0, v0), on(u1, v0), on(u1, v1), on(u0, v1)];
                        let raised: Vec<Point<Pixels>> = quad.iter().map(|p| point(p.x, p.y - px(2.5))).collect();
                        poly(window, &[quad[3], quad[2], quad[1], raised[1], raised[2], raised[3]], base.blend(c.opacity(0.55)));
                        poly(window, &raised, base.blend(c.opacity(if dark { 0.75 } else { 0.62 })));
                    }
                }
                // A leader from its right corner across to its row.
                let lx = f32::from(r.x);
                let right = f32::from(b.origin.x + b.size.width);
                if right > lx + 6. {
                    hline(window, px(lx + 4.), px(cy), px(right - lx - 4.), if picked { c.opacity(0.7) } else { leader.opacity(alpha) });
                    dot(window, point(px(lx + 2.), px(cy)), px(2.), c.opacity(alpha));
                }
            }
        }
    };
    let row_els = layers.iter().enumerate().map(|(ix, layer)| {
        let rows = rows.clone();
        let hovered = hover == Some(ix);
        v_flex()
            .id(frame.id("layer", ix))
            .test_support()
            .relative()
            .gap(px(5.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(8.))
            .border_1()
            .border_color(if hovered { colors[ix].opacity(0.45) } else { gpui_kit::transparent_black() })
            .when(hovered, |el| el.bg(look.fg.opacity(0.035)))
            .opacity(frame.presence(ix).max(0.45) * (0.3 + 0.7 * settle[ix]))
            .on_hover(frame.on_hover(ix))
            .child(canvas(move |b, _, _| rows.borrow_mut()[ix] = Some(b), |_, _, _, _| {}).absolute().inset_0())
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().flex_none().font_family(look.mono.clone()).text_size(px(10.)).text_color(look.muted).child(format!("{:02}", ix + 1)))
                    .child(div().min_w_0().truncate().text_size(px(12.5)).font_semibold().text_color(look.fg).child(layer.label.clone())),
            )
            .when_some(layer.detail.clone(), |el, detail| el.child(div().text_size(px(11.5)).line_height(relative(1.4)).text_color(look.muted).child(detail)))
            .when(!layer.items.is_empty(), |el| {
                el.child(h_flex().flex_wrap().gap(px(4.)).children(layer.items.iter().map(|item| {
                    div().px(px(6.)).h(px(19.)).flex().items_center().rounded(px(5.)).border_1().border_color(look.hairline).bg(look.fg.opacity(0.03)).text_size(px(10.5)).text_color(look.fg.opacity(0.82)).child(item.clone())
                })))
            })
    });
    inset(&look)
        .id(frame.id("layers", 0))
        .test_support()
        .w_full()
        .p_3()
        .child(
            h_flex()
                .w_full()
                .items_stretch()
                .child(div().relative().flex_none().w(relative(if narrow { 0.36 } else { 0.4 })).max_w(px(300.)).child(canvas(|_, _, _| {}, paint).absolute().inset_0()))
                .child(v_flex().flex_1().min_w_0().gap(px(4.)).py(px(if narrow { 18. } else { 26. })).children(row_els)),
        )
        .into_any_element()
}
