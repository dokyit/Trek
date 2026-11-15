# Trek plugins

A Trek plugin changes how Trek looks and works: a whole new look, a meter in every thread's
header, a board in place of the thread list, a timer round the composer, a Music page in the
sidebar. Users rarely write one by hand: they ask an agent, which follows Trek's built-in skill
`trek-plugin`, writes the folder, installs it and reloads it while they watch.

This is the design, and the reasons for it. The skill (`crates/trek-core/skills/trek-plugin`) is
the reference an agent works from; the code is `trek_core::plugins` (the format, the folder,
the CLI, the permissions) and `trek_app::plugins` (drawing, the logic host, Settings › Plugins).

## What a plugin is

A folder with a `plugin.toml`:

```toml
id = "focus-timer"                  # lower case and dashes; the folder's name if left out
name = "Focus timer"
version = "1.0.0"
description = "The hiker walks round the composer while you focus."
author = "Trek"
trek = ">=0.4.54"                   # the Trek versions it runs on (semver requirement)
permissions = []                    # what it may read or do (below)
main = "main.js"                    # optional: its logic (below)

[[slot]]                            # something drawn in one of Trek's places
at = "composer.border"
ui = { type = "trek:focus-timer", minutes = 25 }

[[command]]                         # an entry in the palette, and a key
id = "start"
title = "Start a focus timer"
key = "cmd-alt-f"
```

Besides `[[slot]]` and `[[command]]` it may have `[[theme]]` (a look), `[[page]]` (a sidebar
page), `[[panel]]` (a tab beside a thread), `[[replace]]` (a surface of its own in place of
Trek's), `[[effect]]` (words marked in the composer's draft) and `hooks` (Trek events its logic
hears). Everything is optional: a theme alone is a plugin, and so is one button.

Plugins live in `<data>/plugins/<id>/`, where `<data>` is Trek's data folder. Trek reads the
folder at launch and again whenever anything in it changes (every 1.5 s it compares names, sizes
and times, off the UI thread, as the themes folder is watched), so an edit shows within a couple
of seconds. What's wrong with a plugin is shown in Settings › Plugins and written to
`<data>/plugins/problems.txt`, one line per plugin, for an agent to read and fix.

`<data>/plugins/state.toml` is what the user chose: which plugins are on, the permissions each
was granted, and which surface each replacement slot shows. Settings writes it; so does the CLI.

## The CLI

Trek's own program takes `plugin` as its first argument and does the job without opening a
window (the running Trek sees the folder change and reloads):

```
trek plugin install <folder | git URL | example:<name>>   copy it in (git: a shallow clone)
trek plugin list                                          id, version, on or off, problems
trek plugin enable <id> | disable <id>
trek plugin reload [<id>]                                  read it again now
trek plugin remove <id>
trek plugin dev <folder>                                   link the folder in place, print
                                                           problems and the log as they change
trek plugin new <id>                                       a starting folder to edit
```

`dev` doesn't copy: `<data>/plugins/<id>.link` names the folder, and Trek reads the plugin from
there, so an editor's save is a reload. Enabling from the CLI turns on a plugin that asks for no
permissions; one that asks for some waits for the user to allow them in Trek (below), so an agent
can't grant itself anything.

## How plugin UI is drawn

Trek is GPUI, not a web page: there's no DOM to hand a plugin. A plugin's UI is drawn three ways,
each where it fits.

### 1. Element trees, drawn natively (slots and replacements)

A slot's or a replacement's `ui` is a small tree of elements, as TOML inline or a JSON file
(`ui = "ui/board.json"`), which Trek lays out and paints with GPUI in the theme's own colours,
fonts and corners:

```json
{ "type": "row", "gap": 6, "children": [
  { "type": "icon", "name": "flame", "color": "warning" },
  { "type": "text", "text": "{thread.tokens|k} tokens · {thread.cost|usd}", "size": "meta" }
]}
```

Elements: `row`, `column`, `stack`, `text`, `icon`, `button`, `chip`, `meter`, `spark`,
`image`, `spacer`, `divider`, `each` (one child per item of a list, filtered), `when` (a child
or another by a value), and Trek's own components, `trek:<name>` (the focus timer, the commit
trail, the hiker, Trek's own thread list or composer for a replacement to embed). Style keys are
tokens (`bg = "surface"`, `fg = "muted"`, `radius = "card"`, `size = "meta"`, `font = "mono"`) or
literal values; `accent/20` is the accent at 20%.

Text holds bindings, `{path|filter}`, read from what the slot is drawn for: `thread` (its title,
state, agent, model, tokens, cost, context), `project`, `threads` (a replacement's list),
`state` (what the plugin's logic set) and `theme`. A path that isn't there is empty, never an
error, so a frame can't fail. A button's `on` is an action: `command:<id>` (the plugin's logic),
`open:<thread id>`, `page:<id>`, `new-thread`.

Why this, for slots: it's as fast as the rest of Trek (no process, no second renderer), it looks
like Trek in any theme, and it draws inside menus, popovers and the composer, where nothing else
can go. The tree is checked once, when the plugin is read; a frame only walks it.

### 2. Web views, for full panes (pages and panels)

A `[[page]]` (a row in the sidebar's Pages group that opens in the main pane) or a `[[panel]]`
(a tool beside a thread) is HTML, CSS and JavaScript in the plugin's folder, shown in the
system's web view (WKWebView, WebView2) with the interactive-artifact sandbox
(`trek_core::artifact`):

- served under a scheme of its own, `trek-plugin://<id>/`, with the plugin's own files and
  nothing else, under a Content-Security-Policy with no network at all unless the plugin was
  granted `network:<host>`;
- no navigation away, no new windows, no downloads, no devtools outside debug builds, website
  data kept per plugin;
- Trek's theme as CSS variables (`--trek-bg`, `--trek-fg`, `--trek-accent`, `--trek-font`…,
  with Trek's fonts served to it), changed in place when the theme changes;
- one channel to Trek, `window.trek`, whose messages carry a per-view secret only Trek's own
  bootstrap holds: `trek.storage` (the plugin's own, kept by Trek), `trek.data(name)` (what the
  plugin's permissions allow), `trek.on('theme', …)`, `trek.open(...)`.

A web view is a native view laid over the window: GPUI can't draw over it, so menus and the
palette hide it while they're open. That's why web views are only for full panes, never slots.
The page's view is made when the page opens and dropped when it closes: a page that isn't on
screen has no process.

### 3. Logic in QuickJS

`main.js` runs in QuickJS, the engine Trek already ships for its `script` tool (`trek-script`,
built from source on both platforms). Each call (a hook, a command) evaluates the file in a
fresh context on a background thread and calls the handler: nothing lives between calls but the
plugin's state, so a plugin can't leak, keep a timer, or be left half-broken by an error. A call
has a time budget (250 ms) and a memory cap (32 MB); over either, or on an exception, the plugin
is stopped. Calls are queued, one at a time.

```js
trek.on("turn.end", (e) => trek.state.set({ last: e.thread.title }));
trek.command("start", () => trek.state.set({ running: true }));
```

The logic never runs on the UI thread and is never asked anything while a frame draws: it sets
`state`, which the element trees read from memory. What it may read and do is its permissions'
(`trek.threads()`, `trek.send()`, `trek.open()`, `trek.toast()`, `trek.storage`): what it asks
for comes back as data, and Trek does it or refuses it. Simple state needs no logic at all: an
element's `set:key=value` action sets it.

## What a plugin can contribute

**Looks.** `[[theme]] file = "look.toml"` adds a theme to Settings › Appearance and the palette.
A theme is the `trek-theme` skill's file, widened so a look can really be different: every colour
the kit draws with (`[dark.kit]`, `[light.kit]`, by its key), the interface, conversation and
code fonts (Trek's three, or font files the plugin ships), sizes, corners, border weight,
shadows, how messages are set (a bubble, or a line with a prompt mark), the agents' marks, the
composer's prompt, and the hiker (shown or not).

**Slots** (`at`):

| Slot | Where |
|---|---|
| `composer.actions` | buttons in the composer's bar |
| `composer.plus` | items in the composer's + menu |
| `composer.banner` | a card above the composer |
| `composer.top` | a chip on the composer's top edge |
| `composer.border` | drawn round the composer's edge (Trek's components) |
| `thread.header` | items in a thread's header |
| `sidebar.top`, `sidebar.foot` | rows at the sidebar's top, and its foot |
| `new-thread.above`, `new-thread.below` | over and under the new-thread composer |
| `background` | behind the chat |
| `status` | the title bar's right end |

**Pages and panels** (`[[page]]`, `[[panel]]`): web views, above.

**Replacements** (`[[replace]] surface = "thread-list"`): a surface of the plugin's own in place
of Trek's: `thread-list`, `composer`, `new-thread`. Settings › Plugins picks Trek's own or a
plugin's for each. The plugin's tree may embed Trek's original (`trek:thread-list`), and one that
fails (its tree can't be read, its logic stops) falls back to Trek's at once, with a notice.

**Commands and hooks.** `[[command]]` puts an entry in the palette (`run` names a function of the
logic, or `action` is one of the element actions) and, with `key`, a keybinding (it needs cmd,
ctrl or alt; `cmd` is Ctrl on Windows). The palette also opens each plugin's pages and panels and
turns plugins on and off. `hooks = ["turn.end", …]` are the events its logic hears: `turn.start`,
`turn.end`, `thread.needs-you`, `file.changed` (a turn changed files), `theme.changed`,
`app.start`. Trek watches for them only while some running plugin names one, by looking over
the threads' states a quarter of a second after the workspace changes, never per frame.

**Text effects.** `[[effect]] match = "TODO"` washes or underlines its words wherever they
appear in the composer's draft, painted over the text from the textarea's own layout: it
changes nothing of what's typed or sent.

## Permissions

A plugin asks for what it needs in `permissions`; the user allows them, in words, when they turn
it on (a sheet in Settings › Plugins, or the notice a CLI install leaves):

| Permission | What it allows |
|---|---|
| `threads:read` | threads' titles, states, agents and times (not their transcripts) |
| `usage:read` | tokens and cost, per thread and per day |
| `git:read` | the open project's branch and commit history |
| `notes:read` | the user's notes |
| `threads:open` | opening a thread or starting a new one |
| `threads:send` | sending a message to a thread |
| `network:<host>` | a page may reach `https://<host>` |

Everything else is out of reach: there's no file system, shell or network beyond these. A
plugin's own storage is always there (its own, 1 MB). Trek's style surfaces, slots drawn from the
plugin's own state and pages that read nothing need no permission at all.

What the gate is against: the CLI (`trek plugin enable` turns on only what asks for nothing) and
a plugin's own code, which has no way to reach `state.toml`. It is not against a program that
can already write Trek's data folder: an agent with the user's file access could write a grant
there, as it could edit any other setting. The skill tells agents never to.

## Safety

- Plugins are third-party code, so nothing runs until the user has turned it on and allowed what
  it asks for. A plugin whose permissions grow on an update is turned off until they're allowed
  again.
- A failure stops only that plugin: an exception, a call over its time budget, or a context over
  its memory turns it off, says so in a notice, and keeps the reason in its log. A replacement
  that stops gives the surface back to Trek's own.
- Each plugin has a log (`<data>/plugin-data/<id>/log.txt`, kept short, beside its storage and
  outside the plugins folder so writing it isn't an edit to a plugin), shown in Settings ›
  Plugins: what it printed (`console.log`), files its pages asked for that aren't there, what was
  refused, and why it was stopped.
- A page's web view hands the keyboard back to Trek's own view before it's hidden or dropped: a
  native view that held the keys and then went would leave the window with nothing to type
  into. The capture build checks it at the window on a Mac (`plugin-keys`).

## Performance

Trek is first on ADEMark's speed and weight tests, and plugins must not change that.

- **No plugins, no cost.** With an empty folder Trek reads it once at launch and looks at it on
  the themes folder's own tick (one timer for both, off the UI thread); nothing else is made: no
  thread, no QuickJS, no web view, no key or event listener. Each slot's check is an empty list.
- **Off screen, no cost.** A slot draws only when its place is on screen. A page's web view
  exists only while the page is open. Trek's animated components (the focus timer) tick only
  while visible, at the hiker's 16 fps clock or slower, stop while the window is covered, and
  stand still with Reduce motion.
- **Frames never wait.** A render never reads the disk, runs JavaScript or walks a thread's
  items. Data a slot shows (git history, counts) is worked out off the UI thread when it changes
  and kept; logic runs on its own thread and sets state.

`./bin/agent-cli perf launch idle` measures launch and idle; `docs/PLUGINS.md` keeps the numbers
from when plugins landed (below).

## Not built yet

- Logic on a timer, and `fetch` from logic (pages have `network:<host>`).
- A composer replacement can wrap Trek's composer (`trek:composer`) but can't supply an input
  of its own; a declarative tree has no text field.
- Web content inside a slot: web views are for pages and panels.
- On Windows a release `trek.exe` is a GUI program, so `trek plugin …` typed at a console prints
  nothing (a pipe, as an agent's, gets its output). The folder is the interface either way.
- Plugins' own fonts and icons: a look uses Trek's three families, a tree Trek's Lucide set.

## Phones

Not yet. The iPhone app and the web remote get the theme's colours and fonts already
(`trek_remote::protocol::look`). To show slots there, Trek would send the trees and their bound
values over the same protocol and each client would draw them with its own widgets (SwiftUI on
the phone, the web remote's DOM); pages would need the plugin's files served through the remote
with the same CSP.

## Numbers

Measured with `./bin/agent-cli perf launch idle --threads 0,10 --runs 3 --cold-runs 1
--idle-secs 40` on an M-series Mac, the isolated app (release, `shots`):

| | before plugins | with plugins (none installed) |
|---|---|---|
| warm launch, 0 / 10 threads | 197 / 194 ms | 187 / 169 ms |
| idle, 0 threads | RSS 122 MB, footprint 76 MB, CPU 0.03%, 8.6 context switches/s | RSS 123 MB, footprint 76 MB, CPU 0.03%, 8.1 context switches/s |
| idle, 10 threads | RSS 125 MB, footprint 78 MB, CPU 0.03%, 8.3 context switches/s | RSS 122 MB, footprint 78 MB, CPU 0.03%, 8.1 context switches/s |

The first cut gave the plugins folder a watch of its own, and idle context switches rose to 10.1
a second; looking at it on the themes folder's tick brought them back. Not measured: a session
with plugins running (the focus timer redraws the composer about once a second while a session
is under way and it's on screen; a page is a web view's process while it's open).
