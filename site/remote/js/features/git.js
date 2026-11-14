// A thread's Git (`docs/MOBILE.md` › Git), as the iPhone's Git sheet and the computer's Git panel
// have it: the branch and how far it is from its upstream (or, in a worktree, its base), the
// changed files and their diffs, Commit and Push, and the branches to switch to. A worktree thread
// can be merged into its base or have its worktree removed; when removing would lose work the
// computer says what, and nothing goes until the user agrees. Files are ticked viewed as on the
// computer's panel (`diff_viewed`). From the thread menu's "Git…".

import { useEffect, useMemo, useRef, useState } from '../../vendor/hooks.mjs';
import { html, Icon, IconButton, Sheet, Spinner, addIcons, plural } from '../ui.js';
import * as app from '../model.js';
import { ConnectionBanner } from '../thread.js';
import { registerMenuItems, registerScreen } from '../routes.js';

// Lucide (ISC licence), 24 × 24, stroked.
addIcons({
  'git-merge': '<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/>',
  'git-ahead': '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>',
  'git-behind': '<path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>',
  'git-prev': '<path d="m18 15-6-6-6 6"/>',
  'git-none': '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/><path d="m9.5 10.5 5 5"/><path d="m14.5 10.5-5 5"/>',
  'git-cut': '<circle cx="6" cy="6" r="3"/><path d="M8.12 8.12 12 12"/><path d="M20 4 8.12 15.88"/><circle cx="6" cy="18" r="3"/><path d="M14.8 14.8 20 20"/>',
  'git-viewed': '<circle cx="12" cy="12" r="10" fill="currentColor" stroke="none"/><path d="m8 12.5 2.8 2.8L16.5 9" style="stroke: var(--bg)"/>',
  'git-unviewed': '<circle cx="12" cy="12" r="9"/>',
});

// ---------------------------------------------------------------------------------------------
// A unified diff, as rows to draw

/** Lines longer than this aren't compared: nothing of them would fit on the screen at once. */
const LONGEST_COMPARED = 2000;
/** What comes before a file's first hunk: it says nothing the screen doesn't already. */
const HEADER = /^(diff |index |--- |\+\+\+ |new file mode|deleted file mode|old mode|new mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|Binary files)/;

/**
 * What differs between a line and the one that replaces it (arrays of characters): what's left
 * once what they start and end with in common is set aside, as `[from, to)` on each side. Null
 * when they share too little for that to say anything.
 */
export function changedParts(a, b) {
  if (a.length > LONGEST_COMPARED || b.length > LONGEST_COMPARED) return null;
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let end = 0;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end += 1;
  const shared = start + end;
  if (shared === 0 || shared * 3 < Math.min(a.length, b.length)) return null;
  const oldEnd = a.length - end;
  const newEnd = b.length - end;
  return { old: start < oldEnd ? [start, oldEnd] : null, new: start < newEnd ? [start, newEnd] : null };
}

/** Where a run of removed lines is followed by as many added ones, mark what changed in each pair. */
function markChanges(lines) {
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== 'removed') {
      i += 1;
      continue;
    }
    let removed = i;
    while (removed < lines.length && lines[removed].kind === 'removed') removed += 1;
    let added = removed;
    while (added < lines.length && lines[added].kind === 'added') added += 1;
    if (added - removed === removed - i) {
      for (let k = 0; k < removed - i; k += 1) {
        const parts = changedParts(Array.from(lines[i + k].text), Array.from(lines[removed + k].text));
        if (parts) {
          lines[i + k].changed = parts.old;
          lines[removed + k].changed = parts.new;
        }
      }
    }
    i = Math.max(added, i + 1);
  }
}

/**
 * A unified diff as rows: `hunk` (its `@@` line), `added`, `removed` and `context` lines with the
 * line's number (the new file's; the old one's for a removed line) and, in a line changed rather
 * than added, what changed (`changed`, in characters), and `note` ("No newline at end of file").
 * The header before the first hunk is left out. Text with no header at all (an untracked file, as
 * the computer sends it: every line `+`) is one hunk from line 1.
 */
export function parseDiff(diff) {
  const out = [];
  const rows = String(diff || '').split('\n');
  let inHunk = !rows.some((r) => r.startsWith('@@')) && !HEADER.test(rows[0] || '');
  let oldN = 1;
  let newN = 1;
  rows.forEach((s, i) => {
    if (s.startsWith('@@')) {
      inHunk = true;
      const m = s.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) {
        oldN = Number(m[1]);
        newN = Number(m[2]);
      }
      out.push({ id: i, kind: 'hunk', text: s });
      return;
    }
    if (s.startsWith('diff --git')) {
      inHunk = false;
      return;
    }
    if (!inHunk) return;
    if (s.startsWith('+')) {
      out.push({ id: i, kind: 'added', number: newN, text: s.slice(1) });
      newN += 1;
    } else if (s.startsWith('-')) {
      out.push({ id: i, kind: 'removed', number: oldN, text: s.slice(1) });
      oldN += 1;
    } else if (s.startsWith('\\')) {
      out.push({ id: i, kind: 'note', text: s.slice(2) });
    } else if (!(s === '' && i === rows.length - 1)) {
      out.push({ id: i, kind: 'context', number: newN, text: s.slice(1) });
      oldN += 1;
      newN += 1;
    }
  });
  markChanges(out);
  return out;
}

export function nameOf(path) {
  return String(path).split('/').pop();
}

export function dirOf(path) {
  const at = String(path).lastIndexOf('/');
  return at > 0 ? path.slice(0, at) : '';
}

/** "1 of 3 viewed", as the computer's file list heads it; null with none viewed. */
export function viewedLabel(viewed, total) {
  return viewed ? `${viewed} of ${total} viewed` : null;
}

// ---------------------------------------------------------------------------------------------
// What the computer said, by folder: a thread's (`t:<id>`) or a project's (`p:<id>`)

const folders = new Map();
const listeners = new Set();
/** Threads the computer has no git for (no folder of their own): no "Git…" in their menu. */
const noGit = new Set();

function notify() {
  for (const l of listeners) l();
}

const keyOf = ({ threadId, projectId }) => (threadId ? `t:${threadId}` : `p:${projectId}`);
const targetOf = ({ threadId, projectId }) => (threadId ? { thread_id: threadId } : { project_id: projectId });

function folder(key) {
  let g = folders.get(key);
  if (!g) {
    g = { status: null, branches: null, unavailable: null, busy: 0 };
    folders.set(key, g);
  }
  return g;
}

function useFolder(key) {
  const [, bump] = useState(0);
  useEffect(() => {
    const l = () => bump((n) => n + 1);
    listeners.add(l);
    return () => listeners.delete(l);
  }, []);
  return folder(key);
}

/**
 * Reading a folder's git failed. Before anything of it came in, the screen says why in place of
 * its spinners (a thread with no folder of its own); once it shows something, a toast says so.
 */
function readFailed(t, err) {
  const g = folder(keyOf(t));
  if (g.status) return app.toast(err.message, 'negative');
  g.unavailable = { text: err.message, refused: err.code === 'bad_request' };
  if (g.unavailable.refused && t.threadId) noGit.add(t.threadId);
}

async function loadStatus(t) {
  const g = folder(keyOf(t));
  try {
    g.status = await app.ask({ type: 'git_status', ...targetOf(t) });
    g.unavailable = null;
    if (t.threadId) noGit.delete(t.threadId);
  } catch (err) {
    readFailed(t, err);
  }
  notify();
}

/** The status, then the branches (a folder that isn't a repository, or can't be read, has none to ask for). */
async function loadAll(t) {
  await loadStatus(t);
  const g = folder(keyOf(t));
  if (g.status && g.status.is_repo !== false) await loadBranches(t);
}

async function loadBranches(t) {
  const g = folder(keyOf(t));
  try {
    g.branches = await app.ask({ type: 'git_branches', ...targetOf(t) });
  } catch (err) {
    readFailed(t, err);
  }
  notify();
}

/** An action on the folder (commit, push, switch, merge): its toast, then the status read again. */
async function act(t, msg, done) {
  const g = folder(keyOf(t));
  g.busy += 1;
  notify();
  let ok = false;
  try {
    await app.ask({ ...msg, ...targetOf(t) }, { timeout: 120_000 });
    app.toast(done);
    ok = true;
  } catch (err) {
    app.toast(err.message, 'negative');
  } finally {
    g.busy -= 1;
    notify();
  }
  loadStatus(t);
  return ok;
}

/**
 * Remove a worktree thread's worktree. When something would be lost the computer says what and
 * nothing happens: `confirm` gets its words, and sending again with `force` is the user agreeing.
 */
async function removeWorktree(threadId, deleteBranch, force, confirm) {
  const t = { threadId };
  const g = folder(keyOf(t));
  g.busy += 1;
  notify();
  try {
    await app.ask({ type: 'worktree_remove', thread_id: threadId, ...(deleteBranch ? { delete_branch: true } : {}), ...(force ? { force: true } : {}) }, { timeout: 120_000 });
    app.toast('Removed the worktree');
    // The thread carries on in the project folder: its status and branches are that folder's.
    loadAll(t);
  } catch (err) {
    if (err.code === 'conflict' && !force) confirm(err.message);
    else app.toast(err.message, 'negative');
  } finally {
    g.busy -= 1;
    notify();
  }
}

/** Tick a file viewed, or not: shown at once; the computer keeps the mark with its diff and answers with the status. */
async function setViewed(t, path, viewed) {
  const g = folder(keyOf(t));
  if (g.status) {
    const marked = (g.status.viewed || []).filter((p) => p !== path);
    if (viewed) marked.push(path);
    g.status = { ...g.status, viewed: marked };
    notify();
  }
  try {
    g.status = await app.ask({ type: 'set_viewed', ...targetOf(t), path, viewed });
    notify();
  } catch (err) {
    app.toast(err.message, 'negative');
    loadStatus(t);
  }
}

// ---------------------------------------------------------------------------------------------
// Parts

/** The tag after a file's name: "new", "deleted", or "from <old name>" for a renamed one. */
function FileTag({ file }) {
  if (file.status === 'added' || file.status === 'untracked') return html`<span class="git-tag new">new</span>`;
  if (file.status === 'deleted') return html`<span class="git-tag deleted">deleted</span>`;
  if (file.status === 'renamed' && file.from) return html`<span class="git-tag">from ${nameOf(file.from)}</span>`;
  return null;
}

/** A file's lines: "+12 −3", or "binary". */
function Lines({ file }) {
  if (file.binary) return html`<span class="git-lines muted">binary</span>`;
  return html`<span class="git-lines"><span class="add">+${file.added || 0}</span> <span class="del">−${file.removed || 0}</span></span>`;
}

function Note({ icon, children }) {
  return html`<p class="git-note"><${Icon} name=${icon} size="s" /><span>${children}</span></p>`;
}

// ---------------------------------------------------------------------------------------------
// The Git screen

function GitRoute(props) {
  return html`<${GitScreen} key=${keyOf(props)} threadId=${props.threadId} projectId=${props.projectId} />`;
}

function GitScreen({ threadId, projectId }) {
  const state = app.useApp();
  const t = useMemo(() => ({ threadId, projectId }), [threadId, projectId]);
  const g = useFolder(keyOf(t));
  const row = threadId ? app.row(threadId) : null;
  const project = projectId ? state.projects.find((p) => p.id === projectId) : null;
  const online = state.status === 'online';
  const [message, setMessage] = useState('');
  /** `remove` (which way), or `{ lose, deleteBranch }`: the computer's words for what would be lost. */
  const [asking, setAsking] = useState(null);

  useEffect(() => {
    if (online) loadAll(t);
  }, [online]);

  // The thread's git moved on (a turn changed files, another branch, its worktree went): read it again.
  const moved = row ? [row.branch, row.worktree, row.base, row.git?.changed, row.git?.ahead, row.git?.behind].join('|') : '';
  const seen = useRef(moved);
  useEffect(() => {
    if (seen.current === moved) return;
    const elsewhere = seen.current.split('|').slice(0, 3).join('|') !== moved.split('|').slice(0, 3).join('|');
    seen.current = moved;
    if (!online) return;
    if (elsewhere) loadAll(t);
    else loadStatus(t);
  }, [moved]);

  const s = g.status;
  const busy = g.busy > 0;
  const wt = s?.worktree || null;
  const base = wt?.base || row?.base || null;
  const files = s?.files || [];
  const marks = app.has('diff_viewed');
  const viewed = new Set(marks ? s?.viewed || [] : []);
  const ahead = s?.ahead ?? row?.git?.ahead ?? 0;
  const behind = s?.behind ?? row?.git?.behind ?? 0;
  const branch = s?.branch || row?.branch || '—';
  const isDefault = !!s?.branch && s.branch === (s.default_branch || row?.git?.default_branch);
  const canCommit = online && !busy && !!message.trim() && files.length > 0;

  const commit = async () => {
    if (await act(t, { type: 'git_commit', message: message.trim() }, 'Committed')) setMessage('');
  };
  const remove = (deleteBranch, force) => {
    setAsking(null);
    removeWorktree(threadId, deleteBranch, force, (text) => setAsking({ lose: text, deleteBranch }));
  };
  const openDiff = (path) => app.go({ screen: 'git-diff', threadId, projectId, path });

  const head = html`<div class="git-head">
    <span class="git-head-icon"><${Icon} name=${base ? 'branch' : 'folder'} /></span>
    <div class="git-head-main">
      <div class="git-head-branch"><span class="mono">${branch}</span>${isDefault && html`<span class="badge">default</span>`}</div>
      <div class="git-head-sub">
        <span>${base ? `Worktree from ${base}` : 'In the project folder'}</span>
        ${s?.is_repo === false
          ? null
          : s && !s.has_upstream && !base
          ? html`<span>· not pushed yet</span>`
          : html`<span class="git-ab" aria-label=${`${ahead} ahead, ${behind} behind ${base || 'upstream'}`}>
              <span class=${ahead ? 'on' : ''}><${Icon} name="git-ahead" size="xs" />${ahead}</span>
              <span class=${behind ? 'behind' : ''}><${Icon} name="git-behind" size="xs" />${behind}</span>
            </span>`}
      </div>
    </div>
  </div>`;

  const added = files.reduce((n, f) => n + (f.added || 0), 0);
  const removed = files.reduce((n, f) => n + (f.removed || 0), 0);
  const seenCount = viewedLabel(files.filter((f) => viewed.has(f.path)).length, files.length);
  const changes = html`<div class="git-section-head">
      <h2>Changes${s ? ` (${files.length})` : ''}</h2>
      ${seenCount && html`<span class="git-seen">${seenCount}</span>`}
      ${files.length > 0 && html`<span class="git-lines"><span class="add">+${added}</span> <span class="del">−${removed}</span></span>`}
    </div>
    <div class="group git-files">
      ${!s
        ? html`<div class="item git-wait"><${Spinner} /></div>`
        : files.length === 0
          ? html`<div class="item git-clean"><${Icon} name="check" size="s" />Nothing to commit</div>`
          : files.map((f) => {
              const on = viewed.has(f.path);
              return html`<div class=${`git-file ${on ? 'viewed' : ''}`} key=${f.path}>
                <button type="button" class="git-file-open" onClick=${() => openDiff(f.path)}>
                  <${Icon} name="file" size="s" />
                  <span class="git-file-main">
                    <span class="git-file-top"><span class=${`git-file-name ${f.status === 'deleted' ? 'gone' : ''}`}>${nameOf(f.path)}</span><${FileTag} file=${f} /></span>
                    ${dirOf(f.path) && html`<span class="git-file-dir">${dirOf(f.path)}</span>`}
                  </span>
                  <${Lines} file=${f} />
                </button>
                ${marks &&
                html`<button type="button" class="git-tick" aria-pressed=${on ? 'true' : 'false'} aria-label=${on ? `${nameOf(f.path)}: viewed` : `Mark ${nameOf(f.path)} viewed`} title=${on ? 'Viewed' : 'Mark viewed'} disabled=${!online} onClick=${() => setViewed(t, f.path, !on)}>
                  <${Icon} name=${on ? 'git-viewed' : 'git-unviewed'} />
                </button>`}
              </div>`;
            })}
    </div>
    ${base && html`<p class="git-foot">Against ${base}: its commits, and what isn’t committed yet.</p>`}`;

  const commitPart = html`<h2 class="group-title">Commit</h2>
    <div class="git-commit">
      <textarea class="input" rows="2" placeholder="Commit message" aria-label="Commit message" value=${message} onInput=${(e) => setMessage(e.target.value)}></textarea>
      <div class="git-btns">
        <button class="btn primary" type="button" disabled=${!canCommit} onClick=${commit}><${Icon} name="commit" size="s" />Commit</button>
        <button class="btn" type="button" disabled=${!online || busy} onClick=${() => act(t, { type: 'git_push' }, 'Pushed')}><${Icon} name="upload" size="s" />Push</button>
      </div>
      <p class="git-foot">Commit takes every change, as Trek’s Git panel does.</p>
    </div>`;

  const worktreePart =
    base &&
    threadId &&
    html`<h2 class="group-title">Worktree</h2>
      <div class="group">
        <button type="button" class="item" disabled=${!online || busy || !!wt?.merge_blocked} onClick=${() => act(t, { type: 'worktree_merge' }, 'Merged')}><${Icon} name="git-merge" /><span class="grow">Merge into ${base}</span></button>
        <button type="button" class="item git-danger" disabled=${!online || busy} onClick=${() => setAsking('remove')}><${Icon} name="trash" /><span class="grow">Remove worktree…</span></button>
      </div>
      ${wt?.merge_blocked ? html`<p class="git-foot">${wt.merge_blocked}</p>` : wt?.unmerged > 0 ? html`<p class="git-foot">${plural(wt.unmerged, 'commit')} ${base} doesn’t have yet.</p>` : null}`;

  const list = g.branches?.branches || [];
  const branchesPart = html`<h2 class="group-title">Branches</h2>
    <div class="group git-branches">
      ${!g.branches
        ? html`<div class="item git-wait"><${Spinner} /></div>`
        : list.map((b) => {
            const current = b === (s?.branch ?? g.branches.current);
            return html`<button type="button" key=${b} class="item git-branch" aria-current=${current ? 'true' : undefined} disabled=${current || !online || busy || s?.can_switch === false} onClick=${() => act(t, { type: 'git_switch', branch: b }, `On ${b}`).then(() => loadBranches(t))}>
              <span class="grow mono">${b}</span>
              ${b === g.branches.default_branch && html`<span class="badge">default</span>`}
              ${current && html`<${Icon} name="check" size="s" />`}
            </button>`;
          })}
    </div>
    ${s?.switch_blocked && html`<p class="git-foot">${s.switch_blocked}</p>`}`;

  return html`<div class="screen git">
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <div class="bar-title"><h1>Git</h1>${(row?.title || project?.name) && html`<div class="sub">${row?.title || project?.name}</div>`}</div>
      ${busy && html`<span class="git-busy" role="status" aria-label="Working on it"><${Spinner} /></span>`}
      <${IconButton}
        icon="refresh"
        label="Refresh"
        disabled=${!online}
        onClick=${() => loadAll(t)}
      />
    </header>
    <${ConnectionBanner} />
    <div class="scroll">
      <div class="git-page">
        ${!s && g.unavailable
          ? html`<div class="empty git-none">
              <${Icon} name="git-none" />
              <h2>No Git here</h2>
              <p>${g.unavailable.text}</p>
            </div>`
          : html`${head}
            ${s?.is_repo === false
              ? html`<${Note} icon="folder">This folder isn’t a git repository.<//>`
              : html`${changes} ${commitPart} ${worktreePart} ${branchesPart}`}`}
      </div>
    </div>
    ${asking === 'remove' &&
    html`<${Sheet} title="Remove this thread’s worktree?" onClose=${() => setAsking(null)}>
      <p class="git-ask">The thread carries on in the project folder${base ? ` on ${base}` : ''}.</p>
      <div class="git-ask-btns">
        <button class="btn danger block" type="button" onClick=${() => remove(false, false)}>Remove, keep the branch</button>
        <button class="btn danger block" type="button" onClick=${() => remove(true, false)}>Remove and delete the branch</button>
        <button class="btn block" type="button" onClick=${() => setAsking(null)}>Cancel</button>
      </div>
    <//>`}
    ${asking?.lose &&
    html`<${Sheet} title="Lose this work?" onClose=${() => setAsking(null)}>
      <p class="git-ask">${asking.lose}</p>
      <div class="row-btns git-ask-row">
        <button class="btn" type="button" onClick=${() => setAsking(null)}>Cancel</button>
        <button class="btn danger" type="button" onClick=${() => remove(asking.deleteBranch, true)}>Remove anyway</button>
      </div>
    <//>`}
  </div>`;
}

// ---------------------------------------------------------------------------------------------
// One file's diff

/** A diff longer than this opens folded, its first rows shown. */
const FOLD_OVER = 400;
const FOLD_SHOWS = 300;

function DiffRoute(props) {
  return html`<${DiffScreen} key=${`${keyOf(props)}\n${props.path}`} threadId=${props.threadId} projectId=${props.projectId} path=${props.path} />`;
}

/** One changed file's diff, as git has it in the folder now; up and down go through the others. */
function DiffScreen({ threadId, projectId, path }) {
  const state = app.useApp();
  const t = useMemo(() => ({ threadId, projectId }), [threadId, projectId]);
  const g = useFolder(keyOf(t));
  const online = state.status === 'online';
  const s = g.status;
  const files = s?.files || [];
  const at = files.findIndex((f) => f.path === path);
  // The computer serves a diff only for one of the folder's changed files.
  const inStatus = at >= 0;
  const file = files[at] || { path, status: 'modified', added: 0, removed: 0 };
  const [diff, setDiff] = useState(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!s && online) loadStatus(t);
  }, [online]);
  useEffect(() => {
    if (!s || !inStatus || file.binary || diff || !online) return undefined;
    let alive = true;
    app.ask({ type: 'git_diff', ...targetOf(t), path }, { timeout: 60_000 }).then(
      (d) => alive && setDiff(d),
      (err) => {
        if (!alive) return;
        setFailed(true);
        app.toast(err.message, 'negative');
      },
    );
    return () => {
      alive = false;
    };
  }, [!!s, inStatus, online]);

  const go = (p) => app.go({ screen: 'git-diff', threadId, projectId, path: p }, { replace: true });
  const marks = app.has('diff_viewed') && inStatus;
  const viewed = !!s?.viewed?.includes(path);

  // The computer doesn't always flag one: git's diff of it, or an untracked one it couldn't read as text, says so.
  const binary = file.binary || (diff && (diff.diff === 'Binary or unreadable file' || (/^Binary files .* differ$/m.test(diff.diff) && !/^@@/m.test(diff.diff))));
  let content;
  if (binary) content = html`<${Note} icon="file">A binary file: there are no lines to compare.<//>`;
  else if (diff)
    content = html`<${DiffView} diff=${diff.diff} />
      ${diff.truncated && html`<${Note} icon="git-cut">The diff is cut short here: it’s longer than the computer sends.<//>`}`;
  else if (s && !inStatus) content = html`<${Note} icon="check">Git has no change to this file now: it was committed or changed back since.<//>`;
  else if (failed) content = html`<${Note} icon="alert">Couldn’t load the diff.<//>`;
  else content = html`<div class="empty"><${Spinner} /></div>`;

  return html`<div class="screen git">
    <header class="bar">
      <${IconButton} icon="back" label="Back" onClick=${() => app.back()} />
      <div class="bar-title"><h1>${nameOf(path)}</h1>${dirOf(path) && html`<div class="sub">${dirOf(path)}</div>`}</div>
      ${marks &&
      html`<button type="button" class="icon-btn git-tick" aria-pressed=${viewed ? 'true' : 'false'} aria-label=${viewed ? 'Viewed' : 'Mark viewed'} title=${viewed ? 'Viewed' : 'Mark viewed'} disabled=${!online} onClick=${() => setViewed(t, path, !viewed)}>
        <${Icon} name=${viewed ? 'git-viewed' : 'git-unviewed'} />
      </button>`}
      ${files.length > 1 &&
      inStatus &&
      html`<${IconButton} icon="git-prev" label="Previous file" disabled=${at === 0} onClick=${() => go(files[at - 1].path)} />
        <${IconButton} icon="down" label="Next file" disabled=${at === files.length - 1} onClick=${() => go(files[at + 1].path)} />`}
    </header>
    <${ConnectionBanner} />
    <div class="scroll">
      <div class="git-diff-page">
        <div class="git-diff-file">
          <${FileTag} file=${file} />
          ${(inStatus || file.binary) && html`<${Lines} file=${file} />`}
          ${files.length > 1 && inStatus && html`<span class="git-diff-at">${at + 1} of ${files.length}</span>`}
        </div>
        ${content}
      </div>
    </div>
  </div>`;
}

/**
 * A unified diff, a row per line: additions on green, removals on red, each hunk under a quiet
 * band, the line's number in a gutter that stays put while the code scrolls sideways. In a line
 * changed rather than added, what changed stands out. A long one opens folded.
 */
function DiffView({ diff }) {
  const lines = useMemo(() => parseDiff(diff), [diff]);
  const [all, setAll] = useState(false);
  if (lines.length === 0) return html`<${Note} icon="check">No lines changed.<//>`;
  const folded = !all && lines.length > FOLD_OVER;
  const shown = folded ? lines.slice(0, FOLD_SHOWS) : lines;
  const digits = String(lines.reduce((n, l) => Math.max(n, l.number || 0), 0)).length;
  return html`<div class="diff" style=${{ '--digits': digits }}>
      <div class="diff-rows">${shown.map(diffRow)}</div>
    </div>
    ${folded && html`<button type="button" class="diff-fold" onClick=${() => setAll(true)}><${Icon} name="down" size="s" />Show the other ${(lines.length - FOLD_SHOWS).toLocaleString()} lines</button>`}`;
}

function diffRow(line) {
  if (line.kind === 'hunk') return html`<div class="diff-hunk" key=${line.id}>${line.text}</div>`;
  if (line.kind === 'note') return html`<div class="diff-note" key=${line.id}>${line.text}</div>`;
  const kind = line.kind === 'added' ? 'add' : line.kind === 'removed' ? 'del' : 'ctx';
  return html`<div class=${`diff-line ${kind}`} key=${line.id}>
    <span class="diff-gut" aria-hidden="true"><span class="diff-n">${line.number ?? ''}</span><span class="diff-s">${kind === 'add' ? '+' : kind === 'del' ? '−' : ''}</span></span>
    <span class="diff-code">${kind !== 'ctx' && html`<span class="sr">${kind === 'add' ? 'Added: ' : 'Removed: '}</span>`}${code(line)}</span>
  </div>`;
}

/** The line's text, what changed in it marked. */
function code(line) {
  const text = line.text || ' ';
  if (!line.changed) return text;
  const chars = Array.from(text);
  const [from, to] = line.changed;
  return [chars.slice(0, from).join(''), html`<mark class="diff-mark">${chars.slice(from, to).join('')}</mark>`, chars.slice(to).join('')];
}

// ---------------------------------------------------------------------------------------------
// Where it's opened from

registerScreen('git', GitRoute);
registerScreen('git-diff', DiffRoute);
// A thread with no folder of its own has no branch and no git: nothing to offer.
registerMenuItems(({ row, threadId }) => [
  {
    label: 'Git…',
    icon: 'branch',
    detail: row.git?.changed ? `${row.git.changed} changed` : undefined,
    hidden: !(row.branch || row.git) || noGit.has(threadId),
    onClick: () => app.go({ screen: 'git', threadId }),
  },
]);
