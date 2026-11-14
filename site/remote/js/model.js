// The web remote's state, and what changes it: the paired machines, the connection, and what the
// machine in use has said (its threads, projects and agents, the transcripts open here). Screens
// read it with `useApp` and change it through `actions`; what the machine says comes in through
// `receive`, the iPhone app's `AppModel` and `ThreadStore` in one.

import { useEffect, useState } from '../vendor/hooks.mjs';
import { Client } from './client.js';
import * as store from './store.js';

const listeners = new Set();

export const state = {
  /** The machines paired here, most recently used first. */
  machines: [],
  /** The machine in use: its stored record. */
  machine: null,
  /** The connection to it (`Client` statuses), and what went wrong. */
  status: 'idle',
  detail: '',
  host: null,
  features: new Set(),
  /** What it last said: rows by id, listed ones and the rest. */
  threads: new Map(),
  unlisted: new Map(),
  projects: [],
  agents: [],
  fullAccess: false,
  look: null,
  release: null,
  /** The computer's Settings › Phone, as `settings` answers (`session_approvals`, `phone_rules`…). */
  macSettings: null,
  haveSnapshot: false,
  /** Transcripts open here, by thread: `{ items, index, seq, more, loading, error }`. */
  transcripts: new Map(),
  /** What's on screen: `{ screen: 'threads' | 'thread' | 'new' | 'settings' | 'machines' | 'welcome' | 'pair', ... }`. */
  route: { screen: 'threads' },
  /** Short messages over the screen. */
  toasts: [],
  /** Bumped on every change, for screens that compare. */
  version: 0,
};

let client = null;

export function changed() {
  state.version += 1;
  for (const listener of listeners) listener(state.version);
}

/** Re-render when the state changes. */
export function useApp() {
  const [, setVersion] = useState(state.version);
  useEffect(() => {
    const listener = (v) => setVersion(v);
    listeners.add(listener);
    return () => listeners.delete(listener);
  }, []);
  return state;
}

export function has(feature) {
  return state.features.has(feature);
}

// ---------------------------------------------------------------------------------------------
// Navigation

export function go(route, { replace = false } = {}) {
  state.route = route;
  const url = route.screen === 'thread' ? `#t=${encodeURIComponent(route.threadId)}` : route.screen === 'threads' ? '#' : `#${route.screen}`;
  try {
    if (replace) history.replaceState(route, '', url === '#' ? location.pathname : url);
    else history.pushState(route, '', url === '#' ? location.pathname : url);
  } catch {
    // A sandboxed page: navigation stays in the app.
  }
  changed();
}

export function back() {
  if (history.state && history.length > 1) history.back();
  else go({ screen: 'threads' }, { replace: true });
}

if (typeof window !== 'undefined') {
  window.addEventListener('popstate', (e) => {
    state.route = e.state && e.state.screen ? e.state : { screen: 'threads' };
    changed();
  });
}

export function toast(text, tone = 'neutral') {
  const t = { id: Math.random().toString(36).slice(2), text, tone };
  state.toasts = [...state.toasts, t];
  changed();
  setTimeout(() => {
    state.toasts = state.toasts.filter((x) => x.id !== t.id);
    changed();
  }, 3200);
}

// ---------------------------------------------------------------------------------------------
// Machines and the connection

export async function loadMachines() {
  state.machines = await store.machines();
  changed();
  return state.machines;
}

function hooks() {
  return {
    onStatus: (status, detail) => {
      const wasOnline = state.status === 'online';
      state.status = status;
      state.detail = detail;
      if (status === 'online') {
        state.host = client.host;
        state.features = client.features;
        if (!wasOnline) {
          resubscribe();
          // What the computer lets a phone do (allow for a whole session, write rules).
          ask({ type: 'settings' })
            .then((reply) => {
              state.macSettings = reply;
              changed();
            })
            .catch(() => {});
        }
      }
      changed();
    },
    onMessage: receive,
    onMachine: async (m) => {
      state.machine = m;
      await loadMachines();
      try {
        localStorage.setItem('trek-remote-last', m.hostId);
      } catch {
        // Remembered for this visit only.
      }
    },
  };
}

/** Use `machine` (a stored one, or a pairing from a link) and connect to it. */
export function use(machine, { gesture = false } = {}) {
  if (client) client.stop();
  resetMachineState();
  state.machine = machine;
  state.status = 'connecting';
  client = new Client(machine, hooks());
  changed();
  client.connect({ gesture });
}

/** The demo: a pretend computer in the page, answering as Trek does (`demo.js`). */
export async function startDemo() {
  const { DemoClient } = await import('./demo.js');
  if (client) client.stop();
  resetMachineState();
  state.machine = { hostId: 'demo', name: 'Demo computer', routes: ['in this page'], demo: true };
  client = new DemoClient(hooks());
  go({ screen: 'threads' }, { replace: true });
  client.connect();
}

export function reconnect({ gesture = true } = {}) {
  if (client) client.connect({ gesture });
}

export async function forget(hostId) {
  await store.forgetMachine(hostId);
  if (state.machine?.hostId === hostId) {
    client?.stop();
    client = null;
    state.machine = null;
    resetMachineState();
  }
  await loadMachines();
  go({ screen: state.machines.length ? 'machines' : 'welcome' }, { replace: true });
}

function resetMachineState() {
  state.status = 'idle';
  state.detail = '';
  state.host = null;
  state.features = new Set();
  state.threads = new Map();
  state.unlisted = new Map();
  state.projects = [];
  state.agents = [];
  state.fullAccess = false;
  state.look = null;
  state.release = null;
  state.haveSnapshot = false;
  state.transcripts = new Map();
  state.macSettings = null;
}

/** Ask the machine something and wait for its answer. */
export function ask(msg, options) {
  if (!client) return Promise.reject(new Error('Not connected'));
  return client.ask(msg, options);
}

export function tell(msg) {
  client?.tell(msg);
}

// ---------------------------------------------------------------------------------------------
// What the machine says

export function receive(msg) {
  switch (msg.type) {
    case 'snapshot': {
      state.threads = new Map((msg.threads || []).map((t) => [t.id, t]));
      state.unlisted = new Map((msg.unlisted || []).map((t) => [t.id, t]));
      state.projects = msg.projects || [];
      state.agents = msg.agents || [];
      state.fullAccess = !!msg.full_access;
      state.look = msg.look || null;
      state.release = msg.release || null;
      state.haveSnapshot = true;
      break;
    }
    case 'thread':
      if (msg.thread?.id) {
        state.unlisted.delete(msg.thread.id);
        state.threads.set(msg.thread.id, msg.thread);
        state.threads = new Map(state.threads);
      }
      break;
    case 'unlisted':
      if (msg.thread?.id) {
        state.threads.delete(msg.thread.id);
        state.unlisted.set(msg.thread.id, msg.thread);
        state.unlisted = new Map(state.unlisted);
      }
      break;
    case 'thread_removed':
      state.threads.delete(msg.thread_id);
      state.threads = new Map(state.threads);
      break;
    case 'transcript':
      takeTranscript(msg);
      break;
    case 'transcript_page':
      takePage(msg);
      break;
    case 'item':
      takeItem(msg.thread_id, msg.item);
      break;
    case 'transcript_reset': {
      const t = state.transcripts.get(msg.thread_id);
      if (t) {
        t.seq = null;
        subscribe(msg.thread_id, { fresh: true });
      }
      break;
    }
    default:
      for (const hook of messageHooks) hook(msg);
      return changed();
  }
  for (const hook of messageHooks) hook(msg);
  changed();
}

/** Screens that want messages of their own (a git sheet's `git_status`, notes, uploads). */
const messageHooks = new Set();
export function onMessage(hook) {
  messageHooks.add(hook);
  return () => messageHooks.delete(hook);
}

export function row(threadId) {
  return state.threads.get(threadId) || state.unlisted.get(threadId) || null;
}

// ---------------------------------------------------------------------------------------------
// Transcripts

const PAGE = 200;

function transcript(threadId) {
  let t = state.transcripts.get(threadId);
  if (!t) {
    t = { items: [], index: new Map(), seq: null, more: false, loading: true, error: null, watchers: 0, olderLoading: false };
    state.transcripts.set(threadId, t);
  }
  return t;
}

function reindex(t) {
  t.index = new Map(t.items.map((item, i) => [item.id, i]));
}

function takeTranscript(msg) {
  const t = state.transcripts.get(msg.thread_id);
  if (!t) return;
  if (msg.reset) {
    t.items = msg.items || [];
    t.more = !!msg.more;
    reindex(t);
  } else {
    for (const item of msg.items || []) upsert(t, item);
  }
  t.seq = msg.seq ?? t.seq;
  t.loading = false;
  t.error = null;
}

function takePage(msg) {
  const t = state.transcripts.get(msg.thread_id);
  if (!t) return;
  const fresh = (msg.items || []).filter((item) => !t.index.has(item.id));
  t.items = [...fresh, ...t.items];
  t.more = !!msg.more;
  t.olderLoading = false;
  reindex(t);
}

function upsert(t, item) {
  const at = t.index.get(item.id);
  if (at === undefined) {
    t.index.set(item.id, t.items.length);
    t.items = [...t.items, item];
  } else {
    const items = t.items.slice();
    items[at] = item;
    t.items = items;
  }
  if (item.seq != null && (t.seq == null || item.seq > t.seq)) t.seq = item.seq;
}

function takeItem(threadId, item) {
  const t = state.transcripts.get(threadId);
  if (!t || !item?.id) return;
  // An item from before this phone's first (another phone paged further back) is left out:
  // paging it in brings it as it is.
  if (t.index.has(item.id) || !t.more || t.items.length === 0 || !isBefore(item.id, t.items[0].id)) upsert(t, item);
}

function isBefore(a, b) {
  const n = (id) => Number(String(id).replace(/^\D+/, ''));
  return Number.isFinite(n(a)) && Number.isFinite(n(b)) && n(a) < n(b);
}

/** Open a thread's transcript here (and keep it up to date while it's open). */
export function subscribe(threadId, { fresh = false } = {}) {
  const t = transcript(threadId);
  if (fresh) {
    t.seq = null;
    t.loading = true;
  }
  t.watchers += fresh ? 0 : 1;
  if (state.status !== 'online') return;
  const after = t.items.length && t.seq != null ? t.seq : null;
  ask({ type: 'subscribe', thread_id: threadId, after_seq: after, limit: PAGE }).catch((err) => {
    t.loading = false;
    t.error = err.message;
    changed();
  });
  changed();
}

export function unsubscribe(threadId) {
  const t = state.transcripts.get(threadId);
  if (!t) return;
  t.watchers = Math.max(0, t.watchers - 1);
  if (t.watchers === 0) {
    tell({ type: 'unsubscribe', thread_id: threadId });
    // Kept, with its seq, so opening it again carries on where it left off.
  }
}

function resubscribe() {
  for (const [threadId, t] of state.transcripts) {
    if (t.watchers > 0) {
      const after = t.items.length && t.seq != null ? t.seq : null;
      ask({ type: 'subscribe', thread_id: threadId, after_seq: after, limit: PAGE }).catch(() => {});
    }
  }
}

export async function loadOlder(threadId) {
  const t = state.transcripts.get(threadId);
  if (!t || !t.more || t.olderLoading || t.items.length === 0) return;
  t.olderLoading = true;
  changed();
  try {
    await ask({ type: 'transcript_before', thread_id: threadId, before: t.items[0].id, limit: PAGE });
  } catch (err) {
    t.olderLoading = false;
    toast(err.message, 'negative');
    changed();
  }
}

// ---------------------------------------------------------------------------------------------
// The thread list, as the iPhone groups it

/** Pinned, Needs you, Working, Snoozed, Recent: `[{ key, title, threads }]`, empty ones left out. */
export function sections(threads) {
  const groups = { pinned: [], needs: [], working: [], snoozed: [], recent: [] };
  for (const t of threads) {
    if (t.section === 'pinned' || t.pinned) groups.pinned.push(t);
    else if (t.section === 'snoozed') groups.snoozed.push(t);
    else if ((t.section === 'inbox' && t.needs) || t.run_state === 'failed' || t.run_state === 'needs-you') groups.needs.push(t);
    else if (t.section === 'working' || t.run_state === 'working' || t.busy_in_background) groups.working.push(t);
    else groups.recent.push(t);
  }
  const newest = (a, b) => (b.updated_at || 0) - (a.updated_at || 0);
  groups.pinned.sort(newest);
  groups.needs.sort(newest);
  groups.working.sort((a, b) => (b.working_since || b.updated_at || 0) - (a.working_since || a.updated_at || 0));
  groups.snoozed.sort((a, b) => (a.snoozed_until || 0) - (b.snoozed_until || 0));
  // The rest of the inbox, then settled, newest first.
  groups.recent.sort((a, b) => (a.section === 'settled') - (b.section === 'settled') || newest(a, b));
  return [
    { key: 'pinned', title: 'Pinned', threads: groups.pinned },
    { key: 'needs', title: 'Needs you', threads: groups.needs },
    { key: 'working', title: 'Working', threads: groups.working },
    { key: 'snoozed', title: 'Snoozed', threads: groups.snoozed },
    { key: 'recent', title: 'Recent', threads: groups.recent },
  ].filter((s) => s.threads.length);
}
