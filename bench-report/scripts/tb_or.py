#!/usr/bin/env python3
"""Same-model head-to-head through OpenRouter: Trek's Direct harness, OpenCode, Droid (BYOK),
Claude Code (through LiteLLM, scripts/litellm-openrouter.yaml) and Codex, all on DeepSeek V4 Flash,
on 15 Terminal-Bench CPU-only tasks. One trial each, a 25-minute hard cap for every harness
(Harbor's agent timeout, multiplier 1500/28800), two runs at a time, task by task (every harness
finishes a task before the next starts), harness order rotated per task. A guard polls OpenRouter
and stops everything at the spend cap (CAP_USD) or when the key's own limit is nearly used.

Usage: python3 -I tb_local.py <tb_dir> <jobs_dir> <out.json>   (env: PROXY_KEY_FILE, TREK_BENCH)
Needs: the proxy on :4300, Ollama on :11435, Colima, Harbor 0.25 with the earlier study's adapters
(~/Developer/trek-bench-study/bench on PYTHONPATH, harbor_trek installed in Harbor's env).
"""
import json, os, subprocess, sys, time
from pathlib import Path

TASKS = ["cargo-flight-dispatch", "production-planning", "foodstuff-beta-activity", "risk-scorer-replay", "photonic-waveguide-routing",
         "bun-sourcemap-leak", "roy-polymorph-cn", "music-harmony", "embedding-drift-monitor", "fin-saccr-rwa", "sound-change-cascade",
         "interleaved-vigenere", "mvcc-lsm-compaction", "react-lead-form", "wal-recovery-ordering"]
CAP_S = 1500
MULT = f"{CAP_S / 28800:.6f}"
HOST = "host.docker.internal"
STUDY = Path.home() / "Developer/trek-bench-study"


def harness_args(name, key_file, trek_bench):
    model = "deepseek/deepseek-v4-flash"
    if name == "nop":  # the task as it starts: no agent, no model (the verifier's baseline)
        return ["-a", "nop"]
    if name == "trek-direct":
        return ["-a", "harbor_trek.agent:TrekAgent", "-m", model, "--ak", f"model={model}", "--ak", "effort=high",
                "--ak", "max_turns=200", "--ak", f"time_limit_s={CAP_S}", "--ak", f"trek_bench_path={trek_bench}"]
    if name == "opencode":
        return ["-a", "opencode", "-m", f"openrouter/{model}", "--api-key-name", "OPENROUTER_API_KEY"]
    if name == "claude-code":
        return ["-a", "claude-code", "-m", "litellm/deepseek-v4-flash", "--base-url", f"http://{HOST}:4300", "--api-format", "anthropic_messages",
                "--api-key-name", "BENCH_PROXY_KEY", "--allow-agent-host", HOST]
    if name == "droid":
        return ["-a", "harbor_droid.agent:DroidAgent", "-m", model]
    if name == "codex":
        return ["-a", "codex", "-m", f"openrouter/{model}", "--ak", f"config={STUDY / 'bench/codex-openrouter.toml'}",
                "--api-key-name", "OPENROUTER_API_KEY", "--effort", "high"]
    raise ValueError(name)


HARNESSES = ["trek-direct", "opencode", "droid", "claude-code", "codex"]


def load():
    return float(subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.split()[1])


def summarize(trial_dir):
    r = {}
    try:
        d = json.load(open(trial_dir / "result.json"))
    except (OSError, ValueError):
        return {"result": "missing"}
    vr = d.get("verifier_result") or {}
    r["reward"] = (vr.get("rewards") or {}).get("reward") if isinstance(vr.get("rewards"), dict) else vr.get("rewards")
    exc = d.get("exception_info") or {}
    r["exception"] = (exc.get("exception_type") or "") + (": " + exc.get("exception_message", "")[:200] if exc else "")
    ctrf = trial_dir / "verifier" / "ctrf.json"
    if ctrf.exists():
        try:
            s = json.load(open(ctrf))["results"]["summary"]
            r["tests"] = f'{s.get("passed", 0)}/{s.get("tests", 0)}'
        except (ValueError, KeyError):
            pass
    ar = d.get("agent_result") or {}
    r["tokens_in"], r["tokens_out"] = ar.get("n_input_tokens"), ar.get("n_output_tokens")
    try:
        r["agent_secs"] = round((time.mktime(time.strptime(d["agent_execution"]["finished_at"][:19], "%Y-%m-%dT%H:%M:%S"))
                                 - time.mktime(time.strptime(d["agent_execution"]["started_at"][:19], "%Y-%m-%dT%H:%M:%S"))))
    except (KeyError, TypeError, ValueError):
        pass
    return r


def credits(key):
    import urllib.request
    def get(path):
        req = urllib.request.Request("https://openrouter.ai/api/v1/" + path, headers={"Authorization": f"Bearer {key}"})
        return json.load(urllib.request.urlopen(req, timeout=30))["data"]
    c, k = get("credits"), get("key")
    return c["total_credits"] - c["total_usage"], k.get("limit_remaining"), k["usage"]


def main():
    import threading
    from concurrent.futures import ThreadPoolExecutor
    tb, jobs, out_path = sys.argv[1], Path(sys.argv[2]), sys.argv[3]
    key_file, trek_bench = os.environ["PROXY_KEY_FILE"], os.environ["TREK_BENCH"]
    or_key = open(os.path.expanduser("~/.trek-bench/openrouter.key")).read().strip()
    cap = float(os.environ.get("CAP_USD", "18"))
    jobs.mkdir(parents=True, exist_ok=True)
    res = json.load(open(out_path)) if os.path.exists(out_path) else {"cap_s": CAP_S, "model": "deepseek/deepseek-v4-flash via OpenRouter", "runs": []}
    bal0, rem0, use0 = credits(or_key)
    res.setdefault("spend_start", {"balance": bal0, "key_limit_remaining": rem0, "key_usage": use0, "at": time.strftime("%Y-%m-%d %H:%M")})
    start_use = res["spend_start"]["key_usage"]
    done = {(r["task"], r["harness"]) for r in res["runs"]}
    env = {**os.environ, "PYTHONPATH": str(STUDY / "bench"), "BENCH_PROXY_KEY": open(key_file).read().strip(), "OPENROUTER_API_KEY": or_key}
    lock, stop = threading.Lock(), threading.Event()

    def guard():
        while not stop.is_set():
            try:
                bal, rem, use = credits(or_key)
                spent = use - start_use
                with lock:
                    res["spent_usd"] = round(spent, 3)
                if spent >= cap or bal < 0.3 or (rem is not None and rem < 0.3):
                    res["stopped_by_guard"] = f"spent {spent:.2f}, balance {bal:.2f}, key remaining {rem}"
                    print("GUARD: " + res["stopped_by_guard"], flush=True)
                    stop.set()
                    subprocess.run(["pkill", "-TERM", "-f", f"harbor run -p {tb}"])
                    return
            except Exception as e:  # keep guarding through a failed poll
                print(f"guard poll failed: {e}", flush=True)
            time.sleep(45)

    threading.Thread(target=guard, daemon=True).start()

    def one(task, h):
        if stop.is_set():
            return
        name = f"{h}__{task}"
        argv = ["harbor", "run", "-p", tb, "-k", "1", "-n", "1", "--agent-timeout-multiplier", MULT, "-o", str(jobs),
                "--job-name", name, "-i", task, "-y", *harness_args(h, key_file, trek_bench)]
        began, l0 = time.time(), load()
        p = subprocess.run(argv, cwd=STUDY, env=env, capture_output=True, text=True, timeout=CAP_S + 3600)
        trials = [d for d in (jobs / name).iterdir() if d.is_dir() and d.name.startswith(task)] if (jobs / name).exists() else []
        s = summarize(trials[0]) if trials else {"result": "no trial", "harbor_tail": (p.stdout + p.stderr)[-400:]}
        run = {"task": task, "harness": h, "wall_s": round(time.time() - began), "load_before": l0, "rc": p.returncode, **s}
        if stop.is_set():
            run["note"] = "stopped by the spend guard"
        with lock:
            res["runs"].append(run)
            json.dump(res, open(out_path, "w"), indent=1)
        print(json.dumps(run), flush=True)

    with ThreadPoolExecutor(max_workers=2) as pool:
        for i, task in enumerate(TASKS):
            if stop.is_set():
                break
            order = ["nop"] + HARNESSES[i % 5:] + HARNESSES[:i % 5]
            todo = [h for h in order if (task, h) not in done]
            list(pool.map(lambda h: one(task, h), todo))  # every harness finishes this task before the next task
    stop.set()
    bal, rem, use = credits(or_key)
    res["spend_end"] = {"balance": bal, "key_limit_remaining": rem, "key_usage": use, "spent_usd": round(use - start_use, 3), "at": time.strftime("%Y-%m-%d %H:%M")}
    json.dump(res, open(out_path, "w"), indent=1)
    print("done", json.dumps(res["spend_end"]), flush=True)


if __name__ == "__main__":
    main()
