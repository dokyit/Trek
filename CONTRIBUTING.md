# Contributing to Trek

Thanks for helping. Bug reports, ideas, docs fixes and pull requests are all welcome.

- **Bugs and ideas**: [open an issue](https://github.com/dokyit/Trek/issues/new/choose). Search first; add a +1 to an existing one rather than opening a twin.
- **Bigger changes**: open an issue first, so we agree on the shape before you spend a weekend on it.
- **Security issues**: never in a public issue. See [SECURITY.md](SECURITY.md).
- Everyone here follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Set up

| | macOS | Windows |
| --- | --- | --- |
| OS | 13+ on Apple silicon | 11 on x64 |
| Toolchain | Full Xcode (not just the Command Line Tools) | Visual Studio 2022 Build Tools (C++ workload) + Windows SDK |
| Rust | pinned in [`rust-toolchain.toml`](rust-toolchain.toml); rustup installs it | same, MSVC toolchain |
| Scripts | zsh | PowerShell 7 (`pwsh`) |

```sh
git clone https://github.com/dokyit/Trek && cd Trek
cargo run -p trek-app
```

On Xcode 26 and later the Metal compiler is a separate download. If the build stops at
"missing Metal Toolchain", run `xcodebuild -downloadComponent MetalToolchain` once.

The iPhone app is a separate Xcode project: see [ios/README.md](ios/README.md). Its demo mode
needs no Mac.

## Where things live

```
crates/trek-core     engine, no UI: settings, SQLite store, agent detection, import, updater
crates/trek-agents   live sessions: Claude Code, Codex, ACP agents, API and local models, the mock agent
crates/trek-app      the GPUI app: workspace, sidebar, thread view, composer, tools, settings
crates/trek-mcp      the MCP server agents use (computer use, iOS Simulator, sub-agents)
crates/trek-remote   the phone and machine link (WebSocket over pinned TLS)
ios/                 Trek for iPhone (SwiftUI)
vendor/              patched copies of upstream crates (gpui-base, gpui-pre)
script/              bundle, release, capture and test scripts
media/               capture manifests for screenshots and clips
docs/                design notes; start at docs/README.md
```

The engine crates don't depend on the UI. All I/O runs on one shared tokio runtime and reaches
the UI over channels. [docs/DECISIONS.md](docs/DECISIONS.md) explains the larger choices.

## Run it safely: the mock agent

Never point automated runs at your real profile: imported threads can resume real sessions.

- **`TREK_MOCK_AGENT=1`** adds a scripted "Mock agent" with no process or network. A keyword in
  the prompt picks the script: `tools`, `permission`, `question`, `plan`, `subagents 5s`,
  `mock:long 30s`, `mock:stream 30s`, `error`, … (full list at the top of
  [`crates/trek-agents/src/mock.rs`](crates/trek-agents/src/mock.rs)).
- **`TREK_DATA_DIR=/some/folder`** runs Trek against a throwaway data folder.
- **The verify driver** does all of this for you: a disposable profile, the mock agent only,
  screenshots without Screen Recording permission.

  ```sh
  ./.agents/skills/verify-trek/scripts/trek-dev preflight
  ./.agents/skills/verify-trek/scripts/trek-dev start
  ./.agents/skills/verify-trek/scripts/trek-dev send "tools"
  ./.agents/skills/verify-trek/scripts/trek-dev screenshot thread --output /tmp/trek.png
  ./.agents/skills/verify-trek/scripts/trek-dev check     # what CI runs, plus a UI smoke test
  ./.agents/skills/verify-trek/scripts/trek-dev stop
  ```

  On Windows, run `pwsh .agents/skills/verify-trek/scripts/trek-dev.ps1` with the same commands.
  The [skill file](.agents/skills/verify-trek/SKILL.md) has the details.

Other launch flags: `TREK_OPEN_SETTINGS=updates`, `TREK_OPEN_TOOL=browser`,
`TREK_OPEN_BASECAMP=1`, `TREK_ONBOARDING=1`, `TREK_BACKGROUND=1` (opens behind other windows).

## Before you open a pull request

```sh
cargo fmt --all
cargo test --workspace
```

CI runs both on macOS and Windows (`cargo fmt --all -- --check`, `cargo test --workspace --locked`).
`crates/trek-app/tests` holds headless UI tests: Trek's real views on GPUI's test platform,
with an in-memory database and the mock agent.

For UI changes, check the change in the running app (the verify driver above) and add a
before/after screenshot to the pull request. README and docs images come from
`script/capture.sh` (see [media/README.md](media/README.md)).

## Style

- Format with `rustfmt`; match the code around you.
- Comments say *why*, not *what*.
- Text people read in the app is short, plain and specific. "Couldn't reach the Mac" beats
  "An error occurred".
- New platform-specific code goes behind `cfg(target_os = …)` and keeps the other platform
  building.

## Commits

Look at `git log` and match it:

- **Subject**: `Area: what changed, in plain words`. For example
  `Browser: pages follow Trek's theme from the first load` or
  `Phone: the transcript keeps the reader's place`.
- **Body**: what was wrong and why the change fixes it, wrapped at about 100 columns.
- One logical change per commit, with its tests.

## Releases

Maintainers publish releases from the GitHub **Release** workflow, which builds the Mac and
Windows apps and signs the updates. The process is in [docs/RELEASING.md](docs/RELEASING.md).

## License

By contributing, you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE), like the rest of Trek.
