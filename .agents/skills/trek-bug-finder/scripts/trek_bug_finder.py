#!/usr/bin/env python3
"""
Trek Bug Finder Workflow Orchestrator
Implements Anthropic's Dynamic Workflow methodology:
- Domain partitioning with fresh context boundaries
- Structured findings exchange
- Adversarial skeptic verification
- Actionable synthesis and reporting
"""

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

WORKSPACE_ROOT = Path(__file__).resolve().parents[4]

DOMAINS = {
    "DOMAIN_CORE": {
        "name": "Core State, Database & Skills",
        "files": [
            "crates/trek-core/src/skills.rs",
            "crates/trek-core/src/settings.rs",
            "crates/trek-core/src/store.rs",
            "crates/trek-core/src/paths.rs",
            "crates/trek-core/src/catalog.rs",
        ],
        "focus": [
            "Skill origin & source classification",
            "SQLite migrations & transaction boundaries",
            "Keychain & secrets fallback",
            "Path traversal and filesystem safety",
        ],
    },
    "DOMAIN_AGENTS": {
        "name": "Agents, LLM Providers, Limits & ACP",
        "files": [
            "crates/trek-agents/src/direct.rs",
            "crates/trek-agents/src/limits.rs",
            "crates/trek-agents/src/acp.rs",
            "crates/trek-agents/src/lib.rs",
        ],
        "focus": [
            "Dynamic model parsing & cache consistency",
            "Streaming SSE chunking & error termination",
            "Rate limit reset date/time calculation",
            "ACP process spawning and stdin/stdout pipes",
        ],
    },
    "DOMAIN_WORKSPACE": {
        "name": "Workspace State & Thread Lifecycle",
        "files": [
            "crates/trek-app/src/workspace.rs",
            "crates/trek-app/src/thread_view.rs",
        ],
        "focus": [
            "Async runtime background tasks and GPUI updates",
            "Race conditions during rapid thread switching",
            "Model fetching concurrency and inflight state",
            "Process cleanup on window close",
        ],
    },
    "DOMAIN_COMPOSER": {
        "name": "Composer, Pickers & UI Interaction",
        "files": [
            "crates/trek-app/src/composer/pickers.rs",
            "crates/trek-app/src/composer.rs",
            "crates/trek-app/src/settings_view/pages.rs",
        ],
        "focus": [
            "Picker index bounds & keyboard selection wrapping",
            "Empty list states & loading indicators",
            "Custom model query row edge cases",
            "Key event capturing and paste handling",
        ],
    },
    "DOMAIN_PROTOCOLS": {
        "name": "Protocols, MCP & Remote IPC",
        "files": [
            "crates/trek-mcp/src/main.rs",
            "crates/trek-ipc/src/lib.rs",
            "crates/trek-remote/src/lib.rs",
        ],
        "focus": [
            "JSON-RPC serialization and error responses",
            "Stdio communication hung processes",
            "Socket reconnection and authentication",
        ],
    },
}

@dataclass
class CandidateFinding:
    id: str
    domain: str
    file: str
    line_range: List[int]
    title: str
    mechanism: str
    trigger_condition: str
    severity: str
    suggested_fix: str

@dataclass
class SkepticVerdict:
    finding_id: str
    verdict: str  # CONFIRMED_BUG | DISPROVED | LIMITATION_BY_DESIGN
    disproof_attempt: str
    evidence: str
    reproduction_status: str
    notes: str

# Master ledger populated through parallel dynamic investigation & skeptic passes
CANDIDATE_FINDINGS: List[CandidateFinding] = [
    CandidateFinding(
        id="FIND-AGENT-001",
        domain="DOMAIN_AGENTS",
        file="crates/trek-agents/src/direct.rs",
        line_range=[373, 401],
        title="Anthropic Thinking Signature Dropped Breaking Multi-Turn",
        mechanism=(
            "When Anthropic outputs thinking blocks, the initial block contains {'type':'thinking','thinking':''}. "
            "When signature_delta arrives, append(&mut blocks, i, 'signature', ...) is called. "
            "append requires the field to already exist (b.get_mut(field)), so it silently drops the signature. "
            "The block in history lacks 'signature', causing Anthropic to reject turn 2 with HTTP 400."
        ),
        trigger_condition="Any multi-turn conversation with an Anthropic direct model that uses extended thinking.",
        severity="Critical",
        suggested_fix="In append, if the field is not present in the JSON object, insert it as Value::String(s.to_string())."
    ),
    CandidateFinding(
        id="FIND-AGENT-002",
        domain="DOMAIN_AGENTS",
        file="crates/trek-agents/src/direct.rs",
        line_range=[428, 436],
        title="Direct Provider Models Query Incompatible with Anthropic",
        mechanism=(
            "direct_provider_models unconditionally queries {base_url}/models with bearer_auth. "
            "Anthropic's endpoint is /v1/models and requires x-api-key and anthropic-version headers. "
            "As a result, dynamic model discovery fails for direct Anthropic setups."
        ),
        trigger_condition="Trek attempts to dynamically discover models for an Anthropic direct provider key.",
        severity="High",
        suggested_fix="Check provider.wire; if Wire::Anthropic, query /v1/models with x-api-key and anthropic-version."
    ),
    CandidateFinding(
        id="FIND-AGENT-003",
        domain="DOMAIN_AGENTS",
        file="crates/trek-agents/src/limits.rs",
        line_range=[153, 187],
        title="Parentheses in Body After Time Invalidate Rate Limit Reset",
        mechanism=(
            "s.find('(') searches across the entire remaining string before splitting into sentences. "
            "Any markdown link or parenthetical remark (e.g. '(https://docs.anthropic.com)') gets extracted as zone. "
            "offset() fails on arbitrary non-timezone text, and the '?' operator causes parse_when to return None, "
            "discarding an otherwise completely valid clock reset time."
        ),
        trigger_condition="API rate limit error includes parenthetical text or markdown links after the reset time.",
        severity="High",
        suggested_fix="Extract parentheses only within the first sentence clause, and ignore offset() failures by falling back to local/default timezone instead of returning None."
    ),
    CandidateFinding(
        id="FIND-AGENT-004",
        domain="DOMAIN_AGENTS",
        file="crates/trek-agents/src/limits.rs",
        line_range=[104, 114],
        title="'retry after ' In Incorrect Match Group in reset_in",
        mechanism=(
            "'retry after ' is placed in AT (which expects calendar dates / clock times via parse_when), "
            "rather than IN (which parses duration spans like '60 seconds' via parse_span). "
            "Standard HTTP Retry-After plain text errors fail to parse."
        ),
        trigger_condition="An API error states 'Rate limit reached. Retry after 60s.'",
        severity="Medium",
        suggested_fix="Move 'retry after ' to IN or attempt parse_span when parse_when returns None."
    ),
    CandidateFinding(
        id="FIND-CORE-001",
        domain="DOMAIN_CORE",
        file="crates/trek-core/src/skills.rs",
        line_range=[212, 225],
        title="source_for Misclassifies Disabled Skills and Defaults to Project",
        mechanism=(
            "When restoring disabled skills from disabled_root, source_for checks prefixes. "
            "Synced skills match .claude/skills and become ClaudeCode; unknown/synced/read-only origins "
            "fall into the final else and are marked as SkillSource::Project (which claims editable() == true)."
        ),
        trigger_condition="A user disables a synced or non-project skill, then restarts Trek.",
        severity="Medium",
        suggested_fix="Properly distinguish Synced skills and avoid defaulting to editable Project if origin is outside project root."
    ),
    CandidateFinding(
        id="FIND-CORE-002",
        domain="DOMAIN_CORE",
        file="crates/trek-core/src/skills.rs",
        line_range=[258, 261],
        title="ORIGIN_FILE Removed Before move_dir in set_enabled(true)",
        mechanism=(
            "set_enabled removes .trek-origin before executing move_dir(&skill.dir, &origin). "
            "If move_dir fails (cross-filesystem, permission, or disk full), .trek-origin is permanently lost. "
            "The skill is stranded in disabled_skills forever and can never be discovered or enabled."
        ),
        trigger_condition="A filesystem error or cross-device failure occurs while re-enabling a skill.",
        severity="High",
        suggested_fix="Delete ORIGIN_FILE only after move_dir has successfully returned Ok(())."
    ),
    CandidateFinding(
        id="FIND-CORE-003",
        domain="DOMAIN_CORE",
        file="crates/trek-core/src/skills.rs",
        line_range=[144, 148],
        title="read_front_matter Scans Frontmatter Lines When description is Missing",
        mechanism=(
            "When frontmatter exists but omits 'description:', the fallback scans text.lines() from line 0. "
            "It encounters the frontmatter lines (e.g. 'name: my-tool') and treats it as the description prose."
        ),
        trigger_condition="SKILL.md has YAML frontmatter without an explicit description field.",
        severity="Low",
        suggested_fix="Scan for prose only after the closing '---' frontmatter delimiter."
    ),
    CandidateFinding(
        id="FIND-CORE-004",
        domain="DOMAIN_CORE",
        file="crates/trek-core/src/settings.rs",
        line_range=[925, 930],
        title="None == None Causes Salvaged Settings Resurrection",
        mechanism=(
            "Investigator claimed that unknown or removed keys in self.guard.kept evaluate to None == None "
            "and are resurrected on save."
        ),
        trigger_condition="User removes unrecognized setting from TOML.",
        severity="High",
        suggested_fix="Ensure removed keys are purged."
    ),
    CandidateFinding(
        id="FIND-APP-001",
        domain="DOMAIN_WORKSPACE",
        file="crates/trek-app/src/settings_view/pages.rs",
        line_range=[280, 310],
        title="Environment Variable API Keys Never Trigger Dynamic Model Fetch",
        mechanism=(
            "Investigator claimed fetch_direct_models is only run for Keychain/settings keys, "
            "and shell env vars never trigger background model fetching on startup."
        ),
        trigger_condition="User sets OPENAI_API_KEY in shell without saving to Keychain.",
        severity="Medium",
        suggested_fix="Run fetch_direct_models on startup for env var keys."
    ),
]

SKEPTIC_VERDICTS: List[SkepticVerdict] = [
    SkepticVerdict(
        finding_id="FIND-AGENT-001",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if append inserts missing keys or if signature is handled elsewhere in direct.rs.",
        evidence="direct.rs:402: append requires blocks.get_mut(index).and_then(|b| b.get_mut(field)). If field is missing, get_mut returns None. Signature is never recorded in history.",
        reproduction_status="CONFIRMED_BY_UNIT_TEST",
        notes="Critical bug in extended thinking streaming. Breaks multi-turn Anthropic requests."
    ),
    SkepticVerdict(
        finding_id="FIND-AGENT-002",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if catalog.rs or direct.rs routes Anthropic to /v1/models.",
        evidence="direct.rs:428: Always does client.get(format('{}/models', provider.base_url)).bearer_auth(k). Fails 100% on Anthropic.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="High severity. Blocks dynamic model discovery for Anthropic direct wire."
    ),
    SkepticVerdict(
        finding_id="FIND-AGENT-003",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if parse_when catches ? on offset() or limits search to sentence.",
        evidence="limits.rs:185 finds '(' across the entire string. Line 248 has offset(zone, guess)?. Any invalid zone drops the parsed clock time.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="High severity. Rate limits with links or parens fail to register reset time."
    ),
    SkepticVerdict(
        finding_id="FIND-AGENT-004",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if parse_when handles duration spans or if AT falls through to IN.",
        evidence="AT does not fall through to IN. parse_when fails on duration strings like '60 seconds'.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="Medium severity. Standard 'Retry after X' headers fail parsing."
    ),
    SkepticVerdict(
        finding_id="FIND-CORE-001",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if discover() overrides source_for or if Synced is handled elsewhere.",
        evidence="source_for in skills.rs:212 matches .claude/skills first, turning Synced into ClaudeCode. Final fallback is Project unconditionally.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="Medium severity. Inaccurate source labels and editable permissions."
    ),
    SkepticVerdict(
        finding_id="FIND-CORE-002",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if ORIGIN_FILE is recreated on move_dir error.",
        evidence="Line 260 calls remove_file before move_dir. No catch or error recovery exists.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="High severity. Skill permanently lost on any move_dir failure."
    ),
    SkepticVerdict(
        finding_id="FIND-CORE-003",
        verdict="CONFIRMED_BUG",
        disproof_attempt="Checked if lines.skip(closing_dashes) is implemented.",
        evidence="Line 146 scans text.lines() from the beginning of the file, matching first non-empty line in frontmatter.",
        reproduction_status="CONFIRMED_BY_CODE_TRACE",
        notes="Low severity. Metadata displayed as description."
    ),
    SkepticVerdict(
        finding_id="FIND-CORE-004",
        verdict="DISPROVED",
        disproof_attempt="Checked why None == None was implemented in SaveGuard::to_toml.",
        evidence="Comments on line 915 explain: values load couldn't read go back in unless the setting was changed. This preserves unknown/future TOML fields. If user manually edited the file, load() re-runs.",
        reproduction_status="PROVEN_INTENTIONAL_BY_DESIGN",
        notes="Refuted by skeptic. Safeguard against deleting unrecognized fields."
    ),
    SkepticVerdict(
        finding_id="FIND-APP-001",
        verdict="DISPROVED",
        disproof_attempt="Checked Workspace::detect_agents in workspace.rs line 5617.",
        evidence="workspace.rs:5617 explicitly loops over DIRECT_PROVIDERS and checks secrets::api_key(p.id). secrets::api_key checks std::env::var first! So env vars DO trigger fetch_direct_models on startup.",
        reproduction_status="PROVEN_FALSE_BY_CODE_PATH",
        notes="Refuted by skeptic. Startup detector properly checks env vars."
    ),
]

def run_cmd(cmd: List[str], cwd: Path = WORKSPACE_ROOT) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)

def cmd_domains(args):
    print("=== Trek Bug Finder: Partitioned Domains ===")
    for domain_id, meta in DOMAINS.items():
        print(f"\n[{domain_id}] - {meta['name']}")
        print("  Focus Areas:")
        for f in meta['focus']:
            print(f"    • {f}")
        print("  Target Files:")
        for path in meta['files']:
            exists = (WORKSPACE_ROOT / path).exists()
            status = "✓" if exists else "✗ (missing)"
            print(f"    - {path} [{status}]")

def cmd_check(args):
    print("=== Running Static Analysis & Compiler Checks ===")
    print("1. cargo check --workspace...")
    p1 = run_cmd(["cargo", "check", "--workspace"])
    if p1.returncode != 0:
        print("❌ cargo check failed:\n", p1.stderr)
        return 1
    print("✓ cargo check passed cleanly.")

    print("\n2. cargo clippy (gathering warnings)...")
    p2 = run_cmd(["cargo", "clippy", "--workspace"])
    warnings = [line for line in p2.stderr.splitlines() if "warning:" in line]
    print(f"✓ cargo clippy completed. {len(warnings)} warning lines found.")
    for w in warnings[:10]:
        print("  ", w)
    if len(warnings) > 10:
        print(f"   ... and {len(warnings) - 10} more.")
    return 0

def cmd_test(args):
    print("=== Running Test Probes ===")
    cmd = ["cargo", "test"]
    if args.quick:
        cmd.extend(["-p", "trek-agents", "-p", "trek-core"])
    else:
        cmd.extend(["--workspace"])
    print(f"Running: {' '.join(cmd)}")
    p = run_cmd(cmd)
    if p.returncode != 0:
        print("❌ Tests failed:\n", p.stderr)
        return 1
    print("✓ All targeted tests passed successfully.")
    return 0

def cmd_report(args):
    print("=== Trek Bug Finder: Dynamic Workflow Report ===")
    total = len(CANDIDATE_FINDINGS)
    verdict_map = {v.finding_id: v for v in SKEPTIC_VERDICTS}
    confirmed = [f for f in CANDIDATE_FINDINGS if verdict_map.get(f.id) and verdict_map[f.id].verdict == "CONFIRMED_BUG"]
    disproved = [f for f in CANDIDATE_FINDINGS if verdict_map.get(f.id) and verdict_map[f.id].verdict == "DISPROVED"]

    print(f"\nTotal Candidates Investigated: {total}")
    print(f"Confirmed by Skeptics: {len(confirmed)} ({len(confirmed)/total*100:.1f}% survival rate)")
    print(f"Disproved by Skeptics: {len(disproved)} (prevented false alarms)")

    print("\n--- Verified Bugs (Surviving Skeptic Adversarial Check) ---")
    for f in confirmed:
        v = verdict_map[f.id]
        print(f"\n[{f.id}] [{f.severity}] {f.title}")
        print(f"  File: {f.file}:{f.line_range[0]}-{f.line_range[1]}")
        print(f"  Mechanism: {f.mechanism}")
        print(f"  Skeptic Verification: {v.evidence}")
        print(f"  Remediation: {f.suggested_fix}")

    print("\n--- Disproved Hypotheses (Filtered Out by Skeptics) ---")
    for f in disproved:
        v = verdict_map[f.id]
        print(f"\n[{f.id}] {f.title}")
        print(f"  Reason Disproved: {v.evidence}")

    if args.output:
        data = {
            "summary": {
                "total_investigated": total,
                "confirmed_count": len(confirmed),
                "disproved_count": len(disproved),
                "survival_rate": len(confirmed) / total if total > 0 else 0,
            },
            "confirmed_bugs": [
                {
                    "finding": asdict(f),
                    "verdict": asdict(verdict_map[f.id]),
                }
                for f in confirmed
            ],
            "disproved_hypotheses": [
                {
                    "finding": asdict(f),
                    "verdict": asdict(verdict_map[f.id]),
                }
                for f in disproved
            ]
        }
        with open(args.output, "w", encoding="utf-8") as out:
            json.dump(data, out, indent=2)
        print(f"\nSaved structured JSON report to: {args.output}")

def main():
    parser = argparse.ArgumentParser(description="Trek Bug Finder Workflow Orchestrator")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("domains", help="List partitioned domains and target manifests")
    subparsers.add_parser("check", help="Run compiler and linter checks")
    
    test_parser = subparsers.add_parser("test", help="Run verification tests")
    test_parser.add_argument("--quick", action="store_true", help="Run fast core package test subset")

    report_parser = subparsers.add_parser("report", help="Generate workflow ledger and report")
    report_parser.add_argument("--output", "-o", help="Path to save JSON report")

    args = parser.parse_args()
    if args.command == "domains":
        cmd_domains(args)
    elif args.command == "check":
        sys.exit(cmd_check(args))
    elif args.command == "test":
        sys.exit(cmd_test(args))
    elif args.command == "report":
        cmd_report(args)

if __name__ == "__main__":
    main()
