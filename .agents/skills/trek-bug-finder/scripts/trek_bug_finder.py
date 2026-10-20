#!/usr/bin/env python3
"""Trek bug finder: the one driver for the whole method.

The method (from Anthropic's dynamic workflows): split Trek into slices, give each slice to an
investigator with a fresh context, send every finding to skeptics that try to disprove it, and keep
only what survives. The agents are run by `workflow.js` (the Claude Code Workflow tool). This script
owns everything around them, from one set of files in this skill folder:

  slices.json   what is covered (the only place slices are edited)
  workflow.js   the agent orchestration; its slice list is generated from slices.json
  ledger.json   every finding ever made, with its status (open, fixed, rejected, unverified)

Commands: domains, sync, plan, ingest, mark, report, check, test. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

SKILL = Path(__file__).resolve().parents[1]
ROOT = Path(__file__).resolve().parents[4]
SLICES_FILE = SKILL / "slices.json"
LEDGER_FILE = SKILL / "ledger.json"
WORKFLOW_FILE = SKILL / "workflow.js"
BEGIN = "// BEGIN GENERATED SLICES (from slices.json; regenerate with: trek_bug_finder.py sync)"
END = "// END GENERATED SLICES"

CODE_SUFFIXES = {".rs", ".swift", ".js", ".sh", ".py"}
NOT_SOURCE = ("vendor/", "target", "docs/", "design/", "media/", "assets/", "dist/", "ios/screenshots/",
              "crates/test-fixtures/", ".agents/skills/trek-bug-finder/")
STATUSES = ("open", "fixed", "rejected", "unverified")
SEVERITY_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3}


# ---------------------------------------------------------------- files

def load_json(path: Path, default: Any) -> Any:
    if not path.exists():
        return default
    return json.loads(path.read_text(encoding="utf-8"))


def save_json(path: Path, data: Any) -> None:
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def slices() -> List[Dict[str, Any]]:
    return load_json(SLICES_FILE, {"slices": []})["slices"]


def ledger() -> Dict[str, Any]:
    return load_json(LEDGER_FILE, {"version": 1, "findings": []})


def git_files(paths: List[str]) -> List[str]:
    out = subprocess.run(["git", "ls-files", "--"] + paths, cwd=ROOT, capture_output=True, text=True, check=True)
    return [line for line in out.stdout.splitlines() if line]


def line_count(rel: str) -> int:
    try:
        with open(ROOT / rel, "rb") as f:
            return sum(1 for _ in f)
    except OSError:
        return 0


def norm(title: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", title.lower()).strip()


# ---------------------------------------------------------------- domains

def cmd_domains(args: argparse.Namespace) -> int:
    covered = set()
    rows = []
    for s in slices():
        files = git_files(s["paths"])
        covered.update(files)
        rows.append((s, files))
    if args.uncovered:
        every = [f for f in git_files(["."])
                 if Path(f).suffix in CODE_SUFFIXES
                 and not any(f.startswith(p) for p in NOT_SOURCE)
                 and "/tests/" not in f]
        missing = [f for f in every if f not in covered]
        if args.json:
            print(json.dumps(missing, indent=2))
        else:
            print(f"{len(missing)} tracked source files are in no slice:")
            for f in missing:
                print(f"  {f}")
        return 1 if missing and args.strict else 0
    if args.json:
        print(json.dumps([{"key": s["key"], "what": s["what"], "files": len(fs), "lines": sum(line_count(f) for f in fs)} for s, fs in rows], indent=2))
        return 0
    total = 0
    for s, files in rows:
        lines = sum(line_count(f) for f in files)
        total += lines
        print(f"[{s['key']}] {s['what']}")
        print(f"  {len(files)} files, {lines} lines")
        for focus in s.get("focus", []):
            print(f"  - {focus}")
    print(f"\n{len(rows)} slices, {len(covered)} files, {total} lines. Missing coverage: domains --uncovered")
    return 0


# ---------------------------------------------------------------- sync

def render_slices_block() -> str:
    return f"{BEGIN}\nconst SLICES = {json.dumps(slices(), indent=2, ensure_ascii=False)}\n{END}"


def cmd_sync(args: argparse.Namespace) -> int:
    text = WORKFLOW_FILE.read_text(encoding="utf-8")
    pattern = re.compile(re.escape(BEGIN) + r".*?" + re.escape(END), re.S)
    if not pattern.search(text):
        print(f"workflow.js has no generated block ({BEGIN})", file=sys.stderr)
        return 2
    fresh = pattern.sub(lambda _: render_slices_block(), text)
    if fresh == text:
        print("workflow.js already matches slices.json")
        return 0
    if args.check:
        print("workflow.js is out of date with slices.json. Run: trek_bug_finder.py sync", file=sys.stderr)
        return 1
    WORKFLOW_FILE.write_text(fresh, encoding="utf-8")
    print(f"workflow.js updated from slices.json ({len(slices())} slices)")
    return 0


# ---------------------------------------------------------------- plan

def cmd_plan(args: argparse.Namespace) -> int:
    known = [{"file": f["file"], "title": f["title"], "status": f["status"], **({"reason": f["reason"]} if f.get("reason") else {})}
             for f in ledger()["findings"]]
    plan: Dict[str, Any] = {"stamp": args.stamp or "", "known": known}
    if args.slices:
        keys = {s["key"] for s in slices()}
        wanted = [k.strip() for k in args.slices.split(",") if k.strip()]
        unknown = [k for k in wanted if k not in keys]
        if unknown:
            print(f"unknown slices: {', '.join(unknown)} (known: {', '.join(sorted(keys))})", file=sys.stderr)
            return 2
        plan["slices"] = wanted
    if getattr(args, "model", None):
        plan["model"] = args.model
    if getattr(args, "effort", None):
        plan["effort"] = args.effort
    if getattr(args, "concurrency", None) is not None:
        plan["concurrency"] = args.concurrency
    if getattr(args, "max_agents", None) is not None:
        plan["maxAgents"] = args.max_agents
    if getattr(args, "max_findings", None) is not None:
        plan["maxFindings"] = args.max_findings
    if getattr(args, "drive_app", False):
        plan["driveApp"] = True
    elif getattr(args, "no_app", False):
        plan["driveApp"] = False
    for flag, key in (("investigator_model", "investigatorModel"), ("skeptic_model", "skepticModel"),
                      ("tiebreak_model", "tiebreakModel"), ("skeptic_mode", "skepticMode")):
        if getattr(args, flag, None):
            plan[key] = getattr(args, flag)
    if getattr(args, "delta", False):
        plan["deltaFiles"] = delta_files()
    extra: List[Dict[str, Any]] = []
    for path in getattr(args, "carry", None) or []:
        extra.extend(carried_findings(Path(path)))
    if extra:
        plan["extraFindings"] = extra
    out = getattr(args, "script_out", None)
    if out:
        # Carried-over findings make the args too big to paste into a tool call, so bake them into a copy of
        # the workflow and leave the printed args small. The copy is what scriptPath should point at.
        src = (SKILL / "workflow.js").read_text()
        marker = "const extraFindings = args && Array.isArray(args.extraFindings) ? args.extraFindings : []"
        if marker not in src:
            print("workflow.js has no extraFindings line to embed into", file=sys.stderr)
            return 2
        src = src.replace(marker, "const EMBEDDED_EXTRA = " + json.dumps(plan.pop("extraFindings", []))
                          + "\nconst extraFindings = args && Array.isArray(args.extraFindings) ? args.extraFindings : EMBEDDED_EXTRA")
        Path(out).write_text(src)
        print(f"wrote {out} with {len(extra)} embedded findings; pass it as scriptPath", file=sys.stderr)
    print(json.dumps(plan, indent=2))
    return 0


SOURCE_SKIP = (".agents/skills/trek-bug-finder/", "Cargo.lock", "NOTICE", ".agents/skills/verify-trek/references/")


def delta_files() -> List[str]:
    """Source files changed or added in the working tree since HEAD, for the delta investigator."""
    out = subprocess.run(["git", "diff", "--name-only", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    out += subprocess.run(["git", "ls-files", "--others", "--exclude-standard"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return sorted({f for f in out.split() if not f.startswith(SOURCE_SKIP)})


def carried_findings(path: Path) -> List[Dict[str, Any]]:
    """Investigator findings from an earlier run: its journal.jsonl (completed investigators only) or a saved result."""
    text = path.read_text()
    found: List[Dict[str, Any]] = []
    if path.suffix == ".jsonl":
        label: Dict[str, str] = {}
        for line in text.splitlines():
            if not line.strip():
                continue
            e = json.loads(line)
            if e.get("type") == "started":
                label[e.get("agentId", "")] = e.get("label", "")
            elif e.get("type") == "result" and isinstance(e.get("result"), dict):
                lbl = label.get(e.get("agentId", ""), "")
                if not lbl.startswith("investigate:"):
                    continue
                for f in e["result"].get("findings", []):
                    found.append({**f, "slice": lbl.split(":", 1)[1]})
    else:
        res = json.loads(text)
        for key in ("unverified", "confirmed"):
            for f in res.get(key, []):
                found.append({k: v for k, v in f.items() if k not in ("verdicts", "verdict", "unverified_reason")})
    return found


# ---------------------------------------------------------------- packages

def slice_of(path: str) -> str:
    """The slice whose pathspecs cover a file; findings from the delta and app-driver investigators are regrouped this way."""
    best, best_len = "misc", -1
    for sl in slices():
        for spec in sl["paths"]:
            prefix = spec.split("*")[0]
            if path.startswith(prefix) and len(prefix) > best_len:
                best, best_len = sl["key"], len(prefix)
    return best


def cmd_packages(args: argparse.Namespace) -> int:
    """Write one fixer brief per slice holding its open and unverified findings, plus the shared rules."""
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    groups: Dict[str, List[Dict[str, Any]]] = {}
    for e in ledger()["findings"]:
        if e["status"] not in ("open", "unverified"):
            continue
        groups.setdefault(slice_of(e["file"]), []).append(e)
    order = {"critical": 0, "high": 1, "medium": 2, "low": 3}
    for key, items in sorted(groups.items()):
        items.sort(key=lambda e: (e["status"] != "open", order.get(e["severity"], 9)))
        lines = [f"# Fix package: {key}", "",
                 f"{sum(e['status'] == 'open' for e in items)} confirmed findings (status open) and "
                 f"{sum(e['status'] == 'unverified' for e in items)} unverified ones.", ""]
        for e in items:
            lines += [f"## {e['id']}  [{e['status']}, {e['severity']}]  {e['file']}:{e['line']}", f"**{e['title']}**", "",
                      f"Summary: {e.get('summary', '')}", "", f"Failure scenario: {e.get('failure_scenario', '')}", "",
                      "Evidence:", str(e.get("evidence", ""))[:2500], "", f"Suggested fix: {e.get('suggested_fix', '')}", ""]
            for v in e.get("verdicts", []):
                lines.append(f"Skeptic: {v}")
            if e["status"] == "unverified":
                lines.append("Not yet checked by a skeptic. Assess it first. If it is not a real bug, say so with the reason and leave the code alone.")
            lines.append("")
        (out / f"pkg-{key}.md").write_text("\n".join(lines))
        print(f"{key:16s} open={sum(e['status'] == 'open' for e in items):2d} unverified={sum(e['status'] == 'unverified' for e in items):2d} "
              f"files={sorted({e['file'] for e in items})}")
    rules = SKILL / "fixer-rules.md"
    if rules.exists():
        (out / "fixer-rules.md").write_text(rules.read_text())
    print(f"wrote {len(groups)} packages under {out}; each fixer reads {out}/fixer-rules.md then its package")
    return 0


# ---------------------------------------------------------------- ledger

def next_id(led: Dict[str, Any], slice_key: str, stamp: str) -> str:
    prefix = f"F-{stamp}-{slice_key}-"
    n = sum(1 for f in led["findings"] if f["id"].startswith(prefix)) + 1
    return f"{prefix}{n:02d}"


def find_existing(led: Dict[str, Any], f: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    for e in led["findings"]:
        if e["file"] == rel_path(f["file"]) and (norm(e["title"]) == norm(f["title"])
                                       or (abs(int(e.get("line", 0)) - int(f.get("line", 0))) <= 3 and e.get("category") == f.get("category"))):
            return e
    return None


def verdict_summary(v: Dict[str, Any]) -> Dict[str, Any]:
    return {"real": v.get("real"), "confidence": v.get("confidence"), "reasoning": v.get("reasoning", "")}


def rel_path(path: str) -> str:
    """Investigators sometimes report absolute paths; the ledger keys on repo-relative ones."""
    prefix = str(ROOT) + "/"
    return path[len(prefix):] if path.startswith(prefix) else path


def cmd_ingest(args: argparse.Namespace) -> int:
    data = json.loads(Path(args.result).read_text(encoding="utf-8"))
    data = data.get("result", data)
    stamp = args.stamp or data.get("stamp") or "undated"
    led = ledger()
    added = {"open": 0, "rejected": 0, "unverified": 0}
    notes: List[str] = []
    for bucket, status in (("confirmed", "open"), ("rejected", "rejected"), ("unverified", "unverified")):
        for f in data.get(bucket, []):
            existing = find_existing(led, f)
            if existing:
                if status == "open" and existing["status"] == "fixed":
                    existing["status"] = "open"
                    existing["regressed"] = stamp
                    notes.append(f"{existing['id']} was fixed in {existing.get('fixed_in', '?')} but was found again: reopened")
                elif status == "open" and existing["status"] == "rejected":
                    notes.append(f"{existing['id']} was rejected before but a new run confirmed it. Read both verdicts and decide: {existing['title']}")
                else:
                    notes.append(f"{existing['id']} already in the ledger ({existing['status']}): {existing['title']}")
                continue
            slice_key = f.get("slice") or "misc"
            entry = {
                "id": next_id(led, slice_key, stamp),
                "slice": slice_key,
                "file": rel_path(f["file"]),
                "line": f.get("line", 0),
                "title": f["title"],
                "category": f.get("category", ""),
                "severity": f.get("severity", "medium"),
                "summary": f.get("summary", ""),
                "failure_scenario": f.get("failure_scenario", ""),
                "evidence": f.get("evidence", ""),
                "suggested_fix": f.get("suggested_fix", ""),
                "status": status,
                "found": stamp,
                "verdicts": [verdict_summary(v) for v in f.get("verdicts", [])],
            }
            if status == "rejected":
                entry["reason"] = next((v.get("reasoning", "") for v in f.get("verdicts", []) if not v.get("real")), "")
            led["findings"].append(entry)
            added[status] += 1
    save_json(LEDGER_FILE, led)
    print(f"ingested {args.result}: {added['open']} open, {added['rejected']} rejected, {added['unverified']} unverified added")
    for n in notes:
        print(f"  note: {n}")
    for key in ("failedSlices",):
        if data.get(key):
            print(f"  slices that returned nothing and need a rerun: {', '.join(data[key])}")
    return 0


def cmd_mark(args: argparse.Namespace) -> int:
    led = ledger()
    for f in led["findings"]:
        if f["id"] == args.id:
            f["status"] = args.status
            if args.commit:
                f["fixed_in"] = args.commit
            if args.reason:
                f["reason"] = args.reason
            f.pop("regressed", None) if args.status == "fixed" else None
            save_json(LEDGER_FILE, led)
            print(f"{args.id} -> {args.status}")
            return 0
    print(f"no finding {args.id}", file=sys.stderr)
    return 2


def short(f: Dict[str, Any]) -> str:
    return f"[{f['id']}] {f.get('severity', '?')}: {f['title']}\n    {f['file']}:{f.get('line', '?')}"


def cmd_report(args: argparse.Namespace) -> int:
    led = ledger()["findings"]
    by = {s: [f for f in led if f["status"] == s] for s in STATUSES}
    for s in by:
        by[s].sort(key=lambda f: (SEVERITY_ORDER.get(f.get("severity", "low"), 9), f["id"]))
    if args.output:
        save_json(Path(args.output), {"summary": {s: len(v) for s, v in by.items()}, "findings": led})
        print(f"wrote {args.output}")
    print(f"Ledger: {len(led)} findings. " + ", ".join(f"{len(v)} {s}" for s, v in by.items()))
    for s in ("open", "unverified") if args.open_only else STATUSES:
        if not by[s]:
            continue
        print(f"\n--- {s} ---")
        for f in by[s]:
            print(short(f))
            if s in ("open", "unverified"):
                print(f"    {f.get('summary', '')}")
                print(f"    Trigger: {f.get('failure_scenario', '')}")
                if f.get("suggested_fix"):
                    print(f"    Fix: {f['suggested_fix']}")
            elif s == "fixed":
                print(f"    fixed in {f.get('fixed_in', '?')}")
            elif f.get("reason"):
                print(f"    why not: {f['reason']}")
    return 1 if args.fail_on_open and by["open"] else 0


# ---------------------------------------------------------------- probes

def run_cmd(cmd: List[str]) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)


def cmd_check(_: argparse.Namespace) -> int:
    print("1. cargo check --workspace ...")
    p1 = run_cmd(["cargo", "check", "--workspace"])
    if p1.returncode != 0:
        print("cargo check failed:\n" + p1.stderr)
        return 1
    print("   passed")
    print("2. cargo clippy --workspace ...")
    p2 = run_cmd(["cargo", "clippy", "--workspace"])
    warnings = [line for line in p2.stderr.splitlines() if line.startswith("warning:") and "generated" not in line]
    print(f"   {len(warnings)} warnings")
    for w in warnings[:10]:
        print("   " + w)
    print("3. slices.json against the workflow ...")
    rc = cmd_sync(argparse.Namespace(check=True))
    return rc


def cmd_test(args: argparse.Namespace) -> int:
    cmd = ["cargo", "test"] + (["-p", "trek-agents", "-p", "trek-core"] if args.quick else ["--workspace"])
    print("Running: " + " ".join(cmd))
    p = run_cmd(cmd)
    lines = (p.stdout + p.stderr).splitlines()
    passed = failed = ignored = suites = 0
    for line in lines:
        if line.startswith("test result:"):
            parts = line.split()
            passed += int(parts[3]); failed += int(parts[5]); ignored += int(parts[7]); suites += 1
        elif line.startswith("failures:") or " FAILED" in line or "panicked at" in line:
            print(line)
    print(f"{suites} suites: {passed} passed, {failed} failed, {ignored} ignored")
    if p.returncode != 0:
        print("\n".join(lines[-30:]))
    return p.returncode


# ---------------------------------------------------------------- main

def main() -> int:
    parser = argparse.ArgumentParser(description="Trek bug finder: slices, workflow plan, ledger, probes")
    sub = parser.add_subparsers(dest="command", required=True)

    d = sub.add_parser("domains", help="list slices, or find tracked source files that no slice covers")
    d.add_argument("--uncovered", action="store_true", help="list tracked source files in no slice")
    d.add_argument("--strict", action="store_true", help="with --uncovered: exit 1 if any")
    d.add_argument("--json", action="store_true")

    s = sub.add_parser("sync", help="regenerate workflow.js's slice list from slices.json")
    s.add_argument("--check", action="store_true", help="only report drift (exit 1)")

    p = sub.add_parser("plan", help="print the args to pass to the Workflow tool for workflow.js")
    p.add_argument("--slices", help="comma-separated slice keys; default all")
    p.add_argument("--model", help="model override (e.g. fable-5.1, opus-5.5, sonnet-5.5, haiku-5.5); default sonnet-5.5")
    p.add_argument("--effort", default="high", choices=["low", "medium", "high", "xhigh", "max"], help="reasoning effort; default high")
    p.add_argument("--concurrency", type=int, default=2, help="max concurrent agents (default 2)")
    p.add_argument("--max-agents", type=int, default=20, help="max total subagents to spawn (default 20)")
    p.add_argument("--max-findings", type=int, default=8, help="max findings to send to skeptics (default 8)")
    p.add_argument("--drive-app", action="store_true", help="run the investigator that drives the isolated app")
    p.add_argument("--no-app", action="store_true", help="skip the investigator that drives the isolated app")
    p.add_argument("--stamp", help="run date, e.g. 2026-10-15")
    p.add_argument("--investigator-model", help="model for investigators (overrides --model's default for them)")
    p.add_argument("--skeptic-model", help="model for skeptics")
    p.add_argument("--tiebreak-model", help="model for tie-breakers (default opus-5.5)")
    p.add_argument("--skeptic-mode", choices=["sequential", "parallel"],
                   help="sequential (default): one trace skeptic, a reproduce skeptic only if it says real; parallel: both at once")
    p.add_argument("--delta", action="store_true", help="add one investigator over the uncommitted files in the working tree")
    p.add_argument("--carry", action="append", metavar="FILE",
                   help="carry investigator findings from an earlier run's journal.jsonl or saved result into verification; repeatable")
    p.add_argument("--script-out", metavar="PATH",
                   help="write a copy of workflow.js with the carried findings embedded (keeps the args small); use it as scriptPath")

    k = sub.add_parser("packages", help="write one fixer brief per slice for the open and unverified findings")
    k.add_argument("--out", default="/tmp/trek-bugs/packages", help="folder for pkg-<slice>.md and fixer-rules.md")

    i = sub.add_parser("ingest", help="merge a workflow result (JSON file) into the ledger")
    i.add_argument("result")
    i.add_argument("--stamp")

    m = sub.add_parser("mark", help="set a finding's status")
    m.add_argument("id")
    m.add_argument("status", choices=STATUSES)
    m.add_argument("--commit")
    m.add_argument("--reason")

    r = sub.add_parser("report", help="print the ledger")
    r.add_argument("--open-only", action="store_true", help="only open and unverified findings")
    r.add_argument("--output", "-o", help="also write the ledger summary as JSON")
    r.add_argument("--fail-on-open", action="store_true", help="exit 1 while any finding is open")

    sub.add_parser("check", help="cargo check and clippy, plus slice drift")
    t = sub.add_parser("test", help="cargo tests")
    t.add_argument("--quick", action="store_true", help="trek-agents and trek-core only")

    args = parser.parse_args()
    return {"domains": cmd_domains, "sync": cmd_sync, "plan": cmd_plan, "packages": cmd_packages, "ingest": cmd_ingest, "mark": cmd_mark,
            "report": cmd_report, "check": cmd_check, "test": cmd_test}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
