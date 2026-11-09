# Appearance, notifications, and snapshots

## What it does

Provides Night and Paper themes, text sizing, background/glass/tint and motion preferences. Notifications can use banners, Dock badge, and menu-bar state when work finishes or needs a decision. Snapshots capture a window, area, or screen into the next message with configurable format and retention.

## Reach it as a user

Open **Settings → Appearance**, **Notifications**, or **App Snapshots**. The composer **+** menu and ⌘⇧S trigger snapshot capture.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:appearance
./.agents/skills/verify-trek/scripts/trek-dev control theme paper
./.agents/skills/verify-trek/scripts/trek-dev control glass on
./.agents/skills/verify-trek/scripts/trek-dev screenshot appearance-paper
./.agents/skills/verify-trek/scripts/trek-dev screenshot glass --real             # real glass, on the test display
./.agents/skills/verify-trek/scripts/trek-dev screenshot glass --real --display   # the whole test display
./.agents/skills/verify-trek/scripts/trek-dev open settings:notifications
./.agents/skills/verify-trek/scripts/trek-dev open settings:app-snapshots
```

CLI `screenshot` uses Trek’s offscreen shots renderer; it verifies window rendering without Screen Recording permission but does not exercise the macOS picker/attachment flow. Notifications are disabled in the isolated profile.

Glass and vibrancy are the window server's work, so the offscreen render only stands in for them (see-through areas over a blurred gradient). `screenshot --real` is the real thing: `screencapture` of the window's rectangle on the Trek Test Display (see the skill's "Test display"), where the glass frosts that display's wallpaper as it would the desktop and nothing of the owner's is in the picture or on their screen. Compare it with the plain `screenshot` of the same state: both are the window at 2x, pixel for pixel the same size. A capture of the window alone by its id (`screencapture -l`, what `--real` falls back to with `--no-test-display`) has nothing behind the glass, which then looks flat grey: don't judge glass from that one.

## Code

`crates/trek-app/src/settings_view/pages.rs`, `palette.rs`, `composer.rs`, `integrations.rs`, `tray.rs`; settings in `crates/trek-core/src/settings.rs`.

## Verify

Themes remain legible, tint/glass changes render, text scales without clipping, reduced motion is respected, notification policy maps to the right event, and snapshot output/retention match settings.

