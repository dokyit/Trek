// A `trek-viz` block in an answer, as the iPhone draws it (VizCard.swift): a card with a hairline
// edge, its kind's icon, its title and summary, and the chart on an inset surface; Chart | Data
// shows the values behind it, and the menu copies them. A block still being written draws as far
// as it's written ("Drawing…"), or as a quiet skeleton of its kind until it can; one that can't be
// drawn says why over its JSON.

import { h, render } from '../../vendor/preact.mjs';
import { Menu, addIcons } from '../ui.js';
import * as app from '../model.js';
import { dataTable, describeViz, tableCsv, tableMarkdown, toJson } from './viz-spec.js';
import { el, icon } from './viz-dom.js';
import { bars, donut, heatmap, line, stats, table, tableView, treemap } from './viz-charts.js';
import { flow, layers, timeline } from './viz-diagrams.js';
import { mockup } from './viz-mockup.js';

addIcons({
  'viz-table': '<path d="M12 3v18"/><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M3 15h18"/>',
  'viz-json': '<path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1"/><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1"/>',
});

const DRAW = { bar: bars, line, donut, stats, table, heatmap, treemap, timeline, flow, layers, mockup };

/** Visualizations whose entrance has played (by type and title, both written before their marks),
 * so one streaming in, or scrolled back to, doesn't replay it. */
const played = new Set();

async function copyText(text, done) {
  try {
    await navigator.clipboard.writeText(text);
    app.toast(done);
  } catch {
    app.toast('Couldn’t copy', 'negative');
  }
}

function openMenu(viz, data) {
  const host = document.createElement('div');
  document.body.append(host);
  const close = () => {
    render(null, host);
    host.remove();
  };
  const items = [
    data && { label: 'Copy as Markdown table', icon: 'viz-table', onClick: () => copyText(tableMarkdown(data), 'Table copied') },
    data && { label: 'Copy as CSV', icon: 'copy', onClick: () => copyText(tableCsv(data), 'CSV copied') },
    { label: 'Copy data as JSON', icon: 'viz-json', onClick: () => copyText(toJson(viz), 'Visualization data copied') },
  ].filter(Boolean);
  render(h(Menu, { title: viz.title, items, onClose: close }), host);
}

function head(kind, title, actions) {
  return el('div', { class: 'viz-head' }, el('span', { class: 'viz-kind' }, icon(kind)), title, actions);
}

/** A visualization's card. `drawing`: its marks are still arriving (nothing whole to copy yet);
 * `width`: what its chart has to lay out in. */
export function card(viz, { drawing = false, width = 330 } = {}) {
  const state = { showData: false, selected: null, hidden: new Set() };
  const data = dataTable(viz);
  const ident = `${viz.type}\n${viz.title}`;
  const entering = !played.has(ident);
  played.add(ident);
  if (played.size > 500) played.delete(played.values().next().value);
  const ctx = { state, width };
  const seg =
    data && viz.type !== 'table' && !drawing
      ? el(
          'div',
          { class: 'viz-seg', role: 'group', 'aria-label': 'View' },
          el('button', { type: 'button', 'data-view': 'chart', text: 'Chart' }),
          el('button', { type: 'button', 'data-view': 'data', text: 'Data' }),
        )
      : null;
  const actions = drawing
    ? el('span', { class: 'viz-drawing', text: 'Drawing…' })
    : el('div', { class: 'viz-actions' }, seg, el('button', { type: 'button', class: 'viz-more', 'data-menu': '', 'aria-label': 'Copy', title: 'Copy' }, icon('more')));
  const body = el('div', { class: 'viz-body' });
  const root = el(
    'figure',
    { class: `viz${entering ? ' enter' : ''}`, 'data-kind': viz.type, 'aria-label': `Visualization: ${viz.title}` },
    head(viz.type, el('div', { class: 'viz-title', text: viz.title }), actions),
    el('p', { class: 'viz-summary', text: viz.summary }),
    body,
    el('p', { class: 'sr', text: describeViz(viz) }),
  );

  const draw = () => {
    try {
      drawBody();
    } catch {
      // Marks this can't lay out: the values behind them instead.
      state.showData = !!data;
      if (data) drawBody();
    }
  };
  const drawBody = () => {
    if (state.showData && data) {
      body.replaceChildren(
        tableView(
          data.columns,
          data.rows.map((cells) => ({ cells })),
        ),
        !drawing && el('button', { type: 'button', class: 'viz-csv', 'data-csv': '' }, icon('copy', 'viz-ico xs'), 'CSV'),
      );
    } else {
      body.replaceChildren(DRAW[viz.type](viz, ctx));
    }
    for (const b of seg?.children || []) b.setAttribute('aria-pressed', String((b.dataset.view === 'data') === state.showData));
  };

  root.addEventListener('click', (e) => {
    const t = e.target.closest('[data-view], [data-menu], [data-csv], [data-toggle], [data-pick], [data-line]');
    if (!t || !root.contains(t)) return;
    root.classList.remove('enter');
    if (t.dataset.view) {
      state.showData = t.dataset.view === 'data';
      state.selected = null;
    } else if (t.hasAttribute('data-menu')) {
      return openMenu(viz, data);
    } else if (t.hasAttribute('data-csv')) {
      return copyText(tableCsv(data), 'CSV copied');
    } else if (t.dataset.toggle !== undefined) {
      // A series on or off from the legend; never all of them off.
      const i = Number(t.dataset.toggle);
      if (state.hidden.has(i)) state.hidden.delete(i);
      else if (state.hidden.size + 1 < viz.series.length) state.hidden.add(i);
    } else {
      let i = Number(t.dataset.pick);
      if (t.dataset.line) {
        const n = Number(t.dataset.line);
        const rect = t.getBoundingClientRect();
        i = Math.min(Math.max(Math.round(((e.clientX - rect.left) / Math.max(rect.width, 1)) * (n - 1)), 0), n - 1);
      }
      // Picks mark `i`, or lets go of it when it's already picked.
      state.selected = state.selected === i ? null : i;
    }
    draw();
  });

  /** Drawn again for a new width (the phone turned), when it's changed enough to matter. */
  root.vizFit = (w) => {
    if (Math.abs(w - ctx.width) < 16) return;
    ctx.width = w;
    draw();
  };
  draw();
  if (entering) setTimeout(() => root.classList.remove('enter'), 1400);
  return root;
}

const SKELETON_HEIGHT = { stats: 104, bar: 170, heatmap: 170, timeline: 170, donut: 300, table: 200, line: 230, treemap: 230, flow: 230, layers: 250, mockup: 320 };

/** A visualization still being written with nothing whole to draw yet: its card, its type and
 * title as soon as they're written, and a quiet skeleton of its marks where they will go. */
export function pending(kind, title) {
  let marks = null;
  if (['bar', 'timeline', 'layers', 'table'].includes(kind)) {
    marks = el(
      'div',
      { class: 'viz-sk-rows' },
      [0.82, 0.58, 0.7, 0.36, 0.5].map((w) => el('div', null, el('i'), el('span', null, el('i', { style: { width: `${w * 100}%` } })))),
    );
  } else if (kind === 'stats') {
    marks = el(
      'div',
      { class: 'viz-sk-tiles' },
      [0, 1].map(() => el('div', null, el('i', { class: 'a' }), el('i', { class: 'b' }), el('i', { class: 'c' }))),
    );
  }
  return el(
    'figure',
    { class: 'viz pending', 'data-kind': kind || '', 'aria-label': title ? `Visualization being drawn: ${title}` : 'Visualization being drawn' },
    head(kind, title ? el('div', { class: 'viz-title', text: title }) : el('span', { class: 'viz-stub title' }), el('span', { class: 'viz-drawing', text: 'Drawing…' })),
    el('p', { class: 'viz-summary' }, el('span', { class: 'viz-stub summary' })),
    el('div', { class: 'viz-body' }, el('div', { class: 'viz-skeleton', style: { height: `${SKELETON_HEIGHT[kind] || 180}px` } }, marks)),
  );
}

/** A block that can't be drawn: a short note saying why, over its JSON (left as code). */
export function refused(reason) {
  return el('div', { class: 'viz-refused', role: 'note' }, icon('alert', 'viz-ico s'), el('span', { text: `Couldn’t draw this visualization: ${reason}` }));
}
