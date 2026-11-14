// The demo computer's answers for its own screens (features/mac*.js): plan usage for Claude Code,
// Codex and OpenCode, with a Codex reset to spend; Basecamp from the demo's threads, today by the
// hour, this week by the day and all time by the week; search over titles and what was said, a
// thread settled weeks ago and a sub-agent's among the finds; the computer's settings as Trek
// keeps them; five permission rules with their tester, and "Always allow…"; and what the machine
// is. Refusals are worded as the computer words them.

const MIN = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;

function refuse(code, message) {
  return Object.assign(new Error(message), { code });
}

/** What these screens keep on the demo computer, made the first time one asks. */
function kept(demo) {
  if (!demo.macScreens) {
    demo.macScreens = {
      settings: {
        default_agent: 'claude-code',
        default_effort: 'high',
        default_access: 'auto-accept-edits',
        follow_up: 'steer',
        notifications: 'banner_and_sound',
        push: withLinks({ enabled: true, when: 'away', server: 'https://ntfy.sh', topic: 'trek-q7Hx2VbK9pL3mN8RtY4wZ6cE1fJ5aD0s' }),
        auto_settle_days: 3,
        theme: 'system',
        full_access: false,
        session_approvals: true,
        resume_interrupted: false,
        continue_on_other_account: true,
        // This demo computer lets the phone write rules (a real one starts with it off).
        phone_rules: true,
        send_failed_checks: ['p-api'],
      },
      rules: [
        { kind: 'command', pattern: 'rm', action: 'deny' },
        { kind: 'command', pattern: 'git push', action: 'ask' },
        { kind: 'edit', pattern: '~/.ssh/**', action: 'deny' },
        { kind: 'fetch', pattern: 'docs.rs', action: 'allow' },
        { kind: 'tool', pattern: 'mcp__github__*', action: 'ask' },
      ],
      credits: [{ id: 'rc-1', title: 'Full reset (Weekly + 5 hr)', description: 'Resets the weekly and 5-hour limits now', expires_at: Date.now() + 9 * DAY }],
      codexReset: false,
      usageAsked: false,
      hidden: false,
    };
  }
  return demo.macScreens;
}

/** ntfy's links for the topic, as the computer makes them. */
function withLinks(push) {
  if (!push.topic) return { ...push, topic_url: undefined, subscribe_url: undefined };
  const server = push.server.trim().replace(/\/+$/, '');
  const host = server.split('://')[1] || server;
  return { ...push, topic_url: `${server}/${push.topic}`, subscribe_url: `ntfy://${host}/${push.topic}${server.startsWith('http://') ? '?secure=false' : ''}` };
}

function newTopic() {
  const letters = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789';
  let topic = 'trek-';
  for (let i = 0; i < 32; i++) topic += letters[Math.floor(Math.random() * letters.length)];
  return topic;
}

/** A thread row's agent or project, as the demo's own rows carry it. */
function agentRef(demo, key) {
  return [...demo.threads.values()].find((t) => t.agent?.key === key)?.agent || { key, name: key };
}

function projectRefs(demo) {
  const seen = new Map();
  for (const t of demo.threads.values()) if (t.project?.id && !seen.has(t.project.id)) seen.set(t.project.id, t.project);
  return [...seen.values()];
}

// ---------------------------------------------------------------------------------------------
// Usage

function usage(msg, demo) {
  const m = kept(demo);
  const now = Date.now();
  const codexUsed = (p) => (m.codexReset ? 0 : p);
  const providers = [
    {
      agent: agentRef(demo, 'claude-code'),
      plan: 'Claude Max',
      limits: [
        { label: '5-hour limit', percent: 42, resets_at: now + 2 * HOUR + 10 * MIN, window: '5h' },
        { label: 'Weekly limit', percent: 18, resets_at: now + 4 * DAY + 3 * HOUR, window: '7d' },
        { label: 'Weekly · Opus', percent: 64, resets_at: now + 4 * DAY + 3 * HOUR, window: '7d' },
      ],
    },
    {
      agent: agentRef(demo, 'codex'),
      plan: 'ChatGPT Pro',
      limits: [
        { label: '5-hour limit', percent: codexUsed(91), resets_at: now + (m.codexReset ? 5 * HOUR : 58 * MIN), window: '5h' },
        { label: 'Weekly limit', percent: codexUsed(33), resets_at: now + (m.codexReset ? 7 * DAY : 5 * DAY + 10 * HOUR), window: '7d' },
      ],
      ...(m.credits.length ? { resets: m.credits.map((c) => ({ ...c })) } : {}),
    },
  ];
  // The first ask is answered before OpenCode has said, as a real computer's can be.
  if (!m.usageAsked) {
    m.usageAsked = true;
    return { type: 'usage', providers, loading: true, picked: false };
  }
  providers.push({ agent: agentRef(demo, 'opencode'), limits: [], no_limits: true, today: 'Today in Trek: 48K tokens', as_of: now - 40 * MIN });
  return { type: 'usage', providers, picked: false };
}

function useReset(msg, demo) {
  const m = kept(demo);
  if (!m.credits.some((c) => c.id === msg.credit_id)) throw refuse('not_found', 'That reset isn’t available any more');
  return new Promise((resolve) => {
    setTimeout(() => {
      m.credits = m.credits.filter((c) => c.id !== msg.credit_id);
      m.codexReset = true;
      resolve({ type: 'ack' });
    }, 800);
  });
}

// ---------------------------------------------------------------------------------------------
// Basecamp

const PER_TURN = 41_300;

/** "12K", "1.4M", as the computer words tokens. */
function tokensLabel(n) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${Math.round(n / 1000)}K`;
  if (n < 10_000_000) return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`;
  return `${Math.round(n / 1_000_000)}M`;
}

function cost(tokens) {
  return `$${((tokens / 1_000_000) * 4.6).toFixed(2)}`;
}

function agentTime(secs) {
  if (secs < 60) return '<1m';
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

function bar(label, turns) {
  const tokens = turns * PER_TURN;
  const line = [label, `${turns} ${turns === 1 ? 'turn' : 'turns'}`, ...(tokens > 0 ? [`${tokensLabel(tokens)} tokens`] : [])].join(' · ');
  return { label, turns, tokens, line };
}

const hourWord = (h) => `${h % 12 === 0 ? 12 : h % 12} ${h % 24 < 12 ? 'AM' : 'PM'}`;
const dayWord = (d, opts) => d.toLocaleDateString('en-US', opts);

/** The range's turns, a bar a stretch, ending now: the hours today, the days this week, the weeks since June. */
function chartFor(range, now) {
  if (range === 'today') {
    // This hour, then the hours before it: a working day that ends now, whatever the time.
    const recent = [3, 6, 9, 5, 4, 11, 7, 2, 6, 9, 8, 3, 1, 2];
    const h = now.getHours();
    const bars = Array.from({ length: 24 }, (_, i) => bar(`${hourWord(i)}–${hourWord(i + 1)}`, i <= h ? recent[h - i] || 0 : 0));
    const ticks = [[0, '12 AM'], [6, '6 AM'], [12, 'Noon'], [18, '6 PM']].map(([b, label]) => ({ bar: b, label }));
    return { step: 'hour', bars, ticks };
  }
  const weekday = (now.getDay() + 6) % 7;
  const monday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - weekday);
  if (range === 'week') {
    const shape = [34, 52, 41, 63, 47, 12, 8];
    const days = Array.from({ length: 7 }, (_, i) => new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() + i));
    const bars = days.map((d, i) => bar(`${dayWord(d, { weekday: 'short' })}, ${dayWord(d, { month: 'short', day: 'numeric' })}`, i <= weekday ? shape[i] : 0));
    return { step: 'day', bars, ticks: days.map((d, i) => ({ bar: i, label: dayWord(d, { weekday: 'short' }) })) };
  }
  const n = 19;
  const weeks = Array.from({ length: n }, (_, i) => new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() - 7 * (n - 1 - i)));
  const bars = weeks.map((d, i) => bar(`Week of ${dayWord(d, { month: 'short', day: 'numeric' })}`, Math.round(110 + 90 * Math.sin(i * 0.55) + i * 9)));
  const ticks = [0, 4, 8, 12, 16].map((i) => ({ bar: i, label: dayWord(weeks[i], { month: 'short', day: 'numeric' }) }));
  return { step: 'week', bars, ticks };
}

function reviewRow(t) {
  const label = t.needs ? { approval: 'Approval', question: 'Question', plan: 'Plan to review' }[t.needs.kind] || 'Needs you' : t.run_state === 'failed' ? 'Failed' : null;
  const status = t.needs ? 'needs_you' : t.run_state === 'failed' ? 'failed' : 'done';
  return {
    thread_id: t.id,
    title: t.title,
    status,
    label,
    agent: t.agent,
    project: t.project,
    additions: t.additions || 0,
    deletions: t.deletions || 0,
    updated_at: t.updated_at,
    unseen: !!t.unseen,
    // As a real computer's: a failure settled is history, and raises no hand.
    needs_you: status === 'needs_you' || (status === 'failed' && t.section !== 'settled'),
  };
}

function greeting(now, range) {
  const h = now.getHours();
  const part = h < 12 ? 'Good morning' : h < 18 ? 'Good afternoon' : 'Good evening';
  if (range === 'all') return `${part} — on the trail since 2 June`;
  return `${part}, ${now.toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long' })}`;
}

function basecamp(msg, demo) {
  const range = ['today', 'week', 'all'].includes(msg.range) ? msg.range : 'today';
  const now = new Date();
  const rows = [...demo.threads.values()].filter((t) => t.run_state !== 'working' && (t.needs || t.unseen || t.run_state === 'failed'));
  const review = rows.map(reviewRow).sort((a, b) => b.needs_you - a.needs_you || b.updated_at - a.updated_at);
  const { step, bars, ticks } = chartFor(range, now);
  const turns = bars.reduce((sum, b) => sum + b.turns, 0);
  const tokens = bars.reduce((sum, b) => sum + b.tokens, 0);
  const [threads, failed] = { today: [6, 1], week: [17, 3], all: [142, 21] }[range];
  const secs = turns * 74;
  // Each row's share of the range: threads, turns, tokens and failures alike.
  const row = (label, share, extra = {}, priced = true) => {
    const t = Math.round(tokens * share);
    return { label, threads: Math.max(1, Math.round(threads * share)), turns: Math.round(turns * share), tokens: t, tokens_label: tokensLabel(t), cost: priced ? cost(t) : '—', failed: 0, ...extra };
  };
  const named = projectRefs(demo).slice(0, 3);
  const projects = [...named.map((p, i) => row(p.name, [0.48, 0.27, 0.17][i], { project: p, failed: i === 0 ? failed : 0 })), row('No project', 0.08)];
  const models = [
    row('Claude Opus 5.5', 0.55, { agent: agentRef(demo, 'claude-code'), failed: Math.min(1, failed) }),
    row('GPT-6', 0.28, { agent: agentRef(demo, 'codex'), failed: Math.max(0, failed - 1) }),
    row('Claude Sonnet 5.5', 0.11, { agent: agentRef(demo, 'claude-code') }),
    row('Kimi K3', 0.06, { agent: agentRef(demo, 'opencode') }, false),
  ];
  return {
    type: 'basecamp',
    range,
    greeting: greeting(now, range),
    title: { today: 'Today’s trek', week: 'This week’s trek', all: 'Your trek so far' }[range],
    updated_at: Date.now(),
    review,
    // What phones from before `basecamp_v2` draw.
    narrative: [
      { kind: 'text', text: range === 'today' ? 'You sent ' : range === 'week' ? 'This week you sent ' : 'So far you’ve sent ' },
      { kind: 'strong', text: `${Math.round(turns * 0.9)} prompts` },
      { kind: 'text', text: ' across ' },
      { kind: 'strong', text: `${threads} threads` },
      { kind: 'text', text: '.' },
    ],
    tiles: [],
    totals: { threads, turns, tokens, tokens_label: tokensLabel(tokens), cost: cost(tokens), failed, agent_secs: secs, agent_time: agentTime(secs) },
    chart: { step, bars, ticks, turns, total: `${turns} ${turns === 1 ? 'turn' : 'turns'}` },
    projects,
    models,
  };
}

// ---------------------------------------------------------------------------------------------
// Search

/** Threads the demo's list doesn't hold, as a real computer's search still finds them: one settled weeks ago, and a sub-agent's. */
function hideAway(demo) {
  const m = kept(demo);
  if (m.hidden) return;
  m.hidden = true;
  const now = Date.now();
  const base = demo.threads.get('t-done') || [...demo.threads.values()][0];
  const flaky = demo.threads.get('t-flaky') || base;
  const web = projectRefs(demo).find((p) => p.id === 'p-web') || base.project;
  const rows = [
    { ...base, id: 't-cdn', title: 'Warm the CDN cache after deploys', project: web, section: 'settled', run_state: 'idle', needs: null, unseen: false, pinned: false, updated_at: now - 36 * DAY, additions: 28, deletions: 4, branch: 'main', worktree: false },
    { ...base, id: 't-sub-auth', title: 'Survey the auth tests', project: flaky.project, agent: flaky.agent, parent_id: 't-flaky', section: 'settled', run_state: 'idle', needs: null, unseen: false, pinned: false, updated_at: now - 4 * MIN, additions: 0, deletions: 0, branch: flaky.branch, worktree: flaky.worktree },
  ];
  const said = {
    't-cdn': [
      ['user', 'After a deploy the first visitors wait while the CDN cache fills. Can we warm it before anyone arrives?'],
      ['assistant', 'I added a step to the deploy script that requests the 40 most visited pages once the new build is live, so the cache is warm before the first visitor. A cold start now costs about 3 seconds of the deploy instead of a slow first page.'],
    ],
    't-sub-auth': [
      ['user', 'Read every test under tests/auth and list the ones that share state between runs.'],
      ['assistant', 'Two tests share the token store: refresh_expired and refresh_concurrent. When two concurrent refreshes race on the token, the second write wins and the first request fails with a 401.'],
    ],
  };
  for (const row of rows) {
    // Kept with the demo's threads, so a message sent there carries on as anywhere else; the list
    // learns of one only when it changes.
    demo.threads.set(row.id, row);
    demo.transcripts[row.id] ||= said[row.id].map(([kind, text], i) => ({ id: `i${i + 1}`, seq: i + 1, at: row.updated_at - (2 - i) * MIN, kind, text, ...(kind === 'user' ? { images: 0 } : { streaming: false }) }));
  }
}

const WORD = /[\p{L}\p{N}_]+/gu;

function wordsOf(text) {
  return [...text.matchAll(WORD)].map((m) => ({ word: m[0].toLowerCase(), at: m.index, length: m[0].length }));
}

/** Code points before UTF-16 index `at`. */
function points(text, at) {
  return Array.from(text.slice(0, at)).length;
}

/**
 * A message that has every word of the query (as word prefixes, three letters and up): an excerpt
 * on one line where its words sit closest together, without markdown's marks, its words marked.
 */
function messageHit(text, query) {
  const flat = text.replace(/[*`#>]/g, '').replace(/\s+/g, ' ').trim();
  const wanted = wordsOf(query).map((w) => w.word).filter((w) => w.length >= 3);
  if (!wanted.length) return null;
  const words = wordsOf(flat);
  const hits = [];
  for (const q of wanted) {
    const found = words.filter((w) => w.word.startsWith(q)).map((w) => ({ ...w, q }));
    if (!found.length) return null;
    hits.push(...found);
  }
  hits.sort((a, b) => a.at - b.at);
  // The hit with the most of the query's words in the next few words.
  const near = (h) => new Set(hits.filter((x) => x.at >= h.at && x.at - h.at < 50).map((x) => x.q)).size;
  const best = hits.reduce((a, b) => (near(b) > near(a) ? b : a));
  const all = Array.from(flat);
  const first = points(flat, best.at);
  const from = Math.max(0, first - 30);
  const to = Math.min(all.length, from + 100);
  const lead = from > 0 ? '…' : '';
  const snippet = `${lead}${all.slice(from, to).join('')}${to < all.length ? '…' : ''}`;
  const ranges = [];
  for (const h of hits) {
    const start = points(flat, h.at);
    const end = start + Array.from(flat.slice(h.at, h.at + h.length)).length;
    if (start >= from && end <= to) ranges.push([start - from + lead.length, end - from + lead.length]);
  }
  return { snippet, ranges };
}

function titleHit(title, query) {
  const t = title.toLowerCase();
  const q = query.toLowerCase();
  if (t.includes(q)) return true;
  const words = wordsOf(t).map((w) => w.word);
  const wanted = wordsOf(q).map((w) => w.word);
  return wanted.length > 0 && wanted.every((w) => words.some((x) => x.startsWith(w)));
}

function search(msg, demo) {
  hideAway(demo);
  const query = String(msg.query || '').trim();
  const limit = Math.min(Math.max(Number(msg.limit) || 50, 1), 200);
  const results = [];
  if (query) {
    const threads = [...demo.threads.values()];
    for (const t of threads) if (titleHit(t.title || '', query)) results.push({ thread: { ...t } });
    for (const t of threads) {
      if (results.some((r) => r.thread.id === t.id)) continue;
      for (const it of demo.transcripts[t.id] || []) {
        if ((it.kind !== 'user' && it.kind !== 'assistant') || !it.text) continue;
        const hit = messageHit(it.text, query);
        if (hit) {
          results.push({ thread: { ...t }, ...hit });
          break;
        }
      }
    }
  }
  return new Promise((resolve) => setTimeout(() => resolve({ type: 'search_results', query: msg.query, results: results.slice(0, limit) }), 250));
}

// ---------------------------------------------------------------------------------------------
// Settings

function settings(msg, demo) {
  return { type: 'settings', ...structuredClone(kept(demo).settings) };
}

function setSettings(msg, demo) {
  const m = kept(demo);
  const s = structuredClone(m.settings);
  const projects = new Set(projectRefs(demo).map((p) => p.id));
  // One value the computer won't take refuses the whole change.
  if (msg.default_access === 'full-access' && !s.full_access) throw refuse('bad_request', 'Full access is locked on this computer: unlock it in Trek’s Settings › Permissions first.');
  if (msg.push_server != null && !/^https:\/\/[^/\s]+/i.test(String(msg.push_server).trim())) throw refuse('bad_request', 'The ntfy server must be an https:// address.');
  if (msg.push_test && !(msg.push ?? s.push.enabled)) throw refuse('bad_request', 'Notifications to the phone are off.');
  if (msg.send_failed_checks && !projects.has(msg.send_failed_checks.project_id)) throw refuse('not_found', 'That project isn’t on this computer any more.');
  if (msg.auto_settle_days != null && !(Number.isInteger(msg.auto_settle_days) && msg.auto_settle_days >= 0 && msg.auto_settle_days <= 365)) throw refuse('bad_request', 'Settle after 0 to 365 days.');
  if (msg.default_agent != null && msg.default_agent !== s.default_agent) {
    s.default_agent = msg.default_agent;
    delete s.default_model;
  }
  if (msg.default_model != null) {
    if (msg.default_model === '') delete s.default_model;
    else s.default_model = msg.default_model;
  }
  for (const key of ['default_effort', 'default_access', 'follow_up', 'notifications', 'auto_settle_days', 'theme', 'resume_interrupted', 'continue_on_other_account']) {
    if (msg[key] != null) s[key] = msg[key];
  }
  if (msg.push != null) {
    s.push.enabled = msg.push;
    // Turning it on makes a topic the first time.
    if (msg.push && !s.push.topic) s.push.topic = newTopic();
  }
  if (msg.push_when != null) s.push.when = msg.push_when;
  if (msg.push_server != null) s.push.server = String(msg.push_server).trim();
  if (msg.new_push_topic) s.push.topic = newTopic();
  s.push = withLinks(s.push);
  if (msg.send_failed_checks) {
    const { project_id: id, on } = msg.send_failed_checks;
    s.send_failed_checks = s.send_failed_checks.filter((p) => p !== id);
    if (on) s.send_failed_checks.push(id);
  }
  m.settings = s;
  return { type: 'settings', ...structuredClone(s) };
}

// ---------------------------------------------------------------------------------------------
// Permission rules

const LEVELS = ['supervised', 'auto-accept-edits', 'auto', 'full-access'];
const KIND_WORD = { command: 'command', edit: 'file edit', fetch: 'web fetch', tool: 'tool' };

const shown = (r) => `${r.action[0].toUpperCase()}${r.action.slice(1)} ${KIND_WORD[r.kind] || r.kind} “${r.pattern}”`;
const same = (a, b) => !!a && !!b && a.kind === b.kind && a.pattern === b.pattern && a.action === b.action;

function glob(pattern, text) {
  const source = pattern
    .split('**')
    .map((part) => part.split('*').map((s) => s.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('[^/]*'))
    .join('.*');
  return new RegExp(`^${source}$`).test(text);
}

/** Whether `rule` covers `text` (one command of a line, a path, an address, a tool's name). */
function covers(rule, text) {
  const t = text.trim();
  switch (rule.kind) {
    case 'command':
      return t === rule.pattern || t.startsWith(`${rule.pattern} `);
    case 'edit':
      return rule.pattern.includes('/') ? glob(rule.pattern, t) : glob(rule.pattern, t.split('/').pop());
    case 'fetch': {
      let host = t;
      try {
        host = new URL(/^[a-z]+:\/\//i.test(t) ? t : `https://${t}`).hostname;
      } catch {
        // Taken as it is.
      }
      return host === rule.pattern || host.endsWith(`.${rule.pattern}`);
    }
    default:
      return glob(rule.pattern, t);
  }
}

/** What the rules say about a call, as the computer's tester words it: deny is tried first, then ask, then allow. */
function tryRules(rules, kind, text) {
  const parts = kind === 'command' ? text.split(/[;&|]+/).map((s) => s.trim()).filter(Boolean) : [text.trim()];
  for (const verdict of ['deny', 'ask', 'allow']) {
    for (const [i, rule] of rules.entries()) {
      if (rule.kind !== kind || rule.action !== verdict) continue;
      const part = parts.find((p) => covers(rule, p));
      if (!part) continue;
      const outcome = { deny: 'refused', ask: 'asks', allow: 'runs' }[verdict];
      const why = part === text.trim() ? `${shown(rule)} decides.` : `${shown(rule)} decides, because this line runs “${part}”.`;
      return { type: 'rule_test', kind, text, decided_by: why, rule: i, levels: LEVELS.map((level) => ({ level, outcome })) };
    }
  }
  const asks = (level) => level === 'supervised' || (kind === 'command' && level === 'auto-accept-edits');
  return { type: 'rule_test', kind, text, levels: LEVELS.map((level) => ({ level, outcome: asks(level) ? 'asks' : 'runs' })) };
}

function rulesList(m) {
  return { type: 'rules', rules: m.rules.map((r) => ({ ...r })), editable: !!m.settings.phone_rules };
}

function mayWrite(m) {
  if (!m.settings.phone_rules) throw refuse('bad_request', 'Writing rules from a phone is off on this computer. Turn it on in Trek’s Settings › Phone.');
}

function ruleAction(msg, demo) {
  const m = kept(demo);
  mayWrite(m);
  if (msg.action === 'add') {
    const rule = msg.rule || {};
    const pattern = String(rule.pattern || '').trim();
    if (!KIND_WORD[rule.kind] || !['allow', 'ask', 'deny'].includes(rule.action)) throw refuse('bad_request', 'That isn’t a rule Trek knows.');
    if (!pattern || /[\r\n]/.test(pattern)) throw refuse('bad_request', 'A rule needs a pattern, on one line.');
    const added = { kind: rule.kind, pattern, action: rule.action };
    if (!m.rules.some((r) => same(r, added))) m.rules.push(added);
  } else if (msg.action === 'remove') {
    if (!same(m.rules[msg.index], msg.rule)) throw refuse('conflict', 'Your rules have changed since. Look at them again.');
    m.rules.splice(msg.index, 1);
  } else {
    throw refuse('bad_request', 'Add a rule or remove one.');
  }
  return rulesList(m);
}

function alwaysAllow(msg, demo) {
  const m = kept(demo);
  mayWrite(m);
  const req = (demo.transcripts[msg.thread_id] || []).find((i) => i.request_id === msg.request_id);
  if (!req || req.kind !== 'approval') throw refuse('not_found', 'No such request');
  if (req.state !== 'pending') throw refuse('conflict', 'That was answered already');
  if (!req.always_allow) throw refuse('bad_request', 'There’s no rule to write for this one.');
  const rule = { kind: req.always_allow.kind, pattern: String(msg.pattern || '').trim(), action: 'allow' };
  if (!rule.pattern) throw refuse('bad_request', 'A rule needs a pattern.');
  // The rule has to let this card's call through, with the rest of the list.
  const verdict = tryRules([...m.rules, rule], rule.kind, req.detail || '');
  const deciding = verdict.rule != null ? [...m.rules, rule][verdict.rule] : null;
  if (!deciding || deciding.action !== 'allow') {
    throw refuse('bad_request', deciding ? `${shown(deciding)} would still decide this one.` : `An allow for “${rule.pattern}” wouldn’t cover “${req.detail}”.`);
  }
  if (!m.rules.some((r) => same(r, rule))) m.rules.push(rule);
  demo.answer({ thread_id: msg.thread_id, request_id: msg.request_id, response: { kind: 'approval', decision: 'allow' } });
  return { type: 'ack' };
}

// ---------------------------------------------------------------------------------------------

export default {
  usage,
  use_reset: useReset,
  basecamp,
  search,
  settings,
  set_settings: setSettings,
  rules: (msg, demo) => rulesList(kept(demo)),
  rule_action: ruleAction,
  rule_test: (msg, demo) => tryRules(kept(demo).rules, msg.kind, String(msg.text || '')),
  always_allow: alwaysAllow,
  machine_info: (msg, demo) => ({ type: 'machine_info', name: demo.host?.name || 'Demo computer', platform: 'windows', trek_version: demo.host?.version || 'demo', home: 'C:\\Users\\you' }),
};
