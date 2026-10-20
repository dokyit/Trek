---
name: trek-bug-finder
description: Find real bugs in Trek with fresh-context investigators per code slice and skeptics that try to disprove every finding, then track each finding to a fix in a ledger. Use for "audit Trek", "find bugs", or before a release.
metadata:
  trek: bug-finder
  cli: ./.agents/skills/trek-bug-finder/scripts/trek_bug_finder.py
  workflow: ./.agents/skills/trek-bug-finder/workflow.js
---

# Trek bug finder

One method, one folder. A single agent reading a big codebase loses detail as its context fills, trusts its own findings, and stops early. This finder splits Trek into slices, reads each slice in a fresh context, and sends every finding to skeptics whose only job is to disprove it. Only what survives goes in the ledger.

| File | Role |
|---|---|
| `slices.json` | What is covered. The only place slices are edited. |
| `workflow.js` | The agents: investigators, skeptics, tie-breaker. Its slice list is generated from `slices.json`. |
| `ledger.json` | Every finding ever made, with status `open`, `fixed`, `rejected` or `unverified`. |
| `fixer-rules.md` | What every fixer subagent must follow; `packages` copies it next to the briefs. |
| `scripts/trek_bug_finder.py` | The driver around all of it. Python 3 standard library only. |

## Run it

From the repo root. `B=./.agents/skills/trek-bug-finder/scripts/trek_bug_finder.py`

1. **Check coverage.** `$B domains --uncovered --strict` must list nothing. A tracked source file in no slice is never read. After editing `slices.json`, run `$B sync`. `$B sync --check` fails on drift.
2. **Get the run's args.** `$B plan --stamp 2026-10-15` prints JSON. It carries every ledger entry as `known`, so investigators skip bugs already fixed or rejected.
   - **Model selection:** Pass `--model <name>` (e.g. `fable-5.1`, `opus-5.5`, `sonnet-5.5`, `haiku-5.5`) on `--effort high` (default `high`). Default model is `sonnet-5.5` to safeguard session quotas.
   - **Concurrency and agent caps:** Bounded by default to prevent API rate limits. Configure with `--concurrency <N>` (default 2), `--max-agents <N>` (default 20), and `--max-findings <N>` (default 8).
   - **Scope:** Narrow slices with `--slices agents,remote` or drive the isolated app with `--drive-app`. `--delta` adds one investigator that reads every file changed or added in the working tree since HEAD (slices use `git ls-files`, so untracked files are otherwise never read).
   - **Reuse an aborted run:** `--carry <journal.jsonl>` takes the investigator reports that did finish in an earlier run (the Workflow tool's transcript dir has the journal) and sends their findings straight to skeptics, so only the slices that failed need investigating again. Add `--script-out /tmp/run.js` to embed them in a copy of the workflow, since they make the args too big to paste; then `scriptPath` is that copy.
   - **Skeptic cost:** `--skeptic-mode sequential` (the default) spends one skeptic on a finding and a second only if the first could not refute it, with a tie-breaker on a split: one to three agents per finding. `parallel` always runs two.
   - **Per-stage models:** `--investigator-model`, `--skeptic-model`, `--tiebreak-model` (default opus-5.5 for the tie-breaker, sonnet-5.5 for the rest). Keep the orchestrator on the session model and the subagents on cheaper ones; the run on 2026-10-15 spent 4.9M tokens in 18 minutes with 105 concurrent Fable agents and hit the session limit.
3. **Run the workflow.** With Claude Code's Workflow tool: `scriptPath` is `.agents/skills/trek-bug-finder/workflow.js` (or the `--script-out` copy) and `args` is the JSON from step 2. Slices run in concurrency-limited batches, candidate findings are deduplicated and ranked by severity then confidence, and the top `maxFindings` go to skeptics; the rest come back as `unverified` with a reason. If the run dies (a usage limit, a kill), relaunch with `resumeFromRunId`: finished agents return cached results.
4. **Ingest.** Save the workflow's returned object to a file and run `$B ingest result.json`. New findings are added. A finding that was fixed and is found again is reopened. Slices that returned nothing are named so you can rerun them.
5. **Fix every open finding, with a test.** `$B packages --out /tmp/trek-bugs/packages` writes one brief per slice (open findings first, then the unverified ones to assess) and copies `fixer-rules.md` beside them. Hand each package to one fixer subagent on a cheaper model, at most four at a time, all in the one checkout with targeted cargo commands; the rules file tells them how to behave around each other's half-finished edits and what to report. The orchestrator stays on the session model and only reads reports. Then `$B mark <id> fixed --commit <sha>`. A finding that turns out not to be a bug: `$B mark <id> rejected --reason "..."`. `$B report --open-only` lists what is left. `--fail-on-open` makes it exit 1 while any remain.
6. **Probes.** `$B check` runs cargo check, clippy and the drift check. `$B test --quick` runs the agents and core tests, `$B test` the whole workspace.

## Without the Workflow tool

Any agent that cannot run `workflow.js` (for example a Devin thread) follows the same method by hand: read `workflow.js` for the investigator and skeptic prompts, do each slice from `$B domains` as a separate pass with a fresh context, write the findings to `/tmp/trek-bugs/<slice>.json` before starting the next slice, then try to refute each finding in a separate pass (open the lines, trace callers and guards, look for a test that asserts the behaviour, try a small reproduction; default to rejecting when unsure). Shape the final object like the workflow's return value, with `confirmed`, `rejected`, `unverified`, `failedSlices` and `stamp`, so `$B ingest` accepts it.

## What counts as a bug

Something a user can hit: a panic or crash, lost or corrupted data, wrong behaviour against a clear intent, a race, a leak, a security hole (path traversal, command injection, CSP bypass, secrets in logs), a wrong platform assumption, a dead UI path. Style and "could be simpler" are not bugs. A behaviour a passing test asserts is intended.

## Finding shape

Investigators return, and the ledger stores: `file`, `line`, `title`, `category` (crash, data-loss, wrong-behaviour, race, security, resource-leak, platform, ui-logic), `severity` (critical, high, medium, low), `summary`, `failure_scenario` (concrete inputs, then the wrong result), `evidence` (quoted lines with paths), `suggested_fix`, `confidence`. Skeptics return `real`, `confidence`, `reasoning`, `evidence`, `severity`, and a corrected scenario if the reporter got details wrong.

## Rules for agents in the run

- Only the app driver starts the isolated app. One per checkout.
- Nobody edits the repo during the investigate and verify phases. Fixes come after, one finding at a time.
- cargo is slow and shared. A skeptic runs one targeted test only when its verdict depends on it.
- Never point automation at a normal Trek process. Imported threads can resume real sessions.
