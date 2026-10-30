# Browser

## What it does

Embeds a web browser beside the thread with navigation, zoom, element picking, page screenshots sent to the composer, and developer tools.

Agents use it too (**browser use**): every session gets Trek's `trek-browser` MCP server (`trek-mcp browser`, Settings › Tools & MCP › Browser › Browser tools, on by default), whose tools drive this tool's real page while the user watches. It is Trek's browser-use option for every agent that takes MCP servers (Claude Code, Codex, ACP agents, OpenCode, the mock); there is no other backend to choose. The calls travel over the session's Trek channel (as the sub-agent tools' do) to the main window's Browser, which runs them one at a time:

| Tool | Arguments | Notes |
|---|---|---|
| `browser_open` | `url`, `new_tab?` | waits for the load (20 s); a blank new tab takes the page |
| `browser_tabs` / `browser_switch` / `browser_close` | `tab` (id) | |
| `browser_back` / `browser_forward` / `browser_reload` | | |
| `browser_snapshot` | `interactive_only?` | an accessibility outline (role, name, value, state, `-> href`) with stable refs (`[ref=e12]`, kept as `data-trek-ref`), through open shadow roots and same-origin frames, plus title, URL and scroll |
| `browser_click` / `browser_hover` | `ref` | pointer and mouse events, then `element.click()`; reports a navigation or a new tab |
| `browser_type` | `ref`, `text`, `submit?`, `clear?` | focus, key events per character, text through `execCommand('insertText')` (falls back to the prototype's value setter plus an input event), so React/Vue state sees it |
| `browser_press` | `key`, `ref?` | `Enter`, `Shift+Tab`, `Control+A`, `PageDown`…, with the browser's default actions (Enter submits by the default button, Tab moves focus) |
| `browser_select` | `ref`, `option` | a `<select>` by label or value; an ARIA combobox is opened and its option clicked |
| `browser_scroll` | `direction?`, `amount?`, `ref?` | the window, or the biggest box that scrolls when the window doesn't |
| `browser_wait` | `text` / `selector` / `ms`, `gone?`, `timeout_ms?` | polls, at most 60 s |
| `browser_screenshot` | `full_page?` | an image; a full page is stitched from screens (12 at most). Needs the Browser on screen |
| `browser_text` | | the main content's text |
| `browser_eval` | `js` | an expression or statements that return; promises awaited |

While an agent works the panel shows a strip, "Agent is using the browser · <thread> · <last action>", with **Stop**: the user takes the browser back, the call under way fails at its next step and that turn's later calls fail with "The user took over the browser…" (its next turn may use it again; **Hand back** ends it at once). The element each action touches is outlined in the page for a moment ("Agent · Click"). With Settings › Tools & MCP › Browser › **Show the browser live** on, the Browser comes forward when the agent of the thread in view starts using it (at most once in five minutes, like the Simulator's); off, a Browser tab is made behind the one in front.

Approvals follow the thread's access level and use the thread's own approval card (Deny / Always allow / Allow): typing into a password field and `browser_eval` ask unless the thread has Full access; submitting a form to another origin and downloads (`<a download>`) ask in Supervised and Auto-accept edits. Opening and reading pages never ask. Claude Code and Codex aren't asked by their own permission prompts for these tools: Trek asks where it should.

**Ad blocker** (the user's own uBlock Origin; nothing of it ships with Trek): ⋯ › **Import uBlock Origin…** or Settings › Tools & MCP › Browser › Ad blocker takes its folder (macOS also a .zip/.crx). On macOS its declarativeNetRequest rulesets (and its bundled lists' site-specific `##` element hiding) become WebKit content rules (`browser_adblock/convert.rs`), kept in `<data>/adblock/rules.json` and compiled once into `<data>/adblock/compiled` (a list is looked up by a hash of its JSON: a restart or toggle finds it in ~1 ms; an import or an allowed site compiles, 5–20 s for uBlock Origin 1.75's 35,864 rules). The menu then has **Block ads (uBlock Origin)** (on/off) and **Allow ads on <site>** (shows at once: the tab goes without the lists and reloads while they recompile with the site let through); Settings shows "35,864 rules → N active, M skipped (redirects as blocks)", **Re-import** and **Remove**. On Windows the folder is copied beside WebView2's extensions and installed as a Manifest V2 extension (Edge runs MV2 until its 2027 phase-out), then managed on the Extensions page.

**Extensions** (⋯ › **Extensions…**, Settings › Tools & MCP › Browser › **Extensions…**, or the address `trek://extensions`): one page on both systems (`panels/browser_extensions.rs`), in the page's place. Its search takes a name (filters the starter list; Enter on a single match installs it; a "Search the Chrome Web Store" row opens the store in a tab), a Chrome Web Store or Edge Add-ons link, or a bare id; **Folder…** takes an unpacked extension (run where it is, Chromium's id from its path or manifest `key`), **File…** a `.zip`/`.crx`. The page opens with the store link of the page in front filled in. A download (the store's CRX endpoint, `clients2.google.com/service/update2/crx`, CRX header stripped, `_metadata` dropped) or package waits unpacked in `<extensions>/.incoming/<id>` behind a card: "Install <name>?", what it will be able to do in words (`manifest::describe`), the starter list's verdict, **Cancel** / **Install**. The starter list (`browser_extensions/catalog.rs`: uBlock Origin Lite, Dark Reader, Grammarly, Bitwarden, 1Password, Vimium, Privacy Badger, React Developer Tools) says per extension "Works here", "Partly works" or "Doesn't run here" and why (measured under WebKit: a background stops at the first API WebKit lacks). Installed rows: icon, name, version or the first problem WebKit reported (red), Options (macOS), Permissions (what it can do and every error), on/off, Remove.

- macOS 15.4+ (`browser_extensions/webkit.rs`): WebKit's `WKWebExtensionController`, one for the app, persistent (a test profile gets its own storage by a UUID from its data folder), in every Browser view's configuration (`build_view` hands wry a configuration: `with_webview_configuration`). What's installed is `<data>/browser-extensions/extensions.json` (`registry.rs`: id, own folder or not, on, permissions allowed later); each loads with a stable `uniqueIdentifier`/`webkit-extension://<id>/` and every permission and site it was installed with granted. The Browser's tabs are WebKit tabs (`sync_tabs` from `sync_views`; address, title and loading changes as the Browser draws), so `tabs.query`, `tabs.create` (a Browser tab; an extension page opens in a window of its own), `windows.*` work. Toolbar buttons sit before ⋯ (`browser-extension-action#N`, in install order, two at most; more are behind a puzzle button, `browser-extensions-more`, that opens the page, where each installed row's icon, `browser-extension-row-action#N`, is its button too): the manifest icon, the action's badge; a click opens WebKit's popover under the button (in Trek's appearance). Turning one off and on again (or installing it again) waits two seconds after the unload: sooner, WebKit's old background still holds the address and the new one never answers. Options pages open in a window of their own (they need the extension's own configuration). A runtime `permissions.request` shows an amber strip over the page: "<name> wants to: …" **Don't allow** / **Allow** (allowed ones are kept). Older macOS: the page says extensions need 15.4.
- Windows: WebView2's profile extensions as before (no toolbar buttons or popups); the same page and prompt.

## Reach it as a user

Press ⌘J, choose **Browser**, enter a URL or search in the address field, and use the header controls for back/forward/reload, picker, screenshot, zoom, or devtools.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:browser
./.agents/skills/verify-trek/scripts/trek-dev screenshot browser-panel
```

`control browser go <address>|back|forward|reload|state|snap <name>` drives the tool without a pointer; `control browser adblock-import <folder>`, `adblock on|off` and `adblock-allow` (macOS: the site in front) drive the ad blocker without its file picker (the driver's fresh settings start with it off; a compiled list stays in the profile's data folder): `state` writes `browser.json` (the page, the panel's area and the native view's rectangle), `snap` writes the page as WebKit (or WebView2) draws it to `<name>.png` in the shot folder, even with the window behind others. The GPUI screenshot leaves the native page blank; lay the snap over it at `area` to see both. `control click browser-scheme` cycles the page scheme (Auto, Light, Dark). `control browser focus|unfocus` gives the page the keys (as a click in it does) or takes them back; `control browser eval <js>` writes the page's answer, as JSON, to `browser-eval.json`; `control browser press cmd-v` (macOS) sends a real key event through AppKit (Trek comes to the front for it: a background app has no key window), so editing keys can be checked end to end. The clipboard is the Mac's own: save and restore it around such a check. Avoid shared authenticated sites during verification.

Extensions without the file picker or the Install card (the driver stands in for the user's Install): `control browser ext-page` opens the Extensions page; `ext-install <folder | .zip/.crx | store link or id>` installs (a store id downloads); `ext-list` writes `browser-extensions.json` (id, name, version, on, options, permissions in words, WebKit's problems, the toolbar action's label/badge); `ext-on|ext-off|ext-remove <id or name>`; `ext-allow|ext-deny` answers the first waiting permission strip; `ext-options <id or name>` (macOS) opens its options window; `ext-popup <js>` (macOS) runs JavaScript in the popup last shown and writes the answer to `browser-popup.txt` (an occluded window's popover may not paint, so read it rather than screenshot it, or bring Trek forward with `browser focus` first). A toolbar button is `control click browser-extension-action#N` (its popover is a native window: see it with `screencapture`, or read it through the extension's own page). A test extension in a temp folder (an MV3 content script that adds an element, a popup, a `declarativeNetRequest` block rule) checks the three: `browser eval` finds the element, a request the rule blocks never loads, the popup shows under the button.

```sh
./.agents/skills/verify-trek/scripts/trek-dev control browser ext-install /tmp/test-extension
./.agents/skills/verify-trek/scripts/trek-dev control browser ext-install ddkjiahejlhfcafbddmgiahcphecmpfh   # uBlock Origin Lite
./.agents/skills/verify-trek/scripts/trek-dev control browser ext-list
./.agents/skills/verify-trek/scripts/trek-dev control browser go http://127.0.0.1:8765/
./.agents/skills/verify-trek/scripts/trek-dev control browser eval "document.getElementById('trek-ext-marker') !== null"
./.agents/skills/verify-trek/scripts/trek-dev control click browser-extension-action#0
```

Browser use, live (the Mac test of it): the mock serves a test site of its own and works through it with the real tools, checking each step, and ends with "Browser check: all 27 steps passed." or what failed.

```sh
./.agents/skills/verify-trek/scripts/trek-dev project /tmp/some-folder
./.agents/skills/verify-trek/scripts/trek-dev send "mock:browser" --timeout 240
./.agents/skills/verify-trek/scripts/trek-dev send --no-wait "mock:browser"   # then, mid-run:
./.agents/skills/verify-trek/scripts/trek-dev control browser snap mid        # the outlined element
./.agents/skills/verify-trek/scripts/trek-dev control click browser-agent-stop  # take over: later steps fail "took over"
./.agents/skills/verify-trek/scripts/trek-dev send --no-wait "mock:browser approvals"
./.agents/skills/verify-trek/scripts/trek-dev control click allow-session     # the password; then `allow` or `deny` for the script
```

`mock:browser <url>` opens that page and reads it instead. The tally is the thread's last answer (also in `data/trek.sqlite`, table `items`).

## Code

`crates/trek-app/src/panels/browser.rs`, `panels/browser_extensions.rs` (the Extensions page, the backends, toolbar buttons and permission strips, the macOS tracker blocker) with `browser_extensions/{catalog,manifest,registry,webkit}.rs` (starter list, manifest reading and ids, the installed list, WebKit's controller, tabs, popups and prompts), `panels/browser_adblock.rs` + `browser_adblock/convert.rs` (the imported ad blocker), `panels/mod.rs`; the Edit keys in a focused web view in `vendor/gpui-pre-macos/src/window.rs` ("Trek:"); webview dependencies in `crates/trek-app/Cargo.toml`. Browser use: the page side `panels/browser/agent.js`, what needs no web view (scripts, the outline, keys, approval rules) `panels/browser/agent_core.rs`, the driver and the strip `panels/browser/agent.rs`, routing and approvals `workspace/browser_use.rs`, the tools `crates/trek-ipc/src/browser.rs` and `crates/trek-mcp/src/browser.rs`, the mock's check `crates/trek-agents/src/mock/browser.rs`.

## Verify

Navigation and history update the address/title, picker identifies the intended element, page capture reaches the composer, zoom persists per tab, and devtools opens without crashing the app. Pages lay out at the panel's width (Google's responsive results, not its fixed-width legacy page: the view sends Safari's user agent) and follow the page scheme: `prefers-color-scheme` pages switch at once, Google's search pages a few seconds later (reloaded). ⌘C/⌘X/⌘V/⌥⇧⌘V/⌘A/⌘Z/⇧⌘Z work in a focused page (the page sees each key first), Trek's other shortcuts still win, and the editing keys go to Trek's own fields when the page doesn't have the keys. Browser use: `mock:browser` passes every step, the strip shows during the run and goes about 12 s after, Stop fails the rest of the turn's calls, and `mock:browser approvals` raises the password and script cards (none with the thread on Full access).

