// The `trek-viz` format: strict JSON in a fenced block, which the remote draws as the Mac and the
// iPhone do. This is `trek_core::visualization` in JavaScript: the same slips forgiven (comments,
// trailing commas, synonym keys, dressed-up numbers), the same limits, the same error wording,
// and the same reading of a block still being written. Then what the charts draw with besides
// their marks (VisualizationData.swift): numbers as the Mac writes them, the table behind every
// chart, nice scales, squarified treemaps, layered flows and packed timeline lanes. No DOM here,
// so it runs in Node's tests too.

export const VERSION = 1;
export const MAX_PAYLOAD_BYTES = 32 * 1024;
export const MAX_TEXT_CHARS = 256;
export const MAX_MARKS = 128;
export const MAX_MOCK_DEPTH = 6;
export const MAX_ABS_VALUE = 1_000_000_000_000;
/** Short strings set in tight places: stat values and deltas, units, ids, lane names, ticks. */
export const MAX_SHORT_CHARS = 48;
export const MAX_SERIES = 6;
export const MAX_PARTS = 12;
export const MAX_TILES = 12;
export const MAX_TREND_POINTS = 64;
export const MAX_COLUMNS = 12;
export const MAX_ROWS = 64;
export const MAX_TABLE_CELLS = 384;
export const MAX_EVENTS = 48;
export const MAX_FLOW_NODES = 24;
export const MAX_FLOW_EDGES = 48;
export const MAX_FLOW_GROUPS = 6;
export const MAX_LAYERS = 8;
export const MAX_LAYER_ITEMS = 8;
export const MAX_TABS = 8;

/** Every `type`, in the order the agent instructions list them. */
export const TYPES = ['bar', 'line', 'donut', 'stats', 'table', 'heatmap', 'treemap', 'timeline', 'flow', 'layers', 'mockup'];
export const TONES = ['neutral', 'accent', 'positive', 'warning', 'negative', 'info'];
export const MOCK_KINDS = ['row', 'column', 'card', 'text', 'metric', 'button', 'badge', 'divider', 'input', 'toggle', 'progress', 'avatar', 'image', 'list', 'tabs', 'nav', 'frame'];
export const MOCK_CONTAINERS = ['row', 'column', 'card', 'list', 'tabs', 'nav', 'frame'];
export const MOCK_DEVICES = ['phone', 'browser', 'window', 'tablet'];
export const IMAGE_SHAPES = ['wide', 'square', 'tall'];

/** The top-level fields each type takes (the first four every type takes). */
const TOP_FIELDS = {
  bar: ['version', 'title', 'summary', 'type', 'bars', 'x_label', 'y_label'],
  heatmap: ['version', 'title', 'summary', 'type', 'x_labels', 'y_labels', 'values'],
  treemap: ['version', 'title', 'summary', 'type', 'items'],
  mockup: ['version', 'title', 'summary', 'type', 'nodes'],
  line: ['version', 'title', 'summary', 'type', 'x_labels', 'series', 'area', 'y_label', 'unit'],
  donut: ['version', 'title', 'summary', 'type', 'parts', 'unit'],
  stats: ['version', 'title', 'summary', 'type', 'tiles'],
  table: ['version', 'title', 'summary', 'type', 'columns', 'rows'],
  timeline: ['version', 'title', 'summary', 'type', 'events', 'ticks', 'unit'],
  flow: ['version', 'title', 'summary', 'type', 'nodes', 'edges'],
  layers: ['version', 'title', 'summary', 'type', 'layers'],
};

// ---------------------------------------------------------------------------------------------
// Errors

/** Why a block can't be drawn. `message` reads as the Mac's error does; `reason` is the part
 * after "Couldn't draw this visualization: ". */
export class VizError extends Error {
  constructor(kind, detail, extra = {}) {
    super('');
    this.kind = kind;
    this.detail = detail;
    Object.assign(this, extra);
    this.message = this.display();
  }

  display() {
    switch (this.kind) {
      case 'too_large':
        return `visualization payload is ${this.bytes} bytes; maximum is ${MAX_PAYLOAD_BYTES}`;
      case 'json':
        return `invalid visualization JSON: ${this.detail}`;
      case 'version':
        return `unsupported visualization schema version ${this.version}; expected ${VERSION}`;
      default:
        return `invalid visualization: ${this.detail}`;
    }
  }

  get reason() {
    switch (this.kind) {
      case 'too_large':
        return `it’s ${(this.bytes / 1024).toFixed(1)} KiB, over the ${MAX_PAYLOAD_BYTES / 1024} KiB limit`;
      case 'version':
        return `schema version ${this.version} isn’t supported (expected ${VERSION})`;
      default:
        return this.detail;
    }
  }
}

const invalid = (detail) => new VizError('invalid', detail);
const jsonError = (detail) => new VizError('json', detail);

// ---------------------------------------------------------------------------------------------
// Small helpers

const isObject = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
const has = (o, k) => isObject(o) && Object.prototype.hasOwnProperty.call(o, k);
const chars = (s) => [...s].length;

/** UTF-8 length, as the limits count. */
export function byteLength(text) {
  let n = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x80) n += 1;
    else if (c < 0x800) n += 2;
    else if (c >= 0xd800 && c < 0xdc00 && i + 1 < text.length) {
      n += 4;
      i++;
    } else n += 3;
  }
  return n;
}

function deepEqual(a, b) {
  if (a === b) return true;
  if (Array.isArray(a)) return Array.isArray(b) && a.length === b.length && a.every((x, i) => deepEqual(x, b[i]));
  if (isObject(a)) {
    if (!isObject(b)) return false;
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length && ka.every((k) => has(b, k) && deepEqual(a[k], b[k]));
  }
  return false;
}

/** How serde names a value in "invalid type: …". */
function describe(v) {
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'boolean') return `boolean \`${v}\``;
  if (typeof v === 'number') return Number.isInteger(v) ? `integer \`${v}\`` : `floating point \`${v}\``;
  if (typeof v === 'string') return `string ${JSON.stringify(v)}`;
  if (Array.isArray(v)) return 'sequence';
  return 'map';
}

function expectedList(names) {
  if (names.length === 0) return 'there are no fields';
  if (names.length === 1) return `\`${names[0]}\``;
  if (names.length === 2) return `\`${names[0]}\` or \`${names[1]}\``;
  return `one of ${names.map((n) => `\`${n}\``).join(', ')}`;
}

// ---------------------------------------------------------------------------------------------
// Tidying: JSON5-ish noise a strict parser rejects

/** `//` and `/* *\/` comments and commas that only precede a closing bracket, all outside
 * strings, so a "https://" in a label survives. */
export function tidy(payload) {
  let out = '';
  let i = 0;
  let inString = false;
  let escaped = false;
  const n = payload.length;
  const skipGap = (j) => {
    for (;;) {
      while (j < n && /\s/.test(payload[j])) j++;
      if (payload.startsWith('//', j)) {
        while (j < n && payload[j] !== '\n') j++;
      } else if (payload.startsWith('/*', j)) {
        const end = payload.indexOf('*/', j + 2);
        j = end < 0 ? n : end + 2;
      } else return j;
    }
  };
  while (i < n) {
    const c = payload[i];
    if (inString) {
      out += c;
      if (escaped) escaped = false;
      else if (c === '\\') escaped = true;
      else if (c === '"') inString = false;
      i++;
      continue;
    }
    if (c === '"') {
      inString = true;
      out += c;
      i++;
    } else if (c === '/' && payload[i + 1] === '/') {
      while (i < n && payload[i] !== '\n') i++;
    } else if (c === '/' && payload[i + 1] === '*') {
      const end = payload.indexOf('*/', i + 2);
      i = end < 0 ? n : end + 2;
    } else if (c === ',' && (payload[skipGap(i + 1)] === ']' || payload[skipGap(i + 1)] === '}')) {
      i++;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** One stray inner fence, when a model wraps the JSON in ```json…``` inside the block. Markdown
 * ends the block at the inner fence's closing line, so only its opening line may be left. */
export function unwrapInnerFence(payload) {
  const trimmed = payload.trim();
  if (!trimmed.startsWith('```') && !trimmed.startsWith('~~~')) return payload;
  const nl = trimmed.indexOf('\n');
  if (nl < 0) return payload;
  let inner = trimmed.slice(nl + 1).trimEnd();
  if (inner.endsWith('```')) inner = inner.slice(0, -3);
  return inner.includes('```') ? payload : inner;
}

// ---------------------------------------------------------------------------------------------
// Normalizing: the names and shapes models actually write, at schema-known fields only

const TEXT = 'text';
const NUMBER = 'number';
const TEXTS = 'texts';
const NUMBERS = 'numbers';
const OBJECTS = 'objects';
const MATRIX = 'matrix';

const COLLECTIONS = [
  ['bar', 'bars', OBJECTS],
  ['line', 'series', OBJECTS],
  ['line', 'x_labels', TEXTS],
  ['donut', 'parts', OBJECTS],
  ['stats', 'tiles', OBJECTS],
  ['table', 'columns', TEXTS],
  ['heatmap', 'x_labels', TEXTS],
  ['heatmap', 'y_labels', TEXTS],
  ['heatmap', 'values', MATRIX],
  ['treemap', 'items', OBJECTS],
  ['timeline', 'events', OBJECTS],
  ['timeline', 'ticks', TEXTS],
  ['flow', 'nodes', OBJECTS],
  ['flow', 'edges', OBJECTS],
  ['layers', 'layers', OBJECTS],
  ['mockup', 'nodes', OBJECTS],
];

const PLURALS = [
  ['bar', 'bars'],
  ['part', 'parts'],
  ['tile', 'tiles'],
  ['column', 'columns'],
  ['row', 'rows'],
  ['item', 'items'],
  ['event', 'events'],
  ['tick', 'ticks'],
  ['node', 'nodes'],
  ['edge', 'edges'],
  ['layer', 'layers'],
  ['child', 'children'],
  ['value', 'values'],
  ['x_label', 'x_labels'],
  ['y_label', 'y_labels'],
  ['milestone', 'events'],
  ['slice', 'parts'],
];

const TYPE_ALIASES = new Map([
  ['bars', 'bar'],
  ['bar_chart', 'bar'],
  ['barchart', 'bar'],
  ['bar_graph', 'bar'],
  ['column_chart', 'bar'],
  ['histogram', 'bar'],
  ['line_chart', 'line'],
  ['linechart', 'line'],
  ['area', 'line'],
  ['doughnut', 'donut'],
  ['pie', 'donut'],
  ['pie_chart', 'donut'],
  ['pie_graph', 'donut'],
  ['stat', 'stats'],
  ['statistics', 'stats'],
  ['metrics', 'stats'],
  ['kpi', 'stats'],
  ['kpis', 'stats'],
  ['heat_map', 'heatmap'],
  ['matrix', 'heatmap'],
  ['tree_map', 'treemap'],
  ['gantt', 'timeline'],
  ['time_line', 'timeline'],
  ['roadmap', 'timeline'],
  ['schedule', 'timeline'],
  ['flowchart', 'flow'],
  ['flow_chart', 'flow'],
  ['diagram', 'flow'],
  ['graph', 'flow'],
  ['layer', 'layers'],
  ['stack', 'layers'],
  ['tiers', 'layers'],
  ['wireframe', 'mockup'],
  ['wire_frame', 'mockup'],
  ['mock', 'mockup'],
]);

/** One type's collection under another name. `data` only where a type has one obvious dataset. */
const FIELD_ALIASES = [
  ['bar', 'data', 'bars'],
  ['line', 'data', 'series'],
  ['line', 'labels', 'x_labels'],
  ['line', 'categories', 'x_labels'],
  ['line', 'x_axis', 'x_label'],
  ['line', 'y_axis', 'y_label'],
  ['line', 'units', 'unit'],
  ['donut', 'data', 'parts'],
  ['donut', 'slices', 'parts'],
  ['donut', 'segments', 'parts'],
  ['donut', 'units', 'unit'],
  ['stats', 'data', 'tiles'],
  ['stats', 'metrics', 'tiles'],
  ['stats', 'cards', 'tiles'],
  ['stats', 'kpis', 'tiles'],
  ['table', 'data', 'rows'],
  ['table', 'headers', 'columns'],
  ['heatmap', 'data', 'values'],
  ['treemap', 'data', 'items'],
  ['timeline', 'data', 'events'],
  ['timeline', 'phases', 'events'],
  ['timeline', 'milestones', 'events'],
  ['timeline', 'units', 'unit'],
  ['flow', 'links', 'edges'],
  ['flow', 'connections', 'edges'],
  ['layers', 'tiers', 'layers'],
  ['bar', 'x_axis', 'x_label'],
  ['bar', 'y_axis', 'y_label'],
];

/** Keys a mark carries under another name, per type and collection (`*` for any). */
const MARK_ALIASES = [
  ['*', '*', 'name', 'label'],
  ['*', '*', 'title', 'label'],
  ['bar', '*', 'x', 'label'],
  ['bar', '*', 'y', 'value'],
  ['donut', '*', 'x', 'label'],
  ['donut', '*', 'y', 'value'],
  ['treemap', '*', 'x', 'label'],
  ['treemap', '*', 'y', 'weight'],
  ['treemap', '*', 'size', 'weight'],
  ['treemap', '*', 'value', 'weight'],
  ['line', 'series', 'data', 'values'],
  ['line', 'series', 'points', 'values'],
  ['stats', '*', 'change', 'delta'],
  ['stats', '*', 'sparkline', 'trend'],
  ['stats', '*', 'data', 'trend'],
  ['timeline', '*', 'date', 'start'],
  ['timeline', '*', 'time', 'start'],
  ['timeline', '*', 'at', 'start'],
  ['timeline', '*', 'begin', 'start'],
  ['timeline', '*', 'from', 'start'],
  ['timeline', '*', 'until', 'end'],
  ['timeline', '*', 'through', 'end'],
  ['timeline', '*', 'to', 'end'],
  ['timeline', '*', 'track', 'lane'],
  ['timeline', '*', 'swimlane', 'lane'],
  ['timeline', '*', 'group', 'lane'],
  ['flow', 'edges', 'source', 'from'],
  ['flow', 'edges', 'target', 'to'],
  ['flow', 'edges', 'text', 'label'],
  ['flow', 'nodes', 'key', 'id'],
  ['layers', '*', 'contents', 'items'],
  ['layers', '*', 'components', 'items'],
  ['layers', '*', 'includes', 'items'],
  ['table', 'rows', 'data', 'cells'],
  ['table', 'rows', 'values', 'cells'],
  ['mockup', '*', 'type', 'kind'],
  ['mockup', '*', 'name', 'text'],
  ['mockup', '*', 'title', 'text'],
  ['mockup', '*', 'body', 'text'],
  ['mockup', '*', 'content', 'text'],
  ['mockup', '*', 'items', 'children'],
  ['mockup', '*', 'nodes', 'children'],
];

/** Colour words for the six tones; CSS and hex colours stay errors. */
const TONE_ALIASES = new Map([
  ['neutral', 'neutral'],
  ['gray', 'neutral'],
  ['grey', 'neutral'],
  ['muted', 'neutral'],
  ['secondary', 'neutral'],
  ['default', 'neutral'],
  ['accent', 'accent'],
  ['primary', 'accent'],
  ['purple', 'accent'],
  ['violet', 'accent'],
  ['positive', 'positive'],
  ['green', 'positive'],
  ['success', 'positive'],
  ['ok', 'positive'],
  ['good', 'positive'],
  ['warning', 'warning'],
  ['warn', 'warning'],
  ['yellow', 'warning'],
  ['orange', 'warning'],
  ['amber', 'warning'],
  ['caution', 'warning'],
  ['negative', 'negative'],
  ['red', 'negative'],
  ['danger', 'negative'],
  ['error', 'negative'],
  ['fail', 'negative'],
  ['failure', 'negative'],
  ['critical', 'negative'],
  ['info', 'info'],
  ['blue', 'info'],
  ['information', 'info'],
]);

const MOCK_KIND_ALIASES = new Map([
  ['hstack', 'row'],
  ['col', 'column'],
  ['vstack', 'column'],
  ['stack', 'column'],
  ['box', 'column'],
  ['container', 'column'],
  ['div', 'column'],
  ['section', 'column'],
  ['group', 'column'],
  ['panel', 'card'],
  ['sheet', 'card'],
  ['modal', 'card'],
  ['dialog', 'card'],
  ['popup', 'card'],
  ['screen', 'card'],
  ['label', 'text'],
  ['heading', 'text'],
  ['h1', 'text'],
  ['h2', 'text'],
  ['h3', 'text'],
  ['paragraph', 'text'],
  ['p', 'text'],
  ['span', 'text'],
  ['caption', 'text'],
  ['text_field', 'input'],
  ['textfield', 'input'],
  ['field', 'input'],
  ['search', 'input'],
  ['search_bar', 'input'],
  ['textarea', 'input'],
  ['checkbox', 'toggle'],
  ['switch', 'toggle'],
  ['radio', 'toggle'],
  ['slider', 'progress'],
  ['spinner', 'progress'],
  ['loader', 'progress'],
  ['img', 'image'],
  ['picture', 'image'],
  ['photo', 'image'],
  ['placeholder', 'image'],
  ['icon', 'avatar'],
  ['tag', 'badge'],
  ['chip', 'badge'],
  ['pill', 'badge'],
  ['link', 'button'],
  ['cta', 'button'],
  ['hr', 'divider'],
  ['separator', 'divider'],
  ['spacer', 'divider'],
  ['line', 'divider'],
  ['stat', 'metric'],
  ['kpi', 'metric'],
  ['counter', 'metric'],
  ['header', 'nav'],
  ['footer', 'nav'],
  ['toolbar', 'nav'],
  ['navbar', 'nav'],
  ['nav_bar', 'nav'],
  ['menu', 'nav'],
  ['menubar', 'nav'],
  ['menu_bar', 'nav'],
  ['sidebar', 'nav'],
  ['tabbar', 'nav'],
  ['tab_bar', 'nav'],
  ['tab', 'tabs'],
  ['segmented', 'tabs'],
  ['segmented_control', 'tabs'],
  ['ul', 'list'],
  ['ol', 'list'],
  ['window', 'frame'],
  ['app', 'frame'],
]);

const MOCK_DEVICE_ALIASES = new Map([
  ['ios', 'phone'],
  ['iphone', 'phone'],
  ['mobile', 'phone'],
  ['mac', 'window'],
  ['desktop', 'window'],
  ['app', 'window'],
  ['web', 'browser'],
  ['site', 'browser'],
  ['page', 'browser'],
  ['ipad', 'tablet'],
]);

/** Every field and alias, for folding a wrong-case key to its spelling. */
const KNOWN_KEYS = new Set(
  (
    'version title summary type bars x_label y_label x_labels y_labels values items nodes series area unit parts tiles ' +
    'columns rows events ticks edges layers data slices segments metrics cards kpis headers links connections tiers ' +
    'phases milestones labels categories x_axis y_axis units description subtitle name heading label value weight tone ' +
    'color colour text kind children device start end lane id detail group cells delta trend from to x y points change ' +
    'sparkline size date time at begin until through track swimlane key body content contents components includes ' +
    'source target bar part tile column row item event tick node edge layer child milestone slice'
  ).split(' '),
);

const spelled = (name) => name.trim().toLowerCase().replace(/[- ]/g, '_');

function toneAlias(name) {
  return TONE_ALIASES.get(spelled(name)) || null;
}

/** "Title" → "title", unless the right spelling is already taken. */
function foldKeyCase(o) {
  for (const key of Object.keys(o).sort()) {
    const lower = key.toLowerCase();
    if (lower !== key && KNOWN_KEYS.has(lower) && !has(o, lower)) {
      o[lower] = o[key];
      delete o[key];
    }
  }
}

function rename(o, from, to) {
  if (!has(o, from)) return;
  const value = o[from];
  delete o[from];
  if (has(o, to)) {
    if (!deepEqual(o[to], value)) throw invalid(`conflicting \`${from}\` and \`${to}\``);
  } else {
    o[to] = value;
  }
}

/** A number a model dressed up: commas, a currency sign, a percent sign or a k/M/B scale, dropped
 * only when what's left is still one number ("1,200" yes, "1,2,3" no). */
export function looseNumber(input) {
  let text = input.trim();
  if (/^[$€£¥]/.test(text)) text = text.slice(1).trimStart();
  if (text.endsWith('%')) text = text.slice(0, -1).trimEnd();
  let scale = 1;
  // Lowercase k and uppercase M/B only: "m" and "b" mean too many other things.
  const last = text[text.length - 1];
  if ((last === 'k' || last === 'K' || last === 'M' || last === 'B') && text.length > 1) {
    scale = last === 'M' ? 1e6 : last === 'B' ? 1e9 : 1e3;
    text = text.slice(0, -1).trimEnd();
  }
  if (text.includes(',')) {
    const parts = text.split('.');
    if (parts.length > 2 || (parts.length === 2 && !/^\d*$/.test(parts[1]))) return null;
    const [int, frac = ''] = parts;
    const groups = (int.startsWith('-') ? int.slice(1) : int).split(',');
    const head = groups.shift();
    if (!/^\d{1,3}$/.test(head) || !groups.every((g) => /^\d{3}$/.test(g))) return null;
    text = `${text.startsWith('-') ? '-' : ''}${head}${groups.join('')}${frac ? `.${frac}` : ''}`;
  }
  if (!/^-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?$/.test(text)) return null;
  const n = Number(text) * scale;
  return Number.isFinite(n) ? n : null;
}

/** Endpoints that already carry an order: ISO dates and years (0), months (1), weekdays (2),
 * quarters (3). Endpoints from different vocabularies don't sort together. */
export function timeRank(input) {
  const MONTHS = ['january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september', 'october', 'november', 'december'];
  const WEEKDAYS = ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'];
  const int = (s) => (/^\d{1,18}$/.test(s) ? Number(s) : null);
  const text = input.trim();
  if (text.length >= 4 && /^[\d-]+$/.test(text) && !text.includes('--')) {
    const [year, m, d, ...rest] = text.split('-');
    const month = m === undefined ? 0 : int(m);
    const day = d === undefined ? 0 : int(d);
    if (month === null || day === null) return null;
    if (year.length === 4 && rest.length === 0 && month <= 12 && day <= 31) return [0, int(year) * 400 + month * 32 + day];
    return null;
  }
  const tokens = text.toLowerCase().split(/[^a-z0-9]/).filter(Boolean);
  const year = tokens.map((t) => (t.length === 4 ? int(t) : null)).find((y) => y !== null) ?? null;
  for (const t of tokens) {
    const month = MONTHS.findIndex((m) => t.length >= 3 && m.startsWith(t));
    if (month >= 0) return [1, (year ?? 0) * 100 + month];
  }
  if (tokens.length === 1) {
    const day = WEEKDAYS.findIndex((d) => tokens[0].length >= 3 && d.startsWith(tokens[0]));
    if (day >= 0) return [2, day];
  }
  const q = /^q(\d+)$/.exec(tokens[0] || '');
  if (q && Number(q[1]) >= 1 && Number(q[1]) <= 4 && tokens.slice(1).every((t) => int(t) !== null)) return [3, (year ?? 0) * 10 + Number(q[1])];
  return null;
}

/** `value` in `shape` where that's meaningful; anything else as it was (and an error later). */
function coerce(value, shape) {
  switch (shape) {
    case TEXT:
      if (typeof value === 'number') return String(value);
      if (Array.isArray(value)) {
        // Only strings and numbers: flattening objects would hide a schema error.
        const items = value.map((v) => (typeof v === 'number' ? String(v) : v));
        return items.every((v) => typeof v === 'string') ? items.join(' ') : items;
      }
      return value;
    case NUMBER:
      if (typeof value === 'string') {
        const n = looseNumber(value);
        return n === null ? value : n;
      }
      return value;
    case TEXTS:
    case NUMBERS:
    case OBJECTS: {
      const single = shape === OBJECTS ? isObject(value) : typeof value === 'string' || typeof value === 'number';
      const list = single ? [value] : value;
      if (!Array.isArray(list)) return list;
      if (shape === TEXTS) return list.map((v) => (typeof v === 'number' ? String(v) : v));
      if (shape === NUMBERS) return list.map((v) => coerce(v, NUMBER));
      return list;
    }
    case MATRIX:
      return Array.isArray(value) ? value.map((row) => coerce(row, NUMBERS)) : value;
    default:
      return value;
  }
}

function fields(o, rules) {
  for (const [from, to] of PLURALS) if (rules.some(([field]) => field === to)) rename(o, from, to);
  for (const [field, shape] of rules) if (has(o, field)) o[field] = coerce(o[field], shape);
}

function normalizeTone(o) {
  for (const alias of ['color', 'colour']) {
    if (typeof o[alias] === 'string' && toneAlias(o[alias])) rename(o, alias, 'tone');
  }
  if (typeof o.tone === 'string') {
    const tone = toneAlias(o.tone);
    if (!tone) throw invalid(`\`tone\` must be one of neutral, accent, positive, warning, negative, info (got \`${o.tone}\`)`);
    o.tone = tone;
  }
}

/** A mockup node's `kind` and `device` in the schema's spellings, aliased or inferred. */
function normalizeMockNode(o) {
  if (typeof o.kind === 'string') {
    let name = spelled(o.kind);
    name = MOCK_KIND_ALIASES.get(name) || name;
    if (!MOCK_KINDS.includes(name)) throw invalid(`\`${o.kind}\` isn’t a mockup kind`);
    o.kind = name;
  } else if (o.kind === undefined || o.kind === null) {
    if (has(o, 'device')) o.kind = 'frame';
    else if (has(o, 'children')) o.kind = 'column';
    else if (has(o, 'text')) o.kind = 'text';
    else throw invalid('a mockup node needs a `kind`');
  }
  if (typeof o.device === 'string') {
    let name = spelled(o.device);
    name = MOCK_DEVICE_ALIASES.get(name) || name;
    if (!MOCK_DEVICES.includes(name)) throw invalid(`\`${o.device}\` isn’t a mockup device`);
    o.device = name;
  }
}

/** The renaming half of a mark's normalizing: keys, synonyms, tone names, mockup kinds. */
function shapeMark(o, kind, field) {
  foldKeyCase(o);
  for (const [k, f, from, to] of MARK_ALIASES) {
    const applies = (k === '*' ? kind !== 'mockup' : k === kind) && (f === '*' || f === field);
    if (applies) rename(o, from, to);
  }
  normalizeTone(o);
  if (kind === 'mockup') normalizeMockNode(o);
}

function normalizeMark(mark, kind, field, ticks) {
  if (!isObject(mark)) return;
  shapeMark(mark, kind, field);
  const rules = [['label', TEXT]];
  switch (kind) {
    case 'bar':
    case 'donut':
      rules.push(['value', NUMBER]);
      break;
    case 'treemap':
      rules.push(['weight', NUMBER]);
      break;
    case 'line':
      rules.push(['values', NUMBERS]);
      break;
    case 'stats':
      rules.push(['value', TEXT], ['delta', TEXT], ['trend', NUMBERS]);
      break;
    case 'timeline':
      rules.push(['lane', TEXT]);
      for (const key of ['start', 'end']) {
        if (!has(mark, key)) continue;
        if (!ticks) mark[key] = coerce(mark[key], NUMBER);
        else if (typeof mark[key] === 'number' && ticks.includes(String(mark[key]))) mark[key] = String(mark[key]);
      }
      break;
    case 'flow':
      rules.push(['detail', TEXT], ['group', TEXT]);
      if (field === 'nodes' && !has(mark, 'label') && typeof mark.id === 'string') mark.label = mark.id;
      break;
    case 'layers':
      rules.push(['detail', TEXT], ['items', TEXTS]);
      break;
    case 'mockup':
      rules.push(['text', TEXT], ['value', TEXT], ['children', OBJECTS]);
      break;
  }
  fields(mark, rules);
  if (kind === 'mockup') {
    // A toggle's value written as a boolean is its on/off state.
    if (mark.kind === 'toggle' && typeof mark.value === 'boolean') mark.value = mark.value ? 'on' : 'off';
    if (Array.isArray(mark.children)) for (const child of mark.children) normalizeMark(child, kind, 'children', null);
  }
}

/** Fold what a model wrote toward the schema: no data is invented, truncated, sorted, split or
 * dropped, and what stays ambiguous stays an error. */
function normalize(o) {
  if (!isObject(o)) return;
  foldKeyCase(o);
  // A null reads as "not given": the defaults and inference below apply.
  for (const key of ['version', 'type', 'title', 'summary']) if (o[key] === null) delete o[key];
  // A type is inferable only from a unique, type-specific collection. Never guess an axis.
  if (!has(o, 'type')) {
    const candidates = [
      ['bar', 'bars'],
      ['line', 'series'],
      ['donut', 'parts'],
      ['stats', 'tiles'],
      ['table', 'rows'],
      ['heatmap', 'y_labels'],
      ['treemap', 'items'],
      ['timeline', 'events'],
      ['layers', 'layers'],
    ].filter(([, field]) => has(o, field) || PLURALS.some(([from, to]) => to === field && has(o, from)));
    if (candidates.length === 1) o.type = candidates[0][0];
    if (!has(o, 'type')) {
      // A field alias only one type uses ("links": a flow's edges) gives it away; "data" doesn't.
      const kinds = [
        ...new Set(
          FIELD_ALIASES.filter(([, from]) => has(o, from) && FIELD_ALIASES.filter(([, other]) => other === from).length === 1).map(([k]) => k),
        ),
      ];
      if (kinds.length === 1) o.type = kinds[0];
    }
  }
  if (has(o, 'type')) o.type = coerce(o.type, TEXT);
  if (typeof o.type === 'string') {
    const name = spelled(o.type);
    o.type = TYPE_ALIASES.get(name) || name;
    if (name === 'area') {
      if (has(o, 'area') && o.area !== true) throw invalid('type `area` conflicts with `area`');
      o.area = true;
    }
  }
  const kind = typeof o.type === 'string' ? o.type : '';
  if (!TYPES.includes(kind)) return;
  if (!has(o, 'version')) o.version = VERSION;
  rename(o, 'name', 'title');
  rename(o, 'heading', 'title');
  rename(o, 'description', 'summary');
  rename(o, 'subtitle', 'summary');
  for (const [k, from, to] of FIELD_ALIASES) if (k === kind) rename(o, from, to);
  fields(o, [
    ['version', NUMBER],
    ['title', TEXT],
    ['summary', TEXT],
    ['unit', TEXT],
  ]);
  // Reuse the metadata given, never made-up titles.
  if (!has(o, 'summary') && typeof o.title === 'string') o.summary = o.title;
  if (!has(o, 'title') && typeof o.summary === 'string') o.title = o.summary;
  const rules = COLLECTIONS.filter(([k]) => k === kind).map(([, field, shape]) => [field, shape]);
  if (kind === 'bar') rules.push(['x_label', TEXT], ['y_label', TEXT]);
  else if (kind === 'line') rules.push(['y_label', TEXT]);
  else if (kind === 'table') rename(o, 'row', 'rows');
  else if (kind === 'timeline') {
    rename(o, 'tick', 'ticks');
    // A string isn't a set of ticks. It's a numeric axis's unit only when every endpoint given
    // is a number; otherwise the ambiguity stays an error.
    if (typeof o.ticks === 'string') {
      const events = has(o, 'events') ? o.events : o.event;
      const list = Array.isArray(events) ? events : isObject(events) ? [events] : [];
      const numeric =
        list.length > 0 &&
        list.every((e) => has(e, 'start') && ['start', 'end'].filter((k) => has(e, k)).every((k) => typeof coerce(e[k], NUMBER) === 'number'));
      if (!numeric) throw invalid('`ticks` must be an array of axis labels; use `unit` for a numeric axis unit');
      const unit = o.ticks;
      delete o.ticks;
      if (has(o, 'unit') && !deepEqual(o.unit, unit)) throw invalid('conflicting `ticks` and `unit`');
      o.unit = unit;
    }
  }
  fields(o, rules);
  // Mark key aliases ("from" → "start") settle before ticks are derived.
  for (const [field, shape] of rules) {
    if (shape !== OBJECTS || !Array.isArray(o[field])) continue;
    for (const mark of o[field]) if (isObject(mark)) shapeMark(mark, kind, field);
  }
  if (kind === 'heatmap' && Array.isArray(o.values) && Array.isArray(o.x_labels) && Array.isArray(o.y_labels)) {
    // One row per y label. Rows that run one per x label are transposed, unless they could
    // already be right (a square grid stays put).
    const matrix = o.values.map((row) => (Array.isArray(row) ? row.slice() : []));
    const x = o.x_labels.length;
    const y = o.y_labels.length;
    const fits = (m, w, h) => m.length === h && m.every((row) => row.length === w);
    if (fits(matrix, y, x) && !fits(matrix, x, y)) {
      const flipped = Array.from({ length: y }, () => []);
      for (const row of matrix) row.forEach((cell, c) => flipped[c].push(cell));
      o.values = flipped;
    } else if (!fits(matrix, x, y) && x > 1 && matrix.length === x * y && matrix.every((row) => row.length === 1)) {
      // A flat row-major list chunked to fill the grid.
      const flat = matrix.map((row) => row[0]);
      o.values = Array.from({ length: y }, (_, r) => flat.slice(r * x, r * x + x));
    }
  }
  if (
    kind === 'timeline' &&
    !has(o, 'ticks') &&
    Array.isArray(o.events) &&
    o.events.length > 0 &&
    o.events.every((e) => typeof e?.start === 'string' && (e.end === undefined || e.end === null || typeof e.end === 'string'))
  ) {
    // Every endpoint a string and no ticks: the endpoints are the axis, unless every one is a
    // dressed-up number. Dates, months, weekdays or quarters sort into their order; anything
    // else keeps the order of first appearance.
    let ordered = [];
    for (const e of o.events) {
      for (const key of ['start', 'end']) if (typeof e[key] === 'string' && !ordered.includes(e[key])) ordered.push(e[key]);
    }
    if (!ordered.every((p) => looseNumber(p) !== null)) {
      const ranks = ordered.map(timeRank);
      if (ranks.every(Boolean) && ranks.every((r) => r[0] === ranks[0][0])) {
        ordered = ranks
          .map((r, i) => [r[1], ordered[i]])
          .sort((a, b) => a[0] - b[0])
          .map((r) => r[1]);
      }
      o.ticks = ordered;
    }
  }
  const ticks = Array.isArray(o.ticks) ? o.ticks : null;
  for (const [field, shape] of rules) {
    if (shape !== OBJECTS || !Array.isArray(o[field])) continue;
    for (const mark of o[field]) normalizeMark(mark, kind, field, ticks);
  }
  // Toned table rows get mark aliases and colour words too.
  if (kind === 'table' && Array.isArray(o.rows)) for (const row of o.rows) normalizeMark(row, 'table', 'rows', null);
}

function rejectUnknownTopLevelFields(o) {
  if (!isObject(o)) throw jsonError('top level must be an object');
  if (typeof o.type !== 'string') throw jsonError('missing or non-string `type`');
  const allowed = TOP_FIELDS[o.type];
  if (!allowed) throw jsonError(`unsupported visualization type \`${o.type}\``);
  const field = Object.keys(o)
    .sort()
    .find((k) => !allowed.includes(k));
  if (field !== undefined) {
    const takes = allowed
      .slice(4)
      .map((f) => `\`${f}\``)
      .join(', ');
    throw jsonError(`unknown field \`${field}\` (a ${o.type} takes ${takes})`);
  }
}

// ---------------------------------------------------------------------------------------------
// Reading the schema's types, strictly, failing as serde would

const typeError = (v, expected) => jsonError(`invalid type: ${describe(v)}, expected ${expected}`);

function str(v) {
  if (typeof v !== 'string') throw typeError(v, 'a string');
  return v;
}

function num(v) {
  if (typeof v !== 'number') throw typeError(v, 'f64');
  return v;
}

function list(v, read) {
  if (!Array.isArray(v)) throw typeError(v, 'a sequence');
  return v.map(read);
}

const optional = (o, key, read) => (o[key] === undefined || o[key] === null ? undefined : read(o[key]));

function variant(v, names) {
  if (typeof v !== 'string') throw typeError(v, 'string or map');
  if (!names.includes(v)) throw jsonError(`unknown variant \`${v}\`, expected ${expectedList(names)}`);
  return v;
}

/** An object's members, refusing unknown ones; then each field's reader, `?` marking optional. */
function struct(v, name, spec) {
  if (!isObject(v)) throw typeError(v, `struct ${name}`);
  const names = Object.keys(spec).map((k) => k.replace(/\?$/, ''));
  for (const key of Object.keys(v).sort()) {
    if (!names.includes(key)) throw jsonError(`unknown field \`${key}\`, expected ${expectedList(names)}`);
  }
  const out = {};
  for (const [field, read] of Object.entries(spec)) {
    const opt = field.endsWith('?');
    const key = opt ? field.slice(0, -1) : field;
    if (!has(v, key)) {
      if (!opt) throw jsonError(`missing field \`${key}\``);
      continue;
    }
    const value = opt ? optional(v, key, read) : read(v[key]);
    if (value !== undefined) out[key] = value;
  }
  return out;
}

const tone = (v) => variant(v, TONES);

function cell(v) {
  if (typeof v === 'number' || typeof v === 'string') return v;
  throw jsonError('data did not match any variant of untagged enum TableCell');
}

function row(v) {
  if (Array.isArray(v) && v.every((c) => typeof c === 'number' || typeof c === 'string')) return { cells: v.slice() };
  if (isObject(v)) {
    try {
      return struct(v, 'TonedRow', { cells: (c) => list(c, cell), 'tone?': tone });
    } catch {
      // Neither shape.
    }
  }
  throw jsonError('data did not match any variant of untagged enum TableRow');
}

function timePoint(v) {
  if (typeof v === 'number' || typeof v === 'string') return v;
  throw jsonError('data did not match any variant of untagged enum TimePoint');
}

function mockNode(v) {
  const node = struct(v, 'MockNode', {
    kind: (k) => variant(k, MOCK_KINDS),
    'text?': str,
    'value?': str,
    'tone?': tone,
    'device?': (d) => variant(d, MOCK_DEVICES),
    'children?': (c) => list(c, mockNode),
  });
  node.children ||= [];
  return node;
}

const mark = (name, valueKey) => (v) => struct(v, name, { label: str, [valueKey]: num, 'tone?': tone });

function content(o) {
  const req = (key, read) => {
    if (!has(o, key)) throw jsonError(`missing field \`${key}\``);
    return read(o[key]);
  };
  const opt = (key, read) => optional(o, key, read);
  switch (o.type) {
    case 'bar':
      return { bars: req('bars', (v) => list(v, mark('BarMark', 'value'))), x_label: opt('x_label', str), y_label: opt('y_label', str) };
    case 'heatmap':
      return {
        x_labels: req('x_labels', (v) => list(v, str)),
        y_labels: req('y_labels', (v) => list(v, str)),
        values: req('values', (v) => list(v, (r) => list(r, num))),
      };
    case 'treemap':
      return { items: req('items', (v) => list(v, mark('TreemapItem', 'weight'))) };
    case 'mockup':
      return { nodes: req('nodes', (v) => list(v, mockNode)) };
    case 'line': {
      if (has(o, 'area') && typeof o.area !== 'boolean') throw typeError(o.area, 'a boolean');
      return {
        x_labels: req('x_labels', (v) => list(v, str)),
        series: req('series', (v) => list(v, (s) => struct(s, 'LineSeries', { label: str, values: (x) => list(x, num), 'tone?': tone }))),
        area: o.area === true,
        y_label: opt('y_label', str),
        unit: opt('unit', str),
      };
    }
    case 'donut':
      return { parts: req('parts', (v) => list(v, mark('DonutPart', 'value'))), unit: opt('unit', str) };
    case 'stats':
      return {
        tiles: req('tiles', (v) =>
          list(v, (t) => struct(t, 'StatTile', { label: str, value: str, 'delta?': str, 'trend?': (x) => list(x, num), 'tone?': tone })),
        ),
      };
    case 'table':
      return { columns: req('columns', (v) => list(v, str)), rows: req('rows', (v) => list(v, row)) };
    case 'timeline':
      return {
        events: req('events', (v) =>
          list(v, (e) => struct(e, 'TimelineEvent', { label: str, start: timePoint, 'end?': timePoint, 'lane?': str, 'tone?': tone })),
        ),
        ticks: opt('ticks', (v) => list(v, str)),
        unit: opt('unit', str),
      };
    case 'flow': {
      if (has(o, 'edges') && !Array.isArray(o.edges)) throw typeError(o.edges, 'a sequence');
      return {
        nodes: req('nodes', (v) => list(v, (n) => struct(n, 'FlowNode', { id: str, label: str, 'detail?': str, 'tone?': tone, 'group?': str }))),
        edges: (o.edges || []).map((e) => struct(e, 'FlowEdge', { from: str, to: str, 'label?': str })),
      };
    }
    default:
      return {
        layers: req('layers', (v) =>
          list(v, (l) => {
            const layer = struct(l, 'Layer', { label: str, 'detail?': str, 'items?': (x) => list(x, str), 'tone?': tone });
            layer.items ||= [];
            return layer;
          }),
        ),
      };
  }
}

function deserialize(o) {
  if (!has(o, 'version')) throw jsonError('missing field `version`');
  const version = o.version;
  if (typeof version !== 'number' || !Number.isInteger(version)) throw typeError(version, 'u8');
  if (version < 0 || version > 255) throw jsonError(`invalid value: integer \`${version}\`, expected u8`);
  if (!has(o, 'title')) throw jsonError('missing field `title`');
  if (!has(o, 'summary')) throw jsonError('missing field `summary`');
  const viz = { version, title: str(o.title), summary: str(o.summary), type: o.type, ...content(o) };
  for (const key of Object.keys(viz)) if (viz[key] === undefined) delete viz[key];
  return viz;
}

// ---------------------------------------------------------------------------------------------
// Validation: the limits and shapes every renderer relies on

function textField(field, value, required) {
  const length = chars(value);
  if (required && value.trim() === '') throw invalid(`${field} must be nonempty`);
  if (length > MAX_TEXT_CHARS) throw invalid(`${field} is ${length} characters; maximum is ${MAX_TEXT_CHARS}`);
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(value)) throw invalid(`${field} contains a control character`);
}

const optionalText = (field, value) => value !== undefined && textField(field, value, true);

function short(field, value) {
  textField(field, value, true);
  const length = chars(value);
  if (length > MAX_SHORT_CHARS) throw invalid(`${field} is ${length} characters; maximum is ${MAX_SHORT_CHARS}`);
}

const shortOptional = (field, value) => value !== undefined && short(field, value);

function labels(field, values, brief) {
  values.forEach((v, i) => (brief ? short(`${field}[${i}]`, v) : textField(`${field}[${i}]`, v, true)));
}

function number(field, value) {
  if (!Number.isFinite(value) || Math.abs(value) > MAX_ABS_VALUE) throw invalid(`${field} must be finite and between -${MAX_ABS_VALUE} and ${MAX_ABS_VALUE}`);
}

function dataset(name, n) {
  if (n === 0) throw invalid(`${name} dataset must be nonempty`);
  if (n > MAX_MARKS) throw invalid(`${name} dataset has ${n} marks; maximum is ${MAX_MARKS}`);
}

function identifier(field, value) {
  short(field, value);
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw invalid(`${field} may use only letters, digits, \`-\` and \`_\``);
}

function timeAt(field, point, ticks) {
  if (typeof point === 'number') {
    if (ticks) throw invalid(`${field} must name a tick when \`ticks\` is set`);
    number(field, point);
    return point;
  }
  if (!ticks) throw invalid(`${field} is a string, so the timeline needs \`ticks\``);
  const i = ticks.indexOf(point);
  if (i < 0) throw invalid(`${field} \`${point}\` is not one of the ticks`);
  return i;
}

/** A progress node's `value` ("64", "64%") as a fraction, when it is one. */
export function progressFraction(value) {
  const text = value.trim().replace(/%+$/, '').trim();
  const n = Number(text);
  return text !== '' && Number.isFinite(n) && n >= 0 && n <= 100 ? n / 100 : null;
}

function validateNode(node, depth, inFrame, count, path) {
  if (depth > MAX_MOCK_DEPTH) throw invalid(`${path} exceeds maximum mockup depth ${MAX_MOCK_DEPTH}`);
  count.n += 1;
  if (count.n > MAX_MARKS) throw invalid(`mockup has more than ${MAX_MARKS} nodes`);
  optionalText(`${path}.text`, node.text);
  optionalText(`${path}.value`, node.value);
  if (!MOCK_CONTAINERS.includes(node.kind) && node.children.length) throw invalid(`${path} leaf node cannot have children`);
  if ((node.device !== undefined) !== (node.kind === 'frame')) throw invalid(`${path} \`device\` belongs on a frame, and every frame needs one`);
  const noValue = (what) => {
    if (node.value !== undefined) throw invalid(`${path} ${what} cannot have a value`);
  };
  const needs = (v, field) => {
    if (v === undefined || v.trim() === '') throw invalid(`${field} is required`);
  };
  switch (node.kind) {
    case 'row':
    case 'column':
      if (node.text !== undefined || node.value !== undefined) throw invalid(`${path} row/column cannot have text or value`);
      break;
    case 'card':
    case 'list':
    case 'nav':
      noValue('container');
      break;
    case 'frame':
      noValue('frame');
      if (inFrame) throw invalid(`${path} frames cannot nest`);
      break;
    case 'tabs':
      if (node.text !== undefined) throw invalid(`${path} tabs take their labels from text children`);
      if (!node.children.length || node.children.length > MAX_TABS) throw invalid(`${path} tabs need 1 to ${MAX_TABS} text children`);
      if (node.children.some((c) => c.kind !== 'text')) throw invalid(`${path} tabs children must be text nodes`);
      if (node.value !== undefined && !node.children.some((c) => c.text === node.value)) throw invalid(`${path}.value must name one of its tabs`);
      break;
    case 'text':
    case 'button':
    case 'badge':
    case 'input':
      needs(node.text, `${path}.text`);
      break;
    case 'metric':
      needs(node.text, `${path}.text`);
      needs(node.value, `${path}.value`);
      break;
    case 'toggle':
      needs(node.text, `${path}.text`);
      if (node.value !== undefined && node.value !== 'on' && node.value !== 'off') throw invalid(`${path}.value must be \`on\` or \`off\``);
      break;
    case 'progress':
      if (node.value === undefined || progressFraction(node.value) === null) throw invalid(`${path}.value must be a percentage from 0 to 100`);
      break;
    case 'avatar':
      needs(node.text, `${path}.text`);
      noValue('avatar');
      break;
    case 'image':
      if (node.value !== undefined && !IMAGE_SHAPES.includes(node.value)) throw invalid(`${path}.value must be one of ${IMAGE_SHAPES.join(', ')}`);
      break;
    case 'divider':
      if (node.text !== undefined || node.value !== undefined) throw invalid(`${path} divider cannot have text or value`);
      break;
  }
  node.children.forEach((child, i) => validateNode(child, depth + 1, inFrame || node.kind === 'frame', count, `${path}.children[${i}]`));
}

export function validate(viz) {
  if (viz.version !== VERSION) throw new VizError('version', null, { version: viz.version });
  textField('title', viz.title, true);
  textField('summary', viz.summary, true);
  switch (viz.type) {
    case 'bar':
      dataset('bar', viz.bars.length);
      optionalText('x_label', viz.x_label);
      optionalText('y_label', viz.y_label);
      viz.bars.forEach((b, i) => {
        textField(`bars[${i}].label`, b.label, true);
        number(`bars[${i}].value`, b.value);
      });
      break;
    case 'heatmap': {
      const { x_labels: xs, y_labels: ys, values } = viz;
      if (!xs.length || !ys.length) throw invalid('heatmap axes must be nonempty');
      if (xs.length * ys.length > MAX_MARKS) throw invalid(`heatmap has more than ${MAX_MARKS} cells`);
      if (values.length !== ys.length || values.some((r) => r.length !== xs.length)) {
        throw invalid('heatmap values must have one row per y label and one value per x label');
      }
      xs.forEach((l, i) => textField(`x_labels[${i}]`, l, true));
      ys.forEach((l, i) => textField(`y_labels[${i}]`, l, true));
      values.forEach((r, ri) => r.forEach((v, c) => number(`values[${ri}][${c}]`, v)));
      break;
    }
    case 'treemap':
      dataset('treemap', viz.items.length);
      viz.items.forEach((item, i) => {
        textField(`items[${i}].label`, item.label, true);
        number(`items[${i}].weight`, item.weight);
        if (item.weight <= 0) throw invalid(`items[${i}].weight must be positive`);
      });
      break;
    case 'mockup': {
      if (!viz.nodes.length) throw invalid('mockup nodes must be nonempty');
      const count = { n: 0 };
      viz.nodes.forEach((node, i) => validateNode(node, 1, false, count, `nodes[${i}]`));
      break;
    }
    case 'line':
      if (viz.x_labels.length < 2) throw invalid('line needs at least two x labels');
      dataset('line x_labels', viz.x_labels.length);
      labels('x_labels', viz.x_labels, false);
      optionalText('y_label', viz.y_label);
      shortOptional('unit', viz.unit);
      if (!viz.series.length || viz.series.length > MAX_SERIES) throw invalid(`line needs 1 to ${MAX_SERIES} series`);
      viz.series.forEach((s, i) => {
        textField(`series[${i}].label`, s.label, true);
        if (s.values.length !== viz.x_labels.length) throw invalid(`series[${i}] must have one value per x label`);
        s.values.forEach((v, p) => number(`series[${i}].values[${p}]`, v));
      });
      break;
    case 'donut':
      if (!viz.parts.length || viz.parts.length > MAX_PARTS) throw invalid(`donut needs 1 to ${MAX_PARTS} parts`);
      shortOptional('unit', viz.unit);
      viz.parts.forEach((p, i) => {
        textField(`parts[${i}].label`, p.label, true);
        number(`parts[${i}].value`, p.value);
        if (p.value <= 0) throw invalid(`parts[${i}].value must be positive`);
      });
      break;
    case 'stats':
      if (!viz.tiles.length || viz.tiles.length > MAX_TILES) throw invalid(`stats needs 1 to ${MAX_TILES} tiles`);
      viz.tiles.forEach((t, i) => {
        textField(`tiles[${i}].label`, t.label, true);
        short(`tiles[${i}].value`, t.value);
        shortOptional(`tiles[${i}].delta`, t.delta);
        if (t.trend) {
          if (t.trend.length < 2 || t.trend.length > MAX_TREND_POINTS) throw invalid(`tiles[${i}].trend needs 2 to ${MAX_TREND_POINTS} values`);
          t.trend.forEach((v, p) => number(`tiles[${i}].trend[${p}]`, v));
        }
      });
      break;
    case 'table': {
      const { columns, rows } = viz;
      if (!columns.length || columns.length > MAX_COLUMNS) throw invalid(`table needs 1 to ${MAX_COLUMNS} columns`);
      labels('columns', columns, false);
      if (!rows.length || rows.length > MAX_ROWS) throw invalid(`table needs 1 to ${MAX_ROWS} rows`);
      if (rows.length * columns.length > MAX_TABLE_CELLS) throw invalid(`table has more than ${MAX_TABLE_CELLS} cells`);
      rows.forEach((r, i) => {
        if (r.cells.length !== columns.length) throw invalid(`rows[${i}] must have one cell per column`);
        r.cells.forEach((c, ci) => (typeof c === 'number' ? number(`rows[${i}][${ci}]`, c) : textField(`rows[${i}][${ci}]`, c, false)));
      });
      break;
    }
    case 'timeline': {
      const { events, ticks } = viz;
      if (!events.length || events.length > MAX_EVENTS) throw invalid(`timeline needs 1 to ${MAX_EVENTS} events`);
      shortOptional('unit', viz.unit);
      if (ticks) {
        if (!ticks.length || ticks.length > MAX_MARKS) throw invalid(`timeline ticks need 1 to ${MAX_MARKS} labels`);
        labels('ticks', ticks, true);
        ticks.forEach((t, i) => {
          if (ticks.slice(0, i).includes(t)) throw invalid(`ticks[${i}] repeats \`${t}\``);
        });
      }
      const lanes = [];
      events.forEach((e, i) => {
        textField(`events[${i}].label`, e.label, true);
        if (e.lane !== undefined) {
          short(`events[${i}].lane`, e.lane);
          if (!lanes.includes(e.lane)) lanes.push(e.lane);
        }
        const start = timeAt(`events[${i}].start`, e.start, ticks);
        if (e.end !== undefined && timeAt(`events[${i}].end`, e.end, ticks) < start) throw invalid(`events[${i}] ends before it starts`);
      });
      if (lanes.length > MAX_LAYERS) throw invalid(`timeline has more than ${MAX_LAYERS} lanes`);
      break;
    }
    case 'flow': {
      const { nodes, edges } = viz;
      if (!nodes.length || nodes.length > MAX_FLOW_NODES) throw invalid(`flow needs 1 to ${MAX_FLOW_NODES} nodes`);
      if (edges.length > MAX_FLOW_EDGES) throw invalid(`flow has more than ${MAX_FLOW_EDGES} edges`);
      const groups = [];
      nodes.forEach((n, i) => {
        identifier(`nodes[${i}].id`, n.id);
        if (nodes.slice(0, i).some((m) => m.id === n.id)) throw invalid(`nodes[${i}].id \`${n.id}\` is not unique`);
        textField(`nodes[${i}].label`, n.label, true);
        optionalText(`nodes[${i}].detail`, n.detail);
        if (n.group !== undefined) {
          short(`nodes[${i}].group`, n.group);
          if (!groups.includes(n.group)) groups.push(n.group);
        }
      });
      if (groups.length > MAX_FLOW_GROUPS) throw invalid(`flow has more than ${MAX_FLOW_GROUPS} groups`);
      edges.forEach((e, i) => {
        for (const [end, id] of [
          ['from', e.from],
          ['to', e.to],
        ]) {
          if (!nodes.some((n) => n.id === id)) throw invalid(`edges[${i}].${end} names no node \`${id}\``);
        }
        if (e.from === e.to) throw invalid(`edges[${i}] connects \`${e.from}\` to itself`);
        if (edges.slice(0, i).some((o) => o.from === e.from && o.to === e.to)) throw invalid(`edges[${i}] repeats \`${e.from}\` -> \`${e.to}\``);
        shortOptional(`edges[${i}].label`, e.label);
      });
      break;
    }
    case 'layers':
      if (!viz.layers.length || viz.layers.length > MAX_LAYERS) throw invalid(`layers needs 1 to ${MAX_LAYERS} layers`);
      viz.layers.forEach((l, i) => {
        textField(`layers[${i}].label`, l.label, true);
        optionalText(`layers[${i}].detail`, l.detail);
        if (l.items.length > MAX_LAYER_ITEMS) throw invalid(`layers[${i}] has more than ${MAX_LAYER_ITEMS} items`);
        labels(`layers[${i}].items`, l.items, true);
      });
      break;
  }
}

/** Parse and validate a block's JSON (without its fence): the visualization, or a `VizError`. */
export function parse(payload) {
  const bytes = byteLength(payload);
  if (bytes > MAX_PAYLOAD_BYTES) throw new VizError('too_large', null, { bytes });
  let value;
  try {
    value = JSON.parse(tidy(payload));
  } catch (err) {
    throw jsonError(String(err.message || err).replace(/^JSON\.parse: /, ''));
  }
  normalize(value);
  rejectUnknownTopLevelFields(value);
  const viz = deserialize(value);
  validate(viz);
  return viz;
}

// ---------------------------------------------------------------------------------------------
// A block still being written

/** What can be read from a block's JSON as far as it has arrived: its type and title once each
 * is written in full, and the visualization as far as it's written in whole marks (the text cut
 * back to the last complete element of an open array, and closed). `complete`: the top-level
 * object has closed, so only the fence is missing. Marks still being written are left out rather
 * than guessed at, so a value never shows as 12 on its way to 128. */
export function parsePartial(payload) {
  const out = { kind: null, title: null, drawable: null, complete: false };
  const stack = [];
  let inString = false;
  let escaped = false;
  let stringStart = 0;
  let key = null;
  let end = null;
  for (let i = 0; i < payload.length; i++) {
    const c = payload[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (c === '\\') escaped = true;
      else if (c === '"') {
        inString = false;
        const open = stack[stack.length - 1];
        if (!open) break;
        const isValue = open.array || open.value;
        if (isValue) open.good = i + 1;
        if (stack.length === 1) {
          if (!isValue) key = [stringStart, i + 1];
          else if (key) {
            const name = payload.slice(key[0], key[1]);
            key = null;
            const text = () => {
              try {
                const v = JSON.parse(payload.slice(stringStart, i + 1));
                return typeof v === 'string' ? v : null;
              } catch {
                return null;
              }
            };
            if (name === '"type"') out.kind = text();
            else if (name === '"title"') out.title = text();
          }
        }
      }
      continue;
    }
    if (c === '"' && stack.length) {
      inString = true;
      stringStart = i;
    } else if (c === '{' || c === '[') {
      if (stack.length === 1) key = null;
      stack.push({ array: c === '[', good: i + 1, value: false });
    } else if ((c === '}' || c === ']') && stack.length) {
      stack.pop();
      const parent = stack[stack.length - 1];
      if (!parent) {
        end = i + 1;
        break;
      }
      parent.good = i + 1;
    } else if (c === ':') {
      if (stack.length) stack[stack.length - 1].value = true;
    } else if (c === ',') {
      if (stack.length) {
        stack[stack.length - 1].good = i;
        stack[stack.length - 1].value = false;
      }
      if (stack.length === 1) key = null;
    } else if (!stack.length && !/\s/.test(c)) {
      break;
    }
  }
  if (byteLength(payload) > MAX_PAYLOAD_BYTES) return out;
  if (end !== null) {
    out.complete = true;
    try {
      out.drawable = parse(payload.slice(0, end));
    } catch {
      // Closed but not drawable: the closing fence decides what it says.
    }
    return out;
  }
  // Where to cut: each open array, innermost first, then the top-level object itself.
  const cuts = [];
  for (let k = stack.length - 1; k >= 0 && cuts.length < 4; k--) if (k === 0 || stack[k].array) cuts.push(k);
  for (const k of cuts) {
    let text = payload.slice(0, stack[k].good);
    for (let j = k; j >= 0; j--) text += stack[j].array ? ']' : '}';
    try {
      out.drawable = parse(text);
      break;
    } catch {
      // Not yet.
    }
  }
  return out;
}

/** The visualization as JSON, its fields in the schema's order: what "Copy data as JSON" copies. */
export function toJson(viz) {
  return JSON.stringify(viz, (key, value) => (key === 'area' && value === false ? undefined : value), 2);
}

// ---------------------------------------------------------------------------------------------
// Numbers as the Mac writes them

const roundHalfAway = (x) => Math.sign(x) * Math.round(Math.abs(x));

export function group(s) {
  const sign = s.startsWith('-') ? '-' : '';
  const [int, frac] = (sign ? s.slice(1) : s).split('.');
  const grouped = int.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return frac !== undefined ? `${sign}${grouped}.${frac}` : `${sign}${grouped}`;
}

/** A number for reading: grouped thousands, compact from ten thousand, at most two decimals. */
export function formatNumber(value) {
  const a = Math.abs(value);
  const compact = (div, suffix) => {
    const v = value / div;
    return (roundHalfAway(v * 10) % 10 === 0 ? v.toFixed(0) : v.toFixed(1)) + suffix;
  };
  if (a >= 1e12) return compact(1e12, 'T');
  if (a >= 1e9) return compact(1e9, 'B');
  if (a >= 1e6) return compact(1e6, 'M');
  if (a >= 1e4) return compact(1e3, 'K');
  const decimals = Math.abs(value % 1) < 1e-9 ? 0 : Math.abs((value * 10) % 1) < 1e-9 ? 1 : 2;
  return group(value.toFixed(decimals)).replace(/^-(0(\.0+)?)$/, '$1');
}

/** A number exactly as given, thousands grouped: for tables, where figures are looked up. */
export function formatExact(value) {
  if (Math.abs(value % 1) < 1e-9 && Math.abs(value) < 1e15) return group(value.toFixed(0));
  const text = String(value);
  return text.includes('e') ? text : group(text);
}

/** A tick label: as many decimals as the step needs, and no more. */
export function formatTick(value, step) {
  if (Math.abs(value) >= 1e4) return formatNumber(value);
  let decimals = 6;
  for (let d = 0; d < 6; d++) {
    if (Math.abs((step * 10 ** d) % 1) < 1e-6) {
      decimals = d;
      break;
    }
  }
  return group(value.toFixed(decimals)).replace(/^-(0(\.0+)?)$/, '$1');
}

/** `text` with its unit: currency before, a percent sign tight after, anything else spaced. */
export function withUnit(text, unit) {
  if (!unit) return text;
  if (['$', '€', '£', '¥'].includes(unit)) return text.startsWith('-') ? `-${unit}${text.slice(1)}` : `${unit}${text}`;
  if (unit === '%') return `${text}%`;
  return `${text} ${unit}`;
}

/** A share of a whole: "25.4%". */
export function share(part, total, decimals = 1) {
  return `${((part / total) * 100).toFixed(decimals)}%`;
}

// ---------------------------------------------------------------------------------------------
// The table behind every chart: its Data view, and what the copy actions copy

const cellText = (c) => (typeof c === 'number' ? (c % 1 === 0 && Math.abs(c) < 1e15 ? c.toFixed(0) : String(c)) : c);

/** The values behind `viz` as `{ columns, rows }`, for every type with values (not a mockup). */
export function dataTable(viz) {
  const time = (t) => t;
  switch (viz.type) {
    case 'bar':
      return { columns: [viz.y_label || 'Label', viz.x_label || 'Value'], rows: viz.bars.map((b) => [b.label, b.value]) };
    case 'line':
      return { columns: ['', ...viz.series.map((s) => s.label)], rows: viz.x_labels.map((x, i) => [x, ...viz.series.map((s) => s.values[i])]) };
    case 'donut': {
      const total = viz.parts.reduce((t, p) => t + p.value, 0);
      return { columns: ['Part', 'Value', 'Share'], rows: viz.parts.map((p) => [p.label, p.value, share(p.value, total)]) };
    }
    case 'stats':
      return { columns: ['Figure', 'Value', 'Change'], rows: viz.tiles.map((t) => [t.label, t.value, t.delta || '']) };
    case 'table':
      return { columns: viz.columns, rows: viz.rows.map((r) => r.cells) };
    case 'heatmap':
      return { columns: ['', ...viz.x_labels], rows: viz.y_labels.map((y, i) => [y, ...viz.values[i]]) };
    case 'treemap': {
      const total = viz.items.reduce((t, x) => t + x.weight, 0);
      return { columns: ['Item', 'Weight', 'Share'], rows: viz.items.map((x) => [x.label, x.weight, share(x.weight, total)]) };
    }
    case 'timeline': {
      const lanes = viz.events.some((e) => e.lane !== undefined);
      return {
        columns: ['Event', ...(lanes ? ['Lane'] : []), 'Start', 'End'],
        rows: viz.events.map((e) => [e.label, ...(lanes ? [e.lane || ''] : []), time(e.start), e.end === undefined ? '—' : time(e.end)]),
      };
    }
    case 'flow': {
      const label = (id) => viz.nodes.find((n) => n.id === id)?.label || '';
      const grouped = viz.nodes.some((n) => n.group !== undefined);
      return {
        columns: ['Step', ...(grouped ? ['Group'] : []), 'Leads to', 'Detail'],
        rows: viz.nodes.map((n) => {
          const next = viz.edges
            .filter((e) => e.from === n.id)
            .map((e) => (e.label ? `${label(e.to)} (${e.label})` : label(e.to)))
            .join(', ');
          return [n.label, ...(grouped ? [n.group || ''] : []), next, n.detail || ''];
        }),
      };
    }
    case 'layers':
      return { columns: ['Layer', 'Detail', 'Contains'], rows: viz.layers.map((l) => [l.label, l.detail || '', l.items.join(', ')]) };
    default:
      return null;
  }
}

export function tableMarkdown({ columns, rows }) {
  const escape = (s) => s.replace(/\|/g, '\\|');
  const numeric = columns.map((_, c) => rows.every((r) => typeof r[c] === 'number'));
  let out = `| ${columns.map(escape).join(' | ')} |\n`;
  out += `| ${numeric.map((n) => (n ? '---:' : '---')).join(' | ')} |\n`;
  for (const r of rows) out += `| ${r.map((c) => escape(cellText(c))).join(' | ')} |\n`;
  return out;
}

export function tableCsv({ columns, rows }) {
  const field = (s) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  let out = `${columns.map(field).join(',')}\n`;
  for (const r of rows) out += `${r.map((c) => field(cellText(c))).join(',')}\n`;
  return out;
}

/** A text equivalent for screen readers: the values too, so nothing is told by colour alone. */
export function describeViz(viz) {
  let out = `Visualization: ${viz.title}. ${viz.summary}`;
  const push = (s) => {
    if (out.length < 4096) out += `; ${s}`;
  };
  if (viz.type === 'mockup') {
    const visit = (nodes) => {
      for (const n of nodes) {
        if (n.device) push(`${n.device} frame`);
        if (n.text) push(n.text);
        if (n.value) push(n.value);
        visit(n.children);
      }
    };
    visit(viz.nodes);
  } else if (viz.type === 'flow') {
    const label = (id) => viz.nodes.find((n) => n.id === id)?.label || '';
    for (const n of viz.nodes) push(`${n.label}${n.group ? ` (${n.group})` : ''}${n.detail ? `: ${n.detail}` : ''}`);
    for (const e of viz.edges) push(`${label(e.from)} to ${label(e.to)}${e.label ? ` (${e.label})` : ''}`);
  } else {
    const data = dataTable(viz);
    for (const r of data?.rows || []) {
      push(
        r
          .map((c, i) => {
            const value = typeof c === 'number' ? formatNumber(c) : c;
            return data.columns[i] ? `${data.columns[i]} ${value}` : value;
          })
          .join(', '),
      );
    }
  }
  return out.slice(0, 4096);
}

// ---------------------------------------------------------------------------------------------
// Geometry

const ULP = Number.EPSILON;

/** A linear axis rounded out to "nice" 1-2-5 steps of about `target` ticks. A flat range still
 * gets an axis: around zero when it is zero, else from zero to it. */
export function niceScale(a, b, target) {
  let lo = Math.min(a, b);
  let hi = Math.max(a, b);
  if (Math.abs(hi - lo) < ULP) {
    if (hi === 0) hi = 1;
    else if (hi > 0) lo = 0;
    else hi = 0;
  }
  const raw = (hi - lo) / Math.max(target, 1);
  let step = 1;
  if (Number.isFinite(raw) && raw > 0) {
    const magnitude = 10 ** Math.floor(Math.log10(raw));
    const n = raw / magnitude;
    step = (n < 1.5 ? 1 : n < 3 ? 2 : n < 7 ? 5 : 10) * magnitude;
  }
  const scale = { lo: Math.floor(lo / step) * step, hi: Math.ceil(hi / step) * step, step };
  const count = Math.min(Math.round((scale.hi - scale.lo) / step), 64);
  scale.ticks = Array.from({ length: count + 1 }, (_, i) => {
    const v = scale.lo + i * step;
    return Math.abs(v) < step * 1e-9 ? 0 : v;
  });
  scale.frac = (v) => (v - scale.lo) / Math.max(scale.hi - scale.lo, ULP);
  return scale;
}

/** A squarified treemap (Bruls, Huizing and van Wijk): `{ x, y, w, h }` per weight, of area in
 * proportion, filling `width` × `height` as close to square as the order allows. */
export function squarify(weights, width, height) {
  const out = weights.map(() => ({ x: 0, y: 0, w: 0, h: 0 }));
  const total = weights.filter((w) => w > 0).reduce((t, w) => t + w, 0);
  if (!(total > 0) || width <= 0 || height <= 0) return out;
  const scale = (width * height) / total;
  const order = weights
    .map((_, i) => i)
    .filter((i) => weights[i] > 0)
    .sort((a, b) => (weights[a] !== weights[b] ? weights[b] - weights[a] : a - b));
  const area = (i) => weights[i] * scale;
  const worst = (row, side) => {
    const sum = row.reduce((t, i) => t + area(i), 0);
    const areas = row.map(area);
    const lo = Math.min(...areas);
    const hi = Math.max(...areas);
    return Math.max((side * side * hi) / (sum * sum), (sum * sum) / (side * side * Math.max(lo, ULP)));
  };
  let rect = { x: 0, y: 0, w: width, h: height };
  const place = (row) => {
    const sum = row.reduce((t, i) => t + area(i), 0);
    if (rect.w >= rect.h) {
      // A column down the left edge.
      const thick = rect.h > 0 ? sum / rect.h : 0;
      let y = rect.y;
      for (const i of row) {
        const h = thick > 0 ? area(i) / thick : 0;
        out[i] = { x: rect.x, y, w: thick, h };
        y += h;
      }
      rect = { x: rect.x + thick, y: rect.y, w: Math.max(rect.w - thick, 0), h: rect.h };
    } else {
      // A row along the top edge.
      const thick = rect.w > 0 ? sum / rect.w : 0;
      let x = rect.x;
      for (const i of row) {
        const w = thick > 0 ? area(i) / thick : 0;
        out[i] = { x, y: rect.y, w, h: thick };
        x += w;
      }
      rect = { x: rect.x, y: rect.y + thick, w: rect.w, h: Math.max(rect.h - thick, 0) };
    }
  };
  let row = [];
  for (const i of order) {
    const side = Math.min(rect.w, rect.h);
    const grown = [...row, i];
    if (!row.length || worst(grown, side) <= worst(row, side)) row = grown;
    else {
      place(row);
      row = [i];
    }
  }
  if (row.length) place(row);
  return out;
}

/** A layered (Sugiyama-style) layout of a directed graph, ranks running top to bottom, as the
 * Mac's `flow_layout`: rank by longest path from the sources (cycles broken where a depth-first
 * walk meets them), order each rank by group then by its neighbours' barycentre, and place nodes
 * over their parents where the order allows. `x` is in slots (0 is the first slot's middle). */
export function flowLayout(n, edges, inGroups) {
  const back = edges.map(() => false);
  const state = new Array(n).fill(0);
  const out = Array.from({ length: n }, (_, v) =>
    edges
      .map((e, i) => [i, e])
      .filter(([, e]) => e[0] === v)
      .map(([i, e]) => [i, e[1]]),
  );
  for (let root = 0; root < n; root++) {
    if (state[root] !== 0) continue;
    const stack = [[root, 0]];
    state[root] = 1;
    while (stack.length) {
      const top = stack[stack.length - 1];
      const [v, next] = top;
      if (next < out[v].length) {
        top[1] += 1;
        const [edge, w] = out[v][next];
        if (state[w] === 0) {
          state[w] = 1;
          stack.push([w, 0]);
        } else if (state[w] === 1) back[edge] = true;
      } else {
        state[v] = 2;
        stack.pop();
      }
    }
  }
  let forward = edges.filter((_, i) => !back[i]);
  // Longest path: relax in topological order (Kahn's).
  const indegree = new Array(n).fill(0);
  for (const [, to] of forward) indegree[to] += 1;
  const queue = [];
  for (let v = 0; v < n; v++) if (indegree[v] === 0) queue.push(v);
  const rank = new Array(n).fill(0);
  for (let head = 0; head < queue.length; head++) {
    const v = queue[head];
    for (const [from, to] of forward) {
      if (from !== v) continue;
      rank[to] = Math.max(rank[to], rank[from] + 1);
      indegree[to] -= 1;
      if (indegree[to] === 0) queue.push(to);
    }
  }
  const ranks = (n ? Math.max(...rank) : -1) + 1;
  // An edge across several ranks passes through a virtual node in each rank between.
  const groups = Array.from({ length: n }, (_, v) => (v < inGroups.length ? inGroups[v] : null));
  const chains = edges.map(() => []);
  const through = [];
  edges.forEach(([from, to], e) => {
    if (back[e]) return;
    let prev = from;
    for (let r = rank[from] + 1; r < rank[to]; r++) {
      const v = rank.length;
      rank.push(r);
      groups.push(groups[from]);
      chains[e].push(v);
      through.push([prev, v]);
      prev = v;
    }
    through.push([prev, to]);
  });
  forward = through;
  const all = rank.length;
  const rows = Array.from({ length: Math.max(ranks, 0) }, () => []);
  for (let v = 0; v < all; v++) rows[rank[v]].push(v);
  const groupKey = (v) => (groups[v] === null || groups[v] === undefined ? Infinity : groups[v]);
  for (let r = 0; r < rows.length; r++) {
    rows[r] = rows[r]
      .map((v, i) => [v, i])
      .sort((a, b) => groupKey(a[0]) - groupKey(b[0]) || a[1] - b[1])
      .map(([v]) => v);
  }
  const position = () => {
    const pos = new Array(all).fill(0);
    for (const row of rows) row.forEach((v, i) => (pos[v] = i - (row.length - 1) / 2));
    return pos;
  };
  // A few sweeps down and up: by group, then the mean position of the neighbours just placed.
  for (let sweep = 0; sweep < 4; sweep++) {
    const pos = position();
    const down = sweep % 2 === 0;
    const order = [];
    if (down) for (let r = 1; r < Math.max(ranks, 1); r++) order.push(r);
    else for (let r = Math.max(ranks - 1, 0) - 1; r >= 0; r--) order.push(r);
    for (const r of order) {
      const keyed = rows[r].map((v, i) => {
        const near = [];
        for (const [a, b] of forward) {
          if (down && b === v) near.push(pos[a]);
          if (!down && a === v) near.push(pos[b]);
        }
        const bary = near.length ? near.reduce((t, x) => t + x, 0) / near.length : pos[v];
        return [groupKey(v), bary, i];
      });
      keyed.sort((a, b) => (a[0] !== b[0] ? (a[0] < b[0] ? -1 : 1) : a[1] !== b[1] ? a[1] - b[1] : a[2] - b[2]));
      rows[r] = keyed.map((k) => rows[r][k[2]]);
      rows[r].forEach((v, i) => (pos[v] = i - (rows[r].length - 1) / 2));
    }
  }
  // Coordinates in slots: each rank over the mean of its parents, a slot apart.
  const slots = Math.max(Math.max(0, ...rows.map((r) => r.length)), 1);
  const maxX = slots - 1;
  const x = new Array(all).fill(0);
  const settle = (want) => {
    const at = want.slice();
    for (let i = 0; i < at.length; i++) {
      at[i] = Math.min(Math.max(at[i], 0), maxX);
      if (i > 0) at[i] = Math.max(at[i], at[i - 1] + 1);
    }
    for (let i = at.length - 1; i >= 0; i--) at[i] = Math.min(at[i], i + 1 < at.length ? at[i + 1] - 1 : maxX);
    return at;
  };
  rows.forEach((row, r) => {
    const want = row.map((v, i) => {
      const parents = forward.filter(([a, b]) => b === v && rank[a] < r).map(([a]) => x[a]);
      return parents.length ? parents.reduce((t, p) => t + p, 0) / parents.length : i + (slots - row.length) / 2;
    });
    settle(want).forEach((at, i) => (x[row[i]] = at));
  });
  // Then once back up: a parent moves toward the middle of its children where it can.
  for (const row of [...rows].reverse()) {
    const want = row.map((v) => {
      const kids = forward.filter(([a]) => a === v).map(([, b]) => x[b]);
      return kids.length ? (x[v] + kids.reduce((t, k) => t + k, 0) / kids.length) / 2 : x[v];
    });
    settle(want).forEach((at, i) => (x[row[i]] = at));
  }
  return {
    rank: rank.slice(0, n),
    x: x.slice(0, n),
    slots,
    ranks: Math.max(ranks, 1),
    back,
    routes: chains.map((chain) => chain.map((v) => [rank[v], x[v]])),
  };
}

/** Rows for spans `[start, end]` so none overlap within a row: each takes the first row free by
 * its start; `gap` keeps touching spans apart. */
export function pack(spans, gap) {
  const order = spans.map((_, i) => i).sort((a, b) => (spans[a][0] !== spans[b][0] ? spans[a][0] - spans[b][0] : a - b));
  const ends = [];
  const row = new Array(spans.length).fill(0);
  for (const i of order) {
    const [start, end] = spans[i];
    const r = ends.findIndex((e) => e + gap <= start);
    if (r >= 0) {
      ends[r] = end;
      row[i] = r;
    } else {
      row[i] = ends.length;
      ends.push(end);
    }
  }
  return row;
}

/** Up to two initials from a name. */
export function initials(name) {
  return name
    .split(/\s+/)
    .map((w) => [...w].find((c) => /[\p{L}\p{N}]/u.test(c)))
    .filter(Boolean)
    .slice(0, 2)
    .join('')
    .toUpperCase();
}
