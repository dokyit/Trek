#!/usr/bin/env python3
"""Accessibility re-probe: launch each app (`open -g`), let it settle, read its accessibility tree with
axprobe (AX_WAIT seconds after asking Chromium apps to build it, walked twice), quit it.
Usage: python3 -I ax_pass.py <out.json> [name ...]   (APPPROBE, AXPROBE, AX_WAIT in the environment)"""
import json, os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_apps as m  # noqa: E402

out = sys.argv[1]
names = sys.argv[2:]
res = json.load(open(out)) if os.path.exists(out) else {}
for name, app in m.APPS:
    if names and name not in names or not os.path.isdir(app):
        continue
    bid = m.bundle_id(app)
    if [p for p in m.app_pids(app) if m.comm(p).startswith(app)]:
        res[name] = {"skipped": "already running"}
        continue
    m.sh([m.PROBE, "launch", app, "90"])
    time.sleep(20)
    mains = [p for p in m.app_pids(app) if m.comm(p).startswith(app + "/Contents/MacOS/")]
    if mains:
        r = m.sh([os.environ.get("AXPROBE", "axprobe"), str(mains[0])], timeout=180).stdout.strip()
        try:
            res[name] = json.loads(r)
        except ValueError:
            res[name] = {"error": r[:200]}
    m.quit_app(app, bid)
    time.sleep(3)
    json.dump(res, open(out, "w"), indent=1)
    print(name, res.get(name, {}).get("named_controls"), "/", res.get(name, {}).get("controls"), flush=True)
