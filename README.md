# Trek

**Every agent. One trail.** A native macOS app (Rust + GPUI) for running coding agents —
Claude Code, Codex, and any API or local model — from one fast inbox.

- **Uses the subscriptions you already pay for** by driving each vendor's own CLI with your login
  (`claude` stream-json, `codex app-server`). Trek never reads those credentials.
- **API keys and local models**: Anthropic, OpenAI, Gemini, OpenRouter, DeepSeek, xAI, Mistral, Groq,
  Ollama, LM Studio, llama.cpp/MLX. Keys live in the macOS Keychain.
- **Pulls your existing threads** from Claude Code, Codex and OpenCode (read-only) and resumes them.
- **⌘K** finds any thread by title or by what was said in it (SQLite full-text search, imported history
  included) and runs any command; a message match opens the thread at that message.
- **Inbox sidebar** (settle / snooze / pin / auto-settle), Codex-style **Power slider** model picker,
  four **hand-holding** levels (Supervised · Auto-accept edits · Auto · Full access), Plan mode.
- **Built-in updater**: checks a signed manifest, downloads in the background, and restarts only when
  agents are idle. **Menu-bar icon** shows idle / working / needs-you.
- Small and light: ~22 MB app, ~120–170 MB RAM (Electron competitors: 320–960 MB on disk, ~1 GB RAM).

## Layout
```
crates/trek-core     engine: types, settings, SQLite store, agent detection, thread import, updater
crates/trek-agents   live sessions: Claude Code, Codex app-server, direct API/local (ACP next)
crates/trek-app      GPUI UI (gpui-kit 0.7): sidebar, thread view, composer, settings, onboarding, tray
assets/brand         icon + menu-bar glyph sources (trek_icon.py, menubar.py)
docs/                RESEARCH.md · DECISIONS.md · DESIGN.md
```

## Develop
```sh
cargo run -p trek-app            # debug app
cargo test -p trek-core -p trek-agents
cargo run -p trek-agents --example ping -- claude   # live smoke test (uses your Claude login)
script/bundle.sh [--install]     # release Trek.app + update archive + manifest in dist/
```
`TREK_DATA_DIR=/tmp/trek-scratch` runs against a scratch data folder; `TREK_BACKGROUND=1` opens the window
without making Trek active or taking focus.
`.ref/gpui-kit` (gitignored) holds the GPUI Kit source; check real APIs there, GPUI changes weekly.
