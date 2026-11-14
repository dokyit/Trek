// The computer's own screens' plain code (site/remote/js/features/mac-text.js), and the demo
// computer's answers for them (site/remote/js/demo/mac.js): limits and reset times, a search hit's
// marked words, the settings a change leaves, where a chart's bars and labels go, the rules'
// words, and the demo's search, tester and refusals.
//
//   node --test script/site/test/

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applySettings,
  asOf,
  barAt,
  barLayout,
  barPath,
  decidedBy,
  effortLabel,
  effortsFor,
  highlight,
  httpsAddress,
  isNtfySh,
  left,
  localMatches,
  mergeResults,
  newThreadsLine,
  outcomeLook,
  peak,
  phrase,
  reviewLook,
  ruleShown,
  systemThemeLabel,
  tickLayout,
  tightest,
  until,
  usedTone,
} from '../../../site/remote/js/features/mac-text.js';
import demo from '../../../site/remote/js/demo/mac.js';

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

test('a limit says what is left, coloured as the Mac colours it', () => {
  assert.equal(left(42), 58);
  assert.equal(left(100), 0);
  assert.equal(left(120), 0);
  assert.equal(left(0.4), 100);
  assert.equal(usedTone(69), '');
  assert.equal(usedTone(70), 'approval');
  assert.equal(usedTone(90), 'failed');
  const usage = { providers: [{ limits: [{ label: 'a', percent: 12 }, { label: 'b', percent: 64 }] }, { limits: [] }] };
  assert.equal(peak(usage), 64);
  assert.equal(peak(null), 0);
  assert.equal(tightest(usage.providers[0]).label, 'b');
  assert.equal(tightest(usage.providers[1]), null);
});

test('reset times read as the Usage card says them', () => {
  const now = Date.UTC(2026, 10, 14, 12, 0, 0);
  assert.equal(until(now + 20_000, now), 'in under a minute');
  assert.equal(until(now + 42 * MIN, now), 'in 42m');
  assert.equal(until(now + 2 * HOUR + 10 * MIN, now), 'in 2h 10m');
  assert.match(until(now + 4 * DAY + HOUR, now, 'en-US'), /^\w{3} \d{1,2}:\d{2}\s?[AP]M \(in 4d\)$/);
  assert.equal(until(now - HOUR, now), 'in under a minute');
  assert.equal(asOf(now - 30_000, now), 'As of just now');
  assert.equal(asOf(now - 40 * MIN, now), 'As of 40m ago');
  assert.equal(asOf(now - 3 * DAY, now), 'As of 3d ago');
});

test('a search hit marks its words, counting Unicode scalar values as the Mac does', () => {
  assert.deepEqual(highlight('two refreshes race', [[4, 13], [14, 18]]), [
    { text: 'two ', hit: false },
    { text: 'refreshes', hit: true },
    { text: ' ', hit: false },
    { text: 'race', hit: true },
  ]);
  // An emoji is one scalar value (two UTF-16 units): the range after it still lands on its word.
  assert.deepEqual(highlight('🚀 deploy cache', [[2, 8]]), [
    { text: '🚀 ', hit: false },
    { text: 'deploy', hit: true },
    { text: ' cache', hit: false },
  ]);
  // Ranges that don't fit are left out; overlapping ones join.
  assert.deepEqual(highlight('abc', [[2, 9], [-1, 1], [1, 1], 'x']), [{ text: 'abc', hit: false }]);
  assert.deepEqual(highlight('abcdef', [[0, 3], [2, 4]]), [
    { text: 'abcd', hit: true },
    { text: 'ef', hit: false },
  ]);
  assert.deepEqual(highlight('', [[0, 1]]), []);
});

test('search shows the computer’s finds while they answer what is typed, then the phone’s own', () => {
  const threads = [
    { id: 'a', title: 'Fix the race', updated_at: 1, project: { name: 'api' } },
    { id: 'b', title: 'Release', updated_at: 3, branch: 'race/fix' },
    { id: 'c', title: 'Notes', updated_at: 2, agent: { name: 'Codex' } },
  ];
  assert.deepEqual(localMatches(threads, '').map((t) => t.id), ['b', 'c', 'a']);
  assert.deepEqual(localMatches(threads, 'RACE').map((t) => t.id), ['b', 'a']);
  assert.deepEqual(localMatches(threads, 'codex').map((t) => t.id), ['c']);

  const found = [{ thread: { id: 'z' }, snippet: '…race…' }, { thread: { id: 'a' } }];
  const local = localMatches(threads, 'race');
  // The answer for "rac" still shows while "race" is typed; the phone's own follow, once each.
  assert.deepEqual(mergeResults({ query: 'race', found, foundFor: 'rac', local }).map((r) => r.thread.id), ['z', 'a', 'b']);
  assert.deepEqual(mergeResults({ query: 'ra', found, foundFor: 'rac', local }).map((r) => r.thread.id), ['z', 'a', 'b']);
  // An answer for something else isn't shown.
  assert.deepEqual(mergeResults({ query: 'race', found, foundFor: 'cache', local }).map((r) => r.thread.id), ['b', 'a']);
  assert.deepEqual(mergeResults({ query: '', found, foundFor: 'race', local: [] }), []);
});

test('a change shows at once as the computer would make it', () => {
  const s = { default_agent: 'claude-code', default_model: 'claude-opus-5-5', push: { enabled: false, when: 'away', server: 'https://ntfy.sh' }, send_failed_checks: ['p1'], theme: 'system' };
  assert.deepEqual(applySettings(s, { default_agent: 'codex' }), { ...s, default_agent: 'codex', default_model: null });
  assert.equal(applySettings(s, { default_agent: 'claude-code' }).default_model, 'claude-opus-5-5');
  assert.equal(applySettings(s, { default_model: '' }).default_model, null);
  const pushed = applySettings(s, { push: true, push_when: 'always', push_server: 'https://ntfy.example.com', push_test: true, new_push_topic: true });
  assert.deepEqual(pushed.push, { enabled: true, when: 'always', server: 'https://ntfy.example.com' });
  assert.equal(s.push.enabled, false, 'the settings it was given stay as they were');
  assert.deepEqual(applySettings(s, { send_failed_checks: { project_id: 'p2', on: true } }).send_failed_checks, ['p1', 'p2']);
  assert.deepEqual(applySettings(s, { send_failed_checks: { project_id: 'p1', on: false } }).send_failed_checks, []);
  assert.equal(applySettings(s, { theme: 'paper' }).theme, 'paper');
});

test('a phone sets an https:// ntfy server only', () => {
  assert.equal(httpsAddress('https://ntfy.sh'), true);
  assert.equal(httpsAddress('  https://ntfy.example.com/  '), true);
  assert.equal(httpsAddress('http://ntfy.example.com'), false);
  assert.equal(httpsAddress('ntfy.example.com'), false);
  assert.equal(httpsAddress('https://'), false);
  assert.equal(httpsAddress(''), false);
  assert.equal(isNtfySh('https://ntfy.sh/'), true);
  assert.equal(isNtfySh('https://ntfy.example.com'), false);
});

test('the settings’ words: new threads, efforts, the theme', () => {
  const agents = [
    { key: 'claude-code', name: 'Claude Code', default_model: 'opus', models: [{ id: 'opus', label: 'Opus 5.5', efforts: ['low', 'high', 'xhigh'] }, { id: 'sonnet', label: 'Sonnet 5.5', efforts: ['low'] }] },
    { key: 'opencode', name: 'OpenCode', default_model: null, models: [] },
  ];
  assert.equal(newThreadsLine({ default_agent: 'claude-code', default_access: 'auto-accept-edits' }, agents), 'Claude Code · Opus 5.5 · Auto-accept edits');
  assert.equal(newThreadsLine({ default_agent: 'claude-code', default_model: 'sonnet', default_access: 'auto' }, agents), 'Claude Code · Sonnet 5.5 · Auto');
  assert.equal(newThreadsLine({ default_agent: 'gone', default_model: 'm-1', default_access: 'supervised' }, agents), 'gone · m-1 · Supervised');
  assert.equal(newThreadsLine({ session_approvals: true }, agents), null);
  assert.deepEqual(effortsFor(agents, 'claude-code', null), ['low', 'high', 'xhigh']);
  assert.deepEqual(effortsFor(agents, 'claude-code', 'sonnet'), ['low']);
  assert.deepEqual(effortsFor(agents, 'opencode', null), []);
  assert.equal(effortLabel('xhigh'), 'Extra high');
  assert.equal(effortLabel('medium'), 'Medium');
  assert.equal(systemThemeLabel('windows'), 'Match Windows');
  assert.equal(systemThemeLabel('macos'), 'Match macOS');
  assert.equal(systemThemeLabel(undefined), 'System');
});

test('the chart’s bars fill their slots, 24 wide at most, and the axis keeps labels apart', () => {
  const week = barLayout(7, 350);
  assert.equal(week.gap, 3);
  assert.equal(week.bar, 24);
  const day = barLayout(24, 350);
  assert.ok(day.bar < 24 && day.bar > 10);
  assert.equal(barLayout(90, 350).gap, 1);
  assert.deepEqual(barLayout(0, 350), { slot: 0, gap: 0, bar: 0 });
  assert.equal(barAt(0, 24, 350), 0);
  assert.equal(barAt(349, 24, 350), 23);
  assert.equal(barAt(-5, 24, 350), 0);
  assert.equal(barAt(500, 24, 350), 23);
  // Rounded on top, square on the baseline; a short bar's corners shrink to fit.
  assert.equal(barPath(0, 10, 50, 96), 'M0 96V50Q0 46 4 46H6Q10 46 10 50V96Z');
  assert.match(barPath(0, 10, 2, 96), /^M0 96V96Q0 94 2 94H8Q10 94 10 96V96Z$/);
  const measure = (s) => s.length * 6;
  const ticks = tickLayout([{ bar: 0, label: '12 AM' }, { bar: 1, label: '1 AM' }, { bar: 12, label: 'Noon' }, { bar: 23, label: '11 PM' }, { bar: 40, label: 'gone' }], 24, day, 350, measure);
  assert.deepEqual(ticks.map((t) => t.label), ['12 AM', 'Noon', '11 PM']);
  assert.equal(ticks[0].x, 15, 'kept inside the left edge');
  assert.equal(ticks[2].x, 350 - 15, 'and the right one');
});

test('the rules’ and requests’ words', () => {
  assert.equal(ruleShown({ kind: 'command', pattern: 'rm', action: 'deny' }), 'Deny command “rm”');
  assert.equal(ruleShown({ kind: 'edit', pattern: '~/.ssh/**', action: 'allow' }), 'Allow file edit “~/.ssh/**”');
  assert.deepEqual(outcomeLook('refused'), ['Refused', 'failed']);
  assert.deepEqual(outcomeLook('later'), ['?', '']);
  assert.equal(decidedBy({}), 'No rule decides this: the thread’s level does.');
  assert.equal(phrase('Run command'), 'run a command');
  assert.equal(phrase('Edit file'), 'change files');
  assert.equal(phrase('Fetch'), 'go online');
  assert.equal(phrase('mcp__github__create_issue'), 'use mcp__github__create_issue');
  assert.deepEqual(reviewLook({ status: 'needs_you', label: 'Question' }), { icon: 'question', tone: 'approval' });
  assert.deepEqual(reviewLook({ status: 'needs_you', label: 'Something new' }), { icon: 'alert', tone: 'approval' });
  assert.deepEqual(reviewLook({ status: 'failed' }), { icon: 'mac-circle-x', tone: 'failed' });
  assert.deepEqual(reviewLook({ status: 'done' }), { icon: 'mac-circle-check', tone: 'done' });
});

// ---------------------------------------------------------------------------------------------
// The demo computer's answers

/** A demo computer as demo.js makes one: threads, transcripts, and answering a request. */
function fakeDemo() {
  const row = (id, title, extra = {}) => ({
    id,
    title,
    project: { id: 'p-api', name: 'trek-api', hue: 212, monogram: 'TA' },
    agent: { key: 'claude-code', name: 'Claude Code', logo: 'claude-code' },
    run_state: 'idle',
    section: 'inbox',
    updated_at: Date.now() - HOUR,
    ...extra,
  });
  const threads = new Map(
    [
      row('t-flaky', 'Fix the flaky auth test', { needs: { kind: 'approval', text: 'Run' }, unseen: true }),
      row('t-done', 'Why is the cache cold after deploys?'),
      row('t-web', 'Dark mode for Settings', { project: { id: 'p-web', name: 'web', hue: 28, monogram: 'WE' }, agent: { key: 'codex', name: 'Codex' } }),
    ].map((t) => [t.id, t]),
  );
  const transcripts = {
    't-flaky': [{ id: 'i7', kind: 'approval', request_id: 'r1', title: 'Run command', detail: 'cargo test -p auth -- --test-threads 8', state: 'pending', always_allow: { kind: 'command', pattern: 'cargo test -p auth' } }],
  };
  const answered = [];
  return { host: { name: 'Demo computer', version: 'demo' }, threads, transcripts, answered, answer: (msg) => answered.push(msg) };
}

test('the demo’s tester words a deciding rule as the computer does', () => {
  const d = fakeDemo();
  const t = demo.rule_test({ kind: 'command', text: 'rm -rf build; echo done' }, d);
  assert.equal(t.decided_by, 'Deny command “rm” decides, because this line runs “rm -rf build”.');
  assert.equal(t.rule, 0);
  assert.deepEqual(t.levels.map((l) => l.outcome), ['refused', 'refused', 'refused', 'refused']);
  const none = demo.rule_test({ kind: 'command', text: 'ls -la' }, d);
  assert.equal(none.decided_by, undefined);
  assert.deepEqual(none.levels.map((l) => l.outcome), ['asks', 'asks', 'runs', 'runs']);
  assert.equal(demo.rule_test({ kind: 'edit', text: '~/.ssh/id_ed25519' }, d).levels[3].outcome, 'refused');
  assert.equal(demo.rule_test({ kind: 'fetch', text: 'https://docs.rs/serde' }, d).levels[0].outcome, 'runs');
  assert.equal(demo.rule_test({ kind: 'tool', text: 'mcp__github__create_issue' }, d).levels[3].outcome, 'asks');
});

test('the demo writes rules only as the computer lets it, and checks an always-allow covers its card', () => {
  const d = fakeDemo();
  const list = demo.rules({}, d);
  assert.equal(list.rules.length, 5);
  assert.equal(list.editable, true);
  assert.throws(() => demo.rule_action({ action: 'remove', index: 0, rule: { kind: 'command', pattern: 'git push', action: 'ask' } }, d), { code: 'conflict' });
  assert.equal(demo.rule_action({ action: 'add', rule: { kind: 'command', pattern: ' make ', action: 'deny' } }, d).rules.at(-1).pattern, 'make');
  assert.throws(() => demo.always_allow({ thread_id: 't-flaky', request_id: 'r1', pattern: 'cargo build' }, d), { code: 'bad_request' });
  assert.deepEqual(demo.always_allow({ thread_id: 't-flaky', request_id: 'r1', pattern: 'cargo test' }, d), { type: 'ack' });
  assert.equal(d.answered[0].response.decision, 'allow');
  assert.ok(demo.rules({}, d).rules.some((r) => r.pattern === 'cargo test' && r.action === 'allow'));
  // Off on the computer: the list says so, and writing is refused.
  demo.set_settings({}, d);
  d.macScreens.settings.phone_rules = false;
  assert.equal(demo.rules({}, d).editable, false);
  assert.throws(() => demo.rule_action({ action: 'add', rule: { kind: 'command', pattern: 'x', action: 'deny' } }, d), { code: 'bad_request' });
});

test('the demo’s settings refuse what the computer refuses, and make a topic when push goes on', () => {
  const d = fakeDemo();
  assert.throws(() => demo.set_settings({ push_server: 'http://ntfy.example.com' }, d), { code: 'bad_request' });
  assert.throws(() => demo.set_settings({ default_access: 'full-access' }, d), { code: 'bad_request' });
  const s = demo.set_settings({ default_agent: 'codex', push_server: 'https://ntfy.example.com/' }, d);
  assert.equal(s.type, 'settings');
  assert.equal(s.default_agent, 'codex');
  assert.equal(s.default_model, undefined);
  assert.equal(s.push.topic_url, `https://ntfy.example.com/${s.push.topic}`);
  assert.equal(s.push.subscribe_url, `ntfy://ntfy.example.com/${s.push.topic}`);
  const before = s.push.topic;
  assert.notEqual(demo.set_settings({ new_push_topic: true }, d).push.topic, before);
  assert.throws(() => demo.set_settings({ push: false, push_test: true }, d), { code: 'bad_request' });
});

test('the demo’s search finds titles, then messages with their words marked', async () => {
  const d = fakeDemo();
  const titles = await demo.search({ query: 'cache', limit: 50 }, d);
  assert.equal(titles.type, 'search_results');
  assert.deepEqual(titles.results.map((r) => r.thread.title).sort(), ['Warm the CDN cache after deploys', 'Why is the cache cold after deploys?']);
  const words = await demo.search({ query: 'refresh race', limit: 50 }, d);
  const hit = words.results.find((r) => r.thread.id === 't-sub-auth');
  assert.ok(hit, 'a sub-agent’s thread is found by what was said');
  assert.equal(hit.thread.parent_id, 't-flaky');
  const marked = highlight(hit.snippet, hit.ranges).filter((p) => p.hit).map((p) => p.text);
  assert.ok(marked.includes('race'));
  assert.ok(marked.some((w) => w.startsWith('refresh')));
  assert.deepEqual((await demo.search({ query: 're', limit: 50 }, d)).results.filter((r) => r.snippet), [], 'messages from three letters a word');
});
