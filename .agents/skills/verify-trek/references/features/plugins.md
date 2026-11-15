# Plugins

## What it does

A plugin is a folder with a `plugin.toml` in `<data>/plugins/<id>/` that changes how Trek looks and works: a look (`[[theme]]`), element trees Trek draws in its own places (`[[slot]]`: the composer's bar, + menu, banner, top edge and border, a thread's header, the sidebar's top and foot, over and under the new-thread composer, behind the chat, the title bar's right end), a surface of its own in place of the thread list, the composer or the new-thread screen (`[[replace]]`), pages and panels of its own HTML in a fenced web view (`[[page]]`, `[[panel]]`), palette commands with keys (`[[command]]`), words marked in the composer's draft (`[[effect]]`), and logic in `main.js` that hears Trek's events (`hooks`). `docs/PLUGINS.md` is the design; the built-in skill `trek-plugin` is what an agent follows to make one.

Trek reads the folder at launch and again within two seconds of a change. A plugin that asks for permissions waits for the user to allow them in Settings › Plugins (the grant is sealed with a key in the secret store, so one written into `state.toml` by hand is ignored, and a plugin whose files change asks again; an isolated run keeps that key in memory, so its grants last until it quits); what's wrong with one is shown there and written to `<data>/plugins/problems.txt`; each has a log in `<data>/plugin-data/<id>/log.txt`. Trek ships five examples (`plugins/examples/`): `focus-timer`, `commit-graph`, `summit-log`, `trail-board`, `terminal-look`.

## Reach it as a user

Settings › Plugins: installed plugins with their switch, permissions, log and Remove; "In place of Trek's own" picks a replacement; Examples installs one. The palette runs plugins' commands, opens their pages and panels, and turns them on and off. `trek plugin install <folder | git URL | example:<name>>` from a terminal.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:plugins
./.agents/skills/verify-trek/scripts/trek-dev control plugin install example:focus-timer   # or a folder's path
./.agents/skills/verify-trek/scripts/trek-dev control plugin on trail-board               # as Settings' switch: allows what it asks
./.agents/skills/verify-trek/scripts/trek-dev control plugin off trail-board
./.agents/skills/verify-trek/scripts/trek-dev control plugin running focus-timer          # a check
./.agents/skills/verify-trek/scripts/trek-dev control plugin problems 0                   # a check
./.agents/skills/verify-trek/scripts/trek-dev control plugin focus at 300                 # the focus timer, as if started 300 s ago
./.agents/skills/verify-trek/scripts/trek-dev control plugin focus stop
./.agents/skills/verify-trek/scripts/trek-dev control plugin page summit-log summit-log   # open a page
./.agents/skills/verify-trek/scripts/trek-dev control plugin page-view 1                  # a check: its web view exists (0: it doesn't)
./.agents/skills/verify-trek/scripts/trek-dev control plugin panel <plugin> <panel>       # open a panel beside the thread
./.agents/skills/verify-trek/scripts/trek-dev control plugin command <plugin> <command>   # run a command as the palette does
./.agents/skills/verify-trek/scripts/trek-dev control plugin state <plugin> key=value     # set its state; state-is checks it
./.agents/skills/verify-trek/scripts/trek-dev control plugin-keys hold                    # Mac: give the keyboard to the page's web view
./.agents/skills/verify-trek/scripts/trek-dev control plugin-keys                         # the composer has the keys; on a Mac real key-downs reach it
./.agents/skills/verify-trek/scripts/trek-dev send "mock:plugin"                          # the scripted agent writes a plugin (Trail notes)
```

The same `trek plugin …` CLI works against the isolated profile: run the capture build's own binary (`<runtime_dir>/bin/trek plugin list`) with `TREK_DATA_DIR=<data_dir>` from `status`.

## Elements to check

`plugin-row-<id>`, `plugin-waiting-<id>`, `plugin-problem-<id>`, `plugin-log-<id>` (Settings › Plugins); `focus-chip` (the timer's chip on the composer); `commit-trail` (under the new-thread composer); `plugin-surface-thread-list` (a replacement drawn in place of the thread list; `-composer`, `-new-thread`); `plugin-page` (a page's pane); `page-<plugin>-<page>` (its row in the sidebar's Pages group).

## Notes

- A web view's own pixels need `screenshot --real`, which needs the Trek Test Display: when other isolated apps hold every corner (`status` says `test_display.created: false`), the window counts as covered and WebKit paints nothing, though the page runs. `plugin page-view` and the page's own bridge still answer.
- Code: `crates/trek-core/src/plugins/` (format, folder, CLI, page sandbox), `crates/trek-app/src/plugins/` (registry, drawing, logic host, focus timer, commit trail, page host), `crates/trek-script/src/plugin.rs` (QuickJS), `crates/trek-app/src/settings_view/plugins.rs`.
- Tests: `cargo test -p trek-core --lib plugins`, `cargo test -p trek-script --lib plugin`, `cargo test -p trek-app --test plugins`.
