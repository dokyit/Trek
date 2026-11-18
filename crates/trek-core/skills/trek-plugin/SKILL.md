---
name: trek-plugin
description: Make a Trek plugin - a folder in Trek's plugins folder that makes Trek the user's own (a whole look or theme, a pet or sticker in the sidebar, sounds when agents finish, their own new-thread screen, a status in the title bar, buttons and chips in the composer, a page or a panel of their own HTML, palette commands with keys, logic that hears Trek's events), checked by Trek, loaded live and listed in Settings › Plugins, where the user turns it off or deletes it. This is Trek's own system: not Claude Code's mods, plugins, skills or hooks, and never written into ~/.claude. Use when the user asks you to restyle, personalise, theme, mod, extend or add something to Trek ("make Trek feel like a campfire", "a Trek look called Dusk", "put a marmot in my sidebar that whistles when an agent needs me", "play a chime when an agent finishes", "greet me on the new-thread screen", "add a focus timer round the composer", "show token burn in the thread header").
---

# Trek plugin

People ask for a plugin in one sentence, and you make it whole. What they ask for is usually
about making Trek theirs:

- "Make Trek feel like a campfire: warm ember colours, answers in a serif, soft round corners,
  a little tent as my prompt, and give the hiker an orange pack."
- "Put a little marmot at the bottom of my sidebar that naps when nothing's running, keeps
  watch while agents work, and whistles when one needs me."
- "Play a soft chime when an agent finishes and a gentle knock when one needs me, and let me
  pick other sounds."

Build all of what the sentence asks, make it look right in both the light and the dark (use the
theme's colour names, not your own, except in pictures), and don't rebuild what Trek has (a
page of finished threads is Basecamp; a list of what needs the user is the sidebar's inbox).

## Read this first: Trek's, not Claude Code's

"Trek" here is the app the user is talking to. Whatever you were asked to change about *it* is a
Trek plugin, written in **Trek's plugins folder** and nowhere else:

- Trek's data folder on this computer: `<data>`
- Trek plugins live in **`<data>/plugins/<id>/`** (`<id>`: lower case letters, digits and dashes).
- Never write into `~/.claude`, `.claude`, `dev-mods`, `mods` or any other folder of Claude Code's,
  Codex's or another agent's: those are the agent's own features (Claude Code has "mods" and its
  own plugin skill) and Trek reads none of them, so nothing you write there ever shows up in Trek.
  If you were about to use another skill for "make a plugin", this one is the one for Trek.
- A look (theme) is a plugin too: a plugin with a `[[theme]]`. Don't write loose files into
  `<data>/themes/`.

Trek reads every folder in `<data>/plugins/` at launch and again within two seconds of any change,
checks it, lists it in **Settings › Plugins** and tells the user it was added. You make a plugin by
writing the folder; there's nothing to register. The user can turn it off, back on, or delete it
there at any time.

## Where it goes, and how it's turned on

1. Write `<data>/plugins/<id>/plugin.toml` and the files it names (create the folders).
2. Read `<data>/plugins/problems.txt` two seconds later. It's there only while a plugin has a
   problem, one line each: `<id>: <what's wrong>`. Fix yours until it isn't listed.
3. Turn it on. A plugin that asks for **no permissions** is yours to turn on: run
   `<trek> plugin enable <id>`, or add it to `<data>/plugins/state.toml` (keep what's there):
   ```
   [plugins.<id>]
   on = true
   ```
   One that asks for some needs the user: run the same command (it only marks the plugin as
   waiting) and tell them to allow it in Settings › Plugins, where what it asks is shown in
   words. Permissions are the user's to give: Trek seals each grant with a key of its own, so a
   `granted` list written into the file counts for nothing. The grant is for the plugin as the
   user saw it: edit any of its files afterwards and it waits to be allowed again (say so when
   you change one).
   A `[[replace]]` shows once it's chosen: add `thread-list = "<id>"` (or `composer`,
   `new-thread`) under `[replace]` in the same file, or let the user pick it in Settings.
4. A look: once it's on, write the look's id (the plugin's id, when it has one `[[theme]]`) alone
   to `<data>/themes/apply`; Trek switches to it within a couple of seconds and removes the file.
5. Tell the user what you made, where it shows, and that **Settings › Plugins** turns it off,
   back on, or deletes it (for a look, choosing another look in Settings › Appearance puts this
   one off, and its switch there brings it back).

Trek's own program does the same from a terminal: `<trek> plugin install <folder | git URL |
example:<name>>`, `list`, `enable <id>`, `disable <id>`, `reload`, `remove <id>`, `dev <folder>
--once` (reads a folder in place while you work on it), `new <id>` (a starting folder in the
current directory, not in Trek's plugins folder: `install` it when it's ready), `examples`.
Trek ships examples to read and copy, each made from the sentence at the top of its README: `trek plugin install example:campfire` (a whole look), `trailhead` (a new-thread screen
with a page), `marmot` (a pet in the sidebar, with hooks), `soft-sounds` (sounds, with a panel),
`now-playing` (the title bar, with permissions), `focus-timer` (the composer's edge).

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
key = "cmd-alt-b"                      # needs cmd, ctrl or alt; cmd is Ctrl on Windows; not one of Trek's
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
  shown when there are none. A column, or with `wrap: true` a row that wraps (chips).
- `when`: `then` while the value at `value` is set (or equals `equals`), `else` otherwise.
- Trek's own components: `trek:focus-timer` (`minutes`, `break`; in `composer.border` a chip,
  and the hiker walking round the composer during a walk), `trek:commit-trail` (`style`: `ridge` or `dots`, `mine`: true or false;
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
- `threads`: in a `thread-list` replacement (which needs `threads:read`), the list of threads
  (the same fields), newest first.
- `project.name`; `theme.dark`, `theme.id`; `state.<key>`: the plugin's own state (below).
- `now`: the time of day, `now.part` (`morning`, `afternoon`, `evening`, `night`), `now.hour`,
  `now.weekday` (Tuesday), `now.date` (17 November), `now.weekend`. A greeting or picture that
  follows the day is a `when` on `now.part`.
- Filters: `k` (12.3k), `int` (10,281), `usd`, `pct`, `ago` (a time as 3m, 2h), `upper`,
  `lower`, `plural:commit` (3 commits), `or:text` (when empty). A path that isn't there is empty.

**Actions** (`on`): `command:<id>` (one of the plugin's commands), `page:<id>`, `panel:<id>`,
`insert:<text>` (into the composer), `set:<key>=<value>` (the plugin's state, with no logic),
`open:<thread id>` and `new-thread` (need `threads:open`), `trek:focus-timer.toggle` (`.start`,
`.pause`, `.stop`, `.break`, `.length`).

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
  - `trek.state.get()`, `trek.state.set(patch)`: the state the plugin's slots and replacements
    draw from (`{state.…}`). A page that edits what a slot shows sets both: storage to keep it,
    state to show it now (and logic hearing `app.start` copies storage back into state).
  - `trek.data(name)`: `"threads"` (a list, as above) and `"finished"` (`{days, start, total,
    today, working, waiting}`: threads settled per day for twelve weeks) with `threads:read`;
    `"usage"` (`{tokens, cost, threads}`, each thread's `title` only with `threads:read`) with
    `usage:read`; `"commits"` (`{weeks, total, start, mine}`: the project's year)
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
  text)`, `trek.page(id)`; `trek.sound("sounds/chime.wav")` plays a `.wav` file of the plugin's
  own (no permission; WAV is what both systems play; one at a time); `console.log` goes to the
  plugin's log.
- State lasts until Trek quits. Keep what must outlast it in `trek.storage`.
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
logic on a timer (a pet can't wander about by itself); replace Trek's own alert sound; reach the
phone apps.

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
