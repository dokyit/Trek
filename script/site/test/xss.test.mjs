// Eight ways text posts a script, through the web remote as it draws what comes from a computer:
// an answer, a note or a message as Markdown (`markdown.js`: marked, then DOMPurify), and a thread
// title or a name as plain text (`Plain`). In a real browser (Chrome or Edge, headless), under the
// remote's own Content-Security-Policy from `_headers`; skipped where there's none.
//
//   node script/site/test/run.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { extname, join, normalize } from 'node:path';
import { ROOT, headersFor, parseHeaders } from '../serve.mjs';

export const PAYLOADS = [
  '<img src=x onerror="__pwned.push(\'img\')">',
  "[click](javascript:__pwned.push('md-link'))",
  "[click](&#106;avascript&#58;__pwned.push('entity'))",
  '[click](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)',
  '<svg><a xlink:href="javascript:__pwned.push(\'svg\')"><text x="0" y="15">click</text></a></svg>',
  '<details open ontoggle="__pwned.push(\'details\')">x</details>',
  '<iframe src="javascript:__pwned.push(\'iframe\')"></iframe>',
  '<a href="#" onclick="__pwned.push(\'onclick\')">click</a>',
];

const browser = [
  process.env.TREK_TEST_BROWSER,
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
].find((p) => p && existsSync(p));

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml' };

// The page runs under the remote's policy, so it has no inline script: its script is a file.
const PAGE = '<!doctype html><meta charset="utf-8"><title>xss</title><body><pre id="result">pending</pre><script type="module" src="/remote/__xss.js"></script>';
const SCRIPT = `
import { toHtml, Plain } from './js/markdown.js';
import { html } from './js/ui.js';
import { render } from './vendor/preact.mjs';
window.__pwned = [];
const payloads = ${JSON.stringify(PAYLOADS)};
const out = { bad: [], titles: [] };
try {
  for (const p of payloads) {
    const box = document.createElement('div');
    box.className = 'md';
    box.innerHTML = toHtml(p);
    document.body.append(box);
    const title = document.createElement('div');
    title.className = 'title';
    render(html\`<\${Plain} text=\${p} />\`, title);
    document.body.append(title);
    out.titles.push(title.textContent === p && title.querySelectorAll('*').length === 1);
  }
  await new Promise((r) => setTimeout(r, 300));
  // Every link that isn't a web address, clicked (an <svg>'s <a> has no click(): an event then).
  for (const a of document.querySelectorAll('.md a')) {
    if (!/^https?:/i.test(a.getAttribute('href') || '')) a.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  }
  await new Promise((r) => setTimeout(r, 300));
  for (const el of document.querySelectorAll('.md *')) {
    if (/^(script|iframe|frame|object|embed|form)$/i.test(el.tagName)) out.bad.push(el.tagName);
    for (const attr of el.attributes) {
      const value = attr.value.replace(/[\\s\\u0000-\\u001f]/g, '').toLowerCase();
      if (/^on/i.test(attr.name)) out.bad.push(el.tagName + ' ' + attr.name);
      if (/(^|:)(href|src|action|formaction)$/i.test(attr.name) && /^(javascript|vbscript|data:text)/.test(value)) out.bad.push(el.tagName + ' ' + attr.name + '=' + value);
    }
  }
} catch (err) {
  out.error = String(err);
}
out.pwned = window.__pwned;
document.getElementById('result').textContent = btoa(JSON.stringify(out));
`;

function server() {
  const rules = parseHeaders(readFileSync(join(ROOT, '_headers'), 'utf8'));
  const policy = headersFor(rules, '/remote/');
  delete policy['Strict-Transport-Security'];
  if (policy['Content-Security-Policy']) policy['Content-Security-Policy'] = policy['Content-Security-Policy'].replace(/;?\s*upgrade-insecure-requests/, '');
  const srv = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const send = (type, body) => {
      res.writeHead(200, { ...policy, 'Content-Type': type });
      res.end(body);
    };
    if (path === '/remote/__xss.html') return send(TYPES['.html'], PAGE);
    if (path === '/remote/__xss.js') return send(TYPES['.js'], SCRIPT);
    const file = normalize(join(ROOT, path));
    if (!file.startsWith(ROOT) || !existsSync(file)) {
      res.writeHead(404).end();
      return;
    }
    send(TYPES[extname(file)] || 'application/octet-stream', readFileSync(file));
  });
  return new Promise((ok) => srv.listen(0, '127.0.0.1', () => ok(srv)));
}

function dumpDom(url) {
  const profile = mkdtempSync(join(tmpdir(), 'trek-xss-'));
  return new Promise((ok, fail) => {
    const proc = spawn(browser, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--user-data-dir=${profile}`, '--virtual-time-budget=10000', '--dump-dom', url], { stdio: ['ignore', 'pipe', 'ignore'] });
    let out = '';
    proc.stdout.on('data', (d) => (out += d));
    const timer = setTimeout(() => proc.kill(), 60_000);
    proc.on('close', () => {
      clearTimeout(timer);
      rmSync(profile, { recursive: true, force: true });
      ok(out);
    });
    proc.on('error', fail);
  });
}

test('script-shaped answers, notes and titles run nothing in the web remote', { skip: !browser && 'no Chrome or Edge here' }, async () => {
  const srv = await server();
  try {
    const dom = await dumpDom(`http://127.0.0.1:${srv.address().port}/remote/__xss.html`);
    const encoded = /<pre id="result">([^<]*)<\/pre>/.exec(dom)?.[1];
    assert.ok(encoded && encoded !== 'pending', `the page ran: ${dom.slice(0, 400)}`);
    const out = JSON.parse(Buffer.from(encoded, 'base64').toString('utf8'));
    assert.equal(out.error, undefined);
    assert.deepEqual(out.pwned, [], 'no payload ran');
    assert.deepEqual(out.bad, [], 'no handler, frame or script link is left in what is drawn');
    assert.deepEqual(out.titles, PAYLOADS.map(() => true), 'a title is its text, never markup');
  } finally {
    srv.close();
  }
});
