#!/usr/bin/env node
// Serve site/ on this machine as Cloudflare Pages serves it: the files as they are, with the
// headers `site/_headers` gives each path (its rules in order, a value set twice joined with a
// comma, `! Name` taking one away), so the security policy a page meets here is the one it meets
// at trekdesktop.com. For the web remote while it's worked on or checked:
//
//   node script/site/serve.mjs [port]           # http://localhost:8080/remote/
//   TREK_WEB_REMOTE_PAGE=http://localhost:8080/remote/ <Trek>   # Trek lets that page in
//
// `headersFor` is exported for the tests (script/site/test/).

import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { extname, join, normalize, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const ROOT = resolve(here, '../../site');

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.webmanifest': 'application/manifest+json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.webp': 'image/webp',
  '.jpg': 'image/jpeg',
  '.woff2': 'font/woff2',
  '.txt': 'text/plain; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
};

/** `_headers` as rules: `[{ pattern: RegExp, set: [[name, value]], detach: [name] }]`, in order. */
export function parseHeaders(text) {
  const rules = [];
  let rule = null;
  for (const raw of text.split(/\r?\n/)) {
    if (!raw.trim() || raw.trim().startsWith('#')) continue;
    if (!/^\s/.test(raw)) {
      const glob = raw.trim();
      const pattern = new RegExp(`^${glob.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`);
      rule = { glob, pattern, lines: [] };
      rules.push(rule);
      continue;
    }
    if (!rule) continue;
    const line = raw.trim();
    if (line.startsWith('!')) rule.lines.push({ detach: line.slice(1).trim().toLowerCase() });
    else {
      const colon = line.indexOf(':');
      if (colon > 0) rule.lines.push({ name: line.slice(0, colon).trim(), value: line.slice(colon + 1).trim() });
    }
  }
  return rules;
}

/** The headers a path is served with, as Cloudflare Pages works them out. */
export function headersFor(rules, path) {
  const out = new Map();
  for (const rule of rules) {
    if (!rule.pattern.test(path)) continue;
    for (const line of rule.lines) {
      if (line.detach) {
        out.delete(line.detach);
        continue;
      }
      const key = line.name.toLowerCase();
      const had = out.get(key);
      out.set(key, had ? { name: had.name, value: `${had.value}, ${line.value}` } : { name: line.name, value: line.value });
    }
  }
  return Object.fromEntries([...out.values()].map((h) => [h.name, h.value]));
}

export function serve(port = 8080, root = ROOT) {
  const rules = parseHeaders(readFileSync(join(root, '_headers'), 'utf8'));
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    let path = decodeURIComponent(url.pathname);
    let file = normalize(join(root, path));
    if (!file.startsWith(root)) {
      res.writeHead(403).end();
      return;
    }
    try {
      let info = await stat(file);
      if (info.isDirectory()) {
        if (!path.endsWith('/')) {
          res.writeHead(308, { Location: `${path}/${url.search}` }).end();
          return;
        }
        file = join(file, 'index.html');
        info = await stat(file);
      }
      const body = await readFile(file);
      // Over plain http on this computer, two of the live site's headers would break the page in
      // Safari and Trek's own Browser (WebKit sends the stylesheet's request to https://localhost,
      // where nothing answers; Chrome leaves localhost alone): they are left off here.
      const headers = headersFor(rules, path);
      delete headers['Strict-Transport-Security'];
      if (headers['Content-Security-Policy']) {
        headers['Content-Security-Policy'] = headers['Content-Security-Policy'].replace(/;?\s*upgrade-insecure-requests/, '');
      }
      const type = { 'Content-Type': TYPES[extname(file)] || 'application/octet-stream', 'Accept-Ranges': 'bytes', ...headers };
      // A part of the file, when that's what was asked for: Safari and Trek's Browser play a video
      // only from a server that answers ranges.
      const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
      if (range && (range[1] || range[2])) {
        const size = body.length;
        const start = range[1] ? Number(range[1]) : Math.max(size - Number(range[2]), 0);
        const end = range[1] && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
        if (start > end || start >= size) {
          res.writeHead(416, { 'Content-Range': `bytes */${size}` }).end();
          return;
        }
        res.writeHead(206, { ...type, 'Content-Range': `bytes ${start}-${end}/${size}`, 'Content-Length': end - start + 1 });
        res.end(body.subarray(start, end + 1));
        return;
      }
      res.writeHead(200, type);
      res.end(body);
    } catch {
      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' }).end('Not found');
    }
  });
  return new Promise((ok) => server.listen(port, '127.0.0.1', () => ok(server)));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const port = Number(process.argv[2] || process.env.PORT || 8080);
  serve(port).then(() => console.log(`site/ at http://localhost:${port}/ (the web remote: http://localhost:${port}/remote/)`));
}
