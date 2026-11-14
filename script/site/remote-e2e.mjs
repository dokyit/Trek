#!/usr/bin/env node
// The web remote end to end, on this machine: Trek's capture build (`cargo build -p trek-app
// --features shots`, isolated: its own data folder, the scripted agent, a phone server on the
// loopback at a port of its own) offers a pairing link for a phone's browser (`pair-web`); headless
// Chrome or Edge opens it from site/ as served here (`serve.mjs`), pairs over WebTransport with the
// certificate pinned from the link, lists the threads, allows a command the agent asks to run, and
// sends a follow-up the agent answers. Screenshots of the page and of Trek go in the work folder,
// the scripted agent dressed as Claude Code (`dress`: its names alone), so they read like a real one.
//
//   node script/site/remote-e2e.mjs [--trek <trek binary>] [--browser <chrome or edge>] [--work <folder>]
//
// It exits non-zero when a step didn't happen. Needs a display-less Chrome (`--headless=new`).

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync, openSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { serve } from './serve.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '../..');
const arg = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at > 0 ? process.argv[at + 1] : fallback;
};
const windows = process.platform === 'win32';
const trekBin = arg('trek', join(process.env.CARGO_TARGET_DIR || join(repo, 'target'), 'debug', windows ? 'trek.exe' : 'trek'));
const browserBin =
  arg('browser') ||
  [
    'C:/Program Files/Google/Chrome/Application/chrome.exe',
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    '/usr/bin/google-chrome',
  ].find((p) => existsSync(p));
const work = resolve(arg('work', join(tmpdir(), 'trek-remote-e2e')));
const SITE_PORT = 8739;
const PHONE_PORT = 7491;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const failures = [];
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures.push(what);
};

if (!existsSync(trekBin)) throw new Error(`no Trek at ${trekBin}: cargo build -p trek-app --features shots`);
if (!browserBin) throw new Error('no Chrome or Edge found: --browser <path>');

// ---------------------------------------------------------------------------------------------
// Trek, isolated

rmSync(work, { recursive: true, force: true });
const shot = join(work, 'shot');
const data = join(work, 'data');
const project = join(work, 'atlas-web');
for (const dir of [shot, data, join(project, 'src')]) mkdirSync(dir, { recursive: true });
writeFileSync(join(project, 'README.md'), '# atlas-web\n');
writeFileSync(join(project, 'src', 'main.rs'), 'fn main() {}\n');
writeFileSync(
  join(data, 'settings.toml'),
  ['[general]', 'default_agent = "direct:mock"', '', '[onboarding]', 'completed = true', 'version = 2', '', '[import]', 'claude_code = false', 'codex = false', 'opencode = false', '', '[notifications]', 'mode = "off"', '', '[updates]', 'auto_check = false', 'auto_download = false', '', '[mobile]', `port = ${PHONE_PORT}`, ''].join('\n'),
);
const site = await serve(SITE_PORT);
const trek = spawn(trekBin, [], {
  env: { ...process.env, TREK_SHOT_DIR: shot, TREK_DATA_DIR: data, TREK_FORCE_ACTIVE: '1', TREK_WEB_REMOTE_PAGE: `http://localhost:${SITE_PORT}/remote/` },
  stdio: ['ignore', openSync(join(work, 'trek.out.log'), 'w'), openSync(join(work, 'trek.err.log'), 'w')],
});

async function batch(lines, timeout = 120000) {
  rmSync(join(shot, 'done'), { force: true });
  writeFileSync(join(shot, 'cmd.tmp'), `${lines.join('\n')}\n`);
  renameSync(join(shot, 'cmd.tmp'), join(shot, 'cmd'));
  for (const start = Date.now(); Date.now() - start < timeout; await sleep(250)) {
    if (existsSync(join(shot, 'done'))) {
      await sleep(100);
      return readFileSync(join(shot, 'done'), 'utf8').trim();
    }
  }
  return 'timeout';
}

// ---------------------------------------------------------------------------------------------
// The browser, through the DevTools protocol

async function browser() {
  const port = 9300 + Math.floor(Math.random() * 600);
  const profile = join(work, 'browser');
  const proc = spawn(browserBin, ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--window-size=412,915', 'about:blank'], { stdio: 'ignore' });
  let ws;
  for (let i = 0; i < 75 && !ws; i++) {
    try {
      const page = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === 'page');
      if (page) ws = new WebSocket(page.webSocketDebuggerUrl);
    } catch {
      await sleep(200);
    }
  }
  await new Promise((r) => ws.addEventListener('open', r));
  let id = 0;
  const waiting = new Map();
  const errors = [];
  ws.addEventListener('message', (e) => {
    const msg = JSON.parse(e.data);
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg);
      waiting.delete(msg.id);
    } else if (msg.method === 'Runtime.exceptionThrown') {
      errors.push(msg.params.exceptionDetails.exception?.description || msg.params.exceptionDetails.text);
    } else if (msg.method === 'Log.entryAdded' && msg.params.entry.level === 'error') {
      errors.push(msg.params.entry.text);
    }
  });
  const send = (method, params = {}) =>
    new Promise((r) => {
      const n = ++id;
      waiting.set(n, r);
      ws.send(JSON.stringify({ id: n, method, params }));
    });
  for (const m of ['Runtime.enable', 'Log.enable', 'Page.enable']) await send(m);
  await send('Emulation.setDeviceMetricsOverride', { width: 412, height: 915, deviceScaleFactor: 2, mobile: true });
  const js = async (expression) => (await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })).result?.result?.value;
  const until = async (expression, timeout = 20000) => {
    for (const start = Date.now(); Date.now() - start < timeout; await sleep(200)) if (await js(expression)) return true;
    return false;
  };
  const picture = async (name) => writeFileSync(join(work, `${name}.png`), Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).result.data, 'base64'));
  const go = (url) => send('Page.navigate', { url });
  return { js, until, picture, go, errors, close: () => (ws.close(), proc.kill()) };
}

let page;
try {
  check((await batch(['wait 1500', 'pace 0', 'dress Claude Code|Sonnet 5.5|Opus 5.5', `project ${project}`, 'send Walk me through how the app starts', 'wait idle 40000', 'title Tour the startup code', `project ${project}`, 'send Apply the schema migration once I give permission mock:run ./migrate', 'wait permission 20000', 'title Migrate the schema'], 180000)) === 'ok', 'Trek made two threads, one asking to run a command');
  check((await batch(['pair-web', 'route settings:phone', 'wait 800', 'scroll settings-content 900', 'wait 500', 'shot settings-pair-web'])) === 'ok', 'Trek offered a pairing link for a browser');
  const link = readFileSync(join(shot, 'pair-web.txt'), 'utf8').trim();
  check(link.startsWith(`http://localhost:${SITE_PORT}/remote/#p=`) && link.length < 420, `the link opens the page served here, in ${link.length} characters`);

  page = await browser();
  await page.go(link);
  check(await page.until("!!document.querySelector('#pair-connect')"), 'the page asks to connect');
  check((await page.js('location.hash')) !== link.slice(link.indexOf('#')), 'the code is taken out of the address');
  await page.picture('web-pair');
  await page.js("document.querySelector('#pair-connect').click()");
  check(await page.until("document.querySelectorAll('.thread-row').length >= 2", 30000), 'paired over WebTransport: the threads are listed');
  await page.picture('web-threads');
  await page.js("[...document.querySelectorAll('.thread-row')].find((r) => r.innerText.includes('Migrate')).click()");
  check(await page.until("!!document.querySelector('.pinned-ask .card')"), 'the command waits over the composer');
  await page.picture('web-approval');
  await page.js("[...document.querySelectorAll('.pinned-ask button')].find((b) => b.innerText.trim() === 'Allow').click()");
  check(await page.until("!document.querySelector('.pinned-ask') && document.querySelector('.transcript').innerText.includes('Allowed')"), 'Allow is answered, and the card says so');
  check(await page.until("document.querySelector('.transcript').innerText.includes('Migrations applied')", 30000), 'the agent carries on');
  await page.js('history.back()');
  await sleep(800);
  await page.js("[...document.querySelectorAll('.thread-row')].find((r) => r.innerText.includes('Tour the startup')).click()");
  await page.until("document.querySelectorAll('.transcript > *').length > 1");
  await page.js("(() => { const t = document.querySelector('textarea'); t.value = 'Also list the entry points'; t.dispatchEvent(new Event('input', { bubbles: true })); })()");
  await sleep(200);
  await page.js("document.querySelector('button.send').click()");
  check(await page.until("document.querySelector('.transcript').innerText.includes('Also list the entry points')"), 'a follow-up from the page reaches the thread');
  check(await page.until("document.querySelectorAll('.transcript .turn-end').length >= 2", 40000), 'and the agent answers it');
  await page.picture('web-follow-up');
  check(page.errors.length === 0, `no errors in the page${page.errors.length ? `: ${page.errors.join(' | ')}` : ''}`);

  const devices = existsSync(join(data, 'mobile', 'devices.json')) ? JSON.parse(readFileSync(join(data, 'mobile', 'devices.json'), 'utf8')).devices : [];
  check(devices.some((d) => d.platform === 'web' && d.connected), 'Trek lists the browser as a connected web device');
  await batch(['route title:Tour the startup', 'wait 1200', 'shot trek-after']);
} finally {
  page?.close();
  await batch(['quit'], 15000);
  await sleep(1000);
  trek.kill();
  site.close();
}
console.log(failures.length ? `\n${failures.length} failed. Logs and pictures: ${work}` : `\nAll good. Pictures: ${work}`);
process.exit(failures.length ? 1 : 0);
