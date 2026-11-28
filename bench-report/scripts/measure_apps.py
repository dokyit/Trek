#!/usr/bin/env python3
"""Launch time, idle memory and idle CPU of desktop agent apps, measured the same way for each.

For every app: one warm-up launch (thrown away), then RUNS launches. Each launch:
  - `open -g <app>` (not activated, no keystrokes), timed to the first on-screen window >= 200x150 pt
  - settle SETTLE seconds, then sum `footprint` (phys_footprint) and RSS over every process the app
    is responsible for: its bundle's own executables, helpers whose responsible pid is the app, and
    WebKit processes that appeared during the launch
  - idle CPU: summed process CPU time over CPU_SECS, as % of one core
  - quit with an Apple Event (`tell application id … to quit`), SIGTERM only if it hangs
The app runs with the owner's own profile as installed (no sign-in, no prompt sent).
Usage: python3 -I measure_apps.py <out.json> <shots_dir> [name ...]
"""
import json, os, re, subprocess, sys, time, statistics

PROBE = os.environ.get("APPPROBE", "appprobe")
RUNS = int(os.environ.get("RUNS", "3"))
SETTLE = int(os.environ.get("SETTLE", "25"))
CPU_SECS = int(os.environ.get("CPU_SECS", "30"))

APPS = [
    ("Codex app", "/Applications/ChatGPT.app"),
    ("Cursor", "/Applications/Cursor.app"),
    ("Synara", "/Applications/Synara.app"),
    ("T3 Code", "/Applications/T3 Code (Alpha).app"),
    ("OpenCode desktop", "/Applications/OpenCode.app"),
    ("Zed", "/Applications/Zed.app"),
    ("Antigravity", "/Applications/Antigravity.app"),
    ("VS Code", "/Applications/Visual Studio Code.app"),
    ("Orca", "/Applications/Orca.app"),
    ("Capy", "/Applications/Capy.app"),
    ("Conductor", "/Applications/Conductor.app"),
    ("Claude", "/Applications/Claude.app"),
    ("Warp", "/Applications/Warp.app"),
    ("Paseo", "/Applications/Paseo.app"),
    ("super.engineering", "/Applications/super.engineering.app"),
    # Baselines for the `open -g` path itself (Launch Services + a trivial native window).
    ("Calculator (baseline)", "/System/Applications/Calculator.app"),
    ("TextEdit (baseline)", "/System/Applications/TextEdit.app"),
]


def sh(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, **kw)


def load():
    return float(sh(["sysctl", "-n", "vm.loadavg"]).stdout.split()[1])


def bundle_id(app):
    return sh(["defaults", "read", f"{app}/Contents/Info.plist", "CFBundleIdentifier"]).stdout.strip()


def version(app):
    return sh(["defaults", "read", f"{app}/Contents/Info.plist", "CFBundleShortVersionString"]).stdout.strip()


def app_pids(app):
    out = sh([PROBE, "pids", app]).stdout.split()
    return [int(p) for p in out]


def webkit_pids():
    out = sh(["ps", "-axo", "pid=,comm="]).stdout
    return {int(l.split(None, 1)[0]) for l in out.splitlines() if "com.apple.WebKit" in l}


def comm(pid):
    return sh(["ps", "-o", "comm=", "-p", str(pid)]).stdout.strip()


def classify(app, pids, new_webkit):
    mine, other = [], []
    for p in pids:
        c = comm(p)
        if not c:
            continue
        (mine if c.startswith(app) or "WebKit" in c or "XPCServices" in c else other).append(p)
    mine += [p for p in new_webkit if p not in mine and comm(p)]
    return mine, other


def footprint(pids):
    total = 0
    for p in pids:
        out = sh(["footprint", "-p", str(p), "-f", "bytes", "--noCategories"]).stdout
        m = re.search(r"phys_footprint:\s+(\d+)", out)
        if m:
            total += int(m.group(1))
    return total


def rss(pids):
    if not pids:
        return 0
    out = sh(["ps", "-o", "rss=", "-p", ",".join(map(str, pids))]).stdout
    return sum(int(x) for x in out.split()) * 1024


def cputime(pids):
    if not pids:
        return 0.0
    out = sh(["ps", "-o", "time=", "-p", ",".join(map(str, pids))]).stdout
    total = 0.0
    for t in out.split():
        parts = [float(x) for x in t.split(":")]
        secs = 0.0
        for x in parts:
            secs = secs * 60 + x
        total += secs
    return total


def quit_app(app, bid):
    sh(["osascript", "-e", f'tell application id "{bid}" to quit'], timeout=30)
    for _ in range(40):
        if not [p for p in app_pids(app) if comm(p).startswith(app)]:
            return "quit"
        time.sleep(0.5)
    for p in app_pids(app):
        if comm(p).startswith(app):
            os.kill(p, 15)
    time.sleep(3)
    return "sigterm"


def measure(name, app, shots):
    bid = bundle_id(app)
    res = {"name": name, "app": app, "bundle_id": bid, "version": version(app), "runs": []}
    if [p for p in app_pids(app) if comm(p).startswith(app)]:
        res["skipped"] = "already running (owner's instance); not touched"
        return res
    # warm-up
    w = sh([PROBE, "launch", app, "90"]).stdout.strip()
    time.sleep(8)
    res["warmup_launch_ms"] = w
    quit_app(app, bid)
    time.sleep(3)
    for i in range(RUNS):
        r = {"load_before": load()}
        wk0 = webkit_pids()
        out = sh([PROBE, "launch", app, "90"]).stdout.strip()
        r["launch_ms"] = int(out) if out.isdigit() else None
        time.sleep(SETTLE)
        mine, other = classify(app, app_pids(app), webkit_pids() - wk0)
        r["processes"] = len(mine)
        r["other_children"] = [comm(p) for p in other]
        r["footprint_mb"] = round(footprint(mine) / 2**20, 1)
        r["rss_mb"] = round(rss(mine) / 2**20, 1)
        c0 = cputime(mine)
        time.sleep(CPU_SECS)
        mine2, _ = classify(app, app_pids(app), webkit_pids() - wk0)
        c1 = cputime(mine2)
        r["idle_cpu_pct"] = round(100 * (c1 - c0) / CPU_SECS, 2)
        r["windows"] = sh([PROBE, "windows", app]).stdout.strip()
        if i == 0:
            mains = [p for p in mine if comm(p).startswith(app + "/Contents/MacOS/")]
            if mains:
                ax = sh([os.environ.get("AXPROBE", "axprobe"), str(mains[0])], timeout=120).stdout.strip()
                try:
                    r["accessibility"] = json.loads(ax)
                except ValueError:
                    r["accessibility"] = {"error": ax[:200]}
        if i == 0 and shots:
            wid = sh([PROBE, "winid", app]).stdout.strip()
            if wid and wid != "0":
                f = os.path.join(shots, re.sub(r"\W+", "-", name.lower()).strip("-") + ".png")
                sh(["screencapture", "-x", "-o", "-l", wid, f])
                r["screenshot"] = f if os.path.exists(f) else None
        r["load_after"] = load()
        r["quit"] = quit_app(app, bid)
        res["runs"].append(r)
        time.sleep(3)
    ok = [r for r in res["runs"] if r["launch_ms"]]
    if ok:
        res["median"] = {k: statistics.median([r[k] for r in ok]) for k in ("launch_ms", "footprint_mb", "rss_mb", "idle_cpu_pct", "processes")}
    return res


def main():
    out, shots = sys.argv[1], sys.argv[2]
    names = sys.argv[3:]
    os.makedirs(shots, exist_ok=True)
    results = json.load(open(out)) if os.path.exists(out) else {}
    for name, app in APPS:
        if names and name not in names:
            continue
        if not os.path.isdir(app):
            results[name] = {"name": name, "app": app, "skipped": "not installed"}
            continue
        waited = 0
        while load() > float(os.environ.get("MAX_LOAD", "5")) and waited < int(os.environ.get("MAX_WAIT", "2700")):
            print(f"waiting: load {load():.1f}", flush=True)
            time.sleep(60)
            waited += 60
        print(f"measuring {name}", flush=True)
        try:
            results[name] = measure(name, app, shots)
        except Exception as e:  # keep going; record the failure
            results[name] = {"name": name, "app": app, "error": repr(e)}
        json.dump(results, open(out, "w"), indent=1)
        print(json.dumps(results[name].get("median") or results[name], default=str), flush=True)


if __name__ == "__main__":
    main()
