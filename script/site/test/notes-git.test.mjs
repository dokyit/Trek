// The web remote's Notes and Git, in Node: a note's text as its preview draws it and the edits its
// editor makes, a unified diff as the Git screen draws it, and the demo's answers to both.
//
//   node --test script/site/test/

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { register } from 'node:module';

// markdown.js needs a DOM (DOMPurify); nothing here draws, so a stand-in takes its place.
register(
  `data:text/javascript,${encodeURIComponent(`
export async function resolve(specifier, context, next) {
  if (specifier.endsWith('/markdown.js')) {
    return { shortCircuit: true, url: 'data:text/javascript,export const Markdown = () => null; export const toHtml = (t) => t; export const Plain = () => null;' };
  }
  return next(specifier, context);
}`)}`,
);

const notes = await import('../../../site/remote/js/features/notes.js');
const git = await import('../../../site/remote/js/features/git.js');
const demoNotes = (await import('../../../site/remote/js/demo/notes.js')).default;
const demoGit = (await import('../../../site/remote/js/demo/git.js')).default;

// ---------------------------------------------------------------------------------------------
// Notes

test('only files kept with the notes are attachments', () => {
  assert.equal(notes.attachment('attachments/3f2a.png'), 'attachments/3f2a.png');
  assert.equal(notes.attachment('./attachments/3f2a.png'), 'attachments/3f2a.png');
  for (const bad of ['attachments/../secret', 'attachments/sub/x.png', 'attachments/.hidden', 'attachments/', '/etc/passwd', 'https://example.com/a.png', 'C:\\x.png', 'attachments/a\\b.png']) {
    assert.equal(notes.attachment(bad), null, bad);
  }
});

test('a line of nothing but pictures, and a line that is only a file', () => {
  assert.deepEqual(notes.imagesOnly('![Board](attachments/a.png)  ![](<attachments/b c.png> "title")'), [
    { alt: 'Board', link: 'attachments/a.png' },
    { alt: '', link: 'attachments/b c.png' },
  ]);
  assert.equal(notes.imagesOnly('See ![Board](attachments/a.png)'), null);
  assert.equal(notes.imagesOnly('plain text'), null);
  assert.deepEqual(notes.fileOnly('[Sync flow.pdf](attachments/9c1e.pdf)'), { name: 'Sync flow.pdf', path: 'attachments/9c1e.pdf' });
  assert.equal(notes.fileOnly('[Site](https://example.com)'), null);
  assert.equal(notes.fileOnly('[Doc](attachments/9c1e.pdf) and more'), null);
});

test('a note splits into the pieces its preview draws, a code fence left whole', () => {
  const body = [
    '# Sync flow', //            0
    '',
    '![Board](attachments/a.png)', // 2
    '- [ ] Queue sends', //      3
    '  - [x] Retry', //          4
    'Between', //                5
    '[Notes.pdf](attachments/n.pdf)', // 6
    '```', //                    7
    '- [ ] not a box',
    '![not](attachments/a.png)',
    '```',
    'After',
  ].join('\n');
  const blocks = notes.noteBlocks(body);
  assert.deepEqual(
    blocks.map((b) => b.kind),
    ['md', 'images', 'checks', 'md', 'file', 'md'],
  );
  assert.deepEqual(blocks[2].items, [
    { line: 3, done: false, indent: 0, text: 'Queue sends' },
    { line: 4, done: true, indent: 1, text: 'Retry' },
  ]);
  assert.equal(blocks[4].file.path, 'attachments/n.pdf');
  assert.equal(blocks[5].text, '```\n- [ ] not a box\n![not](attachments/a.png)\n```\nAfter');
  assert.equal(blocks[5].line, 7);
  assert.deepEqual(notes.noteBlocks(''), []);
});

test('a box ticks on its own line, whatever its bullet and indent', () => {
  const text = 'Groceries\n- [ ] Oat milk\n  * [x] Bread';
  assert.equal(notes.toggleCheck(text, 1), 'Groceries\n- [x] Oat milk\n  * [x] Bread');
  assert.equal(notes.toggleCheck(text, 2), 'Groceries\n- [ ] Oat milk\n  * [ ] Bread');
  assert.equal(notes.toggleCheck(text, 0), null);
  assert.equal(notes.toggleCheck(text, 9), null);
});

test('a note is titled by its first line with words in it', () => {
  assert.equal(notes.noteTitle('# Groceries\n- milk'), 'Groceries');
  assert.equal(notes.noteTitle('\n\n- [ ] **Buy** <mark>milk</mark>'), 'Buy milk');
  assert.equal(notes.noteTitle('![](attachments/a.png)\n![Board](attachments/b.png)'), 'Board');
  assert.equal(notes.noteTitle(''), 'Untitled');
  assert.equal(notes.noteTitle('x'.repeat(120)).length, 80);
});

test('return carries a list on, and ends it after an empty item', () => {
  const at = (text) => text.length;
  assert.deepEqual(notes.continueList('- milk\n', at('- milk\n')), { text: '- milk\n- ', cursor: 9 });
  assert.deepEqual(notes.continueList('3. three\n', 9), { text: '3. three\n4. ', cursor: 12 });
  assert.deepEqual(notes.continueList('  - [x] done\n', 13), { text: '  - [x] done\n  - [ ] ', cursor: 21 });
  assert.deepEqual(notes.continueList('- a\n- \n', 7), { text: '- a\n', cursor: 4 });
  assert.equal(notes.continueList('# Title\n', 8), null);
  assert.equal(notes.continueList('plain\n', 6), null);
  // In the middle of the text: what follows stays after the new marker.
  assert.deepEqual(notes.continueList('- a\n\nend', 4), { text: '- a\n- \nend', cursor: 6 });
});

test('a picture goes in on lines of its own, the cursor after it', () => {
  assert.deepEqual(notes.insertLines('before after', 6, 6, '![](attachments/p.jpg)'), { text: 'before\n![](attachments/p.jpg)\n after', cursor: 30 });
  assert.deepEqual(notes.insertLines('', 0, 0, '![](attachments/p.jpg)'), { text: '![](attachments/p.jpg)\n', cursor: 23 });
  assert.deepEqual(notes.insertLines('a\nb', 2, 2, 'X'), { text: 'a\nX\nb', cursor: 4 });
  assert.deepEqual(notes.insertLines('a\n', 2, 2, 'X'), { text: 'a\nX\n', cursor: 4 });
  // A selection is replaced.
  assert.deepEqual(notes.insertLines('a gone b', 2, 6, 'X'), { text: 'a \nX\n b', cursor: 5 });
});

test('files open as what they are, and their sizes read as a phone says them', () => {
  assert.equal(notes.fileKind('attachments/a.PDF'), 'pdf');
  for (const v of ['a.mp4', 'a.m4v', 'a.mov', 'a.webm']) assert.equal(notes.fileKind(v), 'video');
  assert.equal(notes.fileKind('attachments/a.csv'), 'other');
  assert.equal(notes.fileKind('attachments/noext'), 'other');
  assert.equal(notes.bytes(986), '986 bytes');
  assert.equal(notes.bytes(270_000), '270 KB');
  assert.equal(notes.bytes(1_700_000), '1.7 MB');
  assert.equal(notes.bytes(3_000_000), '3 MB');
});

// ---------------------------------------------------------------------------------------------
// Git

test('a diff is its hunks: numbers from the @@ line, the header left out', () => {
  const diff = [
    'diff --git a/src/a.rs b/src/a.rs',
    'index 2b7a9e1..c4d0f55 100644',
    '--- a/src/a.rs',
    '+++ b/src/a.rs',
    '@@ -10,4 +10,5 @@ impl A {',
    ' keep',
    '-let x = 1;',
    '+let x = 2;',
    '+added',
    ' tail',
    '\\ No newline at end of file',
    '',
  ].join('\n');
  const rows = git.parseDiff(diff);
  assert.deepEqual(
    rows.map((r) => [r.kind, r.number ?? null, r.text]),
    [
      ['hunk', null, '@@ -10,4 +10,5 @@ impl A {'],
      ['context', 10, 'keep'],
      ['removed', 11, 'let x = 1;'],
      ['added', 11, 'let x = 2;'],
      ['added', 12, 'added'],
      ['context', 13, 'tail'],
      ['note', null, 'No newline at end of file'],
    ],
  );
});

test('an untracked file, every line +, is one hunk from line 1', () => {
  const rows = git.parseDiff('+first\n+second');
  assert.deepEqual(
    rows.map((r) => [r.kind, r.number, r.text]),
    [
      ['added', 1, 'first'],
      ['added', 2, 'second'],
    ],
  );
  assert.deepEqual(git.parseDiff(''), []);
});

test('a second file header starts over: its header is left out too', () => {
  const rows = git.parseDiff(['diff --git a/a b/a', '--- a/a', '+++ b/a', '@@ -1 +1 @@', '-a', '+b', 'diff --git a/c b/c', '--- a/c', '+++ b/c', '@@ -5,0 +6 @@', '+c'].join('\n'));
  assert.deepEqual(
    rows.map((r) => r.kind),
    ['hunk', 'removed', 'added', 'hunk', 'added'],
  );
  assert.equal(rows[4].number, 6);
});

test('what changed in a line replaced by another stands out', () => {
  assert.deepEqual(git.changedParts([...'let x = 1;'], [...'let x = 2;']), { old: [8, 9], new: [8, 9] });
  assert.deepEqual(git.changedParts([...'use tokio::sync::RwLock;'], [...'use tokio::sync::{Mutex, RwLock};']), { old: [17, 23], new: [17, 32] });
  // Only added to: nothing on the old side stands out.
  assert.deepEqual(git.changedParts([...'call(a, b)'], [...'call(a, b, c)']), { old: null, new: [9, 12] });
  assert.equal(git.changedParts([...'abc'], [...'xyz']), null);
  assert.equal(git.changedParts([...'a long line that changed'], [...'z completely different text']), null);
  const rows = git.parseDiff('@@ -1,3 +1,2 @@\n-let x = 1;\n-gone\n+let x = 2;');
  // Two removed, one added: not a pair, nothing marked.
  assert.ok(rows.every((r) => !r.changed));
  const paired = git.parseDiff('@@ -1 +1 @@\n-let x = 1;\n+let x = 2;');
  assert.deepEqual(paired[1].changed, [8, 9]);
  assert.deepEqual(paired[2].changed, [8, 9]);
});

test('names, folders and how many are viewed', () => {
  assert.equal(git.nameOf('src/auth/session.rs'), 'session.rs');
  assert.equal(git.dirOf('src/auth/session.rs'), 'src/auth');
  assert.equal(git.dirOf('README.md'), '');
  assert.equal(git.viewedLabel(0, 3), null);
  assert.equal(git.viewedLabel(1, 3), '1 of 3 viewed');
});

// ---------------------------------------------------------------------------------------------
// The demo's answers

function fakeDemo() {
  const thread = (id, project, extra) => ({ id, title: id, project: { id: project, name: project }, branch: 'main', worktree: false, run_state: 'idle', ...extra });
  const threads = new Map(
    [thread('t-flaky', 'p-api', { branch: 'fix/auth-flake', worktree: true }), thread('t-scope', 'p-api'), thread('t-work', 'p-web', { run_state: 'working' }), thread('t-pin', 'p-notes', { branch: null })].map((t) => [t.id, t]),
  );
  return {
    threads,
    updates: [],
    items: [],
    update(id, fields) {
      Object.assign(threads.get(id), fields);
      this.updates.push([id, fields]);
    },
    upsert(id, item) {
      this.items.push([id, item]);
    },
  };
}

test('demo notes: listed newest first, saved, and refused when changed since', () => {
  const demo = fakeDemo();
  const list = demoNotes.notes({}, demo).notes;
  assert.ok(list.length >= 5);
  assert.ok(list.every((n, i) => i === 0 || list[i - 1].modified >= n.modified));
  assert.equal(list.find((n) => n.id === 'n-sync').title, 'Sync flow');
  const note = demoNotes.note({ note_id: 'n-groceries' }, demo).note;
  const saved = demoNotes.save_note({ note_id: note.id, body: `${note.body}\n- [ ] Eggs`, modified: note.modified }, demo).note;
  assert.ok(saved.modified > note.modified);
  assert.throws(() => demoNotes.save_note({ note_id: note.id, body: 'stale', modified: note.modified }, demo), { code: 'conflict' });
  const made = demoNotes.create_note({ body: '' }, demo).note;
  assert.equal(made.title, 'Untitled');
  assert.equal(demoNotes.delete_note({ note_id: made.id }, demo).type, 'ack');
  assert.throws(() => demoNotes.note({ note_id: made.id }, demo), { code: 'not_found' });
});

test('demo notes: a file comes in pieces, and only from the notes\' attachments', async () => {
  const demo = fakeDemo();
  const csv = 'attachments/0d4c8e2a6b1f3e5d7c9a0b2d4f6e8a1c.csv';
  const first = await demoNotes.note_file({ path: csv, offset: 0, length: 1 << 20 }, demo);
  assert.equal(first.mime, 'text/csv');
  assert.ok(first.total > 1 << 20, 'more than one piece');
  let got = Buffer.from(first.bytes_b64, 'base64').length;
  while (got < first.total) {
    const piece = await demoNotes.note_file({ path: csv, offset: got }, demo);
    got += Buffer.from(piece.bytes_b64, 'base64').length;
  }
  assert.equal(got, first.total);
  const end = await demoNotes.note_file({ path: csv, offset: first.total }, demo);
  assert.equal(end.bytes_b64, '');
  await assert.rejects(demoNotes.note_file({ path: csv, offset: first.total + 1 }, demo), { code: 'bad_request' });
  const pdf = await demoNotes.note_file({ path: './attachments/9c1e2b7f4a3d5e6f8091a2b3c4d5e6f7.pdf', offset: 0 }, demo);
  assert.ok(Buffer.from(pdf.bytes_b64, 'base64').toString('latin1').startsWith('%PDF-1.4'));
  for (const bad of ['../notes.md', 'attachments/../x', '/etc/passwd', 'attachments/sub/x.pdf']) {
    await assert.rejects(demoNotes.note_file({ path: bad, offset: 0 }, demo), { code: 'bad_request' }, bad);
  }
  await assert.rejects(demoNotes.note_file({ path: 'attachments/gone.pdf', offset: 0 }, demo), { code: 'not_found' });
});

test('demo git: a worktree thread, its refusals in the computer\'s words, and removing it', () => {
  const demo = fakeDemo();
  const s = demoGit.git_status({ thread_id: 't-flaky' }, demo);
  assert.equal(s.branch, 'fix/auth-flake');
  assert.equal(s.worktree.base, 'main');
  assert.equal(s.worktree.merge_blocked, '2 files aren’t committed yet. Commit or revert them first.');
  assert.equal(s.can_switch, false);
  assert.match(s.switch_blocked, /^This thread works in a worktree: fix\/auth-flake stays checked out there/);
  for (const f of s.files) assert.equal(f.diff, undefined, 'diffs come one at a time');
  const d = demoGit.git_diff({ thread_id: 't-flaky', path: 'src/auth/session.rs' }, demo);
  const rows = git.parseDiff(d.diff);
  const file = s.files.find((f) => f.path === 'src/auth/session.rs');
  assert.equal(rows.filter((r) => r.kind === 'added').length, file.added);
  assert.equal(rows.filter((r) => r.kind === 'removed').length, file.removed);
  assert.throws(() => demoGit.git_switch({ thread_id: 't-flaky', branch: 'main' }, demo), { code: 'conflict' });
  assert.throws(() => demoGit.worktree_merge({ thread_id: 't-flaky' }, demo), { code: 'conflict', message: s.worktree.merge_blocked });
  assert.throws(() => demoGit.worktree_remove({ thread_id: 't-flaky', delete_branch: true }, demo), {
    code: 'conflict',
    message: '2 uncommitted changes in the worktree would be lost. fix/auth-flake has 2 commits that main doesn’t have.',
  });
  assert.equal(demoGit.worktree_remove({ thread_id: 't-flaky', force: true }, demo).type, 'ack');
  assert.equal(demo.threads.get('t-flaky').worktree, false);
  assert.equal(demoGit.git_status({ thread_id: 't-flaky' }, demo).worktree, undefined);
  assert.equal(demo.items[0][1].kind, 'notice');
});

test('demo git: a project folder commits, pushes, and switches once it is clean', () => {
  const demo = fakeDemo();
  assert.throws(() => demoGit.git_switch({ thread_id: 't-scope', branch: 'feat/rate-limit' }, demo), { code: 'conflict', message: /would be overwritten by checkout/ });
  assert.throws(() => demoGit.git_commit({ thread_id: 't-scope', message: '  ' }, demo), { code: 'bad_request', message: 'Write a commit message first.' });
  const viewed = demoGit.set_viewed({ project_id: 'p-api', path: 'src/cache/key.rs', viewed: true }, demo);
  assert.deepEqual(viewed.viewed.sort(), ['README.md', 'src/cache/key.rs']);
  assert.throws(() => demoGit.set_viewed({ project_id: 'p-api', path: 'nope.rs', viewed: true }, demo), { code: 'not_found' });
  assert.equal(demoGit.git_commit({ thread_id: 't-scope', message: 'Key the cache on content' }, demo).type, 'ack');
  const after = demoGit.git_status({ thread_id: 't-scope' }, demo);
  assert.deepEqual(after.files, []);
  assert.equal(after.ahead, 1);
  assert.equal(after.viewed, undefined);
  demoGit.git_push({ thread_id: 't-scope' }, demo);
  assert.equal(demoGit.git_status({ project_id: 'p-api' }, demo).ahead, 0);
  assert.equal(demoGit.git_switch({ thread_id: 't-scope', branch: 'feat/rate-limit' }, demo).type, 'ack');
  assert.equal(demoGit.git_branches({ project_id: 'p-api' }, demo).current, 'feat/rate-limit');
  assert.equal(demo.threads.get('t-scope').branch, 'feat/rate-limit');
});

test('demo git: a folder a thread works in stays put, a long diff is cut short, and no repository is said so', () => {
  const demo = fakeDemo();
  const web = demoGit.git_status({ thread_id: 't-work' }, demo);
  assert.equal(web.switch_blocked, 'A thread is working in this folder: switch once it’s done.');
  assert.throws(() => demoGit.git_switch({ thread_id: 't-work', branch: 'chore/vite-7' }, demo), { code: 'conflict', message: web.switch_blocked });
  const long = demoGit.git_diff({ project_id: 'p-web', path: 'src/data/airports.json' }, demo);
  assert.equal(long.truncated, true);
  assert.ok(long.diff.length <= 256 << 10);
  assert.ok(!long.diff.endsWith('\n'));
  assert.equal(demoGit.git_diff({ project_id: 'p-web', path: 'src/styles/tokens.css' }, demo).truncated, undefined);
  assert.ok(git.parseDiff(demoGit.git_diff({ project_id: 'p-web', path: 'src/styles/tokens.css' }, demo).diff).length > 400, 'long enough to open folded');
  assert.throws(() => demoGit.git_diff({ project_id: 'p-web', path: 'unchanged.ts' }, demo), { code: 'not_found' });
  assert.equal(demoGit.git_status({ thread_id: 't-pin' }, demo).is_repo, false);
  assert.throws(() => demoGit.git_branches({ thread_id: 't-pin' }, demo), { code: 'bad_request' });
  assert.throws(() => demoGit.git_status({ thread_id: 'nope' }, demo), { code: 'not_found' });
});
