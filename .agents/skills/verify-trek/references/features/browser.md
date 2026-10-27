# Browser

## What it does

Embeds a web browser beside the thread with navigation, zoom, element picking, page screenshots sent to the composer, and developer tools.

## Reach it as a user

Press ⌘J, choose **Browser**, enter a URL or search in the address field, and use the header controls for back/forward/reload, picker, screenshot, zoom, or devtools.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open tool:browser
./.agents/skills/verify-trek/scripts/trek-dev screenshot browser-panel
```

`control browser go <address>|back|forward|reload|state|snap <name>` drives the tool without a pointer: `state` writes `browser.json` (the page, the panel's area and the native view's rectangle), `snap` writes the page as WebKit (or WebView2) draws it to `<name>.png` in the shot folder, even with the window behind others. The GPUI screenshot leaves the native page blank; lay the snap over it at `area` to see both. `control click browser-scheme` cycles the page scheme (Auto, Light, Dark). The shots protocol cannot interact with page content; avoid shared authenticated sites during verification.

## Code

`crates/trek-app/src/panels/browser.rs`, `panels/mod.rs`; webview dependencies in `crates/trek-app/Cargo.toml`.

## Verify

Navigation and history update the address/title, picker identifies the intended element, page capture reaches the composer, zoom persists per tab, and devtools opens without crashing the app. Pages lay out at the panel's width (Google's responsive results, not its fixed-width legacy page: the view sends Safari's user agent) and follow the page scheme: `prefers-color-scheme` pages switch at once.

