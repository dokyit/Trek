// The web remote's thread extras, in Node: which items take a turn's actions, the words the bars
// over the composer say, and the demo computer's answers to what they ask.
//
//   node --test script/site/test/

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { backgroundWork, checksWords, cutWords, limitLine, queuedPlaces, queuedPreview, subAgentCounts, switchSentence, tokens } from '../../../site/remote/js/features/thread-extras.js';
import { confirmWords, previewVerb, responseText, retryModels, turnEnds } from '../../../site/remote/js/features/turn-actions.js';
import answers, { seed } from '../../../site/remote/js/demo/thread.js';

const user = (id, text = 'Do it', more = {}) => ({ id, kind: 'user', text, ...more });
const said = (id, text = 'Done') => ({ id, kind: 'assistant', text });
const tool = (id) => ({ id, kind: 'tool', tool: 'read', title: 'Read', detail: 'a.rs' });

test('a turn ends at its turn_end, or at the error, limit or notice it stopped with', () => {
  const items = [user('i1'), tool('i2'), said('i3'), { id: 'i4', kind: 'turn_end' }, { id: 'c4', kind: 'changes' }, user('i5'), tool('i6'), { id: 'i7', kind: 'error', text: 'Interrupted' }, user('i8'), said('i9'), { id: 'i10', kind: 'limit' }];
  assert.deepEqual(turnEnds(items, false), ['i4', 'i7', 'i10']);
});

test('a notice ends a turn only once the turn did something', () => {
  assert.deepEqual(turnEnds([user('i1'), { id: 'i2', kind: 'notice', text: 'Switched to Full access' }], false), []);
  assert.deepEqual(turnEnds([user('i1'), tool('i2'), { id: 'i3', kind: 'notice', text: 'Stopped' }], false), ['i3']);
});

test('the turn still running has no end, and an aside starts no turn', () => {
  const items = [user('i1'), said('i2'), { id: 'i3', kind: 'turn_end' }, user('i4'), tool('i5'), { id: 'i6', kind: 'error' }];
  assert.deepEqual(turnEnds(items, true), ['i3']);
  assert.deepEqual(turnEnds(items, false), ['i3', 'i6']);
  // A typed answer between the work and its error doesn't cut the turn in two.
  assert.deepEqual(turnEnds([user('i1'), tool('i2'), user('i3', '/usage', { aside: true }), { id: 'i4', kind: 'error' }], false), ['i4']);
});

test('an error after a turn ended belongs to that turn', () => {
  assert.deepEqual(turnEnds([user('i1'), said('i2'), { id: 'i3', kind: 'turn_end' }, { id: 'i4', kind: 'error' }], false), ['i3']);
});

test('Copy response takes the turn’s answers, not the turn before', () => {
  const items = [user('i1'), said('i2', 'old'), { id: 'i3', kind: 'turn_end' }, user('i4'), said('i5', 'first'), tool('i6'), said('i7', 'second'), { id: 'i8', kind: 'turn_end' }];
  assert.equal(responseText(items, 'i8'), 'first\n\nsecond');
  assert.equal(responseText(items, 'nope'), '');
});

test('a turn is retried with the agent’s other models', () => {
  const agents = [{ key: 'claude-code', models: [{ id: 'opus', label: 'Opus' }, { id: 'sonnet', label: 'Sonnet' }] }];
  assert.deepEqual(retryModels({ agent: { key: 'claude-code' }, model: 'opus' }, agents).map((m) => m.id), ['sonnet']);
  assert.deepEqual(retryModels({ agent: { key: 'codex' }, model: 'gpt' }, agents), []);
});

test('the confirmation says what goes', () => {
  assert.equal(confirmWords({ action: 'undo' }).verb, 'Undo');
  assert.equal(confirmWords({ action: 'retry', model: { label: 'Sonnet 5.5' } }).title, 'Retry with Sonnet 5.5?');
  assert.equal(confirmWords({ action: 'rewind', edit: true }).title, 'Edit this message?');
  assert.equal(confirmWords({ action: 'rewind' }).verb, 'Rewind');
  assert.deepEqual(['revert', 'delete', 'bring_back', 'nested', 'kept'].map(previewVerb), ['revert', 'delete', 'bring back', 'left as is', 'left as is (newer)']);
});

test('tokens are shortened as Trek shortens them', () => {
  assert.deepEqual([950, 1000, 12_300, 9_999, 171_000, 1_200_000, 2_000_000].map(tokens), ['950', '1K', '12K', '10K', '171K', '1.2M', '2M']);
});

test('the limit bar says when it resets, or resumes', () => {
  const at = () => '4:30 PM';
  const now = 1_000_000;
  assert.equal(limitLine({ resets_at: now + (2 * 3600 + 14 * 60) * 1000, resume: false }, now, at), 'Resets 4:30 PM (in 2h 14m)');
  assert.equal(limitLine({ resets_at: now + 90_000, resume: true }, now, at), 'Resumes at 4:30 PM (in 1m 30s)');
  assert.equal(limitLine({ resets_at: now - 1000, resume: false }, now, at), 'Resets 4:30 PM');
  assert.equal(limitLine({ resets_at: null }, now, at), 'Reset time unknown');
});

test('the bars word what happened as the iPhone does', () => {
  assert.deepEqual(cutWords({ state: 'offered', how: 'Reopens its Codex session' }), { title: 'Interrupted', detail: 'Reopens its Codex session', verb: 'Resume' });
  assert.equal(cutWords({ state: 'offered', waiting: true }).verb, 'Ask again');
  assert.equal(cutWords({ state: 'failed', how: 'h', why: 'Its session is gone' }).detail, 'Its session is gone');
  assert.equal(cutWords({ state: 'resuming', how: 'h' }).verb, null);
  assert.deepEqual(checksWords({ number: 128, failing: ['clippy', 'test (macos)'] }), { title: 'CI failed', detail: '#128: clippy, test (macos)' });
  assert.match(checksWords({ failing: ['lint'], stopped: true }).detail, /^lint\. Trek stopped sending it after 2 tries\.$/);
  assert.deepEqual(switchSentence('Codex', true), ['Your next message switches to Codex.', 'It stops this turn, and the conversation so far goes with it.']);
});

test('a queued message is counted among those waiting for the same thing', () => {
  const places = queuedPlaces([{ text: 'a' }, { text: 'b', reset: true }, { text: 'c' }, { text: 'd', reset: true }]);
  assert.deepEqual(places.map((p) => p.index), [0, 0, 1, 1]);
  assert.equal(queuedPreview({ text: '\n  first line\nsecond' }), 'first line');
  assert.equal(queuedPreview({ text: '', images: 2 }), '2 images');
});

test('background work from a computer that only names it', () => {
  assert.deepEqual(backgroundWork({ background: ['pnpm dev'] }), [{ id: 'title:0', kind: 'shell', title: 'pnpm dev' }]);
  assert.equal(backgroundWork({ background_tasks: [{ id: 'b1' }], background: ['x'] })[0].id, 'b1');
  assert.deepEqual(backgroundWork({}), []);
});

test('the sub-agents’ header counts them, and starts its clock at the first still at it', () => {
  const counts = subAgentCounts([
    { state: 'running', since: 50 },
    { state: 'needs_you', since: 20 },
    { state: 'done', since: 5 },
    { state: 'failed' },
  ]);
  assert.deepEqual(counts, { working: 1, waiting: 1, failed: 1, since: 20 });
});

// ---------------------------------------------------------------------------------------------
// The demo computer

/** Enough of the demo's `DemoClient` for its answers. */
class Demo {
  constructor() {
    const row = (id, title, extra = {}) => ({ id, title, project: { id: 'p-api', name: 'trek-api' }, agent: { key: 'claude-code', name: 'Claude Code' }, run_state: 'idle', section: 'inbox', ...extra });
    this.features = new Set();
    this.threads = new Map([
      ['t-done', row('t-done', 'Why is the cache cold?')],
      ['t-work', row('t-work', 'Move the build', { run_state: 'working' })],
      ['t-plan', row('t-plan', 'Dark mode', { project: { id: 'p-web', name: 'web' } })],
    ]);
    this.transcripts = {
      't-done': [user('i1', 'Why is the cache cold?'), said('i2'), { id: 'i3', kind: 'turn_end' }, { id: 'c3', kind: 'changes', files: [{ path: 'src/key.rs', status: 'modified' }, { path: 'src/new.rs', status: 'added' }] }],
      't-work': [user('i1', 'Move it'), tool('i2')],
    };
    this.said = [];
    this.timers = [];
  }

  say(msg) {
    this.said.push(msg);
  }

  update(id, fields) {
    const t = this.threads.get(id);
    if (!t) return;
    Object.assign(t, fields);
    this.say({ type: 'thread', thread: { ...t } });
  }

  upsert(threadId, it) {
    (this.transcripts[threadId] ||= []).push(it);
    this.say({ type: 'item', thread_id: threadId, item: it });
  }

  later(ms, fn) {
    this.timers.push(fn);
  }

  reply() {}

  action(t, a) {
    throw Object.assign(new Error(`The demo doesn’t do ${a.kind}`), { code: 'bad_request' });
  }
}

test('the demo seeds its threads once, with this area’s features', () => {
  const demo = new Demo();
  const x = seed(demo);
  assert.equal(seed(demo), x);
  for (const f of ['limit_actions', 'queue_action', 'side_chat', 'turn_preview']) assert.ok(demo.features.has(f), f);
  for (const id of ['t-limit', 't-cut', 't-ci', 't-subs', 't-switch']) assert.ok(demo.threads.has(id), id);
  assert.ok(x.unlisted.has('t-subs-bench'));
  // Sub-agents' rows go out as the list leaves them out.
  demo.say({ type: 'snapshot', threads: [...demo.threads.values()] });
  const snapshot = demo.said.at(-1);
  assert.ok(!snapshot.threads.some((t) => t.id === 't-subs-review'));
  assert.ok(snapshot.unlisted.some((t) => t.id === 't-subs-review'));
});

test('the demo answers the bars’ thread actions, and leaves the rest to itself', () => {
  const demo = new Demo();
  seed(demo);
  demo.action(demo.threads.get('t-limit'), { kind: 'resume_at_reset' });
  assert.equal(demo.threads.get('t-limit').pause.resume, true);
  assert.throws(() => demo.action(demo.threads.get('t-done'), { kind: 'resume_at_reset' }), { code: 'conflict' });
  assert.throws(() => demo.action(demo.threads.get('t-limit'), { kind: 'continue_on_account', account: 'other' }), /Work has/);
  assert.throws(() => demo.action(demo.threads.get('t-done'), { kind: 'pin' }), { code: 'bad_request' });
});

test('a queued message is changed or taken back, unless it moved on', () => {
  const demo = new Demo();
  seed(demo);
  const ask = (index, text, action) => answers.queue_action({ type: 'queue_action', thread_id: 't-work', index, text, action }, demo);
  ask(1, 'And open a pull request against main.', { kind: 'edit', text: 'And open it as a draft.' });
  assert.equal(demo.threads.get('t-work').queued[1].text, 'And open it as a draft.');
  assert.deepEqual(ask(0, 'Then update the build section of the README.', { kind: 'remove' }), { type: 'ack', text: 'Then update the build section of the README.' });
  assert.throws(() => ask(0, 'Then update the build section of the README.', { kind: 'remove' }), { code: 'conflict' });
});

test('undo takes the turn back to before its message, and gives the message back', () => {
  const demo = new Demo();
  const ack = answers.turn_action({ type: 'turn_action', thread_id: 't-done', item_id: 'i3', action: 'undo' }, demo);
  assert.deepEqual(ack, { type: 'ack', text: 'Why is the cache cold?' });
  assert.deepEqual(demo.transcripts['t-done'], []);
  assert.ok(demo.said.some((m) => m.type === 'transcript_reset' && m.thread_id === 't-done'));
  assert.throws(() => answers.turn_action({ type: 'turn_action', thread_id: 't-work', item_id: 'i1', action: 'rewind' }, demo), { code: 'bad_request', message: 'Stop the running turn to rewind.' });
});

test('a fork from a message starts before it, and brings the message along', () => {
  const demo = new Demo();
  const ack = answers.turn_action({ type: 'turn_action', thread_id: 't-done', item_id: 'i1', action: 'fork' }, demo);
  assert.equal(ack.text, 'Why is the cache cold?');
  assert.deepEqual(demo.transcripts[ack.thread_id], []);
  const whole = answers.turn_action({ type: 'turn_action', thread_id: 't-done', item_id: 'i3', action: 'fork' }, demo);
  assert.deepEqual(demo.transcripts[whole.thread_id].map((i) => i.id), ['i1', 'i2', 'i3', 'c3']);
});

test('the preview lists what restoring would do to each file', () => {
  const demo = new Demo();
  const p = answers.turn_preview({ type: 'turn_preview', thread_id: 't-done', item_id: 'i3', action: 'undo' }, demo);
  assert.deepEqual(p.files, [{ path: 'src/key.rs', change: 'revert' }, { path: 'src/new.rs', change: 'delete' }]);
  assert.throws(() => answers.turn_preview({ type: 'turn_preview', thread_id: 't-done', item_id: 'i3', action: 'fork' }, demo), { code: 'bad_request' });
});

test('the context card’s parts fill its window', () => {
  const demo = new Demo();
  seed(demo);
  const card = answers.context_card({ type: 'context_card', thread_id: 't-switch' }, demo);
  assert.equal(card.parts.reduce((n, p) => n + p.tokens, 0), card.window);
  assert.equal(card.compact, 'ready');
  assert.throws(() => answers.context_card({ type: 'context_card', thread_id: 't-done' }, demo), { code: 'not_found' });
});
