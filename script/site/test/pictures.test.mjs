// No picture on the site is stretched. An <img> with `width` and `height` attributes is drawn at
// the attribute's height unless a stylesheet says `height: auto`; with only its width styled, the
// picture is stretched (the home page's phone captures were, by 30% on a desktop and 66% on a phone,
// November 2026).
//
// Three checks, the first two in plain Node:
//   1. every <img> and <source> in every page has `width` and `height` attributes in the proportions
//      of its own file, to half a per cent;
//   2. the stylesheets keep `img { height: auto }` and never give an image a height of their own
//      without saying how it should fit;
//   3. in a browser, every image on every page is drawn within half a per cent of its own
//      proportions at 390, 768, 1024, 1440 and 1700 px wide. Headless Chrome is used where it is
//      installed (as script/site/make_og.mjs uses it; nothing appears on screen), and WebKit too
//      when SITE_TEST_PLAYWRIGHT names a folder whose node_modules holds playwright-webkit.
//      SITE_TEST_NO_BROWSER=1 leaves this check out; so does a machine with neither browser.
//
//   node script/site/test/run.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { ROOT, serve } from '../serve.mjs';

const TOLERANCE = 0.005;
const WIDTHS = [390, 768, 1024, 1440, 1700];

/** Every page of the site, as a path from its root. */
function pages(dir = ROOT) {
  const out = [];
  for (const name of readdirSync(dir).sort()) {
    if (name.startsWith('.')) continue;
    const file = join(dir, name);
    if (statSync(file).isDirectory()) out.push(...pages(file));
    else if (name.endsWith('.html')) out.push(relative(ROOT, file).split(sep).join('/'));
  }
  return out;
}

/** A picture file's own size in pixels: WebP, PNG, JPEG or SVG. */
export function naturalSize(file) {
  const b = readFileSync(file);
  const ascii = (at, n) => b.toString('latin1', at, at + n);
  if (ascii(0, 4) === 'RIFF' && ascii(8, 4) === 'WEBP') {
    const kind = ascii(12, 4);
    if (kind === 'VP8X') return { w: 1 + b.readUIntLE(24, 3), h: 1 + b.readUIntLE(27, 3) };
    if (kind === 'VP8L') {
      const n = b.readUInt32LE(21);
      return { w: 1 + (n & 0x3fff), h: 1 + ((n >> 14) & 0x3fff) };
    }
    if (kind === 'VP8 ') return { w: b.readUInt16LE(26) & 0x3fff, h: b.readUInt16LE(28) & 0x3fff };
  }
  if (b.readUInt32BE(0) === 0x89504e47) return { w: b.readUInt32BE(16), h: b.readUInt32BE(20) };
  if (b[0] === 0xff && b[1] === 0xd8) {
    for (let at = 2; at < b.length - 9; ) {
      const marker = b[at + 1];
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) return { w: b.readUInt16BE(at + 7), h: b.readUInt16BE(at + 5) };
      at += 2 + b.readUInt16BE(at + 2);
    }
  }
  const svg = /<svg\b[^>]*>/.exec(b.toString('utf8', 0, 2000));
  if (svg) {
    const box = /viewBox="([^"]+)"/.exec(svg[0]);
    const [, , w, h] = box ? box[1].trim().split(/[\s,]+/).map(Number) : [];
    if (w && h) return { w, h };
  }
  throw new Error(`can't read the size of ${file}`);
}

const attr = (tag, name) => (new RegExp(`\\s${name}="([^"]*)"`).exec(tag) || [])[1];
const fileOf = (address) => join(ROOT, address.split(/[?#]/)[0].split(',')[0].trim().split(/\s+/)[0]);
const off = (drawn, own) => Math.abs(drawn / own - 1);

test('every picture in a page has width and height attributes in its own proportions', () => {
  let seen = 0;
  for (const page of pages()) {
    const html = readFileSync(join(ROOT, page), 'utf8').replace(/<!--[\s\S]*?-->/g, '');
    // Each <img>, with the <source>s of the <picture> it sits in.
    for (const m of html.matchAll(/<picture\b[^>]*>([\s\S]*?)<\/picture>|<img\b[^>]*>/g)) {
      const img = /<img\b[^>]*>/.exec(m[0])?.[0];
      assert.ok(img, `${page}: a <picture> without an <img>`);
      const src = attr(img, 'src');
      const w = Number(attr(img, 'width')), h = Number(attr(img, 'height'));
      assert.ok(w > 0 && h > 0, `${page}: <img src="${src}"> needs width and height attributes, to hold its place while it loads`);
      const files = [src, ...[...m[0].matchAll(/<source\b[^>]*>/g)].map((s) => attr(s[0], 'srcset'))];
      for (const address of files) {
        const own = naturalSize(fileOf(address));
        assert.ok(off(h / w, own.h / own.w) <= TOLERANCE,
          `${page}: ${address} is ${own.w} by ${own.h}, but its <img> says ${w} by ${h}: ${(off(h / w, own.h / own.w) * 100).toFixed(1)}% out of proportion`);
        seen++;
      }
    }
  }
  assert.ok(seen >= 4, 'the home page has four phone captures (two pictures, Paper and Night)');
});

test('the stylesheets keep a picture in its own proportions', () => {
  const dir = join(ROOT, 'assets/home');
  const sheets = readdirSync(dir).filter((f) => f.endsWith('.css'));
  const home = readFileSync(join(dir, 'home.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  assert.match(home, /(^|\})\s*img\s*\{[^}]*\bheight:\s*auto\b/,'home.css: img { height: auto } is what keeps a picture from being stretched');
  for (const sheet of sheets) {
    const css = readFileSync(join(dir, sheet), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (!selector.split(',').some((s) => /(^|[\s>+~])img([.:#[][^\s>+~]*)?\s*$/.test(s.trim()))) continue;
      const height = /(?:^|;)\s*height:\s*([^;]+)/.exec(body)?.[1].trim();
      if (!height || height === 'auto') continue;
      assert.match(body, /object-fit:\s*(cover|contain|scale-down|none)/, `${sheet}: "${selector.trim()}" gives an image a height (${height}) without object-fit: it would be stretched`);
    }
  }
});

/* ---------- In a browser ---------- */

// Run in the page: every image, loaded, and how far its drawn box is from its own proportions.
const MEASURE = `(async () => {
  const imgs = [...document.querySelectorAll('img')];
  for (const im of imgs) im.loading = 'eager';
  await Promise.race([Promise.all(imgs.map((im) => im.decode().catch(() => 0))), new Promise((r) => setTimeout(r, 8000))]);
  const out = [];
  for (const im of imgs) {
    if (!im.naturalWidth || !im.naturalHeight) { out.push({ src: im.currentSrc || im.src, unloaded: true }); continue; }
    const cs = getComputedStyle(im), n = (v) => parseFloat(v) || 0;
    if (cs.display === 'none') continue;
    const border = cs.boxSizing === 'border-box';
    const w = n(cs.width) - (border ? n(cs.paddingLeft) + n(cs.paddingRight) + n(cs.borderLeftWidth) + n(cs.borderRightWidth) : 0);
    const h = n(cs.height) - (border ? n(cs.paddingTop) + n(cs.paddingBottom) + n(cs.borderTopWidth) + n(cs.borderBottomWidth) : 0);
    out.push({ src: (im.currentSrc || im.src).replace(location.origin, ''), w, h, nw: im.naturalWidth, nh: im.naturalHeight, fit: cs.objectFit });
  }
  return JSON.stringify(out);
})()`;

/** The pictures that are stretched, as sentences, and how many were measured. */
function stretched(list, where) {
  const bad = [];
  let measured = 0;
  for (const im of list) {
    if (im.unloaded) { bad.push(`${where}: ${im.src} did not load`); continue; }
    if (!(im.w > 0 && im.h > 0)) continue;
    measured++;
    if (im.fit && im.fit !== 'fill') continue;   // cover, contain and the like crop or letterbox: never a stretch
    // Half a per cent, or half a pixel on a small picture.
    const want = im.w * (im.nh / im.nw);
    if (Math.abs(im.h - want) > Math.max(0.5, want * TOLERANCE)) bad.push(`${where}: ${im.src} is drawn ${im.w.toFixed(1)} by ${im.h.toFixed(1)}; its own proportions (${im.nw} by ${im.nh}) make that ${im.w.toFixed(1)} by ${want.toFixed(1)}, ${(off(im.h, want) * 100).toFixed(1)}% out`);
  }
  return { bad, measured };
}

function findChrome() {
  const named = [process.env.CHROME_PATH, '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/Applications/Chromium.app/Contents/MacOS/Chromium',
    join(process.env.PROGRAMFILES || 'C:\\Program Files', 'Google\\Chrome\\Application\\chrome.exe'),
    join(process.env['PROGRAMFILES(X86)'] || 'C:\\Program Files (x86)', 'Google\\Chrome\\Application\\chrome.exe')];
  for (const dir of (process.env.PATH || '').split(delimiter)) for (const name of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']) named.push(join(dir, name));
  return named.find((p) => p && existsSync(p)) || null;
}

/** Headless Chrome over its DevTools socket: { open(url, width, scheme) -> measurements, close() }. */
async function chrome(path) {
  const profile = mkdtempSync(join(tmpdir(), 'trek-site-test-'));
  const child = spawn(path, ['--headless=new', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--disable-gpu',
    '--no-sandbox', '--hide-scrollbars', '--remote-debugging-port=0', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  const close = () => {
    child.kill();
    setTimeout(() => { try { rmSync(profile, { recursive: true, force: true, maxRetries: 3 }); } catch { /* the system clears its temp folder */ } }, 300).unref();
  };
  try {
    const address = await new Promise((ok, fail) => {
      let err = '';
      const timer = setTimeout(() => fail(new Error('Chrome did not start in 30 s')), 30000);
      child.stderr.on('data', (d) => {
        err += d;
        const m = /DevTools listening on (ws:\/\/\S+)/.exec(err);
        if (m) { clearTimeout(timer); ok(m[1]); }
      });
      child.on('error', fail);
      child.on('exit', () => fail(new Error('Chrome left before it could be reached')));
    });
    const ws = new WebSocket(address);
    await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = () => fail(new Error("Chrome's DevTools socket did not open")); });
    let id = 0;
    const waiting = new Map();
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (!m.id || !waiting.has(m.id)) return;
      const [ok, fail] = waiting.get(m.id);
      waiting.delete(m.id);
      if (m.error) fail(new Error(m.error.message));
      else ok(m.result);
    };
    const send = (method, params = {}, sessionId) => Promise.race([
      new Promise((ok, fail) => { const i = ++id; waiting.set(i, [ok, fail]); ws.send(JSON.stringify({ id: i, method, params, sessionId })); }),
      new Promise((_, fail) => setTimeout(() => fail(new Error(`${method} took over 30 s`)), 30000).unref()),
    ]);
    const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
    const run = async (expression) => (await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId)).result.value;
    return {
      name: 'Chrome',
      async open(url, width, scheme) {
        await send('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: width < 700 }, sessionId);
        await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: scheme }, { name: 'prefers-reduced-motion', value: 'reduce' }] }, sessionId);
        await send('Page.navigate', { url }, sessionId);
        // The page asked for, loaded: asked of the page itself, so an earlier page's load can't answer.
        const there = `location.href.split('#')[0] === ${JSON.stringify(url)} && document.readyState === 'complete'`;
        for (let i = 0; !(await run(there).catch(() => false)); i++) {
          if (i > 400) throw new Error(`${url} did not load in 20 s`);
          await new Promise((r) => setTimeout(r, 50));
        }
        return JSON.parse(await run(MEASURE));
      },
      close() { try { ws.close(); } catch { /* already gone */ } close(); },
    };
  } catch (e) {
    close();
    throw e;
  }
}

/** WebKit through Playwright, when SITE_TEST_PLAYWRIGHT says where it is installed. */
async function webkit(from) {
  const require = createRequire(join(from, 'package.json'));
  const pw = await import(pathToFileURL(require.resolve('playwright-webkit')).href);
  const browser = await (pw.webkit || pw.default.webkit).launch({ headless: true });
  return {
    name: 'WebKit',
    async open(url, width, scheme) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, isMobile: width < 700, colorScheme: scheme, reducedMotion: 'reduce' });
      try {
        const page = await context.newPage();
        await page.goto(url, { waitUntil: 'load' });
        return JSON.parse(await page.evaluate(MEASURE));
      } finally {
        await context.close();
      }
    },
    close: () => browser.close(),
  };
}

test('in a browser, no image on any page is drawn out of its own proportions', { timeout: 300000 }, async (t) => {
  if (process.env.SITE_TEST_NO_BROWSER) return t.skip('SITE_TEST_NO_BROWSER is set');
  const browsers = [];
  const path = findChrome();
  if (path && typeof WebSocket === 'function') {
    try { browsers.push(await chrome(path)); } catch (e) { t.diagnostic(`Chrome could not be used: ${e.message}`); }
  }
  if (process.env.SITE_TEST_PLAYWRIGHT) browsers.push(await webkit(process.env.SITE_TEST_PLAYWRIGHT));
  if (!browsers.length) return t.skip('no browser to draw the pages in (Chrome, or WebKit by SITE_TEST_PLAYWRIGHT)');
  const server = await serve(0, ROOT);
  const base = `http://127.0.0.1:${server.address().port}/`;
  try {
    for (const browser of browsers) {
      const bad = [];
      let home = 0;
      for (const page of pages()) {
        const address = page.replace(/index\.html$/, '');
        // Night as well where a page has pictures of its own for it.
        const schemes = /<source\b[^>]*prefers-color-scheme/.test(readFileSync(join(ROOT, page), 'utf8')) ? ['light', 'dark'] : ['light'];
        for (const width of WIDTHS) for (const scheme of schemes) {
          const got = stretched(await browser.open(base + address, width, scheme), `${browser.name}, /${address} at ${width} px, ${scheme}`);
          bad.push(...got.bad);
          if (page === 'index.html') home += got.measured;
        }
      }
      assert.deepEqual(bad, [], `${browser.name} draws ${bad.length} picture(s) out of proportion`);
      assert.equal(home, 2 * WIDTHS.length * 2, `${browser.name}: the home page's two phone captures were measured at every width, Paper and Night`);
      t.diagnostic(`${browser.name}: every picture in proportion at ${WIDTHS.join(', ')} px`);
    }
  } finally {
    for (const browser of browsers) await browser.close();
    server.close();
  }
});
