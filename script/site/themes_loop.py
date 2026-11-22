#!/usr/bin/env python3
"""The home page's themes loop (features/themes.mp4 and themes-poster.webp), from the isolated test app.

  themes_loop.py scene    stage a fresh profile: two demo projects, three finished threads, one open
  themes_loop.py record   screencapture -v of the window's rectangle on the Trek Test Display while
                          the looks change -> $TREK_SITE_SCRATCH/themes/themes.mov (+ themes.json)
  themes_loop.py cut      that recording -> site/assets/img/features/themes.mp4 and themes-poster.webp
  themes_loop.py states   one --real still per state, for checking the looks before a take

Start the app first, from the tree to show (a fresh profile, the default window is too small):
  TREK_REPO=<tree> CARGO_TARGET_DIR=<dir> <tree>/.agents/skills/verify-trek/scripts/trek-dev start --size 1440x900

TREK_REPO is the tree whose trek-dev drives the app (it must be a git checkout; an exported tree
can be `git init`ed); the site files are written to this script's own checkout. The mock agent in
its demo dress only: no plugins (a fresh profile has none; the 17 November take had the Focus walk
plugin on from the plugins still), the tools panel and the thread's hub closed. `cut` takes START
and LENGTH in the recording; the defaults fit the 21 November 2026 take (Journal settled at
2.35 s, 12.3 s long), so look at a new take's frames before cutting it.
"""
import json, os, pathlib, subprocess, sys, time

HERE = pathlib.Path(__file__).resolve().parent
SITE_REPO = HERE.parent.parent
REPO = pathlib.Path(os.environ.get("TREK_REPO", SITE_REPO))
T = str(REPO / ".agents/skills/verify-trek/scripts/trek-dev")
ENV = dict(os.environ)
ENV.pop("TREK_REAL_AGENTS", None)
ENV.pop("TREK_LIVE_AGENT", None)
SCRATCH = pathlib.Path(os.environ["TREK_SITE_SCRATCH"]) / "themes"
DEMO = SCRATCH / "demo"
OUT = SITE_REPO / "site/assets/img/features"


def run(*a, timeout=180):
    r = subprocess.run([T, *a], cwd=REPO, env=ENV, capture_output=True, text=True, timeout=timeout)
    out = r.stdout + r.stderr
    if '"ok": false' in out or r.returncode:
        print("ERR", a, out[-400:], flush=True)
    return out


def ctl(*a, **k):
    return run("control", *a, **k)


def frame():
    ws = json.loads(run("windows"))["windows"]
    return next(w for w in ws if w.get("main"))["frame"]


SHOT = [None]


def raw(lines, timeout=30):
    """All lines in one command file, so a state's changes land together (as shotcmd.py sends them)."""
    if SHOT[0] is None:
        SHOT[0] = pathlib.Path(json.loads(run("status"))["runtime_dir"]) / "shot"
    shot = SHOT[0]
    done = shot / "done"
    done.unlink(missing_ok=True)
    tmp = shot / "cmd.tmp"
    tmp.write_text("\n".join(lines) + "\n")
    os.replace(tmp, shot / "cmd")
    t = time.time()
    while not done.exists():
        if time.time() - t > timeout:
            print("TIMEOUT", lines, flush=True)
            return
        time.sleep(0.01)
    r = done.read_text().strip()
    done.unlink(missing_ok=True)
    if r != "ok":
        print("ERR", lines, r, flush=True)


def apply(cmds):
    raw([" ".join(c) for c in cmds])


# (label, [shots commands]) in order. Leaving Trek's own look keeps its settings (plain Night here)
# and coming back restores them, so the looks go first, straight from plain Night, then Trek's own
# look with its backgrounds. The loop is cut from Journal settled to the end of glass with the
# Oxbow painting; that seam is as instant as Trek's own switches.
STATES = [
    ("trek-night", [("background", "none"), ("glass", "off"), ("theme", "night")]),
    ("journal", [("look", "journal")]),
    ("console", [("look", "console")]),
    ("alpenglow", [("look", "alpenglow")]),
    ("trek-night-back", [("look", "trek")]),
    ("trek-paper", [("theme", "paper")]),
    ("paper-ridges", [("background", "paper", "chat", "0")]),
    ("night-dawn", [("theme", "night"), ("background", "dawn", "chat", "0")]),
    ("night-glass-ridges", [("glass", "on"), ("background", "night", "chat", "0")]),
    ("glass-oxbow", [("background", "paintings/oxbow", "chat", "0.3")]),
    # Not in the loop: a change after the Oxbow hold, so screencapture (which writes a frame only
    # when the window changes) has the whole hold.
    ("after", [("background", "none"), ("glass", "off")]),
]
# Seconds from applying a state to the next: a look cross-fades for most of a second, so it is
# held longer than an instant switch.
HOLDS = {"journal": 1.8, "console": 1.9, "alpenglow": 1.7, "trek-night-back": 1.75,
         "trek-paper": 1.0, "paper-ridges": 1.1, "night-dawn": 1.1, "night-glass-ridges": 1.1,
         "glass-oxbow": 2.0, "after": 1.0}


def scene():
    for name in ("harbor", "relay"):
        d = DEMO / name
        if not d.exists():
            d.mkdir(parents=True)
            subprocess.run([sys.executable, str(HERE / "make_repo.py"), str(d), name], check=True)
    ctl("dress", "Claude Code|Sonnet 5.5|Opus 5.5")
    ctl("pace", "0")
    ctl("glass", "off")
    ctl("theme", "night")
    ctl("tools", "off")
    run("project", str(DEMO / "relay"))
    run("send", "Walk me through how the app starts", "--timeout", "120")
    ctl("wait", "idle", "40000")
    ctl("title", "Tour the startup code")
    run("project", str(DEMO / "harbor"))
    run("send", "Summarise what changed on main this week", "--timeout", "120")
    ctl("wait", "idle", "40000")
    ctl("title", "Catch up on main")
    # The open thread: one calm finished answer.
    run("project", str(DEMO / "harbor"))
    run("send", "Add a --verbose flag to the CLI tools", "--timeout", "120")
    ctl("wait", "idle", "40000")
    ctl("title", "Add a verbose flag")
    ctl("tools", "off")
    # The thread's hub (where it runs, git, sources) opens beside a finished thread: close it.
    # It also names the computer.
    ctl("click", "hub")
    ctl("wait", "1500")
    print("staged")


def record(lead=2.0):
    SCRATCH.mkdir(parents=True, exist_ok=True)
    log = []
    apply([("look", "trek")])
    time.sleep(2.5)
    apply(STATES[0][1])
    ctl("tools", "off")
    ctl("wait", "1500")
    f = frame()
    secs = int(lead + sum(HOLDS.values()) + 3)
    p = SCRATCH / "themes.mov"
    p.unlink(missing_ok=True)
    proc = subprocess.Popen(["screencapture", "-v", "-V", str(secs), "-x",
                             f"-R{round(f['x'])},{round(f['y'])},{round(f['w'])},{round(f['h'])}", str(p)])
    t0 = time.time()
    time.sleep(lead)
    for label, cmds in STATES[1:]:
        log.append({"t": round(time.time() - t0, 2), "state": label})
        print(log[-1], flush=True)
        apply(cmds)
        nxt = t0 + lead + sum(HOLDS[x["state"]] for x in log)
        time.sleep(max(0.0, nxt - time.time()))
    proc.wait()
    (SCRATCH / "themes.json").write_text(json.dumps({"frame": f, "lead": lead, "holds": HOLDS, "marks": log}, indent=1))
    print("recorded", p)


def cut(start="2.35", length="12.3"):
    # fps before trim: screencapture writes frames only on change, so the filter fills the holds.
    # CRF 28.5 keeps the file near the 17 November loop's weight (the dithered art costs bits).
    src = SCRATCH / "themes.mov"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(src), "-an", "-vf",
                    f"fps=30,trim=start={start}:duration={length},setpts=PTS-STARTPTS,"
                    "scale=1600:-2:flags=lanczos,format=yuv420p",
                    "-c:v", "libx264", "-profile:v", "high", "-preset", "slow", "-crf", "28.5",
                    "-movflags", "+faststart", str(OUT / "themes.mp4")], check=True)
    png = SCRATCH / "poster.png"
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(OUT / "themes.mp4"), "-frames:v", "1", str(png)], check=True)
    subprocess.run(["cwebp", "-quiet", "-q", "86", "-m", "6", str(png), "-o", str(OUT / "themes-poster.webp")], check=True)
    print("cut", OUT / "themes.mp4")


def states():
    SCRATCH.mkdir(parents=True, exist_ok=True)
    for label, cmds in STATES:
        apply(cmds)
        ctl("wait", "1500")
        run("screenshot", label, "--real", "--output", str(SCRATCH / f"state-{label}.png"))
        print(label, flush=True)


if __name__ == "__main__":
    cmd, args = sys.argv[1], sys.argv[2:]
    {"scene": scene, "record": record, "cut": cut, "states": states}[cmd](*args)
