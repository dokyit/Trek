# Trek

**Every agent. One trail.** A native macOS app (Rust + GPUI) for running coding agents —
Claude Code, Codex, and any API or local model — from one fast inbox.

- **Uses the subscriptions you already pay for** by driving each vendor's own CLI with your login
  (`claude` stream-json, `codex app-server`). Trek never reads those credentials.
- **API keys and local models**: Anthropic, OpenAI, Gemini, OpenRouter, DeepSeek, xAI, Mistral, Groq,
  Ollama, LM Studio, llama.cpp/MLX. Keys live in the macOS Keychain.
- **Pulls your existing threads** from Claude Code, Codex and OpenCode (read-only) and resumes them.
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
cargo test -p trek-core -p trek-agents -p trek-app   # unit tests + headless UI tests
cargo run -p trek-agents --example ping -- claude   # live smoke test (uses your Claude login)
script/bundle.sh [--install]     # release Trek.app + update archive + manifest in dist/
```
`.ref/gpui-kit` (gitignored) holds the GPUI Kit source; check real APIs there, GPUI changes weekly.

**UI tests** (`crates/trek-app/src/tests`) run Trek's real window and views on GPUI's headless test
platform against an in-memory database and a throwaway data folder (the Keychain stays untouched),
with the **mock agent** standing in for real ones. `cargo test -p trek-app -- --ignored --nocapture
rendering_cost` prints what a working-animation frame and a batch of streamed text cost, and how fast
a 2,000-item thread opens. Those numbers cover layout and painting only (the test platform has no
GPU). For the whole cost, drawing included, run a build with `TREK_LAUNCH_BEHIND=1
TREK_FORCE_ACTIVE=1 TREK_MOCK_AGENT=1 TREK_MOCK_PROMPT="mock:long 60s"` and a throwaway
`TREK_DATA_DIR`, and sample it with `top -pid`: it keeps drawing behind other windows.

**Mock agent** (`trek-agents/src/mock.rs`): a scripted agent with no process or network. Each prompt
plays a script picked by a keyword: none (a streamed markdown answer), `tools`, `subagents 5s`,
`permission`, `question`, `plan`, `mock:long 30s`, `mock:stream 30s`, `error`. Development switches:

| Variable | Effect |
| --- | --- |
| `TREK_DATA_DIR=/tmp/x` | keep settings and the database in another folder |
| `TREK_MOCK_AGENT=1` | offer "Mock agent" in the pickers |
| `TREK_MOCK_PROMPT="mock:long 60s"` | with the mock on, start a mock thread at launch |
| `TREK_LAUNCH_BEHIND=1` | open the window behind other apps without taking focus |
| `TREK_FORCE_ACTIVE=1` | run the working animation as if the window were in front, and keep drawing while it's covered (for measurements) |
