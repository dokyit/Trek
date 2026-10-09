//! Geometry for the visualizations, kept apart from drawing so it can be tested: "nice" axis
//! scales, squarified treemaps, layered flow diagrams and timeline lanes.

/// A linear axis rounded out to "nice" 1-2-5 steps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub lo: f64,
    pub hi: f64,
    pub step: f64,
}

impl Scale {
    /// `min..max` widened to whole steps of about `target` ticks. A flat range still gets an
    /// axis: around zero when it is zero, else from zero to it.
    pub fn nice(min: f64, max: f64, target: usize) -> Scale {
        let (mut min, mut max) = (min.min(max), max.max(min));
        if (max - min).abs() < f64::EPSILON {
            if max == 0.0 {
                max = 1.0;
            } else if max > 0.0 {
                min = 0.0;
            } else {
                max = 0.0;
            }
        }
        let step = nice_round((max - min) / target.max(1) as f64);
        let lo = (min / step).floor() * step;
        let hi = (max / step).ceil() * step;
        Scale { lo, hi, step }
    }

    /// The tick values from `lo` to `hi`, inclusive.
    pub fn ticks(&self) -> Vec<f64> {
        let n = ((self.hi - self.lo) / self.step).round() as usize;
        (0..=n.min(64)).map(|i| self.lo + i as f64 * self.step).map(|v| if v.abs() < self.step * 1e-9 { 0.0 } else { v }).collect()
    }

    /// Where `value` falls on the axis, 0 at `lo` to 1 at `hi`.
    pub fn frac(&self, value: f64) -> f32 {
        let span = (self.hi - self.lo).max(f64::EPSILON);
        ((value - self.lo) / span) as f32
    }
}

/// The 1, 2 or 5 × 10ⁿ nearest `raw` (Heckbert's "nice numbers"): a scale's step, so the
/// tick count lands near the one asked for rather than always under it.
fn nice_round(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let nice = if normalized < 1.5 {
        1.0
    } else if normalized < 3.0 {
        2.0
    } else if normalized < 7.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

/// A rectangle in whatever units the caller lays out in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Squarified treemap (Bruls, Huizing & van Wijk): tiles of area proportional to `weights`
/// filling `bounds`, kept as close to square as the order allows. Returned in input order.
pub fn squarify(weights: &[f64], bounds: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::default(); weights.len()];
    let total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    if total <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }
    let scale = (bounds.w * bounds.h) as f64 / total;
    let mut order: Vec<usize> = (0..weights.len()).filter(|i| weights[*i] > 0.0).collect();
    order.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]));
    let area = |i: usize| (weights[i] * scale) as f32;
    // The worst aspect ratio in a row of `sum` total area along a side of `side`.
    let worst = |row: &[usize], side: f32| -> f32 {
        let sum: f32 = row.iter().map(|i| area(*i)).sum();
        let (lo, hi) = row.iter().map(|i| area(*i)).fold((f32::MAX, 0f32), |(lo, hi), a| (lo.min(a), hi.max(a)));
        let (s2, side2) = (sum * sum, side * side);
        (side2 * hi / s2).max(s2 / (side2 * lo.max(f32::EPSILON)))
    };
    let mut rect = bounds;
    let mut row: Vec<usize> = Vec::new();
    let place = |row: &[usize], rect: &mut Rect, out: &mut Vec<Rect>| {
        let sum: f32 = row.iter().map(|i| area(*i)).sum();
        if rect.w >= rect.h {
            // A column down the left edge.
            let thick = if rect.h > 0.0 { sum / rect.h } else { 0.0 };
            let mut y = rect.y;
            for i in row {
                let h = if thick > 0.0 { area(*i) / thick } else { 0.0 };
                out[*i] = Rect { x: rect.x, y, w: thick, h };
                y += h;
            }
            rect.x += thick;
            rect.w = (rect.w - thick).max(0.0);
        } else {
            // A row along the top edge.
            let thick = if rect.w > 0.0 { sum / rect.w } else { 0.0 };
            let mut x = rect.x;
            for i in row {
                let w = if thick > 0.0 { area(*i) / thick } else { 0.0 };
                out[*i] = Rect { x, y: rect.y, w, h: thick };
                x += w;
            }
            rect.y += thick;
            rect.h = (rect.h - thick).max(0.0);
        }
    };
    for i in order {
        let side = rect.w.min(rect.h);
        let mut grown = row.clone();
        grown.push(i);
        if row.is_empty() || worst(&grown, side) <= worst(&row, side) {
            row = grown;
        } else {
            place(&row, &mut rect, &mut out);
            row = vec![i];
        }
    }
    if !row.is_empty() {
        place(&row, &mut rect, &mut out);
    }
    out
}

/// A layered (Sugiyama-style) layout of a directed graph, ranks running top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowLayout {
    /// Each node's rank (row), 0 at the top.
    pub rank: Vec<usize>,
    /// Each node's centre across, in slots (0.0 is the middle of the first slot).
    pub x: Vec<f32>,
    /// Slots across: the widest rank.
    pub slots: usize,
    pub ranks: usize,
    /// Edges that point back up a rank (a cycle) — drawn around the side.
    pub back: Vec<bool>,
    /// Where each edge crosses the ranks between its ends: (rank, x in slots), top down.
    pub routes: Vec<Vec<(usize, f32)>>,
}

/// Rank by longest path from the sources (cycles broken where a depth-first walk meets them),
/// order each rank by group then by the barycentre of its neighbours to cut crossings, and
/// place nodes over their parents where the order allows.
pub fn flow_layout(nodes: usize, edges: &[(usize, usize)], groups: &[Option<usize>]) -> FlowLayout {
    let n = nodes;
    // Back edges: the ones a depth-first walk finds pointing at a node still on its stack.
    let mut back = vec![false; edges.len()];
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    let out: Vec<Vec<(usize, usize)>> = (0..n).map(|v| edges.iter().enumerate().filter(|(_, e)| e.0 == v).map(|(i, e)| (i, e.1)).collect()).collect();
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        state[root] = 1;
        while let Some(&mut (v, ref mut next)) = stack.last_mut() {
            if let Some(&(edge, w)) = out[v].get(*next) {
                *next += 1;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => back[edge] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    let forward: Vec<(usize, usize)> = edges.iter().zip(&back).filter(|(_, b)| !**b).map(|(e, _)| *e).collect();
    // Longest path: relax in topological order (Kahn's).
    let mut indegree = vec![0usize; n];
    for (_, to) in &forward {
        indegree[*to] += 1;
    }
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|v| indegree[*v] == 0).collect();
    let mut rank = vec![0usize; n];
    while let Some(v) = queue.pop_front() {
        for (from, to) in forward.iter().filter(|(from, _)| *from == v) {
            rank[*to] = rank[*to].max(rank[*from] + 1);
            indegree[*to] -= 1;
            if indegree[*to] == 0 {
                queue.push_back(*to);
            }
        }
    }
    let ranks = rank.iter().max().map_or(0, |r| r + 1);
    // An edge across several ranks passes through a virtual node in each one between, so it
    // takes a slot of its own there instead of running through the nodes in its way.
    let real = n;
    let mut groups: Vec<Option<usize>> = (0..n).map(|v| groups.get(v).copied().flatten()).collect();
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); edges.len()];
    let mut through: Vec<(usize, usize)> = Vec::new();
    for (e, (from, to)) in edges.iter().enumerate() {
        if back[e] {
            continue;
        }
        let mut prev = *from;
        for r in rank[*from] + 1..rank[*to] {
            let v = rank.len();
            rank.push(r);
            groups.push(groups[*from]);
            chains[e].push(v);
            through.push((prev, v));
            prev = v;
        }
        through.push((prev, *to));
    }
    let forward = through;
    let n = rank.len();
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    for v in 0..n {
        rows[rank[v]].push(v);
    }
    let group_key = |v: usize| groups.get(v).copied().flatten().unwrap_or(usize::MAX);
    let position = |rows: &[Vec<usize>]| {
        let mut pos = vec![0f32; n];
        for row in rows {
            for (i, v) in row.iter().enumerate() {
                pos[*v] = i as f32 - (row.len() as f32 - 1.0) / 2.0;
            }
        }
        pos
    };
    for row in rows.iter_mut() {
        row.sort_by_key(|v| group_key(*v));
    }
    // A few sweeps down and up, sorting by group, then the mean position of the neighbours in
    // the rank just placed.
    for sweep in 0..4 {
        let pos = position(&rows);
        let down = sweep % 2 == 0;
        let order: Vec<usize> = if down { (1..ranks).collect() } else { (0..ranks.saturating_sub(1)).rev().collect() };
        let mut pos = pos;
        for r in order {
            let mut keyed: Vec<(usize, f32, usize)> = rows[r]
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let near: Vec<f32> = forward
                        .iter()
                        .filter_map(|(a, b)| if down && *b == *v { Some(pos[*a]) } else if !down && *a == *v { Some(pos[*b]) } else { None })
                        .collect();
                    let bary = if near.is_empty() { pos[*v] } else { near.iter().sum::<f32>() / near.len() as f32 };
                    (group_key(*v), bary, i)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
            rows[r] = keyed.iter().map(|k| rows[r][k.2]).collect();
            for (i, v) in rows[r].iter().enumerate() {
                pos[*v] = i as f32 - (rows[r].len() as f32 - 1.0) / 2.0;
            }
        }
    }
    // Coordinates in slots: each rank over the mean of its parents, a slot apart, inside the
    // widest rank.
    let slots = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let max_x = (slots - 1) as f32;
    let mut x = vec![0f32; n];
    let settle = |row: &[usize], want: &[f32]| -> Vec<f32> {
        let mut at: Vec<f32> = want.to_vec();
        for i in 0..at.len() {
            at[i] = at[i].clamp(0.0, max_x);
            if i > 0 {
                at[i] = at[i].max(at[i - 1] + 1.0);
            }
        }
        for i in (0..at.len()).rev() {
            let cap = if i + 1 < at.len() { at[i + 1] - 1.0 } else { max_x };
            at[i] = at[i].min(cap);
        }
        debug_assert_eq!(at.len(), row.len());
        at
    };
    for (r, row) in rows.iter().enumerate() {
        let centred = |i: usize| i as f32 + (slots - row.len()) as f32 / 2.0;
        let want: Vec<f32> = row
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let parents: Vec<f32> = forward.iter().filter(|(a, b)| *b == *v && rank[*a] < r).map(|(a, _)| x[*a]).collect();
                if parents.is_empty() { centred(i) } else { parents.iter().sum::<f32>() / parents.len() as f32 }
            })
            .collect();
        for (v, at) in row.iter().zip(settle(row, &want)) {
            x[*v] = at;
        }
    }
    // Then once back up: a parent moves toward the middle of its children where it can.
    for row in rows.iter().rev() {
        let want: Vec<f32> = row
            .iter()
            .map(|v| {
                let kids: Vec<f32> = forward.iter().filter(|(a, _)| *a == *v).map(|(_, b)| x[*b]).collect();
                if kids.is_empty() { x[*v] } else { (x[*v] + kids.iter().sum::<f32>() / kids.len() as f32) / 2.0 }
            })
            .collect();
        for (v, at) in row.iter().zip(settle(row, &want)) {
            x[*v] = at;
        }
    }
    let routes = chains.iter().map(|chain| chain.iter().map(|v| (rank[*v], x[*v])).collect()).collect();
    rank.truncate(real);
    x.truncate(real);
    FlowLayout { rank, x, slots, ranks: ranks.max(1), back, routes }
}

/// Rows for spans `(start, end)` so that none overlap within a row: each takes the first row
/// free by its start. `gap` keeps touching spans apart.
pub fn pack(spans: &[(f64, f64)], gap: f64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|a, b| spans[*a].0.total_cmp(&spans[*b].0).then(a.cmp(b)));
    let mut ends: Vec<f64> = Vec::new();
    let mut row = vec![0; spans.len()];
    for i in order {
        let (start, end) = spans[i];
        match ends.iter().position(|e| *e + gap <= start) {
            Some(r) => {
                ends[r] = end;
                row[i] = r;
            }
            None => {
                row[i] = ends.len();
                ends.push(end);
            }
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_scales_land_on_one_two_five() {
        assert_eq!(nice_round(0.8), 1.0);
        assert_eq!(nice_round(1.3), 1.0);
        assert_eq!(nice_round(2.4), 2.0);
        assert_eq!(nice_round(3.0), 5.0);
        assert_eq!(nice_round(7.0), 10.0);
        assert_eq!(nice_round(230.0), 200.0);
        let s = Scale::nice(0.0, 940.0, 4);
        assert_eq!((s.lo, s.hi, s.step), (0.0, 1000.0, 200.0));
        assert_eq!(s.ticks(), vec![0.0, 200.0, 400.0, 600.0, 800.0, 1000.0]);
        let s = Scale::nice(-18.0, 42.0, 4);
        assert_eq!((s.lo, s.hi, s.step), (-20.0, 60.0, 20.0));
        assert_eq!(s.frac(0.0), 0.25);
        // Flat data still has an axis.
        assert_eq!(Scale::nice(0.0, 0.0, 4).hi, 1.0);
        assert_eq!(Scale::nice(5.0, 5.0, 4).lo, 0.0);
        assert_eq!(Scale::nice(-5.0, -5.0, 4).hi, 0.0);
        // Ticks don't drift into -0 or 0.30000000000000004.
        assert!(Scale::nice(0.0, 0.9, 3).ticks().iter().all(|t| (t * 10.0).fract().abs() < 1e-9 || (t * 10.0).fract().abs() > 1.0 - 1e-9));
    }

    #[test]
    fn treemaps_fill_their_box_with_proportional_near_square_tiles() {
        let weights = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let bounds = Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 };
        let tiles = squarify(&weights, bounds);
        let total: f64 = weights.iter().sum();
        for (tile, weight) in tiles.iter().zip(weights) {
            let area = tile.w * tile.h;
            assert!((area as f64 - weight / total * 24.0).abs() < 1e-3, "{tile:?} for {weight}");
            assert!(tile.x >= -1e-4 && tile.y >= -1e-4 && tile.x + tile.w <= 6.0 + 1e-3 && tile.y + tile.h <= 4.0 + 1e-3);
            // Squarified: nothing degenerates into a sliver.
            assert!(tile.w.max(tile.h) / tile.w.min(tile.h) < 4.0, "{tile:?}");
        }
        // No two tiles overlap.
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                let overlap = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                let overlap_y = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                assert!(overlap <= 1e-3 || overlap_y <= 1e-3, "{a:?} {b:?}");
            }
        }
        assert_eq!(squarify(&[1.0], bounds), vec![bounds]);
    }

    #[test]
    fn flows_rank_by_longest_path_and_keep_chains_straight() {
        // a → b → d, a → c → d, d → e; plus a long edge a → e.
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3), (3, 4), (0, 4)];
        let layout = flow_layout(5, &edges, &[None; 5]);
        assert_eq!(layout.rank, vec![0, 1, 1, 2, 3]);
        assert_eq!(layout.ranks, 4);
        assert!(layout.back.iter().all(|b| !b));
        assert!(layout.x[1] != layout.x[2]);
        // The long edge a → e crosses ranks 1 and 2 in slots of its own, clear of b, c and d.
        assert_eq!(layout.routes[5].iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(layout.slots, 3);
        for (r, x) in &layout.routes[5] {
            assert!((0..5).all(|v| layout.rank[v] != *r || (layout.x[v] - x).abs() >= 1.0), "{layout:?}");
        }
        assert!(layout.routes[..5].iter().all(Vec::is_empty));
        // A cycle is broken, not looped forever, and its closing edge is marked.
        let cyclic = flow_layout(3, &[(0, 1), (1, 2), (2, 0)], &[None; 3]);
        assert_eq!(cyclic.rank, vec![0, 1, 2]);
        assert_eq!(cyclic.back, vec![false, false, true]);
    }

    #[test]
    fn flows_order_ranks_to_avoid_crossings_and_keep_groups_together() {
        // Parents a, b; children listed crossed: y under b, x under a.
        let edges = [(0, 3), (1, 2)];
        let layout = flow_layout(4, &edges, &[None; 4]);
        assert!(layout.x[0] < layout.x[1]);
        assert!(layout.x[3] < layout.x[2], "children follow their parents: {:?}", layout.x);
        // Groups stay contiguous within a rank.
        let groups = [Some(0), Some(1), Some(0), Some(1)];
        let layout = flow_layout(4, &[], &groups);
        let mut by_x: Vec<usize> = (0..4).collect();
        by_x.sort_by(|a, b| layout.x[*a].total_cmp(&layout.x[*b]));
        assert_eq!(by_x.iter().map(|v| groups[*v]).collect::<Vec<_>>(), vec![Some(0), Some(0), Some(1), Some(1)]);
        // Every node stays inside the slots, a slot apart from its rank's others.
        for v in 0..4 {
            assert!(layout.x[v] >= 0.0 && layout.x[v] <= (layout.slots - 1) as f32);
        }
    }

    #[test]
    fn lanes_pack_spans_without_overlap() {
        let rows = pack(&[(0.0, 2.0), (1.0, 3.0), (2.5, 4.0), (3.5, 5.0), (3.0, 5.0)], 0.25);
        assert_eq!(rows, vec![0, 1, 0, 1, 2]);
        assert_eq!(pack(&[(0.0, 1.0), (1.0, 2.0)], 0.0), vec![0, 0]);
    }
}
