// What goes with a message, and the pictures and videos a thread names, as the iPhone app has them
// (AttachmentStrip, Composer and Speech, CommandPicker, MessageImages, MediaViews):
//
// - The composer's tools. + attaches photos, made JPEGs under about a megabyte that go with the
//   message, and, from a computer that takes them (`uploads`), videos and files, sent up first a
//   piece at a time. The microphone dictates with the browser's speech recognition, its words
//   added after what's typed. `/` at the start lists the thread's commands. What's attached shows
//   over the box, each with a button to take it off.
// - In the transcript: a message's photos (`attachment`) and videos (`media`), the pictures an
//   answer shows (`media`), and the frames `watch_video` saw. A picture opens full screen.
// - `AgentMark`: an agent's logo, or an added agent's own icon (`icon`).
//
// Pictures and files from the computer are kept in memory as blob: URLs while something shows
// them; past a budget the least recently used are let go and their URLs revoked.

import { render } from '../../vendor/preact.mjs';
import { useEffect, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, Spinner, addIcons } from '../ui.js';
import * as app from '../model.js';
import * as routes from '../routes.js';

// ---------------------------------------------------------------------------------------------
// Limits, and the plain helpers (script/site/test/media.test.mjs)

/** A photo sent with a message: a JPEG under about a megabyte, its long side 2048 px at most. */
export const PHOTO_BYTES = 1_000_000;
export const PHOTO_PX = 2048;
/** Photos one message takes: four of them stay well under the 8 MiB a message may be. */
export const MAX_PHOTOS = 4;
/** `upload_part`'s pieces before base64 (`MAX_UPLOAD_PART`), and the biggest file (`MAX_UPLOAD`). */
export const UPLOAD_PIECE = 4 * 1024 * 1024;
export const MAX_UPLOAD = 2 * 1024 * 1024 * 1024;
/** A video is fetched as its card comes into view up to this size, a bigger one on a tap. */
export const AUTO_VIDEO = 24 * 1024 * 1024;
/** The sizes asked for: a message's thumbnails, an answer's pictures, a watched video's frames, full screen. */
export const THUMB_PX = 480;
export const ANSWER_PX = 1024;
export const FRAME_PX = 360;
export const FULL_PX = 2048;

const VIDEO_NAME = /\.(mp4|m4v|mov|webm)$/i;
const HEIC = /\.(heic|heif)$/i;

/** `width` × `height` made to fit `longest` on its long side (never larger), in whole pixels. */
export function fitWithin(width, height, longest) {
  const k = Math.min(1, longest / Math.max(width, height, 1));
  return { width: Math.max(1, Math.round(width * k)), height: Math.max(1, Math.round(height * k)) };
}

/** The tries at making a photo a small enough JPEG, in order: a lower quality first, then a smaller picture. */
export function photoTries(width, height) {
  const tries = [];
  for (const longest of [PHOTO_PX, 1600, 1200, 900]) {
    const size = fitWithin(width, height, longest);
    if (tries.some((t) => t.width === size.width && t.height === size.height)) continue;
    for (const quality of [0.85, 0.75, 0.65, 0.55, 0.45]) tries.push({ ...size, quality });
  }
  return tries;
}

/** Where each piece of a file starts and how long it is; an empty file is one empty piece. */
export function uploadPieces(total, size = UPLOAD_PIECE) {
  if (total <= 0) return [{ offset: 0, length: 0 }];
  const out = [];
  for (let offset = 0; offset < total; offset += size) out.push({ offset, length: Math.min(size, total - offset) });
  return out;
}

/** The text after the slash while a message is still naming a command; null otherwise. */
export function commandQuery(text) {
  return typeof text === 'string' && text.startsWith('/') && !text.includes('\n') ? text.slice(1) : null;
}

/** Names starting with the query first, then names holding it, then descriptions (from two letters). */
export function matchCommands(commands, query) {
  const q = (query || '').toLowerCase();
  if (!q) return commands;
  const name = (c) => c.name.toLowerCase();
  const prefix = commands.filter((c) => name(c).startsWith(q));
  // A command typed out, its arguments under way: nothing left to pick.
  if (q.includes(' ')) return prefix.filter((c) => name(c) !== q.trim());
  const inName = commands.filter((c) => !name(c).startsWith(q) && name(c).includes(q));
  const inText = q.length < 2 ? [] : commands.filter((c) => !name(c).includes(q) && (c.description || '').toLowerCase().includes(q));
  return [...prefix, ...inName, ...inText];
}

/** Two stretches of dictated words, one space between (either may be empty). */
export function joinWords(a, b) {
  return [a, b]
    .map((s) => (s || '').trim())
    .filter(Boolean)
    .join(' ');
}

/** What the box holds with `heard` kept: after what was typed, a space between unless it ends in one. */
export function mergeDictation(typed, heard) {
  const words = (heard || '').trim();
  if (!words) return typed;
  if (!typed) return words;
  return /\s$/.test(typed) ? typed + words : `${typed} ${words}`;
}

function isAbsolute(path) {
  return path.startsWith('/') || path.startsWith('~/') || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\');
}

/**
 * The files a message names at its end, as the computer adds them (`@/path`, or `@"/a path"` with
 * spaces): `{ text, files }`, the text without them. A mention among the words stays in them.
 */
export function splitMentions(text) {
  if (!text || !text.includes('@')) return { text: text || '', files: [] };
  const mentions = [];
  let i = 0;
  while (i < text.length) {
    const at = text.indexOf('@', i);
    if (at < 0) break;
    const wordStart = at === 0 || /\s/.test(text[at - 1]);
    let end;
    let path;
    if (wordStart && text[at + 1] === '"') {
      const close = text.indexOf('"', at + 2);
      if (close < 0) break;
      path = text.slice(at + 2, close);
      end = close + 1;
    } else {
      const space = text.slice(at + 1).search(/\s/);
      end = space < 0 ? text.length : at + 1 + space;
      path = text.slice(at + 1, end);
    }
    if (wordStart && isAbsolute(path)) mentions.push({ from: at, to: end, path });
    i = end > at ? end : at + 1;
  }
  let cut = text.length;
  const files = [];
  for (let k = mentions.length - 1; k >= 0; k--) {
    const m = mentions[k];
    if (text.slice(m.to, cut).trim() !== '') break;
    files.unshift(m.path);
    cut = m.from;
  }
  return files.length ? { text: text.slice(0, cut).trim(), files } : { text, files: [] };
}

/** A path's last part (either slash). */
export function baseName(path) {
  return String(path || '')
    .split(/[\\/]/)
    .filter(Boolean)
    .pop() || '';
}

/** The name to show for a file the computer keeps: without the `iPhone <date> at <time> ` it was kept under. */
export function displayName(path) {
  const name = baseName(path);
  const m = name.match(/^iPhone \d{4}-\d{2}-\d{2} at \d{2}\.\d{2}\.\d{2} (.+)$/);
  return m ? m[1] : name;
}

/** "0:07", "1:05", "1:02:05". Frames are read down to the second; a length rounded. */
export function clock(secs, round = false) {
  const s = Math.max(0, round ? Math.round(secs || 0) : Math.floor(secs || 0));
  const two = (n) => String(n).padStart(2, '0');
  return s >= 3600 ? `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

/** "820 KB", "12.4 MB", "1.2 GB": a file's size, as the iPhone's chips say it. */
export function sizeWords(bytes) {
  const n = Math.max(0, bytes || 0);
  if (n < 1000) return `${n} bytes`;
  if (n < 1e6) return `${Math.round(n / 1e3)} KB`;
  const [value, unit] = n < 1e9 ? [n / 1e6, 'MB'] : [n / 1e9, 'GB'];
  return `${value >= 100 ? Math.round(value) : Number(value.toFixed(1))} ${unit}`;
}

export function isWeb(link) {
  return /^https?:\/\//i.test(link || '');
}

/** A link as the computer reads it: no `file://`, no `#…` or `?…`, percent-decoded. */
export function mediaPath(link) {
  let p = String(link || '').trim();
  if (p.startsWith('file://')) p = p.slice('file://'.length);
  const cut = p.search(/[#?]/);
  if (cut >= 0) p = p.slice(0, cut);
  try {
    return decodeURIComponent(p);
  } catch {
    return p;
  }
}

/** What goes with attachments sent with no words: the agent and the transcript read it. */
export function captionFor(attachments) {
  const photos = attachments.filter((a) => a.kind === 'image').length;
  const videos = attachments.filter((a) => a.kind === 'file' && a.video).length;
  const files = attachments.filter((a) => a.kind === 'file' && !a.video).length;
  const total = photos + videos + files;
  if (!total) return '';
  if (photos === total) return total > 1 ? `Here are ${total} photos.` : 'Here’s a photo.';
  if (videos === total) return total > 1 ? `Here are ${total} videos.` : 'Here’s a video.';
  if (files === total) return total > 1 ? `Here are ${total} files.` : 'Here’s a file.';
  return `Here are ${total} attachments.`;
}

/** Bytes from base64, and back. */
export function toBytes(b64) {
  if (!b64) return new Uint8Array(0);
  if (typeof Uint8Array.fromBase64 === 'function') {
    try {
      return Uint8Array.fromBase64(b64);
    } catch {
      // Read below, as older browsers do.
    }
  }
  const bin = atob(b64.replace(/\s+/g, ''));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function toBase64(bytes) {
  if (typeof bytes.toBase64 === 'function') return bytes.toBase64();
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function uuid() {
  if (globalThis.crypto?.randomUUID) return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

// ---------------------------------------------------------------------------------------------
// Icons this area draws (Lucide's)

const SHAPES = {
  film: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M7 3v18"/><path d="M3 7.5h4"/><path d="M3 12h18"/><path d="M3 16.5h4"/><path d="M17 3v18"/><path d="M17 7.5h4"/><path d="M17 16.5h4"/>',
  users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  cpu: '<rect width="16" height="16" x="4" y="4" rx="2"/><rect width="6" height="6" x="9" y="9" rx="1"/><path d="M15 2v2"/><path d="M15 20v2"/><path d="M2 15h2"/><path d="M2 9h2"/><path d="M20 15h2"/><path d="M20 9h2"/><path d="M9 2v2"/><path d="M9 20v2"/>',
  'slash-square': '<rect width="18" height="18" x="3" y="3" rx="2"/><line x1="9" x2="15" y1="15" y2="9"/>',
  'image-off': '<line x1="2" x2="22" y1="2" y2="22"/><path d="M10.41 10.41a2 2 0 1 1-2.83-2.83"/><line x1="13.5" x2="6" y1="13.5" y2="21"/><line x1="18" x2="21" y1="12" y2="15"/><path d="M3.59 3.59A1.99 1.99 0 0 0 3 5v14a2 2 0 0 0 2 2h14c.55 0 1.052-.22 1.41-.59"/><path d="M21 15V5a2 2 0 0 0-2-2H9"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>',
  'play-fill': '<polygon points="6 3 20 12 6 21 6 3" fill="currentColor"/>',
};
addIcons(SHAPES);

/** The same shapes for the pictures an answer draws, which live in the markdown's own DOM. */
const DOM_SHAPES = {
  image: '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
  external: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  'image-off': SHAPES['image-off'],
};

function svgIcon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'icon s');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.innerHTML = DOM_SHAPES[name] || '';
  return svg;
}

// ---------------------------------------------------------------------------------------------
// Pictures and files from the computer, kept while they show

/** By key (`attachment:…@480`, `media:…@1024`, `file:…`): `{ url, width, height, cost, kind, count, nodes, used, promise }`. */
const kept = new Map();
/** Pictures as drawn, four bytes a pixel (the iPhone's budget); videos by their bytes. */
const PICTURE_BUDGET = 64 * 1024 * 1024;
const FILE_BUDGET = 96 * 1024 * 1024;

function entryFor(key, load) {
  const had = kept.get(key);
  if (had) {
    had.used = Date.now();
    return had;
  }
  if (!load) return null;
  const e = { key, count: 0, nodes: [], used: Date.now(), url: null, width: 0, height: 0, cost: 0, kind: 'picture' };
  e.promise = load().then(
    (got) => {
      e.url = URL.createObjectURL(got.blob);
      e.width = got.width || 0;
      e.height = got.height || 0;
      e.kind = got.kind || 'picture';
      e.mime = got.blob.type;
      e.size = got.blob.size;
      e.cost = e.kind === 'file' ? got.blob.size : e.width && e.height ? e.width * e.height * 4 : got.blob.size * 8;
      trim();
      return e;
    },
    (err) => {
      if (kept.get(key) === e) kept.delete(key);
      throw err;
    },
  );
  // Nobody may be waiting on it yet: a failure is theirs to hear.
  e.promise.catch(() => {});
  kept.set(key, e);
  return e;
}

/** Keep `key` while it shows (`release` the entry after); `load` fetches it when it isn't here. */
function hold(key, load) {
  const e = entryFor(key, load);
  if (e) e.count += 1;
  return e;
}

function release(e) {
  if (!e) return;
  e.count = Math.max(0, e.count - 1);
  e.used = Date.now();
  trim();
}

/** Kept while `node` (a picture in an answer's markdown) is on the page. */
function holdFor(node, key, load) {
  const e = entryFor(key, load);
  e.nodes.push(new WeakRef(node));
  return e;
}

function showing(e) {
  if (e.count > 0) return true;
  e.nodes = e.nodes.filter((ref) => ref.deref()?.isConnected);
  return e.nodes.length > 0;
}

/** Past the budget: the least recently used that nothing shows go, their URLs revoked. */
function trim() {
  for (const [kind, budget] of [
    ['picture', PICTURE_BUDGET],
    ['file', FILE_BUDGET],
  ]) {
    const ready = [...kept.values()].filter((e) => e.url && e.kind === kind);
    let total = ready.reduce((n, e) => n + e.cost, 0);
    if (total <= budget) continue;
    for (const e of ready.sort((a, b) => a.used - b.used)) {
      if (total <= budget) break;
      if (showing(e)) continue;
      URL.revokeObjectURL(e.url);
      kept.delete(e.key);
      total -= e.cost;
    }
  }
}

/** The computer's reads, a couple at a time: a long thread of pictures doesn't crowd out the rest. */
let running = 0;
const queue = [];
function queued(task) {
  return new Promise((resolve, reject) => {
    const go = () => {
      running += 1;
      task()
        .then(resolve, reject)
        .finally(() => {
          running -= 1;
          queue.shift()?.();
        });
    };
    if (running < 2) go();
    else queue.push(go);
  });
}

async function askFor(msg, timeout = 60_000) {
  try {
    return await app.ask(msg, { timeout });
  } catch (err) {
    if (err.code !== 'rate_limited') throw err;
    await new Promise((r) => setTimeout(r, 800));
    return app.ask(msg, { timeout });
  }
}

const attachmentKey = (threadId, itemId, index, px) => `attachment:${threadId}/${itemId}/${index}@${px}`;
const mediaKey = (threadId, path, px) => `media:${threadId}/${path}@${px}`;
const fileKey = (threadId, path) => `file:${threadId}/${path}`;

/** Image `index` of a message, fitting `px` a side. */
function loadAttachment(threadId, itemId, index, px) {
  return () =>
    queued(async () => {
      const r = await askFor({ type: 'attachment', thread_id: threadId, item_id: itemId, index, max_px: px });
      return { blob: new Blob([toBytes(r.data)], { type: r.mime || 'image/jpeg' }), width: r.width, height: r.height };
    });
}

const SHOWABLE = /^image\/(jpeg|png|gif|webp|avif|bmp|svg\+xml)$/;

/** A picture the thread names, fitting `px` a side: whole in one reply, or (one the computer can't scale) in pieces. */
function loadMediaPicture(threadId, path, px) {
  return () =>
    queued(async () => {
      const first = await askFor({ type: 'media', thread_id: threadId, path, max_px: px });
      const parts = [toBytes(first.bytes_b64)];
      let got = (first.offset || 0) + parts[0].length;
      const total = first.total || got;
      if (got < total) {
        if (!SHOWABLE.test(first.mime || '') || total > 16 * 1024 * 1024) throw new Error('This browser can’t show that picture');
        while (got < total) {
          const r = await askFor({ type: 'media', thread_id: threadId, path, offset: got });
          const piece = toBytes(r.bytes_b64);
          if (!piece.length) break;
          parts.push(piece);
          got += piece.length;
        }
      }
      const blob = new Blob(parts, { type: first.mime || 'image/jpeg' });
      const size = first.width && first.height ? { width: first.width, height: first.height } : await measure(blob);
      return { blob, ...size };
    });
}

async function measure(blob) {
  try {
    const bitmap = await createImageBitmap(blob);
    const size = { width: bitmap.width, height: bitmap.height };
    bitmap.close();
    return size;
  } catch {
    return { width: 0, height: 0 };
  }
}

/** A video is bigger than what's fetched by itself: a tap fetches it. */
class TooBig extends Error {
  constructor(total) {
    super('Too big to fetch by itself');
    this.total = total;
  }
}

/** Files part-way here, by key: a bigger one's first piece stays, so a tap carries on from it. */
const partial = new Map();

/** A file the thread names, as it is, a piece at a time (up to `upTo` bytes, else `TooBig`). */
function loadFile(threadId, path, upTo, onProgress) {
  return async () => {
    const key = fileKey(threadId, path);
    const d = partial.get(key) || { parts: [], got: 0, total: null, mime: '' };
    partial.set(key, d);
    while (d.total == null || d.got < d.total) {
      if (d.total != null && d.total > upTo) throw new TooBig(d.total);
      const r = await queued(() => askFor({ type: 'media', thread_id: threadId, path, offset: d.got }));
      const piece = toBytes(r.bytes_b64);
      d.total = r.total ?? 0;
      d.mime = r.mime || d.mime;
      if (!piece.length) {
        if (d.got < d.total) throw new Error('It came back shorter than it is');
        break;
      }
      d.parts.push(piece);
      d.got += piece.length;
      onProgress?.(d.total ? d.got / d.total : 1, d.total);
    }
    partial.delete(key);
    // A QuickTime movie plays as the MP4 it nearly always is inside; the browser checks what it can.
    const type = d.mime === 'video/quicktime' ? 'video/mp4' : d.mime;
    return { blob: new Blob(d.parts, { type }), kind: 'file' };
  };
}

/** One per root that scrolls (the transcript's), so pictures come a little before they show; gone with it. */
const seers = new WeakMap();
let pageSeer = null;
const onSeen = new WeakMap();
function whenSeen(el, fn) {
  if (typeof IntersectionObserver === 'undefined') {
    fn();
    return () => {};
  }
  const root = el.closest('.scroll');
  let seer = root ? seers.get(root) : pageSeer;
  if (!seer) {
    seer = new IntersectionObserver(
      (entries) => {
        for (const x of entries) {
          if (!x.isIntersecting) continue;
          const f = onSeen.get(x.target);
          onSeen.delete(x.target);
          seer.unobserve(x.target);
          f?.();
        }
      },
      { root, rootMargin: '400px 0px' },
    );
    if (root) seers.set(root, seer);
    else pageSeer = seer;
  }
  onSeen.set(el, fn);
  seer.observe(el);
  return () => {
    onSeen.delete(el);
    seer.unobserve(el);
  };
}

function useSeen(ref) {
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    if (seen || !ref.current) return undefined;
    return whenSeen(ref.current, () => setSeen(true));
  }, [seen]);
  return seen;
}

/** A picture from the computer while this shows it: `{ url, width, height, failed, retry }`. */
function usePicture(key, load, ready = true) {
  const [entry, setEntry] = useState(() => {
    const e = key && kept.get(key);
    return e?.url ? e : null;
  });
  const [failed, setFailed] = useState(false);
  const [tries, setTries] = useState(0);
  useEffect(() => {
    if (!key || !ready) return undefined;
    const e = hold(key, load);
    if (!e) return undefined;
    let live = true;
    setFailed(false);
    e.promise.then(
      (got) => live && setEntry(got),
      () => live && setFailed(true),
    );
    return () => {
      live = false;
      release(e);
    };
  }, [key, ready, tries]);
  const mine = entry && entry.key === key && entry.url ? entry : null;
  return { url: mine?.url || null, width: mine?.width, height: mine?.height, failed, retry: () => setTries((n) => n + 1) };
}

// What the viewer shows: `{ label, name, thumbPx, key(px), load(px) }`.

function attachmentSource(threadId, itemId, index) {
  return {
    label: `Image ${index + 1}`,
    name: `image-${index + 1}.jpg`,
    thumbPx: THUMB_PX,
    key: (px) => attachmentKey(threadId, itemId, index, px),
    load: (px) => loadAttachment(threadId, itemId, index, px),
  };
}

function mediaSource(threadId, path, thumbPx, label) {
  return {
    label: label || baseName(path) || 'Picture',
    name: baseName(path) || 'picture.jpg',
    thumbPx,
    key: (px) => mediaKey(threadId, path, px),
    load: (px) => loadMediaPicture(threadId, path, px),
  };
}

/** A photo picked for a message not sent yet: it's here already. */
function pickedSource(photo) {
  return {
    label: photo.name || 'Photo',
    name: photo.name || 'photo.jpg',
    thumbPx: 0,
    key: () => `picked:${photo.id}`,
    load: () => async () => ({ blob: new Blob([toBytes(photo.data)], { type: photo.mime }), width: photo.width, height: photo.height }),
  };
}

// ---------------------------------------------------------------------------------------------
// The composer: + (photos, videos and files)

const HEIC_WORDS = 'This browser can’t open HEIC photos. Choose a JPEG or PNG instead.';

function readBase64(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result || '');
      resolve(url.slice(url.indexOf(',') + 1));
    };
    reader.onerror = () => reject(reader.error || new Error('Couldn’t read it'));
    reader.readAsDataURL(blob);
  });
}

/** A picture file, drawn: `{ el, width, height, done }` (`done` lets its URL go). */
async function decodeImage(file) {
  const url = URL.createObjectURL(file);
  const el = new Image();
  el.decoding = 'async';
  el.src = url;
  try {
    await el.decode();
  } catch {
    URL.revokeObjectURL(url);
    throw new Error(HEIC.test(file.name || '') || /hei[cf]/i.test(file.type) ? HEIC_WORDS : `Couldn’t open ${file.name || 'that photo'} as a picture`);
  }
  return { el, width: el.naturalWidth, height: el.naturalHeight, done: () => URL.revokeObjectURL(url) };
}

function drawn(el, width, height) {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const g = canvas.getContext('2d');
  // Transparency on white, as the computer sends a picture.
  g.fillStyle = '#fff';
  g.fillRect(0, 0, width, height);
  g.imageSmoothingQuality = 'high';
  g.drawImage(el, 0, 0, width, height);
  return canvas;
}

function jpegOf(canvas, quality) {
  return new Promise((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('Couldn’t make that photo a JPEG'))), 'image/jpeg', quality));
}

/** A photo as it goes with a message: a JPEG under `PHOTO_BYTES`, its long side `PHOTO_PX` at most. */
async function makePhoto(file) {
  const image = await decodeImage(file);
  try {
    let canvas = null;
    for (const t of photoTries(image.width, image.height)) {
      if (!canvas || canvas.width !== t.width || canvas.height !== t.height) canvas = drawn(image.el, t.width, t.height);
      const blob = await jpegOf(canvas, t.quality);
      if (blob.size > PHOTO_BYTES) continue;
      const small = fitWithin(image.width, image.height, 160);
      const thumb = drawn(image.el, small.width, small.height).toDataURL('image/jpeg', 0.8);
      return { kind: 'image', id: uuid(), mime: 'image/jpeg', data: await readBase64(blob), name: file.name || 'Photo', width: t.width, height: t.height, bytes: blob.size, thumb };
    }
    throw new Error(`${file.name || 'That photo'} is too big to send, even made smaller`);
  } finally {
    image.done();
  }
}

/** A video's first frame and length, from the file itself or a URL to it (none when it won't play here). */
function lookAt(url) {
  return new Promise((resolve) => {
    const v = document.createElement('video');
    let settled = false;
    const done = (look) => {
      if (settled) return;
      settled = true;
      v.removeAttribute('src');
      v.load();
      resolve(look);
    };
    v.muted = true;
    v.playsInline = true;
    v.preload = 'auto';
    v.addEventListener('loadeddata', () => {
      v.currentTime = Math.min(0.1, (v.duration || 0.2) / 2);
    });
    v.addEventListener('seeked', () => {
      try {
        const size = fitWithin(v.videoWidth || 16, v.videoHeight || 9, 640);
        const canvas = document.createElement('canvas');
        canvas.width = size.width;
        canvas.height = size.height;
        canvas.getContext('2d').drawImage(v, 0, 0, size.width, size.height);
        done({ poster: canvas.toDataURL('image/jpeg', 0.8), duration: Number.isFinite(v.duration) ? v.duration : null, aspect: v.videoWidth && v.videoHeight ? v.videoWidth / v.videoHeight : 16 / 9 });
      } catch {
        done({ poster: null, duration: Number.isFinite(v.duration) ? v.duration : null, aspect: 16 / 9 });
      }
    });
    v.addEventListener('error', () => done({ poster: null, duration: null, aspect: 16 / 9, unplayable: true }));
    setTimeout(() => done({ poster: null, duration: null, aspect: 16 / 9 }), 8000);
    v.src = url;
  });
}

async function lookAtFile(file) {
  const url = URL.createObjectURL(file);
  try {
    return await lookAt(url);
  } finally {
    URL.revokeObjectURL(url);
  }
}

// Files go up one after another (the computer takes four part-way at most), each a piece at a time,
// each piece waiting for the one before it to be taken.
const sendQueue = [];
let sendingFile = false;
const stopped = new Set();

function stopUpload(id) {
  stopped.add(id);
}

async function pumpUploads() {
  if (sendingFile) return;
  sendingFile = true;
  while (sendQueue.length) {
    const job = sendQueue.shift();
    if (stopped.has(job.id)) continue;
    try {
      await sendFile(job);
    } catch (err) {
      if (!stopped.has(job.id)) {
        app.toast(err.message || `Couldn’t send ${job.file.name}`, 'negative');
        job.drop();
      }
    }
  }
  sendingFile = false;
}

async function sendFile({ id, file, change }) {
  const total = file.size;
  const name = file.name || 'file';
  change({ waiting: false, progress: 0 });
  let offset = 0;
  for (;;) {
    if (stopped.has(id)) return;
    const length = Math.min(UPLOAD_PIECE, total - offset);
    const data = length > 0 ? await readBase64(file.slice(offset, offset + length)) : '';
    if (stopped.has(id)) return;
    const reply = await askFor({ type: 'upload_part', upload_id: id, name, total, offset, data }, 120_000);
    if (reply.type !== 'upload_progress') throw new Error(`Your computer answered something else while ${name} was going up`);
    change({ progress: total ? Math.min(1, reply.received / total) : 1 });
    if (reply.done) {
      change({ pending: false, done: true, progress: 1 });
      return;
    }
    if (!(reply.received > offset)) throw new Error(`Your computer didn’t take the rest of ${name}`);
    offset = reply.received;
  }
}

/** Send `file` up, showing how far it's got in its tile; once in, its id goes with the message. */
function addUpload(file, setAttachments) {
  if (file.size > MAX_UPLOAD) {
    app.toast(`${file.name} is ${sizeWords(file.size)}: the most your computer takes is 2 GB`, 'negative');
    return;
  }
  const id = uuid();
  const video = file.type.startsWith('video/') || VIDEO_NAME.test(file.name || '');
  const change = (fields) => setAttachments((list) => list.map((a) => (a.id === id ? { ...a, ...fields } : a)));
  const drop = () => setAttachments((list) => list.filter((a) => a.id !== id));
  setAttachments((list) => [...list, { kind: 'file', id, name: file.name || 'file', size: file.size, video, progress: 0, pending: true, waiting: true }]);
  if (video) lookAtFile(file).then((look) => change({ poster: look.poster, duration: look.duration }));
  sendQueue.push({ id, file, change, drop });
  pumpUploads();
}

/** Photos made ready one at a time, each a spinner in its place (in the order picked) until it is. */
async function addPhotos(files, attachments, setAttachments) {
  const room = MAX_PHOTOS - attachments.filter((a) => a.kind === 'image' || a.kind === 'preparing').length;
  if (room <= 0 || files.length > room) app.toast(`A message takes ${MAX_PHOTOS} photos at most`, 'negative');
  const taken = files.slice(0, Math.max(0, room)).map((file) => ({ file, id: uuid() }));
  if (!taken.length) return;
  setAttachments((list) => [...list, ...taken.map(({ file, id }) => ({ kind: 'preparing', id, name: file.name, pending: true }))]);
  for (const { file, id } of taken) {
    try {
      const photo = await makePhoto(file);
      setAttachments((list) => list.map((a) => (a.id === id ? { ...photo, id } : a)));
    } catch (err) {
      setAttachments((list) => list.filter((a) => a.id !== id));
      app.toast(err.message, 'negative');
    }
  }
}

function isPicture(file) {
  return file.type.startsWith('image/') || HEIC.test(file.name || '');
}

/** Where the dictation says it's listening: + steps aside, as on the iPhone. */
let listening = false;
const listeningWatchers = new Set();
function setListening(on) {
  listening = on;
  for (const watcher of listeningWatchers) watcher(on);
}
function useListening() {
  const [on, setOn] = useState(listening);
  useEffect(() => {
    listeningWatchers.add(setOn);
    return () => listeningWatchers.delete(setOn);
  }, []);
  return on;
}

/** Closes when a tap lands anywhere outside `ref`. */
function useOutside(ref, open, close) {
  useEffect(() => {
    if (!open) return undefined;
    const away = (e) => {
      if (ref.current && !ref.current.contains(e.target)) close();
    };
    const key = (e) => e.key === 'Escape' && close();
    document.addEventListener('pointerdown', away, true);
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('pointerdown', away, true);
      document.removeEventListener('keydown', key);
    };
  }, [open]);
}

/** +: photos, and from a computer that takes them, videos and files. */
function AttachTool({ attachments, setAttachments, disabled }) {
  const dictating = useListening();
  const [menu, setMenu] = useState(false);
  const box = useRef(null);
  const photos = useRef(null);
  const media = useRef(null);
  const files = useRef(null);
  useOutside(box, menu, () => setMenu(false));
  if (dictating) return null;
  const uploads = app.has('uploads');

  const take = (e, how) => {
    const picked = [...(e.target.files || [])];
    e.target.value = '';
    if (!picked.length) return;
    if (how === 'files') {
      for (const f of picked) addUpload(f, setAttachments);
      return;
    }
    const pictures = picked.filter(isPicture);
    if (pictures.length) addPhotos(pictures, attachments, setAttachments);
    for (const f of picked.filter((f) => !isPicture(f))) {
      if (uploads) addUpload(f, setAttachments);
      else app.toast(`${f.name} isn’t a photo: your computer takes videos and files from a newer Trek`, 'negative');
    }
  };
  const pick = (input) => {
    setMenu(false);
    input.current?.click();
  };

  return html`<span class="media-attach" ref=${box}>
    <button
      type="button"
      class="media-btn plain"
      aria-label=${uploads ? 'Attach' : 'Add photos'}
      title=${uploads ? 'Photos, videos or files' : 'Add photos'}
      aria-haspopup=${uploads ? 'menu' : undefined}
      aria-expanded=${uploads ? (menu ? 'true' : 'false') : undefined}
      disabled=${disabled}
      onClick=${() => (uploads ? setMenu(!menu) : photos.current?.click())}
    ><${Icon} name="plus" /></button>
    <input ref=${photos} type="file" accept="image/*" multiple hidden onChange=${(e) => take(e, 'photos')} />
    <input ref=${media} type="file" accept="image/*,video/*" multiple hidden onChange=${(e) => take(e, 'media')} />
    <input ref=${files} type="file" multiple hidden onChange=${(e) => take(e, 'files')} />
    ${menu &&
    html`<div class="media-pop media-menu" role="menu" aria-label="Attach">
      <button type="button" role="menuitem" class="media-menu-item" onClick=${() => pick(media)}><${Icon} name="image" />Photos and videos</button>
      <button type="button" role="menuitem" class="media-menu-item" onClick=${() => pick(files)}><${Icon} name="folder" />Files</button>
    </div>`}
  </span>`;
}

/** A ring filling clockwise: how much of a file has gone up. */
function Ring({ value = 0, size = 26, light = false }) {
  const r = (size - 4) / 2;
  const c = 2 * Math.PI * r;
  const shown = Math.max(0.03, Math.min(1, value));
  const mid = size / 2;
  return html`<svg class=${`media-ring ${light ? 'light' : ''}`} width=${size} height=${size} viewBox=${`0 0 ${size} ${size}`} role="progressbar" aria-label="Sending" aria-valuemin="0" aria-valuemax="100" aria-valuenow=${Math.round(value * 100)}>
    <circle class="track" cx=${mid} cy=${mid} r=${r} />
    <circle class="fill" cx=${mid} cy=${mid} r=${r} stroke-dasharray=${c} stroke-dashoffset=${c * (1 - shown)} transform=${`rotate(-90 ${mid} ${mid})`} />
  </svg>`;
}

function VideoTile({ file }) {
  const value = file.pending ? (file.waiting ? 'Waiting to send' : `Sending, ${Math.round((file.progress || 0) * 100)} percent`) : [file.duration ? clock(file.duration, true) : null, sizeWords(file.size)].filter(Boolean).join(', ');
  return html`<div class="media-tile video" role="img" aria-label=${`Attached video, ${value}`}>
    ${file.poster && html`<img src=${file.poster} alt="" draggable="false" />`}
    ${file.pending
      ? html`<span class="media-veil"><${Ring} value=${file.progress} light=${true} /></span>`
      : html`<span class="media-length"><${Icon} name="play-fill" size="xs" />${file.duration ? clock(file.duration, true) : ''}</span>`}
  </div>`;
}

function FileTile({ file }) {
  const words = file.pending ? (file.waiting ? 'Waiting…' : `${Math.round((file.progress || 0) * 100)}% sent`) : sizeWords(file.size);
  return html`<div class="media-tile file" role="img" aria-label=${`Attached file ${file.name}, ${words}`}>
    <span class="media-file-icon">${file.pending ? html`<${Ring} value=${file.progress} size=${24} />` : html`<${Icon} name="file" />`}</span>
    <span class="media-file-words"><span class="media-file-name">${file.name}</span><span class="media-file-size">${words}</span></span>
  </div>`;
}

const ATTACHED = new Set(['image', 'preparing', 'file']);

/** What's attached, over the box: photos (a tap opens them full screen), videos and files, each with ✕. */
function AttachmentStrip({ attachments, setAttachments }) {
  const latest = useRef(attachments);
  latest.current = attachments;
  // The composer going (another thread, another screen) takes its files with it.
  useEffect(
    () => () => {
      for (const a of latest.current) if (a.kind === 'file' && a.pending) stopUpload(a.id);
    },
    [],
  );
  const shown = attachments.filter((a) => ATTACHED.has(a.kind));
  if (!shown.length) return null;
  const photos = attachments.filter((a) => a.kind === 'image');
  const remove = (a) => {
    if (a.kind === 'file' && a.pending) stopUpload(a.id);
    setAttachments((list) => list.filter((x) => x.id !== a.id));
  };
  return html`<div class="media-strip" role="list" aria-label="Attached">
    ${shown.map(
      (a) => html`<div class="media-tile-box" role="listitem" key=${a.id}>
        ${a.kind === 'image'
          ? html`<button type="button" class="media-tile photo" aria-label="Attached photo" title="Opens it full screen" onClick=${() => openViewer(photos.map(pickedSource), photos.indexOf(a))}><img src=${a.thumb} alt="" draggable="false" /></button>`
          : a.kind === 'preparing'
            ? html`<div class="media-tile photo" role="img" aria-label="Getting a photo ready"><${Spinner} /></div>`
            : a.video
              ? html`<${VideoTile} file=${a} />`
              : html`<${FileTile} file=${a} />`}
        <button type="button" class="media-remove" aria-label=${a.kind === 'file' ? (a.video ? 'Remove video' : `Remove ${a.name}`) : 'Remove photo'} onClick=${() => remove(a)}><${Icon} name="x" size="xs" /></button>
      </div>`,
    )}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// The composer: dictation

const ANDROID = typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent || '');

function speechRecognition() {
  return typeof window === 'undefined' ? null : window.SpeechRecognition || window.webkitSpeechRecognition || null;
}

const SPEECH_WORDS = {
  'not-allowed': 'Trek needs the microphone to hear you. Allow it for this site in the browser’s settings.',
  'service-not-allowed': 'This browser won’t let the page use speech recognition.',
  'audio-capture': 'The microphone isn’t available right now. Another app may be using it.',
  network: 'Speech recognition needs the internet in this browser. Try again once you’re online.',
  'language-not-supported': 'Speech recognition isn’t available for your language in this browser.',
  stuck: 'Speech recognition stopped working. Tap the microphone to try again.',
};

/**
 * The microphone: what's said goes into the box live, after what was typed, until the button's
 * tapped again (the words stay) or ✕ (they go). Rounds the recognizer closes by itself (a pause,
 * Android's one phrase at a time) open the next, so it listens on, as the iPhone's does.
 */
function DictateTool({ text, setText }) {
  const [on, setOn] = useState(false);
  const run = useRef(null);
  const props = useRef({ text, setText });
  props.current = { text, setText };

  const off = () => {
    setOn(false);
    setListening(false);
  };
  /** `keep` the words heard, `drop` them (back to what was typed), or stop where it is (`typed`, `leave`). */
  const end = (r, how, why) => {
    if (!r) return;
    r.wanted = false;
    if (how === 'keep') {
      // What it heard last still comes: the box follows it until the round ends.
      try {
        r.rec?.stop();
      } catch {
        // Ended already.
      }
    } else {
      if (run.current === r) run.current = null;
      try {
        r.rec?.abort();
      } catch {
        // Ended already.
      }
      if (how === 'drop') props.current.setText(r.base);
    }
    if (how !== 'leave') off();
    if (why) app.toast(SPEECH_WORDS[why] || 'Speech recognition isn’t available right now. Try again in a moment.', 'negative');
  };
  const show = (r) => {
    r.shown = mergeDictation(r.base, joinWords(r.settled, r.round));
    props.current.setText(r.shown);
  };
  const openRound = (r) => {
    const Recognition = speechRecognition();
    const rec = new Recognition();
    rec.lang = navigator.language || 'en-US';
    // Android's recognizer repeats earlier phrases in one long session: a phrase a round there.
    rec.continuous = !ANDROID;
    rec.interimResults = true;
    rec.maxAlternatives = 1;
    r.rec = rec;
    r.round = '';
    r.error = null;
    rec.onresult = (e) => {
      if (run.current !== r || r.rec !== rec) return;
      let finals = '';
      let live = '';
      for (let i = 0; i < e.results.length; i++) {
        const said = e.results[i][0]?.transcript || '';
        if (e.results[i].isFinal) finals = joinWords(finals, said);
        else live = joinWords(live, said);
      }
      r.round = joinWords(finals, live);
      show(r);
    };
    rec.onerror = (e) => {
      if (r.rec !== rec || e.error === 'no-speech' || e.error === 'aborted') return;
      r.error = e.error;
    };
    rec.onend = () => {
      if (r.rec !== rec) return;
      r.settled = joinWords(r.settled, r.round);
      r.round = '';
      if (run.current !== r) return;
      if (!r.wanted) {
        run.current = null;
        return;
      }
      if (r.error) return end(r, 'typed', r.error);
      // A round that closes the moment it opens, again and again, is broken: it stops.
      const now = Date.now();
      r.restarts = now - r.last < 300 ? r.restarts + 1 : 0;
      r.last = now;
      if (r.restarts > 10) return end(r, 'typed', 'stuck');
      openRound(r);
    };
    try {
      rec.start();
    } catch {
      end(r, 'typed', 'start');
    }
  };
  const start = () => {
    if (run.current) end(run.current, 'typed');
    const r = { base: props.current.text, settled: '', round: '', shown: props.current.text, wanted: true, restarts: 0, last: 0, rec: null, error: null };
    run.current = r;
    setOn(true);
    setListening(true);
    openRound(r);
  };

  // Typed into while it listens, or the message went: the box stays as it is, and it stops.
  useEffect(() => {
    const r = run.current;
    if (r && text !== r.shown) end(r, 'typed');
  }, [text]);
  // The page going to the background keeps the words; so does leaving the thread.
  useEffect(() => {
    const away = () => document.visibilityState === 'hidden' && run.current && end(run.current, 'keep');
    document.addEventListener('visibilitychange', away);
    return () => {
      document.removeEventListener('visibilitychange', away);
      if (run.current) end(run.current, 'leave');
      setListening(false);
    };
  }, []);

  if (!speechRecognition()) return null;
  if (!on) {
    return html`<button type="button" class="media-btn media-mic" aria-label="Speak your message" title="Speak your message" onClick=${start}><${Icon} name="mic" size="s" /></button>`;
  }
  return html`<button type="button" class="media-btn media-discard" aria-label="Discard dictation" title="Discard dictation" onClick=${() => end(run.current, 'drop')}><${Icon} name="x" size="s" /></button>
    <button type="button" class="media-btn media-mic on" aria-pressed="true" aria-label="Keep dictation" title="Stop, and keep what you said" onClick=${() => end(run.current, 'keep')}><${Icon} name="mic" size="s" /></button>
    ${!text && html`<span class="media-listening" aria-hidden="true">Listening…</span>`}
    <span class="sr" role="status">Listening</span>`;
}

// ---------------------------------------------------------------------------------------------
// The composer: the `/` picker

/** The thread's commands as last asked, by thread. */
const commandLists = new Map();
const commandAsks = new Map();

function loadCommands(threadId) {
  if (commandAsks.has(threadId)) return commandAsks.get(threadId);
  const asked = askFor({ type: 'commands', thread_id: threadId })
    .then((r) => r.commands || [])
    // A computer too old for it has none to list.
    .catch(() => commandLists.get(threadId) || [])
    .then((list) => {
      commandLists.set(threadId, list);
      commandAsks.delete(threadId);
      return list;
    });
  commandAsks.set(threadId, asked);
  return asked;
}

function commandIcon(c) {
  if (c.kind === 'skill') return 'sparkle';
  if (c.kind === 'agent') return 'users';
  return c.trek ? 'mountain' : 'slash-square';
}

/** `/` at the start of the message: the thread's commands over the box, narrowed as you type; a tap fills one in. */
function SlashTool({ threadId, text, setText }) {
  const query = threadId ? commandQuery(text) : null;
  const [list, setList] = useState(() => commandLists.get(threadId) || null);
  const [dismissed, setDismissed] = useState(null);
  const box = useRef(null);
  const naming = query != null;
  const fresh = text === '/';
  useEffect(() => {
    if (!threadId || !naming) return undefined;
    let live = true;
    // Asked again each time `/` is typed: the agent's may have changed.
    if (fresh || !commandLists.has(threadId)) loadCommands(threadId).then((l) => live && setList(l));
    else setList(commandLists.get(threadId));
    return () => {
      live = false;
    };
  }, [threadId, naming, fresh]);
  // Put away by a tap outside the composer (not the box being typed in) or Escape, until the text changes.
  const open = naming && dismissed !== text;
  useEffect(() => {
    if (!open) return undefined;
    const away = (e) => {
      const composer = box.current?.closest('.composer-box, .new-compose');
      if (composer && !composer.contains(e.target)) setDismissed(text);
    };
    const key = (e) => e.key === 'Escape' && setDismissed(text);
    document.addEventListener('pointerdown', away, true);
    document.addEventListener('keydown', key);
    return () => {
      document.removeEventListener('pointerdown', away, true);
      document.removeEventListener('keydown', key);
    };
  }, [open, text]);
  if (!open || !list) return null;
  const shown = matchCommands(list, query);
  if (!shown.length) return null;
  const pick = (e, c) => {
    setText(`/${c.name} `);
    e.currentTarget.closest('.composer-box, .new-compose')?.querySelector('textarea')?.focus();
  };
  return html`<div class="media-pop media-commands" role="listbox" aria-label="Commands" ref=${box} onPointerDown=${(e) => e.preventDefault()}>
    ${shown.map(
      (c) => html`<button type="button" role="option" aria-selected="false" class="media-command" key=${c.name} onClick=${(e) => pick(e, c)}>
        <span class=${`media-command-icon ${c.trek ? 'trek' : ''}`}><${Icon} name=${commandIcon(c)} size="s" /></span>
        <span class="media-command-words">
          <span class="media-command-name"><span class="mono">/${c.name}</span>${c.kind && c.kind !== 'command' && html`<span class="media-command-kind">${c.kind === 'skill' ? 'skill' : 'agent'}</span>`}</span>
          ${c.description && html`<span class="media-command-desc">${c.description}</span>`}
        </span>
      </button>`,
    )}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// In the transcript: a message's photos, videos and files

/** A tile's side: one large, a few side by side, and four that still fit (the iPhone's). */
export function tileSide(count) {
  return count === 1 ? 168 : count <= 3 ? 76 : 66;
}

function Thumb({ source, px, ready }) {
  const pic = usePicture(source.key(px), source.load(px), ready);
  if (pic.url) return html`<img src=${pic.url} alt="" draggable="false" />`;
  if (pic.failed) return html`<${Icon} name="image-off" />`;
  return ready ? html`<${Spinner} />` : null;
}

/** A message's images, as thumbnails (up to four, the fourth saying how many more); a tap opens them full screen. */
export function MessageImages({ threadId, itemId, count }) {
  const box = useRef(null);
  const seen = useSeen(box);
  const shown = Math.min(count, 4);
  const side = `${tileSide(count)}px`;
  const sources = Array.from({ length: count }, (_, i) => attachmentSource(threadId, itemId, i));
  return html`<div class="msg-photos" ref=${box}>
    ${sources.slice(0, shown).map(
      (s, i) => html`<button type="button" class="msg-photo" key=${i} style=${{ width: side, height: side }} aria-label=${`Image ${i + 1} of ${count}`} title="Opens it full screen" onClick=${() => openViewer(sources, i)}>
        <${Thumb} source=${s} px=${THUMB_PX} ready=${seen} />
        ${i === shown - 1 && count > shown && html`<span class="msg-photo-more">+${count - shown + 1}</span>`}
      </button>`,
    )}
  </div>`;
}

/** A video the thread names: its first frame with a play badge, playing here on a tap (`media`). */
export function VideoCard({ threadId, path, name }) {
  const face = useRef(null);
  const seen = useSeen(face);
  const serves = app.has('media');
  const key = fileKey(threadId, path);
  const [phase, setPhase] = useState('idle');
  const [progress, setProgress] = useState(0);
  const [total, setTotal] = useState(null);
  const [look, setLook] = useState(null);
  const [url, setUrl] = useState(null);
  const [playing, setPlaying] = useState(false);
  const held = useRef(null);
  const playWhenHere = useRef(false);
  const label = name || displayName(path);

  const download = (upTo) => {
    setPhase('loading');
    const e = hold(key, loadFile(threadId, path, upTo, (p, t) => {
      setProgress(p);
      setTotal(t);
    }));
    held.current = e;
    e.promise.then(
      async (got) => {
        setUrl(got.url);
        setTotal(got.size);
        if (!got.look) got.look = lookAt(got.url);
        const seenNow = await got.look;
        setLook(seenNow);
        setPhase('ready');
        if (playWhenHere.current && !seenNow.unplayable) setPlaying(true);
        playWhenHere.current = false;
      },
      (err) => {
        release(e);
        held.current = null;
        if (err instanceof TooBig) {
          setTotal(err.total);
          setPhase('large');
        } else {
          setPhase('failed');
          playWhenHere.current = false;
        }
      },
    );
  };
  useEffect(() => {
    if (!seen) return;
    if (!serves) setPhase('away');
    else if (phase === 'idle' || phase === 'away') download(AUTO_VIDEO);
  }, [seen, serves, key]);
  useEffect(
    () => () => {
      release(held.current);
      held.current = null;
    },
    [key],
  );

  const tap = () => {
    if (phase === 'ready' && !look?.unplayable) setPlaying(true);
    else if (phase === 'large' || phase === 'failed') {
      playWhenHere.current = true;
      download(Infinity);
    }
  };
  const kind = 'Video';
  const size = total ? sizeWords(total) : null;
  const caption =
    phase === 'away'
      ? 'On your computer'
      : phase === 'failed'
        ? 'Couldn’t load it. Tap to try again.'
        : phase === 'large'
          ? `${kind} · ${size} · Tap to download`
          : phase === 'loading'
            ? [kind, size, 'Downloading…'].filter(Boolean).join(' · ')
            : [kind, look?.duration ? clock(look.duration, true) : null, size, look?.unplayable ? 'This browser can’t play it' : null].filter(Boolean).join(' · ');
  const aspect = Math.min(Math.max(look?.aspect || 16 / 9, 0.75), 2.4);

  return html`<div class=${`video-card ${phase}`}>
    <div class="video-face" ref=${face} style=${{ aspectRatio: String(aspect) }}>
      ${playing && url
        ? html`<video src=${url} controls autoplay playsinline onError=${() => {
            setPlaying(false);
            setLook({ ...(look || {}), unplayable: true });
          }}></video>`
        : html`<button type="button" class="video-poster" aria-label=${`${label}: ${caption}`} disabled=${phase === 'away' || phase === 'loading' || (phase === 'ready' && look?.unplayable)} onClick=${tap}>
            ${look?.poster ? html`<img src=${look.poster} alt="" draggable="false" />` : null}
            <span class="video-badge">
              ${phase === 'loading'
                ? html`<${Ring} value=${progress} size=${44} light=${true} />`
                : phase === 'away' || look?.unplayable
                  ? html`<${Icon} name="film" />`
                  : phase === 'large'
                    ? html`<span class="video-play"><${Icon} name="download" /></span>`
                    : html`<span class="video-play"><${Icon} name="play-fill" /></span>`}
            </span>
          </button>`}
    </div>
    <div class="video-words"><div class="video-name">${label}</div><div class=${`video-caption ${phase === 'failed' ? 'bad' : ''}`}>${caption}</div></div>
  </div>`;
}

/** The files a message ends with, by name (a video it plays instead). */
function FileChips({ paths }) {
  return html`<div class="msg-files">
    ${paths.map((p) => html`<span class="file-chip" key=${p} title=${p}><${Icon} name="file" size="s" /><span class="file-chip-name">${displayName(p)}</span></span>`)}
  </div>`;
}

/** Under a message's bubble: its photos (`attachment`), its videos (`media`), and the files it ends with. */
export function MessageMedia({ item, threadId }) {
  if (item?.kind !== 'user') return null;
  const images = app.has('attachment') ? item.images || 0 : 0;
  const videos = item.videos || [];
  const files = splitMentions(item.text || '').files.filter((p) => !videos.includes(p));
  if (!images && !videos.length && !files.length) return null;
  return html`<div class="msg-media">
    ${images > 0 && html`<${MessageImages} threadId=${threadId} itemId=${item.id} count=${images} />`}
    ${videos.map((p) => html`<${VideoCard} key=${p} threadId=${threadId} path=${p} name=${displayName(p)} />`)}
    ${files.length > 0 && html`<${FileChips} paths=${files} />`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// In the transcript: what `watch_video` saw

function FrameThumb({ source, ready }) {
  const pic = usePicture(source.key(FRAME_PX), source.load(FRAME_PX), ready);
  const width = pic.width && pic.height ? Math.min(160, Math.max(36, (64 * pic.width) / pic.height)) : (64 * 16) / 9;
  return html`<span class="watch-thumb" style=${{ width: `${Math.round(width)}px` }}>
    ${pic.url ? html`<img src=${pic.url} alt="" draggable="false" />` : pic.failed ? html`<${Icon} name="image-off" size="s" />` : ready ? html`<${Spinner} />` : null}
  </span>`;
}

/** Under a finished `watch_video` row (`tool.watch`): the video while it's still there, and the frames the agent saw. */
export function WatchedVideo({ watch, threadId }) {
  const box = useRef(null);
  const seen = useSeen(box);
  if (!watch) return null;
  const frames = watch.frames || [];
  const sources = frames.map((f) => mediaSource(threadId, f.path, FRAME_PX, `Frame at ${clock(f.t)}`));
  const note = frames.length === 1 ? 'The frame the agent saw' : frames.length ? `The ${frames.length} frames the agent saw` : 'No frames: the video had no picture, or they were cleared';
  return html`<div class="watch" ref=${box}>
    ${watch.video && html`<${VideoCard} threadId=${threadId} path=${watch.video} name=${baseName(watch.source) || displayName(watch.video)} />`}
    <div class="watch-note">${note}</div>
    ${frames.length > 0 &&
    html`<div class="watch-frames">
      ${frames.map(
        (f, i) => html`<button type="button" class="watch-frame" key=${f.path} aria-label=${`Frame at ${clock(f.t)}`} onClick=${() => openViewer(sources, i)}>
          <${FrameThumb} source=${sources[i]} ready=${seen && app.has('media')} />
          <span class="mono">${clock(f.t)}</span>
        </button>`,
      )}
    </div>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// In the transcript: the pictures an answer shows

function words(text) {
  const span = document.createElement('span');
  span.textContent = text;
  return span;
}

function spinnerNode() {
  const s = document.createElement('span');
  s.className = 'spinner';
  s.setAttribute('role', 'status');
  s.setAttribute('aria-label', 'Loading');
  return s;
}

/** A picture on the web stays there (this page talks only to your computer): a link opens it in the browser. */
function webPicture(img, link) {
  let host = '';
  try {
    host = new URL(link).host;
  } catch {
    return;
  }
  const a = document.createElement('a');
  a.className = 'md-web-pic';
  a.href = link;
  a.target = '_blank';
  a.rel = 'noopener noreferrer';
  const text = document.createElement('span');
  text.className = 'md-web-pic-words';
  const name = words((img.getAttribute('alt') || '').trim() || 'A picture on the web');
  name.className = 'md-web-pic-name';
  const where = words(`${host} · opens in the browser`);
  where.className = 'md-web-pic-host';
  text.append(name, where);
  a.append(svgIcon('image'), text, svgIcon('external'));
  img.replaceWith(a);
}

function openFromAnswer(root, box, threadId) {
  const boxes = [...root.querySelectorAll('.md-pic[data-path]')].filter((b) => !b.classList.contains('away'));
  const sources = boxes.map((b) => mediaSource(threadId, b.dataset.path, ANSWER_PX, b.querySelector('img')?.alt));
  openViewer(sources, Math.max(0, boxes.indexOf(box)));
}

/** A picture on the computer: fetched (`media`) as it comes into view, a tap opens it full screen. */
function localPicture(img, link, threadId, root) {
  const path = mediaPath(link);
  const label = (img.getAttribute('alt') || '').trim() || baseName(path) || 'Picture';
  const box = document.createElement('span');
  box.className = 'md-pic loading';
  box.dataset.path = path;
  const note = document.createElement('span');
  note.className = 'md-pic-note';
  img.replaceWith(box);
  img.alt = label;
  img.draggable = false;
  box.append(img, note);
  if (!threadId || !app.has('media')) {
    box.className = 'md-pic away';
    note.append(svgIcon('image'), words(`${label} · on your computer`));
    return;
  }
  const load = () => {
    box.className = 'md-pic loading';
    note.replaceChildren(spinnerNode());
    holdFor(img, mediaKey(threadId, path, ANSWER_PX), loadMediaPicture(threadId, path, ANSWER_PX)).promise.then(
      (got) => {
        img.src = got.url;
        box.className = 'md-pic';
        box.tabIndex = 0;
        box.setAttribute('role', 'button');
        box.setAttribute('aria-label', `${label}. Opens it full screen`);
      },
      () => {
        box.className = 'md-pic failed';
        box.tabIndex = 0;
        box.setAttribute('role', 'button');
        box.setAttribute('aria-label', `Couldn’t load ${label}. Try again`);
        note.replaceChildren(svgIcon('image-off'), words(`Couldn’t load ${label}. Tap to try again.`));
      },
    );
  };
  const act = () => {
    if (box.classList.contains('failed')) load();
    else if (!box.classList.contains('loading')) openFromAnswer(root, box, threadId);
  };
  box.addEventListener('click', act);
  box.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      act();
    }
  });
  note.append(spinnerNode());
  whenSeen(box, load);
}

/** Every picture an answer shows (markdown.js leaves a link that isn't data: or blob: in `data-src`). */
export function enhancePictures(root, { threadId } = {}) {
  for (const img of root.querySelectorAll('img[data-src]')) {
    if (img.parentElement?.classList.contains('md-pic')) continue;
    const link = img.getAttribute('data-src') || '';
    if (!link) continue;
    if (isWeb(link)) webPicture(img, link);
    else if (/^[a-z][a-z0-9+.-]*:/i.test(link) && !link.startsWith('file:') && !/^[A-Za-z]:[\\/]/.test(link)) img.remove();
    else localPicture(img, link, threadId, root);
  }
}

// ---------------------------------------------------------------------------------------------
// Full screen

let viewerHost = null;

/** Pictures full screen: swipe (or the arrow keys) between them, double-tap to zoom, pull down or Back to close. */
export function openViewer(sources, start = 0) {
  if (typeof document === 'undefined' || !sources.length) return;
  if (!viewerHost) {
    viewerHost = document.createElement('div');
    viewerHost.className = 'viewer-host';
    document.body.append(viewerHost);
  }
  render(html`<${Viewer} key=${String(Date.now())} sources=${sources} start=${Math.max(0, Math.min(start, sources.length - 1))} />`, viewerHost);
}

function closeViewer() {
  if (viewerHost) render(null, viewerHost);
}

function ViewerPage({ source, transform, settling, onUrl }) {
  const thumb = usePicture(source.thumbPx ? source.key(source.thumbPx) : null, null);
  const full = usePicture(source.key(FULL_PX), source.load(FULL_PX));
  useEffect(() => onUrl(full.url), [full.url]);
  const style = { transform, transition: settling ? 'transform 0.25s ease-out' : 'none' };
  if (full.url) return html`<img class="viewer-img" src=${full.url} alt=${source.label} style=${style} draggable="false" />`;
  if (full.failed) {
    return html`<div class="viewer-failed"><${Icon} name="image-off" /><b>Couldn’t load this image</b><span>It may not be on your computer any more.</span></div>`;
  }
  return html`${thumb.url && html`<img class="viewer-img blurred" src=${thumb.url} alt="" style=${style} draggable="false" />`}<span class="viewer-spin"><${Spinner} /></span>`;
}

function Viewer({ sources, start }) {
  const [page, setPage] = useState(start);
  const [shift, setShift] = useState({ x: 0, y: 0, settling: false });
  const [zoom, setZoom] = useState(null);
  const [url, setUrl] = useState(null);
  const stage = useRef(null);
  const gesture = useRef(null);
  const lastTap = useRef(0);
  const pushed = useRef(false);
  const count = sources.length;

  const go = (step) => setPage((p) => Math.max(0, Math.min(count - 1, p + step)));
  const close = () => {
    closeViewer();
    if (pushed.current) {
      pushed.current = false;
      history.back();
    }
  };
  useEffect(() => {
    // Back (Android's gesture too) closes the pictures rather than leaving the thread.
    try {
      history.pushState({ ...(history.state || {}), viewer: true }, '');
      pushed.current = true;
    } catch {
      // A page that can't keep history: the ✕ closes it.
    }
    const back = () => {
      if (!pushed.current) return;
      pushed.current = false;
      closeViewer();
    };
    const key = (e) => {
      if (e.key === 'Escape') close();
      else if (e.key === 'ArrowRight') go(1);
      else if (e.key === 'ArrowLeft') go(-1);
    };
    window.addEventListener('popstate', back);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('popstate', back);
      window.removeEventListener('keydown', key);
    };
  }, []);
  // A new page starts unzoomed; it says its own URL once it has it.
  useEffect(() => setZoom(null), [page]);

  const down = (e) => {
    if (e.button > 0) return;
    stage.current?.setPointerCapture?.(e.pointerId);
    gesture.current = { id: e.pointerId, x: e.clientX, y: e.clientY, axis: null, zoom };
  };
  const move = (e) => {
    const g = gesture.current;
    if (!g || g.id !== e.pointerId) return;
    const dx = e.clientX - g.x;
    const dy = e.clientY - g.y;
    if (!g.axis && Math.hypot(dx, dy) > 8) g.axis = g.zoom ? 'pan' : Math.abs(dx) > Math.abs(dy) ? 'x' : 'y';
    if (g.axis === 'pan') setZoom({ ...g.zoom, x: g.zoom.x + dx, y: g.zoom.y + dy });
    // Past the first or the last, it gives a little and no more.
    else if (g.axis === 'x') setShift({ x: (page === 0 && dx > 0) || (page === count - 1 && dx < 0) ? dx / 3 : dx, y: 0, settling: false });
    else if (g.axis === 'y') setShift({ x: 0, y: dy > 0 ? dy : dy / 4, settling: false });
  };
  const up = (e) => {
    const g = gesture.current;
    if (!g || g.id !== e.pointerId) return;
    gesture.current = null;
    const dx = e.clientX - g.x;
    const dy = e.clientY - g.y;
    if (!g.axis) {
      const now = Date.now();
      if (now - lastTap.current < 320) {
        lastTap.current = 0;
        if (zoom) setZoom(null);
        else {
          const r = stage.current.getBoundingClientRect();
          const s = 2.5;
          setZoom({ s, x: -(e.clientX - (r.left + r.width / 2)) * (s - 1), y: -(e.clientY - (r.top + r.height / 2)) * (s - 1) });
        }
      } else lastTap.current = now;
      return;
    }
    if (g.axis === 'y' && dy > 110) return close();
    if (g.axis === 'x' && dx < -60) go(1);
    if (g.axis === 'x' && dx > 60) go(-1);
    setShift({ x: 0, y: 0, settling: true });
  };

  const tall = typeof window === 'undefined' ? 800 : window.innerHeight;
  const pull = Math.min(1, Math.max(0, shift.y / (tall * 0.5)));
  const transform = zoom ? `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.s})` : `translate(${shift.x}px, ${shift.y}px) scale(${1 - 0.15 * pull})`;
  const source = sources[page];
  return html`<div class="viewer" role="dialog" aria-modal="true" aria-label=${count > 1 ? `Pictures, ${page + 1} of ${count}` : 'Picture'} style=${{ background: `rgb(0 0 0 / ${1 - pull})` }}>
    <div class="viewer-stage" ref=${stage} onPointerDown=${down} onPointerMove=${move} onPointerUp=${up} onPointerCancel=${up}>
      <${ViewerPage} key=${page} source=${source} transform=${transform} settling=${shift.settling && !zoom} onUrl=${setUrl} />
    </div>
    <div class="viewer-bar" style=${{ opacity: Math.max(0, 1 - pull * 3) }}>
      <button type="button" class="viewer-btn" aria-label="Close" onClick=${close}><${Icon} name="x" /></button>
      <span class="viewer-count">${count > 1 ? `${page + 1} of ${count}` : ''}</span>
      ${url ? html`<a class="viewer-btn" href=${url} download=${source.name} aria-label="Save" title="Save"><${Icon} name="download" /></a>` : html`<span class="viewer-btn" aria-hidden="true"></span>`}
    </div>
    ${count > 1 && page > 0 && html`<button type="button" class="viewer-step prev" aria-label="Previous" onClick=${() => go(-1)}><${Icon} name="back" /></button>`}
    ${count > 1 && page < count - 1 && html`<button type="button" class="viewer-step next" aria-label="Next" onClick=${() => go(1)}><${Icon} name="forward" /></button>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// Agents' marks

/** The logos the computer names (`agent.logo`, Trek's `logo_key`): `logos/{dark,light}/<key>.png`. */
export const LOGOS = new Set(['claude-code', 'codex', 'opencode', 'droid', 'cursor', 'github-copilot', 'gemini', 'kimi', 'qwen-code', 'grok', 'devin', 'goose', 'amp', 'pi', 'anthropic', 'openai', 'google', 'openrouter', 'deepseek', 'xai', 'mistral', 'groq', 'ollama', 'lmstudio']);

/** Added agents' icons by hash: `{ png, template }`, here and kept on this phone (a new hash fetches it again). */
const icons = new Map();
const iconAsks = new Map();

function storedIcon(hash) {
  if (icons.has(hash)) return icons.get(hash);
  try {
    const kept = JSON.parse(localStorage.getItem(`trek-remote-icon:${hash}`) || 'null');
    if (kept?.png) {
      icons.set(hash, kept);
      return kept;
    }
  } catch {
    // Asked for again.
  }
  return null;
}

function iconFor(agentKey, hash) {
  if (iconAsks.has(hash)) return iconAsks.get(hash);
  const asked = askFor({ type: 'icon', agent: agentKey })
    .then((r) => {
      const icon = { png: r.png, template: !!r.template };
      for (const h of new Set([hash, r.hash])) {
        icons.set(h, icon);
        try {
          localStorage.setItem(`trek-remote-icon:${h}`, JSON.stringify(icon));
        } catch {
          // Kept for this visit.
        }
      }
      return icon;
    })
    .catch(() => null)
    .finally(() => iconAsks.delete(hash));
  iconAsks.set(hash, asked);
  return asked;
}

function AddedIcon({ agent, size }) {
  const [icon, setIcon] = useState(() => storedIcon(agent.icon));
  useEffect(() => {
    const had = storedIcon(agent.icon);
    setIcon(had);
    if (had || !app.has('icon')) return undefined;
    let live = true;
    iconFor(agent.key, agent.icon).then((got) => live && got && setIcon(got));
    return () => {
      live = false;
    };
  }, [agent.icon, agent.key]);
  const box = { width: `${size}px`, height: `${size}px` };
  if (!icon) return html`<span class="agent-mark none" aria-hidden="true" style=${box}><${Icon} name="cpu" /></span>`;
  const picture = `url("data:image/png;base64,${icon.png}")`;
  return icon.template
    ? html`<span class="agent-mark shape" aria-hidden="true" style=${{ ...box, '--shape': picture }}></span>`
    : html`<span class="agent-mark picture" aria-hidden="true" style=${{ ...box, '--picture': picture }}></span>`;
}

/**
 * An agent's mark: its logo (`agent.logo`, else its key's), an added agent's own icon (`icon`,
 * a shape drawn in the text colour or a picture), or a neutral chip.
 */
export function AgentMark({ agent, size = 20 }) {
  const key = agent?.logo || agent?.key || '';
  const box = { width: `${size}px`, height: `${size}px` };
  if (LOGOS.has(key)) {
    return html`<span class="agent-mark logo" aria-hidden="true" style=${{ ...box, '--logo-dark': `url("/remote/logos/dark/${key}.png")`, '--logo-light': `url("/remote/logos/light/${key}.png")` }}></span>`;
  }
  if (agent?.icon) return html`<${AddedIcon} agent=${agent} size=${size} />`;
  return html`<span class="agent-mark none" aria-hidden="true" style=${box}><${Icon} name="cpu" /></span>`;
}

// ---------------------------------------------------------------------------------------------
// Registered

routes.registerComposerTool(AttachTool);
routes.registerComposerTool(DictateTool);
routes.registerComposerTool(SlashTool);
routes.registerComposerPreview(AttachmentStrip);
routes.registerMarkdownEnhancer(enhancePictures);

/** Under a finished `watch_video` row: what it watched. */
function ToolWatch({ item, threadId }) {
  return item?.watch ? html`<${WatchedVideo} watch=${item.watch} threadId=${threadId} />` : null;
}

/** A message's photos, videos and files, among a message's parts (`turnActions`): drawn first, so under its bubble. */
function MessageMediaPart({ item, threadId }) {
  return item?.kind === 'user' ? html`<${MessageMedia} item=${item} threadId=${threadId} />` : null;
}

// Drawn with a message and under a tool's row where items.js lets a part be (`registerItemPart`);
// until then a message's go first among its actions, and a watched video's frames wait.
routes.registerPart('agent-mark', AgentMark);
routes.registerPart('split-mentions', splitMentions);
routes.registerPart('caption', captionFor);
if (typeof routes.registerItemPart === 'function') {
  routes.registerItemPart('user', MessageMedia);
  routes.registerItemPart('tool', ToolWatch);
} else {
  routes.turnActions.unshift(MessageMediaPart);
}
