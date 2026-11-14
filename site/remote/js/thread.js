// A thread: its transcript, kept up to date while it's open, the requests that wait on you pinned
// over the composer (a sub-agent's too, answered from here as the Mac does), and the composer:
// a follow-up that steers the running turn or waits for it, Stop, and the thread's agent, model,
// effort, access and plan mode. The menu holds what the Mac's thread menu does.

import { useEffect, useLayoutEffect, useRef, useState } from '../vendor/hooks.mjs';
import { html, Icon, IconButton, Menu, Sheet, Spinner, Segmented, ProjectBadge, took } from './ui.js';
import { Item, Approval, Questions, Plan } from './items.js';
import * as app from './model.js';
import { composerPreviews, composerTools, threadParts } from './routes.js';

const REQUESTS = new Set(['approval', 'question', 'plan']);

export function ThreadScreen({ threadId }) {
  const state = app.useApp();
  const row = app.row(threadId);
  const t = state.transcripts.get(threadId);
  const [menu, setMenu] = useState(false);
  const [picker, setPicker] = useState(false);
  const scroller = useRef(null);
  const stuck = useRef(true);

  useEffect(() => {
    app.subscribe(threadId);
    return () => app.unsubscribe(threadId);
  }, [threadId]);

  // A sub-agent that waits on you is answered from here: its transcript is followed too.
  const needsThread = row?.needs_thread && row.needs_thread !== threadId ? row.needs_thread : null;
  useEffect(() => {
    if (!needsThread) return undefined;
    app.subscribe(needsThread);
    return () => app.unsubscribe(needsThread);
  }, [needsThread]);

  // Seen while it's on screen.
  useEffect(() => {
    if (row?.unseen && state.status === 'online' && document.visibilityState === 'visible') app.tell({ type: 'mark_seen', thread_id: threadId });
  }, [row?.unseen, state.status, t?.items.length]);

  // Kept at the bottom while new items come, unless you scrolled up to read.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stuck.current) el.scrollTop = el.scrollHeight;
  });
  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (el.scrollTop < 200 && t?.more && !t.olderLoading) {
      const before = el.scrollHeight;
      app.loadOlder(threadId).then(() => {
        requestAnimationFrame(() => {
          if (scroller.current) scroller.current.scrollTop += scroller.current.scrollHeight - before;
        });
      });
    }
  };

  const items = t?.items || [];
  const pending = items.filter((i) => REQUESTS.has(i.kind) && i.state === 'pending');
  const sub = needsThread ? state.transcripts.get(needsThread) : null;
  const subPending = (sub?.items || []).filter((i) => REQUESTS.has(i.kind) && i.state === 'pending');
  const subRow = needsThread ? app.row(needsThread) : null;
  const working = row?.run_state === 'working';
  const lastIndex = items.length - 1;

  return html`<div class="screen">
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <div class="bar-title">
        <h1>${row?.title || 'Thread'}</h1>
        <div class="sub">
          ${row && html`<span>${row.project?.name || 'Chat'}</span>`}
          ${row?.branch && html`<span><${Icon} name="branch" size="xs" /> ${row.branch}</span>`}
          ${working && html`<span class="badge working">Working${row.working_since ? ` · ${took((Date.now() - row.working_since) / 1000)}` : ''}</span>`}
        </div>
      </div>
      <${IconButton} icon="more" label="Thread menu" onClick=${() => setMenu(true)} />
    </header>
    <${ConnectionBanner} />
    <div class="scroll" ref=${scroller} onScroll=${onScroll}>
      <div class="transcript">
        ${t?.more && html`<button class="btn small ghost load-older" type="button" onClick=${() => app.loadOlder(threadId)} disabled=${t.olderLoading}>${t.olderLoading ? 'Loading…' : 'Show earlier'}</button>`}
        ${t?.loading && items.length === 0 && html`<div class="empty"><${Spinner} /></div>`}
        ${t?.error && items.length === 0 && html`<div class="empty">Couldn’t open it: ${t.error}</div>`}
        ${row?.started_by && !items.some((i) => i.kind === 'thread_link') && html`<button class="btn small ghost" type="button" onClick=${() => app.go({ screen: 'thread', threadId: row.started_by })}>Started by another thread</button>`}
        ${items.map((item, i) => (REQUESTS.has(item.kind) && item.state === 'pending' ? null : html`<${Item} key=${item.id} item=${item} threadId=${threadId} last=${i === lastIndex} />`))}
        ${working && row?.activity && html`<div class="notice"><${Spinner} /> ${row.activity}</div>`}
      </div>
    </div>
    <footer class="composer">
      ${(pending.length > 0 || subPending.length > 0) &&
      html`<div class="pinned-ask">
        ${pending.map((item) => html`<${Request} key=${item.id} item=${item} threadId=${threadId} />`)}
        ${subPending.map(
          (item) => html`<div key=${item.id}>
            <div class="row-btns" style=${{ alignItems: 'center', margin: '0 0 6px' }}>
              <span class="muted" style=${{ flex: 1, fontSize: '13px' }}>Sub-agent “${subRow?.title || 'sub-agent'}” is asking</span>
              <button class="btn small ghost" type="button" onClick=${() => app.go({ screen: 'thread', threadId: needsThread })}>Open it</button>
            </div>
            <${Request} item=${item} threadId=${needsThread} />
          </div>`,
        )}
      </div>`}
      ${row && threadParts.strips.map((Strip, i) => html`<${Strip} key=${i} row=${row} threadId=${threadId} />`)}
      <${Composer} row=${row} threadId=${threadId} onPicker=${() => setPicker(true)} />
    </footer>
    ${menu && html`<${ThreadMenu} row=${row} threadId=${threadId} onClose=${() => setMenu(false)} />`}
    ${picker && row && html`<${PrefsSheet} row=${row} onClose=${() => setPicker(false)} />`}
  </div>`;
}

function Request({ item, threadId }) {
  if (item.kind === 'approval') return html`<${Approval} item=${item} threadId=${threadId} />`;
  if (item.kind === 'question') return html`<${Questions} item=${item} threadId=${threadId} />`;
  return html`<${Plan} item=${item} threadId=${threadId} />`;
}

// ---------------------------------------------------------------------------------------------
// The composer

const ACCESS = [
  ['supervised', 'Supervised'],
  ['auto-accept-edits', 'Accept edits'],
  ['auto', 'Auto'],
  ['full-access', 'Full access'],
];

/** What a message's attachments add to `send` or `new_thread`: photos inline, files by their upload's id. */
export function attached(attachments) {
  const images = attachments.filter((a) => a.kind === 'image').map(({ mime, data }) => ({ mime, data }));
  const files = attachments.filter((a) => a.kind === 'file').map((a) => a.id);
  return { ...(images.length ? { images } : {}), ...(files.length ? { files } : {}) };
}

function draftKey(threadId) {
  return `trek-remote-draft:${threadId}`;
}

export function Composer({ row, threadId, onPicker }) {
  const [text, setText] = useState(() => {
    try {
      return sessionStorage.getItem(draftKey(threadId)) || '';
    } catch {
      return '';
    }
  });
  const [mode, setMode] = useState('steer');
  const [busy, setBusy] = useState(false);
  const [attachments, setAttachments] = useState([]);
  const area = useRef(null);
  const working = row?.run_state === 'working';
  const online = app.state.status === 'online';

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [text]);

  const keep = (value) => {
    setText(value);
    try {
      if (value) sessionStorage.setItem(draftKey(threadId), value);
      else sessionStorage.removeItem(draftKey(threadId));
    } catch {
      // Drafts are kept for this page alone.
    }
  };

  const send = async () => {
    const said = text.trim();
    if ((!said && attachments.length === 0) || busy) return;
    setBusy(true);
    try {
      const ack = await app.ask({ type: 'send', thread_id: threadId, text: said, mode: working ? mode : undefined, ...attached(attachments) }, { timeout: 120_000 });
      keep('');
      setAttachments([]);
      if (ack.open?.screen === 'new_thread') app.go({ screen: 'new', projectId: ack.open.project_id });
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      await app.ask({ type: 'interrupt', thread_id: threadId });
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setBusy(false);
    }
  };

  const setPref = async (prefs) => {
    try {
      await app.ask({ type: 'set_prefs', thread_id: threadId, ...prefs });
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };

  const access = ACCESS.filter(([v]) => v !== 'full-access' || app.state.fullAccess || row?.access === 'full-access');
  const accessIndex = Math.max(0, access.findIndex(([v]) => v === row?.access));

  return html`<div>
    ${row &&
    html`<div class="composer-tools">
      <button class="chip" type="button" onClick=${onPicker} aria-label="Agent and model"><${Icon} name="bot" size="xs" />${row.model_label || row.model || row.agent?.name || 'Model'}${row.effort_label ? ` · ${row.effort_label}` : ''}</button>
      ${row.access && html`<button class=${`chip ${row.access === 'full-access' ? 'on' : ''}`} type="button" onClick=${() => setPref({ access: access[(accessIndex + 1) % access.length][0] })}><${Icon} name="shield" size="xs" />${(ACCESS.find(([v]) => v === row.access) || [, row.access])[1]}</button>`}
      ${row.plan != null && html`<button class=${`chip ${row.plan ? 'on' : ''}`} type="button" aria-pressed=${row.plan ? 'true' : 'false'} onClick=${() => setPref({ plan: !row.plan })}><${Icon} name="plan" size="xs" />Plan</button>`}
      ${row.context?.percent != null && html`<span class="chip" title="Context used"><${Icon} name="gauge" size="xs" />${row.context.percent}%</span>`}
      ${working && html`<${Segmented} label="When it's sent" options=${[['steer', 'Steer'], ['queue', 'Queue']]} value=${mode} onChange=${setMode} />`}
    </div>`}
    ${composerPreviews.map((Preview, i) => html`<${Preview} key=${i} attachments=${attachments} setAttachments=${setAttachments} />`)}
    <div class="composer-box">
      ${composerTools.map((Tool, i) => html`<${Tool} key=${i} threadId=${threadId} text=${text} setText=${keep} attachments=${attachments} setAttachments=${setAttachments} disabled=${!online} />`)}
      <textarea
        ref=${area}
        rows="1"
        value=${text}
        placeholder=${working ? (mode === 'steer' ? 'Steer the turn…' : 'Queue a follow-up…') : 'Reply…'}
        aria-label="Message"
        enterkeyhint="send"
        onInput=${(e) => keep(e.target.value)}
        onKeyDown=${(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && window.matchMedia('(pointer: fine)').matches) {
            e.preventDefault();
            send();
          }
        }}
      ></textarea>
      ${working && !text.trim()
        ? html`<button class="send stop" type="button" aria-label="Stop" disabled=${busy || !online} onClick=${stop}><${Icon} name="stop" size="s" /></button>`
        : html`<button class="send" type="button" aria-label="Send" disabled=${(!text.trim() && attachments.length === 0) || busy || !online} onClick=${send}>${busy ? html`<${Spinner} />` : html`<${Icon} name="send" size="s" />`}</button>`}
    </div>
  </div>`;
}

/** The thread's agent, model and effort. */
function PrefsSheet({ row, onClose }) {
  const state = app.state;
  const agent = state.agents.find((a) => a.key === row.agent?.key) || null;
  const model = agent?.models?.find((m) => m.id === row.model) || null;
  const set = async (prefs) => {
    try {
      await app.ask({ type: 'set_prefs', thread_id: row.id, ...prefs });
    } catch (err) {
      app.toast(err.message, 'negative');
    }
  };
  return html`<${Sheet} title="Agent and model" onClose=${onClose}>
    ${state.agents.length > 1 &&
    html`<div class="field"><span>Agent</span>
      <select class="input" value=${row.agent?.key} onChange=${(e) => set({ agent: e.target.value })}>
        ${state.agents.map((a) => html`<option value=${a.key}>${a.name}</option>`)}
      </select></div>`}
    ${agent?.models?.length > 0 &&
    html`<div class="field"><span>Model</span>
      ${agent.models.map(
        (m) => html`<button type="button" class="option" aria-pressed=${m.id === row.model ? 'true' : 'false'} onClick=${() => set({ model: m.id })}>
          <${Icon} name=${m.id === row.model ? 'check' : 'bot'} size="s" /><span class="label">${m.label}</span>
        </button>`,
      )}</div>`}
    ${model?.efforts?.length > 0 &&
    html`<div class="field"><span>Effort</span>
      <${Segmented} label="Effort" options=${model.efforts.map((e) => [e, e === 'xhigh' ? 'Extra high' : e[0].toUpperCase() + e.slice(1)])} value=${row.effort} onChange=${(e) => set({ effort: e })} />
    </div>`}
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// The thread menu

function ThreadMenu({ row, threadId, onClose }) {
  const [snooze, setSnooze] = useState(false);
  const act = async (action) => {
    try {
      await app.ask({ type: 'thread_action', thread_id: threadId, action });
      return true;
    } catch (err) {
      if (action.kind === 'delete' && err.code === 'conflict' && !action.confirmed) {
        if (confirm(err.message)) return act({ ...action, confirmed: true }).then((ok) => ok && app.back());
        return false;
      }
      app.toast(err.message, 'negative');
      return false;
    }
  };
  if (!row) return html`<${Menu} title="Thread" items=${[]} onClose=${onClose} />`;
  if (snooze) {
    return html`<${Menu}
      title="Snooze"
      onClose=${onClose}
      items=${[
        { label: 'For 1 hour', icon: 'clock', onClick: () => act({ kind: 'snooze', preset: 'hour' }) },
        { label: 'For 3 hours', icon: 'clock', onClick: () => act({ kind: 'snooze', preset: 'three_hours' }) },
        { label: 'Until tomorrow morning', icon: 'sun', onClick: () => act({ kind: 'snooze', preset: 'tomorrow' }) },
        { label: 'Until next week', icon: 'clock', onClick: () => act({ kind: 'snooze', preset: 'next_week' }) },
      ]}
    />`;
  }
  const has = app.has;
  return html`<${Menu}
    title=${row.title}
    onClose=${onClose}
    items=${[
      { label: row.pinned ? 'Unpin' : 'Pin', icon: 'pin', onClick: () => act({ kind: row.pinned ? 'unpin' : 'pin' }) },
      {
        label: 'Rename…',
        icon: 'pencil',
        onClick: () => {
          const title = prompt('Rename the thread', row.title);
          if (title && title.trim() && title !== row.title) act({ kind: 'rename', title: title.trim() });
        },
      },
      { label: 'Write a new title', icon: 'sparkle', hidden: !has('regenerate_title'), onClick: () => act({ kind: 'regenerate_title' }) },
      { label: 'Mark as unread', icon: 'unread', hidden: !has('mark_unread'), onClick: () => act({ kind: 'mark_unread' }).then((ok) => ok && app.back()) },
      row.section === 'snoozed'
        ? { label: 'Wake now', icon: 'bell', hidden: !has('snooze'), onClick: () => act({ kind: 'wake' }) }
        : { label: 'Snooze…', icon: 'snooze', hidden: !has('snooze'), keep: true, onClick: () => setSnooze(true) },
      { label: row.section === 'settled' ? 'Move back to the inbox' : 'Settle', icon: 'check', onClick: () => act({ kind: row.section === 'settled' ? 'unsettle' : 'settle' }) },
      { label: 'Compact the context', icon: 'layers', hidden: !has('context_card'), onClick: () => act({ kind: 'compact' }) },
      ...threadParts.menuItems.flatMap((fn) => fn({ row, threadId, has, act }) || []),
      { label: 'Archive', icon: 'archive', onClick: () => act({ kind: 'archive' }).then((ok) => ok && app.back()) },
      { label: 'Delete…', icon: 'trash', danger: true, hidden: !has('delete'), onClick: () => act({ kind: 'delete' }) },
    ]}
  />`;
}

// ---------------------------------------------------------------------------------------------
// The connection, said over every screen while it isn't up

export function ConnectionBanner() {
  const state = app.useApp();
  const name = state.machine?.name || 'your computer';
  switch (state.status) {
    case 'online':
    case 'idle':
      return null;
    case 'connecting':
      return html`<div class="banner"><${Spinner} /><span class="grow">Connecting to ${name}…</span></div>`;
    case 'permission':
      return html`<div class="banner warn"><${Icon} name="wifi" /><span class="grow">${state.detail === 'denied' ? 'This browser is blocking the page from your local network. Allow “Local network access” for this site in the browser’s site settings, then try again.' : `The browser asks before this page looks for ${name} on your network.`}</span><button class="btn small primary" type="button" onClick=${() => app.reconnect({ gesture: true })}>Connect</button></div>`;
    case 'unauthorized':
      return html`<div class="banner bad"><${Icon} name="alert" /><span class="grow">${name} doesn’t know this phone any more. Pair it again from Trek’s Settings › Phone.</span></div>`;
    case 'unsupported':
      return html`<div class="banner bad"><${Icon} name="alert" /><span class="grow">This browser can’t make the connection Trek needs (WebTransport). Open this page in Chrome, Edge or Samsung Internet.</span></div>`;
    default:
      return html`<div class="banner bad"><${Icon} name="wifi" /><span class="grow">Can’t reach ${name}${state.detail ? ` (${state.detail})` : ''}. Is Trek open, and this phone on the same Wi-Fi or tailnet?</span><button class="btn small" type="button" onClick=${() => app.reconnect({ gesture: true })}>Retry</button></div>`;
  }
}

export { ProjectBadge };
