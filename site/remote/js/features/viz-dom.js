// What every part of a drawn visualization is built with: elements made with DOM calls (text
// always as text, so nothing an agent writes is ever read as markup), icons as Lucide's shapes,
// and the colours, as the theme's tokens so both themes and the computer's own look draw them.

const SVG = 'http://www.w3.org/2000/svg';

function apply(node, props) {
  if (!props) return;
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'style') {
      for (const [p, v] of Object.entries(value)) {
        if (v === undefined || v === null) continue;
        if (p.startsWith('--')) node.style.setProperty(p, v);
        else node.style[p] = v;
      }
    } else if (key === 'text') {
      node.textContent = value;
    } else {
      node.setAttribute(key, value === true ? '' : String(value));
    }
  }
}

function append(node, kids) {
  for (const kid of kids.flat(Infinity)) {
    if (kid === null || kid === undefined || kid === false) continue;
    node.append(typeof kid === 'object' ? kid : String(kid));
  }
}

/** An HTML element: `el('div', { class, style: {…}, text, …attributes }, …children)`. */
export function el(tag, props, ...kids) {
  const node = document.createElement(tag);
  apply(node, props);
  append(node, kids);
  return node;
}

/** An SVG element, the same way. */
export function sv(tag, props, ...kids) {
  const node = document.createElementNS(SVG, tag);
  apply(node, props);
  append(node, kids);
  return node;
}

// Lucide (ISC licence), 24 × 24, stroked: each icon its shapes.
const ICONS = {
  bar: [['path', 'M3 3v16a2 2 0 0 0 2 2h16'], ['path', 'M7 16h8'], ['path', 'M7 11h12'], ['path', 'M7 6h3']],
  line: [['path', 'M3 3v16a2 2 0 0 0 2 2h16'], ['path', 'm19 9-5 5-4-4-3 3']],
  donut: [['path', 'M21 12c.552 0 1.005-.449.95-.998a10 10 0 0 0-8.953-8.951c-.55-.055-.998.398-.998.95v8a1 1 0 0 0 1 1z'], ['path', 'M21.21 15.89A10 10 0 1 1 8 2.83']],
  stats: [['path', 'm12 14 4-4'], ['path', 'M3.34 19a10 10 0 1 1 17.32 0']],
  table: [['path', 'M12 3v18'], ['rect', { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ['path', 'M3 9h18'], ['path', 'M3 15h18']],
  heatmap: [['rect', { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ['path', 'M3 9h18'], ['path', 'M3 15h18'], ['path', 'M9 3v18'], ['path', 'M15 3v18']],
  treemap: [
    ['rect', { width: 7, height: 9, x: 3, y: 3, rx: 1 }],
    ['rect', { width: 7, height: 5, x: 14, y: 3, rx: 1 }],
    ['rect', { width: 7, height: 9, x: 14, y: 12, rx: 1 }],
    ['rect', { width: 7, height: 5, x: 3, y: 16, rx: 1 }],
  ],
  timeline: [['path', 'M10 6h8'], ['path', 'M12 16h6'], ['path', 'M3 3v16a2 2 0 0 0 2 2h16'], ['path', 'M8 11h7']],
  flow: [['rect', { width: 8, height: 8, x: 3, y: 3, rx: 2 }], ['path', 'M7 11v4a2 2 0 0 0 2 2h4'], ['rect', { width: 8, height: 8, x: 13, y: 13, rx: 2 }]],
  layers: [
    ['path', 'm12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z'],
    ['path', 'm22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65'],
    ['path', 'm22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65'],
  ],
  mockup: [['rect', { x: 2, y: 4, width: 20, height: 16, rx: 2 }], ['path', 'M10 4v4'], ['path', 'M2 8h20'], ['path', 'M6 4v4']],
  chart: [['path', 'M3 3v16a2 2 0 0 0 2 2h16'], ['path', 'M18 17V9'], ['path', 'M13 17V5'], ['path', 'M8 17v-3']],
  more: [['circle', { cx: 12, cy: 12, r: 1 }], ['circle', { cx: 19, cy: 12, r: 1 }], ['circle', { cx: 5, cy: 12, r: 1 }]],
  up: [['path', 'M7 7h10v10'], ['path', 'M7 17 17 7']],
  down: [['path', 'm7 7 10 10'], ['path', 'M17 7v10H7']],
  alert: [['path', 'm21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3'], ['path', 'M12 9v4'], ['path', 'M12 17h.01']],
  copy: [['rect', { width: 14, height: 14, x: 8, y: 8, rx: 2 }], ['path', 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2']],
  search: [['circle', { cx: 11, cy: 11, r: 8 }], ['path', 'm21 21-4.3-4.3']],
  chevron: [['path', 'm9 18 6-6-6-6']],
  backward: [['path', 'm15 18-6-6 6-6']],
  lock: [['rect', { width: 18, height: 11, x: 3, y: 11, rx: 2 }], ['path', 'M7 11V7a5 5 0 0 1 10 0v4']],
  image: [['rect', { width: 18, height: 18, x: 3, y: 3, rx: 2 }], ['circle', { cx: 9, cy: 9, r: 2 }], ['path', 'm21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21']],
  signal: [['path', 'M2 20h.01'], ['path', 'M7 20v-4'], ['path', 'M12 20v-8'], ['path', 'M17 20V8']],
  wifi: [['path', 'M12 20h.01'], ['path', 'M2 8.82a15 15 0 0 1 20 0'], ['path', 'M5 12.859a10 10 0 0 1 14 0'], ['path', 'M8.5 16.429a5 5 0 0 1 7 0']],
  battery: [['rect', { width: 16, height: 10, x: 2, y: 7, rx: 2 }], ['path', 'M22 11v2'], ['path', 'M6 11v2'], ['path', 'M10 11v2'], ['path', 'M14 11v2']],
};

/** A kind's icon by its wire name; one not known (or not yet written) gets a plain chart. */
export function icon(name, cls = 'viz-ico') {
  const shapes = ICONS[name] || ICONS.chart;
  return sv(
    'svg',
    { class: cls, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
    shapes.map(([tag, attrs]) => sv(tag, typeof attrs === 'string' ? { d: attrs } : attrs)),
  );
}

/** A mark's colour for its tone; no tone is the accent. */
export function toneColor(tone) {
  switch (tone || 'accent') {
    case 'neutral':
      return 'var(--muted)';
    case 'positive':
      return 'var(--done)';
    case 'warning':
      return 'var(--approval)';
    case 'negative':
      return 'var(--failed)';
    case 'info':
      return 'var(--sky)';
    default:
      return 'var(--accent)';
  }
}

const SERIES = ['var(--sky)', 'var(--accent)', 'var(--done)', 'var(--plan)', 'var(--approval)', 'var(--viz-pink)'];

/** Series `i`'s colour: its tone when it has one, else the next of six hues (by its place in the
 * data, so switching others off never repaints it). */
export function seriesColor(i, tone) {
  return tone ? toneColor(tone) : SERIES[i % SERIES.length];
}

/** `color` at `amount` (0 to 1) over nothing. */
export const mix = (color, amount) => `color-mix(in srgb, ${color} ${Math.round(Math.max(0, Math.min(1, amount)) * 1000) / 10}%, transparent)`;

export const pct = (f) => `${Math.round(f * 100000) / 1000}%`;

export const dark = () => document.documentElement.dataset.scheme !== 'light';

/** A small caption over or under a chart: an axis's name. */
export const caption = (text, cls = '') => el('div', { class: `viz-caption ${cls}`, text });

/** A floating readout over the marks. */
export const readout = (style, ...kids) => el('div', { class: 'viz-readout', style }, ...kids);
