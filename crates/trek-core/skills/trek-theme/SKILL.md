---
name: trek-theme
description: Make a Trek theme - a whole look for Trek itself in one small TOML file (light or dark, colours, the fonts for the interface and the conversation, corner roundness, density, background art, glass and motion), checked by Trek and switched to with a cross-fade. Use when the user asks you to restyle Trek, change how Trek looks, or make Trek look like something ("a 90s terminal", "a newspaper", "my editor's theme").
---

# Trek theme

A Trek theme is one TOML file that sets Trek's whole look. Trek reads every `*.toml` in its
themes folder, watches it, and lists each theme in Settings › Appearance › Theme and in the
command palette ("Use … theme"). You make one by writing a file there, then ask Trek to switch to
it by writing its name to one more file. Trek cross-fades to it.

What a theme can't do: change the layout, add views or widgets, load fonts other than the three
below, or run code. Say so if the user asks for those; the look is what you can change.

## Where it goes

This skill's own file is `<data>/skills/trek-theme/SKILL.md`, where `<data>` is Trek's data
folder. Themes live in **`<data>/themes/`** (two folders up from this file, then `themes`). Create
the folder if it isn't there. The file's name is the theme's id: `ninety-terminal.toml` is
`ninety-terminal`. Lower case and dashes; never `trek`, `console`, `journal` or `alpenglow` (the
built-in ones).

## Steps

1. Pick the look with the user's words: dark or light, two or three colours that carry it, which
   font for the interface and which for the conversation, square or soft corners, compact or roomy,
   art or none.
2. Write `<data>/themes/<id>.toml` (format below).
3. Read `<data>/themes/problems.txt`, a second or two later. It's there only while a file in the
   folder has a problem, one line per file: `<file>: <what's wrong>`. Fix your file until your
   file isn't in it.
4. Apply it: write the id alone to **`<data>/themes/apply`** (for example `ninety-terminal`).
   Trek switches within a couple of seconds and removes that file. If the file is still there
   after five seconds, Trek isn't running from that folder: tell the user to pick the theme in
   Settings › Appearance.
5. Tell the user what you made and that they can go back with "Use Trek theme" in the palette
   (⌘K / Ctrl+K), which restores exactly what they had before they first switched.

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

`<data>/themes/ninety-terminal.toml`:

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

Then write `ninety-terminal` to `<data>/themes/apply`.
