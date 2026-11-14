#!/usr/bin/env node
// The files the web remote's service worker keeps for opening without a connection
// (`site/remote/sw.js`, its SHELL list): every page, script, style and library the remote loads,
// so a phone that has opened it once can open it again with no network. The list is written here,
// from the folder, and a test (script/site/test/remote.test.mjs) fails while it's out of date:
//
//   node script/site/remote-shell.mjs          # rewrite the list in sw.js
//
// KaTeX's fonts are left out: math loads them as it needs them, and they're kept as they are.

import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const REMOTE = resolve(here, '../../site/remote');
export const SW = join(REMOTE, 'sw.js');

/** The site's own files the remote also uses, by their addresses. */
const ALSO = ['/favicon.svg', '/assets/fonts/JetBrainsMono-Regular.woff2'];

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

/** The addresses the list should hold, sorted, the page itself first. */
export function shell() {
  const files = walk(REMOTE)
    .map((p) => `/remote/${relative(REMOTE, p).split(sep).join('/')}`)
    .filter((url) => /\.(js|mjs|css|webmanifest)$/.test(url))
    .filter((url) => url !== '/remote/sw.js')
    .filter((url) => !url.includes('/fonts/'))
    .sort();
  return ['/remote/', ...files, ...ALSO];
}

const START = 'const SHELL = [';
const END = '];';

/** The list as sw.js has it now. */
export function listed(text = readFileSync(SW, 'utf8')) {
  const at = text.indexOf(START);
  const end = text.indexOf(END, at);
  return [...text.slice(at + START.length, end).matchAll(/'([^']+)'/g)].map((m) => m[1]);
}

export function write() {
  const text = readFileSync(SW, 'utf8');
  const at = text.indexOf(START);
  const end = text.indexOf(END, at);
  const list = shell()
    .map((url) => `  '${url}',`)
    .join('\n');
  writeFileSync(SW, `${text.slice(0, at + START.length)}\n${list}\n${text.slice(end)}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  write();
  console.log(`sw.js keeps ${shell().length} files`);
}
