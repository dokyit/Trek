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
snoozed, renamed and searched. Projects get their own icon, defaults for new threads and title-bar actions.
Trek imports your existing Claude Code, Codex and OpenCode history (read-only) and resumes those threads,
titled by what you asked and with each message's time. Sessions that aren't your conversations (sub-agents,
other apps' title generators, one-shot and temp-folder runs) are left out; Settings › Import lists them and
brings back any it misjudged.

**A composer that knows the agent.** Model and effort per agent, four hand-holding levels (Supervised ·
Auto-accept edits · Auto · Full access, the last one unlocked once in Settings), Plan mode on ⇧Tab,
follow-ups that steer the running turn or wait in a queue, `@` for files, `/` for commands, `$` for skills,
images, and app snapshots (⌘⇧S) attached straight to the message.

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
itself until you move it to Applications.

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

A development build (`cargo run`) never updates itself; rebuilding is the update. To build an app bundle:

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
one is `~/Library/Application Support/dev.trek.Trek`), `TREK_BACKGROUND=1` opens without taking focus,
`TREK_OPEN_SETTINGS=updates` and `TREK_OPEN_TOOL=browser` open a settings page or tool at launch, and
`TREK_ONBOARDING=1` replays onboarding.

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
                     direct API / local models; usage limits and auto titles
crates/trek-app      the GPUI app (gpui-kit 0.7): workspace state, sidebar, thread view, composer, settings,
                     onboarding, tools panel, menu bar, notifications
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
