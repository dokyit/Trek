---
name: trekdesktop.com
description: "Trek's website: the app's Paper and Night, its ridges and its hiker, around real captures of the app."
colors:
  accent: "#B53C0B"
  accent-over: "#9C3308"
  on-accent: "#FFFFFF"
  ember: "#E85D1F"
  amber: "#C98500"
  amber-ink: "#8A5A00"
  emerald: "#1E9E62"
  emerald-ink: "#14734A"
  ground: "#F3EEE6"
  raised: "#FBF8F3"
  ink: "#1A1A1C"
  ink-2: "#45403A"
  mute: "#6A6259"
  line: "rgb(40 28 16 / 0.14)"
  accent-night: "#FF7A3D"
  accent-over-night: "#FF8D58"
  on-accent-night: "#1A0E06"
  ember-night: "#FF7A3D"
  amber-night: "#FFB020"
  emerald-night: "#3FCF8E"
  ground-night: "#0E0D12"
  raised-night: "#17161C"
  sky-night: "#0B1222"
  ink-night: "#F3EEE6"
  ink-2-night: "#D2CBC3"
  mute-night: "#A69F98"
  line-night: "rgb(243 238 230 / 0.14)"
typography:
  display:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2.5rem, 4.75vw, 4.25rem)"
    fontWeight: 600
    lineHeight: 1.03
    letterSpacing: "-0.028em"
  headline:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2rem, 3.2vw, 2.75rem)"
    fontWeight: 600
    lineHeight: 1.06
    letterSpacing: "-0.024em"
  page-title:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2.25rem, 4vw, 3.25rem)"
    fontWeight: 600
    lineHeight: 1.05
    letterSpacing: "-0.028em"
  doc-heading:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "1.5rem"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "-0.015em"
  wordmark:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 500
    lineHeight: 1
    letterSpacing: "-0.01em"
  title:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 500
    lineHeight: 1.3
  lede:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "19px"
    fontWeight: 400
    lineHeight: 1.5
  body:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.55
  button:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 500
    letterSpacing: "-0.005em"
  label:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.55
  note:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.55
  mono:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.45
rounded:
  sm: "10px"
  md: "12px"
  lg: "16px"
  phone: "56px"
spacing:
  gutter: "24px"
  gutter-narrow: "16px"
  nav: "68px"
  column-gap: "72px"
  band-top: "120px"
  band-bottom: "132px"
  band-top-narrow: "84px"
  band-bottom-narrow: "96px"
components:
  button-fill:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.on-accent}"
    typography: "{typography.button}"
    rounded: "{rounded.md}"
    padding: "0 20px"
    height: "48px"
  button-fill-hover:
    backgroundColor: "{colors.accent-over}"
  button-plain:
    textColor: "{colors.ink}"
    typography: "{typography.button}"
    rounded: "{rounded.md}"
    padding: "0 20px"
    height: "48px"
  button-small:
    rounded: "{rounded.sm}"
    padding: "0 14px"
    height: "36px"
  nav-link:
    textColor: "{colors.ink-2}"
  nav-link-hover:
    textColor: "{colors.ink}"
  inbox-rows:
    backgroundColor: "{colors.raised}"
    rounded: "{rounded.lg}"
    padding: "8px"
  inbox-row:
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "11px 12px"
  approval-card:
    textColor: "{colors.ink}"
    rounded: "{rounded.lg}"
    padding: "16px"
  approval-answer:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    height: "40px"
  build-row:
    typography: "{typography.title}"
    padding: "20px 0"
  phone:
    backgroundColor: "#131214"
    rounded: "{rounded.phone}"
    padding: "10px"
    width: "318px"
---

# Design System: trekdesktop.com

This file records what was built, read from `index.html`, `assets/site.css`, `assets/site.js`,
`privacy/index.html` and `404.html` on 11 November 2026. Where the build and the plan for it
differ, the build is what is written here, and the difference is listed at the end.

## Overview

**Creative North Star: "The Trailhead"**

The site is where a visitor meets Trek before installing it, and it shows the thing itself: the real app at its real size, on the app's own ground, with the app's own hiker at work on the window's top edge. One idea leads the home page, the Inbox, and each section under it adds one fact about it. The name is the form the owner chose for the home page.

The look is the app's two themes. Paper by day is a warm off-white with near-black ink. Night at dusk is a navy sky over an ember horizon. The backdrop is the app's dithered ridges and nothing else. Type is two grotesques, one for headlines and one for text, upright throughout. Surfaces are flat, with hairlines and small steps in tone, and the page draws a single shadow. The home page set aside what other sites of its kind do: a dark page, a shrunken dark window and a row of agent logos.

Motion is small and each piece has a job. The hiker walks his trail as he does in the app, and the Inbox example plays once to show a thread stopping for an answer and going back to work. With the script off or Reduce Motion on, both stand still and the page says the same things.

**Key Characteristics:**
- Paper by default and Night when the system is dark. There is no switch on the page.
- One accent, ember, for the action and for a thread at work. Amber and emerald mark the other two states.
- Bricolage Grotesque at 600 for headlines, Hanken Grotesk at 400 and 500 for text, JetBrains Mono for commands.
- Three pictures of the app, each a real capture, and one played example of the Inbox.
- Flat surfaces, hairlines, and corners of 10 to 16 px.
- The 12 by 16 pixel hiker on his dotted trail, in the app's own timing.
- No trackers. One request leaves the page, to GitHub's API for the latest release.

## Colors

Warm paper and near-black ink by day, navy and warm white at dusk, with ember as the only accent and the app's three state colours in the Inbox example.

Every colour is a custom property on `:root` in `assets/site.css`, with its Night value under `prefers-color-scheme: dark`. In the tokens above, a name ending in `-night` is the Night value of the name without it.

### Primary
- **Deep ember** (`accent`, #B53C0B; Night #FF7A3D): the filled button, the ring around the visitor's own build in the download list, and the words "At work" on Paper. It is darker than the app's ember so that white text on it reaches 5.8:1.
- **Darker ember** (`accent-over`, #9C3308; Night #FF8D58): the filled button under the pointer.
- **On ember** (`on-accent`, #FFFFFF; Night #1A0E06): text and the icon on the filled button.
- **Ember** (`ember`, #E85D1F; Night #FF7A3D): the turning ring of a thread at work, the focus outline, and the text selection at 30%. This is the app's ember, unchanged.

### Secondary
- **Amber** (`amber`, #C98500; Night #FFB020): a thread that waits on the visitor. The dot, an 11% wash over its row, and the approval card's 9% wash and 55% outline.
- **Amber ink** (`amber-ink`, #8A5A00): the words "Waiting: …" on Paper. At Night the amber itself is used.
- **Emerald** (`emerald`, #1E9E62; Night #3FCF8E): the dot of a finished thread.
- **Emerald ink** (`emerald-ink`, #14734A): the word "Finished" on Paper. At Night the emerald itself is used.

### Neutral
- **Paper** (`ground`, #F3EEE6; Night #0E0D12): the page. On Paper the hero's sky is this same colour.
- **Dusk sky** (`sky-night`, #0B1222): the hero's ground at Night. It runs to the Night ground between 1150 px and 1296 px from the top.
- **Raised paper** (`raised`, #FBF8F3; Night #17161C): the Inbox example's panel and the Deny and Allow buttons.
- **Ink** (`ink`, #1A1A1C; Night #F3EEE6): headlines, titles, button labels.
- **Second ink** (`ink-2`, #45403A; Night #D2CBC3): paragraphs, the lede, navigation links.
- **Muted ink** (`mute`, #6A6259; Night #A69F98): captions, the fine line under the buttons, times, the footer.
- **Hairline** (`line`, rgb(40 28 16 / 0.14); Night rgb(243 238 230 / 0.14)): every rule and outline.

Washes are made in place with `color-mix`: ink at 6% under a plain button on hover, ink at 7% behind code, ink at 42% for the trail's dots, ink at 34% for the dot of a stopped thread. The phone's body (#131214, Night #2C2B31) is set directly, outside the properties.

The lowest text contrast measured is 5.0:1 (amber ink on a waiting row). Muted ink on Paper is 5.2:1; at Night it is 7.1:1 on the sky.

### Named Rules
**The One Accent Rule.** In what the page itself draws, colour beyond ink and paper belongs to two things: the action (the filled button, the ring on the visitor's own build, the focus outline) and a thread's state (ember at work, amber waiting, emerald finished). The art brings its own ember: the cairn, the hiker's pack and the horizon at dusk.

**The Two Themes Rule.** The page has the app's two themes and no others: Paper by day and Night at dusk, chosen by the visitor's system setting. Every colour is defined in both, and each picture of the app ships in both.

**The State Ink Rule.** On Paper a state's words use its ink shade, because the dot's colour is too light for text. At Night the dot's colour serves for both.

## Typography

**Display Font:** Bricolage Grotesque (with Hanken Grotesk, system-ui, sans-serif)
**Body Font:** Hanken Grotesk (with system-ui, sans-serif)
**Label/Mono Font:** JetBrains Mono (with ui-monospace, monospace), for commands and code only

**Character:** A grotesque with some life in its shapes for headlines, over a quiet, even one for reading. The app's own interface face, Inter, appears only inside the captures.

Bricolage Grotesque and Hanken Grotesk are variable files, Latin only, served from this domain. Both are under the SIL Open Font License 1.1 with no reserved font name, and each licence text sits beside its file in `assets/fonts/`. The two are preloaded on every page. `font-synthesis: none` is set on the body, so the browser never fakes a weight or a slant.

### Hierarchy
- **Display** (600, clamp(2.5rem, 4.75vw, 4.25rem), 1.03, -0.028em): the home page's one `h1`. It reaches 68 px at 1440 px wide and sets in two lines, centred, balanced.
- **Headline** (600, clamp(2rem, 3.2vw, 2.75rem), 1.06, -0.024em): each section's `h2`, left-aligned, balanced.
- **Page title** (600, clamp(2.25rem, 4vw, 3.25rem), 1.05): the `h1` of the privacy and not-found pages.
- **Document heading** (600, 1.5rem, 1.2, -0.015em): headings inside the privacy page's text.
- **Wordmark** (Bricolage Grotesque 500, 18 px, -0.01em): "Trek" beside the cairn in the bar and the footer.
- **Title** (Hanken Grotesk 500, 17 px, 1.3): a build's name in the download list. A thread's title in the Inbox example is 500 at 16 px, and the approval card's heading is 500 at 15 px.
- **Lede** (400, 19 px, 1.5; 17 px under 860 px): the sentence under the `h1`, at most 34em wide.
- **Body** (400, 17 px, 1.55): paragraphs, at most 30 to 31em wide beside a headline. `text-wrap: pretty`.
- **Button** (500, 16 px, -0.005em; 14 px on small buttons): button labels. A file size inside a button is 400 at 13 px and 78% strength.
- **Label** (400, 14 px): the fine line under the buttons, a build's requirement, the first-launch notes, the footer, a row's second line.
- **Note** (400, 13 px): captions under pictures, times in rows, the footer's small print.
- **Mono** (JetBrains Mono 400, 13 px, 1.45): the command in the approval card. Inline code on the privacy page is 0.88em.

Numbers that change (times, sizes) use tabular figures.

### Named Rules
**The One Weight Rule.** Headlines are Bricolage Grotesque at 600. The owner's range is 500 to 700, and a light weight is never used. The wordmark, at 500, is the only other display weight on the site.

**The Upright Rule.** There are no italics. No italic file is shipped and the browser is told not to make one.

**The Plain Headline Rule.** A headline is a plain statement in one colour. On the home page each one ends with a full stop. Nothing sits above it: no label, no pill. No paragraph or list item under it opens with a bold lead-in.

## Layout

The home page is one column of full-width bands. Content sits in a centred measure of 1180 px with 24 px gutters (`--wide: min(1180px, 100% - 48px)`). Two measures are narrower because of what they hold: 1040 px for the hero's window and for the Inbox section, since the window is 1040 points wide, and 880 px for the hero's words. The privacy and not-found pages use 760 px.

- **The bar.** 68 px tall, laid over the top of the page and not fixed to the screen: the cairn and wordmark at the left, four links at 15 px with 30 px between them, and a small filled Download button.
- **The hero.** 1296 px tall. The words are centred: `h1`, the lede 20 px under it, two buttons 24 px under that with 10 px between them, and a fine line 14 px below. A 60 px strip that holds the trail starts 6 px below that, and the window sits directly under the strip, so its top edge is near 490 px and the fold cuts it. Its caption sits 22 px above the hero's foot.
- **Bands.** 120 px above and 132 px below. Sections after the first two are parted by a hairline.
- **Inbox section.** Two columns, 1fr and 1.25fr, 72 px apart, aligned at the top: words at the left, the played example at the right.
- **iOS section.** The headline at the left and its paragraph at the right in one row (1.18fr and 1fr, 72 px apart, aligned at the foot), then the whole window 64 px under them across the full measure.
- **iPhone section.** Words at the left with a three-line list between hairlines; the phone at the right, 56 px in from the edge, running 109 px off the foot of its band.
- **Download section.** The headline and its sentence in one row, then one row per build 56 px under them: name, requirement and first-launch note at the left, small buttons at the right.
- **Footer.** A hairline, the wordmark at the left, five links, then the small print at most 62em wide.

Headlines are left-aligned everywhere but the hero.

Two breakpoints:

- **Under 1240 px** the iOS window keeps its size and is cut at the right edge of the screen.
- **Under 860 px** everything is one column with 16 px gutters. The bar keeps the wordmark and Download and drops its links. The hero takes its natural height; its buttons fill the row up to 340 px; the caption moves above the window; the front ridge is not drawn. The hero's window keeps its real size, starts 16 px from the left, runs off the right edge and is cut 468 px down. The iOS picture is moved so that the Simulator panel's own corner is at the top left, 640 px tall, fading out over its last 60 px. The phone is 290 px wide, centred, and runs 98 px off the foot. Bands are 84 px above and 96 px below.

### Named Rules
**The Real Size Rule.** A window capture is set at one CSS pixel to the point: a 2x file at half its pixel size. On a screen too narrow for it the picture is cropped to the part that makes the point, and it is never scaled down.

**The Few Pictures Rule.** The home page has three pictures of the app and one played example, and no section holds more than one. The owner's rule is few pictures, each a real capture.

## Elevation & Depth

The system is flat. A surface is set apart by a hairline (`line`, as a 1 px border or a 1 px inset ring) or by one step in tone (`raised` on `ground`). The ridges give the hero its depth: two layers, the far ridges behind the window and the near ridge in front of them, each faded out at the top and the foot with a mask.

The window pictures carry the shadow macOS drew for them inside the file. Each is placed with negative margins (16 px above, 23 px at the sides, 30 px below) so that the window, not its shadow, sits on the measure.

### Shadow Vocabulary
- **Under the phone** (`box-shadow: 0 30px 60px -26px rgb(var(--shade) / 0.5), inset 0 0 0 1px rgb(255 255 255 / 0.1)`): the one soft shadow the page draws, with a faint inner edge on the phone's body. `--shade` is a warm brown (40 20 8) on Paper and black at Night.
- **Hairline ring** (`box-shadow: inset 0 0 0 1px var(--line)`): the outline of a plain button, the Inbox panel, and the Deny and Allow buttons. It is an outline, not a shadow.

### Named Rules
**The One Shadow Rule.** The page draws one shadow, under the phone. Everything else is flat: a hairline or a step in tone.

## Shapes

Corners are gently rounded and grow with the size of the thing: 10 px for small buttons, a row in the Inbox example, and the Deny and Allow buttons; 12 px for the two main buttons; 16 px for the Inbox panel and the approval card. Smaller pieces inside those step down so the corners stay concentric: 9 px for the command inside the card, 5 px for inline code, 6 px for the focus outline. The phone's body is 56 px with a 46 px screen inside 10 px of padding. A state's dot is a 9 px circle.

Nothing is a pill, and nothing has a coloured side stripe. Rules are 1 px in `line`.

The art is pixel art and is kept on its grid. The ridges are dithered on 3 px cells and drawn with `image-rendering: pixelated`. The hiker's cell is 3 CSS px, rounded to a whole number of device pixels, so no edge of him falls between two.

## Components

### Buttons
Two kinds, one filled and one plain, and a small size of each.
- **Shape:** 48 px tall with 20 px at the sides and 12 px corners; small is 36 px tall with 14 px at the sides and 10 px corners.
- **Filled:** `accent` under `on-accent`. Used for the download in the hero, Download in the bar, and "Back to Trek" on the not-found page. The hero's carries a 17 px line icon, drawn with a 1.8 stroke, 9 px before its label.
- **Plain:** `ink` text inside a hairline ring. Used for "Source on GitHub" and every link in the download list.
- **Hover / Focus:** the filled button goes to `accent-over` and the plain one takes a 6% wash of ink, both over 150 ms and only where there is a pointer that hovers. Pressed, either scales to 0.98 over 120 ms. Focus is a 2 px `ember` outline 3 px off the edge.
- **The visitor's build:** the plain button for the build that matches the visitor's computer has a 1.5 px `accent` ring in place of the hairline.

### Text links
Underlined at 34% of the text's own colour, 3 px under the baseline. The underline goes to full strength over 150 ms on hover.

### Navigation
The bar's links are `ink-2` at 15 px and go to `ink` on hover. There is no menu on narrow screens: the links are dropped and the page's own sections carry them. A "Skip to the page" link appears at the top left on keyboard focus, as a block of `ink` with text in `ground`.

### The hiker and his trail
The app's working animation, used on the hero's window and, standing, on the not-found page.
- **Drawing:** a 12 by 16 cell sprite on a canvas, five poses and eight colours, in the Paper or Night set to match the theme. A cell is 3 CSS px, so he stands 36 by 48 px. The trail is a row of round dots one cell across, four cells apart, in ink at 42%, and his feet are half a cell above the dots.
- **Walk:** a 16-frames-a-second clock moves him 16 cells a second, with 0.35 s to gather and to lose speed, a pose for every 4 cells of ground, and a 1.2 s rest at each end during which he turns. Time moves him. Scrolling only starts and stops the clock: it runs while the trail is on screen and the tab is in front.
- **Pressing him:** a click, Enter or Space stops him, and the next one sends him on. He is a button to assistive technology, named "Stop the hiker" or "Let the hiker walk on".
- **Still:** with Reduce Motion on he stands and is not a control. Without the script a 12 by 16 picture of the standing pose is shown at the start of the trail.

### The Inbox example
A played example of the app's Inbox, built in HTML and labelled "An example with staged threads". It is not presented as a picture of the app.
- **Panel:** `raised` with a hairline ring, 16 px corners and 8 px of padding, holding three rows 2 px apart.
- **Row:** 10 px corners, 11 px and 12 px of padding. First line: the state's dot, the thread's title (500, 16 px) and a time at the right (13 px, muted, tabular). Second line (14 px, muted): the state in words at 500 in the state's ink, then the project.
- **States:** at work is a 9 px ember ring with a gap that turns once every 1.1 s; waiting is an amber dot and an 11% amber wash over the row; finished is an emerald dot; stopped is a dot in ink at 34%.
- **Order:** what waits comes first, then what has finished or stopped, then what is at work. A row that changes place glides there over 380 ms on `cubic-bezier(0.23, 1, 0.32, 1)`.
- **Approval card:** 12 px under the panel, in a space kept for it so nothing below moves. 16 px corners and padding, a 9% amber wash over `raised`, a 55% amber ring. A heading, the command in mono on a 7% ink block with 9 px corners, then Deny and Allow as equal halves, 40 px tall. It comes and goes with opacity and a 6 px rise over 200 ms.
- **The play:** once, when 60% of it is on screen. Three threads are at work; after 1.6 s one stops for an approval and goes to the top; 3.6 s later Allow presses itself unless the visitor has pressed Deny or Allow first; the thread goes back to work, another finishes, then it finishes too. "Play it again" appears when it is over.
- **Still:** without the script, and with Reduce Motion on, it shows the waiting moment.

### Pictures of the app
- **Window captures:** a `picture` with a Night source and a Paper image, `width` and `height` set, no frame, border or drawn shadow. Each is followed by a caption in 13 px muted ink that says what is staged.
- **The phone:** the iPhone capture inside a drawn body: #131214 (Night #2C2B31), 56 px corners, 10 px of padding, the one soft shadow. It is 318 px wide, so the screen is shown 298 px wide, about three quarters of the phone's 390 points. This is the one picture not at real size.

### Download list
One row per build between hairlines, 20 px above and below: the name (500, 17 px), the requirement under it (14 px, muted), small plain buttons at the right with the file size inside each once GitHub has answered, and a first-launch note (14 px, `ink-2`, at most 44em) under the row where there is one.

### Plain pages
The privacy and not-found pages share the bar and the type. Their text column is 760 px, 132 px from the top. The privacy page sets its body in `ink-2` with 52 px above each document heading, bulleted lists, inline code on a 7% ink block, and one table with hairline rows that scrolls sideways when narrow. The not-found page sets a 216 px trail with the standing hiker, a headline, one sentence and one filled button, left-aligned, in the middle of at least 72% of the screen's height.

### Named Rules
**The No Pause Button Rule.** The hiker has no control beside him. Pressing the hiker stops him, and pressing him again sends him on.

**The Staged Is Said Rule.** Every picture or example with staged content says so in a line beside it.

**The Reads Whole Rule.** Without the script, and with Reduce Motion on, every word, picture and link is still there and still true: the hiker stands, the Inbox example shows the waiting moment, and every download link leads to the latest release.

## Do's and Don'ts

### Do:
- **Do** set headlines in Bricolage Grotesque at 600, upright, in one colour.
- **Do** place a window capture at real size, a 2x file at half its pixel size, and crop it on a narrow screen.
- **Do** ship every picture of the app in Paper and in Night, and define every colour in both.
- **Do** say in a line beside a picture or example when its content is staged.
- **Do** keep text at 4.5:1 or better against its ground. On Paper, write a state in its ink shade.
- **Do** keep the page whole without its script and with Reduce Motion on.
- **Do** keep what the page says about Trek within what the repository's README says.
- **Do** serve type, pictures and script from this domain. The only request elsewhere is to GitHub's API for the latest release.

### Don't:
- **Don't** set a headline lighter than 500 or heavier than 700.
- **Don't** use italics, bold lead-ins, two-tone headlines, or a pill or label above a heading.
- **Don't** use ember, amber or emerald in the page's own drawing for anything but the action and a thread's state.
- **Don't** use a mock-up, a generated image or a retouched frame as a picture of the app. The one retouch is the three title-bar buttons in the window captures.
- **Don't** scale a window capture down to fit.
- **Don't** add a pause button for the hiker.
- **Don't** add a second shadow, a pill shape or a coloured side stripe.
- **Don't** add trackers, analytics, cookies, or type or script from another domain.

## Provenance

Everything on the site is the project's own or under an open licence. Nothing is stock and no picture was made by an image model.

### What it is built from

| On the site | Where it comes from in the repository |
|---|---|
| Paper's ground (#F3EEE6) and the dusk sky (#0B1222) | The first sky stops of the Paper and Dawn backdrops in `assets/backgrounds/make_backgrounds.py`. |
| Ink (#1A1A1C) | The foreground of Trek Paper in `crates/trek-assets/assets/themes/trek.json`. |
| Ember, amber and emerald | The primary, warning and success colours of Trek Paper and Trek Night in the same theme file. `crates/trek-palette/src/lib.rs` gives them their meaning: ember for a thread at work, amber for one that waits on the user. |
| Deep ember, the two ink shades, and the other neutrals | The site's own, chosen so that text on them or in them passes 4.5:1. They are not in the app. |
| The ridges | `assets/backgrounds/make_backgrounds.py`: the same ridge function, seeds, colours, glow and 4 by 4 Bayer step on 3-pixel cells. Rendered 2880 px wide in place of 1600, with the app's picture in the middle, and 1500 px tall, in two layers. Paper uses the Paper backdrop and Night uses the Dawn backdrop. |
| The hiker and his walk | `crates/trek-mascot/src/lib.rs`: the five poses, the eight colours for each theme, and `Walk::step`, ported line for line into `assets/site.js` with the same constants (16 frames a second, 16 art pixels a second, a 0.35 s ramp, a 1.2 s rest, a pose every 4 pixels). The site draws him at twice the app's size. |
| The cairn | `assets/img/trek-mark.svg` is a copy of `assets/brand/trek-mark.svg`. |
| The icons | The app's ember icon in `assets/brand` (`trek-icon-ember.svg` and its small drawings). |

### Pictures

| File in `assets/img/` | What it is | Origin |
|---|---|---|
| `window-paper.webp`, `window-night.webp` | The hero's window. 2172 by 1492 px, shown at 1086 by 746; the window itself is 1040 by 700 and the rest is its shadow. | A real capture of the isolated test app in its demo dress: believable names and staged threads. |
| `simulator-paper.webp`, `simulator-night.webp` | The window with the Simulator tool open. 2452 by 1612 px, shown at 1226 by 806; the window is 1180 by 760. | The same app and scene, with a staged thread whose agent checks Trek's own iPhone app in the iOS Simulator. |
| `iphone-paper.webp`, `iphone-night.webp` | Trek for iPhone with an approval waiting. 1170 by 2532 px, the phone's own 3x. | A real capture of the iPhone app in its demo mode, in the iOS Simulator, in light and dark. |
| `ridges-paper-back.png`, `ridges-paper-front.png`, `ridges-dawn-back.png`, `ridges-dawn-front.png` | 2880 by 1500 px each. The back layer is the sky and the two far ridges. The front layer is the near ridge on transparency. | Drawn by the app's backdrop generator, as above. |
| `hiker-stand-paper.png`, `hiker-stand-night.png` | 12 by 16 px. Shown only when the script has not run. | The sprite's standing pose in its Paper and Night colours. |
| `og.jpg` | 1200 by 630 px, for link previews. | A render of the page's own headline and Paper window over the Paper ridges. |
| `icon-192.png`, `icon-512.png`, `icon-maskable-512.png` | The web manifest's icons. | The app's ember icon from `assets/brand`. |

The captures are regenerated from the app and are not edited by hand: one script stages the scene in the test app and takes the masters at 2x, and a second converts the masters into the files here. There is one retouch. The three title-bar buttons are painted in, in macOS's own colours and places, because the test window is never the key window and macOS draws them grey. Nothing else in any picture is changed.

Each PNG and the JPEG carries its origin inside the file, and each WebP has it in a `.json` file of the same name beside it. The design skill's `embed-prompt --read <file>` prints it, and `embed-prompt --scan assets/img` lists any picture without one.

### Type

| Face | Use | Licence |
|---|---|---|
| Bricolage Grotesque, variable, Latin | Headlines and the wordmark | SIL OFL 1.1, no reserved font name. `assets/fonts/OFL-BricolageGrotesque.txt` |
| Hanken Grotesk, variable, Latin | Text | SIL OFL 1.1, no reserved font name. `assets/fonts/OFL-HankenGrotesk.txt` |
| JetBrains Mono Regular, subset | Commands and code | SIL OFL 1.1. `assets/fonts/OFL-JetBrainsMono.txt` |

## Where the build departs from its direction

The direction for the home page is in `.impeccable/surfaces/index-html.md`. The build differs from it, or from the owner's rules, in these places. None of them is a rule for new work.

- **The phone is not at real size.** Its screen is shown 298 px wide against the phone's 390 points (270 px on a narrow screen). The window captures are at real size.
- **More than the hiker and the rows move.** The ring of a thread at work turns without stopping, the approval card fades in and out, and buttons answer a press. All of it stops with Reduce Motion.
- **States have three colours.** The direction gives ember to the action and the states; the build follows the app and uses amber for waiting and emerald for finished.
- **Some corners are under 10 px.** The command block (9 px), inline code (5 px) and the focus outline (6 px) step below the 10 to 16 px range.
- **The wordmark is at 500.** The stylesheet's opening note says the display face has one weight; the wordmark is the exception.
- **The Mac and the iPhone now show the same thread.** The direction lists this as open. In the build the iPhone's approval is "Migrate the schema", the thread the Mac's Inbox shows waiting.
