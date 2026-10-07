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

The shots protocol cannot enter a URL or interact with page content. Use a local test page and the UI or browser-specific tests for navigation and picking; avoid shared authenticated sites during verification.

## Code

`crates/trek-app/src/panels/browser.rs`, `panels/mod.rs`; webview dependencies in `crates/trek-app/Cargo.toml`.

## Verify

Navigation and history update the address/title, picker identifies the intended element, page capture reaches the composer, zoom persists per tab, and devtools opens without crashing the app.

