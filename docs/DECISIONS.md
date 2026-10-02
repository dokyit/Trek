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
