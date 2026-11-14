# Trek docs

Start with the [README](../README.md) for what Trek is and how to install it.

## Using and building Trek

| Doc | What's in it |
| --- | --- |
| [CONTRIBUTING](../CONTRIBUTING.md) | Setting up, the mock agent, the verify driver, tests, commit style |
| [iPhone app](../ios/README.md) | Building Trek for iPhone, demo mode, pairing with a Mac |
| [Capture manifests](../media/README.md) | Making the screenshots and clips in these docs, deterministically |

## Design notes

| Doc | What's in it |
| --- | --- |
| [Decisions](DECISIONS.md) | Why Trek is built the way it is: stack, agents, permissions, import, updater, search, limits, sub-agents |
| [Cross-platform](CROSS_PLATFORM.md) | How the Mac and Windows builds stay one app: the architecture, the parity ledger and its checks, and what to do when changing Trek |
| [Parity](PARITY.md) | Every feature's status on each platform, generated from [`parity/ledger.toml`](parity/ledger.toml) |
| [The Windows sweep](parity/windows-sweep.md) | What the November 2026 sweep of the Windows build found, what was done about each finding, and what holds it |
| [Mobile](MOBILE.md) | The iPhone companion: architecture, pairing, TLS pinning, security model, wire protocol |
| [iPhone Mirroring](IPHONE-MIRRORING.md) | How macOS's iPhone Mirroring works end to end, what it needs, how Trek drives it, and how to read why it fails |
| [Other machines](MULTI-MACHINE.md) | Starting and following a thread on another computer |
| [Performance](PERFORMANCE.md) | Launch, memory and CPU on a Mac, measured, with the command to run it again |
| [Performance on Windows](PERF.md) | Frame times and long streams on Windows: the numbers, and what changed |
| [Prompt budget](PROMPT_BUDGET.md) | What Trek adds to every request of a vendor agent, measured, and the tests that bound it |
| [Next wave](NEXT-WAVE.md) | Work planned next: voice, media, a video watcher, generative UI |

## Maintainers

| Doc | What's in it |
| --- | --- |
| [Releasing](RELEASING.md) | Channels, signing keys, the Release workflow, how an update installs |
| [Security policy](../SECURITY.md) | Reporting vulnerabilities, supported versions |

## Images

README images live in [`assets/`](assets):

- `screenshots/mac/`: from `SIZE=1280x800 script/capture.sh mac/readme` (an isolated build with
  only the mock agent), resized to 2048 or 1200 px wide.
- `screenshots/iphone/`: the demo mode, shot with `ios/scripts/shoot.sh` and framed.
- `screenshots/windows/`: from `pwsh script/capture.ps1 windows/readme` (SIZE=1280x860, two demo
  projects, the scripted agent in its demo dress), resized to 1200 px wide; captioned as an example.
- `icons/features/`: [Lucide](https://lucide.dev) icons (ISC, see the `LICENSE` there) in Trek's ember.
- `icons/tech/`, `icons/platforms/`: tiles drawn around [Simple Icons](https://simpleicons.org) glyphs (CC0).
- `buttons/`, `trek-icon.png`: rendered from `assets/brand/` with Trek's own Inter.

Agent and provider logos in the README are the app's own, from
[`crates/trek-assets/assets/logos`](../crates/trek-assets/assets/logos).
