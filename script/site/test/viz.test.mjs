// How the web remote reads `trek-viz` blocks and math, in Node: the same blocks draw or are
// refused as on the Mac (the cases are trek_core::visualization's tests), the same formulas are
// found (trek-math's detect.rs tests), and marked hands every formula over whole.
//
//   node --test script/site/test/

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_MARKS,
  MAX_MOCK_DEPTH,
  MAX_PAYLOAD_BYTES,
  MAX_TEXT_CHARS,
  TYPES,
  VizError,
  dataTable,
  flowLayout,
  formatExact,
  formatNumber,
  formatTick,
  looseNumber,
  niceScale,
  pack,
  parse,
  parsePartial,
  progressFraction,
  squarify,
  tableCsv,
  tableMarkdown,
  unwrapInnerFence,
  withUnit,
} from '../../../site/remote/js/features/viz-spec.js';
import { hideMath, markdownMath, mathRegions, regionTex, restoreMath } from '../../../site/remote/js/features/viz-math.js';
import { Marked } from '../../../site/remote/vendor/marked.mjs';
import { colour, lookStyle } from '../../../site/remote/js/features/look-mac.js';

const common = (kind, body) => `{"version":1,"title":"Demo","summary":"Useful summary","type":"${kind}",${body}}`;
const ok = (kind, body) => parse(common(kind, body));
/** The whole error, as the Mac's `to_string()` reads. */
function err(kind, body) {
  try {
    parse(common(kind, body));
  } catch (e) {
    assert.ok(e instanceof VizError, `${kind}: ${body}: ${e}`);
    return e.message;
  }
  assert.fail(`${kind}: ${body} parsed`);
}
const reason = (payload) => {
  try {
    parse(payload);
  } catch (e) {
    return e.reason;
  }
  assert.fail('parsed');
};

test('every type parses, and its wire name is its own', () => {
  const bodies = [
    ['bar', '"bars":[{"label":"a","value":1}]'],
    ['line', '"x_labels":["a","b"],"series":[{"label":"s","values":[1,2]}]'],
    ['donut', '"parts":[{"label":"a","value":1}]'],
    ['stats', '"tiles":[{"label":"a","value":"1"}]'],
    ['table', '"columns":["a"],"rows":[["x"]]'],
    ['heatmap', '"x_labels":["a"],"y_labels":["b"],"values":[[1]]'],
    ['treemap', '"items":[{"label":"a","weight":1}]'],
    ['timeline', '"events":[{"label":"a","start":1}]'],
    ['flow', '"nodes":[{"id":"a","label":"A"}]'],
    ['layers', '"layers":[{"label":"a"}]'],
    ['mockup', '"nodes":[{"kind":"divider"}]'],
  ];
  assert.deepEqual(
    bodies.map(([k]) => k),
    TYPES,
  );
  for (const [kind, body] of bodies) {
    assert.equal(ok(kind, body).type, kind);
    assert.match(err(kind, `${body},"html":"<b>"`), /unknown field `html`/, kind);
  }
});

test('bad shapes, values and fields are refused', () => {
  assert.match(err('heatmap', '"x_labels":["A","B"],"y_labels":["C"],"values":[[1]]'), /one value per x label/);
  assert.match(err('treemap', '"items":[{"label":"none","weight":0}]'), /must be positive/);
  assert.throws(() => ok('bar', '"bars":[{"label":"huge","value":1000000000001}]'));
  assert.match(err('bar', '"bars":[{"label":"x","value":1}],"script":"alert(1)"'), /unknown field `script`/);
  assert.match(err('bar', '"bars":[{"label":"x","value":1,"color":"#fff"}]'), /unknown field `color`/);
  assert.match(err('scatter', '"items":[]'), /unsupported visualization type/);
});

test('payload, text, count and depth are bounded', () => {
  assert.equal(reason(' '.repeat(MAX_PAYLOAD_BYTES + 1)), 'it’s 32.0 KiB, over the 32 KiB limit');
  const long = 'x'.repeat(MAX_TEXT_CHARS + 1);
  assert.match(reason(`{"version":1,"title":"${long}","summary":"s","type":"bar","bars":[{"label":"x","value":1}]}`), /maximum is 256/);
  const bars = Array.from({ length: MAX_MARKS + 1 }, (_, i) => `{"label":"${i}","value":1}`).join(',');
  assert.match(err('bar', `"bars":[${bars}]`), /maximum is 128/);
  let node = '{"kind":"text","text":"leaf"}';
  for (let i = 0; i < MAX_MOCK_DEPTH; i++) node = `{"kind":"card","children":[${node}]}`;
  assert.match(err('mockup', `"nodes":[${node}]`), /maximum mockup depth/);
});

test('a wrong version reads plainly', () => {
  const v2 = common('bar', '"bars":[{"label":"x","value":1}]').replace('"version":1', '"version":2');
  assert.equal(reason(v2), 'schema version 2 isn’t supported (expected 1)');
  assert.equal(reason(common('bar', '"bars":[{"label":"x","value":1}],"html":"<b>"')), 'unknown field `html` (a bar takes `bars`, `x_label`, `y_label`, `unit`)');
});

test('the minutes timeline a user hit still parses', () => {
  const viz = parse(
    '{"version":1,"title":"Where the minutes go","summary":"Build, CI, and release all pay the same two costs.","type":"timeline","events":[{"label":"Local dev build","start":0,"end":2,"lane":"Local"},{"label":"cargo test","start":2,"end":4,"lane":"Local"},{"label":"ci.yml macOS job","start":0,"end":40,"lane":"CI"},{"label":"ci.yml Windows job","start":0,"end":75,"lane":"CI"},{"label":"Release","start":0,"end":42,"lane":"Release"}],"ticks":"minutes"}',
  );
  assert.equal(viz.unit, 'minutes');
  assert.equal(viz.ticks, undefined);
  assert.deepEqual(
    viz.events.map((e) => e.lane),
    ['Local', 'Local', 'CI', 'CI', 'Release'],
  );
  const plan = '{"version":1,"title":"Plan","summary":"Duration","type":"timeline","events":[{"label":"Build","start":0,"end":5},{"label":"Test","start":5,"end":12}],"ticks":"minutes"}';
  assert.equal(parse(plan).events[1].end, 12);
  assert.deepEqual(parsePartial(plan).drawable, parse(plan));
});

test('singleton sequences and singular keys are schema-aware', () => {
  for (const [kind, body] of [
    ['bar', '"bar":{"label":"one","value":1}'],
    ['timeline', '"event":{"label":"one","start":1}'],
    ['donut', '"part":{"label":"one","value":1}'],
    ['stats', '"tile":{"label":"one","value":"1"}'],
    ['treemap', '"item":{"label":"one","weight":1}'],
    ['layers', '"layer":{"label":"one","item":"API"}'],
    ['mockup', '"node":{"kind":"card","child":{"kind":"text","text":"Hi"}}'],
    ['table', '"column":"A","row":[[1]]'],
    ['flow', '"node":{"id":"a","label":"A"}'],
    ['line', '"x_labels":["a","b"],"series":{"label":"A","value":[1,2]}'],
  ]) {
    assert.doesNotThrow(() => ok(kind, body), `${kind}: ${body}`);
  }
  // A string is one label, never split on spaces or commas.
  assert.deepEqual(ok('layers', '"layers":[{"label":"L","items":"A, B"}]').layers[0].items, ['A, B']);
});

test('text arrays join, and numbers become text only in text fields', () => {
  const viz = parse('{"version":"1","title":["Build","time"],"summary":42,"type":"stats","tiles":[{"label":2026,"value":12.5,"delta":["+1","today"]}]}');
  assert.equal(viz.title, 'Build time');
  assert.equal(viz.summary, '42');
  assert.deepEqual([viz.tiles[0].label, viz.tiles[0].value, viz.tiles[0].delta], ['2026', '12.5', '+1 today']);
});

test('numeric strings are numbers, without touching ordinal ticks or table cells', () => {
  assert.equal(ok('bar', '"bars":[{"label":"A","value":"1.25e2"}]').bars[0].value, 125);
  const line = ok('line', '"x_labels":[1,2],"series":[{"label":"A","values":["1","2"]}]');
  assert.deepEqual(line.x_labels, ['1', '2']);
  assert.deepEqual(line.series[0].values, [1, 2]);
  assert.equal(ok('timeline', '"events":[{"label":"A","start":"0","end":"12.5"}],"ticks":"minutes"').events[0].start, 0);
  const ordinal = ok('timeline', '"ticks":["2025","2026"],"events":[{"label":"A","start":"2025","end":2026}]');
  assert.deepEqual([ordinal.events[0].start, ordinal.events[0].end], ['2025', '2026']);
  assert.deepEqual(ok('table', '"columns":["ID"],"rows":[["001"]]').rows[0].cells, ['001']);
  assert.equal(looseNumber('$3.2k'), 3200);
  assert.equal(looseNumber('1,200'), 1200);
  assert.equal(looseNumber('-1,234.5'), -1234.5);
  assert.equal(looseNumber('45%'), 45);
  assert.equal(looseNumber('1,2,3'), null);
  assert.equal(looseNumber('NaN'), null);
  assert.equal(looseNumber('2m'), null);
});

test('type aliases have their meanings', () => {
  const body = {
    bar: '"bars":[{"label":"A","value":1}]',
    line: '"x_labels":["a","b"],"series":[{"label":"A","values":[1,2]}]',
    donut: '"parts":[{"label":"A","value":1}]',
    stats: '"tiles":[{"label":"A","value":"1"}]',
    heatmap: '"x_labels":["a"],"y_labels":["b"],"values":[[1]]',
    treemap: '"items":[{"label":"A","weight":1}]',
    timeline: '"events":[{"label":"A","start":1}]',
    flow: '"nodes":[{"id":"a","label":"A"}]',
    layers: '"layers":[{"label":"A"}]',
    mockup: '"nodes":[{"kind":"divider"}]',
  };
  for (const [alias, kind] of [
    ['bar_chart', 'bar'],
    ['histogram', 'bar'],
    ['area', 'line'],
    ['pie', 'donut'],
    ['kpi', 'stats'],
    ['metrics', 'stats'],
    ['matrix', 'heatmap'],
    ['tree_map', 'treemap'],
    ['gantt', 'timeline'],
    ['roadmap', 'timeline'],
    ['diagram', 'flow'],
    ['graph', 'flow'],
    ['stack', 'layers'],
    ['wireframe', 'mockup'],
  ]) {
    const viz = ok(alias, body[kind]);
    assert.equal(viz.type, kind, alias);
    if (alias === 'area') assert.equal(viz.area, true);
  }
  assert.equal(ok('Bar Chart', body.bar).type, 'bar');
  assert.match(err('area', '"x_labels":["a","b"],"series":[{"label":"A","values":[1,2]}],"area":false'), /conflicts/);
});

test('colour words are tones; CSS colours stay errors', () => {
  for (const spelling of ['color', 'colour']) assert.equal(ok('bar', `"bars":[{"label":"A","value":1,"${spelling}":"positive"}]`).bars[0].tone, 'positive');
  assert.doesNotThrow(() => ok('mockup', '"nodes":[{"kind":"badge","text":"OK","colour":"positive"}]'));
  assert.equal(ok('table', '"columns":["a"],"rows":[{"cells":[1],"color":"info"}]').rows[0].tone, 'info');
  assert.match(err('bar', '"bars":[{"label":"A","value":1,"colour":"#f00"}]'), /unknown field/);
  assert.match(err('bar', '"bars":[{"label":"A","value":1,"tone":"#fff"}]'), /`tone` must be one of/);
});

test('missing metadata is taken only from what was given', () => {
  const viz = parse('{"title":"Plan","event":{"label":"A","start":0}}');
  assert.deepEqual([viz.version, viz.summary, viz.type], [1, 'Plan', 'timeline']);
  assert.equal(parse('{"summary":"Build","bars":[{"label":"A","value":1}]}').title, 'Build');
  assert.equal(ok('flow', '"nodes":[{"id":"api"}]').nodes[0].label, 'api');
  assert.throws(() => parse('{"bars":[{"label":"A","value":1}]}'), 'no invented title');
  assert.throws(() => parse('{"title":"A","nodes":[{"id":"api"}]}'), 'ambiguous type');
});

test('ambiguous coercions and conflicting aliases still fail', () => {
  for (const [kind, body] of [
    ['timeline', '"ticks":"minutes","events":[{"label":"A","start":"Monday"}]'],
    ['timeline', '"ticks":"minutes","unit":"hours","events":[{"label":"A","start":0}]'],
    ['bar', '"bar":[{"label":"A","value":1}],"bars":[{"label":"B","value":2}]'],
    ['bar', '"bars":[{"label":"A","value":"1,2,3"}]'],
    ['bar', '"bars":[{"label":"A","value":"NaN"}]'],
    ['bar', '"bars":[{"label":"A","value":1,"color":"positive","tone":"negative"}]'],
    ['line', '"x_labels":[["a","b"],"c"],"series":[{"label":"A","values":[1,2]}]'],
  ]) {
    assert.throws(() => ok(kind, body), `${kind}: ${body}`);
  }
});

test('the slips models make parse, or fail clearly', () => {
  for (const [kind, body] of [
    ['bar', '"data":[{"name":"Build","value":"1,200"},{"x":"Test","y":"45%"}]'],
    ['bar', '"Bars":[{"Label":"A","Value":"$3.2k"}]'],
    ['donut', '"slices":[{"name":"Rust","value":"62%"},{"name":"Other","value":38}],"units":"%"'],
    ['line', '"labels":["Mon","Tue"],"data":{"label":"p95","values":["1,000","2.5k"]},"y_axis":"ms"'],
    ['stats', '"metrics":[{"name":"Uptime","value":"99.9%","change":"+0.1","data":["1","2","3"]}]'],
    ['stats', '"kpis":{"label":"One","value":1}'],
    ['table', '"headers":["Service","p95"],"data":[["API","1,200"],{"cells":["DB",42],"color":"green"}]'],
    ['table', '"columns":["A"],"rows":[{"data":["x"],"tone":"red"}]'],
    ['heatmap', '"x_labels":["Mon","Tue"],"y_labels":["AM","PM"],"data":[["1","2"],["3","4"]]'],
    ['heatmap', '"x_labels":["Mon","Tue"],"y_labels":["AM","PM","Mid"],"values":[[1,2,3],[4,5,6]]'],
    ['heatmap', '"x_labels":["A","B"],"y_labels":["C","D"],"values":[1,2,3,4]'],
    ['treemap', '"data":[{"name":"Core","size":"2k"},{"label":"UI","value":3}]'],
    ['timeline', '"phases":[{"title":"Ship","from":"2026-08","to":"2026-11"},{"title":"Design","from":"2026-01","until":"2026-03"}]'],
    ['timeline', '"events":[{"label":"Ship","start":"Mar","end":"May","track":"Ops","tone":"green"}]'],
    ['timeline', '"events":[{"label":"Ship","start":1,"end":2,"lane":null,"tone":null}]'],
    ['flow', '"nodes":[{"id":"a","name":"API"},{"id":"b","label":"DB"}],"links":[{"source":"a","target":"b","text":"reads"}]'],
    ['layers', '"tiers":[{"name":"UI","contents":["Sidebar","Transcript"]},{"name":"Core"}]'],
    ['mockup', '"nodes":[{"type":"panel","children":[{"kind":"Heading","text":"Hi"},{"kind":"switch","text":"On","value":true},{"kind":"spinner","text":"Load","value":"30%"}]}]'],
    ['mockup', '"nodes":[{"text":"a bare leaf"}]'],
    ['mockup', '"nodes":[{"kind":"frame","device":"iphone"}]'],
  ]) {
    assert.doesNotThrow(() => ok(kind, body), `${kind}: ${body}`);
  }
  // The transposed and the flat heatmap come out one row per y label.
  assert.deepEqual(ok('heatmap', '"x_labels":["Mon","Tue"],"y_labels":["AM","PM","Mid"],"values":[[1,2,3],[4,5,6]]').values, [
    [1, 4],
    [2, 5],
    [3, 6],
  ]);
  assert.deepEqual(ok('heatmap', '"x_labels":["A","B"],"y_labels":["C","D"],"values":[1,2,3,4]').values, [
    [1, 2],
    [3, 4],
  ]);
  for (const [kind, body, needle] of [
    ['flow', '"data":[{"id":"a","label":"A"}]', 'unknown field `data`'],
    ['bar', '"data":[{"label":"A","value":1}],"bars":[{"label":"A","value":2}]', 'conflicting'],
    ['timeline', '"ticks":"minutes","events":[{"label":"A","start":"Monday"}]', 'must be an array of axis labels'],
    ['bar', '"bars":[{"label":"A","value":1,"tone":"#fff"}]', '`tone` must be one of'],
    ['bar', '"bars":[{"label":"A","value":1,"color":"chartreuse"}]', 'unknown field `color`'],
    ['mockup', '"nodes":[{"kind":"carousel"}]', 'isn’t a mockup kind'],
    ['mockup', '"nodes":[{"kind":"frame","device":"watch"}]', 'isn’t a mockup device'],
    ['bar', '"bars":[{"label":"A","value":1}],"footnote":"x"', 'unknown field `footnote`'],
    ['bar', '"bars":[{"label":"A","values":[1,2]}]', 'unknown field `values`'],
  ]) {
    const why = reason(common(kind, body));
    assert.ok(why.includes(needle), `${kind}: ${body} -> ${why}`);
    assert.ok(why.length < 160, why);
  }
});

test('date endpoints become ordinal ticks, in order', () => {
  const viz = ok('timeline', '"events":[{"label":"Build","start":"2026-04","end":"2026-08"},{"label":"Design","start":"2026-01","end":"2026-04"},{"label":"Ship","start":"2026-10"}]');
  assert.deepEqual(viz.ticks, ['2026-01', '2026-04', '2026-08', '2026-10']);
  assert.equal(viz.events[1].end, '2026-04');
  assert.deepEqual(ok('timeline', '"events":[{"label":"B","start":"March","end":"May"},{"label":"A","start":"January","end":"March"}]').ticks, ['January', 'March', 'May']);
  assert.deepEqual(ok('timeline', '"events":[{"label":"A","start":"alpha","end":"beta"},{"label":"B","start":"gamma"}]').ticks, ['alpha', 'beta', 'gamma']);
  assert.match(err('timeline', '"events":[{"label":"A","start":"May","end":"Jan"}]'), /ends before it starts/);
});

test('comments, trailing commas and a fence inside the fence are forgiven', () => {
  const body = `{
    // the build, by phase
    "version": 1,
    "title": "Build",
    "summary": "Where it went",
    "type": "bar",
    "bars": [
      {"label": "compile", "value": 3,},
      {"label": "link", "value": 1}, /* tail */
    ],
  }`;
  assert.equal(parse(body).bars.length, 2);
  // Markdown ends the block at the inner fence's closing line, so only its opening is left.
  assert.equal(parse(unwrapInnerFence(`\`\`\`json\n${body}`)).bars.length, 2);
  assert.equal(parse(unwrapInnerFence(`\`\`\`json\n${body}\n\`\`\``)).bars.length, 2);
  assert.doesNotThrow(() => ok('bar', '"bars":[{"label":"https://x","value":1}]'));
});

test('lines line up with their axis', () => {
  const viz = ok('line', '"x_labels":["Mon","Tue","Wed"],"series":[{"label":"p50","values":[1,2,3],"tone":"info"},{"label":"p95","values":[4,5,6]}],"area":true,"y_label":"latency","unit":"ms"');
  assert.deepEqual([viz.series.length, viz.area, viz.unit], [2, true, 'ms']);
  assert.match(err('line', '"x_labels":["a"],"series":[{"label":"s","values":[1]}]'), /at least two x labels/);
  assert.match(err('line', '"x_labels":["a","b"],"series":[{"label":"s","values":[1]}]'), /one value per x label/);
  assert.match(err('line', '"x_labels":["a","b"],"series":[]'), /1 to 6 series/);
  const seven = Array.from({ length: 7 }, (_, i) => `{"label":"${i}","values":[1,2]}`).join(',');
  assert.match(err('line', `"x_labels":["a","b"],"series":[${seven}]`), /1 to 6 series/);
  assert.match(err('line', '"x_labels":["a","b"],"series":[{"label":"s","values":[1,2]}],"unit":"this unit is far too long to sit beside a number on an axis"'), /maximum is 48/);
});

test('donuts and stats stay small', () => {
  assert.match(err('donut', '"parts":[{"label":"none","value":0}]'), /must be positive/);
  assert.match(err('donut', '"parts":[]'), /1 to 12 parts/);
  assert.deepEqual(ok('stats', '"tiles":[{"label":"Success","value":"99.94%","delta":"+0.2 pts","trend":[1,2,3],"tone":"positive"}]').tiles[0].trend, [1, 2, 3]);
  assert.match(err('stats', '"tiles":[{"label":"a","value":"1","trend":[1]}]'), /2 to 64 values/);
  assert.equal(ok('stats', '"tiles":[{"label":"a","value":1}]').tiles[0].value, '1');
  assert.match(err('stats', `"tiles":[{"label":"a","value":"${'9'.repeat(49)}"}]`), /maximum is 48/);
  assert.match(err('stats', '"tiles":[]'), /1 to 12 tiles/);
});

test('tables are rectangles of text and numbers', () => {
  const viz = ok('table', '"columns":["Service","p95"],"rows":[["API",120.5],{"cells":["Workers",940],"tone":"warning"}]');
  assert.deepEqual(viz.rows[0].cells, ['API', 120.5]);
  assert.equal(viz.rows[1].tone, 'warning');
  assert.match(err('table', '"columns":["a","b"],"rows":[["x"]]'), /one cell per column/);
  assert.match(err('table', '"columns":["a"],"rows":[]'), /1 to 64 rows/);
  assert.match(err('table', '"columns":["a"],"rows":[[true]]'), /invalid/);
  assert.match(err('table', '"columns":["a"],"rows":[{"cells":["x"],"href":"y"}]'), /invalid/);
});

test('timelines take numbers or their own ticks', () => {
  assert.doesNotThrow(() => ok('timeline', '"events":[{"label":"Design","start":0,"end":2,"lane":"UI"},{"label":"Ship","start":4}],"unit":"wk"'));
  assert.doesNotThrow(() => ok('timeline', '"ticks":["Oct","Nov","Dec"],"events":[{"label":"Beta","start":"Oct","end":"Nov","tone":"info"},{"label":"GA","start":"Dec"}]'));
  assert.match(err('timeline', '"events":[{"label":"x","start":3,"end":1}]'), /ends before it starts/);
  assert.match(err('timeline', '"ticks":["Oct"],"events":[{"label":"x","start":"Jan"}]'), /not one of the ticks/);
  assert.match(err('timeline', '"events":[{"label":"x","start":0},{"label":"y","start":"Jan"}]'), /needs `ticks`/);
  assert.match(err('timeline', '"ticks":["Oct"],"events":[{"label":"x","start":1}]'), /must name a tick/);
  assert.match(err('timeline', '"ticks":["Oct","Oct"],"events":[{"label":"x","start":"Oct"}]'), /repeats/);
  const lanes = Array.from({ length: 9 }, (_, i) => `{"label":"e","start":1,"lane":"l${i}"}`).join(',');
  assert.match(err('timeline', `"events":[${lanes}]`), /more than 8 lanes/);
});

test('flows reference only their own nodes', () => {
  const viz = ok('flow', '"nodes":[{"id":"ui","label":"Composer","group":"App"},{"id":"core","label":"Workspace","detail":"routes turns","tone":"accent"}],"edges":[{"from":"ui","to":"core","label":"send"}]');
  assert.deepEqual([viz.nodes.length, viz.edges.length], [2, 1]);
  assert.doesNotThrow(() => ok('flow', '"nodes":[{"id":"a","label":"A"},{"id":"b","label":"B"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"a"}]'), 'cycles are drawn');
  assert.match(err('flow', '"nodes":[{"id":"a","label":"A"}],"edges":[{"from":"a","to":"ghost"}]'), /names no node `ghost`/);
  assert.match(err('flow', '"nodes":[{"id":"a","label":"A"},{"id":"a","label":"B"}]'), /not unique/);
  assert.match(err('flow', '"nodes":[{"id":"a b","label":"A"}]'), /letters, digits/);
  assert.match(err('flow', '"nodes":[{"id":"a","label":"A"}],"edges":[{"from":"a","to":"a"}]'), /to itself/);
  const grouped = Array.from({ length: 7 }, (_, i) => `{"id":"n${i}","label":"N","group":"g${i}"}`).join(',');
  assert.match(err('flow', `"nodes":[${grouped}]`), /more than 6 groups/);
});

test('mockups: their kinds, values and device frames', () => {
  const tree = '"nodes":[{"kind":"frame","device":"phone","text":"Release","children":[{"kind":"nav","text":"Trek","children":[{"kind":"avatar","text":"Ada Lovelace"}]},{"kind":"tabs","value":"Health","children":[{"kind":"text","text":"Health"},{"kind":"text","text":"Deploys"}]},{"kind":"input","text":"Search services","value":"workers"},{"kind":"toggle","text":"Auto-rollback","value":"on"},{"kind":"progress","text":"Rollout","value":"64%","tone":"info"},{"kind":"image","text":"Latency chart","value":"wide"},{"kind":"list","children":[{"kind":"text","text":"API"},{"kind":"text","text":"Workers"}]}]}]';
  assert.equal(ok('mockup', tree).nodes[0].device, 'phone');
  for (const [node, message] of [
    ['{"kind":"frame"}', 'every frame needs one'],
    ['{"kind":"card","device":"phone"}', 'belongs on a frame'],
    ['{"kind":"frame","device":"watch"}', 'invalid'],
    ['{"kind":"frame","device":"phone","children":[{"kind":"frame","device":"browser"}]}', 'cannot nest'],
    ['{"kind":"tabs","children":[{"kind":"button","text":"x"}]}', 'must be text nodes'],
    ['{"kind":"tabs","value":"Z","children":[{"kind":"text","text":"A"}]}', 'name one of its tabs'],
    ['{"kind":"tabs"}', '1 to 8 text children'],
    ['{"kind":"toggle","text":"x","value":"maybe"}', '`on` or `off`'],
    ['{"kind":"progress","value":"140%"}', '0 to 100'],
    ['{"kind":"progress"}', '0 to 100'],
    ['{"kind":"image","value":"https://example.com/a.png"}', 'must be one of'],
    ['{"kind":"input"}', 'text is required'],
    ['{"kind":"avatar","text":"A","value":"x"}', 'cannot have a value'],
    ['{"kind":"input","text":"x","children":[{"kind":"divider"}]}', 'cannot have children'],
    ['{"kind":"image","src":"x"}', 'unknown field `src`'],
  ]) {
    assert.ok(err('mockup', `"nodes":[${node}]`).includes(message), node);
  }
  assert.equal(progressFraction(' 64 % '), 0.64);
});

test('a block still arriving draws only its whole marks', () => {
  const full = common('bar', '"bars":[{"label":"a","value":12},{"label":"b","value":128},{"label":"c \\"q\\"","value":7}],"x_label":"s"');
  const at = (needle) => full.slice(0, full.indexOf(needle) + needle.length);
  const early = parsePartial(at('"type":"bar"'));
  assert.deepEqual([early.kind, early.title, early.drawable], ['bar', 'Demo', null]);
  assert.equal(parsePartial('{"version":1,"title":"De').title, null, 'only a title written in full');
  // A number still arriving is left out, not drawn as 12 on its way to 128.
  assert.equal(parsePartial(at('"value":12')).drawable, null);
  assert.deepEqual(
    parsePartial(at('"value":12},{"label":"b","value":12')).drawable.bars.map((b) => b.value),
    [12],
  );
  assert.equal(parsePartial(at('{"label":"c \\"q')).drawable.bars.length, 2);
  const done = parsePartial(`${full}\n\`\``);
  assert.ok(done.complete);
  assert.deepEqual(done.drawable, parse(full));
  for (let end = 0; end <= full.length; end++) parsePartial(full.slice(0, end));
  const open = (kind, body) => common(kind, body).slice(0, -1);
  assert.equal(parsePartial(open('table', '"columns":["a","b"],"rows":[["x",1],["y",')).drawable.rows.length, 1);
  assert.equal(parsePartial(open('line', '"x_labels":["a","b"],"series":[{"label":"s","values":[1,2]},{"label":"t","values":[3')).drawable.series.length, 1);
});

test('numbers read as the Mac writes them', () => {
  assert.equal(formatNumber(1284), '1,284');
  assert.equal(formatNumber(12900000), '12.9M');
  assert.equal(formatNumber(42000), '42K');
  assert.equal(formatNumber(99.94), '99.94');
  assert.equal(formatNumber(1.5), '1.5');
  assert.equal(formatNumber(-0.001), '0.00', 'no minus on a zero');
  assert.equal(formatExact(1284000), '1,284,000');
  assert.equal(formatExact(0.02), '0.02');
  assert.equal(formatTick(0.5, 0.5), '0.5');
  assert.equal(formatTick(200, 100), '200');
  assert.equal(withUnit('-5', '$'), '-$5');
  assert.equal(withUnit('12', '%'), '12%');
  assert.equal(withUnit('12', 'ms'), '12 ms');
});

test('the table behind a chart copies as Markdown and CSV', () => {
  const viz = ok('bar', '"bars":[{"label":"a|b","value":1.5},{"label":"c, d","value":2}],"x_label":"s"');
  const data = dataTable(viz);
  assert.deepEqual(data.columns, ['Label', 's']);
  assert.equal(tableMarkdown(data), '| Label | s |\n| --- | ---: |\n| a\\|b | 1.5 |\n| c, d | 2 |\n');
  assert.equal(tableCsv(data), 'Label,s\na|b,1.5\n"c, d",2\n');
  assert.equal(dataTable(ok('mockup', '"nodes":[{"kind":"divider"}]')), null);
});

test('scales, treemaps, flows and lanes lay out as on the Mac', () => {
  const scale = niceScale(0, 142, 3);
  assert.deepEqual([scale.lo, scale.hi, scale.step], [0, 150, 50]);
  assert.deepEqual(scale.ticks, [0, 50, 100, 150]);
  assert.deepEqual(niceScale(0, 0, 3).ticks.at(-1), 1);
  const tiles = squarify([6, 6, 4, 3, 2, 2, 1], 6, 4);
  const area = tiles.reduce((t, r) => t + r.w * r.h, 0);
  assert.ok(Math.abs(area - 24) < 1e-9);
  for (const r of tiles) assert.ok(r.x >= -1e-9 && r.y >= -1e-9 && r.x + r.w <= 6 + 1e-9 && r.y + r.h <= 4 + 1e-9);
  // A chain, a fork, and a cycle drawn back up.
  const flow = flowLayout(4, [[0, 1], [1, 2], [1, 3], [3, 0]], [null, null, null, null]);
  assert.deepEqual(flow.rank, [0, 1, 2, 2]);
  assert.deepEqual(flow.back, [false, false, false, true]);
  assert.equal(flow.slots, 2);
  assert.deepEqual(pack([[0, 2], [1, 3], [2, 4]], 0), [0, 1, 0]);
});

// ---------------------------------------------------------------------------------------------
// Math

const found = (s) => mathRegions(s).map((r) => s.slice(r.start, r.end));

test('the four delimiters are found', () => {
  assert.deepEqual(found(String.raw`Euler: $e^{i\pi}+1=0$.`), [String.raw`$e^{i\pi}+1=0$`]);
  assert.deepEqual(found(String.raw`so \(a^2+b^2=c^2\) holds`), [String.raw`\(a^2+b^2=c^2\)`]);
  assert.deepEqual(found('$$\n\\int_0^1 x\\,dx\n$$'), ['$$\n\\int_0^1 x\\,dx\n$$']);
  assert.deepEqual(found('rule, \\[ P(\\theta=1\\mid T=+) \\approx \\boxed{0.0162}. \\]'), ['\\[ P(\\theta=1\\mid T=+) \\approx \\boxed{0.0162}. \\]']);
});

test('display math spans lines but not paragraphs', () => {
  assert.deepEqual(found('\\[\na = b \\\\\nc = d\n\\]'), ['\\[\na = b \\\\\nc = d\n\\]']);
  assert.deepEqual(found('\\[\na = b\n\nc = d\n\\]'), []);
  assert.deepEqual(found('$$\na\n\nb\n$$ c\n\nmore'), []);
});

test('money and shell are not math', () => {
  for (const s of ['It costs $5 and $10.', 'between $1,000 and $2,000', '$5-$10 range', 'Pay $5, then $6 more', 'I have $3', '$ 5 and 6 $', 'US$5 vs US$6', 'echo $HOME and $PATH', 'export PATH=$HOME/bin:$PATH', 'run $(pwd) and $(date)', 'use ${HOME} or ${PATH}', 'cost: $FOO, $BAR']) {
    assert.deepEqual(found(s), [], s);
  }
  assert.deepEqual(found(String.raw`\$5 and \$10 and \$x\$`), []);
  assert.deepEqual(found(String.raw`\$5 but $x$`), ['$x$']);
  assert.deepEqual(found('the $n$th term'), ['$n$']);
});

test('code is never math', () => {
  assert.deepEqual(found('use `$x$` and `\\(y\\)`'), []);
  assert.deepEqual(found('``a $x$ ` b``'), []);
  assert.deepEqual(found('```sh\necho $A $B$\n\\[x\\]\n```'), []);
  assert.deepEqual(found('~~~\n$x$\n~~~'), []);
  assert.deepEqual(found('- item\n  ```\n  $x$\n  ```'), []);
  assert.deepEqual(found('```\n$x$'), []);
  assert.deepEqual(found('`$a$` then $b$'), ['$b$']);
  assert.deepEqual(found('```\n$a$\n```\n$b$'), ['$b$']);
  assert.deepEqual(found('a ` b $c$'), ['$c$']);
  assert.deepEqual(found(String.raw`a \\[b\\] c`), [], 'an escaped backslash, then a bracket');
});

test('math still being written runs to the end, open', () => {
  for (const [s, start] of [
    ['By Bayes, \\[ P(\\theta', 10],
    ['$$\n\\frac{a}', 0],
    ['so \\(x^', 3],
    ['a\n\n$$', 3],
  ]) {
    const r = mathRegions(s);
    assert.equal(r.length, 1, s);
    assert.deepEqual([r[0].closed, r[0].start, r[0].end], [false, start, s.length], s);
  }
  assert.deepEqual(mathRegions('costs $5'), []);
  assert.deepEqual(mathRegions('\\[ a\n\nmore text'), []);
  assert.ok(mathRegions('\\[ P(\\theta) \\]')[0].closed);
});

test('neighbouring formulas are each found, and their TeX is between the delimiters', () => {
  const s = String.raw`- Inline: \(e^{i\pi} + 1 = 0\), $\sqrt{2} \approx 1.4142$, $x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}$, and $\hat\theta$, $\alpha \le \beta \ne \gamma$.`;
  assert.deepEqual(found(s), [String.raw`\(e^{i\pi} + 1 = 0\)`, String.raw`$\sqrt{2} \approx 1.4142$`, String.raw`$x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}$`, String.raw`$\hat\theta$`, String.raw`$\alpha \le \beta \ne \gamma$`]);
  assert.deepEqual(found('- a $x\n- b y$'), [], 'inline math stops at a new list item');
  const t = String.raw`a \[ x^2 \] b $y$ c \(z\) $$w$$`;
  assert.deepEqual(
    mathRegions(t).map((r) => regionTex(r, t)),
    [' x^2 ', 'y', 'z', 'w'],
  );
  const quoted = '> \\[\n> a \\\\\n> b\n> \\]';
  assert.equal(regionTex(mathRegions(quoted)[0], quoted), '\na \\\\\nb\n');
  const many = `${'\\[ '.repeat(20000)}\n\nafter \\[ b \\] done`;
  assert.deepEqual(found(many), ['\\[ b \\]'], 'openers that died are not rescanned');
});

test('marked reads around the formulas and hands each over whole', () => {
  const marked = new Marked({ gfm: true, breaks: false }, markdownMath);
  const html = marked.parse(String.raw`Let $a_1 + b_1$ and \(x_2 * y_2\) and \[ \{x \mid x>0\} \]

$$
\begin{pmatrix} a & b \\ c & d \end{pmatrix}
$$

It costs $5 and $10; ` + '`$x$`' + String.raw` is code. [about $x$](https://e.com/a "on $y$")

| $a|b$ | x |
| --- | --- |
| 1 | 2 |

    indented $z$ code`);
  assert.ok(html.includes('<span class="md-math" data-delim="dollar">a_1 + b_1</span>'), html);
  assert.ok(html.includes('<span class="md-math" data-delim="paren">x_2 * y_2</span>'), html);
  assert.ok(html.includes(String.raw`<span class="md-math" data-delim="bracket"> \{x \mid x&gt;0\} </span>`), html);
  assert.ok(html.includes(String.raw`<div class="md-math-block"><span class="md-math" data-delim="dollars">
\begin{pmatrix} a &amp; b \\ c &amp; d \end{pmatrix}
</span></div>`), html);
  assert.ok(html.includes('It costs $5 and $10; <code>$x$</code> is code.'), html);
  assert.ok(html.includes('title="on $y$"'), 'a title keeps its text as written');
  assert.ok(html.includes('<th><span class="md-math" data-delim="dollar">a|b</span></th>'), 'a formula hides its pipe from the table');
  assert.ok(html.includes('<pre><code>indented $z$ code'), 'code keeps its dollars');
  assert.ok(!/[\u{F0000}-\u{FFFFD}]/u.test(html), 'no placeholder is left');
  // A formula still being written shows as written; text without math is untouched.
  assert.equal(restoreMath(new Marked().parse(hideMath('so \\(x^'))), '<p>so \\(x^</p>\n');
  assert.equal(hideMath('no math, $5 and $6'), 'no math, $5 and $6');
});

// ---------------------------------------------------------------------------------------------
// Match my computer

/** Journal (crates/trek-core/themes/journal.toml) as the computer sends it. */
const journal = {
  theme: {
    id: 'journal',
    name: 'Journal',
    mode: 'light',
    ui_font: 'sans',
    conversation_font: 'serif',
    ui_size: 15,
    conversation_size: 16,
    corners: 1.25,
    density: 'roomy',
    motion: 'calm',
    dark: { accent: '#E0895AFF', window: '#14110EFF', text: '#ECE4D8FF' },
    light: { accent: '#B0552CFF', on_accent: '#FFFFFFFF', window: '#FBF8F2FF', surface: '#F0EAE0FF', border: '#E4DACC80' },
  },
  background: 'builtin:paintings/matterhorn',
  placement: 'chat',
  dim: 0.5,
};

test('a look’s colours become the remote’s tokens, for Night or Paper', () => {
  assert.deepEqual(colour('#E0895AFF'), [224, 137, 90, 1]);
  assert.equal(colour('#E0895A')[3], 1);
  for (const bad of ['#fff', 'red', '#12345', 'url(x)', '#E0895AFF; color: red', null]) assert.equal(colour(bad), null, String(bad));
  const paper = lookStyle(journal, false);
  assert.equal(paper['--bg'], '#fbf8f2');
  assert.equal(paper['--accent'], '#b0552c');
  assert.equal(paper['--link'], '#b0552c', 'links wear the accent');
  assert.equal(paper['--on-accent'], '#ffffff');
  assert.equal(paper['--border'], '#e4dacc80', 'a translucent rule keeps its alpha');
  assert.equal(paper['--fg'], undefined, 'what the theme leaves out stays Trek’s');
  const night = lookStyle(journal, true);
  assert.deepEqual([night['--bg'], night['--fg'], night['--fg-strong']], ['#14110e', '#ece4d8', '#ece4d8']);
  // No text colour on the accent: white if it reads, else near-black.
  assert.equal(night['--on-accent'], '#111111');
});

test('a look’s faces, sizes and corners, within the Mac’s ranges', () => {
  const style = lookStyle(journal, false);
  assert.match(style['--look-convo'], /serif$/);
  assert.equal(style['--font'], undefined, 'a sans interface is the remote’s own');
  assert.equal(style['--text'], '15.54px');
  assert.equal(style['--convo'], '16.3px');
  assert.deepEqual([style['--radius'], style['--radius-s'], style['--radius-l']], ['15px', '10px', '22.5px']);
  const square = lookStyle({ theme: { name: 'Console', ui_font: 'mono', conversation_font: 'mono', corners: 0.25, ui_size: 1e9 } }, true);
  assert.equal(square['--font'], 'var(--mono)');
  assert.equal(square['--text'], '17.14px', 'clamped to the Mac’s 18 points');
  assert.equal(square['--radius'], '3px');
  // Trek's own sends no colours; an unknown face or a corner that isn't a number changes nothing.
  assert.deepEqual(lookStyle({ theme: { id: 'trek', name: 'Trek', ui_font: 'comic', corners: 'round' } }, true), {});
  assert.deepEqual(lookStyle(null, true), {});
});
