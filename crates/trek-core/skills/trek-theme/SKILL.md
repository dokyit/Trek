---
name: trek-theme
description: Make a Trek theme - a whole look for Trek itself in one small TOML file (light or dark, colours, the fonts for the interface and the conversation, corner roundness, density, background art, glass and motion), carried by a Trek plugin so it's listed in Settings › Plugins and Appearance, checked by Trek and switched to with a cross-fade. Use when the user asks you to restyle Trek, change how Trek looks, or make Trek look like something ("a Trek look called Dusk", "a 90s terminal", "a newspaper", "my editor's theme"). This is Trek's own theme system, not Claude Code's: never write into ~/.claude.
---

# Trek theme

A Trek theme is one TOML file that sets Trek's whole look. It travels in a Trek plugin (a folder
with a four-line `plugin.toml` and the theme file), so it's listed in **Settings › Plugins** with
a switch and Delete, and in Settings › Appearance › Theme and the command palette ("Use … theme").
You make one by writing that folder, then ask Trek to switch to it by writing its id to one more
file. Trek cross-fades to it.

A theme is a look and nothing else, with fonts from the three below. To add things to Trek or
change how it works (buttons, chips and meters, pages and panels, a board in place of the thread
list, commands, logic), make a plugin: the `trek-plugin` skill, and put the theme in the same one.

## Where it goes

This skill's own file is `<data>/skills/trek-theme/SKILL.md`, where `<data>` is Trek's data folder:
`<data>` on this computer. Write only there, never into `~/.claude` or any folder of Claude Code's
or another agent's: those are the agent's own features, and Trek reads none of them.

- The plugin: **`<data>/plugins/<id>/plugin.toml`** (below).
- The theme file: **`<data>/plugins/<id>/look.toml`**, in the format below.

`<id>` is lower case letters, digits and dashes, and it is the theme's id; never `trek`,
`console`, `journal` or `alpenglow` (the built-in ones). `plugin.toml`:

```
id = "dusk"
name = "Dusk"
description = "Violet evening light, answers in a serif."
trek = ">=0.4.55"
[[theme]]
file = "look.toml"
```

(That's the whole file: a look needs no permissions and no logic.) Old themes may still be loose
files in `<data>/themes/<id>.toml`: Trek lists them too (Settings › Plugins, as looks), but write
new ones as plugins.

## Steps

1. Pick the look with the user's words: dark or light, two or three colours that carry it, which
   font for the interface and which for the conversation, square or soft corners, compact or roomy,
   art or none.
2. Write `<data>/plugins/<id>/plugin.toml` and `<data>/plugins/<id>/look.toml` (format below).
3. Read `<data>/plugins/problems.txt` (and `<data>/themes/problems.txt`), a second or two later.
   They're there only while something has a problem, one line each. Fix yours until it isn't in them.
4. Turn the plugin on: `<trek> plugin enable <id>` (a look asks for no permissions, so that turns it
   on), then apply it: write the id alone to **`<data>/themes/apply`** (for example `dusk`). Trek
   switches within a couple of seconds and removes that file. If the file is still there after five
   seconds, Trek isn't running from that folder: tell the user to turn the plugin on in Settings ›
   Plugins and pick the theme in Settings › Appearance.
5. Tell the user what you made, and that Settings › Plugins lists it with a switch and Delete.
   Picking another theme in Settings › Appearance, or "Use Trek theme" in the palette (⌘K / Ctrl+K),
   puts it off (its switch shows that), and turning it back on there brings it back, with nothing to
   write again.

To change a theme that's in use, edit its file: Trek reloads it and takes the new values.

## The format (version 1)

Every key but `name` may be left out; what's left out is Trek's own.

```toml
name = "Ninety"                        # 1-32 characters, shown in Settings and the palette
description = "Green phosphor on black, every word in monospace."   # one line, 120 at most
mode = "dark"                          # "dark", "light", or "any" (keep the user's choice)

[fonts]
ui = "mono"                            # "sans" (Inter), "mono" (JetBrains Mono), "serif" (Spectral)
conversation = "mono"                  # the messages and answers
ui_size = 13                           # points, 11-18 (else from density)
conversation_size = 13.5               # 12-22; code is always JetBrains Mono, sized with it

[shape]
corners = 0.25                         # every corner against Trek's: 0 square, 1 Trek's, 2 at most
density = "compact"                    # "compact", "regular" or "roomy": the text sizes not given

[background]                           # leave the table out for no art
art = "builtin:paintings/matterhorn"   # "none", one of Trek's pieces (below), or an image path
                                       # (relative to this file, or absolute)
placement = "chat"                     # "chat", "sidebar" or "new-thread" (below)
dim = 0.2                              # 0-0.9: darkens the art further, on top of what text needs

[glass]                                # leave the table out for an opaque window
on = true                              # the desktop shows through, blurred (hidden under sidebar art)
tint = 0.6                             # 0.2 clear to 0.95 frosted

[motion]
style = "brisk"                        # "still" (nothing moves), "brisk", "standard" or "calm"

[lines]                                # leave out for Trek's own
pills = "like-corners"                 # pills and circles take the corners too; "round" is Trek's
borders = 1                            # every border's weight: 0 none, 1 Trek's, 3 at most
shadows = 0                            # 0 flat, 1 Trek's

[messages]
user = "line"                          # the user's words flush with the answers; "bubble" is Trek's
user_mark = "›"                        # a mark before them, in the accent (1-3 characters)

[composer]
prompt = "›"                           # a mark before what's typed (1-3 characters)

[hiker]
show = true                            # the hiker on the working trail
pack = "#FFB000"                       # its pack's colour

[marks]
agents = "letter"                      # agents drawn by "logo" (Trek's), "letter" or "none"

[dark]                                 # colours when Trek is dark; [light] when it's light
accent = "#6BDB9A"                     # buttons, links, selection, the working beacon
on_accent = "#06140C"                  # text and icons on an accent fill
window = "#0B0D0C"                     # the window behind everything
sidebar = "#0E1110"                    # the sidebar, title bar and tab bars
surface = "#151A18"                    # inputs, chips, code blocks, hovered and selected rows
raised = "#121615"                     # menus, popovers, cards
text = "#D4DDD7"
muted = "#7C8A82"                      # secondary text: times, hints, labels
border = "#222A26"

[dark.kit]                             # any one of the kit's own colours, by its key (below)
"title_bar.background" = "#0B0D0C"
```

Colours are `"#RRGGBB"` or `"#RRGGBBAA"`. Give both `[dark]` and `[light]` when you can: the user
can still flip between light and dark in Settings, and the other table is what they'll see.

### Trek's art

Every piece but Classic's has a Night and a Paper version; Trek shows the one for the light or
dark on screen.

- Classic, anywhere: `builtin:dawn`, `builtin:night`, `builtin:paper` (dithered ridges).
- Paintings, public-domain landscapes in Trek's colours, for `chat`: `builtin:paintings/matterhorn`,
  `builtin:paintings/kauterskill`, `builtin:paintings/oxbow`, `builtin:paintings/notch`,
  `builtin:paintings/green-river`, `builtin:paintings/aosta`, `builtin:paintings/keelmen`
  (moonlight), `builtin:paintings/merced`.
- Light, long-exposure light made of glyphs, for `chat`: `builtin:light/streaks`,
  `builtin:light/aurora`, `builtin:light/window`, `builtin:light/switchbacks`.
- ASCII, mountains drawn in text, shown in the sidebar whatever `placement` says:
  `builtin:ascii/ramp`, `builtin:ascii/braille`, `builtin:ascii/halftone`, `builtin:ascii/dense`,
  `builtin:ascii/led`.

Placements: `chat` lays the art across the whole chat pane (the new-thread screen, the transcript
and the composer) under a scrim Trek works out from your `window` and `text` colours so the text
keeps 4.5:1; `sidebar` puts it behind the sidebar and title bar with the chat plain;
`new-thread` behind the new-thread composer only.

### The kit's colours

The nine colours above set everything; `[dark.kit]` and `[light.kit]` set one of the kit's own
when a look needs it. Its keys: `accent.background`, `accent.foreground`, `accordion.background`,
`accordion.hover.background`, `background`, `border`, `caret`, `chart.1` to `chart.5`,
`danger.background`, `danger.foreground`, `drag.border`, `drop_target.background`, `foreground`,
`group_box.background`, `info.background`, `input.border`, `link`, `link.hover`,
`list.active.background`, `list.active.border`, `list.background`, `list.even.background`,
`list.head.background`, `list.hover.background`, `muted.background`, `muted.foreground`,
`overlay`, `popover.background`, `popover.foreground`, `primary.background`,
`primary.foreground`, `primary.hover.background`, `progress.bar.background`, `ring`,
`scrollbar.thumb.background`, `secondary.background`, `secondary.foreground`,
`secondary.hover.background`, `selection.background`, `sidebar.accent.background`,
`sidebar.background`, `sidebar.border`, `sidebar.foreground`, `skeleton.background`,
`slider.background`, `success.background`, `switch.background`, `tab.active.background`,
`tab.active.foreground`, `tab.background`, `tab.foreground`, `tab_bar.background`,
`table.background`, `table.head.background`, `table.row.border`, `title_bar.background`,
`title_bar.border`, `warning.background`, `window.border`. A key that isn't the kit's is a
problem.

## Rules

Trek refuses a file that breaks these, and says why in `problems.txt` and in Settings:

- Only the keys above. A misspelt key or value is an error, with its line.
- `text` must be at least 4.5:1 against `window`, `muted` at least 2.5:1, and `on_accent` at
  least 3:1 against `accent`, and `accent` at least 3:1 against `window` (links and the working
  beacon are drawn in it).
- Numbers in their ranges; `format`, if you write it, is `1`.
- Art at a path must exist, and built-in art must be one of the pieces above.

And for a theme that looks made, not generated:

- Keep the surfaces close: `window`, `sidebar`, `raised` and `surface` a few steps apart in
  lightness, of one hue family. The accent is the one loud colour; don't colour the text with it.
- `muted` sits between `text` and `window`; `border` is just visible against `window`.
- On dark themes, an `on_accent` near black on a light accent reads better than white.
- Monospace for the interface reads as a terminal; a serif for the conversation reads as a page.
  Pair a serif conversation with a roomy density or `conversation_size` 16.
- Corners: 0-0.3 for terminals and print, 1 for Trek's own, 1.3-1.6 for soft and glassy.
- Art across the chat looks best at a `dim` of 0-0.2 (Trek already keeps the text readable);
  art in the sidebar wants a dark theme and a `dim` of 0.35-0.55. Pick a painting whose light
  suits your accent: Matterhorn and Kauterskill are warm, Notch and Keelmen cool.

## Example: "make Trek look like a 90s terminal"

`<data>/plugins/ninety-terminal/look.toml`:

```toml
name = "Ninety Terminal"
description = "Amber phosphor on black glass, every word in monospace."
mode = "dark"

[fonts]
ui = "mono"
conversation = "mono"

[shape]
corners = 0
density = "compact"

[motion]
style = "brisk"

[dark]
accent = "#FFB000"
on_accent = "#140C00"
window = "#0A0805"
sidebar = "#0D0A06"
surface = "#18130B"
raised = "#120E08"
text = "#FFCC66"
muted = "#A3803F"
border = "#2A2112"

[light]
accent = "#8A5A00"
on_accent = "#FFFFFF"
window = "#FBF6EA"
sidebar = "#F3ECDB"
surface = "#ECE3CF"
raised = "#FFFCF4"
text = "#2B2011"
muted = "#75654A"
border = "#E0D5BD"
```

As a plugin: `<data>/plugins/ninety-terminal/look.toml` is the file above, `plugin.toml` names it
(`[[theme]] file = "look.toml"`), then `<trek> plugin enable ninety-terminal` and write
`ninety-terminal` to `<data>/themes/apply`.
