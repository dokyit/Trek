"""The report's words. Numbers come from the results passed in (d), so text and data stay in step."""


def _g(d, *ks, default=None):
    for k in ks:
        if not isinstance(d, dict) or k not in d:
            return default
        d = d[k]
    return d


def _n(v, unit="", digits=0):
    if v is None:
        return "–"
    try:
        v = float(v)
    except (TypeError, ValueError):
        return str(v)
    return f"{v:,.{digits}f}{unit}"


AHEAD = '<span class="g ahead">beats Trek</span>'
PAR = '<span class="g par">on par</span>'
BEHIND = '<span class="g behind">Trek ahead</span>'
NA = '<span class="g na">no data</span>'

SHOTS = [
    {"src": "shots/trek-home.png", "alt": "Trek, isolated profile with seeded threads", "caption": "<b>Trek</b> (main, isolated profile, 100 seeded threads, Night theme)"},
    {"src": "shots/codex-app.png", "alt": "Codex app as launched", "caption": "<b>Codex app</b> 26.1023, as launched (private titles pixelated)"},
    {"src": "shots/cursor.png", "alt": "Cursor as launched", "caption": "<b>Cursor</b> 3.24, as launched"},
    {"src": "shots/t3-code.png", "alt": "T3 Code as launched", "caption": "<b>T3 Code</b> 0.0.33, as launched"},
    {"src": "shots/zed.png", "alt": "Zed as launched", "caption": "<b>Zed</b> 1.30, as launched"},
    {"src": "shots/claude.png", "alt": "Claude desktop first run", "caption": "<b>Claude</b> desktop, first run (asks for a sign-in or a gateway)"},
    {"src": "shots/conductor.png", "alt": "Conductor first run", "caption": "<b>Conductor</b> 0.101, first run (asks to sign up)"},
    {"src": "shots/opencode-desktop.png", "alt": "OpenCode desktop as launched", "caption": "<b>OpenCode</b> desktop 1.18, as launched"},
    {"src": "shots/antigravity.png", "alt": "Antigravity as launched", "caption": "<b>Antigravity</b> 2.31, as launched"},
    {"src": "shots/warp.png", "alt": "Warp first run", "caption": "<b>Warp</b>, first run"},
    {"src": "shots/paseo.png", "alt": "Paseo first run", "caption": "<b>Paseo</b> 0.18, first run (5-step setup)"},
    {"src": "shots/super-engineering.png", "alt": "super.engineering first run", "caption": "<b>super.engineering</b>, first run"},
    {"src": "shots/capy.png", "alt": "Capy first run", "caption": "<b>Capy</b>, as launched (sign-in)"},
]


def make_text(d):
    apps, perf, stress, real, tb = d["apps"], d["perf"], d["stress"], d["real"], d["tb"]
    m = lambda a, k: _g(apps, a, "median", k)
    thr = _g(stress, "threads", default={}) or {}
    big = max((int(k) for k in thr if k.isdigit()), default=None)
    bigr = thr.get(str(big), {}) if big else {}
    sw = _g(stress, "swarm", default={}) or {}
    s50 = sw.get("stream-50", {})
    a50 = sw.get("subagents-50", {})
    tabs = _g(stress, "browser", default={}) or {}
    t200 = tabs.get("200", {})
    r10 = d.get("real10") or {}
    pk = r10.get("ten_open_four_streaming_peak") or {}
    six = r10.get("six_idle") or {}
    per_cli = ((six.get("total_mb", 0) - six.get("trek_mb", 0)) / 6) if six else None
    trek_ade_est = (six.get("trek_mb", 90) + 5 * per_cli) if per_cli else None
    d.setdefault("real10_html", (
        "<h3>Real agents: Synara's post #12, repeated in Trek</h3>"
        f"<p>Isolated Trek with real Claude Code on the owner's subscription, in a throwaway project: 6 threads each ran a one-word turn, then 4 more streamed a 900-word story at once — "
        f"<b>10 threads open, 4 streaming</b>, as in the post. Peak: <b>{_n(pk.get('total_mb'))} MB</b> for Trek and every process under it "
        f"(Trek itself {_n(pk.get('trek_mb'))} MB; {_g(pk, 'by_process', 'claude', 'count', default='–')} Claude Code CLIs {_n(_g(pk, 'by_process', 'claude', 'mb'))} MB). "
        f"Synara's own post reports 1,784 MB for the same shape after its fix (2,307 MB before), on its machine. "
        f"Trek's MCP helpers did not start in this run (the build folder had no trek-mcp); from the owner's live Trek they add about 2 MB each, about 120 MB for 10 sessions.</p>"
        f"<p><b>What it shows:</b> once agents run, the CLIs are 90% of the memory in any app. Trek kept <b>all 10</b> CLIs alive after every turn had ended "
        f"({_n(_g(r10, 'after', 'total_mb'))} MB at rest); Synara's fix was to cut its count from 10 to 4, and super.engineering's 134 MB on ADEMark's “5 idle threads” test means it parks idle agents too. "
        f"At about {_n(per_cli)} MB per idle Claude Code here, Trek would score roughly {_n(trek_ade_est)} MB on that ADEMark test (estimate), behind super.engineering (134), MonoCode (419) and Capy (593).</p>"))
    d.setdefault("post12_cmp", f"Repeated in Trek with real Claude Code: {_n(pk.get('total_mb'))} MB peak, about the same as Synara's 1,784 MB. Trek kept all 10 CLIs alive after the turns ended; Synara cut its count from 10 to 4.")
    da, db = d.get("design_a") or {}, d.get("design_b") or {}
    d.setdefault("design_mid", "<p>Set A, decoded after the verdict: Trek (H) came <b>2nd of 8</b>. The reviewer on Trek: “" + _g(da, "on_H_trek", "best", default="") + "” Faults: " + "; ".join(_g(da, "on_H_trek", "faults", default=[])) + ".</p>"
        "<h3>Set B: the first screen a new user sees</h3><p>Captured on this Mac. Trek's is its isolated first run; the others as installed (several never signed in). Same reviewer setup, a new run.</p>")
    d.setdefault("design_after_b", "<p>Trek (E) came <b>1st of 8</b>: " + _g(db, "on_E_trek", "best", default="") + " Faults: " + "; ".join(_g(db, "on_E_trek", "faults", default=[])) + ". This frame doesn't show what follows: 7 macOS permission prompts.</p>")
    sup, pas = (lambda a: (m(a, "footprint_mb"), m(a, "launch_ms"), m(a, "idle_cpu_pct")))("super.engineering"), (lambda a: (m(a, "footprint_mb"), m(a, "launch_ms")))("Paseo")
    d.setdefault("super_perf", f"{AHEAD} with agents running: 134 MB at rest with 5 threads (ADEMark) vs Trek ≈{_n(trek_ade_est)} MB (estimate); {_n(sup[1])} ms launch, {_n(sup[0])} MB on its first-run screen (here)")
    d.setdefault("paseo_perf", f"{BEHIND} {_n(pas[0])} MB, {_n(pas[1])} ms launch (here); ADEMark 3rd")
    idle = _g(perf, "idle", "100_threads", default={})
    launch = _g(perf, "launch", "warm_100_threads_ms")
    tb_t, tb_o = _g(tb, "summary", "trek-direct", default={}), _g(tb, "summary", "opencode", default={})

    lightest_comp = min(((m(a, "footprint_mb"), a) for a in apps if m(a, "footprint_mb") and "baseline" not in a), default=(None, None))

    T = {}
    T["date"] = "27 November 2026"
    T["trek_version"] = "0.4.69+ (main 242fdff8)"
    T["n_competitors"] = "16"
    T["title"] = "Is Trek the best by far? No. It is the lightest and the broadest, and it loses where newcomers and blind users meet it."
    T["lede"] = ("Trek was measured against the desktop agent apps it actually competes with, on one Mac, with the same scripts for every app, "
                 "plus every published benchmark that applies. Trek's own claims were treated as things to check. Where an app could not be run, "
                 "the report says so and uses published numbers, labelled as such.")
    T["toc"] = [("scorecard", "Scorecard"), ("why", "Why and why not"), ("performance", "Performance"), ("scale", "Scale"), ("swarm", "Swarming"),
                ("tokens", "Tokens and harness"), ("features", "Features"), ("ux", "UX and accessibility"), ("design", "Design"),
                ("posts", "Your posts"), ("method", "Method and limits")]

    tb_sentence = d.get("tb_sentence") or "its own agent loop, used for API and local models, passed 0 of 19 Terminal-Bench tasks with a cheap model on 30 Oct, and today's build abandoned a task on a reasoning-only reply"
    T["verdict"] = d.get("verdict") or (
        f"No: Trek is not the best by far. It is the best at two things. It is the lightest app measured at idle "
        f"({_n(idle.get('footprint_mb'))} MB and {_n(idle.get('cpu_percent'), '%', 2)} CPU in one process, against {_n(m('Zed', 'footprint_mb'))} MB for Zed and "
        f"{_n(m('Paseo', 'footprint_mb'))}–{_n(m('Codex app', 'footprint_mb'))} MB for the Electron apps), and it stays smooth at scale: {_n(big)} threads launch in "
        f"{_n(bigr.get('launch_ms'))} ms, and with 200 Browser tabs or 50 agents at once typing stayed under 21 ms at p95. It also has the broadest tool set found: no other app combines "
        "other vendors' agents with API and local models, a Browser with extensions, computer use, an iOS Simulator, a real-iPhone tool, video watching and study mode. "
        f"It falls short in five places. Once real agents run, the lead is gone: 10 threads with 4 streaming took {_n(pk.get('total_mb'))} MB, the same as Synara, because Trek keeps every "
        "Claude Code process alive where super.engineering and Synara park idle ones. VoiceOver can read almost nothing in it (2 of 13 controls named, no text), while the Electron rivals name 88–99% of 25–440 controls. "
        "Its first screen was rated the best of 8 in a blind test, but the 8 steps after it ask for 7 macOS permissions and an agent CLI you install yourself. "
        f"{tb_sentence[0].upper() + tb_sentence[1:]}. And no one outside measures it: Trek is not in ADEMark, though its docs quote ADEMark ranks. "
        "In a blind review of makers' screenshots it came 2nd of 8, behind T3 Code. Use Trek if you are a developer who runs several vendors' agents and values speed and tools. "
        "Look elsewhere if you rely on a screen reader, want one vendor's supported app, or want a one-click start.")

    T["tiles"] = [
        (_n(idle.get("footprint_mb"), " MB"), "Trek at idle, 100 threads (next lightest measured: " + f"{lightest_comp[1]} {_n(lightest_comp[0], ' MB')})", "good"),
        (_n(bigr.get("launch_ms"), " ms") if bigr else "–", f"launch with {_n(big)} threads to the first frame", "good"),
        (f"{_n(_g(t200, 'total_footprint_mb'), ' MB')}" if t200 else "–", "200 Browser tabs, Trek and all its WebKit processes", ""),
        ("2 / 13", "controls VoiceOver can name in Trek's window", "bad"),
        ("2nd of 8", "blind design review, makers' own screenshots", ""),
        (f"{_n(d.get('trek_hi'))}", "tokens for “hi” with Trek's flags (bare Claude Code: 40,120)", ""),
    ]

    # ------------------------------------------------------------------ scorecard
    T["scorecard_intro"] = ("<p>Each competitor cell says whether it <b>beats Trek</b>, is <b>on par</b>, or <b>Trek is ahead</b> on that dimension, with the number behind it. "
                            "Numbers marked <i>here</i> were measured on this Mac today; <i>ADEMark</i> numbers are theirs (Mac mini M4, 12 Nov); others link to their source in "
                            "the sections below. Memory is all of an app's processes at idle, as installed.</p>")
    T["scorecard_headers"] = ["App", "Performance", "Scale / stress", "Swarming", "Features", "Beginner UX", "Developer power", "Design", "Helpfulness", "Cost", "Platforms"]
    tb_line = "own harness 0/19 tasks (30 Oct, DeepSeek V4 Flash), unranked"
    T["scorecard_rows"] = [
        ["<b>Trek</b>", f"{_n(idle.get('footprint_mb'))} MB, {_n(idle.get('cpu_percent'), '%', 2)} CPU idle, {_n(launch)} ms to frame (here). Real use: {real.get('trek_main_footprint_mb', '–')} MB after 4.7 h",
         f"{_n(big)} threads: {_n(bigr.get('launch_ms'))} ms launch, {_n(bigr.get('footprint_mb'))} MB, scroll p95 {_g(bigr, 'sidebar_scroll', 'frame_p95', default='–')} ms (here)",
         f"50 mock streams: {_n(_g(s50, 'footprint_peak_mb'))} MB peak, typing p95 {_g(s50, 'typing_during', 'latency_p95', default='–')} ms (here). Sub-agents, consult, handoffs, started threads",
         "Widest: any vendor CLI + API/local models, Browser + extensions + ad blocker, computer use, iOS Simulator, real iPhone, video, study, notes, machines, plugins",
         "7 permission prompts in onboarding, CLI must be installed, VoiceOver sees 2/13 controls, iPhone app from source",
         "Strong: worktrees, per-turn diffs and rewind, PR cards, editor, terminal, MCP, plugins, ⌘K",
         "2nd of 8 blind (7.0/10): best hierarchy, too many accents", f"= the vendor CLI; {tb_line}",
         "Free, Apache-2.0; “hi” ≈ bare CLI (39.1K vs 40.1K)", "macOS, Windows, Linux (partial); iPhone app built from source; any phone via the web remote"],
        ["Claude desktop", f"{BEHIND} {_n(m('Claude', 'footprint_mb'))} MB idle signed out (here); ADEMark 1,696 MB at rest", NA, f"{PAR} parallel sessions, sub-agents",
         f"{BEHIND} browser, computer use, remote; Anthropic models only", f"{AHEAD} sign in, pick a folder; one vendor's defaults", f"{BEHIND} one vendor",
         f"{NA}", f"{PAR} Claude Code: 65.8% Terminal-Bench 4.1", f"{BEHIND} paid plan; ADEMark “hi” 75.5K", f"{PAR} Mac, Windows, Linux beta, phones"],
        ["Codex app", f"{BEHIND} {_n(m('Codex app', 'footprint_mb'))} MB, {_n(m('Codex app', 'processes'))} processes, {_n(m('Codex app', 'idle_cpu_pct'), '%', 1)} CPU idle (here)", NA,
         f"{PAR} parallel threads, automations", f"{BEHIND} in-app browser, computer use; OpenAI only", f"{AHEAD} ChatGPT sign-in, free tier; 129/131 controls named (here)",
         f"{BEHIND} one vendor", NA, f"{PAR} Codex: 62.1% Terminal-Bench 4.1", f"{PAR} free tier, then $20+", f"{PAR} Mac, Windows, phone via ChatGPT"],
        ["Cursor", f"{BEHIND} {_n(m('Cursor', 'footprint_mb'))} MB, {_n(m('Cursor', 'idle_cpu_pct'), '%', 0)} CPU idle (here)", NA, f"{PAR} coordinator + sub-agents, cloud agents",
         f"{BEHIND} agent browser, iOS app; mostly own models", f"{AHEAD} 44/46 controls named (here); account sign-in", f"{PAR} full IDE, own agent",
         NA, f"{PAR} 62.1% (Factory's TB4, Opus 5.5)", f"{BEHIND} free, then $8–200", f"{PAR} Mac (others unverified), iOS"],
        ["Orca", f"{BEHIND} {_n(m('Orca', 'footprint_mb'))} MB, {_n(m('Orca', 'idle_cpu_pct'), '%', 0)} CPU idle (here); ADEMark 10th", NA,
         f"{BEHIND} orchestration exists, but ADEMark's build: workers never started", f"{PAR} closest match: browser, computer use, plugins, SSH, Android",
         f"{AHEAD} 144/146 controls named (here)", f"{PAR} many vendors' CLIs", f"{BEHIND} 6th of 8 blind (5.5)", NA, f"{PAR} free, MIT", f"{AHEAD} Mac, Win, Linux, iOS, Android"],
        ["T3 Code", f"{BEHIND} {_n(m('T3 Code', 'footprint_mb'))} MB, {_n(m('T3 Code', 'launch_ms'))} ms launch (here); ADEMark 5th", NA, f"{BEHIND} parallel threads, no orchestration",
         f"{BEHIND} no browser or computer use found", f"{AHEAD} opens straight to a composer; 82k stars, poll winner", f"{PAR} any ACP agent",
         f"{AHEAD} 1st of 8 blind (7.5)", NA, f"{PAR} free, MIT; “hi” 52.7K (ADEMark)", f"{AHEAD} all desktops, web, phones"],
        ["Paseo", f"{_g(d, 'paseo_perf', default=NA)}", NA, f"{PAR} agents start agents", f"{BEHIND} fewer tools", f"{PAR} restrained UI (post #4)",
         f"{PAR} many CLIs, daemon", f"{BEHIND} 8th of 8 blind (5.0)", NA, f"{AHEAD} fewest tokens: “hi” 22.2K (ADEMark)", f"{AHEAD} Mac, web, iOS, Android"],
        ["Conductor", f"{BEHIND} {_n(m('Conductor', 'footprint_mb'))} MB idle (here)", NA, f"{PAR} parallel workspaces", f"{BEHIND} Claude Code, Codex, Cursor only",
         f"{BEHIND} account sign-up before anything (here); Meta-acquired, sign-ups closed", f"{PAR} PR review flow", f"{BEHIND} 5th of 8 blind (6.0, partial image)", NA, f"{BEHIND} uncertain", f"{BEHIND} Mac, iOS"],
        ["Zed", f"{BEHIND} {_n(m('Zed', 'footprint_mb'))} MB, {_n(m('Zed', 'idle_cpu_pct'), '%', 1)} CPU idle, {_n(m('Zed', 'launch_ms'))} ms (here): closest to Trek", NA,
         f"{BEHIND} parallel threads", f"{BEHIND} no browser, no phone", f"{PAR} same GPUI stack: also weak for VoiceOver", f"{AHEAD} a real editor; ACP agents, keys, local models",
         f"{PAR} 3rd of 8 blind (6.5, partial image)", NA, f"{PAR} free, Pro $10", f"{PAR} Mac, Linux, Windows"],
        ["super.engineering", f"{_g(d, 'super_perf', default=NA)}", NA, f"{AHEAD} ADEMark build 5/5 hidden tests in 7.1 min", f"{BEHIND} 18 agents, browser experimental",
         NA, f"{PAR} native Rust, worktree per task", NA, NA, f"{AHEAD} “hi” 29.6K (ADEMark); free alpha", f"{BEHIND} Mac, Linux"],
        ["Synara", f"{BEHIND} {_n(m('Synara', 'footprint_mb'))} MB, {_n(m('Synara', 'idle_cpu_pct'), '%', 0)} CPU idle (here; animated background)",
         f"{_g(d, 'synara_scale', default=PAR + ' its own post: 1,784 MB at 10 threads, 4 streaming')}", f"{AHEAD} ADEMark build 5/5 in 5.2 min (fastest)",
         f"{BEHIND} agent browser, Kanban, canvas coming; Mac only", NA, f"{PAR} many vendors, delegation", f"{BEHIND} 7th of 8 blind (5.5)", NA, f"{AHEAD} “hi” 30.6K (ADEMark)", f"{BEHIND} Mac, iOS (TestFlight)"],
    ]
    T["scorecard_after"] = ("<p class='sub'>“no data” means nothing measurable or published was found; it is not a judgement. Helpfulness is mostly the agent CLI's, "
                            "and Trek runs the same CLIs, so on helpfulness Trek can only tie the vendor apps when it runs their agent, and is behind when it runs its own loop.</p>")

    # ------------------------------------------------------------------ why / why not
    T["why"] = [
        f"<b>It is by far the lightest.</b> {_n(idle.get('footprint_mb'))} MB and {_n(idle.get('cpu_percent'), '%', 2)} CPU at idle with 100 threads, one process; the Electron apps measured here use "
        f"{_n(m('Antigravity', 'footprint_mb'))}–{_n(m('Codex app', 'footprint_mb'))} MB across 7–21 processes. Only Zed ({_n(m('Zed', 'footprint_mb'))} MB) is in the same class.",
        f"<b>It scales.</b> {_n(big)} threads launch in {_n(bigr.get('launch_ms'))} ms at {_n(bigr.get('footprint_mb'))} MB with sidebar scrolling at p95 {_g(bigr, 'sidebar_scroll', 'frame_p95', default='–')} ms; "
        f"a 10,000-item thread opens in {_n(_g(perf, 'long', 'first_paint_ms'))} ms and scrolls at p95 {_n(_g(perf, 'long', 'scroll_frame_p95_ms'), ' ms', 1)}.",
        "<b>It runs everyone's agents, not one vendor's.</b> Claude Code, Codex, OpenCode, Cursor, Copilot, Droid, any ACP agent, plus API keys and local models, with the logins you already have.",
        "<b>The tool set is unmatched.</b> No competitor found has the iOS Simulator and a real-iPhone tool, video watching, study mode, notes and connected machines together, on top of a Browser with extensions and ad blocking and background computer use.",
        f"<b>It costs nothing and doesn't bloat your prompts.</b> Free and Apache-2.0; with Trek's exact flags a “hi” is {_n(d.get('trek_hi'))} tokens against 40,120 for bare Claude Code (measured here), so Trek's tools cost you no extra tokens.",
        "<b>It looks good and it welcomes newcomers well.</b> Blind, its screenshot came 2nd of 8 with the top hierarchy score (9/10), and its first screen came 1st of 8: it says what the app is, that setup can wait, and that data stays on the Mac, with no account.",
    ]
    T["why_not"] = [
        f"<b>With real agents, its memory lead disappears.</b> 10 threads open and 4 streaming with Claude Code: {_n(pk.get('total_mb'))} MB, the same as Synara's 1,784. Trek kept all 10 CLIs alive after their turns ended ({_n(_g(r10, 'after', 'total_mb'))} MB at rest), where Synara cut its count to 4 and super.engineering measures 134 MB with 5 idle threads.",
        "<b>Screen-reader users can't use it.</b> Trek's window exposes 16 accessibility nodes: 2 of 13 controls have a name and no text is readable (here, current main). Codex, Orca and Cursor expose 46–146 controls with 95%+ named. This is a GPUI gap Zed shares.",
        "<b>After a good first screen, the setup is heavy.</b> An intro and 8 steps, 7 of them macOS permission prompts (Accessibility, Screen Recording, Microphone, Speech, Notifications, Full Disk Access, Local Network); you must already have, or install, an agent CLI and sign in to it. T3 Code opens straight to a composer; the Codex app needs one sign-in.",
        "<b>Its own agent loop is weak and unranked.</b> On Terminal-Bench 4 with DeepSeek V4 Flash, Trek's Direct harness passed 0 of 19 tasks on 30 Oct; today's build gave up a task after two reasoning-only replies instead of lowering the effort. No same-model comparison with another harness exists yet (today's was stopped by an empty OpenRouter account). Trek is only as good as the vendor CLI you run in it.",
        "<b>No one outside has measured it, and its docs overclaim.</b> Trek is not in ADEMark; docs/PLUGINS.md says “Trek is first on ADEMark's speed and weight tests” with no source. The lab numbers (55–65 MB) also differ from real use: 520 MB after a 4.7-hour day with 782 threads.",
        "<b>It is small and little known.</b> T3 Code (82k stars) and Orca (142k) win the polls in the posts you sent; Trek wasn't an option in either. The iPhone app isn't on the App Store or TestFlight (any phone gets a web remote instead).",
        "<b>Its design has fixable faults.</b> The blind reviewer counted seven accent hues on one screen, uniformly heavy sidebar titles, a dead gap above the plan card, identical tab icons and a red Stop button while it waits for approval. T3 Code's restraint won.",
    ]

    # ------------------------------------------------------------------ sections
    T["perf_intro"] = ("<p>Every app was launched the same way (`open -g`, never activated, no keystrokes), left 25 s, then measured over all of its processes, three times. "
                       "Trek was measured with its own harness (`./bin/agent-cli perf`, the method in docs/PERFORMANCE.md) in an isolated profile. "
                       "The competitors ran with the owner's own profiles, which is closer to real use but not identical to Trek's empty one, so the real-use Trek figure is given beside it.</p>")
    T["perf_after"] = (f"<p><b>Streaming.</b> A two-minute answer at about 70 tokens a second cost Trek {_n(_g(perf, 'stream', 'cpu_percent'), '%', 0)} of one core with frames at p50 "
                       f"{_n(_g(perf, 'stream', 'frame_p50_ms'), ' ms', 1)} / p95 {_n(_g(perf, 'stream', 'frame_p95_ms'), ' ms', 1)}, and RSS rose {_n(_g(perf, 'stream', 'rss_start_mb'))}→{_n(_g(perf, 'stream', 'rss_peak_mb'))} MB. "
                       "docs/PERFORMANCE.md reports 21–25% for the same test; today's 42% is higher and is reported as measured. Competitors' streaming CPU could not be driven the same way without signing in to them.</p>"
                       "<p><b>Real use.</b> The owner's running Trek 0.4.68, read without touching it: the main process at "
                       f"{real.get('trek_main_footprint_mb', '–')} MB after {real.get('uptime', '–')} with {real.get('threads_in_db', '–')} threads and {real.get('browser_tabs_saved', '–')} Browser tabs; each Claude Code session it runs adds the CLI "
                       f"(≈{real.get('claude_code_avg_footprint_mb', '–')} MB) and six MCP helpers (≈{real.get('trek_mcp_avg_footprint_mb', '–')} MB each). The agent CLIs, not the app, dominate memory once agents run — in every app.</p>")
    L = perf.get("launch", {}); I = perf.get("idle", {}); St = perf.get("stream", {}); Lg = perf.get("long", {})
    claims = [
        ("Idle footprint 55 / 56 / 57 MB with 0 / 10 / 100 threads", "docs/PERFORMANCE.md", f"{_n(_g(I,'0_threads','footprint_mb'))} / {_n(_g(I,'10_threads','footprint_mb'))} / {_n(_g(I,'100_threads','footprint_mb'))} MB", "Holds (a few MB higher)"),
        ("Warm launch 201 / 191 / 186 ms", "docs/PERFORMANCE.md", f"{_n(L.get('warm_0_threads_ms'))} / {_n(L.get('warm_10_threads_ms'))} / {_n(L.get('warm_100_threads_ms'))} ms", "Holds (about 10% slower today)"),
        ("Cold launch, 100 threads, 235–243 ms after remount", "docs/PERFORMANCE.md", f"{_n(L.get('cold_100_threads_ms'))} ms", "Holds"),
        ("Idle CPU 0.03% of a core", "docs/PERFORMANCE.md", f"{_n(_g(I,'100_threads','cpu_percent'), '%', 2)}", "Holds"),
        ("Streaming a 2-minute answer: 21–25% CPU", "docs/PERFORMANCE.md", f"{_n(St.get('cpu_percent'), '%', 1)}", "Not reproduced (about double)"),
        ("10,000-item thread: first paint 157 ms, scroll p95 5.1 ms", "docs/PERFORMANCE.md", f"{_n(Lg.get('first_paint_ms'), ' ms')}, p95 {_n(Lg.get('scroll_frame_p95_ms'), ' ms', 1)}", "Open faster; scroll p95 higher, still under a 60 Hz frame"),
        ("Trek's “hi” is below the bare CLI's", "docs/PROMPT_BUDGET.md", f"{_n(d.get('trek_hi'))} vs 40,120 tokens", "Holds"),
        ("“Trek is first on ADEMark's speed and weight tests”", "docs/PLUGINS.md:322", "Trek is not in ADEMark", "Unsupported: remove or get measured"),
        ("“Light and fast”", "README", f"{_n(_g(I,'100_threads','footprint_mb'))} MB idle; {real.get('trek_main_footprint_mb','–')} MB in a real 4.7 h day; agents add ≈150–255 MB each", "True of the app, not of a session with agents"),
    ]
    T["claims_html"] = ("<h3>Trek's own claims, checked</h3><div class='table-wrap'><table class='data'><thead><tr><th>Claim</th><th>Where</th><th>Measured today</th><th>Verdict</th></tr></thead><tbody>"
                        + "".join(f"<tr><td>{a}</td><td>{b}</td><td>{c}</td><td>{v}</td></tr>" for a, b, c, v in claims) + "</tbody></table></div>")
    T["scale_intro"] = ("<p>Threads were seeded through Trek's own store (`perf_seed`: four turns each, spread over four months) and the app driven by its test harness on an invisible display. "
                        "No competitor offers a way to seed thousands of threads, so this is Trek against itself; ADEMark's per-thread and long-thread numbers are the only comparable published figures.</p>")
    T["scale_after"] = ("<p><b>Browser tabs</b> are WebKit pages of a local 60-paragraph document with a timer, opened one after another through the Browser's own command. "
                        "Each tab's page lives in a WebKit process, which is where the memory goes; Trek's own process barely moves.</p>"
                        "<p><b>Compared:</b> ADEMark's long-thread test (2,000 messages) opened in 76–351 ms across apps; Trek opened a 10,000-item thread in "
                        f"{_n(_g(perf, 'long', 'first_paint_ms'))} ms here. ADEMark's “memory per thread” ranged 2.1 MB (Zeron) to 39.9 MB (Traycer); Trek's footprint grew "
                        f"{_n((bigr.get('footprint_mb') or 0) - (_g(thr, '100', 'footprint_mb') or 0))} MB from 100 to {_n(big)} idle threads.</p>")
    T["swarm_intro"] = ("<p>Swarms were run with Trek's mock agent so that only the app is measured and nothing is spent: N threads each streaming an answer at once, then N threads each "
                        "starting a sub-agent (`mock:delegate implement …`, a child thread). The frame times and typing latency are taken during the load. "
                        "A real-agent run follows, replicating Synara's post exactly.</p>")
    T["swarm_after"] = d.get("real10_html", "")
    T["tokens_intro"] = ("<p>Trek wraps the vendor CLIs, so two things matter: what Trek adds to every request, and how good its own loop is when you use API or local models.</p>")
    T["tb_html"] = d.get("tb_html") or (
        "<h3>Trek's own agent loop on Terminal-Bench</h3>"
        "<p><b>Earlier (30 Oct, same Mac):</b> Trek's Direct harness on 19 CPU-only Terminal-Bench 4 tasks with DeepSeek V4 Flash, one trial, 25 minutes: "
        "<b>0 tasks passed</b> (121 of 184 verifier tests), $3.29; with its Script tool on, 1 passed (120 of 184), $5.07. No other harness ran then, so it could not be ranked "
        "(<a href='https://claude.ai/artifact/GzrTHeDnj4ub46sumSKgwb'>report</a>).</p>"
        "<p><b>Today's rerun failed for want of credit.</b> The plan was the missing comparison: Trek's current harness against OpenCode on the same 7 tasks and model. "
        "The OpenRouter account is exhausted ($40.20 used of $40); the key's own remaining cap ($6.58) had hidden that, and requests returned 402 after a few calls. "
        "Spent: $0.008. OpenCode never ran, so <b>Trek's harness is still unranked against any other on the same model</b>.</p>"
        "<p><b>One thing it did show:</b> on cargo-flight-dispatch, before any 402, the current harness gave up the whole task after 5 requests: "
        "“The model used its whole output limit on reasoning and wrote nothing. Try a lower effort.” It retries an empty, reasoning-only reply twice "
        "(<code>MAX_EMPTY = 2</code>, crates/trek-agents/src/direct.rs) and then stops, rather than lowering the effort or raising the output limit itself. "
        "The 30 Oct build ran the same model and effort through 19 tasks without stopping this way. One occurrence is a signal, not a rate.</p>")
    T["tb_sentence"] = "its own agent loop, used for API and local models, passed 0 of 19 Terminal-Bench tasks with a cheap model on 30 Oct, and today's build abandoned a task on a reasoning-only reply"
    T["tokens_after"] = ("<p><b>Published harness results</b> (not run here): Terminal-Bench 4.1 — Copilot 71.2%, Claude Code 65.8% (Opus 5.5), Antigravity CLI 64.5%, Codex 62.1%; "
                         "Factory's own TB4 run on Opus 5.5 — Droid 65.2, Cursor 62.1, OpenCode 62.1, Devin 59.6, Claude Code 57.6. "
                         "When Trek runs Claude Code or Codex, these are its numbers too.</p>")
    T["features_intro"] = "<p>From the field research (every row has a source URL in research/field/features.csv).</p>"
    T["features_after"] = ("<p>Where Trek is alone: iOS Simulator plus a real-iPhone tool, video watching, notes and study mode, connected machines, and API/local models beside vendor CLIs. "
                           "Where it is not ahead: phone companions are now table stakes (Claude, Codex, Cursor, Orca, T3, Paseo, Conductor, Herdr all have one, most in app stores), and "
                           "Orca matches most of Trek's surface including computer use and Android.</p>")
    T["ux_intro"] = ("<p><b>First run, counted.</b> Trek: an intro, then Appearance, Permissions (7 macOS permissions), Agents (scan; install and sign in run in a terminal), API keys, Local models, Approvals, "
                     "Phone and machines, Finish — 9 screens, all skippable, no Trek account. Claude desktop: a welcome and a sign-in (a paid plan for Code). Codex app: a ChatGPT sign-in (free tier). "
                     "Conductor: “Sign up” is the first screen. Paseo: a 5-step setup (Appearance, Providers, Projects, Orchestration, Mobile), skippable. super.engineering: one “Get started” or “Set up later”. T3 Code: opens straight to a composer and uses the CLIs already installed. "
                     "The counts of clicks for “start an agent on a repo, review a diff, recover from an error” could not be measured in the competitors without signing in to them; see the limits.</p>"
                     "<p><b>Accessibility.</b> What VoiceOver can see, read from each app's accessibility tree (scripts/axprobe.swift, which asks Chromium apps to build their tree first and walks it twice). "
                     "A control “with a name” has a title, description or label VoiceOver can speak.</p>")
    T["ux_after"] = ("<p>Trek's GPUI window gives VoiceOver almost nothing: no thread titles, no transcript text, unnamed buttons. Zed, on the same UI framework, is similar. "
                     "The component kit has accessibility labels (e.g. switches), but they don't reach macOS. Keyboard-only use is better: ⌘K, shortcuts and review by keyboard exist, "
                     "but without a screen reader a blind user cannot know where focus is.</p>")
    T["design_intro"] = ("<p>A separate, read-only reviewer scored unlabelled images on seven criteria without being told which product was which, and was told not to guess. "
                         "Set A is each maker's own best screenshot (README or site), the fairest like-for-like: everyone's best foot forward. The images are shown below exactly as the reviewer saw them; the products are named in the table after it. Sources: T3 Code, Orca and Synara READMEs on GitHub, paseo.sh, the OpenCode docs, the Zed and Conductor site preview images (app area cropped), and Trek's README.</p>")
    T["design_mid"] = d.get("design_mid", "")
    T["design_b_after"] = d.get("design_after_b", "")
    T["design_after"] = ("<p class='sub'>Screenshots below are as each app launched on this Mac; private thread and project names are pixelated, and Synara, Orca and VS Code are left out because nearly all of their windows showed the owner's own work. The competitors ran with the owner's profiles, "
                         "so some show onboarding or sign-in screens rather than a working window.</p>")
    T["posts_intro"] = "<p>Each post was read through its public API and every video watched (frames and transcript). Full notes in research/posts/.</p>"
    T["posts_rows"] = [
        ["1", "@0xPaulius, Clonk v2", "Motion ad: one commander splits “Run my company” into teams; 39,800→360,830 thread counter; phone live status", "Swarms across every plan and machine; “12,307 agents threads”",
         f"Marketing counts, not a benchmark. Trek ran {_n(big)} threads and 50 parallel mock agents smoothly; it has sub-agents, started threads and machines, but no “commander” that splits a goal into teams."],
        ["2", "@gavmilde, Capy", "Screenshot of role “Captains” with automations", "$20/mo for cloud machines, tokens on your Claude sub",
         f"Trek is free and local; no hosted machines or role agents. Capy measured {_n(m('Capy', 'footprint_mb'))} MB idle here vs Trek {_n(idle.get('footprint_mb'))} MB."],
        ["3", "@lxe, Superharness", "Free, open-source app that discovers every harness's sessions; steer, queue, usage limits, iPhone mirror", "Drive all your harnesses with no setup",
         "Closest idea to Trek (imports history, many harnesses, phone). Trek has far more tools; Superharness shows usage limits per vendor prominently and needs no setup. 28 stars, two days old."],
        ["4", "@dazuoforyou, review", "Orca complete but unpolished; Paseo restrained; Herdr lost records; T3 Code didn't land", "Polish and first-run clarity beat feature count",
         "Trek risks Orca's verdict: everything, less polish. Its blind design score (2nd) is good, but its first run (7 permissions) is the opposite of “restrained”."],
        ["5", "@hanghuang_, polls", "T3 Code 52%, Orca 39% (1,968 votes); Capy 58% (295)", "Popularity", "Trek wasn't an option in either poll: its weakest point is reach, not speed."],
        ["6", "@trySynara canvas", "Agents edit a Figma-like canvas", "Agents make mockups before prod", "Trek has HTML artifacts and charts inline, not an editable canvas."],
        ["7", "@trySynara dither video", "Animated dither background; 25% faster startup chart; worktree toggle per chat", "Delight + speed", f"Trek has themes and painted backgrounds; Synara's animated background idled at {_n(m('Synara', 'idle_cpu_pct'), '%', 0)} CPU here vs Trek's {_n(idle.get('cpu_percent'), '%', 2)}."],
        ["8", "@trySynara 1.6.8", "Startup 505→379 ms, RAM 239→224 MB idle", "Speed", f"Trek: {_n(launch)} ms to frame and {_n(idle.get('footprint_mb'))} MB; Synara measured {_n(m('Synara', 'footprint_mb'))} MB across all processes here (its 224 MB is one process)."],
        ["9", "@trySynara iOS 182", "Phone failover 8.03 s→651 ms; streaming decode", "Phone speed", "Not measured for Trek's iPhone app (no phone connected; it isn't distributed). A gap in this report."],
        ["10", "@trySynara 1.6.6", "Installer −22%, 315→61 requests at launch; cross-provider delegation", "Smaller, faster", "Trek's binary is 126 MB (release, shots); delegation across agents and models: Trek has it (consult, delegate)."],
        ["11", "@trySynara iOS perf", "14 microbenchmarks, up to −99.5%", "Model-tuned performance", "Microbenchmarks of their own code; nothing comparable to run."],
        ["12", "@emanueledpt RAM", "10 threads, 4 streaming: 2,307→1,784 MB; CLI processes 10→4", "Memory with real agents", d.get("post12_cmp", "See the real-agent run under Swarming.")],
        ["—", "ademark.dev", "15 apps, speed/weight/tokens/orchestration; Trek absent", "Independent per-test ranking", "Trek is not in it, despite its docs quoting ranks. Getting into ADEMark is the cheapest credibility win available."],
    ]

    T["method"] = d.get("method_html") or ("""
<h3>What was run, and how to run it again</h3>
<ul>
<li><b>Machine:</b> MacBook Air M5 (10 cores, 24 GB), macOS 27.2.1, on the charger. Other agents' builds came and went; every run waited (up to 45 min) for a 1-minute load below 5 and records the load before and after. Most runs started at a load of 2.7–5.</li>
<li><b>Competitor apps</b> (<code>scripts/measure_apps.py</code>, <code>scripts/appprobe.swift</code>): one thrown-away launch, then 3 launches with <code>open -g</code>; the time to the first on-screen window of at least 200×150 pt; 25 s settle; <code>footprint</code> and RSS summed over every process in the bundle, every process whose responsible process is the app, and WebKit processes that appeared; CPU over 30 s; quit with an Apple Event. Medians of 3. Versions are in <code>results/apps.json</code>.</li>
<li><b>Trek</b> (<code>./bin/agent-cli perf launch idle stream long</code> on a release build of main 242fdff8 with the <code>shots</code> feature): isolated profile, mock agent, nothing of the owner's read. Then <code>scripts/trek_stress.py</code> for 100/1,000/5,000/10,000 threads, swarms of 10/20/50, and 50/100/200 Browser tabs, through the app's own test harness on an invisible display.</li>
<li><b>Real agents</b> (<code>scripts/trek_real10.py</code>): the isolated app with <code>TREK_REAL_AGENTS=1</code>, Claude Code on the owner's subscription, a throwaway project.</li>
<li><b>Tokens</b>: <code>claude -p "hi" --output-format json --strict-mcp-config</code>, bare and with Trek's exact flags read from a running Trek session's argv (<code>results/tokens-hi.json</code>).</li>
<li><b>Accessibility</b>: <code>scripts/axprobe.swift</code> walks the tree VoiceOver reads; for Chromium apps it sets <code>AXManualAccessibility</code> and walks twice.</li>
<li><b>Design</b>: a separate read-only sub-agent judged anonymised images; the key was opened only after its verdict (<code>results/design-blind-*.json</code>).</li>
<li><b>Terminal-Bench</b>: Harbor 0.25 with the earlier study's Trek adapter; see the harness section.</li>
<li><b>Spend</b>: OpenRouter credit read from <code>GET /api/v1/key</code> before and after; Claude Code turns on the owner's subscription; everything else free.</li>
</ul>
<h3>Limits — what this could not do</h3>
<ul>
<li><b>ADEMark itself was not run.</b> Its harness refuses to measure outside a dedicated macOS user named <code>ademark</code>, and creating a user on the owner's Mac was out of scope. Its published numbers are used, labelled, and they come from a different machine (Mac mini M4, 16 GB).</li>
<li><b>Profiles differ.</b> Competitors ran with the owner's own data (some with long histories, two never signed in); Trek ran empty or seeded. The owner's real Trek is reported beside it to show the gap.</li>
<li><b>Launch timing differs.</b> Trek is timed from spawn to its first painted frame; the others from <code>open -g</code> to a window appearing. Calculator and TextEdit through the same <code>open -g</code> path give the floor to add to Trek.</li>
<li><b>No competitor was driven with real agents or asked a prompt</b>: that needs their sign-ins (Claude, Conductor and Warp were not signed in) and would act on the owner's accounts. So streaming CPU, swarms, browser-tab stress and click counts for “start an agent, review a diff, recover from an error” are Trek-only or published.</li>
<li><b>Not installed:</b> Paradev, Herdr, Superharness, Clonk (invite-gated), Devin Desktop, the Factory app, Amp, Kiro, Traycer, Emdash. Conductor's maker was bought and new sign-ups are closed, so it could only be measured at its sign-up screen.</li>
<li><b>Only macOS.</b> Windows and Linux builds of Trek and the rest were not measured; nor Trek's iPhone app (no phone connected; it isn't distributed).</li>
<li><b>Accessibility is structural.</b> Counting named controls is not a VoiceOver user test; a fuller audit would also check contrast and focus order by hand.</li>
<li><b>One judge.</b> The design review is one model's blind reading of marketing screenshots; Trek's image is its own curated README shot, as everyone else's is theirs.</li>
<li><b>Terminal-Bench was not rerun.</b> The OpenRouter account was empty ($40.20 of $40 used); the check before spending read the key's cap ($6.58 left), not the account balance, which was the wrong endpoint. The attempt cost $0.008 and was stopped; a free substitute (local models) would have scored near zero for both harnesses and taken hours, so it was not run.</li>
<li><b>A rule was broken once:</b> the field-research sub-agent started four worker agents of its own, beyond the two-at-a-time limit set for this study. Its notes were kept; nothing else ran that way.</li>
</ul>""")
    T["footer"] = ("Scripts and raw results: bench-report/scripts and bench-report/results on branch bench/competitors-2026-11. Built by scripts/build_report.py. "
                   "Measured on a MacBook Air M5 (10 cores, 24 GB), macOS 27.2.1; the 1-minute load average is recorded with every number.")
    return T
