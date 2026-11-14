// What a thread shows over its composer besides the requests waiting on you, as the iPhone's bars
// do (`ComposerNotes.swift`, `ComposerBars.swift`, `BackgroundStrip.swift`, `QueuedStrip.swift`,
// `SubAgentsCard.swift`): the limit bar of a thread paused at its agent's usage limit, the offer to
// carry on a turn that Trek's closing cut short, a pull request's failing checks, what the agent
// left running in the background, the follow-ups waiting to go, its sub-agents, another thread at
// work in the same folder, a move to another agent waiting for the next message, and the Compact
// chip. Each shows only where the computer serves it (`features`) and keeps to a line or two: a
// phone has little room over its keyboard. The thread's menu gains its side chat and what fills
// its context.

import { render } from '../../vendor/preact.mjs';
import { useEffect, useLayoutEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Menu, Sheet, Spinner, addIcons, clock, plural, took } from '../ui.js';
import * as app from '../model.js';
import * as routes from '../routes.js';
import { registerMenuItems, registerStrip } from '../routes.js';

// Lucide (ISC licence), 24 × 24, stroked, as the rest of the remote's icons.
addIcons({
  'te-clock-alert': '<path d="M12 6v6l4 2"/><path d="M16 21.16a10 10 0 1 1 5-13.52"/><path d="M20 11.5v4"/><path d="M20 19.5h.01"/>',
  'te-pause': '<circle cx="12" cy="12" r="10"/><line x1="10" x2="10" y1="15" y2="9"/><line x1="14" x2="14" y1="15" y2="9"/>',
  'te-x-circle': '<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>',
  'te-switch': '<path d="M8 3 4 7l4 4"/><path d="M4 7h16"/><path d="m16 21 4-4-4-4"/><path d="M20 17H4"/>',
  'te-shrink': '<polyline points="4 14 10 14 10 20"/><polyline points="20 10 14 10 14 4"/><line x1="14" x2="21" y1="10" y2="3"/><line x1="3" x2="10" y1="21" y2="14"/>',
  'te-info': '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  'te-chat': '<path d="M14 9a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z"/><path d="M18 9h2a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1"/>',
  'te-up': '<path d="m18 15-6-6-6 6"/>',
  'te-dots': '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
  'te-box': '<rect width="18" height="18" x="3" y="3" rx="2"/>',
  'te-box-check': '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="m9 12 2 2 4-4"/>',
});

// ---------------------------------------------------------------------------------------------
// Words, as the iPhone and the Mac say them

/** Tokens as Trek shortens them: 950, 12.3K, 171K, 1.2M. */
export function tokens(n) {
  const v = Math.max(0, Math.round(n || 0));
  if (v < 1000) return String(v);
  if (v < 10_000) return `${(v / 1000).toFixed(1).replace(/\.0$/, '')}K`;
  if (v < 1_000_000) return `${Math.floor(v / 1000)}K`;
  return `${(v / 1e6).toFixed(1).replace(/\.0$/, '')}M`;
}

/** The limit bar's line: "Resumes at 4:30 PM (in 2h 14m)", "Resets 4:30 PM (in 2h 14m)", "Reset time unknown". */
export function limitLine(pause, now = Date.now(), at = clock) {
  if (pause?.resets_at == null) return 'Reset time unknown';
  const left = Math.max(0, Math.round((pause.resets_at - now) / 1000));
  const when = left > 0 ? `${at(pause.resets_at)} (in ${took(left)})` : at(pause.resets_at);
  return pause.resume ? `Resumes at ${when}` : `Resets ${when}`;
}

/** The interrupted bar's words: its title, what it says under it, and its button. */
export function cutWords(cut) {
  switch (cut?.state) {
    case 'resuming':
      return { title: 'Resuming…', detail: cut.how || '', verb: null };
    case 'failed':
      return { title: 'Couldn’t resume', detail: cut.why || cut.how || '', verb: 'Retry' };
    default:
      return cut?.waiting
        ? { title: 'Waiting for you when Trek closed', detail: cut.asked || 'Its question went with Trek. Ask again to bring it back.', verb: 'Ask again' }
        : { title: 'Interrupted', detail: cut?.how || '', verb: 'Resume' };
  }
}

/** Trek's tries at sending failing checks by itself before it stops (`pr_checks::AUTO_TRIES`). */
const AUTO_TRIES = 2;

/** The failing-checks bar's title, and "#128: clippy, test (macos)" beside it. */
export function checksWords(checks) {
  const pr = checks?.number != null ? `#${checks.number}: ` : '';
  const names = (checks?.failing || []).join(', ');
  return {
    title: checks?.stopped ? 'CI still failing' : 'CI failed',
    detail: checks?.stopped ? `${pr}${names}. Trek stopped sending it after ${AUTO_TRIES} tries.` : `${pr}${names}`,
  };
}

/** What the switch bar says, as the Mac's `switch_bar::sentence` with room to say it whole. */
export function switchSentence(to, running) {
  return [`Your next message switches to ${to}.`, running ? 'It stops this turn, and the conversation so far goes with it.' : 'The conversation so far goes with it.'];
}

/** The queued follow-ups with each one's place among those waiting for the same thing: `index` in `queue_action`. */
export function queuedPlaces(queued) {
  let turn = 0;
  let reset = 0;
  return (queued || []).map((q) => ({ q, index: q.reset ? reset++ : turn++ }));
}

/** A queued message's line: its first line, or its photos when it has no words. */
export function queuedPreview(q) {
  const first = (q?.text || '').split('\n').find((l) => l.trim()) || '';
  if (first) return first.trim();
  return plural(q?.images || 0, 'image');
}

/** What the agent left running: the row's tasks, or from a computer that only names them, their titles. */
export function backgroundWork(row) {
  if (row?.background_tasks?.length) return row.background_tasks;
  return (row?.background || []).map((title, n) => ({ id: `title:${n}`, kind: 'shell', title }));
}

/** How a background task's kind is named, as the Mac's strip names it. */
const TASK_KIND = { shell: 'Command', monitor: 'Watching', other: 'Task' };

/** A sub-agent's state as its row says it, and the colour it says it in. */
export const SUB_STATE = {
  running: ['Working', 'working'],
  needs_you: ['Needs you', 'approval'],
  done: ['Done', 'done'],
  failed: ['Failed', 'failed'],
  stopped: ['Stopped', 'muted'],
};

/** Still at it: working, or waiting on you. */
export const isLive = (state) => state === 'running' || state === 'needs_you';

/** The sub-agents' header: how many work, wait on you or failed, and since when the first still at it started. */
export function subAgentCounts(agents) {
  let working = 0;
  let waiting = 0;
  let failed = 0;
  let since = null;
  for (const s of agents || []) {
    if (s.state === 'running') working += 1;
    else if (s.state === 'needs_you') waiting += 1;
    else if (s.state === 'failed') failed += 1;
    if (isLive(s.state) && s.since != null && (since == null || s.since < since)) since = s.since;
  }
  return { working, waiting, failed, since };
}

/** "2m 14s" since `ms`. */
const since = (ms, now = Date.now()) => took(Math.max(0, (now - ms) / 1000));

/** One line of a task's command, however it was written. */
const oneLine = (text) => (text || '').replace(/\s+/g, ' ').trim();

// ---------------------------------------------------------------------------------------------
// Shared parts

/**
 * What a strip opens (a sheet, a menu), drawn at the end of the page rather than in place: the
 * composer's blur makes it the box anything fixed is placed in, which would shut a sheet inside it.
 */
export function Layer({ children }) {
  const box = useRef(null);
  if (!box.current) box.current = document.createElement('div');
  useLayoutEffect(() => {
    const el = box.current;
    document.body.appendChild(el);
    return () => {
      render(null, el);
      el.remove();
    };
  }, []);
  useLayoutEffect(() => {
    render(children, box.current);
  });
  return null;
}

/** A question before something that can't be taken back. */
export function Ask({ title, message, verb, onYes, onClose }) {
  return html`<${Sheet} title=${title} onClose=${onClose}>
    ${message && html`<p class="te-ask-msg">${message}</p>`}
    <div class="te-ask-btns">
      <button class="btn" type="button" onClick=${onClose}>Cancel</button>
      <button
        class="btn danger"
        type="button"
        onClick=${() => {
          onClose();
          onYes();
        }}
      >
        ${verb}
      </button>
    </div>
  <//>`;
}

/** Redraws every `ms` while it's asked to (clocks that run between the computer's updates). */
function useTick(ms) {
  const [, set] = useState(0);
  useEffect(() => {
    if (!ms) return undefined;
    const timer = setInterval(() => set((n) => n + 1), ms);
    return () => clearInterval(timer);
  }, [ms]);
}

/** A `thread_action`: `done` is said once the computer takes it, or why it didn't. True when it did. */
async function act(threadId, action, done) {
  try {
    await app.ask({ type: 'thread_action', thread_id: threadId, action });
    if (done) app.toast(done);
    return true;
  } catch (err) {
    app.toast(err.message, 'negative');
    return false;
  }
}

const REQUESTS = new Set(['approval', 'question', 'plan']);

/** The thread waits on an answer of its own (pinned over the composer): bars that would crowd it wait. */
function asking(threadId) {
  return (app.state.transcripts.get(threadId)?.items || []).some((i) => REQUESTS.has(i.kind) && i.state === 'pending');
}

/** A field as a key: a bar dismissed here stays gone while the computer still says the same. */
const keyOf = (value) => JSON.stringify(value ?? null);

/** What the composer holds now (it keeps its draft as it's typed). */
function draftOf(threadId) {
  try {
    return sessionStorage.getItem(`trek-remote-draft:${threadId}`) || '';
  } catch {
    return '';
  }
}

/** A message given back (a queued one not sent): into the composer, after what's there. */
function giveBack(threadId, text) {
  if (!text) return;
  const draft = draftOf(threadId);
  app.setDraft(threadId, draft.trim() ? `${draft}\n\n${text}` : text);
}

/** A bar's button: the strong one inked, the rest quiet; `short` is said where the bar is narrow. */
function Btn({ label, short, strong, onClick, href, disabled }) {
  const words = short ? html`<span class="te-long">${label}</span><span class="te-short">${short}</span>` : label;
  if (href) return html`<a class=${`te-btn ${strong ? 'strong' : ''}`} href=${href} target="_blank" rel="noopener noreferrer">${words}</a>`;
  return html`<button type="button" class=${`te-btn ${strong ? 'strong' : ''}`} disabled=${disabled} onClick=${onClick}>${words}</button>`;
}

/** A bar over the composer: what happened, a line under it, and what to do about it. */
function Bar({ tone = '', icon, spin, title, detail, buttons = [], label }) {
  const shown = buttons.filter(Boolean);
  return html`<section class=${`te-bar ${tone}`} aria-label=${label || title}>
    <div class="te-head">
      ${spin ? html`<${Spinner} />` : html`<span class=${`te-glyph ${tone}`}><${Icon} name=${icon} size="s" /></span>`}
      <div class="te-words">
        <div class="te-title">${title}</div>
        ${detail && html`<div class="te-detail">${detail}</div>`}
      </div>
    </div>
    ${shown.length > 0 && html`<div class="te-btns">${shown}</div>`}
  </section>`;
}

// ---------------------------------------------------------------------------------------------
// The limit bar (`pause`, feature `limit_actions`; "Continue on …" with `continue_on_account`)

function LimitBar({ row, threadId }) {
  const pause = row.pause;
  const shows = !!pause && row.run_state !== 'working';
  const [picking, setPicking] = useState(false);
  useTick(shows && pause.resets_at != null ? 1000 : 0);
  if (!shows) return null;
  const other = app.has('continue_on_account') ? pause.other_account || null : null;
  const buttons = [];
  if (app.has('limit_actions')) {
    if (other) {
      const account = other.account ? { account: other.account } : {};
      buttons.push(html`<${Btn} strong label=${`Continue on ${other.label}`} onClick=${() => act(threadId, { kind: 'continue_on_account', ...account }, `Carrying on with ${other.label}`)} />`);
    }
    if (pause.resets_at != null) {
      buttons.push(
        pause.resume
          ? html`<${Btn} label="Cancel" onClick=${() => act(threadId, { kind: 'cancel_resume' })} />`
          : html`<${Btn} strong=${!other} label="Resume at reset" onClick=${() => act(threadId, { kind: 'resume_at_reset' }, 'Resumes at the reset')} />`,
      );
      if (!row.snoozed_until && !other) buttons.push(html`<${Btn} label="Snooze until reset" short="Snooze" onClick=${() => act(threadId, { kind: 'snooze_until_reset' }, 'Snoozed until the reset')} />`);
    } else {
      buttons.push(html`<${Btn} strong label="Try again" onClick=${() => act(threadId, { kind: 'try_again' })} />`);
    }
    buttons.push(other ? html`<${Btn} label="Switch…" onClick=${() => setPicking(true)} />` : html`<${Btn} label="Switch agent…" short="Switch…" onClick=${() => setPicking(true)} />`);
  }
  return html`<${Bar}
      tone="approval"
      icon=${pause.wrapped ? 'te-pause' : 'te-clock-alert'}
      title=${pause.wrapped ? 'Wrapped up before the limit' : 'Usage limit reached'}
      detail=${limitLine(pause)}
      buttons=${buttons}
    />
    ${picking && html`<${Layer}><${SwitchTo} row=${row} threadId=${threadId} onClose=${() => setPicking(false)} /><//>`}`;
}

/**
 * The limit bar's "Switch agent…": another agent, or another of this one's models. A computer that
 * holds a move for the next message (`switch_preview`) holds it, as its own composer does.
 */
function SwitchTo({ row, threadId, onClose }) {
  const pick = async (agent, model) => {
    const same = agent.key === row.agent?.key;
    const prefs = same ? { model: model?.id } : { agent: agent.key, ...(model ? { model: model.id } : {}), ...(app.has('switch_preview') ? { hold: true } : {}) };
    try {
      await app.ask({ type: 'set_prefs', thread_id: threadId, ...prefs });
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  const items = [];
  for (const agent of app.state.agents) {
    for (const model of agent.models?.length ? agent.models : [null]) {
      if (agent.key === row.agent?.key && (model ? model.id === row.model : true)) continue;
      items.push({ label: model ? `${agent.name} · ${model.label}` : agent.name, icon: 'bot', onClick: () => pick(agent, model) });
    }
  }
  return html`<${Menu} title="Switch to" items=${items} onClose=${onClose} />`;
}

// ---------------------------------------------------------------------------------------------
// The interrupted bar (`cut_short`, feature `resume_interrupted`)

function CutShortBar({ row, threadId }) {
  const cut = row.cut_short;
  const [resuming, setResuming] = useState(null);
  const [gone, setGone] = useState(null);
  if (!cut || row.run_state === 'working' || !app.has('resume_interrupted')) return null;
  const key = keyOf(cut);
  if (gone === key) return null;
  // Resuming said at once, so a second tap can't be taken for another resume.
  const shown = resuming === key ? { ...cut, state: 'resuming' } : cut;
  const words = cutWords(shown);
  const resume = async () => {
    setResuming(key);
    if (!(await act(threadId, { kind: 'resume_interrupted' }))) setResuming(null);
  };
  const dismiss = async () => {
    setGone(key);
    if (!(await act(threadId, { kind: 'dismiss_interrupted' }))) setGone(null);
  };
  const tone = shown.state === 'failed' ? 'failed' : shown.state === 'resuming' ? '' : 'approval';
  return html`<${Bar}
    tone=${tone}
    icon=${shown.state === 'failed' ? 'alert' : 'zap'}
    spin=${shown.state === 'resuming'}
    title=${words.title}
    detail=${words.detail}
    buttons=${words.verb ? [html`<${Btn} strong label=${words.verb} onClick=${resume} />`, html`<${Btn} label="Dismiss" onClick=${dismiss} />`] : []}
  />`;
}

// ---------------------------------------------------------------------------------------------
// The failing-checks bar (`checks`, feature `failed_checks`)

function ChecksBar({ row, threadId }) {
  const checks = row.checks;
  const [gone, setGone] = useState(null);
  if (!checks?.bar || !app.has('failed_checks') || asking(threadId)) return null;
  const key = keyOf(checks);
  if (gone === key) return null;
  const words = checksWords(checks);
  const dismiss = async () => {
    setGone(key);
    if (!(await act(threadId, { kind: 'dismiss_failed_checks' }))) setGone(null);
  };
  const buttons = checks.sending
    ? []
    : [
        html`<${Btn} strong label="Send to agent" onClick=${() => act(threadId, { kind: 'send_failed_checks' }, `Sent to ${row.agent?.name || 'the agent'}`)} />`,
        checks.url && /^https?:\/\//.test(checks.url) && html`<${Btn} label="View on GitHub" short="GitHub" href=${checks.url} />`,
        html`<${Btn} label="Dismiss" onClick=${dismiss} />`,
      ];
  return html`<${Bar} tone="failed" icon="te-x-circle" spin=${checks.sending} title=${words.title} detail=${checks.sending ? 'Reading the log…' : words.detail} buttons=${buttons} />`;
}

// ---------------------------------------------------------------------------------------------
// What the agent runs in the background (`background_tasks`, feature `background`)

function BackgroundStrip({ row, threadId }) {
  const tasks = backgroundWork(row);
  const [all, setAll] = useState(false);
  const [reading, setReading] = useState(null);
  const [stopping, setStopping] = useState(null);
  useTick(tasks.length ? 1000 : 0);
  if (!tasks.length) return null;
  const served = app.has('background') && (row.background_tasks || []).length > 0;
  const shown = all && tasks.length > 1 ? tasks : tasks.slice(0, 1);
  const agent = row.agent?.name || 'The agent';
  const stop = async (task) => {
    try {
      await app.ask({ type: 'stop_background', thread_id: threadId, task_id: task.id });
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  return html`<section class="te-list" aria-label="In the background">
      ${shown.map((task, n) => {
        const more = n === 0 ? tasks.length - 1 : 0;
        const line = oneLine(task.line);
        return html`<div class="te-row" key=${task.id}>
          <button type="button" class="te-row-main" disabled=${!served} onClick=${() => setReading(task.id)} aria-label=${served ? `${oneLine(task.title)}: shows its output` : oneLine(task.title)}>
            <span class="te-row-lead"><${Spinner} /></span>
            <span class="te-row-text">
              <span class="te-row-title mono">${oneLine(task.title)}</span>
              <span class="te-row-sub">${task.stopping ? 'Stopping…' : 'Running'}${!task.stopping && task.since ? ` ${since(task.since)}` : ''}${line ? html` · <span class="mono">${line}</span>` : ''}</span>
            </span>
          </button>
          ${more > 0 &&
          html`<button type="button" class="te-pill" aria-label=${all ? 'Show one' : `Show all ${more + 1}`} aria-expanded=${all ? 'true' : 'false'} onClick=${() => setAll(!all)}>
            ${!all && `+${more}`}<${Icon} name=${all ? 'down' : 'te-up'} size="xs" />
          </button>`}
          ${served &&
          task.stoppable &&
          html`<button type="button" class="te-stop" disabled=${task.stopping} aria-label=${task.stopping ? 'Stopping' : `Stop ${oneLine(task.title)}`} onClick=${() => setStopping(task)}>
            <span><${Icon} name="stop" size="xs" /></span>
          </button>`}
        </div>`;
      })}
    </section>
    ${reading &&
    html`<${Layer}><${TaskSheet}
      threadId=${threadId}
      taskId=${reading}
      onClose=${() => setReading(null)}
      onStop=${(task) => {
        setReading(null);
        setStopping(task);
      }}
    /><//>`}
    ${stopping && html`<${Layer}><${Ask} title="Stop this task?" message=${`${agent} stops it on your computer.`} verb="Stop" onYes=${() => stop(stopping)} onClose=${() => setStopping(null)} /><//>`}`;
}

/** One background task: the end of its output, read every couple of seconds while it shows, as the Mac's popover does. */
function TaskSheet({ threadId, taskId, onClose, onStop }) {
  app.useApp();
  const row = app.row(threadId);
  const task = row?.background_tasks?.find((t) => t.id === taskId) || null;
  const [output, setOutput] = useState(null);
  const [read, setRead] = useState(false);
  const pre = useRef(null);
  useTick(task ? 1000 : 0);
  useEffect(() => {
    let done = false;
    let timer = null;
    const next = async () => {
      if (!app.row(threadId)?.background_tasks?.some((t) => t.id === taskId)) return;
      try {
        const reply = await app.ask({ type: 'background_output', thread_id: threadId, task_id: taskId });
        if (done) return;
        setOutput(reply.output ?? '');
        setRead(true);
      } catch {
        if (done) return;
        setRead(true);
      }
      timer = setTimeout(next, 2000);
    };
    next();
    return () => {
      done = true;
      clearTimeout(timer);
    };
  }, [threadId, taskId]);
  useLayoutEffect(() => {
    if (pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [output]);
  const agent = row?.agent?.name || 'The agent';
  const printed = (output || '').trim();
  const note = !task
    ? 'It isn’t running any more.'
    : !task.readable
      ? `${agent} doesn’t share this task’s output while it runs.`
      : printed
        ? 'The end of its output, read every few seconds.'
        : read
          ? 'Nothing printed yet.'
          : 'Reading its output…';
  const kind = TASK_KIND[task?.kind] || 'Task';
  return html`<${Sheet}
    title=${task ? kind : 'Background'}
    onClose=${onClose}
    actions=${task?.stoppable &&
    html`<button class="btn small danger" type="button" disabled=${task.stopping} onClick=${() => onStop(task)}>${task.stopping ? 'Stopping…' : 'Stop'}</button>`}
  >
    ${task &&
    html`<div class="te-task-meta"><${Icon} name="terminal" size="xs" /> ${task.stopping ? 'Stopping…' : `Running${task.since ? ` ${since(task.since)}` : ''}`}</div>
      <div class="te-task-title">${task.title}</div>`}
    ${printed && html`<pre class="te-output" ref=${pre}>${output}</pre>`}
    <p class="te-note">${note}</p>
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// The follow-ups waiting to go (`queued`, feature `queue_action`)

function QueuedStrip({ row, threadId }) {
  const queued = row.queued || [];
  const [menu, setMenu] = useState(null);
  const [editing, setEditing] = useState(null);
  if (!queued.length || !app.has('queue_action')) return null;
  const working = row.run_state === 'working';
  const ask = (place, action) => app.ask({ type: 'queue_action', thread_id: threadId, index: place.index, ...(place.q.reset ? { reset: true } : {}), text: place.q.text, action });
  const drop = async (place) => {
    try {
      const ack = await ask(place, { kind: 'remove' });
      giveBack(threadId, ack.text);
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  // Now: out of the queue and into the running turn.
  const sendNow = async (place) => {
    let text;
    try {
      text = (await ask(place, { kind: 'remove' })).text ?? place.q.text;
    } catch (err) {
      app.toast(err.message, 'negative');
      return;
    }
    try {
      await app.ask({ type: 'send', thread_id: threadId, text, mode: 'steer' }, { timeout: 120_000 });
    } catch (err) {
      app.toast(err.message, 'negative');
      giveBack(threadId, text);
    }
  };
  const save = async (place, text) => {
    try {
      await ask(place, { kind: 'edit', text });
      app.toast('Changed');
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  return html`<section class="te-list" aria-label="Queued messages">
      ${queuedPlaces(queued).map(
        (place, n) => html`<div class="te-row" key=${`${n}:${place.q.text}`}>
          <div class="te-row-main">
            <span class="te-row-lead"><${Icon} name="clock" size="s" /></span>
            <span class="te-row-text">
              <span class="te-row-title">${queuedPreview(place.q)}</span>
              <span class="te-row-sub">${place.q.reset ? 'Sends when your limit resets' : 'Sends after this turn'}</span>
            </span>
          </div>
          <button type="button" class="te-icon" aria-label="Queued message actions" onClick=${() => setMenu(place)}><${Icon} name="te-dots" size="s" /></button>
        </div>`,
      )}
    </section>
    ${menu &&
    html`<${Layer}><${Menu}
      title="Queued message"
      onClose=${() => setMenu(null)}
      items=${[
        { label: 'Edit', icon: 'pencil', onClick: () => setEditing(menu) },
        { label: 'Send now', icon: 'send', hidden: !working || menu.q.reset || menu.q.images > 0, onClick: () => sendNow(menu) },
        { label: 'Don’t send', icon: 'undo', onClick: () => drop(menu) },
      ]}
    /><//>`}
    ${editing && html`<${Layer}><${EditQueued} q=${editing.q} onSave=${(text) => save(editing, text)} onClose=${() => setEditing(null)} /><//>`}`;
}

/** A queued message's words, changed before it goes (its photos stay). */
function EditQueued({ q, onSave, onClose }) {
  const [text, setText] = useState(q.text || '');
  return html`<${Sheet} title="Queued message" onClose=${onClose}>
    <p class="te-sheet-sub">${q.reset ? 'Sends when your limit resets' : 'Sends after this turn'}</p>
    <textarea class="input te-edit" rows="5" aria-label="Message" value=${text} onInput=${(e) => setText(e.target.value)} autofocus></textarea>
    <div class="te-ask-btns">
      <button class="btn" type="button" onClick=${onClose}>Cancel</button>
      <button
        class="btn primary"
        type="button"
        disabled=${!text.trim()}
        onClick=${() => {
          onClose();
          onSave(text);
        }}
      >
        Save
      </button>
    </div>
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// Sub-agents (`sub_agents`; opening one with `sub_agents` or `sub_agent_view`)

function SubAgentsStrip({ row, threadId }) {
  const agents = row.sub_agents || [];
  const [open, setOpen] = useState(false);
  const live = agents.some((s) => isLive(s.state));
  useTick(live ? 1000 : 0);
  // While its turn runs, or while what it started is still at it, as the iPhone's card.
  if (!agents.length || !(live || row.run_state === 'working')) return null;
  const now = Date.now();
  const counts = subAgentCounts(agents);
  const opens = (s) => (s.thread_id && app.has('sub_agents') ? 'thread' : s.call_id && app.has('sub_agent_view') ? 'view' : null);
  const go = (s) => {
    const how = opens(s);
    if (how === 'thread') app.go({ screen: 'thread', threadId: s.thread_id });
    else if (how === 'view') openSubAgent(threadId, s.call_id, { agent: s.agent, label: s.title, state: s.state, since: s.since });
  };
  return html`<section class="te-list" aria-label="Sub-agents">
    <button type="button" class="te-row-main te-subs-head" aria-expanded=${open ? 'true' : 'false'} onClick=${() => setOpen(!open)}>
      <span class="te-row-lead"><${Icon} name="bot" size="s" /></span>
      <span class="te-row-text">
        <span class="te-row-title">${plural(agents.length, 'sub-agent')}</span>
        <span class="te-row-sub te-counts">
          ${counts.working > 0 && html`<span class="te-working">${counts.working} working</span>`}
          ${counts.waiting > 0 && html`<span class="te-approval">${counts.waiting} need${counts.waiting === 1 ? 's' : ''} you</span>`}
          ${counts.failed > 0 && html`<span class="te-failed">${counts.failed} failed</span>`}
          ${counts.working + counts.waiting + counts.failed === 0 && html`<span class="te-done">All done</span>`}
        </span>
      </span>
      ${counts.since != null && html`<span class="te-time">${since(counts.since, now)}</span>`}
      <span class="te-chev"><${Icon} name=${open ? 'down' : 'te-up'} size="xs" /></span>
    </button>
    ${open &&
    agents.map((s, i) => {
      const [label, tone] = SUB_STATE[s.state] || [s.state, 'muted'];
      const how = opens(s);
      return html`<button type="button" class="te-row-main te-sub" key=${s.thread_id || s.call_id || i} disabled=${!how} onClick=${() => go(s)}>
        <span class="te-row-lead"><span class=${`te-sdot te-${tone} ${s.state === 'running' ? 'pulse' : ''}`}></span></span>
        <span class="te-row-text">
          <span class="te-row-title">${s.title}</span>
          <span class="te-row-sub">${s.model ? `${s.agent?.name || 'Agent'} · ${s.model}` : s.agent?.name || 'Agent'}</span>
        </span>
        <span class=${`te-state te-${tone}`}>${label}</span>
        ${isLive(s.state) && s.since != null && html`<span class="te-time">${since(s.since, now)}</span>`}
        <span class="te-chev">${how && html`<${Icon} name="forward" size="xs" />`}</span>
      </button>`;
    })}
  </section>`;
}

/** Opens one of the agent's own sub-agents over its thread (`sub_agent_view`): from its row in the strip, or anywhere that names one. */
export function openSubAgent(threadId, callId, link) {
  show({ kind: 'sub-agent', threadId, callId, link });
}

/** The transcript's own parts, loaded when a sub-agent's steps are first drawn (so these words can be tried in Node, where markdown can't load). */
let parts = null;
function loadParts() {
  parts ??= Promise.all([import('../items.js'), import('../markdown.js')]).then(([items, md]) => ({ Item: items.Item, Markdown: md.Markdown }));
  return parts;
}

/**
 * One of the agent's own sub-agents (Claude Code's Task, Codex's agents): what it was told, what it
 * said and called, asked for again every second and a half while it works and this shows, its
 * answer once done. Stop where its agent can stop it, a message where it takes one, and where it
 * doesn't, the computer's words for why. What it asks for waits on its parent's thread.
 */
function SubAgentSheet({ threadId, callId, link: first, onClose }) {
  const [log, setLog] = useState(null);
  const [failure, setFailure] = useState(null);
  const [draw, setDraw] = useState(null);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [again, setAgain] = useState(0);
  const end = useRef(null);
  useTick(isLive((log?.link || first)?.state) ? 1000 : 0);
  useEffect(() => {
    loadParts().then(setDraw);
  }, []);
  useEffect(() => {
    let done = false;
    let timer = null;
    let last = null;
    const next = async () => {
      let keeps = true;
      try {
        const reply = await app.ask({ type: 'sub_agent', thread_id: threadId, call_id: callId });
        if (done) return;
        last = reply;
        setLog(reply);
        setFailure(null);
        keeps = isLive(reply.link?.state) && !reply.gone;
      } catch (err) {
        if (done) return;
        setFailure(err.message);
        // Asked again later: a computer out of reach a moment may come back.
        keeps = last ? isLive(last.link?.state) && !last.gone : true;
      }
      if (keeps && !done) timer = setTimeout(next, 1500);
    };
    next();
    return () => {
      done = true;
      clearTimeout(timer);
    };
  }, [threadId, callId, again]);
  useLayoutEffect(() => {
    end.current?.scrollIntoView({ block: 'nearest' });
  }, [log?.items?.length]);
  const link = log?.link || first || {};
  const agent = link.agent?.name || 'The agent';
  const [label, tone] = SUB_STATE[link.state] || ['', 'muted'];
  const time = isLive(link.state) && link.since != null ? since(link.since) : link.took_secs != null ? took(link.took_secs) : null;
  const steer = async (action, text) => {
    setSending(true);
    try {
      const reply = await app.ask({ type: 'sub_agent_action', thread_id: threadId, call_id: callId, action, ...(text ? { text } : {}) });
      setLog(reply);
      // A message it took: its reply comes in as it works.
      if (isLive(reply.link?.state) && !reply.gone) setAgain((n) => n + 1);
      return true;
    } catch (err) {
      app.toast(err.message, 'negative');
      return false;
    } finally {
      setSending(false);
    }
  };
  const send = async () => {
    const said = draft.trim();
    if (!said || sending) return;
    setDraft('');
    if (!(await steer('message', said))) setDraft(said);
  };
  const live = log && isLive(log.link?.state) && !log.gone;
  return html`<${Sheet}
      title="Sub-agent"
      onClose=${onClose}
      actions=${log?.can_stop && html`<button class="btn small danger" type="button" disabled=${sending} onClick=${() => setStopping(true)}>Stop</button>`}
    >
      <div class="te-agent-head">
        <div class="te-title">${link.label || 'Sub-agent'}</div>
        <div class="te-detail">${label && html`<span class=${`te-${tone}`}>${label}</span>`}${label && ' · '}${agent}${time && ` · ${time}`}</div>
      </div>
      ${!log && !failure && html`<div class="empty"><${Spinner} /></div>`}
      ${!log && failure && html`<p class="te-note">${failure}</p>`}
      ${log &&
      html`${log.prompt && html`<div class="te-label">Told to</div><div class="te-quote">${log.prompt}</div>`}
        ${log.asking &&
        html`<div class="te-asking">
          <${Icon} name="question" size="s" /><span class="grow">A request waits on the thread it works for. Answer it there.</span>
          <button class="btn small" type="button" onClick=${onClose}>Go to thread</button>
        </div>`}
        ${log.earlier > 0 && html`<p class="te-note">${plural(log.earlier, 'earlier step')} aren’t kept on the computer.</p>`}
        ${log.gone
          ? html`<p class="te-note">Your computer no longer has its steps: its session ended, or Trek restarted since.</p>`
          : (log.items || []).length === 0 && isLive(log.link?.state) && html`<p class="te-note">Nothing yet: its steps show here as it takes them.</p>`}
        <div class="te-steps">
          ${draw &&
          (log.items || []).map((it) =>
            it.kind === 'user'
              ? html`<div class="msg-user" key=${it.id}>${it.text}</div>`
              : it.kind === 'assistant' || it.kind === 'tool'
                ? html`<${draw.Item} key=${it.id} item=${it} threadId=${threadId} last=${false} />`
                : null,
          )}
        </div>
        ${log.answer && !isLive(log.link?.state) && draw && html`<div class="te-label">Its answer</div><${draw.Markdown} text=${log.answer} threadId=${threadId} />`}
        ${failure && html`<p class="te-note">${failure}</p>`}
        ${live &&
        (log.can_message
          ? html`<div class="te-say">
              <textarea class="input" rows="1" placeholder="Message the sub-agent" aria-label="Message the sub-agent" value=${draft} onInput=${(e) => setDraft(e.target.value)}></textarea>
              <button class="send" type="button" aria-label="Send to the sub-agent" disabled=${!draft.trim() || sending} onClick=${send}><${Icon} name="send" size="s" /></button>
            </div>`
          : log.no_message && html`<p class="te-note">${log.no_message}</p>`)}`}
      <div ref=${end}></div>
    <//>
    ${stopping &&
    html`<${Ask}
      title="Stop this sub-agent?"
      message=${`${agent} stops it on your computer. The thread it works for carries on.`}
      verb="Stop"
      onYes=${() => steer('stop')}
      onClose=${() => setStopping(false)}
    />`}`;
}

// ---------------------------------------------------------------------------------------------
// Another thread in the same folder (`shares_folder`)

function SharedFolder({ row }) {
  if (!row.shares_folder) return null;
  return html`<div class="te-line"><${Icon} name="alert" size="xs" /><span>Another thread is also editing this folder</span></div>`;
}

// ---------------------------------------------------------------------------------------------
// A move to another agent waiting for the next message (`pending_switch`, feature `switch_preview`)

function SwitchBar({ row, threadId }) {
  const pending = row.pending_switch;
  const [gone, setGone] = useState(null);
  const [previewing, setPreviewing] = useState(false);
  if (!pending || !app.has('switch_preview')) return null;
  const key = keyOf(pending);
  if (gone === key) return null;
  const [lead, rest] = switchSentence(pending.to, row.run_state === 'working');
  const stay = async () => {
    setGone(key);
    if (!(await act(threadId, { kind: 'stay_on_agent' }))) setGone(null);
  };
  return html`<${Bar}
      icon="te-switch"
      title=${lead}
      detail=${rest}
      buttons=${[html`<${Btn} strong label=${`Stay on ${pending.from}`} onClick=${stay} />`, html`<${Btn} label="What goes with it" onClick=${() => setPreviewing(true)} />`]}
    />
    ${previewing && html`<${Layer}><${SwitchPreview} threadId=${threadId} onClose=${() => setPreviewing(false)} /><//>`}`;
}

/** What the next message carries to the new agent, as the Mac's preview card shows it. */
function SwitchPreview({ threadId, onClose }) {
  const [preview, setPreview] = useState(null);
  const [failed, setFailed] = useState(null);
  useEffect(() => {
    let live = true;
    app.ask({ type: 'switch_preview', thread_id: threadId }).then(
      (p) => live && setPreview(p),
      (err) => live && setFailed(err.message),
    );
    return () => {
      live = false;
    };
  }, [threadId]);
  return html`<${Sheet} title="Goes with your next message" onClose=${onClose}>
    ${!preview && !failed && html`<div class="empty"><${Spinner} /></div>`}
    ${failed && html`<p class="te-note">${failed}</p>`}
    ${preview &&
    html`<p class="te-sheet-sub">${tokens(preview.tokens)} of ${tokens(preview.budget)} tokens</p>
      ${(preview.text || '').split(/\n\s*\n/).map((p, i) => html`<p class="te-para" key=${i}>${p}</p>`)}
      ${preview.left_out && html`<p class="te-note te-rule">${preview.left_out}</p>`}`}
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// The Compact chip (`compact_offer`, with `context_card` for compacting)

/** Chips dismissed, by thread: the `last_active` each was dismissed at (it's back once the thread is active again). */
const compactDismissed = new Map();

function CompactChip({ row, threadId }) {
  const offer = row.compact_offer;
  const [why, setWhy] = useState(false);
  const [, redraw] = useState(0);
  const eligible = !!offer && app.has('compact_offer') && app.has('context_card') && row.run_state !== 'working' && !asking(threadId);
  const lapsed = eligible && Date.now() >= offer.lapses_at;
  // From when the prompt cache goes.
  useEffect(() => {
    if (!eligible || lapsed) return undefined;
    const timer = setTimeout(() => redraw((n) => n + 1), Math.min(offer.lapses_at - Date.now() + 50, 2 ** 31 - 1));
    return () => clearTimeout(timer);
  }, [eligible, lapsed, offer?.lapses_at]);
  if (!lapsed || compactDismissed.get(threadId) === offer.last_active) return null;
  const dismiss = () => {
    compactDismissed.set(threadId, offer.last_active);
    redraw((n) => n + 1);
  };
  return html`<div class="te-chips">
    <div class="te-chip">
      <button
        type="button"
        class="te-chip-main"
        aria-label="Compact the conversation"
        onClick=${() => {
          dismiss();
          act(threadId, { kind: 'compact' }, 'Compacting');
        }}
      >
        <${Icon} name="te-shrink" size="xs" />Compact
      </button>
      <button type="button" class="te-chip-icon" aria-label="Why compact" aria-expanded=${why ? 'true' : 'false'} onClick=${() => setWhy(!why)}><${Icon} name="te-info" size="xs" /></button>
      <button type="button" class="te-chip-icon" aria-label="Dismiss" onClick=${dismiss}><${Icon} name="x" size="xs" /></button>
    </div>
    ${why && html`<div class="te-why">${offer.why}</div>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// What the thread's menu opens: its side chat, what fills its context, a sub-agent

const opened = { value: null, listeners: new Set() };

function show(value) {
  opened.value = value;
  for (const listener of opened.listeners) listener();
}

function useOpened() {
  const [, set] = useState(0);
  useEffect(() => {
    const listener = () => set((n) => n + 1);
    opened.listeners.add(listener);
    return () => opened.listeners.delete(listener);
  }, []);
  return opened.value;
}

/** Side chats opened from here, by id: the thread each is beside. */
const SIDES = 'trek-remote-side-chats';
const sides = (() => {
  try {
    return JSON.parse(sessionStorage.getItem(SIDES) || '{}') || {};
  } catch {
    return {};
  }
})();

function rememberSide(id, parent) {
  sides[id] = parent;
  try {
    sessionStorage.setItem(SIDES, JSON.stringify(sides));
  } catch {
    // Known for this visit only.
  }
}

/** Draws what's open over this thread, and in a side chat, which thread it's beside. */
function Host({ threadId }) {
  const o = useOpened();
  const close = () => show(null);
  // Left with a sheet open (Back): it doesn't open again on the way back.
  useEffect(
    () => () => {
      if (opened.value?.threadId === threadId) show(null);
    },
    [threadId],
  );
  const parent = sides[threadId];
  const beside = parent ? app.row(parent)?.title : null;
  let sheet = null;
  if (o?.threadId === threadId) {
    if (o.kind === 'context') sheet = html`<${ContextSheet} threadId=${threadId} onClose=${close} />`;
    else if (o.kind === 'side') sheet = html`<${SideChatSheet} threadId=${threadId} onClose=${close} />`;
    else if (o.kind === 'sub-agent') sheet = html`<${SubAgentSheet} threadId=${threadId} callId=${o.callId} link=${o.link} onClose=${close} />`;
  }
  return html`${parent && html`<div class="te-line side"><${Icon} name="te-chat" size="xs" /><span>Side chat beside “${beside || 'its thread'}”</span></div>`}
    ${sheet && html`<${Layer}>${sheet}<//>`}`;
}

/** The Mac's segment colours. */
const PART_COLOUR = { system: '#9A9AA2', mcp: '#A77BF0', skills: '#F0A13C', messages: '#5B92F2' };

const BILLING = { plan: 'Covered by a plan', metered: 'Billed per token', local: 'Runs on your computer: not billed' };

/** What fills the context window, as the Mac's context card shows it, and Compact now. */
function ContextSheet({ threadId, onClose }) {
  app.useApp();
  const row = app.row(threadId);
  const [card, setCard] = useState(null);
  const [failed, setFailed] = useState(null);
  const [compacting, setCompacting] = useState(false);
  const [part, setPart] = useState(null);
  const used = row?.context?.used;
  useEffect(() => {
    let live = true;
    app.ask({ type: 'context_card', thread_id: threadId }).then(
      (c) => {
        if (!live) return;
        setCard(c);
        setFailed(null);
        if (c.compact !== 'running') setCompacting(false);
      },
      (err) => live && setFailed(err.message),
    );
    return () => {
      live = false;
    };
  }, [threadId, used]);
  const context = row?.context;
  const cost = row?.cost;
  const percent = context?.percent ?? 0;
  const ring = percent >= 90 ? 'var(--failed)' : percent >= 75 ? 'var(--approval)' : 'color-mix(in srgb, var(--fg) 75%, transparent)';
  const filled = (card?.parts || []).filter((p) => p.kind !== 'free' && p.kind !== 'reserved');
  const free = card ? (card.parts || []).find((p) => p.kind === 'free') : null;
  const held = card ? ((card.parts || []).find((p) => p.kind === 'reserved')?.tokens ?? card.reserved ?? 0) : 0;
  const width = (n) => `${Math.max(0.6, (100 * n) / Math.max(1, card?.window || 1))}%`;
  const busy = card?.compact === 'busy';
  const running = compacting || card?.compact === 'running';
  const compact = async () => {
    setCompacting(true);
    if (!(await act(threadId, { kind: 'compact' }, 'Compacting'))) setCompacting(false);
  };
  const toggle = (kind) => setPart(part === kind ? null : kind);
  return html`<${Sheet} title="Context" onClose=${onClose}>
    ${context &&
    html`<div class="te-ctx-head">
      <svg class="te-ring" viewBox="0 0 36 36" aria-hidden="true">
        <circle class="track" cx="18" cy="18" r="15" />
        <circle cx="18" cy="18" r="15" style=${{ stroke: ring, strokeDasharray: `${(Math.min(100, Math.max(0, percent)) / 100) * 94.25} 94.25` }} />
      </svg>
      <div>
        <div class="te-title">${percent}% context used</div>
        <div class="te-detail">${tokens(context.used)} of ${tokens(context.window)} tokens</div>
      </div>
    </div>`}
    ${!card && !failed && html`<div class="empty"><${Spinner} /></div>`}
    ${failed && html`<p class="te-note">${failed}</p>`}
    ${card &&
    html`<div class="te-meter" aria-hidden="true">
        ${filled.map((p) => html`<span key=${p.kind} style=${{ width: width(p.tokens), background: PART_COLOUR[p.kind] || '#6BC4A6' }}></span>`)}
        <span class="te-meter-free"></span>
        ${held > 0 && html`<span class="te-meter-held" style=${{ width: width(held) }}></span>`}
      </div>
      ${filled.map((p) => html`<${PartRow} key=${p.kind} part=${p} swatch=${{ background: PART_COLOUR[p.kind] || '#6BC4A6' }} open=${part === p.kind} onToggle=${toggle} />`)}
      ${free && html`<${PartRow} part=${free} swatch=${{ border: '1px solid color-mix(in srgb, var(--fg) 35%, transparent)' }} open=${part === free.kind} onToggle=${toggle} />`}
      <p class="te-note">Estimated from what Trek sends the agent; the total is the agent’s own.</p>
      ${card.compact !== 'none' &&
      html`<button class="btn block te-compact" type="button" disabled=${busy || running} onClick=${compact}>
          ${running ? html`<${Spinner} />` : html`<${Icon} name="te-shrink" size="s" />`}${running ? 'Compacting…' : 'Compact now'}
        </button>
        ${busy && html`<p class="te-note">Compacting waits for the turn to finish.</p>`}`}`}
    ${cost &&
    html`<div class="te-cost">
      <div class="te-title">${cost.label}</div>
      ${BILLING[cost.billing] && html`<div class="te-detail">${BILLING[cost.billing]}</div>`}
      ${cost.plan && html`<div class="te-detail">${cost.plan}</div>`}
      ${cost.detail && html`<div class="te-detail">${cost.detail}</div>`}
      ${cost.cached && html`<div class="te-detail">${cost.cached}${cost.cached_detail ? `: ${cost.cached_detail}` : ''}</div>`}
    </div>`}
  <//>`;
}

/** A segment of the context bar, with what the Mac's flyout says about it a tap away. */
function PartRow({ part, swatch, open, onToggle }) {
  return html`<button type="button" class="te-part" aria-expanded=${open ? 'true' : 'false'} onClick=${() => onToggle(part.kind)}>
      <span class="te-swatch" style=${swatch}></span><span class="grow">${part.label}</span><span class="te-num">${tokens(part.tokens)}</span>
    </button>
    ${open &&
    html`<div class="te-part-more">
      ${(part.detail || []).map((d, i) => html`<div class="te-part-line" key=${i}><span class="grow">${d.label}</span>${d.value && html`<span class="te-num">${d.value}</span>`}</div>`)}
      ${part.note && html`<div class="te-part-note">${part.note}</div>`}
    </div>`}`;
}

/**
 * The thread's side chat (`side_chat`): a quick question beside it, in a session of its own that
 * its agent doesn't hear. The newest one there is opens; with none, the first message starts one.
 */
function SideChatSheet({ threadId, onClose }) {
  const row = app.row(threadId);
  const [phase, setPhase] = useState('finding');
  const [why, setWhy] = useState('');
  const [text, setText] = useState('');
  const [starting, setStarting] = useState(false);
  const open = (side) => {
    rememberSide(side.id, threadId);
    // Its row, as the reply carries it: the computer sends it again once the chat is open.
    if (!app.row(side.id)) app.receive({ type: 'unlisted', thread: side });
    onClose();
    app.go({ screen: 'thread', threadId: side.id });
  };
  useEffect(() => {
    let live = true;
    app.ask({ type: 'side_chat', thread_id: threadId }).then(
      (reply) => {
        if (!live) return;
        if (reply.side?.id) open(reply.side);
        else setPhase('draft');
      },
      (err) => {
        if (!live) return;
        setWhy(err.message);
        setPhase('gone');
      },
    );
    return () => {
      live = false;
    };
  }, [threadId]);
  const start = async () => {
    const said = text.trim();
    if (!said || starting) return;
    setStarting(true);
    try {
      const reply = await app.ask({ type: 'side_chat', thread_id: threadId, text: said }, { timeout: 120_000 });
      if (reply.side?.id) open(reply.side);
      else app.toast('The side chat didn’t start.', 'negative');
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setStarting(false);
    }
  };
  const agent = row?.agent?.name || 'The agent';
  const place = row?.project?.name ? ` in ${row.project.name}` : '';
  return html`<${Sheet} title="Side chat" onClose=${onClose}>
    ${row && html`<p class="te-sheet-sub">Beside “${row.title}”</p>`}
    ${phase === 'finding' && html`<div class="empty"><${Spinner} /></div>`}
    ${phase === 'gone' && html`<div class="te-side"><h3>No side chat</h3><p>${why}</p></div>`}
    ${phase === 'draft' &&
    html`<div class="te-side">
        <${Icon} name="te-chat" />
        <h3>Ask beside the thread</h3>
        <p>A side chat has a session of its own${place}, so ${agent} carries on in the thread and hears none of it. It asks before every edit and command.</p>
      </div>
      <div class="te-say">
        <textarea class="input" rows="2" placeholder=${`Ask ${agent}`} aria-label="Your question" value=${text} onInput=${(e) => setText(e.target.value)} autofocus></textarea>
        <button class="send" type="button" aria-label="Start the side chat" disabled=${!text.trim() || starting} onClick=${start}>${starting ? html`<${Spinner} />` : html`<${Icon} name="send" size="s" />`}</button>
      </div>`}
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// Where they go

// Over the composer, top to bottom.
registerStrip(LimitBar);
registerStrip(CutShortBar);
registerStrip(ChecksBar);
registerStrip(BackgroundStrip);
registerStrip(QueuedStrip);
registerStrip(SubAgentsStrip);
registerStrip(SharedFolder);
registerStrip(SwitchBar);
registerStrip(CompactChip);
registerStrip(Host);

// One of the agent's own sub-agents, for the transcript's rows that stand for one (a `tool` item's
// `sub_agent`): `({ threadId, callId, link, onClose })`.
routes.registerSheet?.('sub-agent', ({ threadId, callId, link, onClose }) => html`<${Layer}><${SubAgentSheet} threadId=${threadId} callId=${callId} link=${link} onClose=${onClose} /><//>`);

registerMenuItems(({ row, threadId, has }) => [
  // A side chat has none of its own.
  { label: 'Side chat', icon: 'te-chat', hidden: !has('side_chat') || !!sides[threadId], onClick: () => show({ kind: 'side', threadId }) },
  { label: 'Context', icon: 'gauge', hidden: !has('context_card') || !row?.context, onClick: () => show({ kind: 'context', threadId }) },
]);
