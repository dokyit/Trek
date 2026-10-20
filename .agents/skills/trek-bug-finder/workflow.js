export const meta = {
  name: 'trek-bug-finder',
  description: 'Find bugs in Trek: fresh-context investigators per code slice, then skeptics that try to disprove each finding',
  whenToUse: 'Audit Trek for real defects. Get the args from: .agents/skills/trek-bug-finder/scripts/trek_bug_finder.py plan. Optional args: model (fable-5.1|opus-5.5|sonnet-5.5|haiku-5.5), effort (high|medium|low), concurrency (default 2), maxAgents (default 20), maxFindings (default 8), slices (keys), driveApp (true/false), known (ledger entries), stamp (run date), deltaFiles (uncommitted files for one extra investigator), extraFindings (candidates from an earlier run to verify), skepticMode (sequential|parallel).',
  phases: [
    { title: 'Investigate', detail: 'concurrency-limited slice investigators reading code in fresh contexts' },
    { title: 'Verify', detail: 'two skeptics per top candidate finding; tie-breaker settles splits' },
  ],
}

// Slices live in slices.json (edit there, then: trek_bug_finder.py sync). Each is a set of git pathspecs.
// Tests under crates/trek-app/src/tests are reference material for intended behaviour, not a slice.
// BEGIN GENERATED SLICES (from slices.json; regenerate with: trek_bug_finder.py sync)
const SLICES = [
  {
    "key": "core-store",
    "what": "persistence: SQLite store, search, usage, checkpoints, rewind, lineage, transcripts, core types",
    "paths": [
      "crates/trek-core/src/store.rs",
      "crates/trek-core/src/store/",
      "crates/trek-core/src/checkpoint.rs",
      "crates/trek-core/src/rewind.rs",
      "crates/trek-core/src/lineage.rs",
      "crates/trek-core/src/transcript.rs",
      "crates/trek-core/src/types.rs",
      "crates/trek-core/src/lib.rs"
    ],
    "focus": [
      "SQLite migrations and transaction boundaries",
      "Search and usage queries on odd input",
      "Checkpoint, rewind and lineage consistency after a crash or partial write"
    ]
  },
  {
    "key": "core-config",
    "what": "settings, agent registry and catalog, paths, detection of installed tools, skills, MCP config, open-in, winget, elevation, process handling, limits, git and worktrees",
    "paths": [
      "crates/trek-core/src/settings.rs",
      "crates/trek-core/src/registry.rs",
      "crates/trek-core/src/catalog.rs",
      "crates/trek-core/src/paths.rs",
      "crates/trek-core/src/detect.rs",
      "crates/trek-core/src/skills.rs",
      "crates/trek-core/src/mcp.rs",
      "crates/trek-core/src/open_in.rs",
      "crates/trek-core/src/winget.rs",
      "crates/trek-core/src/elevated.rs",
      "crates/trek-core/src/procs.rs",
      "crates/trek-core/src/limit.rs",
      "crates/trek-core/src/git.rs",
      "crates/trek-core/src/worktree.rs"
    ],
    "focus": [
      "Skill origin and source classification, disable and enable round trips",
      "Settings salvage: unknown keys survive a save, removed keys do not come back",
      "Keychain and secrets fallback, environment variable keys",
      "Path traversal and filesystem safety, cross-device moves"
    ]
  },
  {
    "key": "core-updates",
    "what": "agent update parsing, Trek self-update and relocation, changelog, feeds, pricing, verification; the trek-update helper binary",
    "paths": [
      "crates/trek-core/src/agent_update.rs",
      "crates/trek-core/src/update.rs",
      "crates/trek-core/src/update/",
      "crates/trek-core/src/changelog.rs",
      "crates/trek-core/src/feeds.rs",
      "crates/trek-core/src/pricing.rs",
      "crates/trek-core/src/verification.rs",
      "crates/trek-update/"
    ],
    "focus": []
  },
  {
    "key": "core-content",
    "what": "imports from Claude Code, Codex and OpenCode history; Basecamp; notes; artifacts and their CSP; visualizations; inline edits; orchestration; hunks and changes; restating",
    "paths": [
      "crates/trek-core/src/import/",
      "crates/trek-core/src/basecamp.rs",
      "crates/trek-core/src/notes.rs",
      "crates/trek-core/src/artifact.rs",
      "crates/trek-core/src/visualization.rs",
      "crates/trek-core/src/inline_edit.rs",
      "crates/trek-core/src/orchestrate.rs",
      "crates/trek-core/src/hunks.rs",
      "crates/trek-core/src/changes.rs",
      "crates/trek-core/src/restate.rs"
    ],
    "focus": []
  },
  {
    "key": "agents",
    "what": "the agent adapters: Claude Code, Codex, OpenCode, ACP, direct API, mock; limits, status, titles, stop handling",
    "paths": [
      "crates/trek-agents/"
    ],
    "focus": [
      "Streaming SSE chunking and error termination",
      "Anthropic thinking blocks and signatures across turns",
      "Dynamic model discovery per provider wire (headers, endpoints, cache)",
      "Rate-limit reset parsing: dates, zones, durations, parentheses",
      "ACP and CLI process spawning, stdin and stdout pipes, stop handling"
    ]
  },
  {
    "key": "remote",
    "what": "the phone link: pairing, TLS, host, server, protocol, the Mac side of the link and push notifications",
    "paths": [
      "crates/trek-remote/",
      "crates/trek-app/src/remote.rs",
      "crates/trek-app/src/remote/",
      "crates/trek-app/src/push.rs"
    ],
    "focus": [
      "Socket reconnection and authentication",
      "Pairing and TLS edge cases",
      "Address advertising on odd networks (IPv6-only, link-local, VPN)"
    ]
  },
  {
    "key": "ios",
    "what": "the iPhone companion app (Swift)",
    "paths": [
      "ios/Trek/*.swift",
      "ios/scripts/"
    ],
    "focus": []
  },
  {
    "key": "ipc-mcp",
    "what": "trek-mcp (computer use, simulator, orchestrate, desktop, keys), trek-ipc, the app IPC, deep links, single instance, the editor extension",
    "paths": [
      "crates/trek-mcp/",
      "crates/trek-ipc/",
      "crates/trek-app/src/ipc.rs",
      "crates/trek-app/src/deep_link.rs",
      "crates/trek-app/src/single_instance.rs",
      "extension/extension.js",
      "crates/trek-app/simhid/"
    ],
    "focus": [
      "JSON-RPC serialisation and error responses",
      "Stdio communication and hung child processes"
    ]
  },
  {
    "key": "workspace-a",
    "what": "the workspace model and root window: thread lifecycle, turns, projects, windows, tabs",
    "paths": [
      "crates/trek-app/src/workspace.rs",
      "crates/trek-app/src/root.rs",
      "crates/trek-app/src/thread_window.rs",
      "crates/trek-app/src/tabs.rs"
    ],
    "focus": [
      "Async runtime tasks and GPUI updates",
      "Race conditions during rapid thread switching",
      "Model fetching concurrency and in-flight state",
      "Process cleanup on window close"
    ]
  },
  {
    "key": "workspace-b",
    "what": "workspace submodules",
    "paths": [
      "crates/trek-app/src/workspace/"
    ],
    "focus": []
  },
  {
    "key": "thread-ui",
    "what": "thread view, composer, mentions, attachments, agent updates, working bar, trail words, activity, changes card, jobs, cost",
    "paths": [
      "crates/trek-app/src/thread_view.rs",
      "crates/trek-app/src/composer.rs",
      "crates/trek-app/src/composer/",
      "crates/trek-app/src/mentions.rs",
      "crates/trek-app/src/attachments.rs",
      "crates/trek-app/src/agent_updates.rs",
      "crates/trek-app/src/working_bar.rs",
      "crates/trek-app/src/words.rs",
      "crates/trek-app/src/activity.rs",
      "crates/trek-app/src/changes_card.rs",
      "crates/trek-app/src/job.rs",
      "crates/trek-app/src/cost.rs"
    ],
    "focus": [
      "Picker index bounds and keyboard wrapping",
      "Empty list and loading states, custom model query row",
      "Key event capture and paste handling"
    ]
  },
  {
    "key": "ide",
    "what": "the IDE: explorer, terminal, preview, browser, editor, LSP client, languages, worktree UI",
    "paths": [
      "crates/trek-app/src/ide/",
      "crates/trek-app/src/editor.rs",
      "crates/trek-app/src/lsp_client.rs",
      "crates/trek-app/src/languages.rs",
      "crates/trek-app/src/worktree_ui.rs",
      "crates/trek-app/src/terminal_dock.rs"
    ],
    "focus": []
  },
  {
    "key": "panels",
    "what": "panels, sidebar, command palette, keys and shortcuts, Basecamp view",
    "paths": [
      "crates/trek-app/src/panels/",
      "crates/trek-app/src/sidebar.rs",
      "crates/trek-app/src/command_palette.rs",
      "crates/trek-app/src/keys.rs",
      "crates/trek-app/src/shortcuts.rs",
      "crates/trek-app/src/basecamp.rs"
    ],
    "focus": []
  },
  {
    "key": "rendering",
    "what": "math, visualizations, markdown, artifact web views, note images and notes UI, image preview, formatting and time",
    "paths": [
      "crates/trek-app/src/math/",
      "crates/trek-app/src/visualization/",
      "crates/trek-app/src/md.rs",
      "crates/trek-app/src/artifact/",
      "crates/trek-app/src/note_images.rs",
      "crates/trek-app/src/notes.rs",
      "crates/trek-app/src/image_preview.rs",
      "crates/trek-app/src/format.rs",
      "crates/trek-app/src/time.rs"
    ],
    "focus": []
  },
  {
    "key": "settings-system",
    "what": "settings UI, onboarding, add agent, integrations, updater, tray, toasts, dictation, screen clips, system and window code, main, logging, perf, shots",
    "paths": [
      "crates/trek-app/src/settings_view.rs",
      "crates/trek-app/src/settings_view/",
      "crates/trek-app/src/onboarding.rs",
      "crates/trek-app/src/add_agent.rs",
      "crates/trek-app/src/integrations.rs",
      "crates/trek-app/src/updater.rs",
      "crates/trek-app/src/tray.rs",
      "crates/trek-app/src/toast.rs",
      "crates/trek-app/src/dictate.rs",
      "crates/trek-app/src/screenclip.rs",
      "crates/trek-app/src/system.rs",
      "crates/trek-app/src/winsys.rs",
      "crates/trek-app/src/winlook.rs",
      "crates/trek-app/src/window_place.rs",
      "crates/trek-app/src/main.rs",
      "crates/trek-app/src/logging.rs",
      "crates/trek-app/src/perf.rs",
      "crates/trek-app/src/shots.rs"
    ],
    "focus": [
      "Pickers and pages reading state that fetches model lists",
      "Updater swap and relaunch"
    ]
  },
  {
    "key": "app-chrome",
    "what": "look and feel: brand, palette, fonts, motion, chrome, mascot, file icons, the shared UI helpers, embedded assets, the background strip",
    "paths": [
      "crates/trek-app/src/assets.rs",
      "crates/trek-app/src/background_strip.rs",
      "crates/trek-app/src/brand.rs",
      "crates/trek-app/src/chrome.rs",
      "crates/trek-app/src/file_icon.rs",
      "crates/trek-app/src/fonts.rs",
      "crates/trek-app/src/mascot.rs",
      "crates/trek-app/src/motion.rs",
      "crates/trek-app/src/palette.rs",
      "crates/trek-app/src/ui.rs"
    ],
    "focus": [
      "Embedded asset lookups that can miss or panic",
      "Animation state that never ends or restarts on every render",
      "Theme colour maths on edge values"
    ]
  },
  {
    "key": "scripts",
    "what": "build, bundle, release, install and verification shell scripts; build.rs; Cargo manifests; the benchmark and seed examples in trek-core",
    "paths": [
      "script/",
      ".agents/skills/verify-trek/scripts/",
      "ios/scripts/",
      "crates/trek-app/build.rs",
      "crates/trek-app/Cargo.toml",
      "Cargo.toml",
      "crates/trek-core/examples/"
    ],
    "focus": []
  }
]
// END GENERATED SLICES

// Supported models and canonical IDs:
// Fable 5.1: claude-fable-5-1
// Opus 5.5:  claude-opus-5-5
// Sonnet 5.5: claude-sonnet-5-5
// Haiku 5.5: claude-haiku-5-5
const CANONICAL_MODELS = {
  'fable': 'claude-fable-5-1',
  'fable-5.1': 'claude-fable-5-1',
  'fable 5.1': 'claude-fable-5-1',
  'claude-fable-5-1': 'claude-fable-5-1',
  'opus': 'claude-opus-5-5',
  'opus-5.5': 'claude-opus-5-5',
  'opus 5.5': 'claude-opus-5-5',
  'claude-opus-5-5': 'claude-opus-5-5',
  'sonnet': 'claude-sonnet-5-5',
  'sonnet-5.5': 'claude-sonnet-5-5',
  'sonnet 5.5': 'claude-sonnet-5-5',
  'claude-sonnet-5-5': 'claude-sonnet-5-5',
  'haiku': 'claude-haiku-5-5',
  'haiku-5.5': 'claude-haiku-5-5',
  'haiku 5.5': 'claude-haiku-5-5',
  'claude-haiku-5-5': 'claude-haiku-5-5',
}

function resolveModel(name, fallback) {
  if (!name) return fallback
  const normalized = String(name).trim().toLowerCase()
  return CANONICAL_MODELS[normalized] || name
}

// Model selection: global model override or per-stage models, with safe fallback to Sonnet 5.5
const globalModel = args && args.model ? resolveModel(args.model, 'claude-sonnet-5-5') : null
const investigatorModel = globalModel || resolveModel(args && args.investigatorModel, 'claude-sonnet-5-5')
const skepticModel = globalModel || resolveModel(args && args.skepticModel, 'claude-sonnet-5-5')
const tiebreakModel = globalModel || resolveModel(args && args.tiebreakModel, 'claude-opus-5-5')
const effortLevel = (args && args.effort) || 'high'

// Concurrency and agent caps to prevent session rate limits and token storms:
const concurrency = Math.max(1, Math.min(Number(args && args.concurrency) || 2, 4))
const maxAgents = Math.max(1, Number(args && args.maxAgents) || 20)
const maxFindings = Math.max(1, Number(args && args.maxFindings) || 8)

let totalAgentsSpawned = 0

async function spawnAgent(prompt, opts) {
  if (totalAgentsSpawned >= maxAgents) {
    log(`[Agent Cap] Reached max agents limit (${maxAgents}). Skipping subagent: ${opts.label || 'unlabeled'}`)
    return null
  }
  totalAgentsSpawned++
  log(`[Agent ${totalAgentsSpawned}/${maxAgents}] Launching ${opts.label} [model: ${opts.model}, effort: ${opts.effort || 'high'}]`)
  try {
    return await agent(prompt, opts)
  } catch (err) {
    log(`[Agent Error] ${opts.label} failed: ${err && err.message ? err.message : String(err)}`)
    return null
  }
}

// Run a list of async thunks in sequential batches of size `batchSize` using native parallel()
async function runBatched(thunks, batchSize) {
  const allResults = []
  for (let i = 0; i < thunks.length; i += batchSize) {
    if (totalAgentsSpawned >= maxAgents) {
      log(`[Agent Cap] Reached max agents limit (${maxAgents}). Skipping remaining ${thunks.length - i} tasks.`)
      break
    }
    const chunk = thunks.slice(i, i + batchSize)
    const chunkResults = await parallel(chunk)
    allResults.push(...chunkResults)
  }
  return allResults
}

const FINDINGS = {
  type: 'object',
  properties: {
    coverage: { type: 'string', description: 'What you read fully, what you skimmed, what you did not reach' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string', description: 'repo-relative path' },
          line: { type: 'integer' },
          title: { type: 'string', description: 'one line, the claim alone' },
          category: { type: 'string', description: 'crash | data-loss | wrong-behaviour | race | security | resource-leak | platform | ui-logic' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'], description: 'critical: data loss or a crash on a common path' },
          summary: { type: 'string', description: 'one or two sentences: what is wrong' },
          failure_scenario: { type: 'string', description: 'concrete inputs or state, then the wrong output, crash or loss' },
          evidence: { type: 'string', description: 'the code lines that show it, quoted, with paths and line numbers' },
          suggested_fix: { type: 'string' },
          confidence: { type: 'number', description: '0 to 1' },
        },
        required: ['file', 'line', 'title', 'category', 'severity', 'summary', 'failure_scenario', 'evidence', 'confidence'],
      },
    },
  },
  required: ['coverage', 'findings'],
}

const VERDICT = {
  type: 'object',
  properties: {
    real: { type: 'boolean', description: 'true only if you could not refute it and the failure scenario holds' },
    confidence: { type: 'number' },
    reasoning: { type: 'string', description: 'what you checked, in order, and what settled it' },
    evidence: { type: 'string', description: 'code or test output that settles it' },
    corrected_scenario: { type: 'string', description: 'if real but the reporter got details wrong, the right scenario; else empty' },
    severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
  },
  required: ['real', 'confidence', 'reasoning', 'evidence', 'severity'],
}

const RULES = `
Ground rules for every agent in this run:
- Repo: /Users/tobias/Developer/Trek (Rust, gpui, macOS app with an iPhone companion). Read code with Bash: cat, sed -n, rg, git ls-files.
- Judge the working tree as it is on disk (it may hold uncommitted changes and untracked files), not HEAD. Line numbers in a report may have drifted; find the code by its quoted evidence with rg before concluding it is gone.
- Do NOT start the app or run the verify-trek driver (./.agents/skills/verify-trek/scripts/trek-dev) unless your brief says you are the app driver. Only one isolated app can run per checkout.
- cargo is shared and slow here (a trek-app test build takes minutes and other agents queue on the same lock). Run a cargo test only when a verdict hinges on it, and then target one crate and one test name.
- Do not edit, create, commit or stash anything in the repo. Scratch files go under /tmp.
- Tests under crates/trek-app/src/tests and the inline #[cfg(test)] modules document intended behaviour. A behaviour that a passing test asserts is intended, not a bug.
- A bug is something a user can hit: a panic or crash, lost or corrupted data, wrong behaviour against a clear intent, a race, a resource leak, a security hole (path traversal, command injection, CSP bypass, secrets in logs), wrong platform assumptions, dead UI paths. Style, naming, missing docs and "could be simpler" are not bugs.
`

const known = args && Array.isArray(args.known) ? args.known : []

function knownBlock(slice) {
  const mine = known.filter(k => slice.paths.some(p => k.file.startsWith(p.split('*')[0])))
  if (!mine.length) return ''
  const lines = mine.map(k => `- ${k.file}: ${k.title} [${k.status}${k.reason ? `: ${k.reason}` : ''}]`).join('\n')
  return `\nAlready in the ledger for this slice. Do not report these again. "fixed" means it was fixed in an earlier commit: report it only if the defect is still present in the current code. "rejected" was checked and found not to be a bug.\n${lines}\n`
}

function focusBlock(slice) {
  return slice.focus && slice.focus.length ? `\nLook hardest at:\n${slice.focus.map(f => `- ${f}`).join('\n')}\n` : ''
}

function investigatorPrompt(slice) {
  return `${RULES}
You are one investigator in a bug-finding workflow over Trek. You have a fresh context and only this slice; other agents cover the rest, so stay inside it.

Your slice: ${slice.key}: ${slice.what}
Paths (git pathspecs): ${slice.paths.join(' ')}
List the files with: git ls-files -- ${slice.paths.map(p => `'${p}'`).join(' ')}
${focusBlock(slice)}${knownBlock(slice)}
Read every file in the slice in full. Do not sample. For each file, read it top to bottom before moving on; follow a call into another file only to settle a question about this slice. As you read, keep a running list of suspicious spots and come back to each one to decide.

Report only defects with a concrete failure scenario and the code that proves it. Rank by severity, then confidence. Report at most 10; if you have more, keep the strongest and name the rest in coverage. Do not pad: an empty list with an honest coverage note is a good answer. Every finding will be sent to skeptics whose job is to refute it, so quote real lines.`
}

function appDriverPrompt() {
  return `${RULES}
You are the app driver in a bug-finding workflow over Trek: the one agent allowed to run the isolated app. Read ./.agents/skills/verify-trek/SKILL.md and the Feature Map at .agents/skills/verify-trek/references/features/README.md first, then drive the app with the trek-dev CLI (run from the repo root): preflight, start, then walk through as many Feature Map pages as you can in about 60 minutes of driving. Use the CLI routes and controls the feature pages describe, take screenshots where the page lists visible success criteria, and read the logs (trek-dev logs --grep error, --grep panic, --grep warn) after each area. Stop the app when done (trek-dev stop).

Report defects you actually observed: crashes, panics, errors in the logs with the action that caused them, screens that do not match the page's success criteria, controls that do nothing. Give the exact CLI commands that reproduce each. Note that native web views only render while the dev window is frontmost; do not count that as a bug.`
}

function deltaPrompt(files) {
  return `${RULES}
You are one investigator in a bug-finding workflow over Trek. Your slice is the uncommitted work in the working tree: files changed or added since HEAD. Other agents cover the rest of the repo; stay inside these files, following a call elsewhere only to settle a question about them.

Files:
${files.map(f => `- ${f}`).join('\n')}

For each tracked file, read the change with: git diff HEAD -- '<file>' and then the whole file as it is on disk. Untracked files have no diff: read them in full. Read every file top to bottom; do not sample. Look for the usual defects and also for what a change in progress tends to leave behind: a new code path with no caller or an old caller left pointing at removed behaviour, a feature flag or setting read in one place and written in another with a different default, platform code (macOS, Windows) that compiles on one and not the other, resources acquired in new code and never released, tests that assert the old behaviour.

Report only defects with a concrete failure scenario and the code that proves it. Rank by severity, then confidence. Report at most 10; if you have more, keep the strongest and name the rest in coverage. Do not pad. Every finding will be sent to skeptics whose job is to refute it, so quote real lines.`
}

function skepticPrompt(f, lens) {
  const lensText = lens === 'trace'
    ? 'Your lens is code tracing: open the file at the line, read the surrounding function and every caller and callee that matters, and determine whether the failure scenario is reachable with the stated inputs. Check for guards, earlier validation, type invariants and tests that already cover it.'
    : 'Your lens is reproduction: try to demonstrate the failure concretely. Prefer a small standalone check (a scratch Rust or shell program under /tmp, or a targeted existing test with inputs that match the scenario) over argument. If you cannot demonstrate it and cannot show by reading that it must happen, it is refuted.'
  return `${RULES}
You are a skeptic in a bug-finding workflow over Trek. Another agent, which you have never spoken to, reported this bug. Your job is to try to disprove it. You gain nothing by agreeing. Default to real=false when uncertain.

${lensText}

The claim:
File: ${f.file}:${f.line}
Title: ${f.title}
Category: ${f.category}; reported severity: ${f.severity}; reporter confidence: ${f.confidence}
Summary: ${f.summary}
Failure scenario: ${f.failure_scenario}
Evidence quoted by the reporter:
${f.evidence}

Answer real=true only if the failure scenario (or a corrected version you state) holds against the current code. If the reporter's evidence is misquoted, the line is guarded, the path is unreachable, or a test asserts the behaviour is intended, answer real=false and say which.`
}

// Slice filtering
let wanted = args && Array.isArray(args.slices) && args.slices.length ? SLICES.filter(s => args.slices.includes(s.key)) : SLICES
if (args && args.maxSlices && Number(args.maxSlices) > 0) {
  wanted = wanted.slice(0, Number(args.maxSlices))
}
const driveApp = Boolean(args && args.driveApp === true)
// Uncommitted work: files changed or added in the working tree, read in full by one extra investigator.
const deltaFiles = args && Array.isArray(args.deltaFiles) ? args.deltaFiles : []
// Candidates from an earlier run (its investigator reports) that still need skeptics. Same shape as a finding, plus slice.
const extraFindings = args && Array.isArray(args.extraFindings) ? args.extraFindings : []
// sequential: one trace skeptic; a reproduce skeptic only if it says real; tie-breaker on a split (1 to 3 agents per finding).
// parallel: trace and reproduce together, tie-breaker on a split (2 to 3 agents per finding).
const skepticMode = (args && args.skepticMode) === 'parallel' ? 'parallel' : 'sequential'

log(`Configured: model=${investigatorModel}, effort=${effortLevel}, concurrency=${concurrency}, maxAgents=${maxAgents}, maxFindings=${maxFindings}`)
log(`Investigating ${wanted.length} slices${driveApp ? ' plus isolated app driver' : ''}`)

// Phase 1: Investigate
phase('Investigate')
const investigatorJobs = wanted.map(s => () =>
  spawnAgent(investigatorPrompt(s), {
    label: `investigate:${s.key}`,
    phase: 'Investigate',
    schema: FINDINGS,
    model: investigatorModel,
    effort: effortLevel,
  }).then(r => ({ slice: s.key, result: r }))
)

if (driveApp) {
  investigatorJobs.push(() =>
    spawnAgent(appDriverPrompt(), {
      label: 'investigate:app-driver',
      phase: 'Investigate',
      schema: FINDINGS,
      model: investigatorModel,
      effort: effortLevel,
    }).then(r => ({ slice: 'app-driver', result: r }))
  )
}

if (deltaFiles.length) {
  investigatorJobs.push(() =>
    spawnAgent(deltaPrompt(deltaFiles), {
      label: 'investigate:delta',
      phase: 'Investigate',
      schema: FINDINGS,
      model: investigatorModel,
      effort: effortLevel,
    }).then(r => ({ slice: 'delta', result: r }))
  )
}

// Run investigators in batches of `concurrency`
const rawReports = await runBatched(investigatorJobs, concurrency)
const reports = rawReports.filter(Boolean)
const failedSlices = [...wanted.map(s => s.key), ...(driveApp ? ['app-driver'] : []), ...(deltaFiles.length ? ['delta'] : [])].filter(k => !reports.some(r => r.slice === k && r.result))
if (failedSlices.length) log(`Investigators that returned nothing (failed/capped): ${failedSlices.join(', ')}`)

// Deduplicate findings across slices
const seen = new Set()
const rawFindings = []
for (const r of reports) {
  if (!r || !r.result) continue
  for (const f of r.result.findings || []) {
    const key = `${f.file}:${f.line}`
    const titleKey = f.title.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
    if (seen.has(key) || seen.has(titleKey)) continue
    seen.add(key)
    seen.add(titleKey)
    rawFindings.push({ ...f, slice: r.slice })
  }
}

let reused = 0
for (const f of extraFindings) {
  const key = `${f.file}:${f.line}`
  const titleKey = String(f.title || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim()
  if (seen.has(key) || seen.has(titleKey)) continue
  seen.add(key)
  seen.add(titleKey)
  rawFindings.push({ ...f, slice: f.slice || 'prior-run' })
  reused++
}
if (extraFindings.length) log(`${reused} of ${extraFindings.length} candidates carried over from an earlier run (the rest were duplicates)`)

log(`${rawFindings.length} distinct candidate findings from ${reports.length} completed investigator reports${extraFindings.length ? ' plus the carried-over ones' : ''}`)

if (!rawFindings.length) {
  return {
    stamp: args && args.stamp,
    confirmed: [],
    rejected: [],
    unverified: [],
    failedSlices,
    slicesRun: wanted.map(s => s.key),
    agentsSpawned: totalAgentsSpawned,
    modelsUsed: { investigator: investigatorModel, skeptic: skepticModel, tiebreak: tiebreakModel, effort: effortLevel },
    coverage: reports.map(r => ({ slice: r.slice, coverage: r.result && r.result.coverage })),
  }
}

// Rank candidates: critical > high > medium > low, then confidence
const SEVERITY_WEIGHT = { critical: 4, high: 3, medium: 2, low: 1 }
rawFindings.sort((a, b) => {
  const sevDiff = (SEVERITY_WEIGHT[b.severity] || 0) - (SEVERITY_WEIGHT[a.severity] || 0)
  if (sevDiff !== 0) return sevDiff
  return (b.confidence || 0) - (a.confidence || 0)
})

const toVerify = rawFindings.slice(0, maxFindings)
const deferred = rawFindings.slice(maxFindings).map(f => ({
  ...f,
  verdict: 'unverified',
  unverified_reason: `Deferred: beyond maxFindings limit (${maxFindings}). Run with specific slices or higher maxFindings to verify.`,
}))

if (deferred.length > 0) {
  log(`Selecting top ${toVerify.length} findings for verification; deferred ${deferred.length} lower-priority findings to prevent token exhaustion.`)
}

// Phase 2: Verify
phase('Verify')
log(`Verifying ${toVerify.length} candidate findings with skeptics (${skepticMode}; skeptic: ${skepticModel}, tiebreak: ${tiebreakModel}, effort: ${effortLevel})...`)

// Process findings in controlled batches.
// sequential: `concurrency` findings at a time, each using one skeptic at a time (trace, then reproduce only if trace says real).
// parallel: floor(concurrency / 2) findings at a time, each running its trace and reproduce skeptics together.
const findingsPerBatch = skepticMode === 'sequential' ? concurrency : Math.max(1, Math.floor(concurrency / 2))
const judged = []

function skepticOpts(f, lens, model) {
  return {
    label: `verify:${lens}:${f.file.split('/').pop()}:${f.line}`,
    phase: 'Verify',
    schema: VERDICT,
    model,
    effort: effortLevel,
  }
}

async function tieBreak(f, votes) {
  if (totalAgentsSpawned >= maxAgents) {
    log(`Agent cap reached; skipping tie-breaker for ${f.file}:${f.line}`)
    return
  }
  log(`Split vote on ${f.file}:${f.line}. Calling tie-breaker (${tiebreakModel})...`)
  const tie = await spawnAgent(
    skepticPrompt(f, 'trace') + '\n\nTwo earlier skeptics disagreed. Settle it: read until you are sure.',
    skepticOpts(f, 'tiebreak', tiebreakModel)
  )
  if (tie) votes.push(tie)
}

function settle(f, votes) {
  if (!votes.length) return { f, votes: [], verdict: 'unverified', unverified_reason: 'Skeptic checks failed or were capped' }
  const realVotes = votes.filter(v => v.real).length
  return { f, votes, verdict: realVotes > votes.length / 2 ? 'confirmed' : 'rejected' }
}

async function judgeSequential(f) {
  const trace = await spawnAgent(skepticPrompt(f, 'trace'), skepticOpts(f, 'trace', skepticModel))
  if (!trace) return settle(f, [])
  if (!trace.real) return settle(f, [trace])
  const repro = await spawnAgent(skepticPrompt(f, 'reproduce'), skepticOpts(f, 'reproduce', skepticModel))
  const votes = [trace, repro].filter(Boolean)
  if (votes.length === 2 && votes.filter(v => v.real).length === 1) await tieBreak(f, votes)
  return settle(f, votes)
}

async function judgeParallel(f) {
  const skepticThunks = ['trace', 'reproduce'].map(lens => () =>
    spawnAgent(skepticPrompt(f, lens), skepticOpts(f, lens, skepticModel))
  )
  const votes = (await parallel(skepticThunks)).filter(Boolean)
  if (votes.length === 2 && votes.filter(v => v.real).length === 1) await tieBreak(f, votes)
  return settle(f, votes)
}

for (let i = 0; i < toVerify.length; i += findingsPerBatch) {
  if (totalAgentsSpawned >= maxAgents) {
    log(`[Agent Cap] Reached max agents limit (${maxAgents}). Deferring remaining ${toVerify.length - i} findings.`)
    for (let j = i; j < toVerify.length; j++) {
      judged.push({
        f: toVerify[j],
        votes: [],
        verdict: 'unverified',
        unverified_reason: 'Agent cap reached before verification completed',
      })
    }
    break
  }

  const batch = toVerify.slice(i, i + findingsPerBatch)
  const batchResults = await parallel(batch.map(f => () => (skepticMode === 'sequential' ? judgeSequential(f) : judgeParallel(f))))
  judged.push(...batchResults.filter(Boolean))
}

const confirmed = judged.filter(j => j.verdict === 'confirmed').map(j => ({ ...j.f, verdicts: j.votes }))
const rejected = judged.filter(j => j.verdict === 'rejected').map(j => ({ ...j.f, verdicts: j.votes }))
const unverified = [
  ...judged.filter(j => j.verdict === 'unverified').map(j => ({ ...j.f, unverified_reason: j.unverified_reason })),
  ...deferred,
]

log(`Audit complete: ${confirmed.length} confirmed, ${rejected.length} rejected, ${unverified.length} unverified/deferred (total agents spawned: ${totalAgentsSpawned}/${maxAgents})`)

return {
  stamp: args && args.stamp,
  confirmed,
  rejected,
  unverified,
  failedSlices,
  slicesRun: wanted.map(s => s.key),
  agentsSpawned: totalAgentsSpawned,
  modelsUsed: {
    investigator: investigatorModel,
    skeptic: skepticModel,
    tiebreak: tiebreakModel,
    effort: effortLevel,
  },
  coverage: reports.map(r => ({ slice: r.slice, coverage: r.result && r.result.coverage })),
}
