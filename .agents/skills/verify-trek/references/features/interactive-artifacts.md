# Interactive artifacts

## What it does

Lets an agent answer with something live: a fenced ```` ```trek-html ```` block holding a complete, self-contained HTML document (a 3D model to turn, a simulation, an explorable diagram, a tabbed report). Once the fence closes, Trek runs it in a card inline in the answer: a header with its title (its `<title>`, else its first heading), where it loads from, and Reload, Copy source and Expand; under it the page in a web view as wide as the answer and as tall as the document reports (160–720 px; the page scrolls inside past that). While the agent is still writing the block, the card says "Building…" and nothing runs. Expand opens it large over the window (Esc, the page's Esc or the close button puts it away).

The page is themed: Trek puts its colours, fonts, text size and radius first in the document as CSS variables (`--trek-bg`, `--trek-fg`, `--trek-muted`, `--trek-accent`, `--trek-border`, `--trek-surface`, `--trek-positive|warning|negative|info`, `--trek-font`, `--trek-mono`, `--trek-font-size`, `--trek-radius`, `--trek-radius-control|surface|pill`) with `color-scheme` and a transparent page, and swaps them in place when the theme changes.

Sandbox: the document is agent-written code, and an agent can be talked into things by what it reads. So the page is treated as someone who may want to send what the agent has read somewhere, and a request to a host is a message to whoever can see that host's requests. It's served from an origin of its own (`trek-artifact://a<hash>/`) under a policy (`trek_core::artifact::net`) with three rules:

- **What loads without asking** is only what nobody but its operator can watch: cdnjs's reviewed library files (`cdnjs.cloudflare.com/ajax/libs/`), Google Fonts, the map card's tiles (`tiles.openfreemap.org`) and the exact library files that card pins on unpkg. Scripts, styles, images, fonts, media, `fetch` and WebSocket all reach these and nothing else. jsDelivr, unpkg and esm.sh are **not** in it although they are library CDNs: they serve whatever anyone publishes, and how often each file or version was asked for is public (jsDelivr's own counts, npm's download counts), which a page could spell a secret in. Nor is OpenStreetMap's tile server (it publishes which tiles were asked for).
- **Any other host is off until the user allows it.** The engine stops the load; the card then shows a strip under its header, drawn by Trek and outside the page: "This page wants to load from cdn.jsdelivr.net." **Allow once** (this page, until Trek quits) / **Always allow** (kept in Settings › General › Sites pages may load from, where each can be removed) / **Don't allow** (not asked again for this page). Allowed, the host joins the page's policy and the page loads again; meanwhile everything inline runs. Only public https sites by name can be asked about: never an IP address, `localhost` or a name on the local network. Nothing over http, from `file:` or the machine; no frames from anywhere (a frame the page writes itself, `srcdoc`, keeps the policy), no forms, plugins or `<base>`, no referrer. What no policy directive covers is closed in the engine: on macOS WebRTC and `<link rel="preconnect">` are switched off in the page's view by WebKit's own feature switches (`PeerConnectionEnabled`, `LinkPreconnect` and three more, `artifact/native.rs`; a WebKit without one is logged); on Linux WebRTC is off in WebKitGTK's settings and preconnect through its feature list (WebKitGTK 2.42 and later; an older one is logged and leaves preconnect to the page); on Windows the document is served with a `Connection-Allowlist` header naming the same sources, which WebView2 152 and later hold every connection to (preconnect, DNS prefetch, WebRTC's STUN and TURN). Trek's script also takes WebRTC's constructors off the page everywhere.
- **A link or a new window opens in the user's browser only on their own click, as the app saw it** (http, https and mailto, one a click, within a second of it): on macOS every mouse button and key AppKit hands the app is noted before it's dispatched, on Linux GTK's own signals on the view, on Windows WebView2's word on each request (`IsUserInitiated`). The page shares its script world with Trek's script, so nothing the page says counts as a click: no message opens a link. `window.open` or a script sending the frame away with no click does nothing. The frame can't navigate away from itself.

A video from the web (an https link to a video file on a line of its own) is a card that says where it is and waits: **Load video** / **Always from this site**. Nothing is asked of its site before that (the address is the agent's, and may carry anything). A site on the "Always allow" list loads at once.

A page that loads the bundled KaTeX (`/_trek/katex/…`, served by the app from `crates/trek-assets/assets/katex`: a Study question card does) gets the same policy with scripts, styles and fonts from its own origin and inline code only, and no `eval`. Downloads are refused. Website data isn't kept (a non-persistent store per view on macOS, InPrivate on Windows). No devtools outside debug builds. The page's only channel to Trek is the bootstrap script's messages (its height, that it took or gave back the keys, a load its policy stopped), tagged with a per-view secret the page's own scripts can't read.

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

`mock:outbound` answers with a page that tries every way a page has of reaching the network (41 of them: images, scripts, styles, fonts, `fetch`, WebSocket, frames, preconnect, forms, a script sending the frame away, WebRTC, workers…), each to a port of its own on this machine, loads one library from cdnjs and one from jsDelivr, and has two links and a `window.open` button at fixed places; then a video from the web. `mock:outbound popups` also asks for windows as it loads, with no click. See "What a page can reach" under Verify.

`mock:interactive` answers with two artifacts that need no network: a tabbed report (a milestone table, an SVG node diagram with hover details, coloured risk cards) and a 3D desk with a monitor clamp in inline WebGL (drag to turn, scroll to zoom, Front / Close-up of clamp / From underneath, a legend). `mock:interactive stream` writes it slowly, to watch the "Building…" card.

The pages are native views, so `screenshot` (the `shots` frame) shows the card without them. To see the real thing on macOS, capture the window's region with `screencapture -R` (Screen Recording is granted to the terminal): `artifacts.json` has the window's frame on screen. Bring the dev window forward by its process (never by sending keys to whatever is in front), and hand focus back afterwards.

`control artifacts click-page <x> <y>` (macOS) clicks at that point of the first live card's page, in CSS px from its top left, as the system delivers a click (an AppKit event through `sendEvent:`), so it counts as the user's own: for links. `artifacts.json` then has `opened` (the links that would have opened in the browser: an isolated run opens none) and `asking` (the hosts cards are asking about).

Real pointer and keyboard input (drag the desk, a wheel over the report, ⌘K from inside the page) can't come from `control`, which dispatches to GPUI and never reaches the native view. Post it to the dev Trek's process (`CGEvent.postToPid` for keys; mouse events need the dev window in front and land where the pointer is), never to whatever app is frontmost.

## Code

Format, sandbox rules, theme stylesheet, bootstrap script and message checks: `crates/trek-core/src/artifact.rs`; what a page may reach, what the card asks about and when a link opens: `crates/trek-core/src/artifact/net.rs`. The card, its state and the window layer that places the views: `crates/trek-app/src/artifact/mod.rs`; what of a card can be seen and what covers it: `artifact/surface.rs`; the WKWebView / WebView2 and its clipping container, snapshots: `artifact/native.rs`. Markdown hook: `md.rs`; the layers in `root.rs` and `thread_window.rs`; the setting in `settings_view/pages.rs`; instructions in `workspace.rs` (`session_notes`); the mock in `crates/trek-agents/src/mock.rs` and `mock_artifacts/`; the phone card in `ios/Trek/Views/MarkdownText.swift`.

## Verify

Both mock artifacts run: the report's tabs switch, the 3D desk turns when dragged and the view buttons move the camera. The cards are as tall as their pages and follow the answer column's width. Scrolled so a card is half under the composer (or the transcript's top), the page is cut exactly at the transcript's edge and moves with the scroll; it never draws over the header, composer, sidebar or tools panel. A menu, popover, dialog, toast or the command palette over a card draws on top of it (the card shows its snapshot meanwhile). Another thread hides it; coming back shows it again. Night and Paper both blend; switching theme restyles the running page. `logs --grep error` is empty. An unfinished block in a finished answer shows as code under a note.

**What a page can reach.** With the app started and a thread open:

```sh
python3 crates/trek-app/src/artifact/fixtures/listen.py 25 > /tmp/knocked.json &   # hears ports 47700–47759
./.agents/skills/verify-trek/scripts/trek-dev send "mock:outbound popups"
```

Run it once more after the Browser tool has been used (`mock:browser hostile`): a WebSocket once got out only then (see browser.md, on `ws` and `wss`). The listener must print `{"knocked": {}}`: no way out reached its port, and the video's site (47759) wasn't asked. The page's own table says which the policy stopped (the rest never start: a navigation the app refused, WebRTC with no constructor). cdnjs says "loaded", jsDelivr "not loaded", and the strip asks about `cdn.jsdelivr.net` alone; **Allow once** reloads the page and jsDelivr says "loaded". `artifacts.json` has `opened: []` after the load (the windows asked for with no click did nothing); `control artifacts click-page 106 72` (the link), `click-page 296 72` (the link to a new window) and `click-page 486 72` (the `window.open` button) each add their address, one a click. The video card says where the video is and shows no player until **Load video**.
