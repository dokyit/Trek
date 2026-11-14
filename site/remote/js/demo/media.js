// The demo computer's answers for media: a message's photos (`attachment`), the files a thread
// names (`media`: an answer's picture, a message's video, the frames `watch_video` saw), the `/`
// picker's commands, files sent up a piece at a time (`upload_part`), and an added agent's icon
// (`icon`). Pictures are drawn here (OffscreenCanvas); the video is recorded from a canvas, once,
// the first time it's asked for. A photo sent with a message comes back as it was sent once the
// demo's `send` keeps it in `demo.images`.

const KB = 1024;
const PIECE = 1024 * KB;
/** Files sent up are kept whole up to this size, so a video sent plays back from "the computer". */
const KEEP_UPLOAD = 48 * 1024 * KB;

function fail(code, message) {
  return Object.assign(new Error(message), { code });
}

function base64(bytes) {
  if (typeof bytes.toBase64 === 'function') return bytes.toBase64();
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function bytesOf(b64) {
  if (!b64) return new Uint8Array(0);
  if (typeof Uint8Array.fromBase64 === 'function') return Uint8Array.fromBase64(b64);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

/** `w` × `h` fitting `px` a side (never larger). */
function fit(w, h, px) {
  const k = Math.min(1, (px || 2048) / Math.max(w, h));
  return [Math.max(1, Math.round(w * k)), Math.max(1, Math.round(h * k))];
}

/** A picture drawn at `w` × `h` by `draw(g, w, h)`, as a JPEG: `{ data, width, height, bytes }`. */
async function picture(w, h, draw) {
  const canvas = new OffscreenCanvas(w, h);
  const g = canvas.getContext('2d');
  draw(g, w, h);
  const blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: 0.84 });
  const bytes = new Uint8Array(await blob.arrayBuffer());
  return { data: base64(bytes), width: w, height: h, bytes };
}

function roundRect(g, x, y, w, h, r) {
  g.beginPath();
  g.roundRect(x, y, w, h, r);
  g.fill();
}

// ---------------------------------------------------------------------------------------------
// The pictures

/** A phone's screenshot of a CI run with the flaky test failing. */
function ciScreenshot(g, w, h) {
  const u = w / 390;
  g.fillStyle = '#0d1117';
  g.fillRect(0, 0, w, h);
  g.fillStyle = '#161b22';
  g.fillRect(0, 0, w, 112 * u);
  g.fillStyle = '#e6edf3';
  g.font = `600 ${17 * u}px system-ui, sans-serif`;
  g.fillText('CI · test (ubuntu-latest)', 20 * u, 64 * u);
  g.fillStyle = '#f85149';
  g.font = `600 ${13 * u}px system-ui, sans-serif`;
  g.fillText('✕ Failed in 4m 12s · run #482', 20 * u, 90 * u);
  const lines = [
    ['#7d8590', '   Compiling auth v0.4.1'],
    ['#7d8590', '    Finished test profile in 41.2s'],
    ['#7d8590', '     Running tests/session.rs'],
    ['#3fb950', 'test session::expires_after_ttl ... ok'],
    ['#3fb950', 'test session::rejects_bad_sig ... ok'],
    ['#f85149', 'test session::refresh_concurrent ... FAILED'],
    ['#3fb950', 'test session::logout_clears ... ok'],
    ['#e6edf3', ''],
    ['#e6edf3', 'failures:'],
    ['#e6edf3', '---- session::refresh_concurrent ----'],
    ['#f85149', "thread panicked at tests/session.rs:88:"],
    ['#f85149', 'assertion `left == right` failed'],
    ['#e6edf3', '  left: "tok_9f2c"'],
    ['#e6edf3', ' right: "tok_31aa"'],
    ['#e6edf3', ''],
    ['#f85149', 'test result: FAILED. 3 passed; 1 failed'],
  ];
  g.font = `${11.5 * u}px ui-monospace, Consolas, monospace`;
  lines.forEach(([color, text], i) => {
    g.fillStyle = color;
    g.fillText(text, 18 * u, (150 + i * 24) * u);
  });
  g.fillStyle = 'rgba(248, 81, 73, 0.12)';
  g.fillRect(0, (150 + 5 * 24 - 17) * u, w, 24 * u);
}

/** A chart of the test's last 30 runs, one in five red. */
function flakyChart(g, w, h) {
  const u = w / 400;
  g.fillStyle = '#fbf8f2';
  g.fillRect(0, 0, w, h);
  g.fillStyle = '#1a1a1c';
  g.font = `650 ${18 * u}px system-ui, sans-serif`;
  g.fillText('auth: the last 30 runs', 24 * u, 44 * u);
  g.fillStyle = '#6e6c68';
  g.font = `${12 * u}px system-ui, sans-serif`;
  g.fillText('Red: refresh_concurrent failed', 24 * u, 66 * u);
  const fails = new Set([3, 8, 9, 15, 21, 27]);
  for (let i = 0; i < 30; i++) {
    const bar = 60 + ((i * 37) % 70);
    g.fillStyle = fails.has(i) ? '#cc343a' : '#177d4e';
    roundRect(g, (24 + i * 11.8) * u, (h / u - 40 - bar) * u, 8 * u, bar * u, 2 * u);
  }
  g.fillStyle = '#e7e5e1';
  g.fillRect(24 * u, h - 38 * u, w - 48 * u, 1.5 * u);
}

/** The cache's hit rate over a day: it fell to nothing at each deploy, and stays up after the fix. */
function hitRate(g, w, h) {
  const u = w / 800;
  g.fillStyle = '#ffffff';
  g.fillRect(0, 0, w, h);
  g.fillStyle = '#1a1a1c';
  g.font = `650 ${24 * u}px system-ui, sans-serif`;
  g.fillText('Cache hit rate', 40 * u, 56 * u);
  g.fillStyle = '#6e6c68';
  g.font = `${15 * u}px system-ui, sans-serif`;
  g.fillText('Deploys at 10:00 and 14:00; keyed on the content hash from 12:00', 40 * u, 84 * u);
  const left = 40 * u;
  const right = w - 40 * u;
  const top = 120 * u;
  const bottom = h - 60 * u;
  g.strokeStyle = '#e7e5e1';
  g.lineWidth = 1.5 * u;
  for (let i = 0; i <= 4; i++) {
    const y = top + ((bottom - top) * i) / 4;
    g.beginPath();
    g.moveTo(left, y);
    g.lineTo(right, y);
    g.stroke();
  }
  const rate = (x) => (x < 0.25 ? 0.82 : x < 0.3 ? 0.05 + (x - 0.25) * 12 : x < 0.5 ? 0.78 : x < 0.75 ? 0.9 : x < 0.77 ? 0.86 : 0.93);
  g.strokeStyle = '#e85d1f';
  g.lineWidth = 4 * u;
  g.beginPath();
  for (let i = 0; i <= 200; i++) {
    const x = i / 200;
    const y = bottom - (bottom - top) * Math.min(0.97, rate(x) + Math.sin(i * 1.7) * 0.015);
    if (i) g.lineTo(left + (right - left) * x, y);
    else g.moveTo(left, y);
  }
  g.stroke();
  g.fillStyle = '#177d4e';
  g.font = `600 ${14 * u}px system-ui, sans-serif`;
  g.fillText('Fix deployed', left + (right - left) * 0.5 + 8 * u, top + 18 * u);
}

/** Dusk over a ridge, Trek's own kind of picture. */
function ridge(index) {
  return (g, w, h) => {
    const hue = (210 + index * 47) % 360;
    const sky = g.createLinearGradient(0, 0, 0, h);
    sky.addColorStop(0, `hsl(${hue} 55% 22%)`);
    sky.addColorStop(0.6, `hsl(${(hue + 40) % 360} 70% 62%)`);
    sky.addColorStop(1, `hsl(${(hue + 60) % 360} 80% 72%)`);
    g.fillStyle = sky;
    g.fillRect(0, 0, w, h);
    for (const [k, light] of [
      [0.62, 18],
      [0.74, 11],
    ]) {
      g.fillStyle = `hsl(${hue} 30% ${light}%)`;
      g.beginPath();
      g.moveTo(0, h);
      for (let x = 0; x <= w; x += w / 12) g.lineTo(x, h * k + Math.sin(x / w * 9 + k * 20) * h * 0.06 - Math.abs(Math.sin(x / w * 3)) * h * 0.1);
      g.lineTo(w, h);
      g.fill();
    }
  };
}

/** The Settings page in dark mode, as the agent screenshotted it. */
function settingsShot(g, w, h) {
  const u = w / 800;
  g.fillStyle = '#0a0a0b';
  g.fillRect(0, 0, w, h);
  g.fillStyle = '#111113';
  g.fillRect(0, 0, 200 * u, h);
  g.fillStyle = '#ededef';
  g.font = `650 ${22 * u}px system-ui, sans-serif`;
  g.fillText('Settings', 240 * u, 60 * u);
  ['General', 'Appearance', 'Notifications', 'Accounts'].forEach((label, i) => {
    g.fillStyle = i === 1 ? '#1c1c1f' : '#111113';
    roundRect(g, 14 * u, (36 + i * 44) * u, 172 * u, 36 * u, 8 * u);
    g.fillStyle = i === 1 ? '#ff7a3d' : '#8b8b94';
    g.font = `${15 * u}px system-ui, sans-serif`;
    g.fillText(label, 30 * u, (60 + i * 44) * u);
  });
  ['Theme', 'Accent colour', 'Reduce motion', 'Font size'].forEach((label, i) => {
    const y = (96 + i * 70) * u;
    g.fillStyle = '#161618';
    roundRect(g, 240 * u, y, 520 * u, 56 * u, 12 * u);
    g.fillStyle = '#e4e4e7';
    g.font = `${16 * u}px system-ui, sans-serif`;
    g.fillText(label, 260 * u, y + 34 * u);
    g.fillStyle = i === 2 ? '#232327' : '#ff7a3d';
    roundRect(g, 680 * u, y + 16 * u, 46 * u, 24 * u, 12 * u);
    g.fillStyle = '#fff';
    g.beginPath();
    g.arc((i === 2 ? 692 : 714) * u, y + 28 * u, 9 * u, 0, Math.PI * 2);
    g.fill();
  });
}

/** One frame of the clip: the page, the flash of the light Settings sliding by as it loads. */
function clipFrame(t) {
  return (g, w, h) => {
    const k = Math.min(1, t / 2.4);
    const flash = Math.max(0, 1 - Math.abs(k - 0.35) * 4);
    g.fillStyle = `rgb(${10 + flash * 235} ${10 + flash * 235} ${11 + flash * 233})`;
    g.fillRect(0, 0, w, h);
    const u = w / 640;
    g.fillStyle = flash > 0.5 ? '#1a1a1c' : '#ededef';
    g.font = `650 ${26 * u}px system-ui, sans-serif`;
    g.fillText('Settings', 40 * u, 70 * u);
    for (let i = 0; i < 4; i++) {
      g.fillStyle = flash > 0.5 ? '#f4f3f1' : '#161618';
      roundRect(g, 40 * u, (100 + i * 58) * u + (1 - k) * 30 * u, 560 * u, 46 * u, 10 * u);
    }
    g.fillStyle = '#ff7a3d';
    g.font = `600 ${15 * u}px ui-monospace, Consolas, monospace`;
    const tenths = Math.round(t * 10);
    g.fillText(`0:0${Math.floor(tenths / 10)}.${tenths % 10}`, w - 110 * u, h - 24 * u);
  };
}

/** A photo a message carries, fitting `px`: the one sent (once the demo keeps them), else a drawn one. */
async function photo(demo, threadId, itemId, index, px) {
  const sent = demo.images?.get(`${threadId}/${itemId}`)?.[index];
  if (sent) {
    const bitmap = await createImageBitmap(new Blob([bytesOf(sent.data)], { type: sent.mime }));
    const [w, h] = fit(bitmap.width, bitmap.height, px);
    const shot = await picture(w, h, (g) => g.drawImage(bitmap, 0, 0, w, h));
    bitmap.close();
    return shot;
  }
  const [W, H, draw] = index === 0 ? [1170, 2532, ciScreenshot] : index === 1 ? [1600, 1200, flakyChart] : [1600, 1067, ridge(index)];
  const [w, h] = fit(W, H, px);
  return picture(w, h, draw);
}

// ---------------------------------------------------------------------------------------------
// The video

let clip = null;

/** A short clip of the Settings page flashing light, recorded from a canvas: `{ bytes, mime }`. */
function recordClip() {
  if (clip) return clip;
  clip = (async () => {
    if (typeof MediaRecorder === 'undefined' || typeof document === 'undefined') throw fail('not_found', 'The demo can’t make a video in this browser');
    const canvas = document.createElement('canvas');
    canvas.width = 640;
    canvas.height = 400;
    const g = canvas.getContext('2d');
    const mime = ['video/mp4;codecs=avc1', 'video/webm;codecs=vp9', 'video/webm'].find((m) => MediaRecorder.isTypeSupported(m));
    if (!mime) throw fail('not_found', 'The demo can’t make a video in this browser');
    const stream = canvas.captureStream(30);
    const recorder = new MediaRecorder(stream, { mimeType: mime, videoBitsPerSecond: 900_000 });
    const parts = [];
    recorder.ondataavailable = (e) => e.data.size && parts.push(e.data);
    const stopped = new Promise((r) => (recorder.onstop = r));
    const began = performance.now();
    clipFrame(0)(g, canvas.width, canvas.height);
    recorder.start(250);
    await new Promise((resolve) => {
      const tick = setInterval(() => {
        const t = (performance.now() - began) / 1000;
        clipFrame(Math.min(t, 2.4))(g, canvas.width, canvas.height);
        if (t >= 2.4) {
          clearInterval(tick);
          resolve();
        }
      }, 33);
    });
    recorder.stop();
    await stopped;
    stream.getTracks().forEach((track) => track.stop());
    const blob = new Blob(parts, { type: mime.split(';')[0] });
    return { bytes: new Uint8Array(await blob.arrayBuffer()), mime: blob.type };
  })();
  clip.catch(() => (clip = null));
  return clip;
}

/** `bytes` from `offset`, a piece at most, as `media` sends a file as it is. */
function piece(msg, bytes, mime) {
  const offset = Math.max(0, msg.offset || 0);
  if (offset > bytes.length) throw fail('bad_request', `That piece starts past the end of the file (${bytes.length} bytes)`);
  const length = Math.min(msg.length || PIECE, PIECE);
  const part = bytes.subarray(offset, offset + length);
  return { type: 'media', thread_id: msg.thread_id, path: msg.path, mime, offset, total: bytes.length, bytes_b64: base64(part) };
}

// ---------------------------------------------------------------------------------------------
// Commands, as the computer lists them: Trek's own first

const COMMANDS = [
  ['new', 'Start a new thread in this project', 'command', true],
  ['usage', 'Show plan usage and reset times', 'command', true],
  ['context', 'Show how much of the context window is used', 'command', true],
  ['cost', 'Show this session’s estimated cost', 'command', true],
  ['model', 'Show the model this thread uses', 'command', true],
  ['permissions', 'Show or change how much the agent asks first', 'command', true],
  ['permissions supervised', 'Ask before every edit and command', 'command', true],
  ['permissions edits', 'Apply file edits; ask before commands', 'command', true],
  ['permissions auto', 'Work on its own; check before risky actions', 'command', true],
  ['permissions full', 'No prompts and no sandbox', 'command', true],
  ['consult', 'Ask other models first: /consult sol high, opus max: your message', 'command', true],
  ['consult arena', 'Have other models each design it, judged blind: /consult arena: your message', 'command', true],
  ['restate', 'Have the agent say back what you asked before it starts', 'command', true],
  ['compact', 'Clear the conversation but keep a summary in context', 'command', false],
  ['review', 'Review a pull request', 'command', false],
  ['init', 'Write an AGENTS.md for this project', 'command', false],
  ['frontend-design', 'Create distinctive, production-grade frontend interfaces', 'skill', false],
  ['verify', 'Run the project’s checks before saying it’s done', 'skill', false],
  ['code-reviewer', 'Reviews code for bugs, style and missed edge cases', 'agent', false],
].map(([name, description, kind, trek]) => ({ name, description, kind, ...(trek ? { trek } : {}) }));

// ---------------------------------------------------------------------------------------------
// Files sent up

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function stamp(at = new Date()) {
  const two = (n) => String(n).padStart(2, '0');
  return `${at.getFullYear()}-${two(at.getMonth() + 1)}-${two(at.getDate())} at ${two(at.getHours())}.${two(at.getMinutes())}.${two(at.getSeconds())}`;
}

const MIME = { mp4: 'video/mp4', m4v: 'video/mp4', mov: 'video/quicktime', webm: 'video/webm', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', pdf: 'application/pdf' };

function mimeOf(name) {
  return MIME[(name.split('.').pop() || '').toLowerCase()] || 'application/octet-stream';
}

// ---------------------------------------------------------------------------------------------
// The answers

export default {
  async attachment(msg, demo) {
    const item = demo.transcripts[msg.thread_id]?.find((i) => i.id === msg.item_id);
    const sent = demo.images?.get(`${msg.thread_id}/${msg.item_id}`);
    // Drawn ones for a message the demo doesn't know as having photos (a page testing it says so).
    const count = sent?.length ?? (item?.images || 8);
    if (!(msg.index >= 0 && msg.index < count)) throw fail('not_found', 'That message has no such image');
    await wait(150 + msg.index * 120);
    const shot = await photo(demo, msg.thread_id, msg.item_id, msg.index, Math.min(msg.max_px || 2048, 2048));
    return { type: 'attachment', thread_id: msg.thread_id, item_id: msg.item_id, index: msg.index, mime: 'image/jpeg', data: shot.data, width: shot.width, height: shot.height };
  },

  async media(msg, demo) {
    const path = msg.path || '';
    const sent = [...(demo.uploads?.values() || [])].find((u) => u.path === path);
    if (sent) {
      if (!sent.bytes) throw fail('not_found', 'The demo kept only its name: it was too big to keep');
      return piece(msg, sent.bytes, mimeOf(sent.name));
    }
    if (/\.(mp4|m4v|mov|webm)$/i.test(path)) {
      const made = await recordClip();
      return piece(msg, made.bytes, made.mime);
    }
    if (/\.(png|jpe?g|gif|webp)$/i.test(path)) {
      const frame = path.match(/f(\d+)\.jpg$/i);
      const [W, H, draw] = frame ? [640, 400, clipFrame(Number(frame[1]) * 0.6)] : /settings/i.test(path) ? [1600, 1000, settingsShot] : /hit|chart|rate/i.test(path) ? [1600, 1000, hitRate] : [1600, 1067, ridge(path.length)];
      const [w, h] = fit(W, H, msg.max_px || Math.max(W, H));
      await wait(200);
      const shot = await picture(w, h, draw);
      if (msg.max_px) return { type: 'media', thread_id: msg.thread_id, path, mime: 'image/jpeg', offset: 0, total: shot.bytes.length, bytes_b64: shot.data, width: w, height: h };
      return piece(msg, shot.bytes, 'image/jpeg');
    }
    throw fail('not_found', 'That file isn’t part of this thread');
  },

  commands(msg) {
    return { type: 'commands', thread_id: msg.thread_id, commands: COMMANDS };
  },

  async upload_part(msg, demo) {
    const id = String(msg.upload_id || '').toLowerCase();
    if (!UUID.test(id)) throw fail('bad_request', `“${msg.upload_id}” isn’t an upload id (a UUID)`);
    if (!(msg.total >= 0) || msg.total > 2 * 1024 * 1024 * KB) throw fail('bad_request', 'That file is too big to send (2 GB at most)');
    const uploads = demo.uploads || (demo.uploads = new Map());
    let u = uploads.get(id);
    if (msg.offset === 0) {
      u = { id, name: msg.name, total: msg.total, received: 0, parts: [] };
      uploads.set(id, u);
    } else if (!u) throw fail('bad_request', 'That upload isn’t on the computer (it may have restarted since): send the file again');
    if (msg.offset !== u.received) throw fail('bad_request', `That piece starts at byte ${msg.offset}, but the next one starts at byte ${u.received}`);
    const bytes = bytesOf(msg.data);
    if (u.received + bytes.length > u.total) throw fail('bad_request', 'That piece runs past the end of the file');
    u.received += bytes.length;
    if (u.total <= KEEP_UPLOAD) u.parts.push(bytes);
    if (u.received >= u.total) {
      u.done = true;
      u.path = `/Users/demo/Library/Application Support/dev.trek.Trek/uploads/iPhone ${stamp()} ${u.name}`;
      if (u.total <= KEEP_UPLOAD) {
        u.bytes = new Uint8Array(u.total);
        let at = 0;
        for (const p of u.parts) {
          u.bytes.set(p, at);
          at += p.length;
        }
      }
      u.parts = [];
    }
    // A piece takes a moment on Wi-Fi; the strip shows how far it got.
    await wait(250);
    return { type: 'upload_progress', upload_id: id, received: u.received, total: u.total, ...(u.done ? { done: true } : {}) };
  },

  async icon(msg) {
    // An agent added on the computer, its registry icon a shape: wind, for Mistral Vibe.
    const canvas = new OffscreenCanvas(96, 96);
    const g = canvas.getContext('2d');
    g.strokeStyle = '#000';
    g.lineWidth = 9;
    g.lineCap = 'round';
    for (const [y, end, curl] of [
      [30, 62, 14],
      [50, 78, 0],
      [70, 54, -12],
    ]) {
      g.beginPath();
      g.moveTo(12, y);
      g.lineTo(end, y);
      if (curl) g.arc(end, y - curl / 2, Math.abs(curl) / 2, Math.PI / 2, -Math.PI / 2, curl < 0);
      g.stroke();
    }
    const blob = await canvas.convertToBlob({ type: 'image/png' });
    const png = base64(new Uint8Array(await blob.arrayBuffer()));
    return { type: 'icon', ...(msg.agent ? { agent: msg.agent } : { project_id: msg.project_id }), hash: 'demo-vibe', png, template: true };
  },
};
