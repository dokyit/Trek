// The web remote's media helpers, in Node: a photo made small enough to send, a file's pieces,
// the `/` picker's matching, dictation's words joined to what was typed, a message's trailing file
// mentions, and the words sizes and times are shown in.
//
//   node --test script/site/test/

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_UPLOAD,
  PHOTO_PX,
  UPLOAD_PIECE,
  baseName,
  captionFor,
  clock,
  commandQuery,
  displayName,
  fitWithin,
  isWeb,
  joinWords,
  matchCommands,
  mediaPath,
  mergeDictation,
  photoTries,
  sizeWords,
  splitMentions,
  tileSide,
  toBase64,
  toBytes,
  uploadPieces,
} from '../../../site/remote/js/features/media.js';

test('a picture fits its long side, never grows, and keeps its shape', () => {
  assert.deepEqual(fitWithin(4032, 3024, 2048), { width: 2048, height: 1536 });
  assert.deepEqual(fitWithin(3024, 4032, 2048), { width: 1536, height: 2048 });
  assert.deepEqual(fitWithin(800, 600, 2048), { width: 800, height: 600 });
  assert.deepEqual(fitWithin(10000, 1, 2048), { width: 2048, height: 1 });
});

test('a photo is tried at the most pixels first, its quality lowered before its size', () => {
  const tries = photoTries(4032, 3024);
  assert.deepEqual(tries[0], { width: PHOTO_PX, height: 1536, quality: 0.85 });
  for (const t of tries) assert.ok(Math.max(t.width, t.height) <= PHOTO_PX);
  // Every quality at one size, then the next size down.
  assert.deepEqual(
    tries.slice(0, 6).map((t) => [t.width, t.quality]),
    [
      [2048, 0.85],
      [2048, 0.75],
      [2048, 0.65],
      [2048, 0.55],
      [2048, 0.45],
      [1600, 0.85],
    ],
  );
  // A small picture isn't tried again at the same size.
  const small = photoTries(640, 480);
  assert.equal(small.length, 5);
  assert.ok(small.every((t) => t.width === 640 && t.height === 480));
});

test('a file goes up in pieces of 4 MiB at most, one after another; an empty one is one empty piece', () => {
  assert.equal(UPLOAD_PIECE, 4 * 1024 * 1024);
  assert.equal(MAX_UPLOAD, 2 ** 31);
  assert.deepEqual(uploadPieces(0), [{ offset: 0, length: 0 }]);
  assert.deepEqual(uploadPieces(10), [{ offset: 0, length: 10 }]);
  const total = 10 * 1024 * 1024;
  const pieces = uploadPieces(total);
  assert.deepEqual(
    pieces.map((p) => p.length),
    [UPLOAD_PIECE, UPLOAD_PIECE, total - 2 * UPLOAD_PIECE],
  );
  let next = 0;
  for (const p of pieces) {
    assert.equal(p.offset, next, 'each starts where the last ended');
    next += p.length;
  }
  assert.equal(next, total);
  assert.equal(uploadPieces(2 * UPLOAD_PIECE).length, 2);
});

test('base64 goes both ways', () => {
  const bytes = new Uint8Array(70_000).map((_, i) => (i * 31) & 255);
  assert.deepEqual(toBytes(toBase64(bytes)), bytes);
  assert.equal(toBase64(new Uint8Array([255, 216, 255])), '/9j/');
  assert.deepEqual(toBytes(''), new Uint8Array(0));
});

const COMMANDS = [
  { name: 'new', description: 'Start a new thread in this project', kind: 'command', trek: true },
  { name: 'permissions', description: 'Show or change how much the agent asks first', kind: 'command', trek: true },
  { name: 'permissions full', description: 'No prompts and no sandbox', kind: 'command', trek: true },
  { name: 'compact', description: 'Clear the conversation but keep a summary in context', kind: 'command' },
  { name: 'frontend-design', description: 'Create distinctive interfaces', kind: 'skill' },
  { name: 'code-reviewer', description: 'Reviews code for bugs', kind: 'agent' },
];

test('the picker reads what follows a slash at the start, on one line', () => {
  assert.equal(commandQuery('/'), '');
  assert.equal(commandQuery('/comp'), 'comp');
  assert.equal(commandQuery('hello /comp'), null);
  assert.equal(commandQuery('/compact\nand more'), null);
  assert.equal(commandQuery(''), null);
});

test('the picker lists names that start with it, then names that hold it, then descriptions', () => {
  assert.equal(matchCommands(COMMANDS, '').length, COMMANDS.length);
  assert.deepEqual(
    matchCommands(COMMANDS, 'per').map((c) => c.name),
    ['permissions', 'permissions full'],
  );
  // `code` starts one name and is in another's description.
  assert.deepEqual(
    matchCommands(COMMANDS, 'code').map((c) => c.name),
    ['code-reviewer'],
  );
  assert.deepEqual(
    matchCommands(COMMANDS, 'design').map((c) => c.name),
    ['frontend-design'],
  );
  assert.deepEqual(
    matchCommands(COMMANDS, 'summary').map((c) => c.name),
    ['compact'],
  );
  // One letter doesn't search the descriptions.
  assert.deepEqual(
    matchCommands(COMMANDS, 'x').map((c) => c.name),
    [],
  );
  // Typed out with its arguments under way: only what goes further.
  assert.deepEqual(
    matchCommands(COMMANDS, 'permissions ').map((c) => c.name),
    ['permissions full'],
  );
  assert.deepEqual(matchCommands(COMMANDS, 'compact now'), []);
});

test('dictated words go after what was typed, a space between', () => {
  assert.equal(mergeDictation('', 'hello there'), 'hello there');
  assert.equal(mergeDictation('Also', 'add a test'), 'Also add a test');
  assert.equal(mergeDictation('Also ', 'add a test'), 'Also add a test');
  assert.equal(mergeDictation('Line one\n', 'line two'), 'Line one\nline two');
  assert.equal(mergeDictation('Kept as typed', ''), 'Kept as typed');
  assert.equal(mergeDictation('Kept', '   '), 'Kept');
  assert.equal(joinWords(' it is', 'can you '), 'it is can you');
  assert.equal(joinWords('', 'alone'), 'alone');
  assert.equal(joinWords('', ''), '');
});

test('the files a message ends with come off its words, in order', () => {
  const upload = '/Users/me/Library/Application Support/dev.trek.Trek/uploads/iPhone 2026-11-08 at 10.15.30 IMG_0042.MOV';
  assert.deepEqual(splitMentions(`What breaks at 0:03? @"${upload}"`), { text: 'What breaks at 0:03?', files: [upload] });
  assert.deepEqual(splitMentions('Look @/tmp/a.png @"/tmp/b c.pdf"'), { text: 'Look', files: ['/tmp/a.png', '/tmp/b c.pdf'] });
  assert.deepEqual(splitMentions('@~/notes.md'), { text: '', files: ['~/notes.md'] });
  // A Windows computer's paths.
  assert.deepEqual(splitMentions('See @"C:\\Users\\me\\My clip.mp4"'), { text: 'See', files: ['C:\\Users\\me\\My clip.mp4'] });
  // A mention among the words stays in them, and so does every one before it.
  assert.deepEqual(splitMentions('Read @/tmp/a.png then fix it'), { text: 'Read @/tmp/a.png then fix it', files: [] });
  assert.deepEqual(splitMentions('@/tmp/a.png and @/tmp/b.png'), { text: '@/tmp/a.png and', files: ['/tmp/b.png'] });
  // Not a mention: an address, a relative path.
  assert.deepEqual(splitMentions('mail me@host.com'), { text: 'mail me@host.com', files: [] });
  assert.deepEqual(splitMentions('see @src/main.rs'), { text: 'see @src/main.rs', files: [] });
  assert.deepEqual(splitMentions(''), { text: '', files: [] });
});

test('a kept file shows by its own name', () => {
  assert.equal(displayName('/Users/me/uploads/iPhone 2026-11-08 at 10.15.30 IMG_0042.MOV'), 'IMG_0042.MOV');
  assert.equal(displayName('C:\\Users\\me\\uploads\\iPhone 2026-11-08 at 10.15.30 Quarterly report.pdf'), 'Quarterly report.pdf');
  assert.equal(displayName('/Users/me/web/clip.mov'), 'clip.mov');
  assert.equal(displayName('/Users/me/iPhone notes.txt'), 'iPhone notes.txt');
  assert.equal(baseName('out/shot.png'), 'shot.png');
  assert.equal(baseName(''), '');
});

test('times and sizes, as the iPhone words them', () => {
  assert.equal(clock(0), '0:00');
  assert.equal(clock(7.9), '0:07');
  assert.equal(clock(7.9, true), '0:08');
  assert.equal(clock(65), '1:05');
  assert.equal(clock(3725), '1:02:05');
  assert.equal(sizeWords(512), '512 bytes');
  assert.equal(sizeWords(820_000), '820 KB');
  assert.equal(sizeWords(12_400_000), '12.4 MB');
  assert.equal(sizeWords(9_000_000), '9 MB');
  assert.equal(sizeWords(240_000_000), '240 MB');
  assert.equal(sizeWords(1_200_000_000), '1.2 GB');
});

test('a link is read as the computer reads it', () => {
  assert.equal(mediaPath('out/my%20shot.png'), 'out/my shot.png');
  assert.equal(mediaPath('file:///Users/me/a.png'), '/Users/me/a.png');
  assert.equal(mediaPath('docs/img.png#top'), 'docs/img.png');
  assert.equal(mediaPath('docs/img.png?raw=1'), 'docs/img.png');
  assert.equal(mediaPath('bad%zzname.png'), 'bad%zzname.png');
  assert.ok(isWeb('https://example.com/a.png'));
  assert.ok(isWeb('HTTP://example.com/a.png'));
  assert.ok(!isWeb('out/a.png'));
  assert.ok(!isWeb('file:///a.png'));
});

test('attachments sent with no words get the iPhone’s caption', () => {
  const photo = { kind: 'image' };
  const video = { kind: 'file', video: true };
  const file = { kind: 'file', video: false };
  assert.equal(captionFor([photo]), 'Here’s a photo.');
  assert.equal(captionFor([photo, photo]), 'Here are 2 photos.');
  assert.equal(captionFor([video]), 'Here’s a video.');
  assert.equal(captionFor([file, file, file]), 'Here are 3 files.');
  assert.equal(captionFor([photo, video]), 'Here are 2 attachments.');
  assert.equal(captionFor([]), '');
});

test('a message’s thumbnails: one large, a few side by side, four smaller', () => {
  assert.equal(tileSide(1), 168);
  assert.equal(tileSide(2), 76);
  assert.equal(tileSide(3), 76);
  assert.equal(tileSide(4), 66);
  assert.equal(tileSide(9), 66);
});
