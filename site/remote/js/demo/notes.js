// The demo's notes, answered as Trek does (`docs/MOBILE.md` › Notes): a few notes, one with two
// pictures, a PDF and check boxes, others with a video, a CSV big enough to come in pieces, a
// highlight and a colour. The pictures are drawn on a canvas at the size asked for, the PDF and
// the CSV are written here, and the video is recorded from a canvas the first time it's asked
// for. Nothing leaves the page.

import { fromBase64, toBase64 } from '../link.js';

const min = 60_000;
const hour = 60 * min;
const day = 24 * hour;

const BOARD = 'attachments/5e0c3a9b71d24f6e8a1b2c3d4e5f6a7b.png';
const SKETCH = 'attachments/b7d2e4f6a8c0e1d3f5a7b9c1d3e5f7a9.png';
const PDF = 'attachments/9c1e2b7f4a3d5e6f8091a2b3c4d5e6f7.pdf';
const CSV = 'attachments/0d4c8e2a6b1f3e5d7c9a0b2d4f6e8a1c.csv';
const VIDEO = 'attachments/7a1b3c5d7e9f1a2b4c6d8e0f2a4b6c8d.webm';
/** The most one `note_file` piece carries (`trek_remote::MAX_NOTE_FILE_CHUNK`). */
const PIECE = 1 << 20;

function seed() {
  const now = Date.now();
  const note = (id, body, ago) => ({ id, body, modified: now - ago });
  return {
    notes: [
      note('n-sync', `# Sync flow\n\nFrom the whiteboard after standup:\n\n![Whiteboard](${BOARD})\n\n- [ ] Queue sends while offline\n- [x] Retry with backoff\n- [ ] Say what’s waiting in the banner\n\n![Offline banner](${SKETCH})\n\nThe write-up for Thursday’s review:\n\n[Sync flow.pdf](${PDF})`, 12 * min),
      note('n-release', '# 0.4.54 release\n\n- [x] Phone: notes and Git from the browser\n- [ ] Changelog\n- [ ] Tag and publish\n\nAsk **Mara** about the <mark>pricing page</mark> copy.', 35 * min),
      note('n-standup', `Standup\n\nThe board as it filled in, for whoever missed it:\n\n[Standup.webm](${VIDEO})\n\n1. Mara: the pricing page\n2. Dev: rate limiting for \`/login\`\n3. Sam: the flaky auth test`, 3 * hour),
      note('n-ideas', 'Ideas\n\n- Rate limit per account *and* per IP\n- Cache the dashboard query for 30 s\n- <span style="color: #ef4444">Drop</span> the Docker image\n- Key `trek-api`’s cache on the content hash, not the build id', 20 * hour),
      note('n-load', `## Load test\n\nTen thousand logins a minute against staging. p99 stayed under **180 ms**; every sample is in the CSV.\n\n[Latency samples.csv](${CSV})\n\n\`\`\`sh\nk6 run --vus 200 --duration 10m login.js\n\`\`\``, 2 * day),
      note('n-groceries', 'Groceries\n\n- [ ] Oat milk\n- [ ] Coffee beans\n- [x] Bread', 3 * day),
    ],
    /** Files kept with the notes, by path: `{ mime, bytes }`, or `make` for one written when first asked for. */
    files: new Map([
      [BOARD, { picture: drawBoard, width: 2000, height: 1250 }],
      [SKETCH, { picture: drawSketch, width: 1200, height: 1500 }],
      [PDF, { mime: 'application/pdf', make: () => pdf('Sync flow', ['Offline sends wait in a queue on the phone, oldest first.', 'Each retry waits twice as long as the last, up to a minute.', 'The banner says how many are waiting, and Retry sends them now.', '', 'Open: what a send that fails for good says, and where it goes.']) }],
      [CSV, { mime: 'text/csv', make: csv }],
      [VIDEO, { mime: 'video/webm', make: video }],
    ]),
    pictures: new Map(),
  };
}

const states = new WeakMap();
function of(demo) {
  let s = states.get(demo);
  if (!s) states.set(demo, (s = seed()));
  return s;
}

function error(code, message) {
  return Object.assign(new Error(message), { code });
}

// ---------------------------------------------------------------------------------------------
// Titles and previews, as the computer's list reads a note (`trek_core::notes::plain`)

function plain(line) {
  return line
    .replace(/!\[([^\]\n]*)\]\([^)\n]*\)/g, '$1')
    .replace(/\[([^\]\n]*)\]\((?:\.\/)?attachments\/[^)\n]*\)/g, '$1')
    .trim()
    .replace(/^#+\s*/, '')
    .replace(/^>\s?/, '')
    .replace(/^([-*+]\s+\[[ xX]\]\s+|[-*+]\s+|\d+\.\s+)/, '')
    .replace(/<[^>]*>/g, '')
    .replace(/[*_~`]/g, '')
    .trim();
}

function title(body) {
  const line = body.split('\n').map(plain).find(Boolean);
  return line ? Array.from(line).slice(0, 80).join('') : 'Untitled';
}

function whole(n) {
  return { id: n.id, title: title(n.body), body: n.body, modified: n.modified };
}

function summary(n) {
  const lines = n.body.split('\n').map(plain).filter(Boolean);
  return { id: n.id, title: title(n.body), preview: lines.slice(1, 4).join(' '), modified: n.modified };
}

function find(demo, id) {
  const n = of(demo).notes.find((x) => x.id === id);
  if (!n) throw error('not_found', 'No such note');
  return n;
}

/** Only `attachments/<name>` (or `./attachments/<name>`), as the computer serves them. */
function kept(demo, path) {
  const link = String(path || '').replace(/^\.\//, '');
  const name = link.startsWith('attachments/') ? link.slice('attachments/'.length) : '';
  if (!name || name.startsWith('.') || /[/\\:]/.test(name) || name.includes('..')) throw error('bad_request', 'Only files kept with the notes can be sent');
  const file = of(demo).files.get(`attachments/${name}`);
  if (!file) throw error('not_found', 'That file isn’t on this computer any more');
  return file;
}

// ---------------------------------------------------------------------------------------------
// The pictures: drawn on a canvas, as JPEGs fitting what's asked

function rand(seedValue) {
  let s = seedValue >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

/** A marker's stroke from `a` to `b`, a little unsteady, as far as `upTo` (0–1) of it. */
function stroke(g, points, r, upTo = 1) {
  const n = points.length - 1;
  const end = Math.max(0, Math.min(1, upTo)) * n;
  g.beginPath();
  g.moveTo(points[0][0] + (r() - 0.5) * 3, points[0][1] + (r() - 0.5) * 3);
  for (let i = 1; i <= Math.ceil(end); i++) {
    const t = Math.min(1, end - (i - 1));
    const [x0, y0] = points[i - 1];
    const [x1, y1] = points[i];
    const mx = x0 + (x1 - x0) * 0.5 + (r() - 0.5) * 6;
    const my = y0 + (y1 - y0) * 0.5 + (r() - 0.5) * 6;
    g.quadraticCurveTo(mx, my, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
  }
  g.stroke();
}

function box(g, x, y, w, h, r, upTo) {
  stroke(g, [[x, y], [x + w, y], [x + w, y + h], [x, y + h], [x, y + 2]], r, upTo);
}

function arrow(g, x0, y0, x1, y1, r, upTo = 1) {
  stroke(g, [[x0, y0], [x1, y1]], r, upTo);
  if (upTo < 1) return;
  const a = Math.atan2(y1 - y0, x1 - x0);
  for (const side of [-1, 1]) stroke(g, [[x1, y1], [x1 - 28 * Math.cos(a + side * 0.45), y1 - 28 * Math.sin(a + side * 0.45)]], r);
}

const HAND = '"Segoe Print", "Bradley Hand", "Comic Sans MS", "Chalkboard SE", cursive';

/** The whiteboard after standup, on a 2000 × 1250 board; `t` seconds into drawing it (the video). */
function drawBoard(g, t = Infinity) {
  const r = rand(7);
  const shown = (from) => Math.max(0, Math.min(1, (t - from) / 0.5));
  const sky = g.createLinearGradient(0, 0, 2000, 1250);
  sky.addColorStop(0, '#f7f6f1');
  sky.addColorStop(1, '#e9e7df');
  g.fillStyle = sky;
  g.fillRect(0, 0, 2000, 1250);
  g.lineCap = 'round';
  g.lineJoin = 'round';
  g.lineWidth = 9;
  g.strokeStyle = '#1f4fa8';
  g.fillStyle = '#1f4fa8';
  g.font = `600 64px ${HAND}`;
  const label = (text, x, y, from) => {
    if (t < from) return;
    g.globalAlpha = shown(from);
    g.fillText(text, x, y);
    g.globalAlpha = 1;
  };
  label('Sync, offline first', 120, 150, 0);
  box(g, 120, 420, 360, 240, r, shown(0.2));
  label('Phone', 195, 560, 0.4);
  box(g, 760, 420, 360, 240, r, shown(0.6));
  label('Queue', 835, 560, 0.8);
  box(g, 1520, 420, 360, 240, r, shown(1.0));
  label('Trek', 1625, 560, 1.2);
  arrow(g, 500, 540, 740, 540, r, shown(1.3));
  arrow(g, 1140, 540, 1500, 540, r, shown(1.5));
  g.font = `500 46px ${HAND}`;
  label('send', 560, 500, 1.4);
  label('when online', 1180, 500, 1.6);
  g.strokeStyle = '#c0392b';
  g.fillStyle = '#c0392b';
  g.lineWidth = 7;
  stroke(g, [[900, 680], [900, 860], [1200, 860], [1200, 770]], r, shown(1.8));
  if (t >= 2.2) arrow(g, 1200, 770, 1140, 690, r);
  label('retry: 1s, 2s, 4s… 60s', 720, 950, 2.0);
  g.strokeStyle = '#2e7d4f';
  g.fillStyle = '#2e7d4f';
  label('✓ banner says how many wait', 140, 1120, 2.2);
}

/** A phone sketched with the offline banner over its composer, on 1200 × 1500. */
function drawSketch(g) {
  const r = rand(11);
  g.fillStyle = '#fbfaf6';
  g.fillRect(0, 0, 1200, 1500);
  g.lineCap = 'round';
  g.lineJoin = 'round';
  g.strokeStyle = '#2b2b2b';
  g.lineWidth = 8;
  box(g, 300, 120, 600, 1260, r);
  stroke(g, [[540, 170], [660, 170]], r);
  g.lineWidth = 6;
  g.strokeStyle = '#c77a12';
  g.fillStyle = '#fdf0d8';
  g.fillRect(340, 300, 520, 150);
  box(g, 340, 300, 520, 150, r);
  g.fillStyle = '#7a4a07';
  g.font = `600 40px ${HAND}`;
  g.fillText('Offline · 2 waiting', 380, 365);
  g.font = `500 32px ${HAND}`;
  g.fillText('Retry now', 380, 415);
  g.strokeStyle = '#8a8a8a';
  g.lineWidth = 5;
  for (let i = 0; i < 6; i++) stroke(g, [[360, 540 + i * 95], [i % 2 ? 840 : 720, 540 + i * 95]], r);
  g.strokeStyle = '#2b2b2b';
  g.lineWidth = 6;
  box(g, 340, 1180, 520, 120, r);
  g.fillStyle = '#5b5b5b';
  g.font = `500 34px ${HAND}`;
  g.fillText('Reply…', 380, 1255);
}

async function picture(file, px, stored) {
  const fit = Math.min(1, px / Math.max(file.width, file.height));
  const w = Math.max(1, Math.round(file.width * fit));
  const h = Math.max(1, Math.round(file.height * fit));
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const g = canvas.getContext('2d');
  if (stored) {
    const bitmap = await createImageBitmap(new Blob([stored], { type: 'image/jpeg' }));
    g.drawImage(bitmap, 0, 0, w, h);
  } else {
    g.scale(w / file.width, h / file.height);
    file.picture(g);
  }
  return { data: canvas.toDataURL('image/jpeg', 0.86).split(',')[1], width: w, height: h };
}

// ---------------------------------------------------------------------------------------------
// The other files: a PDF and a CSV written here, a video recorded from a canvas

/** A one-page PDF in Helvetica (ASCII only). */
function pdf(heading, lines) {
  const esc = (s) => s.replace(/[\\()]/g, (c) => `\\${c}`);
  const text = [`BT /F1 26 Tf 72 760 Td (${esc(heading)}) Tj ET`, ...lines.map((l, i) => `BT /F1 13 Tf 72 ${712 - i * 22} Td (${esc(l)}) Tj ET`)].join('\n');
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    `<< /Length ${text.length} >>\nstream\n${text}\nendstream`,
  ];
  let out = '%PDF-1.4\n';
  const at = [];
  objects.forEach((body, i) => {
    at.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${at.map((o) => `${String(o).padStart(10, '0')} 00000 n \n`).join('')}`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}

/** Sixty thousand login timings: about 1.7 MB, two pieces. */
function csv() {
  const r = rand(42);
  const start = Date.UTC(2026, 10, 12, 14, 0, 0);
  const rows = ['time_ms,endpoint,status,latency_ms'];
  for (let i = 0; i < 60_000; i++) {
    const slow = r() < 0.01;
    const latency = Math.round(38 + r() * 60 + (slow ? 90 + r() * 60 : 0));
    rows.push(`${start + i * 10},/login,${r() < 0.004 ? 429 : 200},${latency}`);
  }
  return new TextEncoder().encode(`${rows.join('\n')}\n`);
}

/** The board filling in, as a short WebM recorded from a canvas. */
async function video() {
  const type = typeof MediaRecorder === 'function' && ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/webm'].find((t) => MediaRecorder.isTypeSupported(t));
  if (!type) throw error('host_error', 'The demo can’t make its video in this browser');
  const canvas = document.createElement('canvas');
  canvas.width = 800;
  canvas.height = 500;
  const g = canvas.getContext('2d');
  const stream = canvas.captureStream(25);
  const recorder = new MediaRecorder(stream, { mimeType: type, videoBitsPerSecond: 1_200_000 });
  const parts = [];
  recorder.ondataavailable = (e) => e.data.size && parts.push(e.data);
  const stopped = new Promise((resolve) => (recorder.onstop = resolve));
  const started = performance.now();
  recorder.start(500);
  await new Promise((resolve) => {
    const frame = () => {
      const t = (performance.now() - started) / 1000;
      g.setTransform(800 / 2000, 0, 0, 500 / 1250, 0, 0);
      drawBoard(g, t);
      if (t < 3.2) setTimeout(frame, 40);
      else resolve();
    };
    frame();
  });
  recorder.stop();
  await stopped;
  for (const track of stream.getTracks()) track.stop();
  return new Uint8Array(await new Blob(parts, { type: 'video/webm' }).arrayBuffer());
}

/** A file's bytes, written the first time they're asked for (once, however many ask at once). */
function bytesOf(file) {
  if (!file.bytes) {
    file.bytes = Promise.resolve()
      .then(file.make)
      .catch((err) => {
        file.bytes = null;
        throw err;
      });
  }
  return file.bytes;
}

// ---------------------------------------------------------------------------------------------
// The answers

function hex(n) {
  return Array.from({ length: n }, () => Math.floor(Math.random() * 16).toString(16)).join('');
}

export default {
  notes: (msg, demo) => ({ type: 'notes', notes: of(demo).notes.slice().sort((a, b) => b.modified - a.modified).map(summary) }),

  note: (msg, demo) => ({ type: 'note', note: whole(find(demo, msg.note_id)) }),

  create_note: (msg, demo) => {
    const n = { id: `n-${hex(8)}`, body: msg.body || '', modified: Date.now() };
    of(demo).notes.push(n);
    return { type: 'note', note: whole(n) };
  },

  save_note: (msg, demo) => {
    const n = find(demo, msg.note_id);
    // Edited on the computer since the phone read it: the phone's copy would undo that.
    if (msg.modified != null && msg.modified !== n.modified) throw error('conflict', 'This note changed on the computer since you opened it. Open it again to see the change.');
    if (n.body !== msg.body) {
      n.body = msg.body;
      n.modified = Math.max(Date.now(), n.modified + 1);
    }
    return { type: 'note', note: whole(n) };
  },

  delete_note: (msg, demo) => {
    const s = of(demo);
    find(demo, msg.note_id);
    s.notes = s.notes.filter((x) => x.id !== msg.note_id);
    return { type: 'ack' };
  },

  note_asset: async (msg, demo) => {
    find(demo, msg.note_id);
    const file = kept(demo, msg.path);
    if (!file.picture && !file.photo) throw error('bad_request', 'That file isn’t a picture');
    const px = Math.max(64, Math.min(2048, msg.max_px || 2048));
    const s = of(demo);
    const key = `${msg.path}@${px}`;
    if (!s.pictures.has(key)) s.pictures.set(key, picture(file, px, file.photo));
    const made = await s.pictures.get(key);
    return { type: 'note_asset', note_id: msg.note_id, path: msg.path, mime: 'image/jpeg', data: made.data, width: made.width, height: made.height };
  },

  add_note_image: async (msg, demo) => {
    find(demo, msg.note_id);
    let photo;
    try {
      photo = fromBase64(msg.data || '');
    } catch {
      throw error('bad_request', 'That photo couldn’t be read: it isn’t base64');
    }
    if (photo.length > 10 << 20) throw error('bad_request', 'That photo is too big (10 MB at most)');
    let size;
    try {
      const bitmap = await createImageBitmap(new Blob([photo], { type: 'image/jpeg' }));
      size = { width: bitmap.width, height: bitmap.height };
    } catch {
      throw error('bad_request', 'That photo couldn’t be read: it isn’t a JPEG or a PNG');
    }
    const path = `attachments/${hex(32)}.jpg`;
    of(demo).files.set(path, { photo, ...size });
    const alt = (msg.name || '').replace(/[[\]\\]/g, '').trim();
    return { type: 'note_image', note_id: msg.note_id, path, markdown: `![${alt}](${path})` };
  },

  note_file: async (msg, demo) => {
    const file = kept(demo, msg.path);
    const all = file.photo || (file.picture ? fromBase64((await picture(file, 2048)).data) : await bytesOf(file));
    const total = all.length;
    const offset = msg.offset || 0;
    if (offset > total) throw error('bad_request', `That file is only ${total} bytes long`);
    const length = Math.min(msg.length ?? PIECE, PIECE, total - offset);
    return { type: 'note_file_chunk', path: msg.path, offset, total, bytes_b64: toBase64(all.subarray(offset, offset + length)), mime: file.mime || 'image/jpeg' };
  },
};
