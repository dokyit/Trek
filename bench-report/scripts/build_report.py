#!/usr/bin/env python3
"""Builds bench-report/index.html from bench-report/results/*.json and research/field/*.csv.
Run from anywhere: python3 bench-report/scripts/build_report.py"""
import csv, json, os, re
from pathlib import Path
from report_lib import CSS, esc, fmt, hbar, table

HERE = Path(__file__).resolve().parent
REP = HERE.parent
RES = REP / "results"


def load(name, default=None):
    p = RES / name
    return json.load(open(p)) if p.exists() else default


apps = load("apps.json", {})
perf = load("trek-perf.json", {})
stress = load("trek-stress.json", {})
tokens = load("tokens-hi.json", {})
ax_trek = load("trek-accessibility.json", {})
ax_pass = load("ax-pass.json", {})
real = load("trek-real-use.json", {})
design_a = load("design-blind-a.json", {})
design_b = load("design-blind-b.json", {})
tb = load("terminal-bench.json", {})
real10 = load("trek-real10.json", {})
boards = load("leaderboards.json", [])
subs = load("subscriptions.json", {})
# Codex, Devin and Gemini CLI first failed to launch (flags their versions reject, workspace trust); they
# were run again the same night with fixed flags, and those runs replace the failed ones.
for _app, _r in (load("subscriptions-rerun.json", {}) or {}).items():
    if isinstance(_r, dict) and "runs" in _r:
        subs[_app] = {**_r, "rerun": True}
tbl = load("tb-local.json", {})
tbo = load("tb-openrouter.json", {})
ademark = list(csv.DictReader(open(REP / "research/field/ademark.csv")))
features = list(csv.DictReader(open(REP / "research/field/features.csv")))
from report_text import make_text, SHOTS  # noqa: E402


def med(app, key):
    return (apps.get(app, {}).get("median") or {}).get(key)


# ---------------------------------------------------------------- performance charts
APP_ORDER = [a for a in apps if not apps[a].get("skipped") and apps[a].get("median") and "baseline" not in a]
trek_idle = perf.get("idle", {}).get("100_threads", {})
trek_launch = perf.get("launch", {})


def perf_rows(key, trek_value, trek_note):
    rows = [(f"Trek (main)", trek_value, trek_note)]
    for a in APP_ORDER:
        v = med(a, key)
        if v is not None:
            rows.append((a, v, ""))
    return rows


baseline_launch = [med(a, "launch_ms") for a in apps if "baseline" in a and med(a, "launch_ms")]
ls_floor = min(baseline_launch) if baseline_launch else None

chart_mem = hbar(perf_rows("footprint_mb", trek_idle.get("footprint_mb"), "100 threads, empty profile")
                 + ([("Trek (owner's, real use)", real.get("trek_main_footprint_mb"), "4.7 h, 782 threads, 37 tabs")] if real else []),
                 "Memory at idle, all of the app's processes (footprint, MB; lower is better)", " MB",
                 note="Competitors: the owner's own profile as installed, 25 s after launch, summed over every process the app is "
                      "responsible for (renderers, GPU, helpers, any agent server it starts at launch), median of 3. Trek: an isolated "
                      "profile with 100 seeded threads, no agent running (docs/PERFORMANCE.md method). The owner's real Trek after 4 h 41 m "
                      f"with 782 threads: {real.get('trek_main_footprint_mb', '–')} MB.")
chart_launch = hbar(perf_rows("launch_ms", trek_launch.get("warm_100_threads_ms"), "spawn to first frame"),
                    "Launch to first window (ms; lower is better)", " ms",
                    note="Competitors: `open -g` to the first on-screen window, median of 3 warm launches. Trek: process spawn to its first "
                         "painted frame (no Launch Services). " + (f"The `open -g` path itself costs at least {fmt(ls_floor)} ms "
                         "(Calculator/TextEdit, same method), so add about that to Trek's bar for a like-for-like reading." if ls_floor else ""))
chart_cpu = hbar(perf_rows("idle_cpu_pct", trek_idle.get("cpu_percent"), "window behind others"),
                 "CPU while idle (% of one core; lower is better)", "%", digits=2,
                 note="Competitors: 30 s, 25 s after launch, window launched in the background (may be partly visible). Trek: 60 s, window "
                      "behind others (macOS stops drawing it), so Trek's figure is a best case; on screen it measured 0.13–0.20% in docs/PERFORMANCE.md.")
chart_proc = hbar(perf_rows("processes", 1, "+ about 6 per agent session"), "Processes at idle (lower is better)", "",
                  note="Trek runs one process until a thread starts; each Claude Code session then adds the CLI (≈255 MB) and Trek's MCP helpers (≈2 MB each).")

# ADEMark published (their machine), for context
def ade_val(row, col):
    v = row.get(col, "")
    try:
        return float(v)
    except ValueError:
        return None


ade_rows_mem = [(r["app"], ade_val(r, "A5/total memory at rest total (MB)"), "") for r in ademark]
ade_rows_hi = [(r["app"], ade_val(r, "B1/total hello tokens (hi) (tokens)"), "") for r in ademark]
trek_hi = None
if tokens.get("runs"):
    tr = [r["input_total"] for r in tokens["runs"] if r["run"].startswith("trek")]
    br = [r["input_total"] for r in tokens["runs"] if r["run"].startswith("bare-")]
    trek_hi, bare_hi = sorted(tr)[0], sorted(br)[0]
    # Place Trek on ADEMark's scale by its measured difference from the bare CLI (both measured here).
    ade_bare = next((ade_val(r, "B1/total hello tokens (hi) (tokens)") for r in ademark if r["app"] == "Bare CLI"), None)
    if ade_bare:
        ade_rows_hi.append(("Trek (est.)", ade_bare + (trek_hi - bare_hi), "bare CLI + Trek's measured Δ"))
chart_hi = hbar(ade_rows_hi, "Tokens sent for “hi” (ADEMark's numbers; Trek placed by its measured difference)", "",
                note="ADEMark measured 15 apps on a Mac mini M4 (12 Nov 2026). Trek is not in ADEMark; here its 'hi' was measured with Claude Code "
                     f"2.1.x on this Mac with Trek's exact flags: {fmt(trek_hi)} tokens vs {fmt(bare_hi) if trek_hi else '–'} for the bare CLI with the same "
                     "user config, then placed on ADEMark's scale as the bare CLI's ADEMark value plus that difference.")

# ---------------------------------------------------------------- stress
thr = stress.get("threads", {})
swarm = stress.get("swarm", {})
tabs = stress.get("browser", {})


def g(d, *ks):
    for k in ks:
        if not isinstance(d, dict):
            return None
        d = d.get(k)
    return d


thread_rows = []
for n, r in sorted(((k, v) for k, v in thr.items() if k.isdigit()), key=lambda kv: int(kv[0])):
    thread_rows.append([f"{int(n):,}", fmt(r.get("launch_ms"), " ms"), fmt(r.get("footprint_mb"), " MB"), fmt(r.get("drawn_footprint_mb"), " MB"),
                        fmt(r.get("idle_cpu_pct"), "%", 2), f'{g(r, "sidebar_scroll", "frame_p50") or "–"} / {g(r, "sidebar_scroll", "frame_p95") or "–"} ms',
                        f'{g(r, "open_thread", "painted") or "–"} ms', f'{g(r, "typing", "latency_p50") or "–"} / {g(r, "typing", "latency_p95") or "–"} ms',
                        fmt(r.get("load"), "", 1)])
swarm_rows = []
for k, r in swarm.items():
    if not isinstance(r, dict) or "agents" not in r:
        continue
    kind, n = k.rsplit("-", 1)
    swarm_rows.append([("Streaming answers" if kind == "stream" else "Sub-agents (delegate)") + f" × {n}", fmt(r.get("footprint_before_mb"), " MB"),
                       fmt(r.get("footprint_peak_mb"), " MB"), fmt(r.get("cpu_pct_of_one_core"), "%", 1),
                       (f'{g(r, "frames", "frame_p50")} / {g(r, "frames", "frame_p95")} ms' if g(r, "frames", "frames") not in (None, "0") else "nothing to draw"), f'{g(r, "typing_during", "latency_p95") or "–"} ms',
                       f'{g(r, "open_thread_during", "painted") or "–"} ms', "yes" if r.get("all_turns_ended") is True else esc(str(r.get("all_turns_ended")))[:60],
                       esc(g(r, "sessions_line", "alive") or "–"), fmt(r.get("load"), "", 1)])
tab_rows = []
for n, r in sorted(((k, v) for k, v in tabs.items() if k.isdigit()), key=lambda kv: int(kv[0])):
    tab_rows.append([n, fmt(r.get("new_tab_ms_p50"), " ms"), fmt(r.get("new_tab_ms_last10_p50"), " ms"), fmt(r.get("trek_footprint_mb"), " MB"),
                     f'{r.get("webkit_processes", "–")} / {fmt(r.get("webkit_footprint_mb"), " MB")}', fmt(r.get("total_footprint_mb"), " MB"),
                     f'{g(r, "typing", "latency_p95") or "–"} ms', esc(str(r.get("tabs_kept")))[:60], fmt(r.get("load"), "", 1)])

chart_thr = hbar([(f"Trek, {int(n):,} threads", r.get("footprint_mb"), f"scroll p95 {g(r, 'sidebar_scroll', 'frame_p95')} ms") for n, r in thr.items() if n.isdigit()],
                 "Trek's memory at idle by threads in the sidebar (footprint, MB)", " MB", note="A thread costs about 9 KB of footprint at idle; the 10,000-thread launch was 221 ms.")
chart_tabs = hbar([(f"Trek, {int(n)} tabs", r.get("total_footprint_mb"), f"{r.get('webkit_processes')} WebKit processes") for n, r in tabs.items() if n.isdigit()],
                  "Trek plus its WebKit processes by open Browser tabs (MB)", " MB", note="About 20 MB per live tab; every tab kept its own page process (no tab was suspended).")

# ---------------------------------------------------------------- agent quality (three free sources)
def quality_html():
    out = []
    # 1. published boards
    rows = []
    for b in boards:
        src = f'<a href="{esc(b.get("url", ""))}">{esc(b.get("source_name") or b.get("benchmark"))}</a>, {esc(b.get("date", ""))}'
        rows.append({"cls": "trek" if str(b.get("app", "")).startswith("Trek") else "", "cells": [
            esc(b.get("app", "")), esc(f'{b.get("benchmark", "")} {b.get("version") or ""}'.strip()), esc(b.get("model", "") or "–"),
            f'<b>{esc(b.get("score", ""))}</b>{esc(b.get("unit", "") or "")}', src, esc(b.get("status", "")), esc((b.get("note") or "")[:140])]})
    full = json.load(open(REP / "research/field/leaderboards-verified.json")) if (REP / "research/field/leaderboards-verified.json").exists() else []
    full_rows = [[esc(x.get("app")), esc(f'{x.get("benchmark")} {x.get("version") or ""}'), esc(x.get("model") or "–"), esc(f'{x.get("score")} {x.get("unit") or ""}'),
                  f'<a href="{esc(x.get("url"))}">link</a>, {esc(x.get("date"))}', esc(x.get("status"))] for x in full]
    out.append("<h3>1. Published leaderboards (the headline numbers)</h3>" + T["quality_boards_intro"]
               + table(["App / harness", "Benchmark", "Model", "Score", "Source and date", "Checked", "Note"], rows)
               + f"<details><summary>All {len(full)} verified rows, with every board's link and date</summary>"
               + table(["App", "Benchmark", "Model", "Score", "Source", "Checked"], full_rows) + "</details>")
    # 2. each app on its own plan
    rows = []
    for app, r in (subs.items() if isinstance(subs, dict) else []):
        if not isinstance(r, dict) or "runs" not in r:
            continue
        runs = r["runs"]
        n = len(runs)
        solved = sum(1 for x in runs if x.get("passed"))
        tp = sum(x.get("tests_passed", 0) for x in runs)
        tt = sum(x.get("tests_total", 0) for x in runs)
        secs = sorted(x.get("secs", 0) for x in runs)
        # From each app's own config where it says (the parsed output named Claude Code's background model).
        models = [{"Claude Code": "Opus (the plan default in ~/.claude/settings.json)", "Trek (Claude Code inside Trek)": "Opus 5.5 (Trek's Claude Code default)",
                   "Codex": "GPT-6 Astra, medium effort (~/.codex/config.toml)"}.get(app, "plan default, not reported")]
        rows.append({"cls": "trek" if app.startswith("Trek") else "", "cells": [
            esc(app), f"<b>{solved} / {n}</b>", f"{tp} / {tt}", f"{secs[len(secs)//2] if secs else '–'} s", esc(", ".join(models)[:80] or "plan default"),
            esc(r.get("stopped", "") or "–"), "this Mac, 27–28 Nov" + (" (rerun after a launch-flag fix)" if r.get("rerun") else "")]})
    out.append("<h3>2. Each app on its own plan (not the same model)</h3>" + T["quality_subs_intro"]
               + table(["App", "Exercises solved", "Unit tests passed", "Median time", "Model reported", "Stopped early", "Source"], rows, "data nums"))
    # 3. same local model
    runs3 = tbo.get("runs", []) if isinstance(tbo, dict) else []
    def tests(x):
        t = x.get("tests") or ""
        return tuple(int(v) for v in t.split("/")) if "/" in t else None
    base = {x["task"]: tests(x) for x in runs3 if x["harness"] == "nop" and tests(x)}
    by = {}
    for x in runs3:
        if x["harness"] != "nop":
            by.setdefault(x["harness"], []).append(x)
    rows = []
    for h in ["trek-direct", "opencode", "droid", "claude-code", "codex"]:
        rs = [x for x in by.get(h, []) if x["task"] in base]
        if not rs:
            continue
        solved = sum(1 for x in rs if (x.get("reward") or 0) >= 1)
        gained = lost = tp = tt = 0
        for x in rs:
            t = tests(x)
            b = base[x["task"]]
            if t:
                tp += t[0]; tt += t[1]
                d = t[0] - b[0]
                gained += max(d, 0); lost += max(-d, 0)
        better = sum(1 for x in rs if tests(x) and tests(x)[0] > base[x["task"]][0])
        capped = sum(1 for x in rs if "timed out" in (x.get("exception") or ""))
        errs = sum(1 for x in rs if x.get("exception") and "timed out" not in x["exception"])
        name = {"trek-direct": "Trek (Direct harness)", "opencode": "OpenCode", "droid": "Factory Droid (BYOK)", "claude-code": "Claude Code (via LiteLLM)", "codex": "Codex"}[h]
        rows.append({"cls": "trek" if h.startswith("trek") else "", "cells": [esc(name), str(len(rs)), f"<b>{solved}</b>", f"{better}", f"+{gained} / −{lost}", f"{tp} / {tt}", str(capped), str(errs)]})
    bt = sum(v[0] for v in base.values()); btt = sum(v[1] for v in base.values())
    spent = (tbo.get("spend_end") or {}).get("spent_usd", tbo.get("spent_usd")) if isinstance(tbo, dict) else None
    out.append("<h3>3. One model, five harnesses (the same-model head-to-head)</h3>" + T["quality_local_intro"]
               + table(["Harness (source: this Mac, DeepSeek V4 Flash via OpenRouter)", "Tasks run", "Tasks passed", "Tasks improved", "Tests gained / lost vs the untouched task", "Tests passing after", "Hit the 8-min cap", "Errors"], rows, "data nums")
               + f"<p class='sub'>The untouched tasks (Harbor's nop agent, no model) already pass {bt} of {btt} verifier tests across the {len(base)} tasks; a harness that makes no change scores exactly that. "
               + (f"Spent on OpenRouter: ${spent:.2f}." if isinstance(spent, (int, float)) else "") + "</p>"
               + T["quality_local_after"])
    return "\n".join(out)


# ---------------------------------------------------------------- accessibility
ax_rows = []
for a in APP_ORDER + [x for x in ax_pass if x not in APP_ORDER]:
    first = (apps.get(a, {}).get("runs") or [{}])[0].get("accessibility") or {}
    second = ax_pass.get(a) or {}
    best = second if (second.get("controls") or 0) >= (first.get("controls") or 0) else first
    if not best:
        continue
    c, nc = best.get("controls"), best.get("named_controls")
    ax_rows.append((a, c, nc, best.get("static_texts"), best.get("nodes")))
t = ax_trek.get("trek_main_isolated_empty_thread", {})
ax_rows.append(("Trek (main)", t.get("controls"), t.get("named_controls"), t.get("static_texts"), t.get("nodes")))
FIRST_RUN = {"Claude", "Conductor", "Capy", "Warp", "super.engineering", "Paseo"}
ax_table = table(["App", "Screen probed", "Accessibility nodes", "Controls", "Controls with a name", "Text a screen reader can read"],
                 [{"cls": "trek" if r[0].startswith("Trek") else "", "cells": [esc(r[0]), "sign-in / first run" if r[0] in FIRST_RUN else "working window", fmt(r[4]), fmt(r[1]), fmt(r[2]), fmt(r[3])]}
                  for r in sorted(ax_rows, key=lambda r: -(r[4] or 0))])

# ---------------------------------------------------------------- feature grid
FEAT_COLS = [("platforms", "Platforms"), ("free_tier", "Free"), ("open_source", "Open source"), ("runs_third_party_agents", "Other vendors' agents"),
             ("own_or_api_or_local_models", "API / local models"), ("parallel_agents", "Parallel"), ("worktrees", "Worktrees"),
             ("agent_drives_browser", "Agent drives browser"), ("computer_use", "Computer use"), ("mobile_companion", "Phone"),
             ("sub_agents_orchestration", "Sub-agents"), ("stack", "Stack")]
feat_rows = []
for r in features[:20]:
    cells = [esc(r["product"])] + [esc((r.get(k) or "")[:60]) for k, _ in FEAT_COLS]
    feat_rows.append({"cls": "trek" if r["product"] == "Trek" else "", "cells": cells})
feat_table = table(["Product"] + [h for _, h in FEAT_COLS], feat_rows)

# ---------------------------------------------------------------- terminal-bench
tb_html = ""

# ---------------------------------------------------------------- screenshots
shots_html = ""
for item in SHOTS:
    shots_html += f'<figure><img src="{esc(item["src"])}" alt="{esc(item["alt"])}" loading="lazy"><figcaption>{item["caption"]}</figcaption></figure>\n'

design_rows = []
if design_a:
    for L in design_a["ranking"]:
        s = design_a["scores"][L]
        name = design_a["key"][L]
        design_rows.append({"cls": "trek" if name.startswith("Trek") else "", "cells": [L, esc(name)] + [str(x) for x in s]})
design_table = table(["Image", "Product (revealed after judging)", "Hierarchy", "Type", "Colour", "Density", "Craft", "Calm", "Distinct", "Overall"], design_rows)
design_b_table = ""
if design_b:
    rows = []
    for L in design_b["ranking"]:
        s = design_b["scores"][L]
        name = design_b["key"][L]
        rows.append({"cls": "trek" if name.startswith("Trek") else "", "cells": [L, esc(name)] + [str(x) for x in s]})
    design_b_table = table(["Image", "Product (revealed after judging)", "Hierarchy", "Type", "Colour", "Density", "Craft", "Calm", "Distinct", "Overall"], rows)


def section(id_, title, body):
    return f'<section id="{id_}"><h2>{title}</h2>\n{body}\n</section>'


T = make_text(dict(real10=real10, apps=apps, perf=perf, stress=stress, tokens=tokens, real=real, tb=tb, ax_pass=ax_pass, design_a=design_a, design_b=design_b, ls_floor=ls_floor, trek_hi=trek_hi))
html_out = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Trek vs the field</title>
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter+Tight:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&family=Newsreader:opsz,wght@6..72,400;6..72,600&display=swap">
<style>{CSS}</style></head><body><main>
<header class="top">
<div class="eyebrow">Competitive benchmark · {esc(T['date'])} · Trek {esc(T['trek_version'])} against {esc(T['n_competitors'])} apps</div>
<h1>{T['title']}</h1>
<p class="lede">{T['lede']}</p>
<nav class="toc">{''.join(f'<a href="#{a}">{esc(b)}</a>' for a, b in T['toc'])}</nav>
</header>
<div class="verdict"><p>{T['verdict']}</p></div>
<div class="tiles">{''.join(f'<div class="tile {c}"><div class="v">{v}</div><div class="k">{k}</div></div>' for v, k, c in T['tiles'])}</div>
{section('scorecard', 'Scorecard', T['scorecard_intro'] + table(T['scorecard_headers'], [{'cls': 'trek' if r[0].startswith('<b>Trek') else '', 'cells': r} for r in T['scorecard_rows']], 'data score') + T['scorecard_after'])}
{section('why', 'Why you should, and shouldn’t, use Trek', '<div class="pros"><section><h3>Why you should</h3><ol>' + ''.join(f'<li>{x}</li>' for x in T['why']) + '</ol></section><section><h3>Why you shouldn’t</h3><ol>' + ''.join(f'<li>{x}</li>' for x in T['why_not']) + '</ol></section></div>')}
{section('performance', 'Performance', T['perf_intro'] + '<div class="grid2">' + chart_mem + chart_launch + '</div><div class="grid2">' + chart_cpu + chart_proc + '</div>' + T['perf_after'] + T['claims_html'])}
{section('scale', 'Scale: thousands of threads, hundreds of tabs', T['scale_intro'] + '<h3>Threads in the sidebar</h3>' + table(['Threads', 'Launch', 'Footprint idle', 'Footprint drawn', 'Idle CPU', 'Sidebar scroll frame p50/p95', 'Open the oldest thread', 'Typing latency p50/p95', 'Load'], thread_rows, 'data nums') + '<div class="grid2">' + chart_thr + chart_tabs + '</div>' + '<h3>Browser tabs</h3>' + table(['Tabs', 'New tab p50', 'Last 10 tabs p50', 'Trek footprint', 'WebKit processes / footprint', 'Total', 'Typing p95', 'Tabs kept', 'Load'], tab_rows, 'data nums') + T['scale_after'])}
{section('swarm', 'Swarming: many agents at once', T['swarm_intro'] + table(['Swarm', 'Footprint before', 'Footprint peak', 'Trek CPU', 'Frame p50/p95', 'Typing p95', 'Open a thread', 'All finished', 'Sessions alive', 'Machine load'], swarm_rows, 'data nums') + T['swarm_after'])}
{section('quality', 'Agent quality: three free sources', quality_html())}
{section('tokens', 'Tokens and harness quality', T['tokens_intro'] + chart_hi + T['tb_html'] + T['tokens_after'])}
{section('features', 'Features', T['features_intro'] + feat_table + T['features_after'])}
{section('ux', 'Beginner UX and accessibility', T['ux_intro'] + ax_table + T['ux_after'])}
{section('design', 'Design: a blind comparison', T['design_intro'] + '<div class="shots">' + ''.join(f'<figure><img src="blind/a/{L}.png" alt="Unlabelled image {L}" loading="lazy"><figcaption>Image {L}</figcaption></figure>' for L in sorted(design_a.get('key', {}))) + '</div>' + design_table + T['design_mid'] + '<div class="shots">' + ''.join(f'<figure><img src="blind/b/{L}.png" alt="Unlabelled first screen {L}" loading="lazy"><figcaption>Image {L}</figcaption></figure>' for L in sorted(design_b.get('key', {}))) + '</div>' + design_b_table + T['design_b_after'] + '<div class="shots">' + shots_html + '</div>' + T['design_after'])}
{section('posts', 'The posts you sent', T['posts_intro'] + table(['#', 'Post', 'What it shows', 'Claim', 'How Trek compares'], T['posts_rows']))}
{section('method', 'Method and limits', T['method'])}
<footer>{T['footer']}</footer>
</main></body></html>"""
html_out = re.sub(r"`([^`<]+)`", r"<code>\1</code>", html_out)
(REP / "index.html").write_text(html_out)
print("wrote", REP / "index.html", len(html_out) // 1024, "KB")
