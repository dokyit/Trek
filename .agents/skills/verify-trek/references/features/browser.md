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

## Reach it as a user

Press ⌘J, choose **Browser**, enter a URL or search in the address field, and use the header controls for back/forward/reload, picker, screenshot, zoom, or devtools.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:browser
./.agents/skills/verify-trek/scripts/trek-dev screenshot browser-panel
```

`control browser go <address>|back|forward|reload|state|snap <name>` drives the tool without a pointer; `control browser adblock-import <folder>`, `adblock on|off` and `adblock-allow` (macOS: the site in front) drive the ad blocker without its file picker (the driver's fresh settings start with it off; a compiled list stays in the profile's data folder): `state` writes `browser.json` (the page, the panel's area and the native view's rectangle), `snap` writes the page as WebKit (or WebView2) draws it to `<name>.png` in the shot folder, even with the window behind others. The GPUI screenshot leaves the native page blank; lay the snap over it at `area` to see both. `control click browser-scheme` cycles the page scheme (Auto, Light, Dark). `control browser focus|unfocus` gives the page the keys (as a click in it does) or takes them back; `control browser eval <js>` writes the page's answer, as JSON, to `browser-eval.json`; `control browser press cmd-v` (macOS) sends a real key event through AppKit (Trek comes to the front for it: a background app has no key window), so editing keys can be checked end to end. The clipboard is the Mac's own: save and restore it around such a check. Avoid shared authenticated sites during verification.

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

`crates/trek-app/src/panels/browser.rs`, `panels/browser_extensions.rs` (Windows extensions page, macOS tracker blocker), `panels/browser_adblock.rs` + `browser_adblock/convert.rs` (the imported ad blocker), `panels/mod.rs`; the Edit keys in a focused web view in `vendor/gpui-pre-macos/src/window.rs` ("Trek:"); webview dependencies in `crates/trek-app/Cargo.toml`. Browser use: the page side `panels/browser/agent.js`, what needs no web view (scripts, the outline, keys, approval rules) `panels/browser/agent_core.rs`, the driver and the strip `panels/browser/agent.rs`, routing and approvals `workspace/browser_use.rs`, the tools `crates/trek-ipc/src/browser.rs` and `crates/trek-mcp/src/browser.rs`, the mock's check `crates/trek-agents/src/mock/browser.rs`.

## Verify

Navigation and history update the address/title, picker identifies the intended element, page capture reaches the composer, zoom persists per tab, and devtools opens without crashing the app. Pages lay out at the panel's width (Google's responsive results, not its fixed-width legacy page: the view sends Safari's user agent) and follow the page scheme: `prefers-color-scheme` pages switch at once, Google's search pages a few seconds later (reloaded). ⌘C/⌘X/⌘V/⌥⇧⌘V/⌘A/⌘Z/⇧⌘Z work in a focused page (the page sees each key first), Trek's other shortcuts still win, and the editing keys go to Trek's own fields when the page doesn't have the keys. Browser use: `mock:browser` passes every step, the strip shows during the run and goes about 12 s after, Stop fails the rest of the turn's calls, and `mock:browser approvals` raises the password and script cards (none with the thread on Full access).

