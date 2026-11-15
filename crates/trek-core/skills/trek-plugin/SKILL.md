---
name: trek-plugin
description: Make a Trek plugin - a folder that changes how Trek itself looks and works (a whole look, buttons and chips in the composer, a meter in each thread's header, rows in the sidebar, a page or a panel of your own HTML, a board in place of the thread list, palette commands with keys, logic that hears Trek's events), checked by Trek and loaded live. Use when the user asks you to restyle, mod, extend or add something to Trek ("make Trek look like a terminal", "add a focus timer round the composer", "show token burn in the thread header", "add a Music page", "replace the thread list with a board").
---

# Trek plugin

A plugin is a folder with a `plugin.toml`. Trek reads every folder in `<data>/plugins/`, reads it
again within two seconds of any change, and lists it in Settings › Plugins. You make one by
writing the folder; the user sees it appear.

For a look alone (colours, fonts, corners), the `trek-theme` skill has the theme file's format;
a plugin can carry that theme (`[[theme]]`) along with everything else here.

## Where it goes, and how it's turned on

This skill's file is `<data>/skills/trek-plugin/SKILL.md`; plugins live in **`<data>/plugins/<id>/`**
(two folders up from this file, then `plugins`). The folder's name is the plugin's id: lower case
letters, digits and dashes.

1. Write `<data>/plugins/<id>/plugin.toml` and the files it names.
2. Read `<data>/plugins/problems.txt` two seconds later. It's there only while a plugin has a
   problem, one line each: `<id>: <what's wrong>`. Fix yours until it isn't listed.
3. Turn it on by adding it to `<data>/plugins/state.toml` (keep what's there):
   ```
   [plugins.<id>]
   on = true
   ```
   A plugin that asks for **no permissions** runs at once. One that asks for some needs the user:
   write `waiting = true` instead of `on = true`, and tell them to allow it in Settings ›
   Plugins, where what it asks is shown in words. Permissions are the user's to give: never
   write a `granted` list yourself.
   A `[[replace]]` shows once it's chosen: add `thread-list = "<id>"` (or `composer`,
   `new-thread`) under `[replace]` in the same file, or let the user pick it in Settings.
4. Tell the user what you made, where it shows, and that Settings › Plugins turns it off or
   removes it.

Trek's own program does the same from a terminal, if it's on the PATH: `trek plugin install
<folder | git URL | example:<name>>`, `list`, `enable <id>`, `disable <id>`, `reload`, `remove
<id>`, `dev <folder> --once` (reads a folder in place while you work on it), `new <id>`,
`examples`. Trek ships examples to read and copy: `trek plugin install example:focus-timer`,
`commit-graph`, `summit-log`, `trail-board`, `terminal-look`.

While you work, the plugin's log is `<data>/plugin-data/<id>/log.txt`: what its logic printed,
files a page asked for that aren't there, anything refused, and why Trek stopped it if it did.

## plugin.toml

```toml
name = "Burn"                          # 1-40 characters, shown in Settings
version = "1.0.0"
description = "Token burn in every thread's header."   # one line, 200 at most
author = "you"
trek = ">=0.4.54"                      # the Trek versions it runs on
permissions = ["threads:read", "usage:read"]   # what it may read or do (below); [] for none
main = "main.js"                       # its logic, if it has any (below)
hooks = ["turn.end"]                   # the events its logic hears

[[slot]]                               # something drawn in one of Trek's places
at = "thread.header"
ui = "ui/meter.json"                   # an element tree: a JSON file, or a table written here

[[command]]                            # an entry in the palette, and a key
id = "reset"
title = "Burn: reset the meter"
key = "cmd-alt-b"                      # needs cmd, ctrl or alt; cmd is Ctrl on Windows
run = "reset"                          # a function of main.js; or action = "page:burn"

[[page]]                               # a page in the sidebar's Pages group (your HTML)
id = "burn"
title = "Burn"
icon = "flame"
# entry = "pages/burn/index.html"      # where it opens; this is the default

[[panel]]                              # the same, in a tab beside the thread
id = "pad"
title = "Scratch pad"
entry = "pad/index.html"

[[replace]]                            # your tree in place of one of Trek's surfaces
surface = "thread-list"                # "thread-list", "composer" or "new-thread"
title = "Board"
ui = "ui/board.json"

[[effect]]                             # mark words in the composer's draft as they're typed
match = "TODO"
color = "warning"                      # a colour (below)
style = "highlight"                    # or "underline"

[[theme]]                              # a look, in the trek-theme skill's format
file = "look.toml"
```

Only these keys; a misspelt one is a problem, with its line. Every file a plugin names must be
inside its folder. Everything but `name` is optional.

## Element trees: what slots and replacements draw

Trek is not a web page: a slot's `ui` is a small tree that Trek draws itself, in the theme's
colours and fonts. JSON in a file, or the same as a TOML table.

```json
{ "type": "row", "gap": 6, "pad": [2, 8], "radius": "full", "border": "border",
  "tooltip": "{thread.tokens|int} tokens",
  "children": [
    { "type": "icon", "name": "flame", "fg": "warning", "size": 12 },
    { "type": "text", "text": "{thread.tokens|k|or:0}", "size": "meta", "font": "mono" },
    { "type": "meter", "value": "{thread.context.used}", "max": "{thread.context.size}", "width": 36 },
    { "type": "text", "text": "{thread.cost|usd}", "size": "meta", "fg": "muted" }
  ] }
```

**Elements** (`type`):

- `row`, `column`, `stack` (children on top of each other): have `children`.
- `text` (`text`), `chip` (`text`, a small rounded label), `icon` (`name`, a Lucide icon:
  `flame`, `mountain`, `tent`, `compass`, `map`, `footprints`, `flag`, `timer`, `hourglass`,
  `clock`, `play`, `pause`, `star`, `heart`, `zap`, `activity`, `gauge`, `target`, `route`,
  `music`, `headphones`, `coffee`, `gamepad-2`, `book-open`, `notebook-pen`, `list-todo`,
  `square-kanban`, `blocks`, `puzzle`, `terminal`, `bug`, `rocket`, `sparkles`, `hand`, `eye`,
  `pin`, `inbox`, `archive`, `users`, `image`, `link`, `git-branch`, `chart-bar-big`,
  `chart-spline`, `sun`, `moon`, `cloud`, `leaf`; one Trek doesn't carry shows a puzzle piece).
- `button` (`text`, `on`: an action).
- `meter` (`value`, `max`), `spark` (`values`: a path to a list of numbers), `image` (`src`: a
  file in the plugin), `spacer`, `divider`.
- `each`: one `item` per entry of the list at `of`, those matching `where` (a table of field =
  value; `a|b` is either), at most `limit`; `as` names the entry (default `item`); `empty` is
  shown when there are none.
- `when`: `then` while the value at `value` is set (or equals `equals`), `else` otherwise.
- Trek's own components: `trek:focus-timer` (`minutes`; in `composer.border` it walks the hiker
  round the composer), `trek:commit-trail` (`style`: `ridge` or `dots`, `mine`: true or false;
  needs `git:read`), `trek:hiker`, and in a replacement `trek:thread-list` or `trek:composer`:
  Trek's own surface, embedded in yours.

**Style keys**, on any element: `bg`, `fg`, `border` (colours), `radius` (`control`, `card`,
`full` or points), `pad` (points, `[vertical, horizontal]` or four), `gap`, `size` (`meta`,
`label`, `body`, `title`, `heading` or points), `weight` (`normal`, `medium`, `semibold`,
`bold`), `font` (`ui`, `mono`, `conversation`), `italic`, `width`, `height` (points or `"full"`),
`grow`, `align`, `justify` (`start`, `center`, `end`, `between`), `opacity`, `wrap`, `truncate`,
`tooltip`, and `on` (an action when it's clicked).

**Colours** are the theme's, by name, so the plugin follows any look: `accent`, `on-accent`,
`text`, `muted`, `window`, `sidebar`, `surface`, `raised`, `border`, `positive`, `warning`,
`danger`; `accent/20` is the accent at 20%. Or `#RRGGBB`.

**Bindings**: `{path|filter}` in any text. What's there to read depends on the place:

- `thread`: the thread it's drawn for (a header, the composer): `id`; with `threads:read` also
  `title`, `state` (`working`, `needs-you`, `idle`, `done`, `failed`), `agent`, `model`,
  `project`, `branch`, `started`, `updated`, `added`, `removed`; with `usage:read` also
  `tokens`, `cost`, `context.used`, `context.size`, `context.share`.
- `threads`: in a `thread-list` replacement, the list of threads (the same fields), newest first.
- `project.name`; `theme.dark`, `theme.id`; `state.<key>`: the plugin's own state (below).
- Filters: `k` (12.3k), `int` (10,281), `usd`, `pct`, `ago` (a time as 3m, 2h), `upper`,
  `lower`, `plural:commit` (3 commits), `or:text` (when empty). A path that isn't there is empty.

**Actions** (`on`): `command:<id>` (one of the plugin's commands), `page:<id>`, `panel:<id>`,
`insert:<text>` (into the composer), `set:<key>=<value>` (the plugin's state, with no logic),
`open:<thread id>` and `new-thread` (need `threads:open`), `trek:focus-timer.start` (`.pause`,
`.stop`).

**Slots** (`at`):

| Slot | Where |
|---|---|
| `composer.actions` | in the composer's bar, before the send button |
| `composer.plus` | an item in the composer's + menu (a `button`) |
| `composer.banner` | a row at the top of the composer's card |
| `composer.top` | a chip on the composer's top edge |
| `composer.border` | round the composer's edge (`trek:focus-timer` only) |
| `thread.header` | after a thread's title, in the title bar |
| `sidebar.top`, `sidebar.foot` | under Basecamp, Notes and Study; above Settings |
| `new-thread.above`, `new-thread.below` | over and under the new-thread composer |
| `background` | behind the chat |
| `status` | the title bar's right end |

Keep a tree small (400 elements at most): it's drawn every frame its place is on screen.

## Pages and panels: your own HTML

A `[[page]]` or `[[panel]]` is HTML, CSS and JavaScript in the plugin's folder, in a web view:
anything a page can draw (charts, a game, a piano). Use them for whole panes; small things in
Trek's own places are slots.

- **Its own files only.** Link them by relative path (`<script src="app.js">`). No CDN, no
  remote fonts, no network at all, unless the plugin has `network:<host>` in `permissions` and
  the user allowed it (then `fetch("https://<host>/…")` works).
- **Trek's look, live**: CSS variables `--trek-bg`, `--trek-fg`, `--trek-muted`, `--trek-accent`,
  `--trek-border`, `--trek-surface`, `--trek-positive`, `--trek-warning`, `--trek-negative`,
  `--trek-font`, `--trek-mono`, `--trek-font-size`, `--trek-radius`. The body already has
  Trek's font, colours and a page's padding. Use the variables, never colours of your own, and
  the page follows every theme.
- **`window.trek`**, each returning a promise:
  - `trek.storage.get()`, `trek.storage.set(object)`: the plugin's own storage (1 MB), kept
    between launches. Use it, not `localStorage`.
  - `trek.data(name)`: `"threads"` (a list, as above) and `"finished"` (`{days, start, total,
    today, working, waiting}`: threads settled per day for twelve weeks) with `threads:read`;
    `"usage"` with `usage:read`; `"commits"` (`{weeks, total, start, mine}`: the project's year)
    and `"project"` with `git:read`; `"notes"` with `notes:read`.
  - `trek.open(threadId)`, `trek.newThread()` (`threads:open`); `trek.send(threadId, text)`
    (`threads:send`); `trek.toast(text)`.
  - `trek.theme` (`{dark, id}`) and `trek.on("theme", fn)`.
  A call the plugin has no permission for rejects, saying which it needs.

## Logic: main.js

`main.js` runs in a sandboxed JavaScript engine (no `fetch`, files, timers or DOM) when one of
the plugin's `hooks` happens or one of its commands runs. Each call starts fresh: what lasts
between calls is `trek.state` (and `trek.storage`). A call has 250 ms.

```js
trek.on("turn.end", (e) => {
  const s = trek.state.get();
  trek.state.set({ finished: (s.finished || 0) + 1, last: e.thread.title });
});
function reset() {                     // [[command]] run = "reset"
  trek.state.set({ finished: 0 });
  trek.toast("Meter reset");
}
```

- Hooks (name them in `hooks`): `turn.start`, `turn.end`, `thread.needs-you`, `file.changed`
  (a turn changed files), each with `e.thread`; `theme.changed`; `app.start`.
- `trek.state.get()`, `trek.state.set(patch)`: what element trees read as `{state.…}`.
- `trek.storage.get()`, `trek.storage.set(object)`; `trek.threads()` (`threads:read`);
  `trek.theme()`; `trek.toast(text)`, `trek.open(id)`, `trek.newThread()`, `trek.send(id,
  text)`, `trek.page(id)`; `console.log` goes to the plugin's log.
- An exception, a call that runs past its time, or too much memory **stops the plugin** (it's
  turned off, with the reason in its log and in Settings). Catch what can fail.

## Permissions

Ask only for what the plugin uses; each is shown to the user in words before it runs.

| Permission | Allows |
|---|---|
| `threads:read` | threads' titles, states, agents and times (never what's in them) |
| `usage:read` | tokens and cost |
| `git:read` | the open project's branch and commit history |
| `notes:read` | the user's notes |
| `threads:open` | opening a thread or starting one |
| `threads:send` | sending a message to a thread's agent |
| `network:<host>` | a page may reach `https://<host>` |

There is no file, shell or other network access, and no way round the list.

## What a plugin can't do (yet)

Say so if the user asks for these: change Trek's own layout beyond the slots and the three
replaceable surfaces; draw web content inside a slot (web views are for pages and panels); type
into or read the composer's draft (it can insert text and mark words); read transcripts; run
logic on a timer; reach the phone apps.

## Example: "show how many turns finished today in the title bar"

`<data>/plugins/turn-count/plugin.toml`:

```toml
name = "Turn count"
description = "How many turns have ended, in the title bar."
main = "main.js"
hooks = ["turn.end"]

[[slot]]
at = "status"
ui = { type = "chip", text = "{state.finished|or:0} turns", tooltip = "Turns ended since Trek started" }
```

`<data>/plugins/turn-count/main.js`:

```js
trek.on("turn.end", () => {
  trek.state.set({ finished: (trek.state.get().finished || 0) + 1 });
});
```

Then add `[plugins.turn-count]` with `on = true` to `<data>/plugins/state.toml`: it asks for no
permissions, so the chip is there within two seconds.
