# Plugins

## What it does

A plugin is a folder with a `plugin.toml` in `<data>/plugins/<id>/` that changes how Trek looks and works: a look (`[[theme]]`), element trees Trek draws in its own places (`[[slot]]`: the composer's bar, + menu, banner, top edge and border, a thread's header, the sidebar's top and foot, over and under the new-thread composer, behind the chat, the title bar's right end), a surface of its own in place of the thread list, the composer or the new-thread screen (`[[replace]]`), pages and panels of its own HTML in a fenced web view (`[[page]]`, `[[panel]]`), palette commands with keys (`[[command]]`), words marked in the composer's draft (`[[effect]]`), and logic in `main.js` that hears Trek's events (`hooks`). `docs/PLUGINS.md` is the design; the built-in skill `trek-plugin` is what an agent follows to make one.

Trek reads the folder at launch and again within two seconds of a change. A plugin that asks for permissions waits for the user to allow them in Settings › Plugins (the grant is sealed with a key in the secret store, so one written into `state.toml` by hand is ignored, and a plugin whose files change asks again; an isolated run keeps that key in memory, so its grants last until it quits); what's wrong with one is shown there and written to `<data>/plugins/problems.txt`; each has a log in `<data>/plugin-data/<id>/log.txt`. Trek ships six examples (`plugins/examples/`), each made from the sentence at the top of its README: `campfire` (a look and text effects), `trailhead` (the new-thread screen, a page that sets state, `{now.part}`), `marmot` (a sidebar pet driven by hooks), `soft-sounds` (`trek.sound`, a panel), `now-playing` (the title bar, asks for `threads:read` and `threads:open`), `focus-timer` (Focus walk: the composer's edge, lengths, a break, kept across a restart). Plugin sounds are logged (`played sounds/chime.wav` in its log) and never played aloud in the isolated app.

## Reach it as a user

Settings › Plugins: first, "Make Trek yours: ask an agent" with three sentences (`plugin-ask-0`…`2`: each opens a new thread with it in the composer); installed plugins with their switch, permissions, log and Remove; "In place of Trek's own" picks a replacement; Examples installs one. The palette runs plugins' commands, opens their pages and panels, and turns them on and off. `trek plugin install <folder | git URL | example:<name>>` from a terminal.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev open settings:plugins
./.agents/skills/verify-trek/scripts/trek-dev control plugin install example:focus-timer   # or a folder's path
./.agents/skills/verify-trek/scripts/trek-dev control plugin on now-playing               # as Settings' switch: allows what it asks (and chooses a replacement)
./.agents/skills/verify-trek/scripts/trek-dev control plugin off now-playing
./.agents/skills/verify-trek/scripts/trek-dev control plugin running focus-timer          # a check
./.agents/skills/verify-trek/scripts/trek-dev control plugin problems 0                   # a check
./.agents/skills/verify-trek/scripts/trek-dev control plugin focus at 300                 # the focus timer, as if started 300 s ago
./.agents/skills/verify-trek/scripts/trek-dev control plugin focus stop
./.agents/skills/verify-trek/scripts/trek-dev control plugin page trailhead prompts       # open a page
./.agents/skills/verify-trek/scripts/trek-dev control plugin page-view 1                  # a check: its web view exists (0: it doesn't)
./.agents/skills/verify-trek/scripts/trek-dev control plugin panel <plugin> <panel>       # open a panel beside the thread
./.agents/skills/verify-trek/scripts/trek-dev control plugin command <plugin> <command>   # run a command as the palette does
./.agents/skills/verify-trek/scripts/trek-dev control plugin state <plugin> key=value     # set its state; state-is checks it
./.agents/skills/verify-trek/scripts/trek-dev control plugin-keys hold                    # Mac: give the keyboard to the page's web view
./.agents/skills/verify-trek/scripts/trek-dev control plugin-keys                         # the composer has the keys; on a Mac real key-downs reach it
./.agents/skills/verify-trek/scripts/trek-dev send "mock:plugin"                          # the scripted agent writes a plugin (Trail notes)
```

The same `trek plugin …` CLI works against the isolated profile: run the capture build's own binary (`<runtime_dir>/bin/trek plugin list`) with `TREK_DATA_DIR=<data_dir>` from `status`.

## What an agent makes shows up in the list

`send mock:plugin` (a Trail notes plugin) and `send mock:look` (a look called Dusk, as a plugin with a `[[theme]]`, turned on and asked for through `themes/apply`) are the scripted agents that write into the plugins folder as the `trek-plugin` and `trek-theme` skills say. Within about two seconds the plugin is listed, running, announced by a toast ("Trail notes was added … Manage it in Settings › Plugins", button Manage), and its row says when it was added (and by which agent when exactly one was at work). Checks and steps for it:

```sh
./.agents/skills/verify-trek/scripts/trek-dev control plugin listed dusk      # in Settings › Plugins, on or off
./.agents/skills/verify-trek/scripts/trek-dev control plugin stopped dusk     # listed and off
./.agents/skills/verify-trek/scripts/trek-dev control plugin delete dusk      # as Delete confirms: folder, what it kept, choices
./.agents/skills/verify-trek/scripts/trek-dev control plugin gone dusk        # a check: not there
./.agents/skills/verify-trek/scripts/trek-dev control look console            # choose a built-in look: Dusk's switch goes off, Trek shows Console
./.agents/skills/verify-trek/scripts/trek-dev control plugin on dusk          # as its switch: its look is back
```

A look is on only while it's the one on screen: another look puts the plugin off (its row says "Its look is off…"); turning the plugin on brings the look back. A theme file written loose into `<data>/themes/` is listed too, as a Look (`look-row-<id>`, switch `look-on-<id>`, `look-added-<id>`).

## Elements to check

`plugin-row-<id>`, `plugin-on-<id>` (its switch), `plugin-added-<id>` (when and by whom), `plugin-look-<id>` and `plugin-use-look-<id>` (a plugin's look: in use or not), `look-row-<id>` (a loose theme file), `plugin-waiting-<id>`, `plugin-problem-<id>`, `plugin-log-<id>` (Settings › Plugins); `plugin-ask` (Settings' first card); `focus-chip`, `focus-length`, `focus-start`, `focus-pause`, `focus-break` (the timer's chip on the composer); `plugin-surface-thread-list` (a replacement drawn in place of the thread list; `-composer`, `-new-thread`); `plugin-page` (a page's pane); `page-<plugin>-<page>` (its row in the sidebar's Pages group).

## Notes

- A web view's own pixels need `screenshot --real`, which needs the Trek Test Display: when other isolated apps hold every corner (`status` says `test_display.created: false`), the window counts as covered and WebKit paints nothing, though the page runs. `plugin page-view` and the page's own bridge still answer.
- Code: `crates/trek-core/src/plugins/` (format, folder, CLI, page sandbox), `crates/trek-app/src/plugins/` (registry, drawing, logic host, focus timer, commit trail, page host), `crates/trek-script/src/plugin.rs` (QuickJS), `crates/trek-app/src/settings_view/plugins.rs`.
- Tests: `cargo test -p trek-core --lib plugins`, `cargo test -p trek-script --lib plugin`, `cargo test -p trek-app --test plugins`.
