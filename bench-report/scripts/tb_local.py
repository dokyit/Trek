#!/usr/bin/env python3
"""Same-model head-to-head on a local model: Trek's Direct harness, OpenCode, Droid (BYOK) and
Claude Code, all on qwen3:14b (thinking off) behind one LiteLLM proxy (scripts/litellm-local.yaml),
on 15 Terminal-Bench CPU-only tasks. One trial each, an 8-minute hard cap for every harness
(Harbor's agent timeout, multiplier 480/28800), one run at a time, harness order rotated per task.

Usage: python3 -I tb_local.py <tb_dir> <jobs_dir> <out.json>   (env: PROXY_KEY_FILE, TREK_BENCH)
Needs: the proxy on :4300, Ollama on :11435, Colima, Harbor 0.25 with the earlier study's adapters
(~/Developer/trek-bench-study/bench on PYTHONPATH, harbor_trek installed in Harbor's env).
"""
import json, os, subprocess, sys, time
from pathlib import Path

TASKS = ["cargo-flight-dispatch", "production-planning", "foodstuff-beta-activity", "risk-scorer-replay", "photonic-waveguide-routing",
         "bun-sourcemap-leak", "roy-polymorph-cn", "music-harmony", "embedding-drift-monitor", "fin-saccr-rwa", "sound-change-cascade",
         "interleaved-vigenere", "mvcc-lsm-compaction", "react-lead-form", "wal-recovery-ordering"]
CAP_S = 480
MULT = f"{CAP_S / 28800:.6f}"
HOST = "host.docker.internal"
STUDY = Path.home() / "Developer/trek-bench-study"


def harness_args(name, key_file, trek_bench):
    proxy_in = f"http://{HOST}:4300"
    if name == "nop":  # the task as it starts: no agent, no model (the verifier's baseline)
        return ["-a", "nop"]
    if name == "trek-direct":
        return ["-a", "harbor_trek.agent:TrekAgent", "-m", "qwen3-14b", "--ak", "model=qwen3-14b", "--ak", "provider=litellm", "--ak", "wire=openai",
                "--ak", "provider_base_url=http://127.0.0.1:4300/v1", "--ak", f"api_key_file={key_file}", "--ak", "effort=high",
                "--ak", "max_turns=200", "--ak", f"time_limit_s={CAP_S}", "--ak", "command_timeout_s=300", "--ak", f"trek_bench_path={trek_bench}"]
    if name == "opencode":
        # A provider of its own name: under "openai" OpenCode calls the Responses API, which the proxy route lacks.
        return ["-a", "opencode", "-m", "local/qwen3-14b", "--base-url", f"{proxy_in}/v1", "--api-format", "openai_chat_completions",
                "--api-key-name", "BENCH_PROXY_KEY", "--allow-agent-host", HOST]
    if name == "claude-code":
        return ["-a", "claude-code", "-m", "litellm/qwen3-14b", "--base-url", proxy_in, "--api-format", "anthropic_messages",
                "--api-key-name", "BENCH_PROXY_KEY", "--allow-agent-host", HOST]
    if name == "droid":
        return ["-a", "harbor_droid.agent:DroidAgent", "-m", "qwen3-14b", "--ak", "model=qwen3-14b", "--ak", f"endpoint={proxy_in}/v1",
                "--ak", "api_key_env=BENCH_PROXY_KEY", "--allow-agent-host", HOST]
    raise ValueError(name)


HARNESSES = ["trek-direct", "opencode", "droid", "claude-code"]


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


def main():
    tb, jobs, out_path = sys.argv[1], Path(sys.argv[2]), sys.argv[3]
    key_file, trek_bench = os.environ["PROXY_KEY_FILE"], os.environ["TREK_BENCH"]
    key = open(key_file).read().strip()
    jobs.mkdir(parents=True, exist_ok=True)
    only = os.environ.get("ONLY_TASKS")
    tasks = only.split(",") if only else TASKS
    res = json.load(open(out_path)) if os.path.exists(out_path) else {"cap_s": CAP_S, "model": "qwen3:14b (Q4_K_M, thinking off), Ollama 0.40.18, 40K context", "runs": []}
    done = {(r["task"], r["harness"]) for r in res["runs"]}
    env = {**os.environ, "PYTHONPATH": str(STUDY / "bench"), "BENCH_PROXY_KEY": key}
    for i, task in enumerate(tasks):
        order = ["nop"] + HARNESSES[i % 4:] + HARNESSES[:i % 4]
        for h in order:
            if (task, h) in done:
                continue
            while load() > 20:
                time.sleep(30)
            name = f"{h}__{task}"
            argv = ["harbor", "run", "-p", tb, "-k", "1", "-n", "1", "--agent-timeout-multiplier", MULT, "-o", str(jobs),
                    "--job-name", name, "-i", task, "-y", *harness_args(h, key_file, trek_bench)]
            began, l0 = time.time(), load()
            p = subprocess.run(argv, cwd=STUDY, env=env, capture_output=True, text=True, timeout=CAP_S + 3600)
            trials = [d for d in (jobs / name).iterdir() if d.is_dir() and d.name.startswith(task)] if (jobs / name).exists() else []
            s = summarize(trials[0]) if trials else {"result": "no trial", "harbor_tail": (p.stdout + p.stderr)[-400:]}
            run = {"task": task, "harness": h, "wall_s": round(time.time() - began), "load_before": l0, "rc": p.returncode, **s}
            res["runs"].append(run)
            json.dump(res, open(out_path, "w"), indent=1)
            print(json.dumps(run), flush=True)


if __name__ == "__main__":
    main()
