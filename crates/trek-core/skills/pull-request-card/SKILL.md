---
name: pull-request-card
description: A live pull request card - the branch's commits on a lane graph, where the PR stands (create PR, conflicts, resolve, run tests, ready, merged), each conflict side by side with Keep mine / Keep theirs, check chips with their details, and one action button that follows the state. Use when you open, update, check or merge a PR, or when a branch conflicts with its base.
---

# Pull request card

A `trek-html` card for **a pull request and where it stands**. It shows the branch's recent commits
on a lane graph (yours, a teammate's, the base branch), the PR's state as one strip
(Create PR → conflicts → Resolve → Run tests → Ready → Merged), and one action button that follows
that state. Hovering (or tapping) "N conflicts" opens each conflict with both sides and **Keep mine**
/ **Keep theirs**; hovering a check chip shows its tool, result and time.

Use it when you create a PR, find that it conflicts, resolve conflicts, run its checks, or merge it.
Show it again, with the new state, each time the state changes: the card never changes the repository
itself, it only asks you to.

## What you fill in

Copy the card into your answer as one fenced `trek-html` block, change the `<title>`, and replace
`DATA` with the real facts (from `git log`, `gh pr view`, the merge attempt, the checks). Never
invent a commit, a conflict or a check result.

- `title`, `number` (`null` before the PR exists), `branch`, `base`, `url` (the PR's https page or `null`).
- `state`: one of `draft` (no PR yet), `checking` (testing whether it merges), `conflicts`,
  `resolved` (conflicts fixed, tests not run), `testing`, `ready`, `merged`.
- `commits`, newest first, at most 12: `label`, `hash` (7 characters), `author` (`"You"` for the
  user), `age` ("now", "25m", "3h", "2d"), `lane` (`"you"`, `"other"` for a teammate's branch, or
  `"base"`), optional `chip` (a branch name shown beside it), `conflicts` (how many of the PR's
  conflicts this base commit causes), and `fork: true` on the base commit the branches start from.
- `conflictsWith`: who changed the same lines ("Lena Park").
- `conflicts`: `{ file, line, theirs, mine }`, one per conflicting hunk, `theirs` and `mine` one
  line of code each (the line that differs).
- `checks`: `{ name, result, state, detail, time }` with `state` `"ok"`, `"fail"`, `"running"` or
  `"pending"` ("Tests", "42 passed", "ok", "vitest · 42 passed, 0 failed", "38s").
- `tests`: `{ done, total }` while `state` is `testing`, else `null`.

## What the buttons send

Each calls `trek.send(text)`, which Trek posts as the user's next message in this thread (marked as
from the card); act on it as on any message, then show the card again with the new state.

| State | Button | Sends |
| --- | --- | --- |
| `draft` | Create PR | `Create the pull request` |
| `conflicts` | Resolve conflicts | `Resolve the conflicts with <base>` |
| a conflict, in the popover | Keep mine / Keep theirs | `Resolve <file>: keep mine` (or `keep theirs`; `<file> line <n>` when a file has several) |
| `resolved` | Run tests | `Run the tests` |
| `ready` | Merge | `Merge pull request #<number>` |
| `merged` | GitHub | opens `url` in the browser |

`checking` and `testing` show a spinner instead of a button.

## Rules

- One card per answer, then a sentence or two of prose with the takeaway ("PR #128 is open, but
  main moved on: Lena's key rotation changed the same lines in auth.ts and middleware.ts").
- No external scripts or fonts; it stays well under 200 KB.
- It styles itself with Trek's `--trek-*` variables (blue is yours, teal a teammate's, amber a
  conflict, green ready, purple merged), so it fits Night and Paper.

## The card

```trek-html
<!doctype html>
<html lang="en">
<head>
<title>Refresh sessions in the auth middleware</title>
<style>
  body { padding: 4px 0 8px; }
  .card { --you: var(--trek-info, #2f6be8); --other: #14a39a; --base: var(--trek-muted, #8a8a8a); --warn: var(--trek-warning, #c98a12); --ok: var(--trek-positive, #1f9d55); --merged: #8957e5;
    position: relative; border: 1px solid var(--trek-border, #e5e5e5); border-radius: 16px; background: var(--trek-surface, #fff); color: var(--trek-fg, #1d1d1f); font-family: var(--trek-font, system-ui); font-size: 13.5px; }
  header { display: flex; align-items: baseline; gap: 8px; padding: 14px 16px 6px; }
  header .t { font-weight: 650; font-size: 15px; flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  header .t span { color: var(--trek-muted, #888); font-weight: 500; margin-left: 4px; }
  .badge { font-size: 12.5px; font-weight: 600; white-space: nowrap; }
  .steps { display: flex; flex-wrap: wrap; gap: 4px 2px; padding: 2px 14px 8px; margin: 0; list-style: none; font-size: 11.5px; color: var(--trek-muted, #888); }
  .steps li { display: flex; align-items: center; gap: 4px; padding: 2px 7px; border-radius: 999px; }
  .steps li + li::before { content: "→"; margin: 0 4px 0 -4px; opacity: .5; }
  .steps li.done { color: var(--trek-fg, #222); }
  .steps li.now { color: var(--c, var(--you)); background: color-mix(in srgb, var(--c, var(--you)) 12%, transparent); font-weight: 600; }
  .log { position: relative; padding: 0 16px 0 0; }
  .log svg { position: absolute; left: 0; top: 0; overflow: visible; pointer-events: none; }
  .row { position: relative; display: flex; align-items: center; gap: 8px; height: 34px; padding-left: 64px; animation: in .35s ease both; }
  .row.dim .label { color: var(--trek-muted, #888); }
  .row.warn .label { color: var(--warn); }
  .row.hot { background: linear-gradient(90deg, color-mix(in srgb, var(--warn) 10%, transparent), transparent 70%); }
  .label { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .chip { font-family: var(--trek-mono, ui-monospace); font-size: 11.5px; padding: 1px 6px; border-radius: 5px; white-space: nowrap; color: var(--c); background: color-mix(in srgb, var(--c) 13%, transparent); }
  .count { font-size: 10.5px; font-weight: 700; min-width: 16px; height: 16px; border-radius: 8px; display: inline-grid; place-items: center; color: #fff; background: var(--warn); }
  .count.done { background: var(--ok); }
  .meta { margin-left: auto; padding-left: 10px; white-space: nowrap; color: var(--trek-muted, #888); font-size: 12px; }
  .meta code { font-family: var(--trek-mono, ui-monospace); margin-right: 6px; }
  .word { margin-left: auto; font-size: 12px; font-weight: 600; color: var(--c); }
  .checks { display: flex; flex-wrap: wrap; gap: 6px; padding: 10px 16px 2px; }
  .check { position: relative; display: inline-flex; align-items: center; gap: 5px; padding: 4px 9px; border: 1px solid var(--trek-border, #e5e5e5); border-radius: 999px; font-size: 12px; cursor: default; outline: none; }
  .check:hover, .check:focus { background: color-mix(in srgb, var(--trek-fg, #000) 5%, transparent); }
  .check b { font-weight: 600; }
  .check span { color: var(--trek-muted, #888); }
  .check i { font-style: normal; font-weight: 700; color: var(--c); }
  .tip { position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 5; white-space: nowrap; padding: 6px 10px; border-radius: 8px; font-size: 12px; background: var(--trek-surface, #fff); color: var(--trek-fg, #222); border: 1px solid var(--trek-border, #e5e5e5); box-shadow: 0 6px 20px rgba(0,0,0,.14); opacity: 0; transform: translateY(3px); transition: opacity .15s, transform .15s; pointer-events: none; }
  .tip em { font-style: normal; color: var(--trek-muted, #888); margin-left: 12px; }
  .check.right .tip { left: auto; right: 0; }
  .check:hover .tip, .check:focus .tip { opacity: 1; transform: none; }
  footer { display: flex; align-items: center; gap: 10px; padding: 12px 16px 14px; }
  .status { flex: 1; min-width: 0; display: flex; align-items: center; gap: 6px; color: var(--trek-muted, #888); }
  .status b { font-weight: 600; color: var(--c, var(--trek-fg)); }
  .conf { color: var(--warn); font-weight: 600; text-decoration: underline dotted; text-underline-offset: 3px; cursor: pointer; }
  .bar { height: 3px; border-radius: 2px; background: color-mix(in srgb, var(--ok) 20%, transparent); margin: 0 16px; overflow: hidden; }
  .bar div { height: 100%; background: var(--ok); transition: width .4s; }
  .act { border: 0; border-radius: 999px; padding: 7px 14px; font: inherit; font-size: 12.5px; font-weight: 600; cursor: pointer; white-space: nowrap; color: var(--you); background: color-mix(in srgb, var(--you) 13%, transparent); text-decoration: none; }
  .act:hover { background: color-mix(in srgb, var(--you) 21%, transparent); }
  .act.busy, .act:disabled { cursor: default; color: var(--trek-muted, #888); background: color-mix(in srgb, var(--trek-fg, #000) 7%, transparent); }
  .act.plain { color: var(--trek-fg, #222); background: none; border: 1px solid var(--trek-border, #ddd); }
  .spin { width: 11px; height: 11px; border-radius: 50%; border: 1.6px solid currentColor; border-right-color: transparent; display: inline-block; animation: spin .8s linear infinite; vertical-align: -1px; }
  .pop { position: absolute; left: 12px; right: 12px; bottom: 56px; z-index: 6; max-width: 460px; padding: 10px 12px 6px; border-radius: 12px; background: var(--trek-surface, #fff); border: 1px solid var(--trek-border, #e5e5e5); box-shadow: 0 10px 34px rgba(0,0,0,.18); max-height: calc(100% - 66px); overflow: auto; opacity: 0; transform: translateY(4px); transition: opacity .15s, transform .15s; pointer-events: none; }
  .pop.open { opacity: 1; transform: none; pointer-events: auto; }
  .pop h4 { margin: 0 0 4px; font-size: 13px; font-weight: 600; }
  .pop h4 span { color: var(--trek-muted, #888); font-weight: 400; }
  .hunk { padding: 8px 0; border-top: 1px solid var(--trek-border, #eee); }
  .hunk .where { font-size: 12px; color: var(--trek-muted, #888); margin-bottom: 4px; }
  .hunk .where b { color: var(--trek-fg, #222); font-weight: 600; }
  .side { display: grid; grid-template-columns: 42px 1fr; align-items: center; gap: 8px; font-size: 11.5px; color: var(--trek-muted, #888); }
  .side code { font-family: var(--trek-mono, ui-monospace); font-size: 12px; color: var(--trek-fg, #222); padding: 3px 8px; border-left: 3px solid var(--c); background: color-mix(in srgb, var(--c) 7%, transparent); white-space: pre; overflow: hidden; text-overflow: ellipsis; }
  .keep { display: flex; gap: 6px; justify-content: flex-end; margin-top: 6px; }
  .keep button { border: 1px solid var(--trek-border, #ddd); background: none; color: var(--trek-fg, #222); border-radius: 999px; padding: 3px 10px; font: inherit; font-size: 11.5px; cursor: pointer; }
  .keep button:hover { background: color-mix(in srgb, var(--trek-fg, #000) 6%, transparent); }
  .keep .sent { color: var(--ok); font-size: 11.5px; font-weight: 600; align-self: center; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes in { from { opacity: 0; transform: translateY(-4px); } }
  @media (max-width: 460px) { .meta code { display: none; } .row { padding-left: 58px; } }
</style>
</head>
<body>
<div class="card" id="card">
  <header><div class="t" id="title"></div><div class="badge" id="badge"></div></header>
  <ol class="steps" id="steps"></ol>
  <div class="log" id="log"><svg id="graph"></svg></div>
  <div class="checks" id="checks"></div>
  <div class="bar" id="bar" hidden><div></div></div>
  <footer><div class="status" id="status"></div><span id="action"></span></footer>
  <div class="pop" id="pop" role="dialog" aria-label="Conflicts"></div>
</div>
<script>
const DATA = {
  title: "Refresh sessions in the auth middleware",
  number: 128,
  branch: "feat/session-refresh",
  base: "main",
  url: "https://github.com/acme/webapp/pull/128",
  state: "conflicts",
  conflictsWith: "Lena Park",
  commits: [
    { label: "Add session refresh tests", hash: "d29f50a", author: "You", age: "now", lane: "you", chip: "feat/session-refresh" },
    { label: "Read sessions with the current key", hash: "5be27d1", author: "Lena", age: "6m", lane: "base", chip: "main", conflicts: 1 },
    { label: "Rotate session keys", hash: "a3f9c10", author: "Lena", age: "18m", lane: "base", conflicts: 2 },
    { label: "Refresh sessions near expiry", hash: "c70a92e", author: "You", age: "25m", lane: "you" },
    { label: "Add analytics consent banner", hash: "e83c51b", author: "Sam", age: "32m", lane: "other", chip: "feat/checkout-analytics" },
    { label: "Read session expiry from the token", hash: "1f8e3b4", author: "You", age: "40m", lane: "you" },
    { label: "Track checkout funnel events", hash: "62b0e47", author: "Sam", age: "2h", lane: "other" },
    { label: "Bump next to 16.3", hash: "9c2d4a8", author: "Sam", age: "3h", lane: "base", fork: true }
  ],
  conflicts: [
    { file: "middleware.ts", line: 18, theirs: "const session = await readSession(req, keys)", mine: "if (nearExpiry(session)) await refresh(session)" },
    { file: "auth.ts", line: 42, theirs: "return verify(token, keys.current)", mine: "return refreshIfNear(token, 5 * MINUTE)" },
    { file: "auth.ts", line: 57, theirs: "const ttl = keys.current.ttl", mine: "const ttl = session.expiresAt - Date.now()" }
  ],
  checks: [],
  tests: null
};

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const ORDER = ["draft", "checking", "conflicts", "resolved", "testing", "ready", "merged"];
const state = ORDER.includes(DATA.state) ? DATA.state : "draft";
const at = ORDER.indexOf(state);
const n = DATA.conflicts.length;
const pr = DATA.number ? "#" + DATA.number : "";
const plural = (k, w) => k + " " + w + (k === 1 ? "" : "s");
const colour = { you: "var(--you)", other: "var(--other)", base: "var(--base)" };
const tone = { draft: "var(--you)", checking: "var(--base)", conflicts: "var(--warn)", resolved: "var(--warn)", testing: "var(--you)", ready: "var(--ok)", merged: "var(--merged)" }[state];

// Header: the title, the number, and whether it's open.
$("title").innerHTML = esc(DATA.title) + (pr ? "<span>" + pr + "</span>" : "");
const badge = $("badge");
badge.textContent = state === "draft" ? "Not opened" : state === "merged" ? "● Merged" : "● Open";
badge.style.color = state === "draft" ? "var(--trek-muted, #888)" : state === "merged" ? "var(--merged)" : "var(--ok)";

// The state machine, as one strip.
const steps = [
  ["Create PR", 0],
  [state === "checking" ? "Checking" : n ? plural(n, "conflict") : "No conflicts", 2],
  ["Resolve", 3],
  ["Run tests", 4],
  ["Ready", 5],
  ["Merged", 6]
];
const now = state === "checking" ? 2 : at;
$("steps").innerHTML = steps.map(([label, i], k) => {
  const next = k + 1 < steps.length ? steps[k + 1][1] : 99;
  const cls = now >= next ? "done" : now >= i ? "now" : "";
  return '<li class="' + cls + '" style="--c:' + tone + '">' + (cls === "done" ? "✓ " : "") + esc(label) + "</li>";
}).join("");

// Commits on lanes: the base branch, the user's, a teammate's; branches curve into the commit they forked from.
const ROW = 34, X = { base: 14, you: 30, other: 46 };
const rows = [{ target: true }].concat(DATA.commits);
const log = $("log"), svg = $("graph");
let resolved = 0;
rows.forEach((c, i) => {
  const el = document.createElement("div");
  el.style.animationDelay = i * 40 + "ms";
  if (c.target) {
    const word = { draft: "not opened", checking: "checking", conflicts: plural(n, "conflict"), resolved: "resolved", testing: DATA.tests ? DATA.tests.done + "/" + DATA.tests.total : "testing", ready: "ready", merged: "merged" }[state];
    el.className = "row" + (state === "conflicts" ? " hot" : "");
    el.innerHTML = '<span class="label">' + (state === "merged" ? "Merged into " : "Merge into ") + esc(DATA.base) + '</span><span class="word" style="--c:' + tone + '">' + esc(word) + "</span>";
  } else {
    const warn = state === "conflicts" && c.conflicts;
    el.className = "row" + (c.author !== "You" ? " dim" : "") + (warn ? " warn" : "");
    const chip = c.chip ? '<span class="chip" style="--c:' + colour[c.lane] + '">' + esc(c.chip) + "</span>" : "";
    const count = warn ? '<span class="count">' + c.conflicts + "</span>" : state !== "conflicts" && c.conflicts && at > 2 ? '<span class="count done">✓</span>' : "";
    el.innerHTML = '<span class="label">' + esc(c.label) + "</span>" + chip + count + '<span class="meta"><code>' + esc(c.hash) + "</code>" + esc(c.author) + " · " + esc(c.age) + "</span>";
  }
  log.appendChild(el);
});
function draw() {
  const h = rows.length * ROW, y = (i) => i * ROW + ROW / 2;
  const merged = state === "merged";
  const lane = (l) => (merged && l === "you" ? "var(--merged)" : colour[l]);
  let out = "";
  const line = (x1, y1, x2, y2, c, dash) => { out += '<path d="M' + x1 + " " + y1 + (x1 === x2 ? " L" + x2 + " " + y2 : " C" + x1 + " " + (y1 + 16) + " " + x2 + " " + (y2 - 16) + " " + x2 + " " + y2) + '" stroke="' + c + '" stroke-width="2" fill="none"' + (dash ? ' stroke-dasharray="3 3"' : "") + "/>"; };
  const fork = Math.max(1, rows.findIndex((c) => c.fork));
  const base = rows.map((c, i) => (c.lane === "base" ? i : -1)).filter((i) => i >= 0);
  if (base.length) { line(X.base, y(base[0]), X.base, y(base[base.length - 1]), lane("base")); line(X.base, y(base[base.length - 1]), X.base, h, lane("base"), true); }
  ["you", "other"].forEach((l) => {
    const mine = rows.map((c, i) => (c.lane === l ? i : -1)).filter((i) => i >= 0);
    if (!mine.length) return;
    line(X[l], y(mine[0]), X[l], y(mine[mine.length - 1]), lane(l));
    line(X[l], y(mine[mine.length - 1]), X.base, y(fork), lane(l));
    if (l === "you") line(X.you, y(0), X.you, y(mine[0]), merged ? "var(--merged)" : tone, !merged);
  });
  rows.forEach((c, i) => {
    if (c.target) {
      const fill = state === "ready" || state === "merged" ? tone : "var(--trek-surface, #fff)";
      out += '<circle cx="' + X.you + '" cy="' + y(i) + '" r="6.5" fill="' + fill + '" stroke="' + tone + '" stroke-width="2"' + (state === "checking" || state === "draft" ? ' stroke-dasharray="2.5 2.5"' : "") + "/>";
      if (state === "conflicts" || state === "resolved") out += '<text x="' + X.you + '" y="' + (y(i) + 3.5) + '" text-anchor="middle" font-size="10" font-weight="800" fill="' + tone + '">!</text>';
      if (state === "ready" || state === "merged") out += '<path d="M' + (X.you - 3) + " " + y(i) + " l2 2.2 l4 -4.4" + '" stroke="#fff" stroke-width="1.8" fill="none" stroke-linecap="round"/>';
      return;
    }
    const warn = state === "conflicts" && c.conflicts;
    const col = warn ? "var(--warn)" : lane(c.lane);
    out += '<circle cx="' + X[c.lane] + '" cy="' + y(i) + '" r="' + (c.fork ? 5 : 4.5) + '" fill="' + col + '" stroke="var(--trek-surface, #fff)" stroke-width="2"/>';
  });
  svg.setAttribute("width", 64);
  svg.setAttribute("height", h);
  svg.innerHTML = out;
}
draw();

// Check chips, with their details on hover.
const checks = DATA.checks || [];
$("checks").hidden = !checks.length;
$("checks").innerHTML = checks.map((c, i) => {
  const mark = { ok: ["✓", "var(--ok)"], fail: ["✕", "var(--trek-negative, #d33)"], running: ['<span class="spin"></span>', "var(--you)"], pending: ["○", "var(--base)"] }[c.state] || ["○", "var(--base)"];
  return '<span class="check' + (i >= checks.length / 2 && checks.length > 1 ? " right" : "") + '" tabindex="0" style="--c:' + mark[1] + '"><i>' + mark[0] + "</i><b>" + esc(c.name) + "</b><span>" + esc(c.result || "") + '</span><span class="tip">' + esc(c.detail || c.result || "") + (c.time ? "<em>" + esc(c.time) + "</em>" : "") + "</span></span>";
}).join("");

// The footer: where it stands, and the one button that moves it on.
const t = DATA.tests;
const say = {
  draft: ["Create PR", "Create the pull request", "Creating PR"],
  conflicts: ["Resolve conflicts", "Resolve the conflicts with " + DATA.base, "Resolving"],
  resolved: ["Run tests", "Run the tests", "Testing"],
  ready: ["Merge", "Merge pull request " + pr, "Merging"]
}[state];
const status = {
  draft: "<b>" + plural(DATA.commits.filter((c) => c.lane === "you").length, "commit") + "</b> on " + esc(DATA.branch) + " to open against " + esc(DATA.base),
  checking: '<span class="spin"></span> Checking it merges cleanly',
  conflicts: '<b style="--c:var(--warn)">⚠</b><span class="conf" id="conf" tabindex="0">' + plural(n, "conflict") + "</span> with " + esc(DATA.base),
  resolved: '<b style="--c:var(--ok)">✓</b> Conflicts resolved, tests not run yet',
  testing: '<span class="spin"></span> ' + (t ? "Running " + t.done + " of " + t.total + " tests" : "Running the tests"),
  ready: '<b style="--c:var(--ok)">✓ Ready to merge</b>',
  merged: '<b style="--c:var(--merged)">Merged into ' + esc(DATA.base) + "</b>"
}[state];
$("status").innerHTML = status;
if (state === "testing" && t && t.total) { $("bar").hidden = false; $("bar").firstChild.style.width = Math.round((100 * t.done) / t.total) + "%"; }
const action = $("action");
if (say) {
  action.innerHTML = '<button class="act">' + esc(say[0]) + "</button>";
  const b = action.firstChild;
  if (!window.trek) b.disabled = true;
  b.onclick = () => {
    if (!window.trek || !window.trek.send(say[1])) return;
    b.disabled = true;
    b.classList.add("busy");
    b.innerHTML = '<span class="spin"></span> ' + esc(say[2]);
    $("status").innerHTML = "Asked: " + esc(say[1]);
  };
} else if (state === "merged" && DATA.url) {
  action.innerHTML = '<a class="act plain" href="' + esc(DATA.url) + '">GitHub ↗</a>';
} else if (state === "checking" || state === "testing") {
  action.innerHTML = '<button class="act busy" disabled><span class="spin"></span> ' + (state === "checking" ? "Checking" : "Testing") + "</button>";
}

// The conflicts, side by side, on hover or tap: keep mine or theirs, one hunk at a time.
const pop = $("pop"), conf = $("conf");
if (conf && n) {
  const files = {};
  DATA.conflicts.forEach((c) => { files[c.file] = (files[c.file] || 0) + 1; });
  const who = (DATA.conflictsWith || "They").split(" ")[0];
  pop.innerHTML = "<h4>" + esc(DATA.conflictsWith || "Someone") + " <span>changed the same lines on " + esc(DATA.base) + "</span></h4>" + DATA.conflicts.map((c, i) => {
    const name = files[c.file] > 1 ? c.file + " line " + c.line : c.file;
    return '<div class="hunk"><div class="where"><b>' + esc(c.file) + "</b> line " + c.line + '</div><div class="side" style="--c:var(--warn)">' + esc(who) + "<code>" + esc(c.theirs) + '</code></div><div class="side" style="--c:var(--you)">You<code>' + esc(c.mine) + '</code></div><div class="keep" data-name="' + esc(name) + '"><button data-keep="mine">Keep mine</button><button data-keep="theirs">Keep theirs</button></div></div>';
  }).join("");
  let hide = 0;
  const open = () => { clearTimeout(hide); pop.classList.add("open"); };
  const close = () => { hide = setTimeout(() => pop.classList.remove("open"), 220); };
  [conf, pop].forEach((el) => { el.addEventListener("mouseenter", open); el.addEventListener("mouseleave", close); });
  conf.addEventListener("click", () => pop.classList.toggle("open"));
  conf.addEventListener("focus", open);
  document.addEventListener("click", (e) => { if (!pop.contains(e.target) && e.target !== conf) pop.classList.remove("open"); });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pop.classList.contains("open")) { pop.classList.remove("open"); e.preventDefault(); } });
  pop.querySelectorAll(".keep button").forEach((b) => {
    b.onclick = () => {
      const keep = b.parentNode;
      if (!window.trek || !window.trek.send("Resolve " + keep.dataset.name + ": keep " + b.dataset.keep)) return;
      keep.innerHTML = '<span class="sent">✓ Asked to keep ' + (b.dataset.keep === "mine" ? "yours" : "theirs") + "</span>";
      resolved += 1;
    };
  });
}
</script>
</body>
</html>
```
