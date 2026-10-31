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
| [Mobile](MOBILE.md) | The iPhone companion: architecture, pairing, TLS pinning, security model, wire protocol |
| [Other machines](MULTI-MACHINE.md) | Starting and following a thread on another computer |
| [Performance](PERF.md) | How Trek is measured, the numbers, and what changed |
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
- `screenshots/windows/`: frames from a Windows build of the same suite.
- `icons/features/`: [Lucide](https://lucide.dev) icons (ISC, see the `LICENSE` there) in Trek's ember.
- `icons/tech/`, `icons/platforms/`: tiles drawn around [Simple Icons](https://simpleicons.org) glyphs (CC0).
- `buttons/`, `trek-icon.png`: rendered from `assets/brand/` with Trek's own Inter.

Agent and provider logos in the README are the app's own, from
[`crates/trek-assets/assets/logos`](../crates/trek-assets/assets/logos).
