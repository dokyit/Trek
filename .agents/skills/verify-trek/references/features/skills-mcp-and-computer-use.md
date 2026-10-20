# Skills, MCP, and computer use

## What it does

Discovers agent skills across supported homes, lets users enable/disable them, and configures Trek’s built-in computer/simulator/delegation tools plus additional MCP servers. The composer exposes matching `$` skill mentions.

On the Mac, computer use works **in the background** by default (Settings → Tools & MCP → Computer use → Work in the background): the agent picks one window (`open_app` opens an app behind the user's windows; `screenshot` with a `window_id` picks an open one), sees just that window even when covered, and its clicks, keys and scrolls go to that window's app alone. The user's pointer, keyboard and front app are untouched. `trek-mcp computer` (with `TREK_COMPUTER_MODE=background`) reports each step to Trek over the session's IPC channel (`computer_activity`), and Trek shows it in a floating picture-in-picture window: the agent's window live, where it last pointed, what it's doing and for which thread. The picture folds to a pill or is put away, and comes back from the menu bar icon (Show Background Use), the command palette, or the thread's working bar ("… in the background · Show"). It closes a few seconds after the turn ends.

## Reach it as a user

Open **Settings → Skills** to scan/filter/toggle skills. Open **Settings → Tools & MCP** to inspect built-ins and add or remove MCP server commands. Type `$` in a composer to choose an enabled skill.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:skills
./.agents/skills/verify-trek/scripts/trek-dev screenshot skills
./.agents/skills/verify-trek/scripts/trek-dev open settings:tools-&-mcp
```

The isolated profile may discover project skills but does not launch a real agent to consume them. Adding/removing MCP configuration is intentionally not automated by this verification CLI.

Background use's picture can be checked with the mock, which reports steps the way `trek-mcp computer` does (nothing is clicked for real). Give it a real window's id and size in points (any app's, from `CGWindowListCopyWindowInfo`) so the picture has something live to show, and don't wait for the turn, or the picture will have closed:

```sh
./.agents/skills/verify-trek/scripts/trek-dev send --no-wait "mock:background <window id> 800x600 Safari"
./.agents/skills/verify-trek/scripts/trek-dev control background-use fold     # or unfold, hide, show, toggle, next
```

The picture is a window of its own (layer 101), so `trek-dev screenshot` doesn't include it: capture it with `screencapture -l <its window id>`. The working bar's chip does show in `trek-dev screenshot`. To check `trek-mcp computer` itself in the background, run it over stdio with `TREK_COMPUTER_MODE=background` against a throwaway window (a TextEdit file, or Chrome with a `--user-data-dir` in /tmp) and confirm the front app and the pointer never change.

## Code

`crates/trek-app/src/settings_view.rs`, `settings_view/pages.rs`; skill discovery in `crates/trek-core/src/skills.rs`; built-in tools in `crates/trek-mcp/src/` (background input in `desktop/skylight.rs`, the window-by-window tools in `computer.rs`); the picture-in-picture window in `crates/trek-app/src/background_use.rs`.

## Verify

This `verify-trek` skill appears with its description, toggles persist, `$` results match enabled skills, MCP rows report actionable startup failures, and removal affects only the selected server. Background use: the picture opens at the screen's bottom right without taking focus, follows each step (its caption and the pointer), folds and unfolds, is put away and brought back, and closes after the turn; the working bar's chip says where the agent works.

