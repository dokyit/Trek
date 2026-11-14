// The computer's notes (`docs/MOBILE.md` › Notes), as the iPhone's Notes tab has them: the list,
// newest first, to search and add to; a note read as its markdown (boxes tick, pictures show and
// open full screen, other files kept with the notes open from a card) or written as plain text,
// saved a moment after typing stops. When the computer refuses a save because the note changed
// there since it was opened here, the user picks which version to keep. Opened from the thread
// list's menu.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, IconButton, Menu, Sheet, Spinner, Segmented, addIcons, ago, clock, copy } from '../ui.js';
import { Markdown } from '../markdown.js';
import * as app from '../model.js';
import { fromBase64, toBase64 } from '../link.js';
import { ConnectionBanner } from '../thread.js';
import { registerListMenuItems, registerScreen } from '../routes.js';

// Lucide (ISC licence), 24 × 24, stroked.
addIcons({
  'note-new': '<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z"/>',
  'note-box': '<circle cx="12" cy="12" r="9"/>',
  'note-ticked': '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="m8 12.5 2.8 2.8L16.5 9" style="stroke: var(--bg)"/>',
  'note-photo': '<path d="M16 5h6"/><path d="M19 2v6"/><path d="M21 11.5V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7.5"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/><circle cx="9" cy="9" r="2"/>',
  'note-film': '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M7 3v18"/><path d="M3 7.5h4"/><path d="M3 12h18"/><path d="M3 16.5h4"/><path d="M17 3v18"/><path d="M17 7.5h4"/><path d="M17 16.5h4"/>',
  'note-save': '<path d="M12 15V3"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/>',
});

// ---------------------------------------------------------------------------------------------
// A note's text: the pieces its preview draws, and the edits the editor makes

const IMAGE = /!\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)/g;
const LINK_ONLY = /^\[([^\]\n]*)\]\(\s*(?:<([^>\n]+)>|([^)\s]+))(?:\s+"[^"\n]*")?\s*\)$/;
const CHECK = /^(\s*)[-*+]\s+\[([ xX])\]\s?(.*)$/;
/** The list, heading or quote marker a line may start with, after its indent. */
const MARKER = /^(\s*)(#{1,6}\s+|[-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+[.)]\s+|>\s?)/;

/** `attachments/<name>` when `link` is a file kept with the notes (as the computer serves them), else null. */
export function attachment(link) {
  const path = String(link || '').startsWith('./') ? link.slice(2) : String(link || '');
  if (!path.startsWith('attachments/')) return null;
  const name = path.slice('attachments/'.length);
  if (!name || name.startsWith('.') || /[/\\:]/.test(name) || name.includes('..')) return null;
  return path;
}

/** The pictures on a line that's nothing but pictures (as the computer's Write puts them in), else null. */
export function imagesOnly(line) {
  const found = [];
  let rest = '';
  let at = 0;
  for (const m of line.matchAll(IMAGE)) {
    rest += line.slice(at, m.index);
    found.push({ alt: m[1], link: m[2] || m[3] || '' });
    at = m.index + m[0].length;
  }
  rest += line.slice(at);
  return found.length > 0 && !rest.trim() ? found : null;
}

/** A line that's only a link to a file kept with the notes, `[name](attachments/…)`: `{ name, path }`. */
export function fileOnly(line) {
  const m = line.match(LINK_ONLY);
  const path = m && attachment(m[2] || m[3] || '');
  return path ? { name: m[1], path } : null;
}

function indentOf(spaces) {
  let n = 0;
  for (const c of spaces) n += c === '\t' ? 2 : 1;
  return Math.floor(n / 2);
}

/**
 * A note's text in the pieces its preview draws, in order: `md` (drawn by `Markdown`), `images`
 * (a line of nothing but pictures), `file` (a line that's only a file kept with the notes) and
 * `checks` (check list items, each with its 0-based `line` for ticking). Lines in a code fence
 * stay markdown, whatever they look like.
 */
export function noteBlocks(body) {
  const out = [];
  let md = [];
  let mdLine = 0;
  let checks = null;
  let fence = null;
  const flush = () => {
    if (md.some((l) => l.trim())) out.push({ kind: 'md', text: md.join('\n'), line: mdLine });
    md = [];
  };
  const endChecks = () => {
    if (checks) out.push(checks);
    checks = null;
  };
  const text = (raw, i) => {
    if (md.length === 0) mdLine = i;
    md.push(raw);
  };
  String(body || '')
    .split('\n')
    .forEach((raw, i) => {
      const trimmed = raw.trim();
      const opens = trimmed.match(/^(`{3,}|~{3,})/);
      if (fence) {
        text(raw, i);
        if (opens && opens[1][0] === fence[0] && opens[1].length >= fence.length && !trimmed.slice(opens[1].length).trim()) fence = null;
        return;
      }
      if (opens) {
        endChecks();
        fence = opens[1];
        text(raw, i);
        return;
      }
      const images = trimmed.startsWith('![') ? imagesOnly(trimmed) : null;
      if (images) {
        endChecks();
        flush();
        out.push({ kind: 'images', images, line: i });
        return;
      }
      const file = trimmed.startsWith('[') ? fileOnly(trimmed) : null;
      if (file) {
        endChecks();
        flush();
        out.push({ kind: 'file', file, line: i });
        return;
      }
      const check = raw.match(CHECK);
      if (check) {
        flush();
        if (!checks) checks = { kind: 'checks', items: [], line: i };
        checks.items.push({ line: i, done: check[2] !== ' ', indent: indentOf(check[1]), text: check[3] });
        return;
      }
      endChecks();
      text(raw, i);
    });
  endChecks();
  flush();
  return out;
}

/** Tick or untick the check box on line `index` (0-based); null when there's none there. */
export function toggleCheck(text, index) {
  const lines = String(text).split('\n');
  const m = lines[index]?.match(/^(\s*[-*+]\s+\[)([ xX])(\])/);
  if (!m) return null;
  lines[index] = m[1] + (m[2] === ' ' ? 'x' : ' ') + m[3] + lines[index].slice(m[0].length);
  return lines.join('\n');
}

/** The note's title as the computer takes it: its first line with words in it, without markdown. */
export function noteTitle(body) {
  for (const line of String(body || '').split('\n')) {
    let s = line.replace(MARKER, '');
    s = s
      .replace(/!\[([^\]\n]*)\]\([^)\n]*\)/g, '$1')
      .replace(/\[([^\]\n]*)\]\([^)\n]*\)/g, '$1')
      .replace(/<[^>]+>/g, '')
      .replace(/[*_~`]/g, '')
      .trim();
    if (s) return Array.from(s).slice(0, 80).join('');
  }
  return 'Untitled';
}

/**
 * Return was typed and its new line ends at `at`: carry the line's list on (a bullet, the next
 * number, an empty check box), or end the list when the line was an empty item. `{ text, cursor }`,
 * or null when the line isn't a list's.
 */
export function continueList(text, at) {
  if (at <= 0 || at > text.length || text[at - 1] !== '\n') return null;
  let start = at - 1;
  while (start > 0 && text[start - 1] !== '\n') start -= 1;
  const previous = text.slice(start, at - 1);
  const m = previous.match(MARKER);
  if (!m || !m[0]) return null;
  const marker = m[0];
  const trimmed = marker.trim();
  if (trimmed.startsWith('#')) return null;
  if (previous.length === marker.length || previous.trim() === trimmed) {
    // An empty item: the list ends here, and its marker goes.
    return { text: text.slice(0, start) + text.slice(at), cursor: start };
  }
  let next = marker;
  const num = trimmed.match(/^(\d+)([.)])/);
  if (num) next = `${m[1]}${Number(num[1]) + 1}${num[2]} `;
  else if (trimmed.includes('[')) next = `${m[1]}- [ ] `;
  return { text: text.slice(0, at) + next + text.slice(at), cursor: at + next.length };
}

/**
 * Put `block` (whole lines: a picture's) where the selection was, on lines of its own, as the
 * computer's Write does: the text around keeps its lines, and the cursor goes to the start of the
 * line after it.
 */
export function insertLines(text, from, to, block) {
  const lo = Math.max(0, Math.min(from, text.length));
  const hi = Math.max(lo, Math.min(to, text.length));
  const before = text.slice(0, lo);
  const after = text.slice(hi);
  const lead = !before || before.endsWith('\n') ? '' : '\n';
  const tail = after.startsWith('\n') ? '' : '\n';
  return { text: before + lead + block + tail + after, cursor: before.length + lead.length + block.length + 1 };
}

/** What a file kept with the notes opens as: `pdf`, `video` (what a browser plays) or `other`. */
export function fileKind(path) {
  const ext = (String(path).match(/\.([^./]+)$/)?.[1] || '').toLowerCase();
  if (ext === 'pdf') return 'pdf';
  if (['mp4', 'm4v', 'mov', 'webm'].includes(ext)) return 'video';
  return 'other';
}

/** "2.4 MB", as a phone says a file's size. */
export function bytes(n) {
  if (!Number.isFinite(n)) return '';
  if (n < 1000) return `${n} bytes`;
  if (n < 1e6) return `${Math.round(n / 1000)} KB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(1).replace(/\.0$/, '')} MB`;
  return `${(n / 1e9).toFixed(1).replace(/\.0$/, '')} GB`;
}

function baseName(path) {
  return String(path).split('/').pop() || String(path);
}

// ---------------------------------------------------------------------------------------------
// The notes, as the computer last said them

const notes = {
  /** The list, newest first; null before it first comes. */
  list: null,
  loading: false,
  error: null,
  /** The machine the list is from: another one's isn't shown. */
  host: null,
  /** Notes read whole, by id, as the computer last had them (their `modified` goes with a save). */
  open: new Map(),
  /** Notes with a save out: one at a time, each carrying the version it was made from. */
  saving: new Set(),
  /** What a closed editor left unsaved, by id: saved once the save out is back, or when the connection is. */
  left: new Map(),
};

const listeners = new Set();
function notify() {
  for (const l of listeners) l();
}

function useNotes() {
  const [, bump] = useState(0);
  useEffect(() => {
    const l = () => bump((n) => n + 1);
    listeners.add(l);
    return () => listeners.delete(l);
  }, []);
  return notes;
}

const newestFirst = (a, b) => (b.modified || 0) - (a.modified || 0);

function sameHost() {
  const host = app.state.machine?.hostId || null;
  if (notes.host !== host) {
    notes.host = host;
    notes.list = null;
    notes.error = null;
    notes.open.clear();
  }
}

async function loadNotes() {
  sameHost();
  notes.loading = true;
  notify();
  try {
    const reply = await app.ask({ type: 'notes' });
    notes.list = (reply.notes || []).slice().sort(newestFirst);
    notes.error = null;
  } catch (err) {
    notes.error = err.message;
  } finally {
    notes.loading = false;
    notify();
  }
  for (const id of [...notes.left.keys()]) saveLeft(id);
}

async function openNote(id) {
  const reply = await app.ask({ type: 'note', note_id: id });
  notes.open.set(id, reply.note);
  notify();
  return reply.note;
}

async function createNote(body = '') {
  sameHost();
  const reply = await app.ask({ type: 'create_note', body });
  notes.open.set(reply.note.id, reply.note);
  loadNotes();
  return reply.note;
}

/**
 * Save a note's text, carrying the `modified` it was opened at: if it changed on the computer
 * since, nothing is written and this rejects with `conflict`.
 */
async function saveNote(id, body) {
  const modified = notes.open.get(id)?.modified;
  notes.saving.add(id);
  notify();
  try {
    const reply = await app.ask({ type: 'save_note', note_id: id, body, ...(modified != null ? { modified } : {}) }, { timeout: 20_000 });
    const n = reply.note;
    notes.open.set(id, n);
    const at = notes.list ? notes.list.findIndex((x) => x.id === id) : -1;
    if (at >= 0) {
      notes.list[at] = { ...notes.list[at], title: n.title, modified: n.modified };
      notes.list = notes.list.slice().sort(newestFirst);
    }
    notes.saving.delete(id);
    notify();
    saveLeft(id);
    return n;
  } catch (err) {
    notes.saving.delete(id);
    // What its closed editor left would be refused the same way.
    if (err.code === 'conflict' && notes.left.delete(id)) app.toast('That note changed on your computer, so your last changes to it weren’t saved.', 'negative');
    notify();
    throw err;
  }
}

/** Save what a note's editor had when it closed: now, after the save out for it, or once the connection is back. */
function saveNoteLeft(id, body) {
  notes.left.set(id, body);
  saveLeft(id);
}

function saveLeft(id) {
  if (notes.saving.has(id) || !notes.left.has(id) || app.state.status !== 'online') return;
  const body = notes.left.get(id);
  notes.left.delete(id);
  if (notes.open.get(id)?.body === body) return;
  saveNote(id, body).catch((err) => {
    if (err.code === 'conflict') return;
    app.toast(err.message, 'negative');
    if (!notes.left.has(id)) notes.left.set(id, body);
  });
}

/** Delete a note: the computer keeps it in the notes folder's Deleted. */
async function deleteNote(id, { quiet = false } = {}) {
  const before = notes.list;
  if (notes.list) notes.list = notes.list.filter((x) => x.id !== id);
  notes.open.delete(id);
  notes.left.delete(id);
  notify();
  try {
    await app.ask({ type: 'delete_note', note_id: id });
  } catch (err) {
    if (quiet && err.code === 'not_found') return;
    app.toast(err.message, 'negative');
    notes.list = before;
    notify();
  }
}

// Back in touch (a snapshot follows every welcome): what couldn't be saved meanwhile goes now.
app.onMessage((msg) => {
  if (msg.type === 'snapshot') for (const id of [...notes.left.keys()]) setTimeout(() => saveLeft(id), 0);
});

// ---------------------------------------------------------------------------------------------
// Pictures (`note_asset`), drawn from blob: URLs

/** A thumbnail sharp across a phone's width; the viewer asks for the most the computer sends. */
const THUMB_PX = 1024;
const FULL_PX = 2048;
const KEEP_PICTURES = 80;

const assetUrls = new Map();
const assetAsks = new Map();

const assetKey = (noteId, path, px) => `${noteId}\n${path}\n${px}`;

function keepAsset(key, url) {
  assetUrls.set(key, url);
  while (assetUrls.size > KEEP_PICTURES) {
    const [oldest, gone] = assetUrls.entries().next().value;
    assetUrls.delete(oldest);
    assetAsks.delete(oldest);
    URL.revokeObjectURL(gone);
  }
}

/** A picture already here, at `px` or bigger. */
function cachedAsset(noteId, path, px = THUMB_PX) {
  return assetUrls.get(assetKey(noteId, path, 'full')) || (px <= THUMB_PX ? assetUrls.get(assetKey(noteId, path, THUMB_PX)) : null) || assetUrls.get(assetKey(noteId, path, FULL_PX)) || null;
}

/** One of a note's pictures as a blob: URL, fitting `px` a side. */
function noteAsset(noteId, path, px) {
  const here = assetUrls.get(assetKey(noteId, path, 'full')) || assetUrls.get(assetKey(noteId, path, px));
  if (here) return Promise.resolve(here);
  const key = assetKey(noteId, path, px);
  let asking = assetAsks.get(key);
  if (!asking) {
    asking = app.ask({ type: 'note_asset', note_id: noteId, path, max_px: px }).then((reply) => {
      const url = URL.createObjectURL(new Blob([fromBase64(reply.data || '')], { type: reply.mime || 'image/jpeg' }));
      keepAsset(key, url);
      return url;
    });
    asking.catch(() => assetAsks.delete(key));
    assetAsks.set(key, asking);
  }
  return asking;
}

function canvasBlob(canvas, quality) {
  return new Promise((resolve) => canvas.toBlob(resolve, 'image/jpeg', quality));
}

/**
 * A photo picked on the phone as it goes to the computer, as the iPhone sends one: a JPEG with
 * its long side at most 1600 pixels, under about 900 KB.
 */
async function photoUpload(file) {
  const picture = await createImageBitmap(file, { imageOrientation: 'from-image' });
  const scale = Math.min(1, 1600 / Math.max(picture.width, picture.height, 1));
  const canvas = document.createElement('canvas');
  canvas.width = Math.max(1, Math.round(picture.width * scale));
  canvas.height = Math.max(1, Math.round(picture.height * scale));
  const g = canvas.getContext('2d');
  // A PNG's see-through parts would come out black in a JPEG.
  g.fillStyle = '#fff';
  g.fillRect(0, 0, canvas.width, canvas.height);
  g.drawImage(picture, 0, 0, canvas.width, canvas.height);
  picture.close?.();
  let quality = 0.82;
  let blob = await canvasBlob(canvas, quality);
  while (blob && blob.size > 900_000 && quality > 0.35) {
    quality -= 0.12;
    blob = await canvasBlob(canvas, quality);
  }
  if (!blob) throw new Error('no JPEG');
  return { blob, data: toBase64(new Uint8Array(await blob.arrayBuffer())) };
}

// ---------------------------------------------------------------------------------------------
// Other files kept with the notes (`note_file`): fetched a piece at a time, then opened from a blob: URL

/** A first piece, enough to learn the file's size and type as its card appears. */
const PEEK = 64 << 10;
/** The most one piece carries (`trek_remote::MAX_NOTE_FILE_CHUNK`). */
const PIECE = 1 << 20;
/** Downloaded files kept in the page, least recently opened going first. */
const KEEP_BYTES = 150e6;

const files = new Map();
const fileListeners = new Set();

function fileChanged() {
  for (const l of fileListeners) l();
}

function fileEntry(path) {
  let f = files.get(path);
  if (!f) {
    f = { path, total: null, mime: '', parts: [], received: 0, state: 'idle', error: null, url: null, used: 0, asking: null };
    files.set(path, f);
  }
  return f;
}

function useFile(path) {
  const [, bump] = useState(0);
  useEffect(() => {
    const l = () => bump((n) => n + 1);
    fileListeners.add(l);
    return () => fileListeners.delete(l);
  }, []);
  return fileEntry(path);
}

async function askPiece(f, length) {
  const reply = await app.ask({ type: 'note_file', path: f.path, offset: f.received, length }, { timeout: 60_000 });
  const got = fromBase64(reply.bytes_b64 || '');
  f.total = reply.total ?? f.total;
  f.mime = reply.mime || f.mime;
  if (got.length) {
    f.parts.push(got);
    f.received += got.length;
  }
  return got.length;
}

function finish(f) {
  f.url = URL.createObjectURL(new Blob(f.parts, { type: f.mime || 'application/octet-stream' }));
  f.parts = [];
  f.state = 'done';
  f.used = Date.now();
  let kept = 0;
  const done = [...files.values()].filter((x) => x.url).sort((a, b) => b.used - a.used);
  for (const x of done) {
    kept += x.total || 0;
    if (kept > KEEP_BYTES && x !== f) {
      URL.revokeObjectURL(x.url);
      Object.assign(x, { url: null, state: 'idle', received: 0, parts: [] });
    }
  }
}

function failed(f, err) {
  f.state = err.code === 'not_found' ? 'gone' : 'failed';
  f.error = err.message;
  f.parts = [];
  f.received = 0;
}

/** Its size and type, from a first piece (kept, so the download carries on from it). */
function peekFile(path) {
  const f = fileEntry(path);
  if (f.state !== 'idle' || f.total != null || f.asking) return;
  f.asking = askPiece(f, PEEK)
    .then(() => {
      if (f.received >= (f.total ?? Infinity)) finish(f);
    })
    .catch((err) => {
      if (err.code === 'not_found') failed(f, err);
    })
    .finally(() => {
      f.asking = null;
      fileChanged();
    });
}

/** The whole file, a piece at a time; resolves with its entry once it's here. */
async function fetchFile(path) {
  const f = fileEntry(path);
  if (f.url) {
    f.used = Date.now();
    return f;
  }
  if (f.state === 'loading') return f.loading;
  f.state = 'loading';
  f.error = null;
  fileChanged();
  f.loading = (async () => {
    try {
      if (f.asking) await f.asking.catch(() => {});
      while (f.total == null || f.received < f.total) {
        // An empty piece is the end.
        if ((await askPiece(f, PIECE)) === 0) break;
        fileChanged();
      }
      finish(f);
    } catch (err) {
      failed(f, err);
      throw err;
    } finally {
      fileChanged();
    }
    return f;
  })();
  return f.loading;
}

/** Open a file that's here: a PDF in a new tab, anything but a video saved with its name. */
function openFileUrl(f, name, kind) {
  const a = document.createElement('a');
  a.href = f.url;
  if (kind === 'pdf') {
    a.target = '_blank';
    a.rel = 'noopener';
  } else {
    a.download = name;
  }
  document.body.append(a);
  a.click();
  a.remove();
}

// ---------------------------------------------------------------------------------------------
// The list

function NotesScreen() {
  const state = app.useApp();
  const n = useNotes();
  const [query, setQuery] = useState(null);
  const [creating, setCreating] = useState(false);
  const search = useRef(null);
  const online = state.status === 'online';
  const mine = n.host === (state.machine?.hostId || null);

  useEffect(() => {
    if (online) loadNotes();
  }, [online]);
  useEffect(() => {
    if (query === '') search.current?.focus();
  }, [query === '']);

  const newNote = async () => {
    if (creating) return;
    setCreating(true);
    try {
      const note = await createNote('');
      app.go({ screen: 'note', noteId: note.id, isNew: true });
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      setCreating(false);
    }
  };

  const all = (mine && n.list) || [];
  const q = (query || '').trim().toLowerCase();
  const shown = q ? all.filter((x) => (x.title || '').toLowerCase().includes(q) || (x.preview || '').toLowerCase().includes(q)) : all;

  let body;
  if (!mine || !n.list) {
    body = n.error && !n.loading ? html`<div class="empty">Couldn’t load the notes: ${n.error}</div>` : html`<div class="empty">${online || n.loading ? html`<${Spinner} />` : 'Nothing here until Trek answers.'}</div>`;
  } else if (shown.length === 0) {
    body = q
      ? html`<div class="empty">No note matches “${query.trim()}”.</div>`
      : html`<div class="empty notes-none">
          <${Icon} name="notes" />
          <h2>No notes yet</h2>
          <p>Jot something down here or in Notes on your computer.</p>
          <button class="btn" type="button" disabled=${!online || creating} onClick=${newNote}><${Icon} name="note-new" size="s" />New note</button>
        </div>`;
  } else {
    body = shown.map(
      (x) => html`<button type="button" key=${x.id} class="note-row" onClick=${() => app.go({ screen: 'note', noteId: x.id })}>
        <span class="note-row-top"><span class="note-row-title">${x.title || 'Untitled'}</span><span class="note-row-time">${ago(x.modified)}</span></span>
        <span class="note-row-preview">${x.preview || 'No more text'}</span>
      </button>`,
    );
  }

  return html`<div class="screen notes">
    <header class="bar">
      ${query == null
        ? html`<${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
            <div class="bar-title"><h1>Notes</h1>${state.machine?.name && html`<div class="sub">${state.machine.name}</div>`}</div>
            <${IconButton} icon="search" label="Search notes" onClick=${() => setQuery('')} />
            <${IconButton} icon="note-new" label="New note" disabled=${!online || creating} onClick=${newNote} />`
        : html`<${IconButton} icon="back" label="Close search" onClick=${() => setQuery(null)} />
            <input class="input notes-search" ref=${search} type="search" placeholder="Search notes" aria-label="Search notes" value=${query} onInput=${(e) => setQuery(e.target.value)} />`}
    </header>
    <${ConnectionBanner} />
    <div class="scroll notes-list">${body}</div>
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// A note

/** A note's screen, made again for each note (the route carries which). */
function NoteRoute(props) {
  return html`<${NoteScreen} key=${props.noteId} noteId=${props.noteId} isNew=${!!props.isNew} />`;
}

/**
 * A note, read (as its markdown draws, boxes tickable) or written (its text). Saved a moment after
 * typing stops, when switched to Preview, when the page is put away and when left; one save at a
 * time. A note made here just now opens for writing, and goes again if it's left empty.
 */
function NoteScreen({ noteId, isNew }) {
  const state = app.useApp();
  useNotes();
  const online = state.status === 'online';
  // What the editor knows, read by its timers and replies as it is then.
  const ed = useRef(null);
  if (!ed.current) ed.current = { text: '', saved: '', loaded: false, saving: false, adding: false, left: false, deleted: false, theirs: null, timer: 0, caret: null };
  const e = ed.current;
  const [, redraw] = useState(0);
  const touch = () => redraw((n) => n + 1);
  const [mode, setMode] = useState('preview');
  const [menu, setMenu] = useState(false);
  const [asking, setAsking] = useState(null);
  const [viewing, setViewing] = useState(null);
  const area = useRef(null);
  const picker = useRef(null);
  const pickAt = useRef(null);

  const load = (note) => {
    e.text = note.body;
    e.saved = note.body;
    if (!e.loaded) {
      e.loaded = true;
      if (isNew || !note.body) setMode('write');
    }
    touch();
  };

  const open = () => {
    openNote(noteId)
      .then((note) => {
        // Only while nothing's been typed: the text on screen is the user's.
        if (!e.left && e.text === e.saved) load(note);
      })
      .catch((err) => {
        app.toast(err.message, 'negative');
        if (err.code === 'not_found' && !e.left) app.back();
      });
  };

  const scheduleSave = () => {
    clearTimeout(e.timer);
    if (!e.loaded || e.text === e.saved) return;
    e.timer = setTimeout(save, 1000);
  };

  // Offline, it waits for the connection to come back (as "Edited").
  const save = () => {
    clearTimeout(e.timer);
    if (!e.loaded || e.deleted || e.left || e.saving || e.theirs || e.text === e.saved || app.state.status !== 'online') return;
    // Another save of it is out (what this note's last editor left): this one follows it.
    if (notes.saving.has(noteId)) {
      e.timer = setTimeout(save, 500);
      return;
    }
    const body = e.text;
    e.saving = true;
    touch();
    saveNote(noteId, body)
      .then(() => {
        e.saving = false;
        e.saved = body;
        if (e.text !== e.saved && !e.left) scheduleSave();
        touch();
      })
      .catch((err) => {
        e.saving = false;
        touch();
        if (err.code === 'conflict') {
          if (!e.left)
            openNote(noteId)
              .then((theirs) => {
                e.theirs = theirs;
                setAsking('conflict');
              })
              .catch((why) => app.toast(why.message, 'negative'));
        } else {
          // Said here; the next change, or the connection coming back, tries again.
          app.toast(err.message, 'negative');
        }
      });
  };

  useEffect(() => {
    const here = notes.open.get(noteId);
    if (here) load(here);
    if (app.state.status === 'online') open();
    const away = () => {
      if (document.visibilityState === 'hidden') save();
    };
    document.addEventListener('visibilitychange', away);
    return () => {
      document.removeEventListener('visibilitychange', away);
      clearTimeout(e.timer);
      if (e.deleted) return;
      e.left = true;
      if (isNew && !e.text.trim()) deleteNote(noteId, { quiet: true });
      // The notes keep it from here: this screen, and the text in it, are gone.
      else if (e.loaded && !e.theirs && e.text !== e.saved) saveNoteLeft(noteId, e.text);
    };
  }, []);

  // Back in touch: what couldn't be read or saved meanwhile goes now.
  const wasOnline = useRef(online);
  useEffect(() => {
    if (online && !wasOnline.current) {
      if (e.loaded) scheduleSave();
      else open();
    }
    wasOnline.current = online;
  }, [online]);

  useLayoutEffect(() => {
    if (e.caret != null && area.current) {
      area.current.setSelectionRange(e.caret, e.caret);
      e.caret = null;
    }
  });

  useEffect(() => {
    if (mode === 'write' && e.loaded) area.current?.focus();
  }, [mode, e.loaded]);

  const setText = (text, caret = null) => {
    e.text = text;
    e.caret = caret;
    touch();
    scheduleSave();
  };

  const typed = (ev) => {
    const el = ev.target;
    const value = el.value;
    const at = el.selectionStart;
    // Return after a list item carries the list on (or ends it, after an empty one).
    if (value.length === e.text.length + 1 && at > 0 && value[at - 1] === '\n' && value.slice(0, at - 1) + value.slice(at) === e.text) {
      const edit = continueList(value, at);
      if (edit) return setText(edit.text, edit.cursor);
    }
    setText(value);
  };

  const switchTo = (next) => {
    setMode(next);
    if (next === 'preview') save();
  };

  const tick = (line) => {
    const ticked = toggleCheck(e.text, line);
    if (ticked != null) setText(ticked);
  };

  const takeTheirs = () => {
    const theirs = e.theirs;
    e.theirs = null;
    e.saved = theirs.body;
    e.text = theirs.body;
    setAsking(null);
    touch();
  };

  const keepMine = () => {
    // The computer's version is the one opened now, so this save goes over it.
    e.theirs = null;
    setAsking(null);
    save();
  };

  const remove = () => {
    e.deleted = true;
    clearTimeout(e.timer);
    setAsking(null);
    deleteNote(noteId);
    app.back();
  };

  const pickPhoto = () => {
    const el = area.current;
    pickAt.current = el ? [el.selectionStart, el.selectionEnd] : [e.text.length, e.text.length];
    picker.current?.click();
  };

  // Sent to the computer to keep with the notes; the line that shows it goes where the cursor was.
  const addPhoto = async (ev) => {
    const file = ev.target.files?.[0];
    ev.target.value = '';
    if (!file) return;
    e.adding = true;
    touch();
    try {
      let photo;
      try {
        photo = await photoUpload(file);
      } catch {
        app.toast('That photo couldn’t be read', 'negative');
        return;
      }
      const reply = await app.ask({ type: 'add_note_image', note_id: noteId, data: photo.data }, { timeout: 60_000 });
      // What the phone just sent is drawn at once, without asking for it back.
      keepAsset(assetKey(noteId, reply.path, 'full'), URL.createObjectURL(photo.blob));
      const [from, to] = pickAt.current || [e.text.length, e.text.length];
      const edit = insertLines(e.text, from, to, reply.markdown);
      setText(edit.text, edit.cursor);
    } catch (err) {
      app.toast(err.message, 'negative');
    } finally {
      e.adding = false;
      touch();
    }
  };

  const modified = notes.open.get(noteId)?.modified;
  const status = e.adding ? 'Adding photo…' : e.saving ? 'Saving…' : e.text !== e.saved ? 'Edited' : modified ? `Saved · ${clock(modified)}` : '';
  const pictures = app.has('note_images');

  let content;
  if (!e.loaded) {
    content = html`<div class="scroll"><div class="empty">${online ? html`<${Spinner} />` : 'Nothing here until Trek answers.'}</div></div>`;
  } else if (mode === 'write') {
    content = html`<div class="note-write">
      <textarea
        ref=${area}
        class="note-text"
        value=${e.text}
        placeholder="Write something"
        aria-label="Note"
        autocapitalize="sentences"
        spellcheck="true"
        onInput=${typed}
      ></textarea>
    </div>`;
  } else {
    content = html`<div class="scroll">
      ${e.text.trim()
        ? html`<${NoteView} text=${e.text} noteId=${noteId} onTick=${tick} onView=${setViewing} />`
        : html`<button type="button" class="note-blank" onClick=${() => switchTo('write')}>Empty note. Tap to write.</button>`}
    </div>`;
  }

  return html`<div class="screen notes">
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <div class="bar-title"><h1>${e.loaded ? noteTitle(e.text) : 'Note'}</h1>${status && html`<div class="sub">${status}</div>`}</div>
      <${IconButton} icon="more" label="Note menu" disabled=${!e.loaded} onClick=${() => setMenu(true)} />
    </header>
    <${ConnectionBanner} />
    ${e.loaded &&
    html`<div class="note-tools">
      <${Segmented} label="Write or preview" options=${[['write', 'Write'], ['preview', 'Preview']]} value=${mode} onChange=${switchTo} />
      ${mode === 'write' && pictures && html`<button type="button" class="btn small ghost" disabled=${!online || e.adding} onClick=${pickPhoto}>${e.adding ? html`<${Spinner} />` : html`<${Icon} name="note-photo" size="s" />`}Add a photo</button>`}
      <input ref=${picker} type="file" accept="image/*" hidden onChange=${addPhoto} />
    </div>`}
    ${e.theirs && asking !== 'conflict' && html`<div class="banner warn"><${Icon} name="alert" /><span class="grow">This note changed on your computer since you opened it.</span><button class="btn small" type="button" onClick=${() => setAsking('conflict')}>Choose</button></div>`}
    ${content}
    ${menu &&
    html`<${Menu}
      title=${noteTitle(e.text)}
      onClose=${() => setMenu(false)}
      items=${[
        { label: 'Copy text', icon: 'copy', onClick: () => copy(e.text, app.toast) },
        { label: 'Delete note', icon: 'trash', danger: true, onClick: () => setAsking('delete') },
      ]}
    />`}
    ${asking === 'delete' &&
    html`<${Sheet} title="Delete this note?" onClose=${() => setAsking(null)}>
      <p class="notes-ask">Your computer keeps it in the notes folder’s Deleted.</p>
      <div class="row-btns notes-ask-btns">
        <button class="btn" type="button" onClick=${() => setAsking(null)}>Cancel</button>
        <button class="btn danger" type="button" onClick=${remove}>Delete</button>
      </div>
    <//>`}
    ${asking === 'conflict' && e.theirs && html`<${Conflict} theirs=${e.theirs} noteId=${noteId} keepMine=${keepMine} takeTheirs=${takeTheirs} onClose=${() => setAsking(null)} />`}
    ${viewing && html`<${Viewer} items=${viewing.items} start=${viewing.start} onClose=${() => setViewing(null)} />`}
  </div>`;
}

/** The note changed on the computer since it was opened here: keep this version (saved over that one) or take that one. */
function Conflict({ theirs, noteId, keepMine, takeTheirs, onClose }) {
  return html`<${Sheet} title="Changed on your computer" onClose=${onClose}>
    <p class="notes-ask">This note was edited on your computer after you opened it here. Here’s its version, ${clock(theirs.modified)}:</p>
    <div class="note-theirs"><${NoteView} text=${theirs.body} noteId=${noteId} /></div>
    <div class="notes-ask-btns stack">
      <button class="btn primary block" type="button" onClick=${keepMine}>Keep mine</button>
      <button class="btn block" type="button" onClick=${takeTheirs}>Use the computer’s version</button>
    </div>
    <p class="notes-foot">Keep mine saves this phone’s version over the computer’s.</p>
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// A note as it reads

/**
 * A note's markdown as `Markdown` draws it, with what a note has besides: pictures on lines of
 * their own (from the computer, a tap shows them full screen), files kept with the notes as cards,
 * and check boxes that tick (`onTick` with the line; without it they only show).
 */
function NoteView({ text, noteId, onTick, onView }) {
  const blocks = useMemo(() => noteBlocks(text), [text]);
  // The pictures the viewer steps through: the note's own, kept on the computer.
  const gallery = useMemo(
    () =>
      blocks
        .filter((b) => b.kind === 'images')
        .flatMap((b) => b.images)
        .map((image) => ({ noteId, path: attachment(image.link), alt: image.alt }))
        .filter((x) => x.path),
    [blocks, noteId],
  );
  const view = (path) => {
    const start = gallery.findIndex((x) => x.path === path);
    if (onView && start >= 0) onView({ items: gallery, start });
  };

  // A link to a file kept with the notes, in the middle of a line, opens the file, not a page.
  const click = (ev) => {
    const a = ev.target.closest?.('a[href]');
    const path = a && attachment(a.getAttribute('href'));
    if (!path) return;
    ev.preventDefault();
    if (!app.has('note_files')) return app.toast('That file is on your computer.');
    const name = a.textContent.trim() || baseName(path);
    fetchFile(path).then(
      (f) => (fileKind(path) === 'video' ? openFileUrl(f, name, 'other') : openFileUrl(f, name, fileKind(path))),
      (err) => app.toast(err.message, 'negative'),
    );
  };

  return html`<div class="note-body" onClick=${click}>
    ${blocks.map((b) => {
      switch (b.kind) {
        case 'images':
          return html`<div class="note-imgs" key=${`i${b.line}`}>${b.images.map((image, i) => html`<${NoteImage} key=${`${i}:${image.link}`} image=${image} noteId=${noteId} onOpen=${view} />`)}</div>`;
        case 'file':
          return html`<${NoteFileCard} key=${`f${b.line}:${b.file.path}`} file=${b.file} />`;
        case 'checks':
          return html`<ul class="note-checks" key=${`c${b.line}`}>
            ${b.items.map(
              (item) => html`<li key=${item.line} class=${item.done ? 'done' : ''} style=${item.indent ? { paddingLeft: `${item.indent * 18}px` } : null}>
                <button type="button" class="note-tick" aria-pressed=${item.done ? 'true' : 'false'} aria-label=${item.done ? 'Done' : 'Not done'} disabled=${!onTick} onClick=${() => onTick?.(item.line)}>
                  <${Icon} name=${item.done ? 'note-ticked' : 'note-box'} />
                </button>
                <${Markdown} text=${item.text || ' '} class="note-inline" />
              </li>`,
            )}
          </ul>`;
        default:
          return html`<${Markdown} key=${`m${b.line}`} text=${b.text} class="note-md" />`;
      }
    })}
  </div>`;
}

/** One of a note's pictures: from the computer when it's kept there; else a placeholder that says where it is. */
function NoteImage({ image, noteId, onOpen }) {
  const path = attachment(image.link);
  const web = /^https?:\/\//i.test(image.link) ? image.link : null;
  const served = app.has('note_images');
  const label = image.alt.trim() || baseName(image.link) || 'Picture';
  const [url, setUrl] = useState(() => (path && served ? cachedAsset(noteId, path) : null));
  const [broken, setBroken] = useState(false);
  useEffect(() => {
    if (!path || !served || url) return undefined;
    let alive = true;
    noteAsset(noteId, path, THUMB_PX).then(
      (u) => alive && setUrl(u),
      () => alive && setBroken(true),
    );
    return () => {
      alive = false;
    };
  }, [noteId, path, served]);
  if (url) return html`<button type="button" class="note-img" aria-label=${image.alt.trim() || 'Picture'} title="Show it full screen" onClick=${() => onOpen(path)}><img src=${url} alt=${image.alt} /></button>`;
  if (path && served && !broken) return html`<div class="note-ph" role="status" aria-label="Loading the picture"><${Spinner} /></div>`;
  if (broken) return html`<div class="note-ph"><${Icon} name="image" /><span>Couldn’t load ${label}</span></div>`;
  // The page draws only what comes from the computer; one on the web opens in the browser.
  if (web) return html`<a class="note-ph" href=${web} target="_blank" rel="noopener noreferrer"><${Icon} name="image" /><span>${label} · on the web</span></a>`;
  return html`<div class="note-ph"><${Icon} name="image" /><span>${label} · on your computer</span></div>`;
}

/** A note's pictures full screen, the one tapped first; a swipe or the arrows step through the rest. */
function Viewer({ items, start, onClose }) {
  const [at, setAt] = useState(start);
  const item = items[at];
  const [url, setUrl] = useState(() => cachedAsset(item.noteId, item.path));
  const swipe = useRef(null);
  const step = (by) => setAt((i) => Math.min(items.length - 1, Math.max(0, i + by)));
  useEffect(() => {
    let alive = true;
    setUrl(cachedAsset(item.noteId, item.path));
    noteAsset(item.noteId, item.path, FULL_PX).then(
      (u) => alive && setUrl(u),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [at]);
  useEffect(() => {
    const key = (ev) => {
      if (ev.key === 'Escape') onClose();
      else if (ev.key === 'ArrowLeft') step(-1);
      else if (ev.key === 'ArrowRight') step(1);
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, []);
  return html`<div
    class="note-viewer"
    role="dialog"
    aria-modal="true"
    aria-label=${item.alt || 'Picture'}
    onPointerDown=${(ev) => (swipe.current = ev.clientX)}
    onPointerUp=${(ev) => {
      const dx = swipe.current == null ? 0 : ev.clientX - swipe.current;
      swipe.current = null;
      if (Math.abs(dx) > 50) step(dx < 0 ? 1 : -1);
    }}
  >
    <div class="note-viewer-bar">
      <${IconButton} icon="x" label="Close" onClick=${onClose} />
      <span class="grow">${items.length > 1 ? `${at + 1} of ${items.length}` : ''}</span>
    </div>
    <div class="note-viewer-pic">${url ? html`<img src=${url} alt=${item.alt} draggable="false" />` : html`<${Spinner} />`}</div>
    ${items.length > 1 &&
    html`<div class="note-viewer-steps">
      <${IconButton} icon="back" label="Previous picture" disabled=${at === 0} onClick=${() => step(-1)} />
      <${IconButton} icon="forward" label="Next picture" disabled=${at === items.length - 1} onClick=${() => step(1)} />
    </div>`}
  </div>`;
}

/**
 * A file a note links on a line of its own, as a card: what it is and how big, and a tap that
 * fetches it from the computer a piece at a time (a ring says how far) and opens it: a PDF in a
 * new tab, a video in a player here, anything else saved to the phone. Once here it opens at once.
 * From a computer that doesn't send notes' files, the card just says it's there.
 */
function NoteFileCard({ file }) {
  const f = useFile(file.path);
  const served = app.has('note_files');
  const kind = fileKind(file.path);
  const name = file.name.trim() || baseName(file.path);
  const ext = (file.path.match(/\.([^./]+)$/)?.[1] || '').toUpperCase();
  const what = kind === 'pdf' ? 'PDF' : kind === 'video' ? 'Video' : ext || 'File';
  const [playing, setPlaying] = useState(false);
  const tapped = useRef(0);
  useEffect(() => {
    if (served) peekFile(file.path);
  }, [file.path, served]);

  const open = () => {
    if (kind === 'video') setPlaying(true);
    else openFileUrl(f, name, kind);
  };
  const tap = () => {
    if (f.url) return open();
    if (!served || f.state === 'loading' || f.state === 'gone') return;
    tapped.current = Date.now();
    fetchFile(file.path).then(
      // Opened by itself while the tap still counts; a slow one waits for another.
      () => (kind === 'video' || Date.now() - tapped.current < 4000 ? open() : null),
      () => {},
    );
  };

  const size = f.total != null ? bytes(f.total) : '';
  let caption;
  if (!served && !f.url) caption = 'On your computer';
  else if (f.state === 'gone') caption = f.error || 'Not on your computer any more';
  else if (f.state === 'failed') caption = 'Couldn’t load it. Tap to try again.';
  else if (f.state === 'loading') caption = [what, size, f.total ? `Downloading… ${Math.floor((100 * f.received) / f.total)}%` : 'Downloading…'].filter(Boolean).join(' · ');
  else if (f.url) caption = [what, size, kind === 'video' ? 'Tap to play' : kind === 'pdf' ? 'Tap to open' : 'Tap to save'].filter(Boolean).join(' · ');
  else caption = [what, size, 'Tap to download'].filter(Boolean).join(' · ');
  const fraction = f.state === 'loading' && f.total ? f.received / f.total : null;
  const off = (!served && !f.url) || f.state === 'gone';

  return html`<div class="note-file-wrap">
    <button type="button" class=${`note-file ${off ? 'off' : ''} ${f.state === 'failed' ? 'failed' : ''}`} disabled=${off} onClick=${tap} aria-label=${`${name}, ${caption}`}>
      <span class="note-file-icon"><${Icon} name=${kind === 'video' ? 'note-film' : 'file'} /></span>
      <span class="note-file-text"><span class="note-file-name">${name}</span><span class="note-file-cap">${caption}</span></span>
      ${fraction != null
        ? html`<span class="note-ring" style=${{ '--part': `${Math.max(3, Math.round(fraction * 100))}%` }} role="progressbar" aria-label="Downloading" aria-valuenow=${Math.round(fraction * 100)} aria-valuemin="0" aria-valuemax="100"></span>`
        : f.state === 'loading'
          ? html`<${Spinner} />`
          : !off && html`<${Icon} name=${f.url ? (kind === 'video' ? 'play' : kind === 'pdf' ? 'external' : 'note-save') : f.state === 'failed' ? 'refresh' : 'note-save'} size="s" />`}
    </button>
    ${f.url && kind !== 'other' && html`<a class="icon-btn note-file-save" href=${f.url} download=${name.includes('.') ? name : `${name}.${ext.toLowerCase()}`} aria-label=${`Save ${name}`} title="Save it"><${Icon} name="note-save" size="s" /></a>`}
    ${playing && f.url && html`<${Player} url=${f.url} name=${name} onClose=${() => setPlaying(false)} />`}
  </div>`;
}

/** A video from the notes, playing as its sheet opens. */
function Player({ url, name, onClose }) {
  const [broken, setBroken] = useState(false);
  return html`<${Sheet} title=${name} onClose=${onClose}>
    ${broken
      ? html`<p class="notes-ask">This browser can’t play it. Save it to play it in another app.</p>`
      : html`<video class="note-video" src=${url} controls autoplay playsinline onError=${() => setBroken(true)}></video>`}
    <div class="row-btns notes-ask-btns"><a class="btn small" href=${url} download=${name}><${Icon} name="note-save" size="s" />Save</a></div>
  <//>`;
}

// ---------------------------------------------------------------------------------------------
// Where it's opened from

registerScreen('notes', NotesScreen);
registerScreen('note', NoteRoute);
registerListMenuItems(() => [{ label: 'Notes', icon: 'notes', onClick: () => app.go({ screen: 'notes' }) }]);
