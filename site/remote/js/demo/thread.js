// The demo's answers for what a thread shows over its composer and under its turns
// (`features/thread-extras.js`, `features/turn-actions.js`), and the threads that show them, as the
// iPhone's MockHost has them: one paused at its agent's usage limit (another account has room),
// one Trek's closing cut short, one whose pull request fails CI, one with sub-agents out (one of
// them asking), and one with a move to another agent waiting and its prompt cache gone. The
// working thread gets a dev server in the background, two follow-ups queued and another thread in
// its folder. `seed(demo)` puts them in; the demo calls it once it's made.

const MIN = 60_000;
const ACK = { type: 'ack' };

/** What this area needs the demo computer to list in `features`. */
const FEATURES = [
  'limit_actions',
  'continue_on_account',
  'resume_interrupted',
  'failed_checks',
  'background',
  'queue_action',
  'sub_agents',
  'sub_agent_view',
  'switch_preview',
  'compact_offer',
  'context_card',
  'turn_preview',
  'side_chat',
];

const LABELS = { 'claude-opus-5-5': 'Opus 5.5', 'claude-sonnet-5-5': 'Sonnet 5.5', 'gpt-6': 'GPT-6' };

function fail(code, message) {
  return Object.assign(new Error(message), { code });
}

function row(demo, id) {
  const t = id && demo.threads.get(id);
  if (!t) throw fail('not_found', 'No such thread');
  return t;
}

/** A sub-agent's thread as its parent's row counts it. */
const KID_STATE = { working: 'running', 'needs-you': 'needs_you', failed: 'failed', idle: 'done' };

// ---------------------------------------------------------------------------------------------
// The threads

/**
 * Puts this area's threads and fields into the demo computer (once), and wires what it answers:
 * the thread actions its bars send, and sub-agents' and side chats' rows, which go out as
 * `unlisted` (in the snapshot too) and keep their parent's row in step.
 */
export function seed(demo) {
  if (demo.threadExtras) return demo.threadExtras;
  const x = { unlisted: new Set(), sideOf: new Map(), changed: [], reads: 0, runs: new Map() };
  demo.threadExtras = x;
  for (const f of FEATURES) demo.features.add(f);
  const now = Date.now();
  const all = [...demo.threads.values()];
  const base = demo.threads.get('t-done') || all[0];
  const project = (id) => all.find((t) => t.project?.id === id)?.project || base.project;
  const agent = (key, name) => all.find((t) => t.agent?.key === key)?.agent || { key, name, logo: key };
  const claude = agent('claude-code', 'Claude Code');
  const codex = agent('codex', 'Codex');
  let seq = 1;
  const item = (id, kind, fields) => ({ id, seq: seq++, at: null, kind, ...fields });
  const tool = (id, kind, title, detail, more = {}) => item(id, 'tool', { call_id: `c-${id}`, tool: kind, title, detail, status: 'done', output: '', added: null, removed: null, ...more });
  const put = (id, title, fields, items) => {
    const t = {
      ...base,
      id,
      title,
      run_state: 'idle',
      section: 'inbox',
      unseen: false,
      pinned: false,
      needs: null,
      activity: null,
      working_since: null,
      additions: 0,
      deletions: 0,
      ...fields,
    };
    demo.threads.set(id, t);
    demo.transcripts[id] = items;
    if (t.parent_id) x.unlisted.add(id);
    x.changed.push(id);
  };
  const plan = { label: '≈ $2.31 at API prices', billing: 'plan', plan: 'Claude Max', detail: 'Included in your Claude Max plan', cached: '91% cached', cached_detail: '80K of 88K prompt tokens came from the prompt cache, cheaper and faster than sending them again.' };

  // At work: a dev server it started, two follow-ups sent while it runs, another thread in its folder.
  const work = demo.threads.get('t-work');
  if (work) {
    Object.assign(work, {
      background: ['pnpm dev'],
      background_tasks: [{ id: 'bg-dev', kind: 'shell', title: 'pnpm dev', line: 'GET /api/health 200 in 4ms', since: now - 18 * MIN, readable: true, stoppable: true }],
      queued: [{ text: 'Then update the build section of the README.' }, { text: 'And open a pull request against main.' }],
      shares_folder: true,
      context: { used: 61_200, window: 200_000, percent: 31 },
    });
    x.changed.push('t-work');
  }

  // Paused at the agent's usage limit, with a message waiting for the reset and another account with room.
  put(
    't-limit',
    'Port the docs search to Rust',
    {
      agent: claude,
      model: 'claude-opus-5-5',
      model_label: 'Opus 5.5',
      needs: { kind: 'limit', text: '5-hour limit reached' },
      branch: 'feat/search-rs',
      updated_at: now - 4 * MIN,
      additions: 184,
      pause: { resets_at: now + 134 * MIN, resume: false, wrapped: false, other_account: { account: 'acct-work', label: 'Work' } },
      queued: [{ text: 'Then wire it into the docs build.', reset: true }],
      context: { used: 88_000, window: 200_000, percent: 44 },
      cost: plan,
    },
    [
      item('i1', 'user', { at: now - 40 * MIN, text: 'Port the docs search from the Python script to Rust, as a crate the docs build can call.', images: 0 }),
      item('i2', 'reasoning', { text: 'The script tokenises, builds an inverted index and writes it as JSON. I’ll keep the JSON as it is, so the page doesn’t change.' }),
      tool('i3', 'read', 'Read', 'scripts/search_index.py'),
      tool('i4', 'edit', 'Edit', 'crates/docs-search/src/lib.rs', { added: 184, removed: 0 }),
      tool('i5', 'command', 'Run', 'cargo test -p docs-search', { output: 'test result: ok. 12 passed; 0 failed' }),
      item('i6', 'assistant', { text: 'The crate builds the same index as the script, token for token. Next I’ll call it from the docs build and drop the script.', streaming: false }),
      item('i7', 'limit', { text: '5-hour limit reached', resets_at: now + 134 * MIN }),
    ],
  );

  // Trek closed on the computer in the middle of its turn.
  put(
    't-cut',
    'Index the search tables',
    { agent: codex, model: 'gpt-6', model_label: 'GPT-6', updated_at: now - 50 * MIN, cut_short: { state: 'offered', how: 'Reopens its Codex session', waiting: false } },
    [
      item('i1', 'user', { at: now - 55 * MIN, text: 'The search page is slow on big projects. Add the indexes its queries need.', images: 0 }),
      tool('i2', 'read', 'Read', 'src/db/schema.sql'),
      tool('i3', 'search', 'Search', 'ORDER BY updated_at', { output: 'src/db/threads.rs:88\nsrc/db/search.rs:41' }),
      item('i4', 'assistant', { text: 'Both slow queries filter by project and sort by `updated_at`: an index on `(project_id, updated_at)` covers them. Writing the migration now.', streaming: false }),
      tool('i5', 'edit', 'Edit', 'migrations/0042_search_indexes.sql', { added: 6, removed: 0 }),
    ],
  );

  // A pull request it opened fails CI.
  const pr = 'https://github.com/acme/trek-api/pull/128';
  put(
    't-ci',
    'Refresh sessions in the auth middleware',
    {
      agent: claude,
      model: 'claude-opus-5-5',
      model_label: 'Opus 5.5',
      needs: { kind: 'failed', text: 'CI failed: clippy and 1 more' },
      checks: { url: pr, number: 128, failing: ['clippy', 'test (macos)'], bar: true },
      branch: 'feat/session-refresh',
      worktree: true,
      updated_at: now - 7 * MIN,
      additions: 128,
      deletions: 36,
      context: { used: 104_000, window: 200_000, percent: 52 },
      cost: plan,
    },
    [
      item('i1', 'user', { at: now - 30 * MIN, text: 'Move the session refresh into the auth middleware, and open a pull request.', images: 0 }),
      tool('i2', 'edit', 'Edit', 'src/auth/middleware.rs', { added: 96, removed: 20 }),
      tool('i3', 'edit', 'Edit', 'src/auth/session.rs', { added: 32, removed: 16 }),
      tool('i4', 'command', 'Run', 'git push -u origin feat/session-refresh'),
      item('i5', 'assistant', { text: 'Done: the middleware refreshes the session before the handler runs, and the old refresh in `session.rs` is gone. I opened **#128**.', streaming: false }),
      item('i6', 'card', {
        card: { key: pr, kind: 'pr', phase: 'failed', title: 'Refresh sessions in the auth middleware', number: 128, url: pr, head: 'feat/session-refresh', base: 'main', added: 128, removed: 36, checks: [{ name: 'clippy', state: 'fail' }, { name: 'test (macos)', state: 'fail' }, { name: 'test (linux)', state: 'pass' }] },
      }),
      item('i7', 'turn_end', { at: now - 7 * MIN, took_secs: 412 }),
      item('c7', 'changes', { files: [{ path: 'src/auth/middleware.rs', status: 'modified', added: 96, removed: 20 }, { path: 'src/auth/session.rs', status: 'modified', added: 32, removed: 16 }], added: 128, removed: 36 }),
    ],
  );

  // Sub-agents out: two of Trek's (one asking), and two of Codex's own.
  const web = project('p-web');
  const subs = [
    { agent: claude, model: 'Opus 5.5', title: 'Review the theme tokens', state: 'running', since: now - 95_000, thread_id: 't-subs-review' },
    { agent: claude, model: 'Sonnet 5.5', title: 'Benchmark the dark palette', state: 'needs_you', since: now - 240_000, thread_id: 't-subs-bench' },
    { agent: codex, title: 'token scout', state: 'running', since: now - 70_000, call_id: 'agent-scout' },
    { agent: codex, title: 'copy check', state: 'done', since: now - 300_000, call_id: 'agent-copy' },
  ];
  put(
    't-subs',
    'Audit the theme tokens',
    {
      project: web,
      agent: codex,
      model: 'gpt-6',
      model_label: 'GPT-6',
      effort: 'medium',
      effort_label: 'Medium',
      run_state: 'needs-you',
      needs: { kind: 'approval', text: 'Sub-agent needs you' },
      needs_thread: 't-subs-bench',
      busy_in_background: true,
      branch: 'main',
      updated_at: now - MIN,
      sub_agents: subs,
      context: { used: 52_000, window: 272_000, percent: 19 },
    },
    [
      item('i1', 'user', { at: now - 6 * MIN, text: 'Audit the theme tokens: every colour in the web app should come from a token, and both themes should pass contrast.', images: 0 }),
      item('i2', 'assistant', { text: 'I’ll split it up: one sub-agent reviews the tokens themselves, one measures the dark palette’s contrast, and two scouts of mine find hard-coded colours and check the empty states.', streaming: false }),
      tool('i3', 'agent', 'Sub-agent', 'Review the theme tokens', { status: 'running', sub_agent: { agent: claude, label: 'Opus 5.5: Review the theme tokens', detail: 'Running · Read src/theme/tokens.css', state: 'running', thread_id: 't-subs-review' } }),
      tool('i4', 'agent', 'Sub-agent', 'Benchmark the dark palette', { status: 'running', sub_agent: { agent: claude, label: 'Sonnet 5.5: Benchmark the dark palette', detail: 'Waiting on you', state: 'needs_you', thread_id: 't-subs-bench' } }),
      tool('i5', 'agent', 'Agent', 'token scout', { status: 'running', call_id: 'agent-scout', sub_agent: { agent: codex, label: 'token scout', detail: 'Searching src/ · 3 steps', state: 'running', since: now - 70_000 } }),
      tool('i6', 'agent', 'Agent', 'copy check', { call_id: 'agent-copy', sub_agent: { agent: codex, label: 'copy check', detail: 'Done · 4 steps', state: 'done', took_secs: 48 } }),
    ],
  );
  put(
    't-subs-review',
    'Review the theme tokens',
    { project: web, agent: claude, model: 'claude-opus-5-5', model_label: 'Opus 5.5', parent_id: 't-subs', run_state: 'working', section: 'working', activity: 'Reading src/theme/tokens.css', working_since: now - 95_000, updated_at: now - 20_000 },
    [
      item('i1', 'user', { at: now - 95_000, text: 'Review src/theme/tokens.css: say which tokens are unused, which repeat another, and which colours are too close to tell apart in either theme.', images: 0 }),
      tool('i2', 'read', 'Read', 'src/theme/tokens.css', { status: 'running' }),
    ],
  );
  put(
    't-subs-bench',
    'Benchmark the dark palette',
    {
      project: web,
      agent: claude,
      model: 'claude-sonnet-5-5',
      model_label: 'Sonnet 5.5',
      parent_id: 't-subs',
      run_state: 'needs-you',
      needs: { kind: 'approval', text: 'Run `pnpm exec playwright test`' },
      updated_at: now - MIN,
    },
    [
      item('i1', 'user', { at: now - 240_000, text: 'Measure the contrast of every text colour on its background, in both themes, with the Playwright contrast check.', images: 0 }),
      tool('i2', 'read', 'Read', 'tests/contrast.spec.ts'),
      item('i3', 'approval', { request_id: 'r-bench', title: 'Run command', detail: 'pnpm exec playwright test tests/contrast.spec.ts --project=dark', state: 'pending' }),
    ],
  );

  // Another agent picked for the next message, and a large thread whose prompt cache has gone.
  put(
    't-switch',
    'Cache the session lookups',
    {
      agent: codex,
      model: 'gpt-6',
      model_label: 'GPT-6',
      effort: 'medium',
      effort_label: 'Medium',
      updated_at: now - 3 * 60 * MIN,
      additions: 18,
      deletions: 4,
      pending_switch: { agent: claude, model: 'claude-opus-5-5', model_label: 'Opus 5.5', effort: 'high', to: 'Claude Code', from: 'Codex' },
      compact_offer: {
        used: 128_400,
        window: 272_000,
        last_active: now - 180 * MIN,
        lapses_at: now - 120 * MIN,
        why: 'The prompt cache has expired: it lasts about an hour on a subscription.\nYour next message re-reads all 128K tokens, and they count against your limits.\nCompact first to send less.',
      },
      context: { used: 128_400, window: 272_000, percent: 47 },
      cost: { label: '$0.41', billing: 'metered', detail: 'Billed per token by your API provider' },
    },
    [
      item('i1', 'user', { at: now - 210 * MIN, text: 'Session lookups hit the database on every request. Cache them for a minute.', images: 0 }),
      tool('i2', 'read', 'Read', 'src/auth/session.rs'),
      tool('i3', 'edit', 'Edit', 'src/auth/session.rs', { added: 18, removed: 4 }),
      tool('i4', 'command', 'Run', 'cargo test -p auth', { output: 'test result: ok. 41 passed; 0 failed' }),
      item('i5', 'assistant', { text: 'Sessions are cached for 60 s, keyed by token, and dropped on logout. The tests pass.', streaming: false }),
      item('i6', 'turn_end', { at: now - 180 * MIN, took_secs: 133 }),
      item('c6', 'changes', { files: [{ path: 'src/auth/session.rs', status: 'modified', added: 18, removed: 4 }], added: 18, removed: 4 }),
    ],
  );

  wire(demo, x);
  return x;
}

/**
 * This area's state in the demo, seeding it first if it wasn't: then the rows go out as they'd
 * have gone in the snapshot (the demo was seeded after its snapshot went).
 */
function extras(demo) {
  if (demo.threadExtras) return demo.threadExtras;
  const x = seed(demo);
  for (const id of x.changed) {
    const t = demo.threads.get(id);
    if (t) demo.say({ type: 'thread', thread: { ...t } });
  }
  return x;
}

/** What the demo computer does differently with this area's threads in it. */
function wire(demo, x) {
  const say = demo.say.bind(demo);
  demo.say = (msg) => {
    // A copy: the page keeps what it's sent, and the demo goes on adding to its own list (the
    // next item would be in the page's twice).
    if (msg?.type === 'transcript') return say({ ...msg, items: [...(msg.items || [])] });
    if (msg?.type === 'snapshot') {
      const threads = msg.threads || [];
      return say({ ...msg, threads: threads.filter((t) => !x.unlisted.has(t.id)), unlisted: [...(msg.unlisted || []), ...threads.filter((t) => x.unlisted.has(t.id))] });
    }
    if (msg?.type === 'thread' && x.unlisted.has(msg.thread?.id)) {
      say({ type: 'unlisted', thread: msg.thread });
      return follow(demo, msg.thread);
    }
    return say(msg);
  };
  // The bars' thread actions, which the demo's own don't do.
  const action = demo.action.bind(demo);
  demo.action = (t, a) => {
    const own = ACTIONS[a?.kind];
    if (!own) return action(t, a);
    if (!t) throw fail('not_found', 'No such thread');
    return own(t, a, demo, x);
  };
}

/** A sub-agent's thread changed: its parent's row says so, as the Mac's does. */
function follow(demo, kid) {
  const parent = kid.parent_id && demo.threads.get(kid.parent_id);
  if (!parent) return;
  const last = (demo.transcripts[kid.id] || []).at(-1);
  const state = kid.run_state === 'idle' && last?.kind === 'error' ? 'stopped' : KID_STATE[kid.run_state] || 'running';
  const subs = (parent.sub_agents || []).map((s) => (s.thread_id === kid.id ? { ...s, state } : s));
  const fields = {};
  if (subs.some((s, i) => s.state !== parent.sub_agents[i].state)) {
    fields.sub_agents = subs;
    fields.busy_in_background = subs.some((s) => s.state === 'running' || s.state === 'needs_you');
  }
  if (parent.needs_thread === kid.id && kid.run_state !== 'needs-you') Object.assign(fields, { needs: null, needs_thread: null, run_state: 'idle' });
  if (Object.keys(fields).length) demo.update(parent.id, fields);
}

// ---------------------------------------------------------------------------------------------
// The bars' thread actions

/** A message the thread carries on with, and its answer. */
function carryOn(demo, t, text, activity = 'Thinking') {
  const n = (demo.transcripts[t.id] || []).length + 1;
  demo.upsert(t.id, { id: `i${n}`, at: Date.now(), kind: 'user', text, images: 0 });
  demo.update(t.id, { run_state: 'working', section: 'working', needs: null, activity, working_since: Date.now(), updated_at: Date.now() });
  demo.reply(t.id, n + 1);
}

function paused(t) {
  if (!t.pause) throw fail('conflict', 'It isn’t paused at a usage limit');
  return t.pause;
}

const ACTIONS = {
  resume_at_reset(t, a, demo) {
    demo.update(t.id, { pause: { ...paused(t), resume: true } });
    return ACK;
  },
  cancel_resume(t, a, demo) {
    demo.update(t.id, { pause: { ...paused(t), resume: false } });
    return ACK;
  },
  try_again(t, a, demo) {
    paused(t);
    const waiting = (t.queued || []).filter((q) => q.reset);
    demo.update(t.id, { pause: null, queued: (t.queued || []).filter((q) => !q.reset) });
    carryOn(demo, t, waiting.map((q) => q.text).join('\n\n') || 'Carry on.', 'Trying again');
    return ACK;
  },
  snooze_until_reset(t, a, demo) {
    const pause = paused(t);
    if (pause.resets_at == null) throw fail('conflict', 'When the limit resets isn’t known');
    demo.update(t.id, { section: 'snoozed', snoozed_until: pause.resets_at, pinned: false });
    return ACK;
  },
  continue_on_account(t, a, demo) {
    const other = paused(t).other_account;
    if (!other) throw fail('conflict', 'No other account of its agent has room now');
    if ((other.account ?? null) !== (a.account ?? null)) throw fail('conflict', `That account has no room now. ${other.label} has.`);
    const waiting = (t.queued || []).filter((q) => q.reset).map((q) => q.text);
    demo.update(t.id, { pause: null, queued: (t.queued || []).filter((q) => !q.reset) });
    demo.upsert(t.id, { id: `i${demo.transcripts[t.id].length + 1}`, kind: 'notice', text: `Carrying on with ${other.label}, from a recap` });
    carryOn(demo, t, waiting.join('\n\n') || 'Carry on.');
    return ACK;
  },
  compact(t, a, demo) {
    if (t.run_state === 'working') throw fail('conflict', 'Stop the running turn to compact.');
    if (t.pause) throw fail('conflict', 'It’s paused at a usage limit: compacting waits for the reset.');
    if (!(demo.transcripts[t.id] || []).some((i) => i.kind === 'user')) throw fail('conflict', 'There’s nothing to compact yet.');
    demo.update(t.id, { run_state: 'working', section: 'working', activity: 'Summarizing messages', working_since: Date.now(), compact_offer: null });
    demo.later(1500, () => {
      const c = demo.threads.get(t.id)?.context;
      const used = c ? Math.round(c.used / 4) : null;
      demo.upsert(t.id, { id: `i${demo.transcripts[t.id].length + 1}`, kind: 'notice', text: 'Compacted the conversation' });
      demo.update(t.id, { run_state: 'idle', section: 'inbox', activity: null, working_since: null, ...(c ? { context: { used, window: c.window, percent: Math.round((used * 100) / c.window) } } : {}) });
    });
    return ACK;
  },
  resume_interrupted(t, a, demo) {
    if (!t.cut_short) throw fail('conflict', 'There’s nothing to resume');
    demo.update(t.id, { cut_short: { ...t.cut_short, state: 'resuming' } });
    demo.later(1200, () => {
      demo.update(t.id, { cut_short: null, run_state: 'working', section: 'working', activity: 'Carrying on', working_since: Date.now() });
      demo.reply(t.id, (demo.transcripts[t.id] || []).length + 1);
    });
    return ACK;
  },
  dismiss_interrupted(t, a, demo) {
    if (!t.cut_short) throw fail('conflict', 'There’s nothing to resume');
    demo.update(t.id, { cut_short: null });
    return ACK;
  },
  send_failed_checks(t, a, demo) {
    const checks = t.checks;
    if (!checks) throw fail('conflict', 'Nothing is failing now');
    demo.update(t.id, { checks: { ...checks, sending: true } });
    demo.later(1200, () => {
      const now = demo.threads.get(t.id);
      demo.update(t.id, { checks: null, needs: now?.run_state === 'failed' ? now.needs : null });
      const names = (checks.failing || []).map((n) => `**${n}**`).join(', ');
      carryOn(demo, now || t, `CI failed on #${checks.number}: ${names}. Fix it, and push again.`);
    });
    return ACK;
  },
  dismiss_failed_checks(t, a, demo) {
    if (!t.checks) throw fail('conflict', 'Nothing is failing now');
    demo.update(t.id, { checks: null, needs: t.run_state === 'failed' ? t.needs : null });
    return ACK;
  },
  stay_on_agent(t, a, demo) {
    if (!t.pending_switch) throw fail('conflict', 'No switch waits for its next message');
    demo.update(t.id, { pending_switch: null });
    return ACK;
  },
};

// ---------------------------------------------------------------------------------------------
// The agent's own sub-agents (`sub_agent`), as the iPhone's DemoSubAgents: their steps come in a
// few at a time while they work, counted from when they were first asked about.

const CODEX = { key: 'codex', name: 'Codex', logo: 'codex' };

const OWN = {
  'agent-scout': {
    link: { agent: CODEX, label: 'token scout', state: 'running' },
    since: 70_000,
    prompt: 'Find every colour in src/ that isn’t a theme token: hex, rgb() and hsl() values in CSS, TSX and SVG. List each with its file and line.',
    steps: [
      { at: 0, kind: 'assistant', text: 'I’ll search the styles first, then the components.' },
      { at: 0, kind: 'tool', tool: 'search', title: 'Search', detail: '#[0-9a-fA-F]{3,8} in src/', output: 'src/components/Badge.tsx:14\nsrc/components/Chart.tsx:88\nsrc/styles/legacy.css:3' },
      { at: 0, kind: 'tool', tool: 'read', title: 'Read', detail: 'src/styles/legacy.css', output: '.legacy-panel {\n  color: #6b6b6b;\n}' },
      { at: 3, kind: 'tool', tool: 'search', title: 'Search', detail: 'rgb\\(|hsl\\( in src/', output: 'src/components/Chart.tsx:91\nsrc/components/Chart.tsx:92' },
      { at: 6, kind: 'assistant', text: 'Three files hold colours of their own: `Badge.tsx`, `Chart.tsx` and `legacy.css`. The chart’s are its series colours; the others are a leftover grey.' },
      { at: 9, kind: 'tool', tool: 'read', title: 'Read', detail: 'src/components/Chart.tsx', status: 'running' },
    ],
    stops: true,
    messages: true,
  },
  'agent-copy': {
    link: { agent: CODEX, label: 'copy check', state: 'done', took_secs: 48 },
    prompt: 'Read the empty states and say whether each still reads right in both themes.',
    steps: [
      { at: 0, kind: 'tool', tool: 'search', title: 'Search', detail: 'EmptyState in src/', output: 'src/routes/Projects.tsx:31\nsrc/routes/Threads.tsx:58\nsrc/routes/Notes.tsx:22' },
      { at: 0, kind: 'tool', tool: 'read', title: 'Read', detail: 'src/routes/Projects.tsx' },
      { at: 0, kind: 'tool', tool: 'read', title: 'Read', detail: 'src/components/EmptyState.tsx' },
      { at: 0, kind: 'assistant', text: 'Two of the three empty states read right.' },
    ],
    answer: 'Two of the three empty states read right as they are. **Projects** says “No projects yet” in `--border` on Paper, too faint to read: it wants `--muted`.',
    stops: false,
    messages: false,
  },
};

/** What call `call` has done by now, as the computer's `sub_agent` says it. */
function subAgentLog(threadId, call, x, now = Date.now()) {
  const own = OWN[call];
  if (!own) throw fail('not_found', 'No such sub-agent');
  const run = x.runs.get(call) || { started: now, extra: [], stoppedAt: null };
  x.runs.set(call, run);
  const working = own.link.state === 'running';
  const stopped = run.stoppedAt != null;
  const elapsed = (run.stoppedAt ?? now - run.started) / 1000;
  const since = working ? run.started - own.since : null;
  let steps = [...own.steps, ...run.extra].filter((s) => !working || s.at <= elapsed).sort((a, b) => a.at - b.at);
  let link = { ...own.link, since: since ?? undefined };
  if (stopped) {
    link = { ...link, state: 'stopped', detail: 'Stopped', since: undefined, took_secs: Math.round((run.stoppedAt + own.since) / 1000) };
    // What was running ended with it.
    steps = steps.map((s) => (s.status === 'running' ? { ...s, status: 'failed' } : s));
  } else if (working) {
    const calls = steps.filter((s) => s.kind === 'tool').length;
    const lastTool = [...steps].reverse().find((s) => s.kind === 'tool');
    link.detail = `${lastTool ? `${lastTool.title} ${lastTool.detail.split('/').pop()}` : 'Working'} · ${calls} step${calls === 1 ? '' : 's'}`;
  } else {
    link.detail = `Done · ${steps.length} steps`;
  }
  const live = link.state === 'running';
  const items = steps.map((s, n) =>
    s.kind === 'tool'
      ? { id: `s${n}`, seq: n, at: null, kind: 'tool', call_id: `${call}-${n}`, tool: s.tool, title: s.title, detail: s.detail, status: s.status || 'done', output: s.output || '', added: null, removed: null }
      : { id: `s${n}`, seq: n, at: null, kind: s.kind, text: s.text, ...(s.kind === 'assistant' ? { streaming: false } : { images: 0 }) },
  );
  const noMessage = `${own.link.agent.name} doesn’t take messages for its sub-agents while they run. Write to the thread, and its agent can pass it on.`;
  return {
    type: 'sub_agent',
    thread_id: threadId,
    call_id: call,
    link,
    prompt: own.prompt,
    items,
    ...(live ? {} : { answer: stopped ? 'Stopped by you.' : own.answer }),
    can_stop: live && own.stops,
    can_message: live && own.messages,
    ...(own.messages ? {} : { no_message: noMessage }),
  };
}

// ---------------------------------------------------------------------------------------------
// The rest of what this area asks

/** A dev server's output, a little longer each time it's read. */
function devOutput(reads) {
  const lines = ['> web@0.9.0 dev', '> vite --port 5173', '', '  VITE v7.1.4  ready in 284 ms', '', '  ➜  Local:   http://localhost:5173/', '  ➜  Proxy:   /api → http://localhost:8080', '[vite] page reload src/App.tsx'];
  for (let n = 0; n < Math.min(reads, 12); n += 1) lines.push(n % 2 === 0 ? `GET /api/health 200 in ${3 + (n % 3)}ms` : '[vite] hmr update /src/routes/Home.tsx');
  return lines.join('\n');
}

/** The message at or before `i` that started its turn (an aside doesn't). */
function startOf(list, i) {
  for (let j = i; j >= 0; j -= 1) if (list[j].kind === 'user' && !list[j].aside) return j;
  return -1;
}

const pendingRequest = (it) => ['approval', 'question', 'plan'].includes(it.kind) && it.state === 'pending';

/** Drops item `at` and everything after it, and has the page load the thread again. */
function cut(demo, t, at) {
  const list = demo.transcripts[t.id] || [];
  const gone = list.slice(at);
  demo.transcripts[t.id] = list.slice(0, at);
  demo.say({ type: 'transcript_reset', thread_id: t.id });
  // It was just active: an offer to compact what was there goes with it.
  const fields = { compact_offer: null };
  if (gone.some(pendingRequest) || t.needs) Object.assign(fields, { needs: null, needs_thread: null, run_state: 'idle', section: t.section === 'pinned' ? 'pinned' : 'inbox' });
  demo.update(t.id, fields);
}

/** A side chat's answer: short, as a quick question's is. */
function answerAside(demo, id) {
  const words = 'Short answer: it retries three times, waiting 200 ms, then 400 ms, then 800 ms, and gives up with the last error. The waits are in `retry_policy()` in `src/net/retry.rs`.'.split(' ');
  demo.later(600, () => demo.upsert(id, { id: 'i2', kind: 'tool', call_id: 'c-side', tool: 'search', title: 'Search', detail: 'retry( in src/', status: 'done', output: 'src/net/retry.rs:12', added: null, removed: null }));
  let text = '';
  words.forEach((w, i) => {
    demo.later(900 + i * 70, () => {
      text += (i ? ' ' : '') + w;
      demo.upsert(id, { id: 'i3', kind: 'assistant', text, streaming: i < words.length - 1 });
    });
  });
  demo.later(1000 + words.length * 70, () => {
    demo.upsert(id, { id: 'i4', kind: 'turn_end', at: Date.now(), took_secs: 3 });
    demo.update(id, { run_state: 'idle', activity: null, working_since: null, updated_at: Date.now() });
  });
}

export default {
  /** The demo, seeded after its snapshot (until it seeds itself when it's made). */
  _demo_seed(msg, demo) {
    extras(demo);
    return ACK;
  },

  turn_action(msg, demo) {
    const t = row(demo, msg.thread_id);
    const list = demo.transcripts[t.id] || [];
    const i = list.findIndex((it) => it.id === msg.item_id);
    if (i < 0) throw fail('not_found', 'That message isn’t in this thread any more.');
    if (msg.action !== 'fork' && t.run_state === 'working') throw fail('bad_request', `Stop the running turn to ${msg.action}.`);
    const message = list[i].kind === 'user';
    switch (msg.action) {
      case 'fork': {
        // From a turn: everything up to its end (and what it changed). From a message: what came before it.
        let end = message ? i : i + 1;
        if (!message && list[end]?.kind === 'changes') end += 1;
        const id = `t-fork-${Date.now()}`;
        const fork = { ...t, id, title: `Fork of ${t.title}`, run_state: 'idle', section: 'inbox', needs: null, needs_thread: null, activity: null, working_since: null, pinned: false, unseen: false, updated_at: Date.now() };
        for (const k of ['pause', 'cut_short', 'checks', 'queued', 'background', 'background_tasks', 'sub_agents', 'pending_switch', 'compact_offer', 'shares_folder', 'busy_in_background']) delete fork[k];
        demo.threads.set(id, fork);
        demo.transcripts[id] = list.slice(0, end).filter((it) => !pendingRequest(it)).map((it) => ({ ...it }));
        demo.say({ type: 'thread', thread: { ...fork } });
        return { type: 'ack', thread_id: id, ...(message ? { text: list[i].text } : {}) };
      }
      case 'rewind':
        if (!message) throw fail('bad_request', 'Rewind from one of your messages.');
        cut(demo, t, i);
        return { type: 'ack', text: list[i].text };
      case 'undo':
      case 'retry': {
        const start = startOf(list, i);
        if (start < 0) throw fail('bad_request', 'This turn didn’t start from a message of yours.');
        const text = list[start].text;
        cut(demo, t, start);
        if (msg.action === 'undo') return { type: 'ack', text };
        if (msg.model) demo.update(t.id, { model: msg.model, model_label: LABELS[msg.model] || msg.model });
        demo.later(300, () => carryOn(demo, demo.threads.get(t.id) || t, text));
        return ACK;
      }
      default:
        throw fail('bad_request', `The demo doesn’t do ${msg.action}`);
    }
  },

  turn_preview(msg, demo) {
    const t = row(demo, msg.thread_id);
    if (msg.action === 'fork') throw fail('bad_request', 'A fork asks nothing first');
    if (t.run_state === 'working') throw fail('conflict', `Stop the running turn to ${msg.action}.`);
    const list = demo.transcripts[t.id] || [];
    const i = list.findIndex((it) => it.id === msg.item_id);
    if (i < 0) throw fail('not_found', 'That message isn’t in this thread any more.');
    const start = msg.action === 'rewind' ? i : startOf(list, i);
    if (start < 0) throw fail('bad_request', 'This turn didn’t start from a message of yours.');
    const reply = { type: 'turn_preview', thread_id: t.id, item_id: msg.item_id, recap: t.agent?.key === 'opencode' };
    if (t.project?.id === 'p-notes') return { ...reply, files: null, unavailable: 'This folder isn’t a git repository, so Trek keeps no file checkpoints for it.' };
    // What the turns from there changed: their changed files, and what their edits touched.
    const files = new Map();
    for (const it of list.slice(start)) {
      if (it.kind === 'changes') for (const f of it.files || []) files.set(f.path, f.status === 'added' ? 'delete' : f.status === 'deleted' ? 'bring_back' : 'revert');
      else if (it.kind === 'tool' && it.tool === 'edit' && it.detail && !files.has(it.detail)) files.set(it.detail, it.removed === 0 && it.added ? 'delete' : 'revert');
    }
    return { ...reply, files: [...files].map(([path, change]) => ({ path, change })), unavailable: null };
  },

  context_card(msg, demo) {
    const t = row(demo, msg.thread_id);
    if (!t.context) throw fail('not_found', 'No context reported yet');
    const { used, window } = t.context;
    const reserved = Math.max(0, Math.min(Math.round(window / 30), window - used));
    const system = 21_000;
    const mcp = 9_000;
    const skills = 1_200;
    const list = demo.transcripts[t.id] || [];
    const said = list.filter((i) => ['user', 'assistant', 'tool'].includes(i.kind)).length;
    return {
      type: 'context_card',
      thread_id: t.id,
      used,
      window,
      reserved,
      parts: [
        { kind: 'system', label: 'System', tokens: system, detail: [{ label: 'The agent’s own prompt and its tools’ definitions', value: null }], note: 'Estimated: what the agent counts that isn’t a message, skill or server.' },
        { kind: 'mcp', label: 'MCP', tokens: mcp, detail: [{ label: 'trek-orchestrate, trek-browser', value: '1.8K' }, { label: 'Instructions', value: '7.2K' }], note: 'Estimated, about 900 tokens a server.' },
        { kind: 'skills', label: 'Skills', tokens: skills, detail: [{ label: '15 skills', value: '1.2K' }], note: 'Estimated, about 80 tokens a skill.' },
        { kind: 'messages', label: 'Messages', tokens: Math.max(0, used - system - mcp - skills), detail: [{ label: `${said} messages and tool results`, value: null }], note: 'Estimated from their text (4 characters a token).' },
        { kind: 'free', label: 'Free', tokens: Math.max(0, window - used - reserved), detail: [{ label: 'Room for the conversation, before the agent has to compact.', value: null }], note: null },
        { kind: 'reserved', label: 'Reserved', tokens: reserved, detail: [{ label: 'Kept free so compacting has room to run', value: null }], note: 'Estimated: no agent reports its reserve.' },
      ],
      compact: !list.some((i) => i.kind === 'user') ? 'none' : t.activity === 'Summarizing messages' ? 'running' : t.run_state === 'working' || t.pause ? 'busy' : 'ready',
    };
  },

  background_output(msg, demo) {
    const t = row(demo, msg.thread_id);
    if (!(t.background_tasks || []).some((b) => b.id === msg.task_id)) throw fail('not_found', 'That task isn’t running any more');
    const x = extras(demo);
    x.reads += 1;
    return { type: 'background_output', thread_id: t.id, task_id: msg.task_id, output: devOutput(x.reads) };
  },

  stop_background(msg, demo) {
    const t = row(demo, msg.thread_id);
    const tasks = t.background_tasks || [];
    if (!tasks.some((b) => b.id === msg.task_id)) throw fail('not_found', 'That task isn’t running any more');
    demo.update(t.id, { background_tasks: tasks.map((b) => (b.id === msg.task_id ? { ...b, stopping: true } : b)) });
    demo.later(1200, () => {
      const left = (demo.threads.get(t.id)?.background_tasks || []).filter((b) => b.id !== msg.task_id);
      demo.update(t.id, { background_tasks: left.length ? left : undefined, background: left.length ? left.map((b) => b.title) : undefined });
    });
    return ACK;
  },

  queue_action(msg, demo) {
    const t = row(demo, msg.thread_id);
    const queued = [...(t.queued || [])];
    const places = queued.map((q, i) => i).filter((i) => !!queued[i].reset === !!msg.reset);
    const at = places[msg.index];
    if (at == null || queued[at].text !== msg.text) throw fail('conflict', 'That message isn’t queued any more');
    let taken = null;
    if (msg.action?.kind === 'edit') queued[at] = { ...queued[at], text: msg.action.text };
    else taken = queued.splice(at, 1)[0].text;
    demo.update(t.id, { queued: queued.length ? queued : undefined });
    return taken == null ? ACK : { type: 'ack', text: taken };
  },

  sub_agent(msg, demo) {
    row(demo, msg.thread_id);
    return subAgentLog(msg.thread_id, msg.call_id, extras(demo));
  },

  sub_agent_action(msg, demo) {
    row(demo, msg.thread_id);
    const x = extras(demo);
    const own = OWN[msg.call_id];
    const before = subAgentLog(msg.thread_id, msg.call_id, x);
    if (before.link.state !== 'running') throw fail('bad_request', 'It has already finished.');
    const run = x.runs.get(msg.call_id);
    const elapsed = (Date.now() - run.started) / 1000;
    if (msg.action === 'stop') {
      if (!own.stops) throw fail('bad_request', `${own.link.agent.name} doesn’t let Trek stop its sub-agents.`);
      run.stoppedAt = Date.now() - run.started;
    } else {
      if (!own.messages) throw fail('bad_request', before.no_message || 'It takes no messages.');
      const said = (msg.text || '').trim();
      if (!said) throw fail('bad_request', 'Write something to send.');
      run.extra.push({ at: elapsed, kind: 'user', text: said }, { at: elapsed + 1.5, kind: 'assistant', text: 'Got it: I’ll take that into account, and say so in my report.' });
    }
    const after = subAgentLog(msg.thread_id, msg.call_id, x);
    // Its parent's row says so too.
    if (msg.action === 'stop') {
      const parent = demo.threads.get(msg.thread_id);
      demo.update(msg.thread_id, { sub_agents: (parent.sub_agents || []).map((s) => (s.call_id === msg.call_id ? { ...s, state: 'stopped' } : s)) });
    }
    return after;
  },

  switch_preview(msg, demo) {
    const t = row(demo, msg.thread_id);
    const p = t.pending_switch;
    if (!p) throw fail('not_found', 'No switch waits for its next message');
    return {
      type: 'switch_preview',
      thread_id: t.id,
      text: `You’re picking up a conversation from ${p.from}. The user moved it to you: carry on from here.\n\nWhat they asked: “${t.title}”. ${p.from} read the code around it and made the change; ${t.additions} lines added and ${t.deletions} removed so far, not committed yet.\n\nWhere it was: the last turn ended with the change in place and its tests passing. Nothing waits on an answer.\n\nThe full transcript is on the computer if you need what was left out: search it rather than guess.`,
      tokens: 3_100,
      budget: 24_000,
      left_out: 'Left out: 6 tool outputs and 2 long answers.',
    };
  },

  side_chat(msg, demo) {
    const parent = row(demo, msg.thread_id);
    const x = extras(demo);
    if (x.sideOf.has(parent.id)) throw fail('bad_request', 'A side chat has no side chat of its own');
    if (msg.text == null) {
      const id = [...x.sideOf].filter(([, of]) => of === parent.id).map(([side]) => side).at(-1);
      const side = id && demo.threads.get(id);
      if (!side) return { type: 'side_chat', thread_id: parent.id };
      demo.say({ type: 'unlisted', thread: { ...side } });
      return { type: 'side_chat', thread_id: parent.id, side: { ...side } };
    }
    const text = msg.text.trim();
    if (!text) throw fail('bad_request', 'A side chat starts with a message');
    const id = `t-side-${x.sideOf.size + 1}`;
    const side = {
      ...parent,
      id,
      title: text.split(/\s+/).slice(0, 7).join(' '),
      run_state: 'working',
      section: 'settled',
      needs: null,
      needs_thread: null,
      activity: 'Thinking',
      working_since: Date.now(),
      updated_at: Date.now(),
      access: 'supervised',
      unseen: false,
      pinned: false,
    };
    for (const k of ['pause', 'cut_short', 'checks', 'queued', 'background', 'background_tasks', 'sub_agents', 'pending_switch', 'compact_offer', 'shares_folder', 'busy_in_background', 'context', 'cost']) delete side[k];
    demo.threads.set(id, side);
    x.unlisted.add(id);
    x.sideOf.set(id, parent.id);
    demo.transcripts[id] = [{ id: 'i1', seq: 1, at: Date.now(), kind: 'user', text, images: 0 }];
    demo.say({ type: 'unlisted', thread: { ...side } });
    answerAside(demo, id);
    return { type: 'side_chat', thread_id: parent.id, side: { ...side } };
  },
};
