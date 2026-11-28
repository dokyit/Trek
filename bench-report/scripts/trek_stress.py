#!/usr/bin/env python3
"""Stress runs of an isolated Trek (mock agent, throwaway data, never the owner's Trek).

Reuses agent-cli's PerfTrek harness (bin/agent-cli). Every run is a release build with the
`shots` feature, launched with TREK_DATA_DIR in a temp folder and the mock agent only.

  threads  N in THREADS (default 100,1000,5000): launch to first frame (median of RUNS after one
           thrown away), footprint/RSS after 25 s, idle CPU, sidebar scroll frame times, opening
           the oldest thread, typing latency in the composer
  swarm    K in SWARM (default 10,20,50) threads streaming at once (mock:stream), then K threads
           each starting a sub-agent (mock:delegate): memory, CPU, frame times, typing latency
           and thread-open time during the load, and whether every turn ended
  browser  N in TABS (default 50,100,200) Browser tabs of a local page: time per tab, Trek's
           footprint plus every WebKit process that appeared, and whether the panel still answers

Usage: python3 -I trek_stress.py <trek_bin> <perf_seed_bin> <out.json> [threads|swarm|browser ...]
Records the 1-minute load average with every number.
"""
import importlib.machinery, importlib.util, json, os, subprocess, sys, tempfile, time, statistics, http.server, threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
loader = importlib.machinery.SourceFileLoader("agentcli", str(ROOT / "bin" / "agent-cli"))
spec = importlib.util.spec_from_loader("agentcli", loader)
ac = importlib.util.module_from_spec(spec)
loader.exec_module(ac)
PerfTrek, perf_seed, median = ac.PerfTrek, ac.perf_seed, ac.median

RUNS = int(os.environ.get("RUNS", "3"))
SETTLE = int(os.environ.get("SETTLE", "25"))


def load():
    return float(subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.split()[1])


def webkit_pids():
    out = subprocess.run(["ps", "-axo", "pid=,comm="], capture_output=True, text=True).stdout
    return {int(l.split(None, 1)[0]) for l in out.splitlines() if "com.apple.WebKit" in l}


def children(pid):
    out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout
    return [int(x) for x in out.split()]


def footprint_mb(pids):
    import re
    total = 0
    for p in pids:
        out = subprocess.run(["footprint", "-p", str(p), "-f", "bytes", "--noCategories"], capture_output=True, text=True).stdout
        m = re.search(r"phys_footprint:\s+(\d+)", out)
        if m:
            total += int(m.group(1))
    return round(total / 2**20, 1)


def rss_mb(pids):
    if not pids:
        return 0
    out = subprocess.run(["ps", "-o", "rss=", "-p", ",".join(map(str, pids))], capture_output=True, text=True).stdout
    return round(sum(int(x) for x in out.split()) / 1024, 1)


def line(t, kind, label):
    try:
        return t.wait_line(kind, timeout=300, label=label)
    except Exception as e:
        return {"error": str(e)}


def run_threads(trek, seed_bin, work, say):
    out = {}
    for n in [int(x) for x in os.environ.get("THREADS", "100,1000,5000").split(",")]:
        t0 = time.time()
        seed = perf_seed(seed_bin, work, n, 4)
        r = {"seed_secs": round(time.time() - t0, 1), "load": load()}
        launches = []
        for i in range(RUNS + 1):
            with PerfTrek(trek, work, f"thr-{n}", seed) as t:
                s = t.wait_line("startup", timeout=300)
                if i:
                    launches.append(float(s["at"]) - t.spawned_ms)
        r["launch_ms_runs"] = [round(x) for x in launches]
        r["launch_ms"] = round(median(launches))
        with PerfTrek(trek, work, f"thr-{n}-idle", seed, poll_ms=5000) as t:
            t.wait_line("startup", timeout=300)
            time.sleep(SETTLE)
            r["rss_mb"] = round(t.sample()[0])
            r["footprint_mb"] = t.footprint() and round(t.footprint())
            cpu, sw = t.over(30)
            r["idle_cpu_pct"] = round(cpu, 2)
        with PerfTrek(trek, work, f"thr-{n}-drawn", seed, force_active=True, test_display=True) as t:
            t.wait_line("startup", timeout=300)
            time.sleep(SETTLE)
            r["drawn_footprint_mb"] = t.footprint() and round(t.footprint())
            t.batch(["perf scroll sidebar 120 300 sidebar-down"], timeout=300)
            s = line(t, "scroll", "sidebar-down")
            r["sidebar_scroll"] = {k: s.get(k) for k in ("frames", "frame_p50", "frame_p95", "latency_p95", "unpainted", "error") if k in s}
            # The oldest of the seeded threads, far down the list.
            verbs = ["Fix", "Add", "Refactor", "Speed up", "Document", "Test", "Clean up", "Migrate", "Profile", "Review"]
            nouns = ["the login flow", "pagination", "the build cache", "retry backoff", "the settings page", "CSV export",
                     "the websocket client", "search ranking", "error toasts", "the release script"]
            i = n - 1
            r["opened_title"] = f"{verbs[i % 10]} {nouns[(i // 10) % 10]} #{i}"
            t.batch([f"perf open {r['opened_title']}"], timeout=300)
            o = line(t, "open", None)
            r["open_thread"] = {k: o.get(k) for k in ("loaded", "painted", "error") if k in o}
            t.batch(["new", "wait 1000", "perf type 60 typing"], timeout=300)
            ty = line(t, "type", "typing")
            r["typing"] = {k: ty.get(k) for k in ("latency_p50", "latency_p95", "unpainted", "frames", "error") if k in ty}
            r["footprint_after_mb"] = t.footprint() and round(t.footprint())
        r["load_after"] = load()
        out[str(n)] = r
        say(f"threads {n}: {json.dumps(r)}")
    return out


def run_swarm(trek, seed_bin, work, say):
    out = {}
    seed = perf_seed(seed_bin, work, 100, 4)
    project = f"{seed}/projects/atlas-api"
    for kind, prompt in (("stream", "mock:stream {secs}s"), ("subagents", "mock:delegate implement Review how the app starts")):
        for k in [int(x) for x in os.environ.get("SWARM", "10,20,50").split(",")]:
            secs = 90
            r = {"load": load(), "agents": k}
            with PerfTrek(trek, work, f"swarm-{kind}-{k}", seed, force_active=True, test_display=True) as t:
                t.wait_line("startup", timeout=300)
                time.sleep(10)
                rss0, cpu0 = t.sample()
                r["footprint_before_mb"] = t.footprint() and round(t.footprint())
                lines = ["pace 1", f"route project:{project}"]
                for i in range(k):
                    lines += ["new", f"send {prompt.format(secs=secs)}"]
                began = time.time()
                t.batch(lines, timeout=600)
                r["start_all_secs"] = round(time.time() - began, 2)
                time.sleep(15)
                t.batch(["perf sessions swarm"], timeout=60)
                ses = line(t, "sessions", "swarm")
                r["sessions_line"] = {k2: v for k2, v in ses.items() if k2 != "kind"}
                t.batch(["perf frames 3000 swarm"], timeout=120)
                f = line(t, "frames", "swarm")
                r["frames"] = {k2: f.get(k2) for k2 in ("frames", "frame_p50", "frame_p95", "lag_p95", "error") if k2 in f}
                t.batch(["perf open Fix CSV export #50"], timeout=120)
                o = line(t, "open", None)
                r["open_thread_during"] = {k2: o.get(k2) for k2 in ("loaded", "painted", "error") if k2 in o}
                t.batch(["new", "wait 500", "perf type 40 swarm-typing"], timeout=120)
                ty = line(t, "type", "swarm-typing")
                r["typing_during"] = {k2: ty.get(k2) for k2 in ("latency_p50", "latency_p95", "unpainted", "error") if k2 in ty}
                peak = 0
                while time.time() - began < secs - 5:
                    peak = max(peak, t.sample()[0])
                    time.sleep(5)
                rss1, cpu1 = t.sample()
                r["footprint_peak_mb"] = t.footprint() and round(t.footprint())
                r["rss_peak_mb"] = round(max(peak, rss1))
                r["cpu_pct_of_one_core"] = round((cpu1 - cpu0) / (time.time() - began) * 100, 1)
                r["child_processes"] = len(children(t.proc.pid))
                try:
                    t.batch([f"wait idle {(secs + 120) * 1000}"], timeout=secs + 200)
                    r["all_turns_ended"] = True
                except Exception as e:
                    r["all_turns_ended"] = f"no: {e}"
                r["rss_after_mb"] = round(t.sample()[0])
            r["load_after"] = load()
            out[f"{kind}-{k}"] = r
            say(f"swarm {kind} {k}: {json.dumps(r)}")
    return out


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def run_browser(trek, seed_bin, work, say):
    out = {}
    site = Path(work) / "site"
    site.mkdir(exist_ok=True)
    body = "".join(f"<p>Paragraph {i}: " + "lorem ipsum dolor sit amet " * 20 + "</p>" for i in range(60))
    for i in range(220):
        (site / f"p{i}.html").write_text(f"<!doctype html><title>Page {i}</title><h1>Page {i}</h1>{body}"
                                         "<script>let n=0;setInterval(()=>{n++},1000)</script>")
    handler = lambda *a, **kw: Quiet(*a, directory=str(site), **kw)
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    seed = perf_seed(seed_bin, work, 10, 4)
    for n in [int(x) for x in os.environ.get("TABS", "50,100,200").split(",")]:
        r = {"load": load(), "tabs": n}
        wk0 = webkit_pids()
        with PerfTrek(trek, work, f"browser-{n}", seed, force_active=True, test_display=True) as t:
            t.wait_line("startup", timeout=300)
            time.sleep(5)
            t.batch(["tools browser", "wait 2000"], timeout=60)
            began = time.time()
            per = []
            for i in range(n):
                s = time.time()
                t.batch([f"browser new-tab http://127.0.0.1:{port}/p{i}.html"], timeout=120)
                per.append((time.time() - s) * 1000)
            r["open_all_secs"] = round(time.time() - began, 1)
            r["new_tab_ms_p50"] = round(statistics.median(per))
            r["new_tab_ms_p95"] = round(sorted(per)[int(len(per) * 0.95) - 1])
            r["new_tab_ms_last10_p50"] = round(statistics.median(per[-10:]))
            time.sleep(10)
            try:
                st = Path(t.shot / "browser-state.json")
                t.batch(["browser state"], timeout=60)
                r["state_ok"] = True
            except Exception as e:
                r["state_ok"] = f"no: {e}"
            try:
                t.batch([f"browser expect tabs:{n + 1}"], timeout=60)
                r["tabs_kept"] = f"{n} + the first tab"
            except Exception as e:
                r["tabs_kept"] = f"check failed: {str(e)[:200]}"
            new_wk = sorted(webkit_pids() - wk0)
            r["webkit_processes"] = len(new_wk)
            r["trek_footprint_mb"] = t.footprint() and round(t.footprint())
            r["webkit_footprint_mb"] = footprint_mb(new_wk)
            r["total_footprint_mb"] = round((r["trek_footprint_mb"] or 0) + r["webkit_footprint_mb"])
            r["webkit_rss_mb"] = rss_mb(new_wk)
            t.batch(["perf frames 3000 tabs"], timeout=60)
            f = line(t, "frames", "tabs")
            r["frames"] = {k: f.get(k) for k in ("frames", "frame_p50", "frame_p95", "error") if k in f}
            t.batch(["new", "wait 500", "perf type 40 tabs-typing"], timeout=120)
            ty = line(t, "type", "tabs-typing")
            r["typing"] = {k: ty.get(k) for k in ("latency_p50", "latency_p95", "unpainted", "error") if k in ty}
        r["load_after"] = load()
        out[str(n)] = r
        say(f"browser {n}: {json.dumps(r)}")
    httpd.shutdown()
    return out


def main():
    trek, seed_bin, out_path = sys.argv[1:4]
    what = sys.argv[4:] or ["threads", "swarm", "browser"]
    work = Path(tempfile.mkdtemp(prefix="trek-stress-"))
    # Copied out, as agent-cli perf does: another build may replace the binary mid-run.
    import shutil
    shutil.copy2(trek, work / "trek")
    trek = work / "trek"
    results = json.load(open(out_path)) if os.path.exists(out_path) else {}
    say = lambda m: print(m, flush=True)
    for w in what:
        waited = 0
        while load() > float(os.environ.get("MAX_LOAD", "5")) and waited < int(os.environ.get("MAX_WAIT", "2700")):
            print(f"waiting: load {load():.1f}", flush=True)
            time.sleep(60)
            waited += 60
        try:
            results[w] = {"threads": run_threads, "swarm": run_swarm, "browser": run_browser}[w](trek, seed_bin, work, say)
        except Exception as e:
            results[w] = {"error": repr(e), "partial": results.get(w)}
            say(f"{w} failed: {e!r}")
        results.setdefault("_meta", {})[w] = {"finished": time.strftime("%Y-%m-%d %H:%M:%S"), "work": str(work)}
        json.dump(results, open(out_path, "w"), indent=1)
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
