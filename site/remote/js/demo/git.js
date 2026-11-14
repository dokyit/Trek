// The demo's git, answered as Trek does (`docs/MOBILE.md` › Git, and `diff_viewed`): trek-api's
// project folder on main, the flaky-test thread's worktree (fix/auth-flake, from main, with
// work not committed yet), and web's folder, where a thread is at work so its branch stays put.
// The diffs are real ones; a long new file opens folded, a longer one comes cut short.

const MAX_DIFF = 256 << 10;

/** A file's diff as git prints it: its header, then each hunk (`[oldStart, lines, after@@]`) under its `@@` line. */
function unified(path, hunks, { from = null, created = false } = {}) {
  const out = [`diff --git a/${from || path} b/${path}`];
  if (from) out.push('similarity index 71%', `rename from ${from}`, `rename to ${path}`);
  if (created) out.push('new file mode 100644', 'index 0000000..5c2e9a1', '--- /dev/null');
  else out.push('index 2b7a9e1..c4d0f55 100644', `--- a/${from || path}`);
  out.push(`+++ b/${path}`);
  let shift = 0;
  for (const [start, lines, after] of hunks) {
    const before = lines.filter((l) => !l.startsWith('+')).length;
    const now = lines.filter((l) => !l.startsWith('-')).length;
    out.push(`@@ -${before ? start : start - 1},${before} +${now ? start + shift : 0},${now} @@${after ? ` ${after}` : ''}`, ...lines);
    shift += now - before;
  }
  return out.join('\n');
}

/** An untracked file, as the computer sends one from a project folder: every line `+`, no header. */
function untracked(lines) {
  return lines.map((l) => `+${l}`).join('\n');
}

/** Lines added and removed, as the status counts them. */
function counts(diff) {
  let added = 0;
  let removed = 0;
  let inHunk = !/^@@/m.test(diff);
  for (const l of diff.split('\n')) {
    if (l.startsWith('@@')) inHunk = true;
    else if (l.startsWith('diff --git')) inHunk = false;
    else if (inHunk && l.startsWith('+')) added += 1;
    else if (inHunk && l.startsWith('-')) removed += 1;
  }
  return { added, removed };
}

function file(path, status, diff, extra = {}) {
  return { path, status, diff, ...(diff ? counts(diff) : { added: 0, removed: 0 }), ...extra };
}

// ---------------------------------------------------------------------------------------------
// The files

const keyRs = unified('src/cache/key.rs', [
  [
    1,
    [
      ' use sha2::{Digest, Sha256};',
      ' ',
      ' /// The key a rendered page is cached under.',
      ' #[derive(Debug, Clone, PartialEq, Eq, Hash)]',
      ' pub struct CacheKey(String);',
      ' ',
      ' impl CacheKey {',
      '-    /// Keyed on the build, so every deploy starts with an empty cache.',
      '-    pub fn for_page(path: &str) -> Self {',
      '-        Self(format!("{}:{path}", crate::build::BUILD_ID))',
      '+    /// Keyed on what the page is made from, so a deploy that doesn\'t',
      '+    /// change a page keeps its cached copy warm.',
      '+    pub fn for_page(path: &str, manifest: &crate::assets::Manifest) -> Self {',
      '+        // Same inputs, same key, whichever build made them.',
      '+        let mut hash = Sha256::new();',
      '+        hash.update(path.as_bytes());',
      '+        for input in manifest.inputs_of(path) {',
      '+            hash.update(input.content_hash.as_bytes());',
      '+        }',
      '+        Self(format!("page:{:x}", hash.finalize()))',
      '     }',
      ' ',
      '     pub fn as_str(&self) -> &str {',
      '         &self.0',
      '     }',
      ' }',
      '+',
      '+// Two deploys of the same inputs share a key: see tests/cache_warm.rs.',
    ],
  ],
]);

const readme = unified('README.md', [
  [
    14,
    [
      ' cargo run -p trek-api',
      ' ```',
      ' ',
      '-The page cache is cold after every deploy: warm it with `scripts/warm.sh`.',
      '+Pages are cached under a hash of what they\'re made from, so a deploy that',
      '+doesn\'t change a page keeps its cached copy. `scripts/warm.sh` is only needed',
      '+after the cache is flushed.',
      ' ',
      ' ## Tests',
      ' ',
    ],
    'Run it locally:',
  ],
]);

const cachingMd = untracked([
  '# Caching',
  '',
  'Rendered pages are kept in Redis for an hour, under a key made from the page\'s',
  'path and a hash of every input it was built from (templates, data files, the',
  'asset manifest).',
  '',
  '## Why not the build id',
  '',
  'Keying on the build id threw the whole cache away on every deploy, even when a',
  'deploy changed one stylesheet. With content hashes, only the pages whose inputs',
  'changed are rendered again.',
  '',
  '## Flushing',
  '',
  '    redis-cli --scan --pattern \'page:*\' | xargs redis-cli unlink',
  '',
  'Only needed when a template\'s output changes without its inputs changing: a',
  'new version of the renderer, say.',
]);

const sessionRs = unified('src/auth/session.rs', [
  [3, [' use std::time::Duration;', ' ', '-use tokio::sync::RwLock;', '+use tokio::sync::{Mutex, RwLock};', ' ', ' use super::{AuthClient, Result, Token};'], ''],
  [12, [' pub struct Session {', '     token: RwLock<Token>,', '     client: AuthClient,', '+    /// Held while a refresh runs, so two requests never refresh at once.', '+    refreshing: Mutex<()>,', ' }', ' '], ''],
  [
    84,
    [
      '     /// A token that\'s good for at least another minute.',
      '     pub async fn token(&self) -> Result<Token> {',
      '-        let token = self.token.read().await.clone();',
      '-        if token.expires_in() > Duration::from_secs(60) {',
      '-            return Ok(token);',
      '+        {',
      '+            let token = self.token.read().await;',
      '+            if token.expires_in() > Duration::from_secs(60) {',
      '+                return Ok(token.clone());',
      '+            }',
      '         }',
      '-        self.refresh().await',
      '+        let _guard = self.refreshing.lock().await;',
      '+        // Another request may have refreshed it while this one waited.',
      '+        let token = self.token.read().await.clone();',
      '+        if token.expires_in() > Duration::from_secs(60) {',
      '+            return Ok(token);',
      '+        }',
      '+        self.refresh(&token).await',
      '     }',
      ' ',
      '-    async fn refresh(&self) -> Result<Token> {',
      '-        let fresh = self.client.refresh(&self.token.read().await.refresh).await?;',
      '+    async fn refresh(&self, stale: &Token) -> Result<Token> {',
      '+        let fresh = self.client.refresh(&stale.refresh).await?;',
      '         *self.token.write().await = fresh.clone();',
      '         Ok(fresh)',
      '     }',
    ],
    'impl Session {',
  ],
]);

const refreshTest = unified(
  'tests/auth_refresh.rs',
  [
    [
      1,
      [
        '//! Ten requests that all find the token expired refresh it once between them.',
        '',
        'use std::sync::atomic::{AtomicUsize, Ordering};',
        'use std::sync::Arc;',
        '',
        'use futures::future::join_all;',
        'use trek_api::auth::{Session, Token};',
        'use trek_api::testing::FakeAuthClient;',
        '',
        '#[tokio::test(flavor = "multi_thread", worker_threads = 8)]',
        'async fn concurrent_refreshes_share_one_token() {',
        '    let refreshes = Arc::new(AtomicUsize::new(0));',
        '    let client = FakeAuthClient::counting(refreshes.clone());',
        '    let session = Arc::new(Session::new(Token::expired(), client));',
        '',
        '    let tokens = join_all((0..10).map(|_| {',
        '        let session = session.clone();',
        '        tokio::spawn(async move { session.token().await })',
        '    }))',
        '    .await;',
        '',
        '    let tokens: Vec<Token> = tokens.into_iter().map(|t| t.unwrap().unwrap()).collect();',
        '    assert_eq!(refreshes.load(Ordering::SeqCst), 1, "one refresh for ten requests");',
        '    assert!(tokens.windows(2).all(|w| w[0] == w[1]), "every request got the same token");',
        '}',
        '',
        '#[tokio::test]',
        'async fn a_fresh_token_is_not_refreshed() {',
        '    let refreshes = Arc::new(AtomicUsize::new(0));',
        '    let client = FakeAuthClient::counting(refreshes.clone());',
        '    let session = Session::new(Token::valid_for(3600), client);',
        '',
        '    session.token().await.unwrap();',
        '    assert_eq!(refreshes.load(Ordering::SeqCst), 0);',
        '}',
      ].map((l) => `+${l}`),
    ],
  ],
  { created: true },
);

const cargoToml = unified('Cargo.toml', [[18, [' thiserror = "2"', ' tracing = "0.1"', '+', '+[dev-dependencies]', '+futures = "0.3"'], 'tokio = { version = "1", features = ["full"] }']]);

const packageJson = unified('package.json', [
  [
    6,
    [
      '   "scripts": {',
      '     "dev": "vite",',
      '     "build": "tsc -b && vite build",',
      '     "preview": "vite preview"',
      '   },',
      '   "dependencies": {',
      '     "react": "^19.1.0",',
      '     "react-dom": "^19.1.0"',
      '   },',
      '   "devDependencies": {',
      '-    "@vitejs/plugin-react": "^4.3.4",',
      '-    "typescript": "~5.6.2",',
      '-    "vite": "^6.0.5"',
      '+    "@vitejs/plugin-react": "^5.0.0",',
      '+    "typescript": "~5.8.3",',
      '+    "vite": "^7.0.0"',
      '   }',
      ' }',
    ],
  ],
]);

const viteConfig = unified('vite.config.ts', [
  [
    1,
    [
      " import { defineConfig } from 'vite';",
      " import react from '@vitejs/plugin-react';",
      "+import { proxy } from './src/dev/proxy';",
      ' ',
      ' export default defineConfig({',
      '   plugins: [react()],',
      '-  server: {',
      "-    proxy: require('./src/legacy/proxy.js'),",
      '-  },',
      '+  server: { port: 5173, proxy },',
      '   build: {',
      "-    target: 'es2020',",
      "+    // Vite 7's default, said out loud: browsers from the last two and a half years.",
      "+    target: 'baseline-widely-available',",
      '     sourcemap: true,',
      '   },',
      ' });',
    ],
  ],
]);

const proxyTs = unified(
  'src/dev/proxy.ts',
  [
    [
      1,
      [
        '-// The dev server\'s proxy: /api goes to the API on 8080.',
        '-module.exports = {',
        "+import type { ProxyOptions } from 'vite';",
        '+',
        '+// The dev server\'s proxy: /api goes to the API on 8080, websockets too.',
        '+export const proxy: Record<string, ProxyOptions> = {',
        "   '/api': {",
        "     target: 'http://localhost:8080',",
        '     changeOrigin: true,',
        '+    ws: true,',
        '   },',
        ' };',
      ],
    ],
  ],
  { from: 'src/legacy/proxy.js' },
);

function rand(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

/** The design tokens, generated as a design system's are: long enough to open folded. */
function tokensCss() {
  const hues = [['gray', 260, 0.012], ['orange', 50, 0.17], ['amber', 75, 0.16], ['green', 150, 0.15], ['teal', 185, 0.12], ['blue', 255, 0.17], ['indigo', 275, 0.16], ['violet', 295, 0.17], ['pink', 350, 0.16], ['red', 25, 0.19]];
  const steps = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];
  const parts = ['button', 'input', 'select', 'checkbox', 'switch', 'tabs', 'card', 'dialog', 'sheet', 'menu', 'toast', 'tooltip', 'badge', 'avatar', 'table', 'list'];
  const props = ['bg', 'bg-hover', 'bg-active', 'fg', 'border', 'border-focus', 'shadow', 'radius'];
  const out = ['/* Design tokens: colours, type and spacing, as the design system names them.', '   Generated from tokens.json by scripts/tokens.mjs: change that, not this. */', '', ':root {'];
  for (const [name, h, c] of hues) {
    out.push(`  /* ${name} */`);
    steps.forEach((step, i) => out.push(`  --${name}-${step}: oklch(${(0.98 - i * 0.083).toFixed(3)} ${(c * (1 - Math.abs(i - 5) / 7)).toFixed(3)} ${h});`));
    out.push('');
  }
  for (let i = 0; i <= 12; i++) out.push(`  --space-${i}: ${i * 4}px;`);
  out.push('', '  --radius-s: 6px;', '  --radius-m: 10px;', '  --radius-l: 16px;', '}');
  for (const [theme, light] of [['light', true], ['dark', false]]) {
    out.push('', `[data-theme='${theme}'] {`);
    for (const part of parts) {
      out.push(`  /* ${part} */`);
      for (const p of props) {
        const step = p === 'fg' ? (light ? 900 : 50) : p.startsWith('border') ? (light ? 200 : 700) : light ? 50 : 900;
        const value = p === 'radius' ? 'var(--radius-m)' : p === 'shadow' ? (light ? '0 1px 2px oklch(0 0 0 / 0.08)' : 'none') : `var(--gray-${step})`;
        out.push(`  --${part}-${p}: ${value};`);
      }
    }
    out.push('}');
  }
  return out;
}

/** Airports for the search box: far more than the computer sends of a diff. */
function airportsJson() {
  const r = rand(19);
  const letters = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
  const parts = ['Port', 'Lake', 'North', 'Saint', 'East', 'New', 'Fort', 'Glen', 'Bay', 'Cedar', 'Alder', 'Mill', 'Stone', 'Clear', 'Red', 'Pine'];
  const ends = ['field', 'ford', 'haven', 'ton', 'wick', 'mouth', 'ridge', 'dale', 'brook', ' Falls', ' Springs', ' Harbor'];
  const out = ['['];
  for (let i = 0; i < 6000; i++) {
    const code = Array.from({ length: 3 }, () => letters[Math.floor(r() * 26)]).join('');
    const city = `${parts[Math.floor(r() * parts.length)]} ${parts[Math.floor(r() * parts.length)]}${ends[Math.floor(r() * ends.length)]}`;
    out.push(`  { "code": "${code}", "city": "${city}", "lat": ${(r() * 140 - 70).toFixed(4)}, "lon": ${(r() * 340 - 170).toFixed(4)}, "runways": ${1 + Math.floor(r() * 4)} }${i < 5999 ? ',' : ''}`);
  }
  out.push(']');
  return out;
}

const ogImage = 'diff --git a/public/og-image.png b/public/og-image.png\nnew file mode 100644\nindex 0000000..3f1c2ab\nBinary files /dev/null and b/public/og-image.png differ';

// ---------------------------------------------------------------------------------------------
// The folders

function seed() {
  return {
    folders: new Map([
      [
        'p:p-api',
        {
          project: 'p-api',
          repo: true,
          branch: 'main',
          branches: ['main', 'feat/rate-limit', 'fix/auth-flake', 'release/0.4'],
          ahead: 0,
          behind: 2,
          upstream: true,
          files: [file('src/cache/key.rs', 'modified', keyRs), file('README.md', 'modified', readme), file('docs/caching.md', 'untracked', cachingMd)],
          viewed: new Set(['README.md']),
        },
      ],
      [
        'wt:t-flaky',
        {
          project: 'p-api',
          repo: true,
          branch: 'fix/auth-flake',
          branches: ['fix/auth-flake', 'main', 'feat/rate-limit', 'release/0.4'],
          ahead: 2,
          behind: 0,
          upstream: false,
          files: [file('src/auth/session.rs', 'modified', sessionRs), file('tests/auth_refresh.rs', 'untracked', refreshTest), file('Cargo.toml', 'modified', cargoToml)],
          viewed: new Set(),
          worktree: { base: 'main', uncommitted: 2, unmerged: 2, unpushed: null },
        },
      ],
      [
        'p:p-web',
        {
          project: 'p-web',
          repo: true,
          branch: 'main',
          branches: ['main', 'chore/vite-7', 'feat/dark-settings', 'release/2.3'],
          ahead: 1,
          behind: 0,
          upstream: true,
          files: [
            file('package.json', 'modified', packageJson),
            file('vite.config.ts', 'modified', viteConfig),
            file('src/dev/proxy.ts', 'renamed', proxyTs, { from: 'src/legacy/proxy.js' }),
            file('src/styles/tokens.css', 'untracked', untracked(tokensCss())),
            file('src/data/airports.json', 'untracked', untracked(airportsJson())),
            file('public/og-image.png', 'added', ogImage, { added: 0, removed: 0, binary: true }),
          ],
          viewed: new Set(),
        },
      ],
      ['p:p-notes', { project: 'p-notes', repo: false, files: [], viewed: new Set() }],
    ]),
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

function plural(n, one, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`;
}

/** Whose folder a request is about: a thread's (its worktree, for one in a worktree), else a project's. */
function place(demo, msg) {
  if (msg.thread_id) {
    const t = demo.threads.get(msg.thread_id);
    if (!t) throw error('not_found', 'No such thread');
    if (!t.project) throw error('bad_request', 'This thread has no folder of its own');
    const key = t.worktree && of(demo).folders.has(`wt:${t.id}`) ? `wt:${t.id}` : `p:${t.project.id}`;
    return { key, f: of(demo).folders.get(key), thread: t };
  }
  if (!msg.project_id) throw error('bad_request', 'Name a thread or a project');
  const f = of(demo).folders.get(`p:${msg.project_id}`);
  if (!f) throw error('not_found', 'No such project');
  return { key: `p:${msg.project_id}`, f, thread: null };
}

/** Why another branch can't be checked out there now, if it can't. */
function switchBlocked(demo, f) {
  if (f.worktree) return `This thread works in a worktree: ${f.branch} stays checked out there. Merge it into ${f.worktree.base} instead.`;
  const working = [...demo.threads.values()].some((t) => t.project?.id === f.project && !t.worktree && t.run_state === 'working');
  return working ? 'A thread is working in this folder: switch once it’s done.' : null;
}

function mergeBlocked(f) {
  const w = f.worktree;
  if (w.uncommitted === 1) return '1 file isn’t committed yet. Commit or revert it first.';
  if (w.uncommitted > 1) return `${w.uncommitted} files aren’t committed yet. Commit or revert them first.`;
  if (w.unmerged === 0) return `${w.base} already has everything on this branch.`;
  return null;
}

function status(demo, f) {
  if (!f.repo) return { type: 'git_status', is_repo: false, ahead: 0, behind: 0, has_upstream: false, files: [], can_switch: false };
  const blocked = switchBlocked(demo, f);
  const viewed = f.files.filter((x) => f.viewed.has(x.path)).map((x) => x.path);
  const w = f.worktree;
  return {
    type: 'git_status',
    is_repo: true,
    branch: f.branch,
    default_branch: 'main',
    ahead: f.ahead,
    behind: f.behind,
    has_upstream: f.upstream,
    files: f.files.map(({ diff, ...rest }) => rest),
    ...(w ? { worktree: { branch: f.branch, base: w.base, uncommitted: w.uncommitted, ...(w.unpushed != null ? { unpushed: w.unpushed } : {}), ...(mergeBlocked(f) ? { merge_blocked: mergeBlocked(f) } : {}), unmerged: w.unmerged } } : {}),
    can_switch: !blocked,
    ...(blocked ? { switch_blocked: blocked } : {}),
    ...(viewed.length ? { viewed } : {}),
  };
}

/** The threads in a folder carry its state on their rows, as the computer's do. */
function syncRows(demo, key, f) {
  if (!f.repo) return;
  for (const t of demo.threads.values()) {
    const here = key.startsWith('wt:') ? `wt:${t.id}` === key && t.worktree : t.project?.id === f.project && !t.worktree;
    if (!here) continue;
    const git = { changed: f.worktree ? f.worktree.uncommitted : f.files.length, ahead: f.ahead, behind: f.behind, default_branch: 'main' };
    if (JSON.stringify(t.git) !== JSON.stringify(git) || t.branch !== f.branch) demo.update(t.id, { git, branch: f.branch });
  }
}

// ---------------------------------------------------------------------------------------------
// The answers

export default {
  git_status: (msg, demo) => {
    const { key, f } = place(demo, msg);
    syncRows(demo, key, f);
    return status(demo, f);
  },

  git_diff: (msg, demo) => {
    const { f } = place(demo, msg);
    const changed = f.files.find((x) => x.path === msg.path);
    if (!changed) throw error('not_found', `${msg.path} has no changes here`);
    let diff = changed.diff;
    let truncated = false;
    if (diff.length > MAX_DIFF) {
      diff = diff.slice(0, diff.lastIndexOf('\n', MAX_DIFF));
      truncated = true;
    }
    return { type: 'git_diff', path: msg.path, diff, ...(truncated ? { truncated } : {}) };
  },

  git_commit: (msg, demo) => {
    if (!String(msg.message || '').trim()) throw error('bad_request', 'Write a commit message first.');
    const { key, f } = place(demo, msg);
    if (!f.repo) throw error('bad_request', 'This folder isn’t a git repository');
    if (f.worktree) {
      if (f.worktree.uncommitted === 0) throw error('conflict', 'nothing to commit, working tree clean');
      f.worktree.uncommitted = 0;
      f.worktree.unmerged += 1;
      if (f.worktree.unpushed != null) f.worktree.unpushed += 1;
    } else {
      if (f.files.length === 0) throw error('conflict', 'nothing to commit, working tree clean');
      f.files = [];
      f.viewed.clear();
    }
    f.ahead += 1;
    syncRows(demo, key, f);
    return { type: 'ack' };
  },

  git_push: (msg, demo) => {
    const { key, f } = place(demo, msg);
    if (!f.repo) throw error('bad_request', 'This folder isn’t a git repository');
    // A branch without an upstream gets one, as the panel's Push does.
    if (f.worktree) f.worktree.unpushed = 0;
    else f.ahead = 0;
    f.upstream = true;
    syncRows(demo, key, f);
    return { type: 'ack' };
  },

  git_branches: (msg, demo) => {
    const { f } = place(demo, msg);
    if (!f.repo) throw error('bad_request', 'This folder isn’t a git repository');
    return { type: 'git_branches', current: f.branch, default_branch: 'main', branches: f.branches };
  },

  git_switch: (msg, demo) => {
    const { key, f } = place(demo, msg);
    if (!f.repo) throw error('bad_request', 'This folder isn’t a git repository');
    const blocked = switchBlocked(demo, f);
    if (blocked) throw error('conflict', blocked);
    if (!f.branches.includes(msg.branch)) throw error('not_found', `There’s no local branch called ${msg.branch}`);
    if (msg.branch === 'fix/auth-flake' && of(demo).folders.get('wt:t-flaky')?.worktree) throw error('conflict', '\'fix/auth-flake\' is already used by worktree at \'~/.trek/worktrees/trek-api/fix-auth-flake\'');
    // What git says when a change here would be lost on the other branch.
    const dirty = f.files.filter((x) => x.status === 'modified').map((x) => x.path);
    if (dirty.length && msg.branch !== f.branch) throw error('conflict', `Your local changes to the following files would be overwritten by checkout ${dirty.join(', ')}. Commit or stash them first.`);
    f.branch = msg.branch;
    f.behind = 0;
    f.ahead = 0;
    syncRows(demo, key, f);
    return { type: 'ack' };
  },

  worktree_merge: (msg, demo) => {
    const t = demo.threads.get(msg.thread_id);
    if (!t) throw error('not_found', 'No such thread');
    const f = of(demo).folders.get(`wt:${t.id}`);
    if (!t.worktree || !f?.worktree) throw error('bad_request', 'This thread has no worktree.');
    const blocked = mergeBlocked(f);
    if (blocked) throw error('conflict', blocked);
    const base = of(demo).folders.get(`p:${f.project}`);
    if (base) base.ahead += f.worktree.unmerged;
    f.worktree.unmerged = 0;
    f.ahead = 0;
    f.files = [];
    f.viewed.clear();
    syncRows(demo, `wt:${t.id}`, f);
    return { type: 'ack' };
  },

  worktree_remove: (msg, demo) => {
    const t = demo.threads.get(msg.thread_id);
    if (!t) throw error('not_found', 'No such thread');
    const s = of(demo);
    const f = s.folders.get(`wt:${t.id}`);
    if (!t.worktree || !f?.worktree) throw error('bad_request', 'This thread has no worktree.');
    const w = f.worktree;
    if (!msg.force && (w.uncommitted > 0 || (msg.delete_branch && w.unmerged > 0))) {
      const lines = [];
      if (w.uncommitted > 0) lines.push(`${plural(w.uncommitted, 'uncommitted change')} in the worktree would be lost.`);
      if (w.unmerged === 0) lines.push(`${f.branch} has nothing ${w.base} doesn’t, so it goes too.`);
      else if (msg.delete_branch) lines.push(`${f.branch} has ${plural(w.unmerged, 'commit')} that ${w.base} doesn’t have.`);
      else lines.push(`${f.branch} keeps its ${plural(w.unmerged, 'commit')} that ${w.base} doesn’t have.`);
      throw error('conflict', lines.join(' '));
    }
    s.folders.delete(`wt:${t.id}`);
    const base = s.folders.get(`p:${f.project}`);
    if (base && (msg.delete_branch || w.unmerged === 0)) base.branches = base.branches.filter((b) => b !== f.branch);
    // The thread carries on in the project folder.
    demo.update(t.id, { worktree: false, branch: base?.branch || w.base, base: null, git: base ? { changed: base.files.length, ahead: base.ahead, behind: base.behind, default_branch: 'main' } : undefined });
    demo.upsert(t.id, { id: `i-wt-${Date.now()}`, kind: 'notice', text: 'Its worktree was removed. The thread runs in the project folder now, in a new agent session.' });
    return { type: 'ack' };
  },

  set_viewed: (msg, demo) => {
    const { f } = place(demo, msg);
    if (!f.files.some((x) => x.path === msg.path)) throw error('not_found', `${msg.path} has no changes here`);
    if (msg.viewed) f.viewed.add(msg.path);
    else f.viewed.delete(msg.path);
    return status(demo, f);
  },
};
