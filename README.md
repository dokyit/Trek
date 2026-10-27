<div align="center">

<img src="docs/assets/trek-icon.png" width="112" height="112" alt="Trek app icon">

<h1>Trek</h1>

<p><b>Every agent. One trail.</b></p>

<p>A native desktop app that runs your coding agents from one inbox.<br>
Claude Code, Codex, OpenCode, Cursor, Copilot, API and local models. On macOS and Windows, with an iPhone companion.</p>

<p>
  <a href="https://github.com/dokyit/Trek/releases/latest"><img src="https://img.shields.io/github/v/release/dokyit/Trek?label=release&color=F4692B" alt="Latest release"></a>
  <a href="https://github.com/dokyit/Trek/actions/workflows/ci.yml"><img src="https://github.com/dokyit/Trek/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License: Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20iOS-555" alt="Platforms: macOS, Windows, iOS">
</p>

<p>
  <a href="https://github.com/dokyit/Trek/releases/latest"><img src="docs/assets/buttons/download-macos.png" width="236" height="56" alt="Download for macOS"></a>
  &nbsp;
  <a href="https://github.com/dokyit/Trek/releases/latest/download/Trek-Setup-x86_64.exe"><img src="docs/assets/buttons/download-windows.png" width="250" height="56" alt="Download for Windows"></a>
</p>

<sub><a href="https://github.com/dokyit/Trek/releases">All releases</a> · <a href="#install">Install notes</a> · <a href="#build-from-source">Build from source</a></sub>

</div>

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/mac/hero-night.png">
  <img src="docs/assets/screenshots/mac/hero-paper.png" alt="Trek on macOS: a plan waiting for approval, with threads that need you in the sidebar">
</picture>

## Platforms

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/assets/icons/platforms/macos.svg" width="24" height="24" align="top" alt="">&nbsp; <b>macOS</b> &nbsp;<sub>13+ · Apple silicon</sub>
      <br><br>
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/mac/approval-night.png">
        <img src="docs/assets/screenshots/mac/approval-paper.png" alt="Trek on macOS: an approval card waiting for a decision">
      </picture>
      <br>
      <sub>Menu bar icon, Dock badge, notifications, iOS Simulator.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/assets/icons/platforms/windows.svg" width="24" height="24" align="top" alt="">&nbsp; <b>Windows</b> &nbsp;<sub>11 · x64</sub>
      <br><br>
      <img src="docs/assets/screenshots/windows/thread-night.png" alt="Trek on Windows: a finished answer in the Night theme">
      <br>
      <sub>Tray icon, taskbar badge, toasts, Mica.</sub>
    </td>
  </tr>
</table>

<table>
  <tr>
    <td colspan="3">
      <img src="docs/assets/icons/platforms/iphone.svg" width="24" height="24" align="top" alt="">&nbsp; <b>iPhone</b> &nbsp;<sub>iOS 26 · SwiftUI</sub>
      <br>
      <sub>The agents keep running on your computer; the phone is the inbox and the remote. Every thread and its status, approvals, questions and plans, steering, new threads, Notes and voice input. Built from source for now (<a href="ios/README.md">ios/README.md</a>).</sub>
    </td>
  </tr>
  <tr>
    <td width="33%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/iphone/threads-dark.png">
        <img src="docs/assets/screenshots/iphone/threads-light.png" width="250" alt="Trek for iPhone: every thread and its status">
      </picture>
      <br><sub>Threads</sub>
    </td>
    <td width="33%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/iphone/approval-dark.png">
        <img src="docs/assets/screenshots/iphone/approval-light.png" width="250" alt="Trek for iPhone: answering an approval">
      </picture>
      <br><sub>Approvals</sub>
    </td>
    <td width="33%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/iphone/basecamp-dark.png">
        <img src="docs/assets/screenshots/iphone/basecamp-light.png" width="250" alt="Trek for iPhone: Basecamp, what needs you today">
      </picture>
      <br><sub>Basecamp</sub>
    </td>
  </tr>
</table>

## Features

<table>
  <tr><th colspan="4" align="left">Agents and models</th></tr>
  <tr>
    <td width="25%" valign="top"><img src="docs/assets/icons/features/bot.svg" width="22" height="22" alt=""><br><b>Your CLI agents</b><br><sub>With the logins you already have</sub></td>
    <td width="25%" valign="top"><img src="docs/assets/icons/features/key-round.svg" width="22" height="22" alt=""><br><b>API and local models</b><br><sub>Keys in the system keychain</sub></td>
    <td width="25%" valign="top"><img src="docs/assets/icons/features/messages-square.svg" width="22" height="22" alt=""><br><b>Agents consult agents</b><br><sub>Codex reviews Claude's plan, and back</sub></td>
    <td width="25%" valign="top"><img src="docs/assets/icons/features/shield-check.svg" width="22" height="22" alt=""><br><b>You stay in charge</b><br><sub>Four hand-holding levels, plus Plan mode</sub></td>
  </tr>
  <tr><th colspan="4" align="left">Inbox and threads</th></tr>
  <tr>
    <td valign="top"><img src="docs/assets/icons/features/inbox.svg" width="22" height="22" alt=""><br><b>One inbox</b><br><sub>Running and waiting threads on top</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/search.svg" width="22" height="22" alt=""><br><b>Search everything</b><br><sub>Full-text search and a ⌘K palette</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/undo-2.svg" width="22" height="22" alt=""><br><b>Undo any turn</b><br><sub>Edit, retry or fork; files roll back too</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/download.svg" width="22" height="22" alt=""><br><b>Bring your history</b><br><sub>Imports Claude Code, Codex and OpenCode</sub></td>
  </tr>
  <tr><th colspan="4" align="left">Review and git</th></tr>
  <tr>
    <td valign="top"><img src="docs/assets/icons/features/file-diff.svg" width="22" height="22" alt=""><br><b>Changes per turn</b><br><sub>See, review or undo each turn's edits</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/git-branch.svg" width="22" height="22" alt=""><br><b>Worktrees</b><br><sub>Give a thread its own branch</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/git-pull-request.svg" width="22" height="22" alt=""><br><b>Pull request cards</b><br><sub>Checks, conflicts and merge, live</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/code-xml.svg" width="22" height="22" alt=""><br><b>Built-in editor</b><br><sub>Keep or undo the agent's hunks</sub></td>
  </tr>
  <tr><th colspan="4" align="left">Tools beside the thread</th></tr>
  <tr>
    <td valign="top"><img src="docs/assets/icons/features/square-terminal.svg" width="22" height="22" alt=""><br><b>Terminal</b><br><sub>A real one, next to every thread</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/globe.svg" width="22" height="22" alt=""><br><b>Browser</b><br><sub>Element picking, devtools, agent-driven</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/tablet-smartphone.svg" width="22" height="22" alt=""><br><b>iOS Simulator</b><br><sub>A live mirror with touch (macOS)</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/mouse-pointer-click.svg" width="22" height="22" alt=""><br><b>Computer use and MCP</b><br><sub>Opt-in, plus your own MCP servers</sub></td>
  </tr>
  <tr><th colspan="4" align="left">Phone and machines</th></tr>
  <tr>
    <td valign="top"><img src="docs/assets/icons/features/smartphone.svg" width="22" height="22" alt=""><br><b>Trek for iPhone</b><br><sub>Answer approvals away from your desk</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/bell-ring.svg" width="22" height="22" alt=""><br><b>Push through ntfy</b><br><sub>Know when an agent needs you</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/monitor-smartphone.svg" width="22" height="22" alt=""><br><b>Your other machines</b><br><sub>Start and follow threads on them</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/lock.svg" width="22" height="22" alt=""><br><b>Pinned TLS pairing</b><br><sub>Over your LAN or Tailscale</sub></td>
  </tr>
  <tr><th colspan="4" align="left">Native</th></tr>
  <tr>
    <td valign="top"><img src="docs/assets/icons/features/app-window-mac.svg" width="22" height="22" alt=""><br><b>Menu bar and tray</b><br><sub>Idle, working or needs you</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/refresh-cw.svg" width="22" height="22" alt=""><br><b>Signed updates</b><br><sub>Stable, Beta or Nightly; never mid-turn</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/palette.svg" width="22" height="22" alt=""><br><b>Night, Paper, glass</b><br><sub>Themes for the whole app</sub></td>
    <td valign="top"><img src="docs/assets/icons/features/feather.svg" width="22" height="22" alt=""><br><b>Light and fast</b><br><sub>Rust and GPUI, drawn on the GPU</sub></td>
  </tr>
</table>

<table>
  <tr>
    <td width="50%">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/mac/basecamp-night.png">
        <img src="docs/assets/screenshots/mac/basecamp-paper.png" alt="Basecamp: what needs you today, with turns, tokens and failures">
      </picture>
      <p align="center"><sub><b>Basecamp</b>: today across every agent</sub></p>
    </td>
    <td width="50%">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/mac/editor-night.png">
        <img src="docs/assets/screenshots/mac/editor-paper.png" alt="The editor, with the agent's chat and plan beside the code">
      </picture>
      <p align="center"><sub><b>Editor</b>: the agent beside your code</sub></p>
    </td>
  </tr>
</table>

## Works with

Trek drives each vendor's own CLI, so your subscriptions work as they do in the terminal. Trek never reads or stores those credentials.

<table>
  <tr><th colspan="7" align="left">Agents</th></tr>
  <tr>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/claude-code.png"><img src="crates/trek-assets/assets/logos/light/claude-code.png" width="36" height="36" alt=""></picture><br><sub>Claude Code</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/codex.png"><img src="crates/trek-assets/assets/logos/light/codex.png" width="36" height="36" alt=""></picture><br><sub>Codex</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/opencode.png"><img src="crates/trek-assets/assets/logos/light/opencode.png" width="36" height="36" alt=""></picture><br><sub>OpenCode</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/cursor.png"><img src="crates/trek-assets/assets/logos/light/cursor.png" width="36" height="36" alt=""></picture><br><sub>Cursor</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/github-copilot.png"><img src="crates/trek-assets/assets/logos/light/github-copilot.png" width="36" height="36" alt=""></picture><br><sub>GitHub Copilot</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/droid.png"><img src="crates/trek-assets/assets/logos/light/droid.png" width="36" height="36" alt=""></picture><br><sub>Factory Droid</sub></td>
    <td align="center" width="14%"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/kimi.png"><img src="crates/trek-assets/assets/logos/light/kimi.png" width="36" height="36" alt=""></picture><br><sub>Kimi</sub></td>
  </tr>
  <tr>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/qwen-code.png"><img src="crates/trek-assets/assets/logos/light/qwen-code.png" width="36" height="36" alt=""></picture><br><sub>Qwen Code</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/grok.png"><img src="crates/trek-assets/assets/logos/light/grok.png" width="36" height="36" alt=""></picture><br><sub>Grok</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/devin.png"><img src="crates/trek-assets/assets/logos/light/devin.png" width="36" height="36" alt=""></picture><br><sub>Devin</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/goose.png"><img src="crates/trek-assets/assets/logos/light/goose.png" width="36" height="36" alt=""></picture><br><sub>Goose</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/amp.png"><img src="crates/trek-assets/assets/logos/light/amp.png" width="36" height="36" alt=""></picture><br><sub>Amp</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/pi.png"><img src="crates/trek-assets/assets/logos/light/pi.png" width="36" height="36" alt=""></picture><br><sub>Pi</sub></td>
    <td align="center"><sub>+ any agent that speaks <a href="https://agentclientprotocol.com">ACP</a></sub></td>
  </tr>
  <tr><th colspan="7" align="left">API keys and local models</th></tr>
  <tr>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/anthropic.png"><img src="crates/trek-assets/assets/logos/light/anthropic.png" width="36" height="36" alt=""></picture><br><sub>Anthropic</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/openai.png"><img src="crates/trek-assets/assets/logos/light/openai.png" width="36" height="36" alt=""></picture><br><sub>OpenAI</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/gemini.png"><img src="crates/trek-assets/assets/logos/light/gemini.png" width="36" height="36" alt=""></picture><br><sub>Gemini</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/openrouter.png"><img src="crates/trek-assets/assets/logos/light/openrouter.png" width="36" height="36" alt=""></picture><br><sub>OpenRouter</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/deepseek.png"><img src="crates/trek-assets/assets/logos/light/deepseek.png" width="36" height="36" alt=""></picture><br><sub>DeepSeek</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/xai.png"><img src="crates/trek-assets/assets/logos/light/xai.png" width="36" height="36" alt=""></picture><br><sub>xAI</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/mistral.png"><img src="crates/trek-assets/assets/logos/light/mistral.png" width="36" height="36" alt=""></picture><br><sub>Mistral</sub></td>
  </tr>
  <tr>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/groq.png"><img src="crates/trek-assets/assets/logos/light/groq.png" width="36" height="36" alt=""></picture><br><sub>Groq</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/ollama.png"><img src="crates/trek-assets/assets/logos/light/ollama.png" width="36" height="36" alt=""></picture><br><sub>Ollama</sub></td>
    <td align="center"><picture><source media="(prefers-color-scheme: dark)" srcset="crates/trek-assets/assets/logos/dark/lmstudio.png"><img src="crates/trek-assets/assets/logos/light/lmstudio.png" width="36" height="36" alt=""></picture><br><sub>LM Studio</sub></td>
    <td align="center"><sub>llama.cpp<br>MLX</sub></td>
    <td align="center" colspan="3"><sub>+ any OpenAI-compatible endpoint</sub></td>
  </tr>
</table>

<sub>API and local models get Trek's own tools (read, edit, write, run) behind the same approval gates.</sub>

## Built with

<table>
  <tr>
    <td align="center" width="96"><img src="docs/assets/icons/tech/rust.svg" width="44" height="44" alt=""><br><sub>Rust</sub></td>
    <td align="center" width="96"><img src="docs/assets/icons/tech/gpui.svg" width="44" height="44" alt=""><br><sub>GPUI</sub></td>
    <td align="center" width="96"><img src="docs/assets/icons/tech/tokio.svg" width="44" height="44" alt=""><br><sub>Tokio</sub></td>
    <td align="center" width="96"><img src="docs/assets/icons/tech/sqlite.svg" width="44" height="44" alt=""><br><sub>SQLite</sub></td>
    <td align="center" width="96"><img src="docs/assets/icons/tech/swift.svg" width="44" height="44" alt=""><br><sub>SwiftUI</sub></td>
  </tr>
</table>

## Install

**macOS** (13 or later, Apple silicon)

1. Download the `.dmg` from [the latest release](https://github.com/dokyit/Trek/releases/latest) and drag **Trek** to **Applications**.
2. Releases are signed but not notarized yet. On first launch, open **System Settings › Privacy & Security** and click **Open Anyway**. Or run:
   ```sh
   xattr -dr com.apple.quarantine /Applications/Trek.app
   ```
3. Install and log in to the agents you use (`claude`, `codex`, `opencode`, …). Trek finds them on your `PATH`.

**Windows** (11, x64)

1. Run [`Trek-Setup-x86_64.exe`](https://github.com/dokyit/Trek/releases/latest/download/Trek-Setup-x86_64.exe), or unpack the portable `.zip` from the [release page](https://github.com/dokyit/Trek/releases/latest) into a folder of its own.
2. Not Authenticode-signed yet: SmartScreen asks once. Click **More info › Run anyway**.

<details>
<summary>How Windows differs from the Mac</summary>
<br>

- A tray icon and taskbar badge instead of the menu bar icon and Dock badge.
- No menu bar: every command is in the palette (Ctrl+K), Settings or a context menu. Shortcuts use Ctrl where the Mac uses ⌘.
- Dictation is Windows' own (Win+H). Snapshots go through Snipping Tool.
- No iOS Simulator tool.
- The first time the phone server is on, allow Trek through Windows Firewall on private networks.
- Windows 10 22H2 works on a best-effort basis, with an opaque window instead of Mica.
- Keep Trek out of folders that Controlled folder access guards (Documents, Desktop…), or updates can't install. Settings › Updates can move it.
- Logs: `%LOCALAPPDATA%\Trek\logs`.

</details>

**iPhone**: build it from source in Xcode ([ios/README.md](ios/README.md)), then pair it in **Settings › Phone & Machines**.

**Updates**: Trek updates itself from **Settings › Updates** (Stable, Beta or Nightly). Each update is checked against a minisign signature and installed when you restart Trek, never while an agent is working.

## Build from source

macOS needs full Xcode (GPUI compiles Metal shaders); Windows needs Visual Studio 2022 Build Tools (C++) and the Windows SDK. Rust comes from [`rust-toolchain.toml`](rust-toolchain.toml).

```sh
git clone https://github.com/dokyit/Trek && cd Trek
cargo run -p trek-app        # debug build
cargo test --workspace       # what CI runs
```

On Xcode 26+, a missing Metal compiler is one command away: `xcodebuild -downloadComponent MetalToolchain`. App bundles, the mock agent and environment variables: [CONTRIBUTING.md](CONTRIBUTING.md).

## Docs

| Doc | What's in it |
| --- | --- |
| [Design decisions](docs/DECISIONS.md) | Why Trek is built the way it is |
| [iPhone design and protocol](docs/MOBILE.md) | Pairing, TLS pinning, the wire protocol |
| [Other machines](docs/MULTI-MACHINE.md) | Running a thread on another computer |
| [Performance](docs/PERF.md) | How Trek is measured, and the numbers |
| [Releasing](docs/RELEASING.md) | Channels, signing and the update path |
| [All docs](docs/README.md) | The full index |

## Status

Trek is **0.x**: used every day, moving fast. Expect rough edges.
Apple silicon and Windows x64 only (no Linux yet). History import covers Claude Code, Codex and OpenCode.
See [open issues](https://github.com/dokyit/Trek/issues) and [what's next](docs/NEXT-WAVE.md).

## Contributing

Bug reports, ideas and pull requests are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).
Found a security issue? Please report it privately: [SECURITY.md](SECURITY.md).

## License

[Apache-2.0](LICENSE). Third-party notices are in [NOTICE](NOTICE).

<sub>Trek isn't affiliated with or endorsed by Anthropic, OpenAI, Google, Microsoft or any other maker of the agents it runs. Their names and logos belong to their owners and are shown only to say which agent a thread uses. To have a mark changed or removed, open an issue.</sub>
