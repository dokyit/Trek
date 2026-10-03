# Trek

**Every agent. One trail.** Trek is a native macOS app (Rust + GPUI) that runs your coding agents —
Claude Code, Codex, OpenCode, Cursor, Copilot and other ACP agents, plus API and local models — from one
inbox-style window.

Trek is early (0.x). It's used daily by its author, but expect rough edges, and read
[What isn't done](#what-isnt-done) before you rely on it.

## What it does

**Your agents, with the logins you already have.** Trek drives each vendor's own CLI as a child
process, so your subscriptions work as they do in the terminal and Trek never reads or stores those
credentials:

- **Claude Code** through `claude`'s stream-json mode: permission prompts, AskUserQuestion and plan
  approval as cards, sub-agent progress, switching to Full access mid-thread.
- **Codex** through `codex app-server` (JSON-RPC): approvals, questions and plan approval as cards, plan
  progress, sub-agent progress, steering a running turn, OpenAI models.
- **OpenCode, Factory Droid and ACP agents** (Cursor, GitHub Copilot, Gemini CLI, Kimi, Qwen Code, Grok,
  Devin, Goose, Amp, Pi) through the Agent Client Protocol, when they're installed: approvals, plan
  progress, the agent's own slash commands, resuming saved sessions. OpenCode gets "ask" rules so Trek can
  gate its edits and commands, added only where your own OpenCode config sets none.
- **API keys and local models**: Anthropic, OpenAI, Gemini, OpenRouter, DeepSeek, xAI, Mistral, Groq,
  custom OpenAI-compatible endpoints, Ollama, LM Studio and llama.cpp / MLX. Keys live in the macOS
  Keychain (or come from your shell's `*_API_KEY` variables). These are chat only for now.

**One inbox for all of it.** Running threads and anything waiting on you sit at the top of the sidebar;
finished work settles on its own (after a few idle days, or when you settle it), and threads can be pinned,
snoozed, renamed and searched. Search is full text: the sidebar and the ⌘K palette find a thread by its
title or by anything said in it (imported history included), and a message match opens the thread at that
message. ⌘K also runs any command: new thread, settings pages, tools, theme, hand-holding. Any thread can
also open in a window of its own (⌘⇧↩ or the thread menu) with its own composer, while the main window
carries on. Any message can be edited and sent again, any turn undone or retried (with another model if
you like) and any point forked into a new thread; the agent forgets what was taken back, and in a git repo
the files go back too, from a checkpoint Trek takes as each turn starts. Projects get their own icon, defaults for new threads and title-bar actions. Trek imports your
existing Claude Code, Codex and OpenCode history (read-only) and resumes those threads, titled by what you
asked and with each message's time. Sessions that aren't your conversations (sub-agents, other apps' title
generators, one-shot and temp-folder runs) are left out; Settings › Import lists them and brings back any it
misjudged.

**A composer that knows the agent.** Model and effort per agent, four hand-holding levels (Supervised ·
Auto-accept edits · Auto · Full access, the last one unlocked once in Settings), Plan mode on ⇧Tab,
follow-ups that steer the running turn or wait in a queue, `@` for files, `/` for commands, `$` for skills,
images (pasted with ⌘V, dropped or picked), and app snapshots (⌘⇧S) attached straight to the message.
Cost is shown as money only when you pay per token (an API key); on a subscription such as Claude Max the
API-price estimate stays in a tooltip, since the plan already covers it.

**Tools next to the thread** (⌘J): a real terminal, an embedded browser with element picking, screenshots
and devtools, a live iOS Simulator mirror with touch and typing, a file explorer, source control (diff,
commit, push) and a side chat. Agents get Trek's own MCP server (`trek-mcp`) for computer use and the iOS
Simulator, plus any MCP servers you add.

**A good Mac citizen.** Menu bar icon (idle / working / needs you), notifications and Dock badge when an
agent finishes or needs a decision, keeps the Mac awake while agents work, Night and Paper themes, and
roughly 50 MB on disk. A built-in updater installs signed releases without ever interrupting a running agent.

## Screenshots

Screenshots are coming with the first public release.

## Install

Download `Trek-<version>-darwin-aarch64.app.tar.gz` from
[Releases](https://github.com/dokyit/Trek/releases), unpack it (double-click) and move **Trek.app** to
Applications. Trek needs macOS 13 or later on Apple silicon.

Releases are signed with Trek's own certificate but **not notarized by Apple**, so the first launch needs
one extra step. Either:

- In Finder, right-click (or Control-click) Trek.app → **Open** → **Open**; or, on macOS 15 and later,
  try to open it once, then go to System Settings → Privacy & Security and click **Open Anyway**.
- Or clear the download quarantine in Terminal:

  ```sh
  xattr -dr com.apple.quarantine /Applications/Trek.app
  ```

After that Trek updates itself: Settings → Updates picks the channel (Stable, Beta or Nightly). Updates are
checked once a day, downloaded in the background, verified (SHA-256 and a minisign signature against the
key built into Trek) and installed when you restart or quit Trek — never while an agent is working. If you
run Trek straight from Downloads without moving it, macOS runs a read-only copy and Trek can't update
itself until you move it to Applications; the same goes for a folder your account can't write to.

To use the agents, install and log in to their CLIs as usual (`claude`, `codex`, `opencode`, …). Trek finds
them through your login shell's `PATH`; Settings → Agents & Subscriptions shows what it found.

## Build from source

You need macOS 13+, Xcode and Rust (stable, edition 2024). The full Xcode, not just the Command Line Tools:
GPUI compiles its Metal shaders at build time. Since Xcode 26 the Metal compiler is a separate download; if
the build stops at "missing Metal Toolchain", run `xcodebuild -downloadComponent MetalToolchain` once.

```sh
git clone https://github.com/dokyit/Trek && cd Trek
cargo run -p trek-app                 # debug build, runs from target/
cargo test -p trek-core -p trek-agents -p trek-app -p trek-mcp
```

A development build never updates itself; rebuilding is the update. That's `cargo run`, and also a bundle
you build with `script/bundle.sh`: only bundles `script/release.sh` makes are releases. To build an app
bundle:

```sh
brew install resvg                    # renders the app icon
script/signing-identity.sh            # once: a stable self-signed code-signing identity
script/bundle.sh [--install]          # dist/Trek.app, optionally copied to /Applications
```

`bundle.sh` signs with `$TREK_SIGN_IDENTITY`, else the "Trek Local Signing" identity, else "Shelf Dev" (an
older self-signed identity on the maintainer's Mac), else ad-hoc. Use a stable identity: macOS ties
Accessibility and Screen Recording permission (computer use, snapshots) to the signing certificate, and an
ad-hoc signature changes on every build, so you'd be asked again each time.

Handy environment variables: `TREK_DATA_DIR=/some/folder` runs Trek against another data folder (your real
one is `~/Library/Application Support/dev.trek.Trek`), `TREK_BACKGROUND=1` opens behind other apps' windows
without taking focus, `TREK_OPEN_SETTINGS=updates`, `TREK_OPEN_TOOL=browser` and
`TREK_OPEN_THREAD_WINDOW=<thread id>` open a settings page, a tool or a thread window at launch, and
`TREK_ONBOARDING=1` replays onboarding. macOS stops drawing windows that other windows cover, so
screenshots of a `TREK_BACKGROUND=1` launch also need `TREK_FORCE_ACTIVE=1` (below), which keeps every Trek
window drawing. An update's relaunch keeps `TREK_DATA_DIR` and the like, never these launch-only flags.

### Tests and the mock agent

The test command above includes headless UI tests (`crates/trek-app/src/tests`): Trek's real windows and
views run on GPUI's test platform against an in-memory database and a throwaway data folder (the Keychain
stays untouched, and nothing reaches a real agent), with the **mock agent** standing in for real ones.

The mock agent (`crates/trek-agents/src/mock.rs`) is a scripted agent with no process or network. Each
prompt plays a script picked by a keyword: none (a streamed markdown answer), `tools`, `subagents 5s`,
`permission`, `question`, `plan`, `mock:long 30s`, `mock:stream 30s` or `error`. `TREK_MOCK_AGENT=1` offers it
as "Mock agent" in the pickers, and `TREK_MOCK_PROMPT="mock:long 60s"` (with the mock on) starts a mock
thread at launch.

`cargo test -p trek-app [--release] -- --ignored --nocapture rendering_cost` prints what a working-animation
frame and a batch of streamed text cost, and how fast a 2,000-item thread opens; those numbers cover layout
and painting only (the test platform has no GPU). For the whole cost, drawing included, run a build with
`TREK_BACKGROUND=1 TREK_FORCE_ACTIVE=1 TREK_MOCK_AGENT=1 TREK_MOCK_PROMPT="mock:long 60s"` and a throwaway
`TREK_DATA_DIR`, and sample it with `top -pid`. `TREK_FORCE_ACTIVE=1` is a measurement aid, not a setting: it
runs the working animation as if the window were in front and keeps drawing the window while it's covered.

## Releases

Releases are published to GitHub by `script/release.sh <version> [--channel stable|beta|nightly]`, which
sets the version, builds and signs the app, signs the archive with minisign, writes the channel manifest
and creates the GitHub release. Only stable releases commit their version to the branch; betas and
nightlies are tagged builds off it. Run it with `--dry-run` first. The full process, the channel layout, the
keys and how to test an update locally are in [docs/RELEASING.md](docs/RELEASING.md).

## Architecture

```
crates/trek-core     engine, no UI: types, settings (TOML), SQLite store, agent detection, thread import,
                     skills, model catalog, updater
crates/trek-agents   live sessions: Claude Code (stream-json), Codex (app-server JSON-RPC), ACP agents,
                     direct API / local models, the scripted mock agent; usage limits and auto titles
crates/trek-app      the GPUI app (gpui-kit 0.7): workspace state, sidebar, thread view, composer, settings,
                     onboarding, tools panel, menu bar, notifications; headless UI tests
crates/trek-mcp      MCP server agents use for computer use and the iOS Simulator
vendor/gpui-base     gpui-base 0.7 with Trek's small markdown patch (semibold emphasis, calmer headings)
assets/              brand art, backgrounds, the update signing public key
script/              bundle, release, signing identity, updater end-to-end test
docs/                DECISIONS.md, DESIGN.md, RESEARCH.md, RELEASING.md
```

The engine crates don't depend on the UI toolkit. All I/O runs on one shared tokio runtime and reaches the
UI over channels; agent output is batched per frame. Data lives in `~/Library/Application
Support/dev.trek.Trek` (settings.toml, trek.sqlite, snapshots, updates, and acp-agents: what each ACP agent
last reported, so Trek doesn't open a session to ask again). See
[docs/DECISIONS.md](docs/DECISIONS.md) for why things are the way they are.

## What isn't done

- **Apple silicon only, macOS only.** No Intel build is published; Windows and Linux aren't supported.
- **Not notarized.** First launch needs the step above. Notarization needs an Apple Developer ID.
- **API keys and local models are chat only.** Trek's own tool loop (read, edit, run with the hand-holding
  gates) isn't built yet; use a CLI agent for real work in a repository.
- **Imports** cover Claude Code, Codex and OpenCode; Cursor, Copilot and other agents' history isn't
  imported yet.
- **No screenshots or website yet**, and no delta updates (each update downloads the whole app, about
  15 MB).

## License

[Apache-2.0](LICENSE). `vendor/gpui-base` is a patched copy of Longbridge's gpui-base, Apache-2.0 under its
own [license](vendor/gpui-base/LICENSE-APACHE).
