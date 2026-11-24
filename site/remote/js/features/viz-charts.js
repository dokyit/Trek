// Charts of values, as the iPhone lays them out (VizCharts.swift): bars, lines, a donut, stat
// tiles, tables, heatmaps and treemaps. Tapping a mark picks it out and reads its value; tapping
// it again lets go. Each takes the visualization and `ctx`: what the reader has picked
// (`state.selected`), the series switched off (`state.hidden`) and the width to lay out in.

import { formatExact, formatNumber, formatTick, niceScale, share, squarify, withUnit } from './viz-spec.js';
import { caption, dark, el, icon, mix, pct, readout, seriesColor, sv, toneColor } from './viz-dom.js';

const presence = (ctx, i) => (ctx.state.selected !== null && ctx.state.selected !== i ? 0.38 : 1);

/** A monotone curve through `points` (Fritsch–Carlson): it never overshoots a value. */
export function monotone(points) {
  const n = points.length;
  if (!n) return '';
  const at = ([x, y]) => `${Math.round(x * 100) / 100},${Math.round(y * 100) / 100}`;
  if (n < 3) return `M${points.map(at).join('L')}`;
  const d = [];
  for (let i = 0; i < n - 1; i++) d.push((points[i + 1][1] - points[i][1]) / (points[i + 1][0] - points[i][0] || 1));
  const m = points.map((_, i) => (i === 0 ? d[0] : i === n - 1 ? d[n - 2] : d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2));
  for (let i = 0; i < n - 1; i++) {
    if (d[i] === 0) {
      m[i] = 0;
      m[i + 1] = 0;
      continue;
    }
    const a = m[i] / d[i];
    const b = m[i + 1] / d[i];
    const s = a * a + b * b;
    if (s > 9) {
      const t = 3 / Math.sqrt(s);
      m[i] = t * a * d[i];
      m[i + 1] = t * b * d[i];
    }
  }
  let path = `M${at(points[0])}`;
  for (let i = 0; i < n - 1; i++) {
    const [x0, y0] = points[i];
    const [x1, y1] = points[i + 1];
    const dx = (x1 - x0) / 3;
    path += `C${at([x0 + dx, y0 + m[i] * dx])} ${at([x1 - dx, y1 - m[i + 1] * dx])} ${at([x1, y1])}`;
  }
  return path;
}

// ---------------------------------------------------------------------------------------------
// Bar

export function bars(viz, ctx) {
  const values = viz.bars.map((b) => b.value);
  const lo = Math.min(0, ...values);
  const hi = Math.max(0, ...values);
  const scale = niceScale(lo, hi, 3);
  // Room past the scale for the value written at a bar's end.
  const room = (scale.hi - scale.lo) * 0.22;
  const d0 = lo < 0 ? scale.lo - room : scale.lo;
  const d1 = hi > 0 ? scale.hi + room : scale.hi;
  const frac = (v) => (v - d0) / (d1 - d0 || 1);
  const zero = frac(0);
  const picked = ctx.state.selected;
  return el(
    'div',
    { class: 'viz-chart' },
    viz.y_label && caption(viz.y_label),
    el(
      'div',
      { class: 'viz-inset viz-bars' },
      el(
        'div',
        { class: 'viz-bar-rows' },
        el(
          'div',
          { class: 'viz-bar-grid', 'aria-hidden': 'true' },
          scale.ticks.map((t) => el('span', { class: t === 0 ? 'zero' : null, style: { left: pct(frac(t)) } })),
        ),
        viz.bars.map((b, i) => {
          const f = frac(b.value);
          const neg = b.value < 0;
          return el(
            'div',
            { class: `viz-bar-row${picked === i ? ' on' : ''}`, 'data-pick': i, style: { '--presence': presence(ctx, i) } },
            el('span', { class: 'viz-bar-label', text: b.label }),
            el(
              'span',
              { class: 'viz-bar-track' },
              el('span', { class: `viz-bar-fill${neg ? ' neg' : ''}`, style: { left: pct(Math.min(zero, f)), width: pct(Math.abs(f - zero)), '--c': toneColor(b.tone) } }),
              el('span', { class: 'viz-bar-value', style: neg ? { right: `calc(${pct(1 - f)} + 6px)` } : { left: `calc(${pct(f)} + 6px)` }, text: withUnit(formatNumber(b.value), viz.unit) }),
            ),
          );
        }),
      ),
      el(
        'div',
        { class: 'viz-bar-axis', 'aria-hidden': 'true' },
        scale.ticks.map((t) => el('span', { style: { left: pct(frac(t)) }, text: formatTick(t, scale.step) })),
      ),
    ),
    viz.x_label && caption(viz.x_label, 'center'),
  );
}

// ---------------------------------------------------------------------------------------------
// Line

/** As many x labels as fit, evenly spread, always the first and the last. */
function shownX(n, width) {
  const room = Math.max(Math.floor((width - 38 - 40) / 56), 2);
  const every = Math.max(Math.ceil(n / Math.min(room, n)), 1);
  const out = [];
  for (let i = 0; i < n; i++) {
    if (i % every !== 0 && i !== n - 1) continue;
    if (i === n - 1 || n - 1 - i >= Math.floor(every / 2) + 1 || every === 1) out.push(i);
  }
  return out;
}

/** Chips that switch a series on or off. */
function legend(series, colors, hidden) {
  return el(
    'div',
    { class: 'viz-legend' },
    series.map((s, i) => {
      const off = hidden.has(i);
      return el(
        'button',
        { type: 'button', class: `viz-chip${off ? ' off' : ''}`, 'data-toggle': i, 'aria-pressed': off ? 'false' : 'true' },
        el('span', { class: 'viz-dash', style: { '--c': colors[i] } }),
        el('span', { text: s.label }),
      );
    }),
  );
}

export function line(viz, ctx) {
  const { x_labels: xs, series, unit } = viz;
  const n = xs.length;
  const hidden = ctx.state.hidden;
  const visible = series.map((_, i) => i).filter((i) => !hidden.has(i));
  const all = visible.flatMap((i) => series[i].values);
  const mn = all.length ? Math.min(...all) : 0;
  const mx = all.length ? Math.max(...all) : 1;
  // Zero unless the values sit far from it: a line's shape matters more than its base.
  const scale = niceScale(mn >= 0 && mn <= mx * 0.5 ? 0 : mn, mx, 3);
  const colors = series.map((s, i) => seriesColor(i, s.tone));
  const base = Math.min(Math.max(0, scale.lo), scale.hi);
  // Overlapping washes muddy each other: the more lines, the fainter each.
  const wash = viz.area ? (visible.length > 1 ? 0.06 : 0.14) : 0.06;
  const washed = viz.area || visible.length === 1;
  const sel = ctx.state.selected;
  const cursor = sel !== null && sel < n ? sel : null;
  const X = (i) => (n > 1 ? i / (n - 1) : 0);
  const Y = (v) => 1 - scale.frac(v);
  const dot = (i, v, color, big) => el('span', { class: `viz-dot${big ? ' big' : ''}`, style: { left: pct(X(i)), top: pct(Y(v)), '--c': color } });
  const plot = el(
    'div',
    { class: 'viz-plot', 'data-line': n },
    el(
      'div',
      { class: 'viz-line-grid', 'aria-hidden': 'true' },
      scale.ticks.map((t) => el('span', { class: t === 0 ? 'zero' : null, style: { top: pct(Y(t)) } })),
    ),
    sv(
      'svg',
      { class: 'viz-line-svg', viewBox: '0 0 1000 1000', preserveAspectRatio: 'none', 'aria-hidden': 'true' },
      visible.map((s) => {
        const pts = series[s].values.map((v, i) => [X(i) * 1000, Y(v) * 1000]);
        const curve = monotone(pts);
        const floor = Y(base) * 1000;
        return [
          washed && sv('path', { d: `${curve}L1000,${floor}L0,${floor}Z`, style: { fill: colors[s], fillOpacity: wash } }),
          sv('path', { class: 'viz-line-stroke', d: curve, style: { '--c': colors[s] } }),
        ];
      }),
    ),
    cursor !== null && el('span', { class: 'viz-cursor', style: { left: pct(X(cursor)) } }),
    cursor !== null
      ? visible.map((s) => dot(cursor, series[s].values[cursor], colors[s], true))
      : visible.map((s) => el('span', { class: 'viz-end' }, dot(n - 1, series[s].values[n - 1], colors[s], false))),
    cursor !== null &&
      readout(
        X(cursor) <= 0.5 ? { left: `calc(${pct(X(cursor))} + 12px)`, top: '2px' } : { right: `calc(${pct(1 - X(cursor))} + 12px)`, top: '2px' },
        el('div', { class: 'viz-readout-head', text: xs[cursor] }),
        visible.map((s) =>
          el(
            'div',
            { class: 'viz-readout-row' },
            el('span', { class: 'viz-dash', style: { '--c': colors[s] } }),
            el('b', { class: 'viz-mono', text: withUnit(formatNumber(series[s].values[cursor]), unit) }),
            el('span', { class: 'muted', text: series[s].label }),
          ),
        ),
      ),
  );
  const shown = shownX(n, ctx.width);
  return el(
    'div',
    { class: 'viz-chart' },
    viz.y_label && caption(viz.y_label + (unit ? ` (${unit})` : '')),
    series.length > 1 && legend(series, colors, hidden),
    el(
      'div',
      { class: 'viz-inset viz-line' },
      el(
        'div',
        { class: 'viz-line-y', 'aria-hidden': 'true' },
        scale.ticks.map((t) => el('span', { style: { top: pct(Y(t)) }, text: formatTick(t, scale.step) })),
      ),
      plot,
      el(
        'div',
        { class: 'viz-line-x', 'aria-hidden': 'true' },
        shown.map((i) =>
          el('span', { class: `${i === 0 ? 'first' : i === n - 1 ? 'last' : ''}${cursor === i ? ' on' : ''}`, style: { left: pct(X(i)) }, text: xs[i] }),
        ),
      ),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Donut

/** A ring's sector from `a0` to `a1` (radians, clockwise from three o'clock), `gap` px apart. */
function sector(c, r0, r1, a0, a1, gap) {
  const p = (r, a) => `${Math.round((c + r * Math.cos(a)) * 100) / 100},${Math.round((c + r * Math.sin(a)) * 100) / 100}`;
  if (a1 - a0 >= Math.PI * 2 - 1e-6) {
    return `M${p(r1, 0)}A${r1},${r1} 0 1 1 ${p(r1, Math.PI)}A${r1},${r1} 0 1 1 ${p(r1, 0)}M${p(r0, 0)}A${r0},${r0} 0 1 0 ${p(r0, Math.PI)}A${r0},${r0} 0 1 0 ${p(r0, 0)}Z`;
  }
  const g = Math.min(gap / ((r0 + r1) / 2), (a1 - a0) / 3);
  const s = a0 + g / 2;
  const e = a1 - g / 2;
  const large = e - s > Math.PI ? 1 : 0;
  return `M${p(r1, s)}A${r1},${r1} 0 ${large} 1 ${p(r1, e)}L${p(r0, e)}A${r0},${r0} 0 ${large} 0 ${p(r0, s)}Z`;
}

export function donut(viz, ctx) {
  const D = 164;
  const thick = 19;
  const outer = D / 2 - 4;
  const { parts, unit } = viz;
  const total = parts.reduce((t, p) => t + p.value, 0);
  const sel = ctx.state.selected;
  const picked = sel !== null && sel < parts.length ? sel : null;
  const colors = parts.map((p, i) => seriesColor(i, p.tone));
  let angle = -Math.PI / 2;
  const sectors = parts.map((p, i) => {
    const a0 = angle;
    angle += (p.value / total) * Math.PI * 2;
    return sv('path', {
      class: 'viz-sector',
      'data-pick': i,
      'fill-rule': 'evenodd',
      d: sector(D / 2, outer - thick, picked === i ? outer + 3 : outer, a0, angle, 1.2),
      style: { fill: colors[i], opacity: picked === null || picked === i ? 1 : 0.4 },
    });
  });
  const center = picked !== null
    ? [
        el('b', { class: 'viz-donut-value', text: withUnit(formatNumber(parts[picked].value), unit) }),
        el('span', { class: 'viz-donut-label', text: parts[picked].label }),
        el('span', { class: 'viz-donut-share viz-mono', text: share(parts[picked].value, total) }),
      ]
    : [el('b', { class: 'viz-donut-value', text: withUnit(formatNumber(total), unit) }), el('span', { class: 'viz-donut-label', text: 'Total' })];
  return el(
    'div',
    { class: 'viz-inset viz-donut' },
    el(
      'div',
      { class: 'viz-donut-ring' },
      sv(
        'svg',
        { viewBox: `0 0 ${D} ${D}`, width: D, height: D, 'aria-hidden': 'true' },
        sv('circle', { class: 'viz-donut-track', cx: D / 2, cy: D / 2, r: outer - thick / 2, 'stroke-width': thick }),
        sectors,
      ),
      el('div', { class: 'viz-donut-center' }, center),
    ),
    el(
      'div',
      { class: 'viz-donut-legend' },
      parts.map((p, i) =>
        el(
          'div',
          { class: `viz-donut-row${picked === i ? ' on' : ''}`, 'data-pick': i, style: { opacity: Math.max(presence(ctx, i), 0.5) } },
          el('span', { class: 'viz-swatch', style: { '--c': colors[i] } }),
          el('span', { class: 'viz-grow', text: p.label }),
          el('span', { class: 'viz-mono viz-donut-num', text: withUnit(formatNumber(p.value), unit) }),
          el('span', { class: 'viz-mono viz-donut-pct', text: share(p.value, total, 0) }),
        ),
      ),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Stats

/** A delta's direction from its sign: up, down, or flat (unsigned). */
function direction(delta) {
  const c = delta.trim()[0];
  if (c === '+' || c === '↑' || c === '▲') return 1;
  if (c === '-' || c === '−' || c === '–' || c === '↓' || c === '▼') return -1;
  return 0;
}

/** A trend of recent values: a quiet line over a wash of the tile's colour, its latest marked. */
function sparkline(values, accent) {
  const n = values.length;
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const pad = hi > lo ? (hi - lo) * 0.1 : 1;
  const X = (i) => (n > 1 ? i / (n - 1) : 0);
  const Y = (v) => 1 - (v - (lo - pad)) / (hi + pad - (lo - pad));
  const curve = monotone(values.map((v, i) => [X(i) * 1000, Y(v) * 1000]));
  return el(
    'div',
    { class: 'viz-spark', 'aria-hidden': 'true' },
    sv(
      'svg',
      { viewBox: '0 0 1000 1000', preserveAspectRatio: 'none' },
      sv('path', { d: `${curve}L1000,1000L0,1000Z`, style: { fill: accent, fillOpacity: 0.09 } }),
      sv('path', { class: 'viz-spark-line', d: curve }),
    ),
    el('span', { class: 'viz-end' }, el('span', { class: 'viz-dot', style: { left: '100%', top: pct(Y(values[n - 1])), '--c': accent } })),
  );
}

export function stats(viz) {
  return el(
    'div',
    { class: 'viz-stats' },
    viz.tiles.map((t, i) => {
      const status = t.tone ? toneColor(t.tone) : 'var(--muted)';
      const accent = !t.tone || t.tone === 'neutral' ? 'var(--sky)' : toneColor(t.tone);
      const dir = t.delta ? direction(t.delta) : 0;
      return el(
        'div',
        { class: 'viz-tile viz-inset', style: { '--i': i } },
        el('div', { class: 'viz-tile-label', text: t.label }),
        el(
          'div',
          { class: 'viz-tile-figure' },
          el('span', { class: 'viz-tile-value', text: t.value }),
          t.delta && el('span', { class: 'viz-delta', style: { '--c': status } }, dir !== 0 && icon(dir > 0 ? 'up' : 'down', 'viz-ico xs'), el('span', { text: t.delta })),
        ),
        t.trend && sparkline(t.trend, accent),
      );
    }),
  );
}

// ---------------------------------------------------------------------------------------------
// Table

/** A table's rows (or a chart's values, on its Data view): figures right-aligned in mono,
 * exact; it scrolls sideways rather than wrapping a number. `rows`: `{ cells, tone }`. */
export function tableView(columns, rows) {
  const numeric = columns.map(
    (_, c) =>
      rows.some((r) => typeof r.cells[c] === 'number') &&
      rows.every((r) => typeof r.cells[c] !== 'string' || r.cells[c] === '' || r.cells[c] === '—'),
  );
  return el(
    'div',
    { class: 'viz-table-wrap viz-inset' },
    el(
      'table',
      { class: 'viz-table' },
      el('thead', null, el('tr', null, columns.map((c, i) => el('th', { class: numeric[i] ? 'num' : null, scope: 'col', text: c })))),
      el(
        'tbody',
        null,
        rows.map((r) =>
          el(
            'tr',
            { class: r.tone ? 'toned' : null, style: r.tone ? { '--c': toneColor(r.tone) } : null },
            columns.map((_, c) => {
              const v = r.cells[c];
              // Text longer than a column's width (26 characters, as the iPhone sizes them) wraps.
              return typeof v === 'number'
                ? el('td', { class: `num viz-mono${c === 0 ? ' first' : ''}`, text: formatExact(v) })
                : el('td', { class: `${numeric[c] ? 'num' : ''}${c === 0 ? ' first' : ''}${(v ?? '').length > 26 ? ' wrap' : ''}`, text: v ?? '' });
            }),
          ),
        ),
      ),
    ),
  );
}

export const table = (viz) => tableView(viz.columns, viz.rows);

// ---------------------------------------------------------------------------------------------
// Heatmap

export function heatmap(viz, ctx) {
  const { x_labels: xs, y_labels: ys, values } = viz;
  const flat = values.flat();
  const mn = Math.min(...flat);
  const mx = Math.max(...flat);
  // Values on both sides of zero diverge from it (two hues); otherwise one hue, light to dark.
  const diverging = mn < 0 && mx > 0;
  const reach = Math.max(Math.abs(mn), Math.abs(mx), Number.EPSILON);
  const span = Math.max(mx - mn, Number.EPSILON);
  const cols = xs.length;
  const labelW = 72;
  const cellW = (ctx.width - 26 - labelW - cols * 3) / Math.max(cols, 1);
  const every = Math.max(Math.ceil(40 / Math.max(cellW, 1)), 1);
  const numbers = cellW >= 30;
  const sel = ctx.state.selected;
  const picked = sel !== null && sel < cols * ys.length ? [Math.floor(sel / cols), sel % cols] : null;
  const night = dark();
  const paint = (v) => (diverging ? [v < 0 ? 'var(--failed)' : 'var(--sky)', Math.abs(v) / reach] : ['var(--sky)', (v - mn) / span]);
  const grid = el(
    'div',
    { class: 'viz-heat', style: { gridTemplateColumns: `${labelW}px repeat(${cols}, minmax(8px, 1fr))` } },
    el('span'),
    xs.map((x, c) => el('span', { class: `viz-heat-x${picked?.[1] === c ? ' on' : ''}`, text: c % every === 0 ? x : '' })),
    values.map((row, r) => [
      el('span', { class: `viz-heat-y${picked?.[0] === r ? ' on' : ''}`, text: ys[r] }),
      row.map((v, c) => {
        const [color, strength] = paint(v);
        const s = Math.min(Math.max(strength, 0), 1);
        const light = night === s < 0.55;
        const ix = r * cols + c;
        return el(
          'span',
          {
            class: `viz-cell${sel === ix ? ' on' : ''}${light ? ' light' : ''}`,
            'data-pick': ix,
            style: { background: mix(color, 0.07 + s * 0.85), '--d': c },
            title: `${ys[r]} · ${xs[c]}: ${formatNumber(v)}`,
          },
          numbers && formatNumber(v),
        );
      }),
    ]),
  );
  const ramp = diverging ? 'linear-gradient(90deg, var(--failed), var(--sky))' : `linear-gradient(90deg, ${mix('var(--sky)', 0.07)}, ${mix('var(--sky)', 0.92)})`;
  return el(
    'div',
    { class: 'viz-chart' },
    el('div', { class: 'viz-inset' }, grid),
    el(
      'div',
      { class: 'viz-heat-key viz-mono' },
      picked && el('span', { class: 'viz-heat-pick' }, `${ys[picked[0]]} · ${xs[picked[1]]}: `, el('b', { text: formatNumber(values[picked[0]][picked[1]]) })),
      el('span', { class: 'viz-grow' }),
      el('span', { text: formatNumber(diverging ? -reach : mn) }),
      el('span', { class: 'viz-ramp', style: { background: ramp } }),
      el('span', { text: formatNumber(diverging ? reach : mx) }),
    ),
  );
}

// ---------------------------------------------------------------------------------------------
// Treemap

export function treemap(viz, ctx) {
  const height = 230;
  const width = Math.max(ctx.width, 120);
  const { items } = viz;
  const total = items.reduce((t, x) => t + x.weight, 0);
  const biggest = Math.max(...items.map((x) => x.weight), Number.EPSILON);
  const order = items.map((_, i) => i).sort((a, b) => items[b].weight - items[a].weight);
  const tiles = squarify(
    items.map((x) => x.weight),
    width,
    height,
  );
  const sel = ctx.state.selected;
  const picked = sel !== null && sel < items.length ? sel : null;
  const parts = items.map((item, i) => {
    const r = tiles[i];
    // Untoned tiles are quiet greys stepped by weight, so a toned one stands out.
    const toned = !!item.tone;
    const color = toned ? toneColor(item.tone) : 'var(--fg)';
    const k = Math.sqrt(item.weight / biggest);
    const on = picked === i;
    const strength = (toned ? 0.16 + 0.22 * k : 0.03 + 0.07 * k) + (on ? (toned ? 0.1 : 0.04) : 0);
    const w = r.w - 3;
    const h = r.h - 3;
    return el(
      'div',
      {
        class: 'viz-tm',
        'data-pick': i,
        title: `${item.label}, ${formatNumber(item.weight)}, ${share(item.weight, total)}`,
        style: {
          left: pct((r.x + 1.5) / width),
          top: `${r.y + 1.5}px`,
          width: pct(Math.max(w, 0) / width),
          height: `${Math.max(h, 0)}px`,
          background: mix(color, strength),
          borderColor: mix(color, on ? 0.6 : toned ? 0.32 : 0.07),
          opacity: Math.max(presence(ctx, i), 0.45),
          '--d': order.indexOf(i),
        },
      },
      w > 54 && h > 30 && el('span', { class: 'viz-tm-label', text: item.label }),
      w > 54 &&
        h > 48 &&
        el('span', { class: 'viz-tm-num viz-mono' }, el('span', { text: formatNumber(item.weight) }), w > 96 && el('span', { class: 'muted', text: share(item.weight, total, 0) })),
    );
  });
  let read = null;
  if (picked !== null) {
    const r = tiles[picked];
    const left = r.x + r.w / 2 < width / 2;
    const top = r.y + r.h / 2 < height / 2;
    read = readout(
      { [left ? 'left' : 'right']: '8px', [top ? 'bottom' : 'top']: '8px' },
      el('div', { class: 'viz-readout-title', text: items[picked].label }),
      el('div', { class: 'viz-mono muted', text: `${formatNumber(items[picked].weight)} · ${share(items[picked].weight, total)}` }),
    );
  }
  return el('div', { class: 'viz-treemap', style: { height: `${height}px` } }, parts, read);
}
