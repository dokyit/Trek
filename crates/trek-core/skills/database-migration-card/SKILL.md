---
name: database-migration-card
description: A live database migration card - one tab per table, each table's Schema step then its Data step, new and renamed columns highlighted, a sweep down the rows as they're backfilled, and an Apply button; once applied it replays the run and ends in "Applied in Ns". Use when you plan, dry-run or apply a schema migration on a real database (Postgres, Supabase, MySQL, SQLite).
---

# Database migration card

A `trek-html` card for **a schema migration**: what it changes, table by table, and, once it has
run, how it went. One tab per table; each table has a **Schema** step (create, alter, rename,
index: the new or renamed columns are highlighted green, unfilled cells show `NULL`) and usually a
**Data** step (the backfill: a green line sweeps down the rows and fills them in).

- Before it runs (`phase: "plan"`, after a dry run if you can do one) the card shows the plan and
  an **Apply** button that calls `trek.send("Apply the migration")`. Trek posts that as the user's
  next message in this thread (marked as from the card): that message is the go-ahead. Apply it
  only then, and never to production on your own initiative.
- After it ran (`phase: "applied"`) the card replays the run, table by table, and ends in
  "Applied in Ns" with the real time and each table's summary. If it failed, `phase: "failed"`
  with the table it stopped at and the error.

Use it when you write or run a migration (an `@` connector such as Supabase, `psql`, an ORM's
migrate command). Don't use it for a single trivial statement: say it in prose.

## What you fill in

Copy the card into your answer as one fenced `trek-html` block, change the `<title>`, and replace
`DATA` with the real migration. Sample rows come from the database (five at most per table, the
first ones by primary key); never invent values or counts.

- `name` (the migration's), `database`, `connector` (`"Supabase"`, `"Postgres"`…) and `url` (an
  https page to open it there, or `null`).
- `phase`: `"plan"`, `"applied"` or `"failed"`; `seconds`: how long the run took (applied);
  `error`: `{ table, message }` (failed).
- `tables`, in the order they run, at most 8:
  - `name`, `rows` (the table's row count), `summary` (one line for when it's done: "1,284 users,
    each in their workspace"),
  - `columns`: `{ name, type, key, added, renamed, ref }`: `key` for the primary key, `added` for
    a new column, `renamed` holding the old name, `ref` for a column whose values are names shown
    with a coloured dot (workspace, team),
  - `steps`: `{ kind: "Schema" | "Data", sql }`, one line of SQL each,
  - `before` and `after`: the sample rows (arrays in column order) before and after the migration;
    `null` for an empty cell. A new table has `before: []`.

## Rules

- One card per answer, then a sentence with the takeaway ("Done in 9.9 s: every user, project and
  message now belongs to a workspace").
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
  .card { --ok: var(--trek-positive, #1f9d55); --bad: var(--trek-negative, #d33); position: relative; border: 1px solid var(--trek-border, #e5e5e5); border-radius: 16px; background: var(--trek-surface, #fff); color: var(--trek-fg, #1d1d1f); font-family: var(--trek-font, system-ui); font-size: 13.5px; overflow: hidden; }
  header { display: flex; align-items: center; gap: 12px; padding: 14px 16px 12px; }
  .icon { width: 36px; height: 36px; border-radius: 10px; display: grid; place-items: center; border: 1px solid var(--trek-border, #e5e5e5); color: var(--ok); flex: none; }
  .t { flex: 1; min-width: 0; }
  .t b { display: block; font-size: 15px; font-weight: 650; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .t span { color: var(--trek-muted, #888); font-size: 12.5px; }
  .clock { white-space: nowrap; color: var(--trek-muted, #888); font-size: 12.5px; font-variant-numeric: tabular-nums; }
  .clock b { color: var(--trek-fg, #222); font-weight: 650; }
  .clock.done, .clock.done b { color: var(--ok); }
  .clock.bad, .clock.bad b { color: var(--bad); }
  .tabs { display: flex; gap: 2px; padding: 4px; margin: 0 12px; border-radius: 10px; background: color-mix(in srgb, var(--trek-fg, #000) 5%, transparent); overflow-x: auto; scrollbar-width: none; }
  .tab { display: flex; align-items: center; gap: 6px; border: 0; background: none; color: var(--trek-muted, #888); font: inherit; font-size: 12.5px; padding: 6px 10px; border-radius: 7px; cursor: pointer; white-space: nowrap; }
  .tab:hover { background: color-mix(in srgb, var(--trek-fg, #000) 6%, transparent); }
  .tab.on { background: var(--trek-surface, #fff); color: var(--trek-fg, #222); box-shadow: 0 1px 3px rgba(0,0,0,.12); }
  .tab .ok { color: var(--ok); font-weight: 700; }
  .tab .bad { color: var(--bad); font-weight: 700; }
  .grid { position: relative; margin: 8px 0 0; overflow-x: auto; transition: opacity .18s; }
  .grid.swap { opacity: 0; }
  table { border-collapse: collapse; width: 100%; font-size: 12.5px; }
  th { text-align: left; font-weight: 600; padding: 8px 10px; white-space: nowrap; position: relative; }
  th small { font-weight: 400; color: var(--trek-muted, #888); margin-left: 5px; font-size: 11px; }
  th svg { vertical-align: -2px; margin-right: 4px; }
  td { height: 36px; padding: 0 10px; border-top: 1px solid var(--trek-border, #eee); white-space: nowrap; max-width: 180px; overflow: hidden; text-overflow: ellipsis; }
  td.n, th.n { color: var(--trek-muted, #888); width: 1%; text-align: right; font-variant-numeric: tabular-nums; }
  td.id { font-family: var(--trek-mono, ui-monospace); color: var(--trek-muted, #888); font-size: 12px; }
  .new { color: var(--ok); }
  th.new, td.new { background: linear-gradient(color-mix(in srgb, var(--ok) 11%, transparent), color-mix(in srgb, var(--ok) 3%, transparent)); }
  tr.writing td { background: color-mix(in srgb, var(--ok) 12%, transparent); }
  .null { font-family: var(--trek-mono, ui-monospace); font-size: 10.5px; color: var(--trek-muted, #888); border: 1px dashed var(--trek-border, #ccc); border-radius: 4px; padding: 1px 5px; }
  .val { animation: fill .35s ease both; }
  .dot { display: inline-block; width: 7px; height: 7px; border-radius: 50%; margin-right: 6px; vertical-align: 1px; }
  .sweep { position: absolute; left: 0; right: 0; height: 2px; background: var(--ok); box-shadow: 0 0 8px var(--ok); opacity: 0; pointer-events: none; transition: top .25s linear, opacity .2s; }
  .empty td { color: var(--trek-muted, #888); text-align: center; }
  footer { display: flex; align-items: center; gap: 10px; padding: 12px 16px 14px; border-top: 1px solid var(--trek-border, #eee); min-height: 30px; }
  .step { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; }
  .kind { font-size: 11px; font-weight: 600; padding: 2px 7px; border-radius: 5px; color: var(--trek-muted, #888); background: color-mix(in srgb, var(--trek-fg, #000) 7%, transparent); }
  .kind.data { color: var(--ok); background: color-mix(in srgb, var(--ok) 13%, transparent); }
  .sql { font-family: var(--trek-mono, ui-monospace); font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--trek-fg, #222); }
  .count { color: var(--trek-muted, #888); font-size: 12px; white-space: nowrap; font-variant-numeric: tabular-nums; }
  .sum { color: var(--trek-muted, #888); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sum b { color: var(--ok); }
  .err { color: var(--bad); white-space: normal; }
  .btn { border: 0; border-radius: 999px; padding: 7px 14px; font: inherit; font-size: 12.5px; font-weight: 600; cursor: pointer; white-space: nowrap; color: var(--ok); background: color-mix(in srgb, var(--ok) 14%, transparent); text-decoration: none; }
  .btn:hover { background: color-mix(in srgb, var(--ok) 22%, transparent); }
  .btn:disabled { cursor: default; color: var(--trek-muted, #888); background: color-mix(in srgb, var(--trek-fg, #000) 7%, transparent); }
  .spin { width: 11px; height: 11px; border-radius: 50%; border: 1.6px solid currentColor; border-right-color: transparent; display: inline-block; animation: spin .8s linear infinite; vertical-align: -1px; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes fill { from { opacity: 0; filter: blur(2px); } }
</style>
</head>
<body>
<div class="card">
  <header>
    <div class="icon"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v6c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 11v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6"/></svg></div>
    <div class="t"><b id="name"></b><span id="where"></span></div>
    <div class="clock" id="clock"></div>
  </header>
  <div class="tabs" id="tabs" role="tablist"></div>
  <div class="grid" id="grid"><table id="table"></table><div class="sweep" id="sweep"></div></div>
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
      before: [["5d2e8a10", "Ava Chen", "ava@acme.co", null], ["c71b4f92", "Liam Patel", "liam@globex.io", null], ["09fa3d6e", "Noah Kim", "noah@acme.co", null], ["e6c05b37", "Mia Rossi", "mia@initech.com", null], ["7a48d1c5", "Leo Novak", "leo@globex.io", null]],
      after: [["5d2e8a10", "Ava Chen", "ava@acme.co", "acme"], ["c71b4f92", "Liam Patel", "liam@globex.io", "globex"], ["09fa3d6e", "Noah Kim", "noah@acme.co", "acme"], ["e6c05b37", "Mia Rossi", "mia@initech.com", "initech"], ["7a48d1c5", "Leo Novak", "leo@globex.io", "globex"]] },
    { name: "projects", rows: 312, summary: "312 projects moved in; owner is now created_by",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "name", type: "text" }, { name: "created_by", type: "uuid", renamed: "owner" }, { name: "workspace_id", type: "uuid", added: true, ref: true }],
      steps: [{ kind: "Schema", sql: "alter table projects rename column owner to created_by; alter table projects add column workspace_id uuid references workspaces" }, { kind: "Data", sql: "update projects set workspace_id = users.workspace_id from users where users.id = projects.created_by" }],
      before: [["0a91c2d4", "Website redesign", "Ava Chen", null], ["5f3e8b17", "Billing v2", "Liam Patel", null], ["c2d74a90", "Mobile app", "Noah Kim", null], ["91be0f36", "Onboarding", "Mia Rossi", null], ["e47a2c58", "Help centre", "Leo Novak", null]],
      after: [["0a91c2d4", "Website redesign", "Ava Chen", "acme"], ["5f3e8b17", "Billing v2", "Liam Patel", "globex"], ["c2d74a90", "Mobile app", "Noah Kim", "acme"], ["91be0f36", "Onboarding", "Mia Rossi", "initech"], ["e47a2c58", "Help centre", "Leo Novak", "globex"]] },
    { name: "messages", rows: 48213, summary: "48,213 messages moved in, and indexed",
      columns: [{ name: "id", type: "uuid", key: true }, { name: "body", type: "text" }, { name: "workspace_id", type: "uuid", added: true, ref: true }],
      steps: [{ kind: "Schema", sql: "alter table messages add column workspace_id uuid references workspaces" }, { kind: "Data", sql: "update messages set workspace_id = projects.workspace_id from projects where projects.id = messages.project_id" }, { kind: "Schema", sql: "create index concurrently on messages (workspace_id)" }],
      before: [["b81f4c20", "Moving standup to 10", null], ["2c9e7a14", "Pushed the new header", null], ["f04d61b8", "Who owns the logo files?", null], ["6ab3e092", "Invoices look right now", null], ["d5c2f871", "Let's ship it Friday", null]],
      after: [["b81f4c20", "Moving standup to 10", "acme"], ["2c9e7a14", "Pushed the new header", "initech"], ["f04d61b8", "Who owns the logo files?", "globex"], ["6ab3e092", "Invoices look right now", "acme"], ["d5c2f871", "Let's ship it Friday", "initech"]] }
  ]
};

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fmt = (k) => Number(k).toLocaleString("en-US");
const ICON = {
  table: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M9 10v10"/></svg>',
  key: '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="8" cy="15" r="4"/><path d="M11 12l9-9M17 6l3 3"/></svg>',
  link: '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1"/></svg>'
};
const DOTS = ["#2f6be8", "#e8772e", "#8957e5", "#14a39a", "#d6457a", "#9a8a12"];
const hue = {};
const dot = (v) => { if (!(v in hue)) hue[v] = DOTS[Object.keys(hue).length % DOTS.length]; return '<span class="dot" style="background:' + hue[v] + '"></span>'; };
const tables = DATA.tables;
const phase = ["plan", "applied", "failed"].includes(DATA.phase) ? DATA.phase : "plan";
const failedAt = phase === "failed" && DATA.error ? Math.max(0, tables.findIndex((t) => t.name === DATA.error.table)) : -1;
// Per table: "idle", "run", "done" or "bad".
const status = tables.map(() => (phase === "plan" ? "idle" : "done"));
let shown = 0;

$("name").textContent = DATA.name;
$("where").textContent = [DATA.database, tables.length + " table" + (tables.length === 1 ? "" : "s"), DATA.connector].filter(Boolean).join(" · ");

function tabs() {
  $("tabs").innerHTML = tables.map((t, i) => {
    const s = status[i];
    const mark = s === "run" ? '<span class="spin" style="color:var(--ok)"></span>' : s === "done" && phase !== "plan" ? '<span class="ok">✓</span>' : s === "bad" ? '<span class="bad">✕</span>' : "";
    return '<button class="tab' + (i === shown ? " on" : "") + '" role="tab" aria-selected="' + (i === shown) + '" data-i="' + i + '">' + ICON.table + esc(t.name) + mark + "</button>";
  }).join("");
  $("tabs").querySelectorAll(".tab").forEach((b) => { b.onclick = () => { if (!running) show(+b.dataset.i); }; });
}

// The table: `filled` rows show their values after the migration, the rest as before; new
// columns are green, unfilled cells NULL.
function grid(i, filled, schema, writing) {
  const t = tables[i];
  const head = '<tr><th class="n"></th>' + t.columns.map((c) => {
    const isNew = schema && (c.added || c.renamed);
    if (c.added && !schema) return "";
    const icon = c.key ? ICON.key : isNew ? ICON.link : "";
    return '<th class="' + (isNew ? "new" : "") + '">' + icon + esc(c.name) + (c.type && !isNew ? "<small>" + esc(c.type) + "</small>" : "") + (c.renamed && schema ? "<small>was " + esc(c.renamed) + "</small>" : "") + "</th>";
  }).join("") + "</tr>";
  const fresh = t.before.length === 0;
  const count = Math.max(fresh ? t.after.length : t.before.length, 1);
  let body = "";
  for (let r = 0; r < Math.min(count, 5); r++) {
    if (fresh && r >= filled) { body += '<tr class="empty"><td class="n"></td><td colspan="' + t.columns.length + '"></td></tr>'; continue; }
    const row = r < filled ? t.after[r] || [] : t.before[r] || [];
    body += "<tr" + (r === writing ? ' class="writing"' : "") + '><td class="n">' + (r + 1) + "</td>" + t.columns.map((c, k) => {
      if (c.added && !schema) return "";
      const v = row[k];
      const isNew = schema && (c.added || c.renamed);
      const cls = (c.key ? "id " : "") + (isNew ? "new" : "");
      const cell = v === null || v === undefined ? '<span class="null">NULL</span>' : (c.ref ? dot(v) : "") + esc(v);
      return '<td class="' + cls + '">' + (r < filled && isNew ? '<span class="val">' + cell + "</span>" : cell) + "</td>";
    }).join("") + "</tr>";
  }
  $("table").innerHTML = "<thead>" + head + "</thead><tbody>" + body + "</tbody>";
}

function foot(html) { $("foot").innerHTML = html; }
const stepLine = (s, extra) => '<div class="step"><span class="kind' + (s.kind === "Data" ? " data" : "") + '">' + esc(s.kind) + '</span><span class="sql" title="' + esc(s.sql) + '">' + esc(s.sql) + "</span></div>" + (extra || "");
const openLink = () => (DATA.url ? '<a class="btn" href="' + esc(DATA.url) + '">Open in ' + esc(DATA.connector || "the console") + " ↗</a>" : "");

// A finished table, or the plan for one.
function show(i) {
  shown = i;
  const g = $("grid");
  g.classList.add("swap");
  setTimeout(() => {
    const t = tables[i];
    tabs();
    if (phase === "plan") {
      grid(i, 0, true, -1);
      foot(stepLine(t.steps[0] || { kind: "Schema", sql: "" }, '<span class="count">' + (t.steps.length > 1 ? "+" + (t.steps.length - 1) + " more" : "") + "</span>") + applyButton());
      wireApply();
    } else if (status[i] === "bad") {
      grid(i, 0, true, -1);
      foot('<div class="step err">✕ ' + esc(DATA.error ? DATA.error.message : "Failed") + "</div>");
    } else if (status[i] === "done") {
      grid(i, 5, true, -1);
      foot('<div class="step sum"><b>✓</b> ' + esc(t.summary || fmt(t.rows) + " rows") + "</div>" + openLink());
    } else {
      grid(i, 0, true, -1);
      foot('<div class="step sum">Not run</div>');
    }
    g.classList.remove("swap");
  }, 120);
}

let applied = false;
function applyButton() { return '<button class="btn" id="apply"' + (applied || !window.trek ? " disabled" : "") + ">" + (applied ? '<span class="spin"></span> Applying' : "Apply") + "</button>"; }
function wireApply() {
  const b = $("apply");
  if (!b) return;
  b.onclick = () => {
    if (applied || !window.trek || !window.trek.send("Apply the migration")) return;
    applied = true;
    b.disabled = true;
    b.innerHTML = '<span class="spin"></span> Applying';
    $("clock").innerHTML = "Asked to apply";
  };
}

// The replay: each table's Schema step, then its Data step with a sweep down the rows; the clock
// runs to the real time, and stops at "Applied in Ns".
let running = false;
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
async function replay() {
  running = true;
  const last = failedAt >= 0 ? failedAt : tables.length - 1;
  const per = Math.min(2200, Math.max(1200, 9000 / (last + 1)));
  const total = per * (last + 1), start = performance.now();
  const clock = $("clock");
  const tick = setInterval(() => { const s = Math.min(1, (performance.now() - start) / total) * (DATA.seconds || 0); clock.innerHTML = '<span class="spin"></span> Running <b>' + s.toFixed(1) + "s</b>"; }, 100);
  status.fill("idle");
  const sweep = $("sweep");
  for (let i = 0; i <= last; i++) {
    const t = tables[i];
    status[i] = "run";
    shown = i;
    tabs();
    grid(i, 0, false, -1);
    const schema = t.steps.find((s) => s.kind !== "Data") || t.steps[0];
    const data = t.steps.find((s) => s.kind === "Data");
    if (schema) foot(stepLine(schema));
    await wait(per * 0.3);
    grid(i, 0, true, -1);
    if (failedAt === i) { status[i] = "bad"; break; }
    await wait(per * 0.15);
    if (data) {
      const rows = Math.min(5, Math.max(t.after.length, t.before.length));
      sweep.style.opacity = 1;
      for (let r = 0; r <= rows; r++) {
        grid(i, r, true, r < rows ? r : -1);
        const tr = $("table").tBodies[0].rows[Math.min(r, rows - 1)];
        if (tr) sweep.style.top = tr.offsetTop + (r < rows ? 0 : tr.offsetHeight) + "px";
        foot(stepLine(data, '<span class="count">' + fmt(Math.round((t.rows * r) / rows)) + " of " + fmt(t.rows) + "</span>"));
        await wait((per * 0.5) / (rows + 1));
      }
      sweep.style.opacity = 0;
    } else {
      await wait(per * 0.5);
    }
    const index = t.steps.filter((s) => s !== schema && s !== data)[0];
    if (index) foot(stepLine(index));
    await wait(per * 0.05);
    status[i] = "done";
  }
  clearInterval(tick);
  running = false;
  if (phase === "failed") { clock.className = "clock bad"; clock.innerHTML = "✕ Failed at <b>" + esc(tables[last].name) + "</b>"; status.forEach((s, i) => { if (i > last) status[i] = "idle"; }); }
  else { clock.className = "clock done"; clock.innerHTML = "✓ Applied in <b>" + Number(DATA.seconds || 0).toFixed(1) + "s</b>"; }
  show(phase === "failed" ? last : tables.length - 1);
}

if (phase === "plan") {
  $("clock").innerHTML = "Not applied";
  show(0);
} else {
  tabs();
  replay();
}
</script>
</body>
</html>
```
