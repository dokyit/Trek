# Trek — Key Decisions (2026-10-01)

## 1. Stack: Rust + GPUI (via GPUI Kit)

**Decision:** Native Rust app. UI on GPUI through `gpui-kit = "=0.7.0"` (which pins `gpui-pre` 0.3.7).
All agent orchestration, storage, git, PTY and protocol code lives in UI-agnostic crates
(`trek-core`, `trek-agents`) so the UI layer could be swapped (Tauri / web) without touching the engine.

Why:
- Every competitor installed on this machine except Zed is Electron: T3 Code 414 MB, Cursor 963 MB,
  Antigravity 410 MB, OpenCode 408 MB, Orca 568 MB, Synara 704 MB, Capy 320 MB on disk.
  T3 Code Nightly measured at ~1.2 GB RSS while running. Codex/ChatGPT desktop has public 6–8 GB+ reports.
- GPUI apps: ~60–150 MB idle, ~0 % idle CPU (redraws only on change), 120 fps GPU rendering, <300 ms startup.
- GPUI Kit ships what Trek needs: Input w/ IME, Textarea, markdown TextView with highlighting, VirtualList,
  Sidebar, Resizable, Dock, Select/Combobox/Command palette, Slider, Switch, Dialog/Sheet/Notification,
  Settings panel, Message/MessageScroller/Shimmer/Skeleton, plus a Motion layer (retargetable springs,
  `Presence` exit animations, keyframes, `MotionReveal`).
- Zed (GPUI) proves the agent-panel use case; Zeron, Runner, Waku, Arbor are GPUI agent harnesses too.

Accepted costs / risks:
- ~2× slower UI iteration than React; no devtools; LLMs know older GPUI APIs → `.ref/gpui-kit` is vendored
  locally (gitignored) and is the source of truth for every API we call. **Never guess an API.**
- GPUI is pre-1.0 with weekly breaking changes → pin exact versions, upgrade deliberately.
- Web preview pane (wry) is an overlay with no clipping — keep it a fixed pane.
- Terminal: build on `alacritty_terminal` (Apache-2.0). Do NOT copy Zed's `terminal`/`auto_update` crates (GPL).
- Tokio: GPUI executors aren't tokio. One shared tokio runtime (`trek_core::runtime()`) for I/O;
  results are sent back to the UI over channels and applied on the foreground thread. Stream tokens are
  batched per frame, never `cx.notify()` per token. Keep every `Task` handle (dropping cancels).

Rejected: Electron (the problem we're solving), Tauri (WebKit perf is why OpenCode left it; still a fallback),
SwiftUI (no Windows/Linux), Iced/Slint/Dioxus/Makepad (immature for this app class).

## 2. Model access: three backend kinds behind one trait

Legal constraint that drives the design: **Anthropic forbids third-party apps from using Claude.ai
subscription credentials**, but explicitly allows the user signing in to the *unmodified Claude Code binary*.
Google suspended accounts for Gemini-CLI OAuth reuse. OpenAI launched "Sign in with ChatGPT" for third-party
apps (2026-09-29) and documents `codex app-server` for product integration.

So Trek never touches subscription tokens. It drives the vendor's own CLI the user is logged into.

| Kind | Used for | Transport |
|---|---|---|
| **Native CLI** | Claude Code, Codex, OpenCode, Factory Droid | `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio`; `codex app-server` (JSON-RPC over stdio, types generated from `codex app-server generate-json-schema`); `opencode serve` (HTTP+SSE); `droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc` |
| **ACP** (Agent Client Protocol v1, crate `agent-client-protocol` 2.2) | Cursor (`agent acp`), Copilot (`copilot --acp`), Gemini (`gemini --acp`), Kimi, Qwen, Grok, Devin, Goose, Amp, Pi, + registry (~60 agents) | stdio JSON-RPC; registry at `cdn.agentclientprotocol.com/registry/v1/latest/registry.json` |
| **Direct** (Trek's own agent loop + tools) | API keys (Anthropic, OpenAI, Google, OpenRouter, DeepSeek, xAI, Mistral, Groq, any OpenAI-compatible), local models (Ollama :11434, LM Studio :1234, llama.cpp/MLX :8080), later Sign in with ChatGPT | 3 thin wire adapters: Anthropic Messages, OpenAI Responses/Chat, Gemini; Ollama native |

Model metadata (context, cost, tool support, **reasoning options**) comes from `models.dev/api.json`
(cached snapshot shipped in-app, refreshed daily).

### Effort mapping (one internal scale → every provider)
Internal: `Off · Minimal · Low · Medium · High · XHigh · Max`. Clamp to what the model supports
(`reasoning_options` on models.dev).
- Anthropic (Opus/Sonnet 5.x, Fable): `thinking:{type:"adaptive"}` + `output_config.effort` low…max (no `budget_tokens`; 400s). Haiku 4.5: `budget_tokens`.
- OpenAI: `reasoning.effort`. Gemini 3+: `thinkingLevel`; Gemini 2.5: `thinkingBudget` (fraction of max). Ollama: `think`.
- CLIs: `claude --effort`, Codex `turn/start.effort`, ACP config option in the model category.

## 3. Hand-holding (permission) modes

Trek's four modes, mapped per backend:

| Trek mode | Meaning | Claude Code | Codex | ACP / Direct |
|---|---|---|---|---|
| **Supervised** | Ask before edits and commands | `default` | `read-only` + `on-request`, reviewer=user | ask every tool |
| **Auto-accept edits** | File edits auto-applied; commands ask | `acceptEdits` | `workspace-write` + Trek auto-approves file-change requests | edits allow, exec ask |
| **Auto** | Runs freely; risky actions checked | `auto` | `workspace-write` + `approvals_reviewer=auto_review` | allow, deny-list + risk classifier asks |
| **Full access** | No prompts, no sandbox | `bypassPermissions` | `danger-full-access` + `never` | allow all |

Plan mode is a separate toggle (⇧Tab), not a hand-holding level, matching Claude Code/Codex/Factory.
Full access must be unlocked once in Settings → Permissions (Claude/Codex both gate it this way).

## 4. Thread import ("pull threads from the machine")

Prefer the tool's own API, fall back to read-only file parsing. Index metadata only; load transcripts lazily
(Claude Code JSONL files on this machine reach 130 MB).

Helper sessions (sub-agents, untouched forks, title generators, temp-folder and one-shot runs, empty
sessions, Trek's own) are left out by conservative rules, since a false positive hides a real conversation;
already-imported ones are archived unless they were pinned or continued in Trek, and "Show in sidebar"
(Settings › Import) marks a session kept so later imports leave it. Titles come from the source's own title when it's
meaningful, else the first real user message; the imported title is stored so a rename in Trek survives
re-import. Imported turns get the live response footer, timed wall-clock from the user's message.

| Source | Where | Resume |
|---|---|---|
| Claude Code | `~/.claude/projects/<cwd-slug>/<id>.jsonl` | `claude -p --resume <id>` |
| Codex | `~/.codex/state_5.sqlite` `threads` + rollout JSONL; or app-server `thread/list` | app-server `thread/resume` |
| OpenCode | `~/.local/share/opencode/opencode.db` | `opencode serve` `/session/:id` |
| Cursor CLI | `~/.cursor/projects/<slug>/agent-transcripts/` | ACP `session/resume` |
| Copilot CLI | `~/.copilot/session-state/<id>/events.jsonl` | `copilot --resume` / ACP |
| T3 Code | `~/.t3/userdata/state.sqlite` `projection_threads` | resume cursor → underlying CLI |
| Pi | `~/.pi/agent/sessions/` | `pi --session` / pi-acp |

## 5. Updater

Built-in, no manual re-download. GitHub Releases feed: stable is the latest non-prerelease release
(`releases/latest/download/stable.json`), beta and nightly are prereleases under fixed, moving tags
(`releases/download/beta/beta.json`); higher channels also see the stable feed. The manifest has the
version, notes and per-platform archive URL, SHA-256 and minisign signature; the public key is compiled in
(`assets/update/minisign.pub`). Flow: check on launch and daily → download in the background → verify →
unpack and inspect the bundle → "Update" pill in the sidebar footer and **Restart to update** → install
only when no agent turn is running, or on quit (Conductor's pattern; Claude desktop's update-kills-sessions
bug is the anti-pattern). The swap is one atomic rename with one backup kept, and never replaces a version
that's already as new. Changing the channel drops whatever the old channel downloaded. Bundles are signed with a
stable identity so macOS permissions survive updates. Swap in Sparkle (macOS) or Velopack later if delta
updates are needed. Details: docs/RELEASING.md.

## 6. Transcripts, search and ⌘K

**Append-only transcript rows with stable ids.** Each item is a row (`items`: uuid v7 `id`, `thread_id`,
`seq`, JSON `data`, `created_at`). The live transcript (`trek_core::transcript::Transcript`) carries the id of
every item and records what changed (appended, edited, removed), so a save writes only those rows, in one
transaction. Streaming text and tool status are edits to one row; empty thoughts dropped at turn end are
deletes, and because views and saves key on ids, the shifted positions don't matter. Ids are what later
features hang off: edit a message, retry or fork from a point (`Store::truncate_after(thread, id)`),
per-turn checkpoints. Databases from before ids are migrated on open (rows keep their order and timestamps).
`id` has a SQL default (a random uuid), so an older Trek build sharing the database still saves; SQLite
allows an expression default only in CREATE TABLE, so tables without it are rebuilt on open, ids and keys
unchanged. A message sent to an imported thread while its history is still being read waits ("1 queued")
and goes out after it, so stored rows never land ahead of the history.

**Full-text search is SQLite FTS5** (bundled SQLite has it compiled in), tokenizer `unicode61` with
diacritics folded, prefix indexes for as-you-type queries. Every word typed must match, each as a prefix.
- Titles and stored messages (yours and the agent's; not tool output or thinking) are indexed by triggers,
  so every write keeps the index current, older builds' writes included. Rows from before the index
  existed, and big appends (an imported thread's first save, megabytes of history), skip the triggers and
  are indexed in the background in transactions of about 256 KB of text; nothing blocks the window and
  the store is never held for long. Only a message's first 64K characters are indexed (pasted logs
  otherwise dominate the index).
- Titles are keyed through a table of thread ids, not `threads.rowid`: that table has no INTEGER PRIMARY
  KEY, so VACUUM may renumber its rowids, and an older build's INSERT OR REPLACE moves the row. Entries an
  older build orphaned are pruned when indexing starts.
- **Imported threads** keep their transcripts in the other agent's files until you continue them in Trek
  (copying them would freeze them at import time). Their messages go into a separate index keyed by position
  in `load_transcript`'s output: after each import, transcripts up to 16 MB are read in the background
  (on this machine that is most Claude Code and Codex sessions, ~300 MB, a few seconds, once); bigger ones
  are indexed the first time they are opened. A thread that changes outside Trek is re-indexed at the next
  import; one continued in Trek switches to its stored, id-keyed rows.
- `Store::search(query, limit)` returns title matches, then the best message match per thread (thread,
  title, position, item id when stored, one-line snippet with match ranges). The best match per thread is
  picked before the limit applies, so one thread with many matching messages can't crowd out the rest.
  Titles come back as written; message excerpts drop markdown emphasis and code ticks.

**⌘K palette** (`command_palette.rs`): threads (title matches, then message matches with the snippet),
projects (new thread in it, its settings), commands (new thread, open folder, every settings page, tools
panel and each tool, theme, hand-holding for the thread on screen, settle, check for updates). Commands
rank by prefix, then word start, then all words, then substring, then keywords, then letters in order.
Opening a message match opens its thread scrolled to that message (a long message of yours is opened up),
tinted for a moment. The sidebar's search field matches titles as you type (substring at once, then the
same word-prefix rules as ⌘K) and adds threads whose messages match, with the matching line. Results on
screen refresh when the index takes in more (background indexing, a finished turn); Enter in ⌘K waits for
the results of what was typed.

**Checkpoints, rewind, edit, retry and fork** (`trek_core::checkpoint`, `trek_core::rewind`,
`Workspace::rewind` / `fork_thread`).
- **File checkpoints are commits under `refs/trek/checkpoints/<thread>/<message>`**, made through a
  temporary index seeded with a copy of the user's (its stat data means only changed files are read):
  `add -A`, `write-tree`, `commit-tree -p HEAD`, `update-ref`. The user's index, HEAD, branches and stash are
  never written; restoring reads the checkpoint into another temporary index and `checkout-index`es just the
  files that differ, after removing files created since (ignored files are never in either tree). git is
  run by its real path (`git --exec-path`), not macOS's `/usr/bin/git` shim, which costs ~100 ms a call. A
  snapshot takes ~75 ms on this repo and ~130 ms on a 30,000-file one. It runs off the main thread as a turn
  starts, and the message is held until it's done, so it always shows the files before the agent touched
  them (a message that steers a running turn gets none). The newest 100 per thread are kept; refs go with
  the messages a rewind removes and with deleted threads (after any snapshot still running). Folders
  outside git get none.
- **A snapshot never holds a turn up for long, and never fails over one path.** Reading the files is
  stopped after 10 s (a clean filter that hangs, huge untracked files); the message then goes without a
  checkpoint, and the popover says why. Stop never waits behind git work: a message still held for its
  checkpoint simply never reaches the agent. `add -A --ignore-errors` leaves out what git can't take (a
  nested repository with no commit yet) the same way every time. Nested repositories (mode 160000: a
  clone the agent made) are left as they are by a restore, never deleted. A file that can't be put back
  doesn't stop the others; the toast names it, and the checkpoints a failed restore would have dropped are
  kept. There's no size cap on untracked files: `.gitignore` is the way to keep artifacts out.
- **Every restore can be undone.** Before it writes anything, a restore commits the files as they are to
  `refs/trek/undo/<thread>`; the "Restored N files" toast's Undo restores that. Edit & resend restores by
  default (as the popover does) and shows the count, with the files in its tooltip, before it's sent.
- **The agent forgets what was taken back.** Each message records where the agent's session stood when it
  was sent (`ResumePoint`: session id and the last message uuid / turn id before it, from
  `AgentEvent::Mark`; importers fill it in for Claude Code and Codex history). Claude Code resumes with
  `--resume <id> --resume-session-at <uuid>` (in place: the session file keeps the old branch, later
  resumes follow the new one) and forks with `--fork-session`; message uuids survive a fork. Codex reverts
  in place with `thread/revert` (the turn after the point is found with `thread/turns/list`) and forks with
  `thread/fork { lastTurnId }`; turn ids survive a fork too. Both verified live (haiku 4.5, gpt-5.6-luna).
  A point in a session other than the thread's own (the thread is a fork) is always forked, never cut in
  place: that session belongs to another thread. ACP and direct agents, messages without a point and
  cut-backs that fail (the session or message is gone) start a new session whose first message carries a
  compact recap of the conversation kept; the transcript says so.
- **A thread that doesn't know where its session stands finds out before it sends.** Imported threads,
  and ones an older Trek kept, have no latest point; their next message is held while the point is read
  from the agent's own files (`trek_agents::session_tail`: the last message uuid of the Claude Code
  session, the last ended turn of the Codex rollout), so it can still be cut back natively.
- **Commands Trek answers itself (`/model`, `/cost`) and typed answers to the agent's questions are
  asides** (`Item::User::aside`): they don't start turns and aren't rewind points (they keep only Copy).
  Undo, Retry and Fork sit on every turn's end: its footer, its error, or its "Interrupted" line; a turn the
  agent started itself (a sub-agent reporting back) has nothing to undo, and says so.
- Rewinding is refused while a turn runs. Sessions a thread leaves behind are recorded
  (`retired_sessions`), whether a rewind or fork planned it or the agent moved to a new session itself
  (a cut-back that failed, `/clear`), so imports don't bring them back as threads; an imported thread that
  moves to a new session becomes Trek's own.
- **In worktrees.** A worktree thread's checkpoints are of its worktree (its own index; the refs live in
  the repository it shares with the project folder). A fork of a worktree thread works in the same
  worktree, since its files and the agent's session are there (agents keep sessions per folder). The two
  share it: both are warned when both run, deleting or archiving one leaves the worktree to the other, and
  removing it moves every thread in it to the project folder. A thread that leaves its worktree drops its
  checkpoints and the points its old session could be cut back to; earlier messages rewind without files.
