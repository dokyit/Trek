// The demo: a pretend computer running Trek, in the page, that answers the protocol as Trek does
// (the iPhone app's MockHost, smaller): threads across three projects and three agents, one asking
// to run a command, one with a question, one with a plan, one at work, and replies to what's sent.
// Nothing leaves the page.

import { handlers, seeds } from './demo/index.js';

const now = Date.now();
const min = 60_000;

const projects = [
  { id: 'p-api', name: 'trek-api', hue: 212, monogram: 'TA', branch: 'main', is_repo: true },
  { id: 'p-web', name: 'web', hue: 28, monogram: 'WE', branch: 'main', is_repo: true },
  { id: 'p-notes', name: 'notes', hue: 140, monogram: 'NO', branch: null, is_repo: false },
];
const agents = [
  { key: 'claude-code', name: 'Claude Code', logo: 'claude-code', default_model: 'claude-opus-5-5', models: [{ id: 'claude-opus-5-5', label: 'Opus 5.5', efforts: ['low', 'medium', 'high', 'xhigh', 'max'] }, { id: 'claude-sonnet-5-5', label: 'Sonnet 5.5', efforts: ['low', 'medium', 'high'] }] },
  { key: 'codex', name: 'Codex', logo: 'codex', default_model: 'gpt-6', models: [{ id: 'gpt-6', label: 'GPT-6', efforts: ['low', 'medium', 'high'] }] },
  { key: 'opencode', name: 'OpenCode', logo: 'opencode', default_model: null, models: [] },
];
// How the demo computer looks: `?demo&look=journal` (or `console`) puts one of Trek's themes on it, as
// the iPhone's demo Mac takes `-TrekDemoMacTheme`; otherwise Trek's own, as a new computer has on.
const LOOKS = {
  journal: {
    id: 'journal',
    name: 'Journal',
    description: 'Warm paper, the conversation set in a serif, terracotta accents.',
    mode: 'light',
    ui_font: 'sans',
    conversation_font: 'serif',
    ui_size: 15,
    conversation_size: 16,
    corners: 1.25,
    density: 'roomy',
    motion: 'calm',
    dark: { accent: '#E0895AFF', on_accent: '#1C0904FF', window: '#14110EFF', sidebar: '#191612FF', surface: '#221E19FF', raised: '#1D1915FF', text: '#ECE4D8FF', muted: '#9B9082FF', border: '#2E2922FF' },
    light: { accent: '#B0552CFF', on_accent: '#FFFFFFFF', window: '#FBF8F2FF', sidebar: '#F3EEE5FF', surface: '#F0EAE0FF', raised: '#FFFDF9FF', text: '#231E19FF', muted: '#776C61FF', border: '#E4DACCFF' },
  },
  console: {
    id: 'console',
    name: 'Console',
    description: 'Every word in monospace, square corners, phosphor green on soft black.',
    mode: 'dark',
    ui_font: 'mono',
    conversation_font: 'mono',
    ui_size: 13,
    conversation_size: 13.5,
    corners: 0.25,
    density: 'compact',
    motion: 'brisk',
    dark: { accent: '#6BDB9AFF', on_accent: '#06140CFF', window: '#0B0D0CFF', sidebar: '#0E1110FF', surface: '#151A18FF', raised: '#121615FF', text: '#D4DDD7FF', muted: '#7C8A82FF', border: '#222A26FF' },
    light: { accent: '#17804AFF', on_accent: '#FFFFFFFF', window: '#F6F6F1FF', sidebar: '#EEEEE7FF', surface: '#E9EAE2FF', raised: '#FBFBF7FF', text: '#1A1E1BFF', muted: '#646B65FF', border: '#DADBD1FF' },
  },
};
const TREK = { id: 'trek', name: 'Trek', description: 'Night and Paper, as Trek comes.', mode: 'any', ui_font: 'sans', conversation_font: 'sans', ui_size: 14, conversation_size: 14.5, corners: 1, density: 'regular', motion: 'standard', dark: {}, light: {} };
const look = { theme: LOOKS[typeof location === 'undefined' ? '' : new URLSearchParams(location.search).get('look')] || TREK };

const ref = (p) => ({ id: p.id, name: p.name, hue: p.hue, monogram: p.monogram });
const agentRef = (a) => ({ key: a.key, name: a.name, logo: a.logo });

function thread(id, title, p, a, extra) {
  return {
    id,
    title,
    project: ref(projects[p]),
    agent: agentRef(agents[a]),
    model: agents[a].default_model,
    model_label: agents[a].models[0]?.label || null,
    run_state: 'idle',
    section: 'inbox',
    unseen: false,
    pinned: false,
    branch: projects[p].branch,
    worktree: false,
    activity: null,
    working_since: null,
    updated_at: now - 30 * min,
    additions: 0,
    deletions: 0,
    effort: a === 0 ? 'high' : 'medium',
    effort_label: a === 0 ? 'High' : 'Medium',
    access: 'auto-accept-edits',
    plan: false,
    busy_in_background: false,
    ...extra,
  };
}

const threads = [
  thread('t-flaky', 'Fix the flaky auth test', 0, 0, { run_state: 'needs-you', needs: { kind: 'approval', text: 'Run `cargo test -p auth`' }, unseen: true, updated_at: now - 2 * min, branch: 'fix/auth-flake', worktree: true, additions: 42, deletions: 7 }),
  thread('t-scope', 'Rate limiting for /login', 0, 1, { run_state: 'needs-you', needs: { kind: 'question', text: 'Per IP, or per account too?' }, updated_at: now - 9 * min }),
  thread('t-plan', 'Dark mode for Settings', 1, 0, { run_state: 'needs-you', needs: { kind: 'plan', text: 'A plan to review' }, updated_at: now - 14 * min, plan: true }),
  thread('t-work', 'Move the build to Vite 7', 1, 1, { run_state: 'working', section: 'working', activity: 'Running pnpm build', working_since: now - 4 * min, updated_at: now - 20000 }),
  thread('t-pin', 'Release checklist', 2, 0, { pinned: true, section: 'pinned', updated_at: now - 3 * 3600_000 }),
  thread('t-done', 'Why is the cache cold after deploys?', 0, 0, { updated_at: now - 26 * 3600_000, additions: 12, deletions: 3 }),
  thread('t-old', 'Tidy up the README', 1, 2, { section: 'settled', updated_at: now - 5 * 86400_000 }),
  thread('t-size', 'Why is the web bundle so big?', 1, 0, { updated_at: now - 40 * min }),
];

let seq = 100;
const item = (id, kind, fields) => ({ id, seq: ++seq, at: null, kind, ...fields });

const transcripts = {
  't-flaky': [
    item('i1', 'user', { at: now - 6 * min, text: 'The auth test is flaky on CI, about one run in five. Find out why and fix it.', images: 2 }),
    item('i2', 'reasoning', { text: 'Let me read the test and the session code it covers first.' }),
    item('i3', 'tool', { call_id: 'c1', tool: 'read', title: 'Read', detail: 'src/auth/session.rs', status: 'done', output: '', added: null, removed: null }),
    item('i4', 'tool', { call_id: 'c2', tool: 'search', title: 'Search', detail: 'refresh(', status: 'done', output: 'src/auth/session.rs:88\nsrc/auth/middleware.rs:41', added: null, removed: null }),
    item('i5', 'assistant', { text: 'The race is in **`refresh()`**: two requests that both see an expired token both refresh it, and the second overwrites the first’s new token.\n\nI’ll hold a lock around the refresh, and add a test that runs ten refreshes at once:\n\n```rust\nlet _guard = self.refreshing.lock().await;\n```', streaming: false }),
    item('i6', 'tool', { call_id: 'c3', tool: 'edit', title: 'Edit', detail: 'src/auth/session.rs', status: 'done', output: '', added: 12, removed: 3 }),
    item('i7', 'approval', { request_id: 'r1', title: 'Run command', detail: 'cargo test -p auth -- --test-threads 8', state: 'pending', always_allow: { kind: 'command', pattern: 'cargo test -p auth' } }),
  ],
  't-scope': [
    item('i1', 'user', { at: now - 12 * min, text: 'Add rate limiting to /login', images: 0 }),
    item('i2', 'assistant', { text: 'I can put a token bucket in front of `/login`. One thing first:', streaming: false }),
    item('i3', 'question', { request_id: 'r2', questions: [{ header: 'Scope', question: 'Limit per IP address, or per account too?', options: [{ label: 'Per IP', description: 'Simplest; shared offices share a limit' }, { label: 'Per IP and account', description: 'Stops one account being guessed at from many places' }], multi: false, secret: false }], state: 'pending', answers: null }),
  ],
  't-plan': [
    item('i1', 'user', { at: now - 20 * min, text: 'Settings should follow dark mode like the rest of the app. @/Users/demo/web/settings-flash.mp4', images: 0, videos: ['/Users/demo/web/settings-flash.mp4'] }),
    item('i2', 'tool', { call_id: 'c0', tool: 'other', title: 'Watch video', detail: 'settings-flash.mp4', status: 'done', output: '', added: null, removed: null, watch: { source: 'settings-flash.mp4', video: '/Users/demo/web/settings-flash.mp4', duration: 2.4, frames: [0, 0.6, 1.2, 1.8].map((t, i) => ({ t, path: `/Users/demo/.trek/videos/settings-flash/f${i}.jpg` })) } }),
    item('i3', 'plan', { request_id: 'r3', markdown: '1. Replace the hard-coded colours in `settings.css` with the theme tokens.\n2. Add the two missing tokens (`--field`, `--field-border`) to both themes.\n3. Check every Settings page in both themes, and the contrast of the hint text.\n4. Screenshots for the pull request.', state: 'pending' }),
  ],
  't-work': [
    item('i1', 'user', { at: now - 5 * min, text: 'Move the build to Vite 7 and make sure the dev server still proxies /api.', images: 0 }),
    item('i2', 'tool', { call_id: 'c1', tool: 'edit', title: 'Edit', detail: 'package.json', status: 'done', output: '', added: 3, removed: 3 }),
    item('i3', 'tool', { call_id: 'c2', tool: 'command', title: 'Run', detail: 'pnpm build', status: 'running', output: 'vite v7.0.0 building for production...\ntransforming (412) src/routes/settings.tsx', added: null, removed: null }),
  ],
  't-pin': [item('i1', 'user', { at: now - 4 * 3600_000, text: 'Keep the release checklist here.', images: 0 }), item('i2', 'assistant', { text: '- [x] Changelog\n- [x] Version bump\n- [ ] Tag and push\n- [ ] Announce', streaming: false })],
  't-done': [
    item('i1', 'user', { at: now - 27 * 3600_000, text: 'Why is the cache cold after every deploy?', images: 0 }),
    item('i2', 'assistant', { text: 'The cache key includes the build id, so every deploy starts empty. Keying on the content hash instead keeps it warm.\n\n![Hit rate after the fix](docs/img/cache-hit-rate.png)', streaming: false }),
    item('i3', 'turn_end', { at: now - 26 * 3600_000, took_secs: 95 }),
    item('c3', 'changes', { files: [{ path: 'src/cache/key.rs', status: 'modified', added: 12, removed: 3 }], added: 12, removed: 3 }),
  ],
  't-old': [item('i1', 'user', { at: now - 5 * 86400_000, text: 'Tidy up the README.', images: 0 }), item('i2', 'assistant', { text: 'Done: shorter install section, and the badges fixed.', streaming: false })],
  // An answer drawn beyond plain Markdown: two charts (`trek-viz`) and math.
  't-size': [
    item('i1', 'user', { at: now - 42 * min, text: 'Why is the web bundle so big? The first page takes ages on my phone.', images: 0 }),
    item('i2', 'assistant', { text: "The editor is the biggest part of it, and it only shows on the settings page:\n\n```trek-viz\n{\"version\":1,\"title\":\"Where the bundle’s bytes go\",\"summary\":\"The production bundle by package, gzipped.\",\"type\":\"donut\",\"parts\":[{\"label\":\"Editor\",\"value\":412},{\"label\":\"Charts\",\"value\":268},{\"label\":\"React\",\"value\":141},{\"label\":\"Dates and i18n\",\"value\":96},{\"label\":\"Everything else\",\"value\":83,\"tone\":\"neutral\"}],\"unit\":\"KB\"}\n```\n\nSo every page waits for it, settings or not:\n\n```trek-viz\n{\"version\":1,\"title\":\"First load by page\",\"summary\":\"On a mid-range phone, cold cache.\",\"type\":\"bar\",\"bars\":[{\"label\":\"Home\",\"value\":2.6},{\"label\":\"Dashboard\",\"value\":3.1},{\"label\":\"Settings\",\"value\":3.4,\"tone\":\"warning\"}],\"x_label\":\"seconds\"}\n```\n\nLoading the editor only on the settings page takes 412 KB off the rest. A phone that fetches $r$ KB a second spends about $t = b / r$ seconds on a bundle of $b$ KB, so at 400 KB a second:\n\n$$\nt_{\\text{before}} - t_{\\text{after}} = \\frac{1000 - 588}{400} \\approx 1.03\\ \\text{s}\n$$\n\nThe charts come second, at 268 KB; they can wait for the dashboard too.", streaming: false }),
    item('i3', 'turn_end', { at: now - 40 * min, took_secs: 48 }),
  ],
};

export class DemoClient {
  constructor(hooks) {
    this.hooks = hooks;
    this.status = 'idle';
    this.host = { id: 'demo', name: 'Demo computer', version: 'demo' };
    this.features = new Set(['search', 'snooze', 'mark_unread', 'delete', 'regenerate_title', 'use_reset', 'basecamp_v2', 'rules', 'machines', 'note_images', 'note_files', 'diff_viewed', 'attachment', 'media', 'uploads', 'icon']);
    this.threads = new Map(threads.map((t) => [t.id, { ...t }]));
    this.transcripts = Object.fromEntries(Object.entries(transcripts).map(([k, v]) => [k, v.map((i) => ({ ...i }))]));
    this.subscribed = new Set();
    this.timers = [];
    for (const seed of seeds) seed(this);
  }

  connect() {
    this.status = 'connecting';
    this.hooks.onStatus('connecting');
    this.later(300, () => {
      this.status = 'online';
      this.hooks.onStatus('online');
      this.say({ type: 'snapshot', threads: [...this.threads.values()], projects, agents, full_access: false, unlisted: [], look });
    });
  }

  stop() {
    for (const t of this.timers) clearTimeout(t);
  }

  later(ms, fn) {
    this.timers.push(setTimeout(fn, ms));
  }

  say(msg) {
    this.hooks.onMessage(msg);
  }

  tell(msg) {
    if (msg.type === 'unsubscribe') this.subscribed.delete(msg.thread_id);
    if (msg.type === 'mark_seen') this.update(msg.thread_id, { unseen: false });
  }

  update(id, fields) {
    const t = this.threads.get(id);
    if (!t) return;
    Object.assign(t, fields);
    this.say({ type: 'thread', thread: { ...t } });
  }

  upsert(threadId, it) {
    const list = this.transcripts[threadId] || (this.transcripts[threadId] = []);
    const at = list.findIndex((x) => x.id === it.id);
    const next = { ...it, seq: ++seq };
    if (at < 0) list.push(next);
    else list[at] = next;
    if (this.subscribed.has(threadId)) this.say({ type: 'item', thread_id: threadId, item: next });
  }

  async ask(msg) {
    await new Promise((r) => setTimeout(r, 120));
    const t = msg.thread_id && this.threads.get(msg.thread_id);
    switch (msg.type) {
      case 'ping':
        return { type: 'pong' };
      case 'subscribe': {
        this.subscribed.add(msg.thread_id);
        const reply = { type: 'transcript', thread_id: msg.thread_id, reset: true, seq, items: [...(this.transcripts[msg.thread_id] || [])] };
        this.say(reply);
        return reply;
      }
      case 'answer':
        return this.answer(msg);
      case 'send': {
        if (!t) throw error('not_found', 'No such thread');
        const n = (this.transcripts[t.id] || []).length + 1;
        this.upsert(t.id, { id: `i${n}`, at: Date.now(), kind: 'user', ...this.message(msg, `${t.id}/i${n}`) });
        this.update(t.id, { run_state: 'working', section: 'working', activity: 'Thinking', working_since: Date.now(), updated_at: Date.now() });
        this.reply(t.id, n + 1);
        return { type: 'ack' };
      }
      case 'interrupt':
        if (t) {
          this.update(t.id, { run_state: 'idle', section: 'inbox', activity: null, working_since: null });
          this.upsert(t.id, { id: `i${this.transcripts[t.id].length + 1}`, kind: 'error', text: 'Interrupted' });
        }
        return { type: 'ack' };
      case 'new_thread': {
        const p = projects.find((x) => x.id === msg.project_id) || projects[0];
        const a = agents.findIndex((x) => x.key === msg.agent);
        const id = `t-new-${Date.now()}`;
        const row = thread(id, msg.text.slice(0, 48), projects.indexOf(p), Math.max(0, a), { run_state: 'working', section: 'working', activity: 'Thinking', working_since: Date.now(), updated_at: Date.now() });
        this.threads.set(id, row);
        this.transcripts[id] = [item('i1', 'user', { at: Date.now(), ...this.message(msg, `${id}/i1`) })];
        this.say({ type: 'thread', thread: { ...row } });
        this.reply(id, 2);
        return { type: 'ack', thread_id: id };
      }
      case 'thread_action':
        return this.action(t, msg.action);
      case 'set_prefs': {
        const fields = {};
        if (msg.model) {
          const m = agents.flatMap((a) => a.models).find((x) => x.id === msg.model);
          Object.assign(fields, { model: msg.model, model_label: m?.label || msg.model });
        }
        if (msg.agent) {
          const a = agents.find((x) => x.key === msg.agent);
          if (a) Object.assign(fields, { agent: agentRef(a), model: a.default_model, model_label: a.models[0]?.label || null });
        }
        if (msg.effort) Object.assign(fields, { effort: msg.effort, effort_label: msg.effort[0].toUpperCase() + msg.effort.slice(1) });
        if (msg.access) fields.access = msg.access;
        if (msg.plan != null) fields.plan = msg.plan;
        this.update(msg.thread_id, fields);
        return { type: 'ack' };
      }
      default:
        if (handlers[msg.type]) return handlers[msg.type](msg, this);
        throw error('bad_request', `The demo doesn’t do ${msg.type}`);
    }
  }

  /** A sent message's item fields: its text with the files it names, its photos (kept for `attachment`) and videos. */
  message(msg, key) {
    const files = (msg.files || []).map((id) => this.uploads?.get(String(id).toLowerCase()));
    if (files.some((u) => !u?.done)) throw error('bad_request', 'A file sent with this message hasn’t finished uploading');
    const mention = (p) => (/\s/.test(p) ? `@"${p}"` : `@${p}`);
    const text = [msg.text, ...files.map((u) => mention(u.path))].filter(Boolean).join(' ');
    const videos = files.filter((u) => /\.(mp4|m4v|mov|webm)$/i.test(u.name)).map((u) => u.path);
    if (msg.images?.length) (this.images ||= new Map()).set(key, msg.images);
    return { text, images: msg.images?.length || 0, ...(videos.length ? { videos } : {}) };
  }

  answer(msg) {
    const list = this.transcripts[msg.thread_id] || [];
    const req = list.find((i) => i.request_id === msg.request_id);
    if (!req || req.state !== 'pending') throw error('conflict', 'That was answered already');
    const r = msg.response;
    const state = r.kind === 'approval' ? { allow: 'allowed', allow_for_session: 'allowed_for_session', deny: 'denied' }[r.decision] : r.kind === 'plan' ? (r.approve ? 'approved' : 'rejected') : 'answered';
    this.upsert(msg.thread_id, { ...req, state, answers: r.answers || null });
    this.update(msg.thread_id, { needs: null, run_state: 'working', section: 'working', activity: 'Carrying on', working_since: Date.now() });
    this.reply(msg.thread_id, list.length + 1, state === 'denied' || state === 'rejected');
    return { type: 'ack' };
  }

  /** An answer that streams in, then the turn's end. */
  reply(threadId, n, declined) {
    const id = `i${n}`;
    const words = (declined ? 'Understood, I won’t do that. Tell me what you’d like instead.' : 'On it. I read the code around it, made the change, and the tests pass: **12 passed**, none failed.').split(' ');
    let text = '';
    words.forEach((w, i) => {
      this.later(400 + i * 90, () => {
        text += (i ? ' ' : '') + w;
        this.upsert(threadId, { id, kind: 'assistant', text, streaming: i < words.length - 1 });
      });
    });
    this.later(500 + words.length * 90, () => {
      this.upsert(threadId, { id: `i${n + 1}`, kind: 'turn_end', at: Date.now(), took_secs: 4 });
      this.update(threadId, { run_state: 'idle', section: 'inbox', activity: null, working_since: null, unseen: !this.subscribed.has(threadId), updated_at: Date.now() });
    });
  }

  action(t, action) {
    if (!t) throw error('not_found', 'No such thread');
    switch (action.kind) {
      case 'pin':
        this.update(t.id, { pinned: true, section: 'pinned' });
        break;
      case 'unpin':
        this.update(t.id, { pinned: false, section: 'inbox' });
        break;
      case 'settle':
        this.update(t.id, { section: 'settled' });
        break;
      case 'unsettle':
        this.update(t.id, { section: 'inbox' });
        break;
      case 'rename':
        this.update(t.id, { title: action.title });
        break;
      case 'mark_unread':
        this.update(t.id, { unseen: true });
        break;
      case 'snooze':
        this.update(t.id, { section: 'snoozed', snoozed_until: Date.now() + 3600_000 });
        break;
      case 'wake':
        this.update(t.id, { section: 'inbox', snoozed_until: null });
        break;
      case 'regenerate_title':
        this.update(t.id, { title: `${t.title} (retitled)` });
        break;
      case 'archive':
        this.threads.delete(t.id);
        this.say({ type: 'thread_removed', thread_id: t.id });
        break;
      case 'delete':
        if (!action.confirmed) throw error('conflict', 'The thread and its transcript are deleted. This can’t be undone.');
        this.threads.delete(t.id);
        this.say({ type: 'thread_removed', thread_id: t.id });
        break;
      default:
        throw error('bad_request', `The demo doesn’t do ${action.kind}`);
    }
    return { type: 'ack' };
  }
}

function error(code, message) {
  const e = new Error(message);
  e.code = code;
  return e;
}
