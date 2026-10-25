---
name: database-migration-card
description: A live database migration card - one tab per table, each table's Schema step then its Data step, new and renamed columns highlighted, a scan line down the rows as they're backfilled, and an Apply button; once applied it replays the run with a running clock and ends in "Applied in Ns". Use when you plan, dry-run or apply a schema migration on a real database (Postgres, Supabase, MySQL, SQLite).
---

# Database migration card

A `trek-html` card for **a schema migration**: what it changes, table by table, and, once it has
run, how it went. One tab per table; each table has a **Schema** step (create, alter, rename,
index: the new or renamed columns are highlighted green, unfilled cells show `NULL`) and usually a
**Data** step (the backfill: a green scan line runs down the rows and fills them in; a new table's
rows arrive one by one with a green flash).

- Before it runs (`phase: "plan"`, after a dry run if you can do one) the card shows the plan and
  an **Apply** button that calls `trek.send("Apply the migration")`. Trek posts that as the user's
  next message in this thread (marked as from the card): that message is the go-ahead. Apply it
  only then, and never to production on your own initiative.
- After it ran (`phase: "applied"`) the card replays the run, table by table: the header says
  "Running 3.3s" while the tabs appear one at a time with a progress ring, and it ends in
  "Applied in Ns" with the real time, each table's summary and an **Open in …** link. If it failed,
  `phase: "failed"` with the table it stopped at and the error.

Use it when you write or run a migration (an `@` connector such as Supabase, `psql`, an ORM's
migrate command). Don't use it for a single trivial statement: say it in prose.

## What you fill in

Copy the card into your answer as one fenced `trek-html` block, change the `<title>`, and replace
`DATA` with the real migration. Sample rows come from the database (up to twelve per table, the
first ones by primary key; five show at once and the rest scroll by during the backfill); never
invent values or counts.

- `name` (the migration's), `database`, `connector` (`"Supabase"`, `"Postgres"`…; Supabase gets its
  bolt, others a database icon) and `url` (an https page to open it there, or `null`: the
  **Open in …** link shows only with one).
- `phase`: `"plan"`, `"applied"` or `"failed"`; `seconds`: how long the run took (applied);
  `error`: `{ table, message }` (failed).
- `tables`, in the order they run, at most 8:
  - `name`, `rows` (the table's row count), `summary` (one line for when it's done: "1,284 users,
    each in their workspace"),
  - `columns`: `{ name, type, key, added, renamed, ref, index }`: `key` for the primary key, `added`
    for a new column, `renamed` holding the old name, `ref` for a column whose values are names shown
    with a coloured dot (workspace, team), `index` for a column the migration indexes (shown with a
    bolt once the index step runs; by default the column a `create index` step names),
  - `steps`: `{ kind: "Schema" | "Data", sql }`, one line of SQL each, in the order they run,
  - `before` and `after`: the sample rows (arrays in column order) before and after the migration;
    `null` for an empty cell. A new table has `before: []`.

## Rules

- One card per answer, then a sentence with the takeaway ("Done in **9.9s**. Every user, project
  and message now belongs to a workspace").
- No external scripts or fonts; it stays well under 200 KB.
- It styles itself with Trek's `--trek-*` variables, so it fits Night and Paper.

## The card

```trek-html
<!doctype html>
<html lang="en">
<head>
<title>add_workspaces on acme-prod</title>
<style>
  body { padding: 4px 0 8px; }
  .card {
    --bg: var(--trek-bg, #fff); --fg: var(--trek-fg, #1c1c1c); --mute: var(--trek-muted, #6b6b6b);
    --ok: var(--trek-positive, #1f7a57); --bad: var(--trek-negative, #d33); --green: #3ecf8e;
    --t3: color-mix(in srgb, var(--mute) 70%, var(--bg)); --t4: color-mix(in srgb, var(--mute) 48%, var(--bg));
    --line: color-mix(in srgb, var(--trek-border, #e5e5e5) 72%, transparent);
    --hair: color-mix(in srgb, var(--fg) 6.5%, transparent); --row: color-mix(in srgb, var(--fg) 4%, transparent);
    --band: var(--trek-surface, #f5f5f5); --hover: color-mix(in srgb, var(--fg) 4%, transparent);
    --strip: color-mix(in srgb, var(--green) 6%, var(--bg)); --strip-line: color-mix(in srgb, var(--ok) 9%, var(--bg));
    --flash: color-mix(in srgb, var(--green) 14%, var(--bg)); --tint: color-mix(in srgb, var(--green) 10%, var(--bg));
    position: relative; height: 344px; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 16px; background: var(--bg); color: var(--fg); font-family: var(--trek-font, system-ui); font-size: 13px; line-height: 20px; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,.025); -webkit-font-smoothing: antialiased; animation: enter .25s ease-out both;
  }
  .mono { font-family: var(--trek-mono, ui-monospace, monospace); }
  header { flex: none; display: flex; align-items: center; gap: 10px; height: 60px; padding: 0 16px; }
  .icon { flex: none; width: 30px; height: 30px; border-radius: 9px; display: grid; place-items: center; border: 1px solid var(--line); box-shadow: 0 1px 2px rgba(0,0,0,.04); color: var(--mute); }
  .t { flex: 1; min-width: 0; }
  .t b { display: block; font-size: 14px; font-weight: 500; line-height: 20px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .t span { display: block; color: var(--mute); font-size: 12px; line-height: 16px; }
  .clock { display: flex; align-items: center; gap: 6px; white-space: nowrap; color: var(--mute); font-variant-numeric: tabular-nums; }
  .clock b { color: var(--fg); font-weight: 500; }
  .clock.done, .clock.done b { color: var(--ok); font-weight: 500; }
  .clock.bad, .clock.bad b { color: var(--bad); font-weight: 500; }
  .tabs { flex: none; display: flex; align-items: flex-end; height: 40px; padding: 0 16px; background: var(--band); overflow-x: auto; scrollbar-width: none; }
  .tabs::-webkit-scrollbar { display: none; }
  .tab { position: relative; flex: none; display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 12px; border: 0; border-radius: 10px; background: none; color: var(--mute); font: inherit; white-space: nowrap; cursor: pointer; overflow: hidden; }
  .tab svg { flex: none; color: var(--t3); }
  .tab:not(.on):hover { background: var(--hover); }
  .tab:not(.on) + .tab:not(.on)::before { content: ""; position: absolute; left: 0; top: 9px; width: 1px; height: 14px; background: color-mix(in srgb, var(--fg) 11%, transparent); }
  .tab.on { background: var(--bg); color: var(--fg); font-weight: 500; border-radius: 10px 10px 0 0; cursor: default; }
  .tab.on svg { color: var(--fg); }
  .tab .ok svg { color: var(--ok); }
  .tab .x svg { color: var(--bad); }
  .tab.new { animation: grow .22s ease-out both; }
  .grid { position: relative; flex: 1; min-height: 0; overflow: hidden; -webkit-mask-image: linear-gradient(#000 calc(100% - 20px), transparent); mask-image: linear-gradient(#000 calc(100% - 20px), transparent); }
  .grid.swap { animation: swap .3s ease both; }
  table { border-collapse: collapse; width: 100%; white-space: nowrap; }
  tbody { transition: transform .3s ease; }
  th { position: relative; z-index: 1; height: 33px; padding: 0 12px; background: var(--bg); text-align: left; font-size: 12px; font-weight: 500; color: var(--mute); border-bottom: 1px solid var(--hair); }
  th small { margin-left: 6px; font-size: 11px; font-weight: 400; color: var(--t4); }
  th svg { vertical-align: -2px; margin-right: 6px; }
  th .key { color: var(--t4); }
  .bolt-badge { display: inline-grid; place-items: center; width: 16px; height: 16px; margin-right: 6px; border-radius: 4px; background: color-mix(in srgb, var(--green) 18%, var(--bg)); vertical-align: -3px; }
  .bolt-badge svg { margin: 0; }
  td { height: 31px; padding: 0 12px; border-bottom: 1px solid var(--row); max-width: 190px; overflow: hidden; text-overflow: ellipsis; }
  td.n, th.n { width: 1%; padding: 0 0 0 16px; text-align: right; font-size: 11px; color: var(--t4); font-variant-numeric: tabular-nums; }
  td.id { font-size: 11.5px; color: var(--t3); }
  td.date { font-size: 12px; }
  td.fill, th.fill { width: 100%; padding: 0; }
  th.new, td.new { min-width: 104px; background: var(--strip); box-shadow: inset 1px 0 var(--strip-line), inset -1px 0 var(--strip-line); transition: background .4s; }
  th.new { color: var(--ok); }
  th.new svg { color: var(--ok); }
  .strong th.new, .strong td.new { background: var(--flash); }
  .null { display: inline-block; height: 16px; padding: 0 5px; border: 1px dashed color-mix(in srgb, var(--fg) 18%, transparent); border-radius: 6px; font-size: 9.5px; line-height: 16px; color: var(--t4); letter-spacing: .3px; vertical-align: 1px; }
  .dot { display: inline-block; width: 6px; height: 6px; margin-right: 6px; border-radius: 50%; vertical-align: 1px; }
  .in { animation: blur-in .16s ease-out both; }
  tr.flash td { animation: flash .6s ease-out both; }
  .skel td { height: 31px; }
  .scan { position: absolute; left: 0; right: 0; top: 0; height: 2px; background: var(--green); opacity: 0; pointer-events: none; transition: transform .1s linear, opacity .2s; }
  .scan::before { content: ""; position: absolute; left: 0; right: 0; bottom: 2px; height: 30px; background: linear-gradient(transparent, color-mix(in srgb, var(--green) 16%, transparent)); }
  footer { flex: none; display: flex; align-items: center; gap: 10px; height: 52px; padding: 0 16px; }
  footer > div { flex: 1; min-width: 0; display: flex; align-items: center; gap: 10px; animation: blur-in .18s ease-out both; }
  .kind { flex: none; height: 20px; padding: 0 6px; border-radius: 6px; font-size: 11px; font-weight: 500; line-height: 20px; color: var(--mute); background: var(--band); }
  .kind.data { color: var(--ok); background: var(--tint); }
  .sql { min-width: 0; font-size: 12px; color: var(--mute); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sum { min-width: 0; color: var(--mute); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .ok-mark { flex: none; color: var(--ok); margin-right: -2px; }
  .err { color: var(--bad); white-space: normal; line-height: 16px; font-size: 12.5px; }
  .more { flex: none; color: var(--t3); font-size: 12px; }
  .btn { flex: none; display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 12px; border: 0; border-radius: 13px; font: inherit; font-weight: 500; color: var(--ok); background: var(--tint); text-decoration: none; cursor: pointer; white-space: nowrap; animation: blur-in .18s .1s ease-out both; }
  .btn:hover { background: color-mix(in srgb, var(--green) 16%, var(--bg)); }
  .btn:disabled { cursor: default; color: var(--mute); background: var(--band); }
  .spin { flex: none; animation: spin .8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes enter { from { opacity: 0; filter: blur(4px); } }
  @keyframes blur-in { from { opacity: 0; filter: blur(3px); } }
  @keyframes swap { 0% { opacity: 1; filter: none; } 40% { opacity: 0; filter: blur(3px); } 100% { opacity: 1; filter: none; } }
  @keyframes grow { from { max-width: 0; opacity: 0; } to { max-width: 220px; opacity: 1; } }
  @keyframes flash { from { background: var(--flash); } to { background: transparent; } }
  @media (prefers-reduced-motion: reduce) { .card, .in, footer > div, .btn, .tab.new, .grid.swap, tr.flash td { animation: none; } }
</style>
</head>
<body>
<div class="card" id="card">
  <header>
    <div class="icon" id="icon"></div>
    <div class="t"><b id="name"></b><span id="where"></span></div>
    <div class="clock" id="clock"></div>
  </header>
  <div class="tabs" id="tabs" role="tablist"></div>
  <div class="grid" id="grid"><table id="table"></table><div class="scan" id="scan"></div></div>
  <footer id="foot"></footer>
</div>
<script>
const DATA = {
  name: "add_workspaces",
  database: "acme-prod",
  connector: "Supabase",
  url: null,
  phase: "applied",
  seconds: 9.9,
  error: null,
  tables: [
    { name: "workspaces", rows: 3, summary: "New table, with a workspace for each organisation",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "name", type: "text", ref: true }, { name: "created_at", type: "timestamptz" }],
      steps: [{ kind: "Schema", sql: "create table workspaces (id uuid primary key, name text, created_at timestamptz)" }, { kind: "Data", sql: "insert into workspaces (name) select distinct org from users" }],
      before: [],
      after: [["8c1f02ab", "acme", "2026-10-20 09:14"], ["3e9d71c4", "globex", "2026-10-20 09:14"], ["b4a06e58", "initech", "2026-10-20 09:14"]] },
    { name: "users", rows: 1284, summary: "1,284 users, each in their workspace",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "name", type: "text" }, { name: "email", type: "text" }, { name: "workspace_id", type: "uuid", added: true, ref: true }],
      steps: [{ kind: "Schema", sql: "alter table users add column workspace_id uuid references workspaces" }, { kind: "Data", sql: "update users set workspace_id = workspaces.id from workspaces where workspaces.name = users.org" }],
      before: [["5d2e8a10", "Ava Chen", "ava@acme.co", null], ["c71b4f92", "Liam Patel", "liam@globex.io", null], ["09fa3d6e", "Noah Kim", "noah@acme.co", null], ["e6c05b37", "Mia Rossi", "mia@initech.com", null], ["7a48d1c5", "Leo Novak", "leo@globex.io", null], ["3b9e0f6a", "Zoe Park", "zoe@acme.co", null], ["d0c4a8e2", "Omar Haddad", "omar@initech.com", null], ["81f7b3c9", "Ella Brooks", "ella@globex.io", null]],
      after: [["5d2e8a10", "Ava Chen", "ava@acme.co", "acme"], ["c71b4f92", "Liam Patel", "liam@globex.io", "globex"], ["09fa3d6e", "Noah Kim", "noah@acme.co", "acme"], ["e6c05b37", "Mia Rossi", "mia@initech.com", "initech"], ["7a48d1c5", "Leo Novak", "leo@globex.io", "globex"], ["3b9e0f6a", "Zoe Park", "zoe@acme.co", "acme"], ["d0c4a8e2", "Omar Haddad", "omar@initech.com", "initech"], ["81f7b3c9", "Ella Brooks", "ella@globex.io", "globex"]] },
    { name: "projects", rows: 312, summary: "312 projects moved in; owner is now created_by",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "name", type: "text" }, { name: "created_by", type: "uuid", renamed: "owner" }, { name: "workspace_id", type: "uuid", added: true, ref: true }],
      steps: [{ kind: "Schema", sql: "alter table projects rename column owner to created_by" }, { kind: "Schema", sql: "alter table projects add column workspace_id uuid references workspaces" }, { kind: "Data", sql: "update projects set workspace_id = users.workspace_id from users where users.id = projects.created_by" }],
      before: [["f1e2a4b7", "Website redesign", "Ava Chen", null], ["2c9d8e01", "Billing v2", "Liam Patel", null], ["a7b3c5d9", "Mobile app", "Noah Kim", null], ["64e0f2a8", "Onboarding", "Mia Rossi", null], ["d3c1b7e5", "Help centre", "Leo Novak", null]],
      after: [["f1e2a4b7", "Website redesign", "Ava Chen", "acme"], ["2c9d8e01", "Billing v2", "Liam Patel", "globex"], ["a7b3c5d9", "Mobile app", "Noah Kim", "acme"], ["64e0f2a8", "Onboarding", "Mia Rossi", "initech"], ["d3c1b7e5", "Help centre", "Leo Novak", "globex"]] },
    { name: "messages", rows: 48213, summary: "48,213 messages moved in, and indexed",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "body", type: "text" }, { name: "workspace_id", type: "uuid", added: true, ref: true, index: true }],
      steps: [{ kind: "Schema", sql: "alter table messages add column workspace_id uuid references workspaces" }, { kind: "Data", sql: "update messages set workspace_id = projects.workspace_id from projects where projects.id = messages.project_id" }, { kind: "Schema", sql: "create index concurrently on messages (workspace_id)" }],
      before: [["6be302d5", "Moving standup to 10", null], ["0a1ae2b1", "Pushed the new header", null], ["a850ca59", "Who owns the logo files?", null], ["4688b54d", "Invoices look right now", null], ["e4c29145", "Let's ship it Friday", null]],
      after: [["6be302d5", "Moving standup to 10", "acme"], ["0a1ae2b1", "Pushed the new header", "initech"], ["a850ca59", "Who owns the logo files?", "globex"], ["4688b54d", "Invoices look right now", "acme"], ["e4c29145", "Let's ship it Friday", "initech"]] }
  ]
};

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (k) => Number(k).toLocaleString("en-US");
const S = (w, d, extra) => '<svg width="' + w + '" height="' + w + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"' + (extra || "") + ">" + d + "</svg>";
const ICON = {
  bolt: '<svg width="18" height="18" viewBox="0 0 24 24"><defs><linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3ecf8e"/><stop offset="1" stop-color="#249361"/></linearGradient></defs><path fill="url(#bg)" d="M13.6 2.2c.5-.6 1.4-.2 1.4.5v7.8h5.4c.8 0 1.2.9.7 1.5l-9.7 11.8c-.5.6-1.4.2-1.4-.5v-7.8H4.6c-.8 0-1.2-.9-.7-1.5z"/></svg>',
  db: S(17, '<ellipse cx="12" cy="5.5" rx="7.5" ry="2.8"/><path d="M4.5 5.5v6.5c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V5.5M4.5 12v6.5c0 1.5 3.4 2.8 7.5 2.8s7.5-1.3 7.5-2.8V12"/>'),
  table: S(14, '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M3.5 9.5h17M9 9.5v10"/>'),
  tablePlus: S(14, '<path d="M20.5 12V7a2.5 2.5 0 0 0-2.5-2.5H6A2.5 2.5 0 0 0 3.5 7v10A2.5 2.5 0 0 0 6 19.5h6M3.5 9.5h17M9 9.5v10M18 15v6M15 18h6"/>'),
  key: S(11, '<circle cx="7.5" cy="16.5" r="3.5"/><path d="M10 14l9-9M16 8l2.5 2.5"/>', ' class="key"'),
  link: S(12, '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>'),
  smallBolt: '<svg width="10" height="10" viewBox="0 0 24 24"><path fill="currentColor" d="M13.6 2.2c.5-.6 1.4-.2 1.4.5v7.8h5.4c.8 0 1.2.9.7 1.5l-9.7 11.8c-.5.6-1.4.2-1.4-.5v-7.8H4.6c-.8 0-1.2-.9-.7-1.5z"/></svg>',
  check: S(12, '<path d="M5 12.5l4.5 4.5L19 7.5"/>', ' stroke-width="2.4"'),
  cross: S(12, '<path d="M6 6l12 12M18 6L6 18"/>', ' stroke-width="2.4"'),
  out: S(10, '<path d="M8 16L16 8M9 8h7v7"/>', ' stroke-width="2.4"'),
  spin: '<svg class="spin" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke-width="3"><circle cx="12" cy="12" r="9.5" stroke="currentColor" opacity=".2"/><path d="M12 2.5a9.5 9.5 0 0 1 9.5 9.5" stroke="currentColor" stroke-linecap="round"/></svg>'
};
const ring = (f) => { const c = 2 * Math.PI * 5; return '<svg width="13" height="13" viewBox="0 0 13 13"><circle cx="6.5" cy="6.5" r="5" fill="none" stroke-width="2" stroke="color-mix(in srgb, var(--fg) 10%, transparent)"/><circle cx="6.5" cy="6.5" r="5" fill="none" stroke-width="2" stroke="var(--green)" stroke-linecap="round" stroke-dasharray="' + (c * Math.min(1, Math.max(0.04, f))).toFixed(1) + " " + c.toFixed(1) + '" transform="rotate(-90 6.5 6.5)" style="transition:stroke-dasharray .25s"/></svg>'; };

const tables = DATA.tables.slice(0, 8);
const phase = ["plan", "applied", "failed"].includes(DATA.phase) ? DATA.phase : "plan";
const failedAt = phase === "failed" && DATA.error ? Math.max(0, tables.findIndex((t) => t.name === DATA.error.table)) : -1;
// The coloured dots: the same name has the same colour in every table.
const DOTS = ["#3b82f6", "#d97706", "#8b5cf6", "#14b8a6", "#ec4899", "#65a30d"];
const names = [...new Set(tables.flatMap((t) => t.columns.flatMap((c, k) => (c.ref ? t.after.concat(t.before).map((r) => r[k]) : []))).filter((v) => v != null).map(String))].sort();
const dot = (v) => '<span class="dot" style="background:' + DOTS[Math.max(0, names.indexOf(String(v))) % DOTS.length] + '"></span>';
const isIndex = (s) => /create\s+(unique\s+)?index/i.test(s.sql || "");
const indexed = (t, c) => c.index || t.steps.some((s) => isIndex(s) && new RegExp("\\(\\s*" + c.name + "\\b").test(s.sql));

$("icon").innerHTML = /supabase/i.test(DATA.connector || "") ? ICON.bolt : ICON.db;
$("name").textContent = DATA.name;
$("where").textContent = [DATA.database, tables.length + " table" + (tables.length === 1 ? "" : "s")].filter(Boolean).join(" · ");

// Per table: "idle", "run", "done" or "bad"; `progress` fills the running table's ring.
const status = tables.map(() => (phase === "plan" ? "idle" : "done"));
let shown = 0, upto = phase === "plan" ? tables.length - 1 : -1, progress = 0, running = false;
function tabs(fresh) {
  $("tabs").innerHTML = tables.slice(0, Math.max(upto, phase === "plan" ? tables.length - 1 : upto) + 1).map((t, i) => {
    const s = status[i];
    const mark = s === "run" ? ring(progress) : s === "done" && phase !== "plan" ? '<span class="ok">' + ICON.check + "</span>" : s === "bad" ? '<span class="x">' + ICON.cross + "</span>" : "";
    return '<button class="tab' + (i === shown ? " on" : "") + (i === fresh ? " new" : "") + '" role="tab" aria-selected="' + (i === shown) + '" data-i="' + i + '">' + (t.before.length ? ICON.table : ICON.tablePlus) + esc(t.name) + mark + "</button>";
  }).join("");
  $("tabs").querySelectorAll(".tab").forEach((b) => { b.onclick = () => { if (!running && +b.dataset.i !== shown) show(+b.dataset.i, true); }; });
}

// The grid. `v`: how far the table is: `schema` (new and renamed columns shown), `filled` (rows
// that show their values after the migration), `index` (the index is built), `enter` (columns
// blur in one by one), `flash` (the row that just arrived).
function grid(i, v) {
  const t = tables[i];
  const cols = t.columns.map((c, k) => ({ c, k })).filter(({ c }) => !c.added || v.schema);
  const head = '<tr><th class="n"></th>' + cols.map(({ c }, j) => {
    const isNew = v.schema && (c.added || c.renamed);
    const icon = c.key ? ICON.key : isNew && v.index && indexed(t, c) ? '<span class="bolt-badge" style="color:var(--ok)">' + ICON.smallBolt + "</span>" : isNew ? ICON.link : "";
    const label = c.renamed && !v.schema ? c.renamed : c.name;
    return '<th class="' + (isNew ? "new" : "") + (v.enter ? " in" : "") + '" style="animation-delay:' + j * 0.1 + 's">' + icon + esc(label) + (c.type && !isNew ? "<small class=mono>" + esc(c.type) + "</small>" : "") + "</th>";
  }).join("") + '<th class="fill"></th></tr>';
  const fresh = t.before.length === 0;
  const count = Math.min(12, Math.max(t.after.length, t.before.length));
  let body = "";
  for (let r = 0; r < Math.max(count, 5); r++) {
    if (r >= count || (fresh && r >= v.filled)) { body += '<tr class="skel"><td class="n"></td>' + cols.map(() => "<td></td>").join("") + '<td class="fill"></td></tr>'; continue; }
    const row = r < v.filled ? t.after[r] || [] : t.before[r] || [];
    body += "<tr" + (r === v.flash ? ' class="flash"' : "") + '><td class="n mono">' + (r + 1) + "</td>" + cols.map(({ c, k }, j) => {
      const val = row[k];
      const isNew = v.schema && (c.added || c.renamed);
      const date = typeof val === "string" && /^\d{4}-\d\d-\d\d/.test(val);
      const cls = (c.key ? "id mono " : date ? "date mono " : "") + (isNew ? "new" : "") + (v.enter || (r === v.flash && fresh) ? " in" : "");
      const cell = val === null || val === undefined ? (c.key || !v.schema ? "" : '<span class="null mono">NULL</span>') : (c.ref ? dot(val) : "") + esc(val);
      return '<td class="' + cls + '" style="animation-delay:' + (r === v.flash ? (c.ref ? 0.1 : 0) : j * 0.1) + 's">' + (isNew && r < v.filled && r === v.flash ? '<span class="in">' + cell + "</span>" : cell) + "</td>";
    }).join("") + '<td class="fill"></td></tr>';
  }
  $("table").innerHTML = "<thead>" + head + "</thead><tbody style=\"transform:translateY(" + -offset * 32 + "px)\">" + body + "</tbody>";
  $("grid").classList.toggle("strong", !!v.strong);
}
let offset = 0;
function scroll(r) { offset = r; const b = $("table").tBodies[0]; if (b) b.style.transform = "translateY(" + -r * 32 + "px)"; }

function foot(html) { $("foot").innerHTML = html; }
const stepLine = (s, extra) => '<div><span class="kind' + (s.kind === "Data" ? " data" : "") + '">' + esc(s.kind) + '</span><span class="sql mono" title="' + esc(s.sql) + '">' + esc(s.sql) + "</span>" + (extra || "") + "</div>";
const summary = (t) => '<div><span class="ok-mark">' + ICON.check + '</span><span class="sum">' + esc(t.summary || fmt(t.rows) + " rows") + "</span></div>";
const openLink = () => (/^https:\/\//.test(DATA.url || "") ? '<a class="btn" href="' + esc(DATA.url) + '" target="_blank" rel="noopener">Open in ' + esc(DATA.connector || "the console") + " " + ICON.out + "</a>" : "");

// A table at rest: finished, failed, or (plan) what it will change.
function show(i, animate) {
  shown = i;
  const t = tables[i];
  const g = $("grid");
  tabs();
  if (animate) { g.classList.remove("swap"); void g.offsetWidth; g.classList.add("swap"); }
  setTimeout(() => {
    scroll(0);
    if (phase === "plan") {
      grid(i, { schema: true, filled: 0 });
      foot(stepLine(t.steps[0] || { kind: "Schema", sql: "" }, t.steps.length > 1 ? '<span class="more">+' + (t.steps.length - 1) + " more</span>" : "") + applyButton());
      wireApply();
    } else if (status[i] === "bad") {
      grid(i, { schema: true, filled: 0 });
      foot('<div class="err">' + esc(DATA.error ? DATA.error.message : "Failed") + "</div>");
    } else if (status[i] === "done") {
      // A new table's rows arrive again with their flash.
      grid(i, { schema: true, filled: 12, index: true, flash: t.before.length ? -1 : 99 });
      if (!t.before.length) $("table").querySelectorAll("tbody tr:not(.skel)").forEach((tr) => tr.classList.add("flash"));
      foot(summary(t) + openLink());
    } else {
      grid(i, { schema: false, filled: 0 });
      foot('<div><span class="sum">Not run</span></div>');
    }
  }, animate ? 120 : 0);
}

let applied = false;
function applyButton() { return '<button class="btn" id="apply"' + (applied || !window.trek ? " disabled" : "") + ">" + (applied ? ICON.spin + "Applying" : "Apply") + "</button>"; }
function wireApply() {
  const b = $("apply");
  if (!b) return;
  b.onclick = () => {
    if (applied || !window.trek || !window.trek.send("Apply the migration")) return;
    applied = true;
    b.disabled = true;
    b.innerHTML = ICON.spin + "Applying";
    $("clock").innerHTML = ICON.spin + "Applying";
  };
}

// The replay: each table's tab appears, its steps run in order (Schema shows the new columns,
// Data fills the rows under a scan line, an index step builds the index), the clock runs to the
// real time and stops at "Applied in Ns".
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
async function replay() {
  running = true;
  const last = failedAt >= 0 ? failedAt : tables.length - 1;
  const per = Math.min(2600, Math.max(1500, 10000 / (last + 1)));
  const total = per * (last + 1), start = performance.now();
  const clock = $("clock");
  const tick = () => { const s = Math.min(1, (performance.now() - start) / total) * (DATA.seconds || 0); clock.innerHTML = ICON.spin + "<span>Running <b>" + s.toFixed(1) + "s</b></span>"; };
  tick();
  const timer = setInterval(tick, 100);
  status.fill("idle");
  const scan = $("scan");
  for (let i = 0; i <= last; i++) {
    const t = tables[i];
    const fresh = !t.before.length;
    const rows = Math.min(12, Math.max(t.after.length, t.before.length));
    const v = { schema: false, filled: 0, enter: true, flash: -1 };
    status[i] = "run";
    progress = 0;
    shown = i;
    upto = i;
    tabs(i);
    scroll(0);
    if (i) { $("grid").classList.remove("swap"); void $("grid").offsetWidth; $("grid").classList.add("swap"); await wait(120); }
    grid(i, v);
    v.enter = false;
    const steps = t.steps.length ? t.steps : [{ kind: "Schema", sql: "" }];
    for (let k = 0; k < steps.length; k++) {
      const s = steps[k];
      foot(stepLine(s));
      if (s.kind === "Data") {
        await wait(per * 0.06);
        if (fresh) {
          for (let r = 0; r < rows; r++) { v.filled = r + 1; v.flash = r; grid(i, v); progress = (k + (r + 1) / rows) / steps.length; tabs(); await wait((per * 0.5) / rows); }
          v.flash = -1;
        } else {
          const box = $("grid").clientHeight;
          scan.style.opacity = 1;
          for (let r = 0; r <= rows; r++) {
            v.filled = r; v.flash = -1;
            const top = Math.max(0, Math.min(r - 2, rows - 5));
            scroll(top);
            grid(i, v);
            scan.style.transform = "translateY(" + Math.min(box - 2, 33 + (r - top + 1) * 32) + "px)";
            if (r === rows) scan.style.opacity = 0;
            progress = (k + r / rows) / steps.length;
            tabs();
            await wait((per * 0.55) / (rows + 1));
          }
          scan.style.opacity = 0;
          v.strong = true; grid(i, v);
          await wait(per * 0.08);
          v.strong = false; grid(i, v);
        }
      } else if (isIndex(s)) {
        v.index = true; v.strong = true; grid(i, v);
        await wait(per * 0.25);
        v.strong = false;
      } else {
        await wait(per * 0.12);
        v.schema = true; grid(i, v);
        $("table").querySelectorAll("th.new, td.new").forEach((c) => c.classList.add("in"));
        if (failedAt === i) break;
        await wait(per * 0.12);
      }
      progress = (k + 1) / steps.length;
      tabs();
    }
    if (failedAt === i) { status[i] = "bad"; break; }
    status[i] = "done";
    scroll(0);
    grid(i, Object.assign(v, { filled: 12, index: true, strong: false }));
    tabs();
    foot(summary(t));
    await wait(per * 0.12);
  }
  clearInterval(timer);
  running = false;
  if (phase === "failed") { clock.className = "clock bad"; clock.innerHTML = ICON.cross + "<span>Failed at <b>" + esc(tables[last].name) + "</b></span>"; }
  else { clock.className = "clock done"; clock.innerHTML = ICON.check + "Applied in " + Number(DATA.seconds || 0).toFixed(1) + "s"; }
  upto = phase === "failed" ? last : tables.length - 1;
  show(phase === "failed" ? last : tables.length - 1);
}

if (phase === "plan") {
  $("clock").innerHTML = "Not applied";
  show(0);
} else {
  replay();
}
</script>
</body>
</html>
```
