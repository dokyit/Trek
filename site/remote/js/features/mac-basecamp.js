// Basecamp, the computer's activity dashboard, for today, this week or all time, as the iPhone has
// it from a computer with `basecamp_v2`: what needs you and what's ready for review, then the
// range's totals, its turns over time and the tables by project and by model, worded as the
// computer words them. A computer from before it gets its older recap: the rows to review, the
// trek in a sentence and its tiles.

import { useLayoutEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Segmented, ProjectBadge, ago, plural } from '../ui.js';
import * as app from '../model.js';
import { Frame, Asking, AgentMark, Title, useOnline, useMounted, recall, remember } from './mac-parts.js';
import { barAt, barLayout, barPath, barX, left, peak, reviewLook, tickLayout } from './mac-text.js';

const RANGES = [
  ['today', 'Today'],
  ['week', 'This week'],
  ['all', 'All time'],
];
const RANGE_KEY = 'trek-remote-basecamp-range';
/** Rows of "Needs review" shown before "Show more", as on the Mac. */
const REVIEW_ROWS = 8;

function savedRange() {
  try {
    const r = localStorage.getItem(RANGE_KEY);
    return RANGES.some(([v]) => v === r) ? r : 'today';
  } catch {
    return 'today';
  }
}

export function BasecampScreen() {
  const state = app.useApp();
  const [range, setRange] = useState(savedRange);
  const [recap, setRecap] = useState(() => recall(`basecamp:${range}`));
  const [usage, setUsage] = useState(() => recall('usage'));
  const current = useRef(range);
  const mounted = useMounted();

  const load = async (r = current.current) => {
    try {
      const reply = await app.ask({ type: 'basecamp', range: r });
      remember(`basecamp:${r}`, reply);
      if (mounted.current && current.current === r) setRecap(reply);
    } catch (err) {
      if (mounted.current) app.toast(err.message, 'negative');
    }
  };
  // The gauge top right follows the limit closest to running out: asked quietly, once a visit.
  const loadUsage = () => {
    if (recall('usage')) return;
    app
      .ask({ type: 'usage' })
      .then((u) => {
        remember('usage', u);
        if (mounted.current) setUsage(u);
      })
      .catch(() => {});
  };
  const online = useOnline(() => {
    load();
    loadUsage();
  });

  const pick = (r) => {
    if (r === range) return;
    try {
      localStorage.setItem(RANGE_KEY, r);
    } catch {
      // Remembered for this visit only.
    }
    current.current = r;
    setRange(r);
    setRecap(recall(`basecamp:${r}`));
    if (online) load(r);
  };

  const unseen = (recap?.review || []).filter((row) => row.unseen);
  const markAllRead = () => {
    for (const row of unseen) app.tell({ type: 'mark_seen', thread_id: row.thread_id });
    setRecap({ ...recap, review: recap.review.map((row) => ({ ...row, unseen: false })) });
    load();
  };

  const v2 = app.has('basecamp_v2');
  const most = usage ? peak(usage) : 0;
  return html`<${Frame}
    title="Basecamp"
    id="mac-basecamp"
    sub=${!v2 && recap?.greeting ? recap.greeting : state.machine?.name}
    actions=${html`
      ${unseen.length > 0 && html`<button class="btn small ghost mac-bar-btn" type="button" onClick=${markAllRead}>Mark all read</button>`}
      <button
        type="button"
        class=${`icon-btn mac-gauge ${most >= 95 ? 'failed' : most >= 80 ? 'approval' : ''}`}
        aria-label=${usage ? `Usage: ${left(most)}% left on the tightest limit` : 'Usage'}
        title="Usage"
        onClick=${() => app.go({ screen: 'usage' })}
      ><${Icon} name="gauge" /></button>`}
  >
    <div class="mac-page">
      <${Segmented} label="Range" options=${RANGES} value=${range} onChange=${pick} />
      ${!recap
        ? html`<${Asking} text="Reading the trail…" />`
        : v2
          ? html`<${Sections} key=${range} recap=${recap} />`
          : html`<${OlderRecap} key=${range} recap=${recap} />`}
    </div>
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// Basecamp as the computer draws it now

function Sections({ recap }) {
  const [more, setMore] = useState(false);
  const rows = recap.review || [];
  const needs = rows.filter((r) => r.needs_you);
  const review = rows.filter((r) => !r.needs_you);
  return html`
    <section>
      <${Title} text="Needs you" count=${needs.length} />
      ${needs.length ? html`<${ReviewList} rows=${needs} />` : html`<p class="mac-quiet">Nothing is waiting on you.</p>`}
    </section>
    ${review.length > 0 &&
    html`<section>
      <${Title} text="Needs review" count=${review.length} />
      <${ReviewList} rows=${more ? review : review.slice(0, REVIEW_ROWS)} />
      ${review.length > REVIEW_ROWS && html`<button class="mac-more" type="button" onClick=${() => setMore(!more)}>${more ? 'Show less' : `Show ${review.length - REVIEW_ROWS} more`}</button>`}
    </section>`}
    ${recap.empty
      ? html`<${NoActivity} range=${recap.range} />`
      : html`
          ${recap.totals && html`<${Totals} totals=${recap.totals} />`}
          ${recap.chart?.bars?.length > 0 && html`<${Turns} chart=${recap.chart} />`}
          ${recap.projects?.length > 0 && html`<${Table} title="Projects" rows=${recap.projects} />`}
          ${recap.models?.length > 0 && html`<${Table} title="Models" rows=${recap.models} />`}
        `}
  `;
}

function ReviewList({ rows, showsAgent = false }) {
  return html`<div class="mac-card mac-list">${rows.map((row) => html`<${ReviewRow} key=${row.thread_id} row=${row} showsAgent=${showsAgent} />`)}</div>`;
}

/** A row of "Needs you" or "Needs review": what it waits on (or how it ended), its title, project and diff stat; a tap opens the thread. */
function ReviewRow({ row, showsAgent }) {
  const look = reviewLook(row);
  const diff = (row.additions || 0) + (row.deletions || 0) > 0;
  return html`<button type="button" class=${`mac-review ${row.unseen ? 'unseen' : ''}`} onClick=${() => app.go({ screen: 'thread', threadId: row.thread_id })}>
    <span class=${`mac-review-icon ${look.tone}`}><${Icon} name=${look.icon} size="s" /></span>
    <span class="mac-review-main">
      <span class="mac-review-title">${row.title}</span>
      ${(showsAgent || row.project || diff) &&
      html`<span class="mac-review-meta">
        ${showsAgent && row.agent && html`<${AgentMark} agent=${row.agent} />`}
        ${row.project && html`<span class="mac-badge-s"><${ProjectBadge} project=${row.project} /></span><span class="mac-ellipsis">${row.project.name}</span>`}
        ${diff && html`<span class="mono"><span class="add">+${row.additions || 0}</span> <span class="del">−${row.deletions || 0}</span></span>`}
      </span>`}
    </span>
    <span class=${`mac-review-state ${row.label ? look.tone : ''}`}>${row.label || ago(row.updated_at)}</span>
  </button>`;
}

/** The range's totals: the computer's row of figures, three to a row. */
function Totals({ totals }) {
  const figures = [
    ['Threads', String(totals.threads ?? 0)],
    ['Turns', String(totals.turns ?? 0)],
    ['Tokens', totals.tokens_label ?? String(totals.tokens ?? 0)],
    ['Cost', totals.cost ?? '—'],
    ['Failures', String(totals.failed ?? 0), (totals.failed ?? 0) > 0],
    ['Agent time', totals.agent_time ?? ''],
  ];
  return html`<div class="mac-card mac-totals">
    ${figures.map(([label, value, alert]) => html`<div class="mac-figure"><span>${label}</span><b class=${alert ? 'failed' : ''}>${value}</b></div>`)}
  </div>`;
}

/** Turns over the range, a bar a stretch: tap (or point at) a bar to read it, as the computer shows one hovered. */
function Turns({ chart }) {
  const [selected, setSelected] = useState(null);
  const bar = selected != null ? chart.bars[selected] : null;
  return html`<section class="mac-card mac-turns">
    <div class=${`mac-turns-head ${bar ? 'on' : ''}`}>
      <span class="mac-ellipsis" aria-live="polite">${bar ? bar.line : `Turns per ${chart.step}`}</span>
      <span class="mac-num">${chart.total}</span>
    </div>
    <${TurnsChart} chart=${chart} selected=${selected} onSelect=${setSelected} />
    <ul class="sr">${chart.bars.map((b, i) => html`<li key=${i}>${b.line}</li>`)}</ul>
  </section>`;
}

const PLOT = 96;
const AXIS = 20;

let measurer = null;
/** A label's width as the axis draws it. */
function measure(text) {
  try {
    if (!measurer) {
      measurer = document.createElement('canvas').getContext('2d');
      measurer.font = `11px ${getComputedStyle(document.body).fontFamily}`;
    }
    return measurer.measureText(text).width;
  } catch {
    return text.length * 6;
  }
}

/** The bars, drawn as the computer draws them: one quiet ink, the one being read darker, the axis labels under their bars. */
function TurnsChart({ chart, selected, onSelect }) {
  const box = useRef(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return undefined;
    const take = () => setWidth(el.clientWidth);
    take();
    const watch = new ResizeObserver(take);
    watch.observe(el);
    return () => watch.disconnect();
  }, []);
  const n = chart.bars.length;
  const layout = barLayout(n, width);
  const top = Math.max(0, ...chart.bars.map((b) => b.turns || 0));
  const ticks = width > 0 ? tickLayout(chart.ticks, n, layout, width, measure) : [];
  const at = (e) => barAt(e.clientX - box.current.getBoundingClientRect().left, n, width);
  // From the bar being read now, however fast the keys repeat.
  const key = (e) => {
    const step = {
      ArrowRight: (s) => Math.min((s ?? -1) + 1, n - 1),
      ArrowLeft: (s) => Math.max((s ?? n) - 1, 0),
      Home: () => 0,
      End: () => n - 1,
      Escape: () => null,
    }[e.key];
    if (!step) return;
    e.preventDefault();
    onSelect(step);
  };
  return html`<div
    class="mac-chart"
    ref=${box}
    tabindex="0"
    role="group"
    aria-label=${`Turns per ${chart.step}. Arrow keys read each bar.`}
    onKeyDown=${key}
    onPointerMove=${(e) => e.pointerType === 'mouse' && onSelect(at(e))}
    onPointerLeave=${(e) => e.pointerType === 'mouse' && onSelect(null)}
    onPointerUp=${(e) => {
      if (e.pointerType === 'mouse') return;
      const i = at(e);
      onSelect(selected === i ? null : i);
    }}
  >
    ${width > 0 &&
    html`<svg width=${width} height=${PLOT + AXIS} viewBox=${`0 0 ${width} ${PLOT + AXIS}`} aria-hidden="true">
      ${selected != null && html`<rect class="mac-chart-band" x=${selected * (layout.slot + layout.gap) - layout.gap / 2} y="0" width=${layout.slot + layout.gap} height=${PLOT} />`}
      ${chart.bars.map((b, i) => {
        if (!(b.turns > 0) || !(top > 0)) return null;
        const h = Math.max(((b.turns || 0) / top) * (PLOT - 2), 2);
        return html`<path key=${i} class=${i === selected ? 'on' : ''} style=${{ animationDelay: `${((i / n) * 0.35).toFixed(3)}s` }} d=${barPath(barX(i, layout), layout.bar, h, PLOT)} />`;
      })}
      <rect class="mac-chart-base" x="0" y=${PLOT} width=${width} height="1" />
      ${ticks.map((t) => html`<text x=${t.x} y=${PLOT + 15} text-anchor="middle">${t.label}</text>`)}
    </svg>`}
  </div>`;
}

/** A table by project or by model, a two-line row each: its mark and name with its tokens, then its threads, turns and failures with its cost. */
function Table({ title, rows }) {
  return html`<section>
    <${Title} text=${title} />
    <div class="mac-card mac-list">
      ${rows.map(
        (row, i) => html`<div class="mac-trow" key=${`${row.agent?.key || row.project?.id || ''}:${row.label}:${i}`}>
          <span class="mac-trow-mark">${row.agent ? html`<${AgentMark} agent=${row.agent} />` : row.project ? html`<span class="mac-badge-s"><${ProjectBadge} project=${row.project} /></span>` : html`<${Icon} name="mac-inbox" size="s" />`}</span>
          <span class="mac-trow-main">
            <span class="mac-trow-line"><span class="mac-ellipsis mac-trow-label">${row.label}</span><b class="mac-num">${row.tokens_label} tokens</b></span>
            <span class="mac-trow-line mac-trow-sub">
              <span>${plural(row.threads ?? 0, 'thread')} · ${plural(row.turns ?? 0, 'turn')}${row.failed > 0 && html` · <span class="failed">${row.failed} failed</span>`}</span>
              <span class="mac-num">${row.cost}</span>
            </span>
          </span>
        </div>`,
      )}
    </div>
  </section>`;
}

/** Nothing done in the range yet, and New thread. */
function NoActivity({ range }) {
  const state = app.useApp();
  const text = { week: 'No activity this week.', all: 'No activity yet.' }[range] || 'No activity today.';
  return html`<div class="mac-card mac-none">
    <span class="muted">${text}</span>
    ${state.status === 'online' && state.projects.length > 0 && html`<button class="btn small" type="button" onClick=${() => app.go({ screen: 'new' })}><${Icon} name="plus" size="s" />New thread</button>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// A computer from before `basecamp_v2`

function OlderRecap({ recap }) {
  const rows = recap.review || [];
  return html`
    ${rows.length > 0 && html`<section><${Title} text="Ready for review" count=${rows.length} /><${ReviewList} rows=${rows} showsAgent=${true} /></section>`}
    ${recap.empty
      ? html`<div class="mac-card"><p class="mac-narrative">${recap.invitation || 'Nothing on the trail yet — start a thread.'}</p></div>`
      : html`<section class="mac-card">
          <h3 class="mac-card-title">${recap.title}</h3>
          <p class="mac-narrative">${(recap.narrative || []).map((span) => (span.kind === 'text' ? span.text : html`<b>${span.text}</b>`))}</p>
        </section>`}
    ${(recap.tiles || []).length > 0 &&
    html`<div class="mac-tiles">
      ${recap.tiles.map(
        (tile) => html`<button type="button" class="mac-card mac-tile" disabled=${tile.kind !== 'plan_left'} onClick=${() => app.go({ screen: 'usage' })}>
          <span class="muted">${tile.label}</span><b>${tile.figure}</b><span class="muted">${tile.note}</span>
        </button>`,
      )}
    </div>`}
  `;
}
