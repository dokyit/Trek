#!/usr/bin/env python3
"""Each agent app on the owner's own plan, the way it is really used, on one fixed set of tasks.

Tasks: 10 Python exercises of Aider's polyglot benchmark (github.com/Aider-AI/polyglot-benchmark,
commit 7e0611e), chosen before any run. Each agent gets the exercise's instructions and its stub
in a fresh folder; the test file is held back and run afterwards with unittest. One attempt,
10 minutes, no hints. Apps run side by side, each app's tasks one after another; an app's runs
stop at once if it reports a usage limit, so its plan isn't drained.

Models differ: each app runs the default model of the plan it is signed in with (recorded where
the app reports it). Trek is the real app (isolated profile, real agents on) driving Claude Code.

Usage: python3 -I subs_run.py <polyglot_dir> <work_dir> <out.json> [--start-at HH:MM] [app ...]
"""
import importlib.machinery, importlib.util, json, os, re, shutil, subprocess, sys, threading, time
from datetime import datetime
from pathlib import Path

TASKS = os.environ["SUBS_TASKS"].split(",") if os.environ.get("SUBS_TASKS") else ["book-store", "bowling", "dominoes", "forth", "go-counting", "poker", "react", "rest-api", "sgf-parsing", "zebra-puzzle"]
LIMIT_S = 600
LIMIT_RE = re.compile(r"usage limit|rate[ -]?limit|quota|limit reached|reached your|too many requests|\b429\b|out of credits|upgrade (your|to)|weekly limit|5-hour|insufficient", re.I)

TASK = """{instructions}

Implement the solution in `{stub}` in this folder, replacing the stub. Use only the Python standard library.
Keep the existing function and class names: hidden unit tests will import them from `{module}`.
Don't create a test file here. Work on your own until it is done."""

# Every app gets the same one-line message; the task itself is in TASK.md (Trek's test harness
# sends one line per command, so the long text goes in a file for everyone alike).
PROMPT = "Do the task described in TASK.md in this folder. Work on your own until it is done; don't ask me anything."


def apps(work):
    return {
        "Claude Code": lambda p: ["claude", "-p", p, "--permission-mode", "bypassPermissions", "--strict-mcp-config", "--output-format", "json"],
        "Codex": lambda p: ["codex", "exec", "--json", "--sandbox", "workspace-write", "--skip-git-repo-check", p],
        "GitHub Copilot": lambda p: ["copilot", "-p", p, "--allow-all"],
        "Gemini CLI": lambda p: ["gemini", "-p", p, "--yolo", "--skip-trust"],
        "Antigravity CLI": lambda p: [os.path.expanduser("~/.local/bin/agy"), "--print", p, "--dangerously-skip-permissions", "--output-format", "json"],
        "Devin": lambda p: [os.path.expanduser("~/.local/bin/devin"), "-p", "--permission-mode", "dangerous", "--respect-workspace-trust", "false", "--", p],
        "Trek (Claude Code inside Trek)": None,  # driven through the isolated app below
    }


def prepare(src, dst):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns(".meta", "*_test.py", "__pycache__"))
    subprocess.run(["git", "init", "-q"], cwd=dst)
    subprocess.run(["git", "add", "."], cwd=dst)
    subprocess.run(["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", "commit", "-qm", "stub"], cwd=dst)
    docs = src / ".docs"
    text = (docs / "instructions.md").read_text()
    if (docs / "instructions.append.md").exists():
        text += "\n" + (docs / "instructions.append.md").read_text()
    stub = next(p.name for p in src.glob("*.py") if not p.name.endswith("_test.py"))
    (dst / "TASK.md").write_text(TASK.format(instructions=text.strip(), stub=stub, module=stub[:-3]))
    subprocess.run(["git", "add", "TASK.md"], cwd=dst)
    subprocess.run(["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", "commit", "-qm", "task"], cwd=dst)
    return PROMPT


def grade(src, dst):
    test = next(src.glob("*_test.py"))
    shutil.copy2(test, dst / test.name)
    total = len(re.findall(r"^\s+def test_", test.read_text(), re.M))
    try:
        r = subprocess.run([sys.executable, "-m", "unittest", test.stem], cwd=dst, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        return {"tests_total": total, "tests_passed": 0, "passed": False, "grader": "timeout"}
    m = re.search(r"Ran (\d+) test", out)
    ran = int(m.group(1)) if m else 0
    f = sum(int(x) for x in re.findall(r"(?:failures|errors)=(\d+)", out))
    passed_n = max(ran - f, 0) if ran else 0
    return {"tests_total": total or ran, "tests_passed": passed_n, "passed": r.returncode == 0 and ran > 0, "grader": out[-300:]}


def model_of(name, out):
    pats = [r'"model"\s*:\s*"([^"]+)"', r'"modelUsage"\s*:\s*\{\s*"([^"]+)"', r"model[:=]\s*([A-Za-z0-9._/-]+)"]
    for p in pats:
        m = re.search(p, out)
        if m:
            return m.group(1)
    return None


def run_cli(name, argv_of, polyglot, work, results, lock):
    res = results.setdefault(name, {"runs": []})
    for t in TASKS:
        src = polyglot / t
        dst = work / re.sub(r"\W+", "-", name.lower()) / t
        prompt = prepare(src, dst)
        began = time.time()
        try:
            r = subprocess.run(argv_of(prompt), cwd=dst, capture_output=True, text=True, timeout=LIMIT_S, stdin=subprocess.DEVNULL)
            out, rc = (r.stdout or "") + "\n" + (r.stderr or ""), r.returncode
        except subprocess.TimeoutExpired as e:
            out, rc = ((e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")), "timeout"
        except FileNotFoundError as e:
            out, rc = str(e), "missing"
        took = round(time.time() - began, 1)
        g = grade(src, dst)
        run = {"task": t, "secs": took, "rc": rc, "model": model_of(name, out), **g, "tail": out[-400:]}
        limited = bool(LIMIT_RE.search(out)) and (rc != 0 or len(out) < 2000)
        run["usage_limit"] = limited
        with lock:
            res["runs"].append(run)
            print(f"{name} {t}: {'PASS' if g['passed'] else 'fail'} {g['tests_passed']}/{g['tests_total']} {took}s rc={rc}{' LIMIT' if limited else ''}", flush=True)
        if limited or rc == "missing":
            res["stopped"] = f"after {t}: {'usage limit reported' if limited else 'not installed'}"
            break


def run_trek(name, polyglot, work, results, lock):
    """The isolated Trek, real agents on, Claude Code as the default agent, full access."""
    here = Path(__file__).resolve().parent
    loader = importlib.machinery.SourceFileLoader("agentcli", str(here.parents[1] / "bin" / "agent-cli"))
    spec = importlib.util.spec_from_loader("agentcli", loader)
    ac = importlib.util.module_from_spec(spec)
    loader.exec_module(ac)
    res = results.setdefault(name, {"runs": []})
    t_ = ac.PerfTrek(os.environ["TREK_BIN"], work, "trek-subs", force_active=True, test_display=True)
    (t_.data / "settings.toml").write_text(ac.PERF_SETTINGS.replace('default_agent = "direct:mock"', 'default_agent = "claude-code"\nhand_holding = "full-access"'))
    if not os.environ.get("SUBS_TREK_MOCK"):  # SUBS_TREK_MOCK=1: plumbing check on the mock agent, no plan used
        t_.env.pop("TREK_MOCK_AGENT", None)
        t_.env["TREK_REAL_AGENTS"] = "1"
    else:
        (t_.data / "settings.toml").write_text(ac.PERF_SETTINGS.replace('default_agent = "direct:mock"', 'default_agent = "direct:mock"\nhand_holding = "full-access"'))
    with t_:
        t_.wait_line("startup", timeout=120)
        time.sleep(5)
        for t in TASKS:
            src = polyglot / t
            dst = work / "trek" / t
            prompt = prepare(src, dst)
            began = time.time()
            err = None
            try:
                # One line per command: the prompt is sent as one message, its newlines kept by the composer.
                t_.batch([f"route project:{dst}", "new", "send " + prompt], timeout=60)
                t_.batch([f"wait idle {LIMIT_S * 1000}"], timeout=LIMIT_S + 30)
            except Exception as e:
                err = str(e)[:300]
            took = round(time.time() - began, 1)
            log = "\n".join(p.read_text(errors="replace")[-20000:] for p in (t_.data / "logs").glob("trek-*.log"))
            limited = bool(re.search(r"usage limit|rate[ -]?limit|limit reached|reached your", log[-4000:], re.I))
            g = grade(src, dst)
            run = {"task": t, "secs": took, "rc": err or 0, "model": "Claude Code default (Trek's agent)", **g, "usage_limit": limited}
            with lock:
                res["runs"].append(run)
                print(f"{name} {t}: {'PASS' if g['passed'] else 'fail'} {g['tests_passed']}/{g['tests_total']} {took}s{' LIMIT' if limited else ''}{' ERR ' + err if err else ''}", flush=True)
            if limited:
                res["stopped"] = f"after {t}: usage limit reported"
                break


def main():
    polyglot, work, out_path = Path(sys.argv[1]) / "python/exercises/practice", Path(sys.argv[2]), sys.argv[3]
    rest = sys.argv[4:]
    start_at = None
    if rest[:1] == ["--start-at"]:
        start_at, rest = rest[1], rest[2:]
    work.mkdir(parents=True, exist_ok=True)
    if start_at:
        hh, mm = map(int, start_at.split(":"))
        now = datetime.now()
        target = now.replace(hour=hh, minute=mm, second=0, microsecond=0)
        wait = (target - now).total_seconds()
        if wait > 0:
            print(f"waiting until {start_at} ({int(wait)} s)", flush=True)
            time.sleep(wait)
    results, lock = {"started": datetime.now().isoformat(timespec="seconds"), "tasks": TASKS, "limit_s": LIMIT_S}, threading.Lock()
    threads = []
    for name, argv_of in apps(work).items():
        if rest and name not in rest:
            continue
        target = (lambda n=name: run_trek(n, polyglot, work, results, lock)) if argv_of is None else (lambda n=name, a=argv_of: run_cli(n, a, polyglot, work, results, lock))
        th = threading.Thread(target=target, daemon=True)
        th.start()
        threads.append(th)
    for th in threads:
        th.join()
    results["finished"] = datetime.now().isoformat(timespec="seconds")
    json.dump(results, open(out_path, "w"), indent=1)
    print("done", flush=True)


if __name__ == "__main__":
    main()
