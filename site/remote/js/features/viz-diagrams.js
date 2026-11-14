// Diagrams of structure rather than values, as the iPhone lays them out (VizDiagrams.swift):
// timelines, flows and layer stacks. Tapping a mark picks it out; tapping it again lets go.

import { flowLayout, formatTick, niceScale, pack, withUnit } from './viz-spec.js';
import { dark, el, mix, pct, readout, seriesColor, sv, toneColor } from './viz-dom.js';

const presence = (ctx, i) => (ctx.state.selected !== null && ctx.state.selected !== i ? 0.38 : 1);
const round = (v) => Math.round(v * 100) / 100;

// ---------------------------------------------------------------------------------------------
// Timeline

export function timeline(viz, ctx) {
  const { events, ticks, unit } = viz;
  const rowH = 30;
  const at = (p) => (typeof p === 'number' ? p : Math.max(ticks ? ticks.indexOf(p) : 0, 0));
  // Where an event starts and ends on the axis, and whether it's a milestone. A tick names a whole
  // slot: Oct to Nov runs to the end of November.
  const spans = events.map((e) => {
    const start = at(e.start);
    if (e.end === undefined) return ticks ? [start + 0.5, start + 0.5, true] : [start, start, true];
    return [start, at(e.end) + (ticks ? 1 : 0), false];
  });
  const when = (p) => (typeof p === 'number' ? withUnit(formatTick(p, 1), unit) : p);
  let lo;
  let hi;
  let marks;
  let grid;
  if (ticks) {
    lo = 0;
    hi = ticks.length;
    marks = ticks.map((t, i) => ({ at: i + 0.5, label: t }));
    grid = Array.from({ length: ticks.length + 1 }, (_, i) => i / ticks.length);
  } else {
    const scale = niceScale(Math.min(...spans.map((s) => s[0])), Math.max(...spans.map((s) => s[1])), 4);
    lo = scale.lo;
    hi = scale.hi;
    marks = scale.ticks.map((t) => ({ at: t, label: withUnit(formatTick(t, scale.step), unit) }));
    grid = scale.ticks.map((t) => scale.frac(t));
  }
  const frac = (v) => (v - lo) / Math.max(hi - lo, Number.EPSILON);
  const lanes = [];
  for (const e of events) if (e.lane !== undefined && !lanes.includes(e.lane)) lanes.push(e.lane);
  const laned = lanes.length > 0;
  // Rows: one per event, or per lane with its events packed into as few rows as fit.
  let rows = [];
  if (laned) {
    for (const lane of [...lanes, ...(events.some((e) => e.lane === undefined) ? [''] : [])]) {
      const members = events.map((_, i) => i).filter((i) => (events[i].lane ?? '') === lane);
      // Touching spans share a row; a milestone keeps room for its label.
      const packed = pack(
        members.map((i) => [spans[i][0], spans[i][2] ? spans[i][0] + (hi - lo) * 0.18 : spans[i][1]]),
        0,
      );
      const count = Math.max(-1, ...packed) + 1;
      for (let r = 0; r < count; r++) rows.push({ label: r === 0 ? lane || 'Other' : null, members: members.filter((_, k) => packed[k] === r) });
    }
  } else {
    rows = events.map((e, i) => ({ label: e.label, members: [i] }));
  }
  const picked = ctx.state.selected;
  const event = (ix) => {
    const e = events[ix];
    const [s0, s1, milestone] = spans[ix];
    const color = toneColor(e.tone);
    const on = picked === ix;
    const p = presence(ctx, ix);
    if (milestone) {
      return el(
        'span',
        {
          class: 'viz-milestone',
          'data-pick': ix,
          style: { left: `calc(${pct(frac(s0))} - 7px)`, '--c': mix(color, p), '--t': mix('var(--fg)', 0.86 * p), '--d': ix },
          title: `${e.label}, ${when(e.start)}`,
        },
        el('i'),
        laned && el('span', { text: e.label }),
      );
    }
    return el(
      'span',
      {
        class: `viz-span${laned ? ' laned' : ''}`,
        'data-pick': ix,
        style: {
          left: `calc(${pct(frac(s0))} + 1px)`,
          width: `max(4px, calc(${pct(frac(s1) - frac(s0))} - 2px))`,
          background: mix(color, laned ? (on ? 0.34 : 0.2 * p) : on ? 1 : 0.85 * p),
          boxShadow: laned ? `inset 0 0 0 1px ${mix(color, 0.55 * p)}` : null,
          '--t': Math.max(p, 0.4),
          '--d': ix,
        },
        title: `${e.label}, ${when(e.start)} to ${when(e.end)}`,
      },
      laned && el('span', { text: e.label }),
    );
  };
  return el(
    'div',
    { class: 'viz-inset viz-timeline' },
    el(
      'div',
      { class: 'viz-tl-axis', 'aria-hidden': 'true' },
      marks.map((m) => el('span', { class: ticks ? 'named' : 'viz-mono', style: { left: pct(frac(m.at)) }, text: m.label })),
    ),
    el(
      'div',
      { class: 'viz-tl-rows' },
      el(
        'div',
        { class: 'viz-tl-grid', 'aria-hidden': 'true' },
        grid.map((x) => el('span', { style: { left: pct(x) } })),
      ),
      rows.map((row, r) => {
        const solo = !laned && row.members.length === 1;
        const p = picked !== null && row.members.includes(picked) ? picked : null;
        return el(
          'div',
          { class: `viz-tl-row${laned && row.label !== null && r > 0 ? ' lane' : ''}` },
          el('span', {
            class: `viz-tl-label${laned ? ' laned' : ''}`,
            style: solo ? { opacity: presence(ctx, row.members[0]) } : null,
            'data-pick': solo ? row.members[0] : null,
            text: row.label ?? '',
          }),
          el(
            'span',
            { class: 'viz-tl-track' },
            row.members.map(event),
            p !== null &&
              (() => {
                const e = events[p];
                const right = frac(spans[p][1]) < 0.55;
                return readout(
                  right ? { left: `calc(${pct(frac(spans[p][1]))} + 8px)`, top: '-11px' } : { right: `calc(${pct(1 - frac(spans[p][0]))} + 8px)`, top: '-11px' },
                  el('div', { class: 'viz-readout-title', text: e.label }),
                  el('div', { class: 'muted', text: e.end !== undefined ? `${when(e.start)} – ${when(e.end)}` : when(e.start) }),
                );
              })(),
          ),
        );
      }),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Flow

export function flow(viz, ctx) {
  const { nodes, edges } = viz;
  const index = (id) => Math.max(
    nodes.findIndex((n) => n.id === id),
    0,
  );
  const pairs = edges.map((e) => [index(e.from), index(e.to)]);
  const names = [];
  for (const n of nodes) if (n.group !== undefined && !names.includes(n.group)) names.push(n.group);
  const groups = nodes.map((n) => (n.group !== undefined ? names.indexOf(n.group) : null));
  const layout = flowLayout(nodes.length, pairs, groups);
  const grouped = names.length > 0;
  const backs = layout.back.includes(true);
  // The inset's padding and edges, and room on the right for edges drawn back up.
  const inner = Math.max(ctx.width - 26 - (backs ? 26 : 0), 1);
  const slotW = inner / layout.slots;
  const nodeW = Math.max(Math.min(slotW - 12, 220), 40);
  // Details when a node has room for them.
  const details = nodeW >= 118 && nodes.some((n) => n.detail !== undefined);
  const nodeH = details ? 46 : 34;
  const gap = grouped ? 52 : 38;
  const top = grouped ? 26 : 6;
  const rankTop = (r) => top + r * (nodeH + gap);
  const height = rankTop(layout.ranks - 1) + nodeH + (grouped ? 14 : 8) + (backs ? 4 : 0);
  const cx = (v) => slotW * (layout.x[v] + 0.5);
  const members = (g) => nodes.map((_, v) => v).filter((v) => groups[v] === g);
  // A group gets a container when one fits around its nodes and nobody else's; groups that
  // interleave with others are named on their nodes instead.
  const boxed = names.map((_, g) => {
    const m = members(g);
    const lo = Math.min(...m.map((v) => layout.x[v]));
    const hi = Math.max(...m.map((v) => layout.x[v]));
    const r0 = Math.min(...m.map((v) => layout.rank[v]));
    const r1 = Math.max(...m.map((v) => layout.rank[v]));
    return !nodes.some((_, v) => groups[v] !== g && layout.rank[v] >= r0 && layout.rank[v] <= r1 && layout.x[v] > lo - 0.99 && layout.x[v] < hi + 0.99);
  });
  const sel = ctx.state.selected;
  const picked = sel !== null && sel < nodes.length ? sel : null;
  const colors = nodes.map((n) => (n.tone ? toneColor(n.tone) : 'var(--muted)'));
  const linked = (e) => picked !== null && (pairs[e][0] === picked || pairs[e][1] === picked);
  const near = (v) => picked === null || picked === v || pairs.some(([a, b]) => (a === picked && b === v) || (b === picked && a === v));
  const night = dark();
  const edgeColor = mix('var(--fg)', night ? 0.26 : 0.24);
  const lit = picked !== null ? (nodes[picked].tone ? colors[picked] : mix('var(--fg)', 0.75)) : edgeColor;
  const boxes = names.map((name, g) => {
    if (!boxed[g]) return null;
    const m = members(g);
    const lo = Math.min(...m.map((v) => layout.x[v]));
    const hi = Math.max(...m.map((v) => layout.x[v]));
    const r0 = Math.min(...m.map((v) => layout.rank[v]));
    const r1 = Math.max(...m.map((v) => layout.rank[v]));
    return sv('rect', {
      class: 'viz-flow-group',
      x: round(slotW * lo + 2),
      y: round(rankTop(r0) - 22),
      width: round(slotW * (hi + 1 - lo) - 4),
      height: round(rankTop(r1) - rankTop(r0) + nodeH + 30),
      rx: 10,
    });
  });
  const lines = pairs.map(([from, to], e) => {
    const color = linked(e) ? lit : picked !== null ? mix('var(--fg)', (night ? 0.26 : 0.24) * 0.45) : edgeColor;
    let d;
    let tip;
    let dir;
    if (layout.back[e] || layout.rank[to] <= layout.rank[from]) {
      // Back up the ranks: out of the source's right side and round into the target's.
      const s = [cx(from) + nodeW / 2, rankTop(layout.rank[from]) + nodeH / 2];
      const t = [cx(to) + nodeW / 2 + 2, rankTop(layout.rank[to]) + nodeH / 2];
      d = `M${round(s[0])},${round(s[1])}C${round(s[0] + 26)},${round(s[1])} ${round(t[0] + 26)},${round(t[1])} ${round(t[0])},${round(t[1])}`;
      [tip, dir] = [t, [-1, 0]];
    } else {
      // Down through each rank it crosses, in its own slot there.
      const way = [[cx(from), rankTop(layout.rank[from]) + nodeH]];
      for (const [r, x] of layout.routes[e]) {
        way.push([slotW * (x + 0.5), rankTop(r)], [slotW * (x + 0.5), rankTop(r) + nodeH]);
      }
      const t = [cx(to), rankTop(layout.rank[to]) - 2];
      way.push(t);
      d = `M${round(way[0][0])},${round(way[0][1])}`;
      for (let i = 0; i < way.length - 1; i++) {
        const [s, u] = [way[i], way[i + 1]];
        if (i % 2 === 1) d += `L${round(u[0])},${round(u[1])}`;
        else {
          const dy = (u[1] - s[1]) / 2;
          d += `C${round(s[0])},${round(s[1] + dy)} ${round(u[0])},${round(u[1] - dy)} ${round(u[0])},${round(u[1])}`;
        }
      }
      [tip, dir] = [t, [0, 1]];
    }
    // The arrowhead, pointing along the curve's end.
    const base = [tip[0] - dir[0] * 6.5, tip[1] - dir[1] * 6.5];
    const head = `M${round(tip[0])},${round(tip[1])}L${round(base[0] - dir[1] * 3.6)},${round(base[1] + dir[0] * 3.6)}L${round(base[0] + dir[1] * 3.6)},${round(base[1] - dir[0] * 3.6)}Z`;
    return [
      sv('path', { class: 'viz-flow-edge', d, style: { stroke: color, strokeWidth: linked(e) ? 1.75 : 1.25 } }),
      sv('path', { d: head, style: { fill: color } }),
    ];
  });
  const groupNames = names.map((name, g) => {
    if (!boxed[g]) return null;
    const m = members(g);
    return el('span', {
      class: 'viz-flow-group-name',
      style: { left: `${slotW * Math.min(...m.map((v) => layout.x[v])) + 12}px`, top: `${rankTop(Math.min(...m.map((v) => layout.rank[v]))) - 19}px` },
      text: name,
    });
  });
  const boxesHtml = nodes.map((node, v) => {
    const tag = groups[v] !== null && !boxed[groups[v]] && nodeW >= 170 ? names[groups[v]] : null;
    return el(
      'div',
      {
        class: `viz-flow-node${node.tone ? ' toned' : ''}${picked === v ? ' on' : ''}`,
        'data-pick': v,
        style: {
          left: `${round(cx(v) - nodeW / 2)}px`,
          top: `${round(rankTop(layout.rank[v]))}px`,
          width: `${round(nodeW)}px`,
          height: `${nodeH}px`,
          opacity: near(v) ? 1 : 0.4,
          '--c': colors[v],
          '--d': layout.rank[v],
        },
      },
      el(
        'div',
        { class: 'viz-grow' },
        el('div', { class: 'viz-flow-label', text: node.label }),
        details && el('div', { class: 'viz-flow-detail', text: node.detail ?? '' }),
      ),
      tag && el('span', { class: 'viz-flow-tag', text: tag }),
    );
  });
  const labels = edges.map((edge, e) => {
    const [from, to] = pairs[e];
    if (!edge.label || layout.rank[to] <= layout.rank[from]) return null;
    // Halfway down its first stretch, just under the source.
    const next = layout.routes[e][0]?.[1] ?? layout.x[to];
    return el('span', {
      class: `viz-flow-edge-label${linked(e) ? ' on' : ''}`,
      style: { left: `${round(slotW * ((layout.x[from] + next) / 2 + 0.5))}px`, top: `${round(rankTop(layout.rank[from]) + nodeH + gap / 2 - 9.5)}px` },
      text: edge.label,
    });
  });
  return el(
    'div',
    { class: 'viz-inset viz-flow' },
    el(
      'div',
      { class: 'viz-flow-canvas', style: { width: `${inner}px`, height: `${height}px`, marginRight: backs ? '26px' : null } },
      sv('svg', { class: 'viz-flow-svg', width: inner, height, viewBox: `0 0 ${round(inner)} ${round(height)}`, 'aria-hidden': 'true' }, boxes, lines),
      groupNames,
      boxesHtml,
      labels,
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Layers

/** A layer's slab beside its row: a plane seen from above at an angle, its items as blocks on
 * its face, a leader across to its row. */
function slab(color, items, on, faded) {
  const C = 116;
  const pw = C * 0.8;
  const ph = pw * 0.44;
  const cx = C * 0.45;
  const thick = 6;
  const cy = 30;
  const alpha = faded ? 0.4 : 1;
  const night = dark();
  const [l, t, r, b] = [
    [cx - pw / 2, cy],
    [cx, cy - ph / 2],
    [cx + pw / 2, cy],
    [cx, cy + ph / 2],
  ];
  const down = (p) => [p[0], p[1] + thick];
  const pts = (list) => list.map((p) => `${round(p[0])},${round(p[1])}`).join(' ');
  // Over the opaque surface, so a slab hides what's behind it.
  const poly = (list, a) => [
    sv('polygon', { points: pts(list), class: 'viz-slab-base', style: { opacity: alpha } }),
    sv('polygon', { points: pts(list), style: { fill: color, fillOpacity: a * alpha } }),
  ];
  const parts = [
    poly([l, b, down(b), down(l)], night ? 0.42 : 0.5),
    poly([b, r, down(r), down(b)], night ? 0.3 : 0.38),
    poly([l, t, r, b], on ? 0.34 : night ? 0.2 : 0.16),
    sv('polyline', { points: pts([l, t, r]), class: 'viz-slab-rim', style: { opacity: alpha } }),
    sv('polyline', { points: pts([l, b, r]), style: { fill: 'none', stroke: color, strokeOpacity: (on ? 1 : 0.75) * alpha, strokeWidth: on ? 1.5 : 1 } }),
  ];
  const count = Math.min(items, 8);
  if (count > 0) {
    const cols = Math.ceil(Math.sqrt(count));
    const lines = Math.ceil(count / cols);
    const onFace = (u, v) => [l[0] + (t[0] - l[0]) * u + (b[0] - l[0]) * v, l[1] + (t[1] - l[1]) * u + (b[1] - l[1]) * v];
    for (let j = 0; j < count; j++) {
      const du = 0.64 / cols;
      const dv = 0.64 / lines;
      const u0 = 0.18 + (j % cols) * du;
      const v0 = 0.18 + Math.floor(j / cols) * dv;
      const u1 = u0 + du * 0.7;
      const v1 = v0 + dv * 0.7;
      const quad = [onFace(u0, v0), onFace(u1, v0), onFace(u1, v1), onFace(u0, v1)];
      const raised = quad.map((p) => [p[0], p[1] - 2.5]);
      parts.push(poly([quad[3], quad[2], quad[1], raised[1], raised[2], raised[3]], 0.55), poly(raised, night ? 0.75 : 0.62));
    }
  }
  parts.push(
    sv('rect', { x: round(r[0] + 4), y: cy - 0.5, width: round(C - r[0] - 4), height: 1, style: { fill: on ? mix(color, 0.7) : mix('var(--fg)', 0.18 * alpha) } }),
    sv('circle', { cx: round(r[0] + 2), cy, r: 2, style: { fill: color, opacity: alpha } }),
  );
  return sv('svg', { class: 'viz-slab', width: C, height: 60, viewBox: `0 0 ${C} 60`, 'aria-hidden': 'true' }, parts);
}

export function layers(viz, ctx) {
  const n = viz.layers.length;
  const colors = viz.layers.map((l, i) => (l.tone ? toneColor(l.tone) : seriesColor(i)));
  const picked = ctx.state.selected;
  return el(
    'div',
    { class: 'viz-inset viz-layers' },
    viz.layers.map((layer, i) =>
      el(
        'div',
        {
          class: `viz-layer${picked === i ? ' on' : ''}`,
          'data-pick': i,
          style: { '--c': colors[i], '--k': n - 1 - i, zIndex: n - i, opacity: Math.max(presence(ctx, i), 0.45) },
        },
        slab(colors[i], layer.items.length, picked === i, picked !== null && picked !== i),
        el('div', { class: 'viz-layer-head' }, el('span', { class: 'viz-mono muted', text: String(i + 1).padStart(2, '0') }), el('b', { text: layer.label })),
        layer.detail && el('div', { class: 'viz-layer-detail', text: layer.detail }),
        layer.items.length > 0 && el('div', { class: 'viz-layer-items' }, layer.items.map((item) => el('span', { text: item }))),
      ),
    ),
  );
}
