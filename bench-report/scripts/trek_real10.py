#!/usr/bin/env python3
"""Synara's post (#12) in Trek: 10 threads open with real Claude Code sessions, 4 of them streaming at once.

Isolated Trek (throwaway data folder, imports and notifications off, gh signed out) with
TREK_REAL_AGENTS=1, so turns run on the owner's Claude Code login (subscription, no API spend).
A throwaway git project folder is used, so the sessions land in a project key of their own.

Measures: total footprint of Trek and every process under it (Claude Code CLIs, Trek's MCP
helpers), the CLI count, and Trek's own footprint, (a) with 6 threads idle after one short turn
each and (b) at the peak while 4 more stream a long answer (10 open).
Usage: python3 -I trek_real10.py <trek_bin> <out.json>
"""
import importlib.machinery, importlib.util, json, os, re, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader("agentcli", str(ROOT / "bin" / "agent-cli"))
spec = importlib.util.spec_from_loader("agentcli", loader)
ac = importlib.util.module_from_spec(spec)
loader.exec_module(ac)


def descendants(pid):
    out = subprocess.run(["ps", "-axo", "pid=,ppid=,comm="], capture_output=True, text=True).stdout
    kids, names = {}, {}
    for line in out.splitlines():
        p, pp, c = line.split(None, 2)
        kids.setdefault(int(pp), []).append(int(p))
        names[int(p)] = c
    todo, seen = [pid], []
    while todo:
        p = todo.pop()
        seen.append(p)
        todo += kids.get(p, [])
    return seen, names


def fp(p):
    out = subprocess.run(["footprint", "-p", str(p), "-f", "bytes", "--noCategories"], capture_output=True, text=True).stdout
    m = re.search(r"phys_footprint:\s+(\d+)", out)
    return int(m.group(1)) / 2**20 if m else 0.0


def snapshot(pid):
    pids, names = descendants(pid)
    per = {}
    total = 0.0
    for p in pids:
        f = fp(p)
        total += f
        n = os.path.basename(names.get(p, "?"))
        per.setdefault(n, [0, 0.0])
        per[n][0] += 1
        per[n][1] += f
    return {"total_mb": round(total), "trek_mb": round(fp(pid)),
            "by_process": {k: {"count": v[0], "mb": round(v[1])} for k, v in sorted(per.items(), key=lambda kv: -kv[1][1])}}


def main():
    trek, out_path = sys.argv[1], sys.argv[2]
    work = Path(tempfile.mkdtemp(prefix="trek-real10-"))
    proj = work / "real10-project"
    proj.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=proj)
    (proj / "README.md").write_text("A throwaway project for a memory benchmark.\n")
    subprocess.run(["git", "add", "."], cwd=proj)
    subprocess.run(["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", "commit", "-qm", "init"], cwd=proj)
    t = ac.PerfTrek(trek, work, "real10", force_active=True, test_display=True)
    (t.data / "settings.toml").write_text(ac.PERF_SETTINGS.replace('default_agent = "direct:mock"', 'default_agent = "claude-code"'))
    t.env.pop("TREK_MOCK_AGENT", None)
    t.env["TREK_REAL_AGENTS"] = "1"
    res = {"load": float(subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.split()[1])}
    with t:
        t.wait_line("startup", timeout=120)
        time.sleep(10)
        res["empty"] = snapshot(t.proc.pid)
        lines = [f"route project:{proj}"]
        for i in range(6):
            lines += ["new", f"send Reply with just the word ok. (thread {i + 1})"]
        t.batch(lines, timeout=300)
        try:
            t.batch(["wait idle 300000"], timeout=330)
        except Exception as e:
            res["idle_wait"] = str(e)[:200]
        time.sleep(20)
        res["six_idle"] = snapshot(t.proc.pid)
        lines = []
        for i in range(4):
            lines += ["new", "send Write a 900-word story about a lighthouse keeper, as plain prose, without using any tools."]
        t.batch(lines, timeout=120)
        peak = None
        began = time.time()
        while time.time() - began < 150:
            time.sleep(8)
            s = snapshot(t.proc.pid)
            if not peak or s["total_mb"] > peak["total_mb"]:
                peak = s
        res["ten_open_four_streaming_peak"] = peak
        try:
            t.batch(["wait idle 300000"], timeout=330)
        except Exception as e:
            res["stream_wait"] = str(e)[:200]
        time.sleep(15)
        res["after"] = snapshot(t.proc.pid)
        log = "\n".join(p.read_text(errors="replace") for p in (t.data / "logs").glob("trek-*.log"))
        res["log_errors"] = [l[-240:] for l in log.splitlines() if " ERROR " in l][:10]
    res["note"] = ("6 threads each ran one short real Claude Code turn; then 4 more threads streamed a long answer at once "
                   "(10 threads open, 4 streaming, as in Synara's post). Footprint summed over Trek and every process under it.")
    json.dump(res, open(out_path, "w"), indent=1)
    print(json.dumps({k: (v.get("total_mb") if isinstance(v, dict) and "total_mb" in v else v) for k, v in res.items()}))


if __name__ == "__main__":
    main()
