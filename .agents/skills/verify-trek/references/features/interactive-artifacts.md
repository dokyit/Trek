# Interactive artifacts

## What it does

Lets an agent answer with something live: a fenced ```` ```trek-html ```` block holding a complete, self-contained HTML document (a 3D model to turn, a simulation, an explorable diagram, a tabbed report). Once the fence closes, Trek runs it in a card inline in the answer: a header with its title (its `<title>`, else its first heading), where it loads from, and Reload, Copy source and Expand; under it the page in a web view as wide as the answer and as tall as the document reports (160–720 px; the page scrolls inside past that). While the agent is still writing the block, the card says "Building…" and nothing runs. Expand opens it large over the window (Esc, the page's Esc or the close button puts it away).

The page is themed: Trek puts its colours, fonts, text size and radius first in the document as CSS variables (`--trek-bg`, `--trek-fg`, `--trek-muted`, `--trek-accent`, `--trek-border`, `--trek-surface`, `--trek-positive|warning|negative|info`, `--trek-font`, `--trek-mono`, `--trek-font-size`, `--trek-radius`) with `color-scheme` and a transparent page, and swaps them in place when the theme changes.

Sandbox: the document is agent-written code. It's served from an origin of its own (`trek-artifact://a<hash>/`) under a CSP that allows scripts, styles, images, fonts, media and `fetch` over **https only** (so CDNs like jsdelivr and unpkg work: that's what makes it useful) and nothing over http, from `file:` or the machine; no forms, plugins or `<base>`. The frame can't navigate away from itself; a link the reader clicks opens in their browser (http, https and mailto only, one a second). New windows and downloads are refused. Website data isn't kept (a non-persistent store per view on macOS, InPrivate on Windows). No devtools outside debug builds. The page's only channel to Trek is the bootstrap script's messages (its height, that it took or gave back the keys, a link to open), tagged with a per-view secret the page's own scripts can't read.

Clicking into the page gives it the keyboard: its editing keys (⌘C, ⌘V, ⌘A, arrows, Esc…) are its own, while Trek's shortcuts (⌘K, ⌘W, ⌘N) still work and take the keys back; an Esc the page doesn't use, or a click anywhere else in Trek, gives them back too. A wheel the page has no use for (nothing in it scrolls that way) scrolls the answer around it.

Settings › General › Interactive artifacts turns it off: agents aren't told about `trek-html`, and blocks show as code. The phone shows such a block as a card, "Interactive — open on your Mac".

## Reach it as a user

Ask an agent for something you'd want to turn, play with or click through. The agent is told the format automatically (next to the `trek-viz` note) and prefers `trek-viz` for plain charts.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open draft
./.agents/skills/verify-trek/scripts/trek-dev send "mock:interactive"
./.agents/skills/verify-trek/scripts/trek-dev control artifacts           # artifacts.json: each card, live or not, its frame
./.agents/skills/verify-trek/scripts/trek-dev control scroll transcript -300
./.agents/skills/verify-trek/scripts/trek-dev control theme paper
```

`mock:interactive` answers with two artifacts that need no network: a tabbed report (a milestone table, an SVG node diagram with hover details, coloured risk cards) and a 3D desk with a monitor clamp in inline WebGL (drag to turn, scroll to zoom, Front / Close-up of clamp / From underneath, a legend). `mock:interactive stream` writes it slowly, to watch the "Building…" card.

The pages are native views, so `screenshot` (the `shots` frame) shows the card without them. To see the real thing on macOS, capture the window's region with `screencapture -R` (Screen Recording is granted to the terminal): `artifacts.json` has the window's frame on screen. Bring the dev window forward by its process (never by sending keys to whatever is in front), and hand focus back afterwards.

Real pointer and keyboard input (drag the desk, a wheel over the report, ⌘K from inside the page) can't come from `control`, which dispatches to GPUI and never reaches the native view. Post it to the dev Trek's process (`CGEvent.postToPid` for keys; mouse events need the dev window in front and land where the pointer is), never to whatever app is frontmost.

## Code

Format, sandbox rules, theme stylesheet, bootstrap script and message checks: `crates/trek-core/src/artifact.rs`. The card, its state and the window layer that places the views: `crates/trek-app/src/artifact/mod.rs`; what of a card can be seen and what covers it: `artifact/surface.rs`; the WKWebView / WebView2 and its clipping container, snapshots: `artifact/native.rs`. Markdown hook: `md.rs`; the layers in `root.rs` and `thread_window.rs`; the setting in `settings_view/pages.rs`; instructions in `workspace.rs` (`session_notes`); the mock in `crates/trek-agents/src/mock.rs` and `mock_artifacts/`; the phone card in `ios/Trek/Views/MarkdownText.swift`.

## Verify

Both mock artifacts run: the report's tabs switch, the 3D desk turns when dragged and the view buttons move the camera. The cards are as tall as their pages and follow the answer column's width. Scrolled so a card is half under the composer (or the transcript's top), the page is cut exactly at the transcript's edge and moves with the scroll; it never draws over the header, composer, sidebar or tools panel. A menu, popover, dialog, toast or the command palette over a card draws on top of it (the card shows its snapshot meanwhile). Another thread hides it; coming back shows it again. Night and Paper both blend; switching theme restyles the running page. `logs --grep error` is empty. An unfinished block in a finished answer shows as code under a note.
