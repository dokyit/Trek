---
name: trekdesktop.com
description: "Trek's website: one photograph of a trail, a pinned stage where an example Inbox acts as you scroll, and real captures of the app at their real size."
colors:
  accent: "#B53C0B"
  accent-over: "#9C3308"
  on-accent: "#FFFFFF"
  ember: "#E85D1F"
  ember-ink: "#B53C0B"
  amber: "#C98500"
  amber-ink: "#8A5A00"
  emerald: "#1E9E62"
  emerald-ink: "#14734A"
  ground: "#F3EEE6"
  raised: "#FBF8F3"
  sky-top: "#BABACA"
  ink: "#1A1A1C"
  ink-2: "#45403A"
  mute: "#6A6259"
  line: "rgb(40 28 16 / 0.14)"
  shade: "rgb(40 20 8)"
  phone-body: "#131214"
  accent-night: "#FF7A3D"
  accent-over-night: "#FF8D58"
  on-accent-night: "#1A0E06"
  ember-night: "#FF7A3D"
  ember-ink-night: "#FF7A3D"
  amber-night: "#FFB020"
  amber-ink-night: "#FFB020"
  emerald-night: "#3FCF8E"
  emerald-ink-night: "#3FCF8E"
  ground-night: "#0E0D12"
  raised-night: "#17161C"
  sky-top-night: "#18192B"
  ink-night: "#F3EEE6"
  ink-2-night: "#D2CBC3"
  mute-night: "#A69F98"
  line-night: "rgb(243 238 230 / 0.14)"
  shade-night: "rgb(0 0 0)"
  phone-body-night: "#2C2B31"
typography:
  display:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2.6rem, 5vw, 4.5rem)"
    fontWeight: 600
    lineHeight: 1.02
    letterSpacing: "-0.03em"
  headline:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2rem, 3.2vw, 2.75rem)"
    fontWeight: 600
    lineHeight: 1.06
    letterSpacing: "-0.024em"
  caption:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(1.75rem, 3.5vw, 3rem)"
    fontWeight: 600
    lineHeight: 1.12
    letterSpacing: "-0.022em"
  caption-still:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(1.5rem, 2.6vw, 2.25rem)"
    fontWeight: 600
    lineHeight: 1.12
    letterSpacing: "-0.022em"
  page-title:
    fontFamily: "Bricolage Grotesque, Hanken Grotesk, system-ui, sans-serif"
    fontSize: "clamp(2.25rem, 4vw, 3.25rem)"
    fontWeight: 600
    lineHeight: 1.05
    letterSpacing: "-0.03em"
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
    fontSize: "clamp(1.0625rem, 1.45vw, 1.25rem)"
    fontWeight: 400
    lineHeight: 1.5
  page-lede:
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
  compact:
    fontFamily: "Hanken Grotesk, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.55
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
  code: "5px"
  focus: "6px"
  sm: "10px"
  md: "12px"
  lg: "14px"
  panel: "18px"
  phone-stage-screen: "27px"
  phone-stage: "34px"
  phone-screen: "36px"
  phone: "44px"
spacing:
  gutter: "24px"
  gutter-narrow: "16px"
  bar: "68px"
  column-gap: "72px"
  stage-gap: "44px"
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
  button-big:
    rounded: "{rounded.lg}"
    padding: "0 26px"
    height: "54px"
  button-small:
    rounded: "{rounded.sm}"
    padding: "0 14px"
    height: "36px"
  button-plain:
    textColor: "{colors.ink}"
    typography: "{typography.button}"
    rounded: "{rounded.md}"
    padding: "0 20px"
    height: "48px"
  nav-link:
    textColor: "{colors.ink-2}"
    typography: "{typography.compact}"
  nav-link-hover:
    textColor: "{colors.ink}"
  skip-link:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.ground}"
    rounded: "{rounded.sm}"
    padding: "10px 14px"
  first-screen:
    backgroundColor: "{colors.sky-top}"
    textColor: "{colors.ink}"
    typography: "{typography.display}"
  story-caption:
    textColor: "{colors.ink}"
    typography: "{typography.caption}"
  story-caption-still:
    textColor: "{colors.ink-2}"
    typography: "{typography.caption-still}"
  inbox-rows:
    backgroundColor: "{colors.raised}"
    rounded: "{rounded.panel}"
    padding: "8px"
    width: "440px"
  inbox-row:
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    padding: "12px"
  question-card:
    textColor: "{colors.ink}"
    typography: "{typography.compact}"
    rounded: "{rounded.panel}"
    padding: "16px"
  command-block:
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.sm}"
    padding: "10px 12px"
  question-answer:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.sm}"
    height: "40px"
  question-answer-allow:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.on-accent}"
  question-answer-allow-pressed:
    backgroundColor: "{colors.accent-over}"
  phone:
    backgroundColor: "{colors.phone-body}"
    rounded: "{rounded.phone}"
    padding: "8px"
    width: "210px"
  phone-night:
    backgroundColor: "{colors.phone-body-night}"
  phone-stage:
    rounded: "{rounded.phone-stage}"
    padding: "7px"
    width: "183px"
  staged-note:
    textColor: "{colors.mute}"
    typography: "{typography.note}"
  build-row:
    typography: "{typography.title}"
    padding: "20px 0"
  footer:
    textColor: "{colors.mute}"
    typography: "{typography.label}"
    padding: "40px 0 48px"
---

# Design System: trekdesktop.com

This file records what is built. It was read from `index.html`, `assets/site.css`, `assets/site.js`, `privacy/index.html`, `404.html`, `script/site/make_hero.py`, `script/site/make_hero_layers.py` and `script/site/make_icons.py` on 12 November 2026, and again for the first screen and its hand-off into the story on 13 November 2026. A size given as measured was read from the page in headless Chrome on that day. Where the build and its direction differ, the build is what is written here, and the difference is listed at the end.

## Overview

**Creative North Star: "The Trailhead"**

The site is where a visitor meets Trek before installing it. The first screen is one photograph of a trail going up a dry hill. A headline, two short sentences, one button and one fine line sit in its sky. Below the photograph the page takes the app's own ground: Paper by day, Night when the system is dark. The photograph does not end at an edge. Scrolling holds it and walks up the trail while the dusk closes on its horizon, and the story's dotted trail begins on that last line of light.

Under the first screen the page shows the app three times. First a small stage is pinned while the visitor scrolls, and an example Inbox acts on it: three threads start, one stops to ask, the phone offers the same question, the thread goes back to work, finished work waits. Then the real window is shown at its real size, twice. A list of builds and a one-line footer end the page.

Type is two grotesques, upright throughout. Surfaces are flat, with hairlines and small steps in tone. The page draws two soft shadows, both on the stage. Decorative art is a photograph or it is absent. There is no drawn scenery, and the mark is never used as scenery. The home page does not use a dark page by default, a shrunken window under the headline, a row of agent logos or a wall of copy.

Motion has one grammar. Things arrive by opacity and a short rise, most with a blur that clears. Scrolling plays the first screen and the stage forwards and backwards, as one continuous scene. Where there is a mouse, the photograph's two planes move a little apart as it moves. The hiker walks on a clock. Without the script, with Reduce Motion on, or in a window under 560 px tall, the story is a still layout that says the same things.

**Key Characteristics:**
- Paper by default and Night when the system is dark. There is no switch on the page.
- One photograph on the first screen, in two planes (sky and land): the day picture on Paper, a dusk grade of the same frame at Night. It is held while the page scrolls one more screen, and hands off to the story's trail.
- One accent, ember, for the action and for a thread at work. Amber and emerald mark the other two states.
- Bricolage Grotesque at 600 for display, Hanken Grotesk at 400 and 500 for text, JetBrains Mono for commands.
- A pinned story 430vh tall, with one caption on screen at a time.
- Two window captures at one CSS pixel to the point. One reduced capture, the story's iPhone.
- The 12 by 16 pixel hiker on a dotted trail, in the story only, in the app's own timing.
- Flat surfaces, hairlines, and corners of 10 to 18 px.
- No trackers, analytics or third-party fonts. One request leaves the page, to GitHub's API for the latest release.

## Colors

Warm paper and near-black ink by day, near-black and warm white at Night, with ember as the one accent and the app's three state colours in the story.

Every colour in the tokens above but the phone's body is a custom property on `:root` in `assets/site.css`, with its Night value under `prefers-color-scheme: dark`. A name ending in `-night` is the Night value of the name without it.

### Primary
- **Deep ember** (`accent`, #B53C0B; Night #FF7A3D): the filled button, the Allow answer in the question card, and the ring around the visitor's own build. It is darker than the app's ember so that white text on it reaches 5.8:1.
- **Darker ember** (`accent-over`, #9C3308; Night #FF8D58): the filled button under the pointer, and Allow while it is pressed.
- **On ember** (`on-accent`, #FFFFFF; Night #1A0E06): the label and the icon on a filled button.
- **Ember** (`ember`, #E85D1F; Night #FF7A3D): the turning ring of a thread at work, the focus outline, and the text selection at 30%. This is the app's ember, unchanged.
- **Ember ink** (`ember-ink`, #B53C0B; Night #FF7A3D): the words "At work" in a row.

### Secondary
- **Amber** (`amber`, #C98500; Night #FFB020): a thread that waits on the visitor. The dot, an 11% wash over its row, and the question card's 9% wash and 55% outline.
- **Amber ink** (`amber-ink`, #8A5A00; Night #FFB020): the words "Waiting: run ./migrate?" in a row.
- **Emerald** (`emerald`, #1E9E62; Night #3FCF8E): the dot of a finished thread.
- **Emerald ink** (`emerald-ink`, #14734A; Night #3FCF8E): the word "Finished" in a row.

### Neutral
- **Paper** (`ground`, #F3EEE6; Night #0E0D12): the page under the first screen.
- **Raised paper** (`raised`, #FBF8F3; Night #17161C): the Inbox panel, the Deny answer, and the base of the question card.
- **Sky** (`sky-top`, #BABACA; Night #18192B): the first screen's own background. It is seen only until the photograph has loaded. Each value is the colour of its picture's top row, and the home page's `theme-color` uses the same two.
- **Ink** (`ink`, #1A1A1C; Night #F3EEE6): headlines, captions, titles and button labels.
- **Second ink** (`ink-2`, #45403A; Night #D2CBC3): paragraphs, the lede, the fine line, the bar's links, the footer's links, and the captions in the still layout.
- **Muted ink** (`mute`, #6A6259; Night #A69F98): the line that says what is staged, a row's second line and its time, a build's requirement, the footer.
- **Hairline** (`line`, rgb(40 28 16 / 0.14); Night rgb(243 238 230 / 0.14)): every rule and outline.
- **Shade** (`shade`, rgb(40 20 8); Night rgb(0 0 0)): the colour of the two soft shadows. The stylesheet holds it as three numbers so that each shadow sets its own strength.
- **Phone body** (`phone-body`, #131214; Night #2C2B31): the drawn body around the story's iPhone, with an inner edge of white at 10%. The stylesheet sets it directly. It is not a custom property.

Washes are mixed in place with `color-mix`: ink at 6% under a plain button on hover, ink at 7% behind a command or inline code, ink at 40% for the trail's dots, ink at 30% for the scrollbar. A text link's underline is 34% of the text's own colour.

Three sets of colour are outside the tokens. The mark's colours are in `script/site/make_icons.py`. The hiker's eight colours for each theme are in `assets/site.js`. The photograph brings its own.

Contrast was computed from the tokens on 12 November 2026. The lowest pair is amber ink on a waiting row at 5.0:1. Muted ink is 5.2:1 on Paper and 5.1:1 on a waiting row. Text over the photograph was sampled from the review stills at 1440 by 900 and 390 by 844. By day the lowest is the bar's links at 5.3:1, then the lede at 6.0:1 and the fine line at 6.3:1. At Night the lowest is the fine line at 6.8:1.

### Named Rules
**The One Accent Rule.** In what the page itself draws, colour beyond ink and paper belongs to two things: the action (the filled button, Allow, the ring on the visitor's own build, the focus outline) and a thread's state (ember at work, amber waiting, emerald finished). The pictures bring their own colour: the photograph, the mark, the hiker's pack.

**The Two Themes Rule.** The page has the app's two themes and no others: Paper by day and Night when the system is dark. Every colour is defined in both, and every picture ships in both.

**The State Ink Rule.** On Paper a state's words use its ink shade, because the dot's colour is too light for text. At Night the dot's colour serves for both.

## Typography

**Display Font:** Bricolage Grotesque (with Hanken Grotesk, system-ui, sans-serif)
**Body Font:** Hanken Grotesk (with system-ui, sans-serif)
**Label/Mono Font:** JetBrains Mono (with ui-monospace, monospace), for commands and code only

**Character:** A grotesque with uneven, drawn shapes for display, over an even one for reading. The app's own interface face, Inter, appears only inside the captures.

Bricolage Grotesque and Hanken Grotesk are variable files, Latin only, served from this domain. Both are under the SIL Open Font License 1.1 with no reserved font name, and each licence text sits beside its file in `assets/fonts/`. The two are preloaded on every page. `font-synthesis: none` is set on the body, so the browser never fakes a weight or a slant.

### Hierarchy
- **Display** (600, clamp(2.6rem, 5vw, 4.5rem), 1.02, -0.03em): the home page's one `h1`, centred and balanced, in two lines. It is 72 px from 1440 px wide and 41.6 px on a 390 px phone.
- **Headline** (600, clamp(2rem, 3.2vw, 2.75rem), 1.06, -0.024em): each section's `h2`, left-aligned and balanced. It is 44 px from 1375 px wide.
- **Caption** (600, clamp(1.75rem, 3.5vw, 3rem), 1.12, -0.022em): the story's one line on the pinned stage, centred. It is 48 px from 1372 px wide and 28 px on a phone. In the still layout the five lines are clamp(1.5rem, 2.6vw, 2.25rem) in second ink.
- **Page title** (600, clamp(2.25rem, 4vw, 3.25rem), 1.05, -0.03em): the `h1` of the privacy and not-found pages.
- **Document heading** (600, 1.5rem, 1.2, -0.015em): headings inside the privacy page's text.
- **Wordmark** (Bricolage Grotesque 500, 18 px, -0.01em): "Trek" beside the mark in the bar and the footer.
- **Title** (Hanken Grotesk 500, 17 px, 1.3): a build's name in the builds list. A thread's title in a row is 500 at 16 px.
- **Lede** (400, clamp(1.0625rem, 1.45vw, 1.25rem), 1.5): the two sentences under the `h1`, at most 29em wide, in second ink. It is 17 px up to 1172 px wide and 20 px from 1380 px wide. On the privacy and not-found pages the lede is 19 px.
- **Body** (400, 17 px, 1.55): paragraphs, at most 28 to 30em wide beside or under a headline. `text-wrap: pretty`.
- **Button** (500, 16 px, -0.005em): button labels. The big button's label is 17 px and a small button's is 14 px. A file size inside a button is 400 at 13 px and 78% strength.
- **Compact** (400, 15 px): the bar's links and the privacy page's table. The question card's heading and its two answers are this size at 500.
- **Label** (400, 14 px): the fine line under the button, a build's requirement, a row's second line, the footer.
- **Note** (400, 13 px): the line under a capture, the story's staged line and the times in rows.
- **Mono** (JetBrains Mono 400, 13 px, 1.45): the command in the question card. Inline code on the privacy page is 0.88em.

Times in the rows use tabular figures.

### Named Rules
**The One Weight Rule.** Display type is Bricolage Grotesque at 600. A headline's weight stays between 500 and 700, and a light weight is never used. The wordmark, at 500, is the only other display weight on the site.

**The Upright Rule.** There are no italics. No italic file is shipped and the browser is told not to make one.

**The Plain Headline Rule.** A headline is a plain statement in one colour. On the home page each headline and each caption ends with a full stop. Nothing sits above a headline: no label and no pill. Display copy has no colon and no semicolon. No paragraph of copy opens with a bold lead-in. The example row's "Waiting: run ./migrate?" is the app's own wording and is not display copy.

## Layout

The home page is one column of full-width parts: the first screen, the story, two capture bands, the builds band and the footer. Content sits in a centred measure of 1180 px with 24 px gutters (`--wide: min(1180px, 100% - 48px)`). Three measures are narrower because of what they hold: 820 px for the first screen's words, 1040 px for the "This is Trek." band because that window is 1040 points wide, and 760 px for the reading column on the privacy and not-found pages.

- **The bar.** 68 px tall, laid over the top of the page and not fixed to the screen.
- **The first screen.** As tall as the screen and never under 720 px (`max(100svh, 720px)`, called H here). With the script the section is 1.85 H tall and the scene inside it is pinned for the extra 0.85 H, while the words scroll away over it; the story is drawn up under its end by 0.29 H plus the depth of the story's trail (441 px at 1440 by 900), so the trail's dots reach 71% of H just as the scene closes. Without the script, with Reduce Motion on or in a window under 560 px tall, it is at least H tall and not held, and grows if its words need more room (the scene and the words share one grid cell). The photograph covers it, anchored 40% across and 70% down, so the horizon holds at 70% of the height at any size. The words are centred and start between 104 and 260 px from the top (`30svh - 110px`), which is 160 px in a 900 px window.
- **The story.** With the script it is 430vh tall. Its stage is pinned for that length, 100vh tall and never under 560 px. The block of trail, rows and question card is 440 px wide and centred. The caption sits 13vh above the foot of the stage. A line that says the example is staged sits 22 px above the same foot. The phone stands 44 px to the right of the block.
- **Bands.** 120 px above and 132 px below. The two capture bands have 40 px above. The builds band opens with a hairline.
- **"This is Trek."** The headline, one sentence 16 px under it, the window 56 px under that, and a note 28 px under the window.
- **"Agents can run your iOS app."** The headline at the left and its paragraph at the right in one row (1.18fr and 1fr, 72 px apart, aligned at the foot), then the window 56 px under them across the full measure, and a note.
- **"Get Trek."** The headline, one sentence that carries the version and its date once GitHub has answered, then one row per build 52 px under it.
- **The footer.** A hairline, 40 px above and 48 px below, and one line.

Headlines are left-aligned. The first screen's words and the story's captions are centred.

Width changes:

- **Under 1240 px** the iOS window keeps its size and is cut at the right edge of the screen.
- **Under 1100 px** the "This is Trek." window does the same: it keeps its size and is cut at the right edge of the screen.
- **Under 1000 px** there is no room beside the block. The phone takes the block's place for its one caption, and the rows and the question card fade out while it shows. The trail and the hiker stay.
- **Under 860 px** everything is one column with 16 px gutters. The bar drops its two links. Bands are 84 px above and 96 px below. Each window keeps its real size, starts at the left gutter and runs off the right edge. The "This is Trek." window is cut 468 px down and fades out over its last 60 px. The iOS picture is moved so that the Simulator panel shows, 640 px tall, fading out over its last 60 px. In the builds list the buttons go under the text. The footer's copyright line takes a row of its own.

Height changes:

- **A short window** shrinks the stage's block to fit. See "Fit to a short window" under Components.
- **Under 560 px tall** the story is the still layout.

### Named Rules
**The Real Size Rule.** A window capture is set at one CSS pixel to the point: a 2x file at half its pixel size. On a screen too narrow for it the picture is cropped to the part that makes the point. It is never scaled down.

**The One Reduced Capture Rule.** The story's iPhone is the one capture that is not at real size. It is a prop beside the stage, not a product shot. Its screen is 169 px wide on the stage and 194 px wide in the still layout, against the phone's 390 points.

**The Few Pictures Rule.** The home page has one photograph, two captures of the app's window and one reduced iPhone capture. No section holds more than one.

## Elevation & Depth

The system is flat. A surface is set apart by a hairline (`line`, as a 1 px border or a 1 px inset ring) or by one step in tone (`raised` on `ground`). The first screen's depth is the photograph's own, in two planes that move a little apart. The bar has no ground of its own, so on the home page it sits on the photograph.

The page draws two soft shadows, and both belong to things that stand on the story's stage: the Inbox panel and the phone. Their colour is `shade`, a warm brown on Paper and black at Night.

The window captures carry the shadow macOS drew for them inside the file. Each is placed with negative margins (16 px above, 23 px at the sides, 30 px below) so that the window, not its shadow, sits on the measure. Where a window is cut at the edge of the screen, the shadow in its file is cut with it.

### Shadow Vocabulary
- **Under the Inbox panel** (`box-shadow: inset 0 0 0 1px var(--line), 0 24px 48px -32px rgb(var(--shade) / 0.35)`): a hairline ring and a low, wide shadow under the panel of rows.
- **Under the phone** (`box-shadow: 0 30px 60px -26px rgb(var(--shade) / 0.5), inset 0 0 0 1px rgb(255 255 255 / 0.1)`): the shadow under the phone, with a faint inner edge on its body.
- **Hairline ring** (`box-shadow: inset 0 0 0 1px var(--line)`): the outline of a plain button and of the Deny answer. It is an outline, not a shadow.

### Named Rules
**The Two Shadows Rule.** The page draws two soft shadows, under the Inbox panel and under the phone. Everything else is flat: a hairline or a step in tone.

## Shapes

Corners are gently rounded and grow with the size of the thing. The scale is 5, 6, 10, 12, 14 and 18 px. A small button, the skip link, a row in the Inbox panel, the command block and the two answers in the question card are 10 px. A button is 12 px. The big button is 14 px. The Inbox panel and the question card are 18 px. Two small pieces are smaller: inline code on the privacy page is 5 px, and the focus outline rounds a focused link to 6 px.

A row's 10 px is concentric with the Inbox panel: the panel's 18 px corner less its 8 px of padding. The pieces inside the question card take the 10 px step.

The phone is a drawing of an object and has its own corners. In each layout the screen's corner is the body's corner less the padding. In the still layout the body is 44 px with a 36 px screen inside 8 px of padding. On the stage it is 34 px with a 27 px screen inside 7 px.

A state's dot is a 9 px circle. The trail's dots are 3 px across and 12 px apart. Nothing is a pill, and nothing has a coloured side stripe. Rules are 1 px in `line`.

The hiker is pixel art and is kept on his grid. His cell is 3 CSS px at full size and he is drawn with `image-rendering: pixelated`. The script sets the cell so that one art pixel is a whole number of device pixels, also when a short window has scaled the stage's block.

## Components

### The bar
The top of every page. It has no ground of its own.
- **Left:** the mark at 26 px and the wordmark, 9 px apart, as one link to the home page.
- **Right:** two links, "How it works" and "GitHub", in second ink at 15 px and 30 px apart. Under a pointer a link goes to ink over 150 ms. Then a small filled Download button.
- **Narrow:** under 860 px the two links are dropped. There is no menu.
- **Skip link:** "Skip to the page" comes in at the top left on keyboard focus, as a block of ink with paper text and 10 px corners.

### Buttons
Two kinds, one filled and one plain, in three sizes.
- **Shape:** 48 px tall with 20 px at the sides and 12 px corners. Big is 54 px tall with 26 px at the sides and 14 px corners. Small is 36 px tall with 14 px at the sides and 10 px corners.
- **Filled:** `accent` under `on-accent`. Big for the download on the first screen, with a 17 px line icon drawn with a 1.8 stroke 9 px before its label. Small for Download in the bar. Standard for "Back to Trek" on the not-found page.
- **Plain:** `ink` text inside a hairline ring. The build uses it small, for every link in the builds list.
- **Hover / Focus:** a filled button goes to `accent-over` and a plain one takes a 6% wash of ink, both over 150 ms and only where there is a pointer that hovers. Pressed, either scales to 0.98 over 120 ms. Focus is a 2 px `ember` outline 3 px off the edge.
- **The visitor's build:** the plain button for the build that matches the visitor's computer has a 1.5 px `accent` ring in place of the hairline.

### Text links
Underlined at 34% of the text's own colour, 3 px under the baseline. The underline goes to full strength over 150 ms on hover. Used for the footer's links and for the "Take the Intel build" link the script adds to the fine line when it cannot tell which Mac it is on. Links in the privacy page's text carry the same underline without the hover change.

### The mark
A faceted mountain with a lit face and a shaded face, a dotted trail of nine dots that climbs to the summit, and a flag on the summit. It sits on a tile with rounded corners (22 of its 96 units).
- **Two dresses:** on a Paper page the tile is ink (#1A1A1C), the mountain is #FF7A3D with a #C9551F shaded face, and the flag is #FFC7A0. In dark mode the tile is raised paper (#FBF8F3), the mountain is #E85D1F, and the shaded face and the flag are #B8461A. The tile always stands against the page.
- **Where:** at 26 px beside the wordmark in the bar and the footer, and as the favicon, the touch icon, the manifest's icons and the share image's logo. Nowhere else.
- **Small sizes:** each is drawn for its size. The favicon and the 32 px icon have four larger dots and a larger flag. The 16 px icon has no dots. The touch icon and the maskable icon use a square tile.
- **Source:** `script/site/make_icons.py` writes every file from one set of paths.

### The first screen
One photograph and a few words in its sky.
- **The photograph:** two planes cut from it along the ridge, the sky (with the far hills) and the land (the near hill and its trail, with a transparent sky). They lie exactly over each other, each filling the screen with `object-fit: cover` at 40% across and 70% down, and are scaled 1.03 so a small shift never shows an edge. Each ships 1600 and 2880 px wide, chosen by `sizes="max(100vw, 160vh)"`, with a Night source for dark mode. They are decoration and have an empty `alt`.
- **The pointer:** where there is a mouse (`hover: hover` and `pointer: fine`), the land moves up to 14 px across and 7 px down against the pointer and the sky up to 5 and 3 px, as a near hill moves against the sky. The view follows the mouse over about a second (an exponential ease of 0.45 s) and settles back when the mouse leaves the page. Touch does nothing.
- **The walk up the trail:** over the held 0.85 H of scroll the land scales from 1.03 to 1.30 and the sky to 1.08, both around the point where the trail meets the ridge (41% across, 71% down), so the camera draws nearer the crest. The scale eases out (`1 - (1 - q)²` of the scroll), so the hill is seen to come nearer from the first pixel of scroll, before the dusk reaches it.
- **The dusk:** the page's ground comes up from below and down from above, each led by a soft edge that clears along a smoothstep (eleven stops of `color-mix` with transparent). At rest the lower one is the foot of the picture, its edge 14% of H tall, and that is also the whole of the still state. Scrolling moves both in step with the scroll: the lower to 70.4% of H and the upper to 71.6%, so they overlap and the scene is all ground when it closes. Their soft edges are widest half way (about a quarter of H) and sharpen to 1% of H as they close, so the photograph keeps its colour until the edges reach it and a thin line of horizon stays lit to the end: afterglow at Night, pale pink haze by day. Over the last quarter of the walk two more blocks of ground close in from the sides, each with an 80 px soft edge, until the lit line is only as wide as the story's trail. All of it is `transform` (a translate on each block and a `scaleY` on each soft edge).
- **The hand-off:** the story's dots fade in over the last eighth of the walk, rising into the short closing line and reaching it as it goes; the Inbox panel follows from 97% of the walk to just past its end; the hiker steps on only once the scene has closed, over the next 0.085 H of scroll. Scrolling back plays it in reverse.
- **The words:** the `h1`, the lede 22 px under it, one big filled button 30 px under that, and a fine line 14 px under the button at 14 px in second ink.
- **The button:** the script names the visitor's system on it ("Download for macOS" or "Download for Windows") and adds the version, the chip and the file's size to the fine line. On a phone or on Linux the button says "See the downloads" and goes to the builds list, and the fine line says why. Without the script the button says "Download for macOS", leads to the latest release, and the fine line says "Free and open source."
- **Entrance:** on load the four pieces rise once. Each goes from clear to solid, comes up 14 px and loses an 8 px blur over 700 ms on `--out`. They start 0, 90, 180 and 260 ms in. This is CSS, so it runs without the script. It is off with Reduce Motion.
- **Nothing else:** no hiker, no capture, no label above the headline, and nothing drawn: the dusk is the page's own ground colour.

### The story
One stage, pinned while the page scrolls past it. Scrolling plays it, forwards and backwards. It is built in HTML from the app's own parts and is not a capture. A visible line says so, and its list is named "An example Inbox" for assistive technology.

**The moments.** The scroll through the story is cut at 18%, 40%, 60% and 80%.

| Share of the scroll | Caption | What the stage shows |
|---|---|---|
| 0 to 18% | Start a few agents. | Three threads at work. |
| 18 to 40% | One stops to ask. | "Migrate the schema" waits and goes to the top. The question card opens under the rows. |
| 40 to 60% | Answer on your computer or your phone. | The same, and the phone comes up with the same question. |
| 60 to 80% | It gets back to work. | Allow is pressed, the card goes, and the thread is at work again in its old place. |
| 80 to 100% | Finished work waits for you. | Two threads have finished and sit at the top. One is still at work. |

The played position follows the scroll position with easing: each frame it moves 16% of the way there.

**Rows.** The example Inbox.
- **Panel:** `raised` with a hairline ring, the shadow under the Inbox panel, 18 px corners and 8 px of padding. It holds three rows 2 px apart and is 440 px wide.
- **Row:** 12 px of padding and 10 px corners. First line: the state's dot, the thread's title (500, 16 px) and a time at the right (13 px, muted, tabular). Second line (14 px, muted): the state in words at 500 in the state's ink, then the project. A row names its state in weight 500 because the app's row does. That is a picture of the app's row, not a bold lead-in.
- **States:** at work is a 9 px ember ring with a gap that turns once every 1.1 s. Waiting is an amber dot and an 11% amber wash over the row. Finished is an emerald dot. A wash or a dot changes colour over 200 ms.
- **Order:** what waits comes first, then what has finished, then what is at work. A row that changes place glides there over 420 ms on `--out`. Nothing jumps.
- **Arrival:** the panel waits for the trail's dots (above), then the rows gather as the stage comes up the screen. Each comes in by opacity, a 26 px rise and a 7 px blur that clears, starting after the row above it.

**Question card.** What the waiting thread asks.
- **Shape:** 12 px under the panel, in a space kept for it so nothing below moves. 18 px corners, 16 px of padding, a 9% amber wash over `raised`, and a 55% amber ring.
- **Parts:** a heading (500, 15 px), the command in mono on a 7% ink block, then Deny and Allow as equal halves, 40 px tall. The block and the two answers have 10 px corners. Deny is `raised` in a hairline ring. Allow is `accent` under `on-accent`. They are drawn, not buttons: the visitor does not press them.
- **Motion:** the card comes and goes over 260 ms with opacity, a 10 px rise and a 6 px blur. Going forward into "It gets back to work." Allow is pressed first. It scales to 0.95 over 140 ms and goes to `accent-over`, and the stage settles 220 ms after the press.

**Phone.** The story's iPhone, the one reduced capture.
- **Body:** a drawn body around the real capture, in `phone-body`, with the shadow under the phone.
- **On the stage:** 183 px wide with 7 px of padding, 34 px corners and a 27 px screen corner. It stands 44 px to the right of the block with its foot level with the block's foot, so it never reaches the caption.
- **Motion:** scroll brings it up between 40% and 47% and takes it away between 58% and 63%. It comes by opacity with a 70 px rise and a 4 degree turn that straightens.
- **Under 1000 px wide:** it is centred over the block, rises 60 px and does not turn. The rows and the question card fade out as it fades in. The trail and the hiker stay.

**Captions.** One line at a time.
- **Place:** a box 2.4em tall, 13vh above the foot of the stage, centred.
- **Word by word:** the script splits each line into words. A word starts clear, 10 px low and blurred by 9 px. It arrives over 520 ms on `--out`. The first word starts 170 ms after the change, so the line before has gone, and each next word starts 55 ms later.
- **Leaving:** all the words of a line go together over 180 ms.
- **First line:** a line is said only once the stage has all but arrived, so the first one is not over before it is seen.

**The staged line.** One line, "An example, with staged threads.", in 13 px muted ink, centred.
- **On the stage:** 22 px above the foot of the stage, for the whole of the story.
- **In the still layout:** under the captions, 72 px below them.

**The hiker and his trail.** The app's working animation. On the site he walks only here.
- **Trail:** a strip 54 px tall at the top of the block, 14 px in from each side of the panel, with a row of dots along its foot in ink at 40%. The trail's foot is the panel's top edge. The dots and the hiker fade separately, so the dots can arrive on the first screen's last line of light before he steps on.
- **Drawing:** a 12 by 16 cell sprite on a canvas, five poses and eight colours, in the Paper or Night set to match the theme. At full size a cell is 3 CSS px, so he stands 36 by 48 px, with his feet 5 px above the foot of the trail.
- **Walk:** a 16-frames-a-second clock moves him 16 cells a second. He takes 0.35 s to gather and to lose speed, takes a pose for every 4 cells of ground, and rests 1.2 s at each end, where he turns. Time moves him. Scroll never sets his place. It only starts and stops the clock, which runs while the trail is on screen and the tab is in front.
- **Pressing him:** a click, Enter or Space stops him, and the next one sends him on. He is a button to assistive technology, named "Stop the hiker" or "Let the hiker walk on".
- **Still:** with Reduce Motion on he stands at the start of the trail and is not a control. Without the script a 12 by 16 picture of the standing pose is shown there.

**Fit to a short window.** The script measures the room above the caption and scales the block from its top centre so the two never meet. The scale (`--k`) runs from 1 down to 0.62. It measured 0.81 in a window of 1366 by 650. The hiker's cell follows the scale: at 0.81 one art pixel measured 2 device pixels on a 1x screen and 5 on a 2x screen.

**The still layout.** Used without the script, with Reduce Motion on, and in a window under 560 px tall.
- **What it shows:** the asking moment. "Migrate the schema" waits at the top, two threads are at work, and the question card is open. The phone stands under the card, 210 px wide, 24 px below it and centred. All five captions are listed under it in second ink, 6 px apart, and the staged line follows them.
- **Spacing:** not pinned. 110 px above and 96 px below, with 44 px between the block and the captions.
- **What still moves:** without the script the ring of a thread at work turns and the first screen's entrance plays, because both are CSS. In a short window the hiker still walks. With Reduce Motion on nothing moves.

### Capture figures
The app's real window, twice.
- **Markup:** a `picture` with a Night source and a Paper image, `width` and `height` set, loaded lazily. No frame, no border and no drawn shadow.
- **"This is Trek.":** 2172 by 1492 px, shown at 1086 by 746. The window itself is 1040 by 700.
- **"Agents can run your iOS app.":** 2452 by 1612 px, shown at 1226 by 806. The window itself is 1180 by 760.
- **The note:** each capture is followed, 28 px under it, by one line in 13 px muted ink that says what is staged.

### Builds list
One row per build between hairlines, with 20 px above and below.
- **Left:** the build's name (500, 17 px), and under it in 14 px muted ink what it needs and what the first launch asks.
- **Right:** small plain buttons 8 px apart. Each carries the file's size once GitHub has answered. The visitor's own build has the `accent` ring.
- **Rows:** macOS, Windows, iPhone. The iPhone row's one button leads to the build instructions.

### Footer
One line at 14 px in muted ink between a hairline and the foot of the page: the mark and wordmark in ink, the copyright line, then four text links in second ink at the right. It has no small print and no font credit. The not-found page has no footer.

### Reading column
The privacy and not-found pages share the bar and the type. Their column is 760 px wide, 132 px from the top and 120 px from the foot (108 and 88 px under 860 px).
- **Head:** the page title, and a 19 px lede in second ink 20 px under it.
- **Privacy:** the text starts 56 px under the lede. A document heading has 52 px above it. Paragraphs and lists are 14 px apart in second ink. Lists are bulleted. Inline code is 0.88em mono on a 7% ink block with 5 px corners. One table at 15 px has hairline rows and scrolls sideways when the screen is narrow. A "Last updated" line in 14 px muted ink ends it.
- **Not found:** the headline, one sentence and one filled button, left-aligned, in the middle of at least 72% of the screen's height.

### Named Rules
**The Story Only Rule.** The hiker walks on the story's dotted trail and nowhere else. He is never in the first screen, and steps on only after its photograph has closed.

**The Last Light Rule.** The photograph never ends at an edge. It ends as a line of light on its own horizon, made of the page's ground closing over it, and the story's trail begins on that line. Anything that moves in the hand-off moves by transform or opacity.

**The Clock Not Scroll Rule.** Time moves the hiker. Scroll starts and stops his clock and never sets his place.

**The No Pause Button Rule.** The hiker has no control beside him. Pressing the hiker stops him, and pressing him again sends him on.

**The One Line Rule.** On the pinned stage one caption is on screen at a time, and it arrives word by word.

**The Photograph Or Nothing Rule.** Decorative art is a photograph or it is absent. The hiker and his dotted trail are the app's own working animation, not scenery.

**The Mark Is Not Scenery Rule.** The mark is a logo beside the name and an icon. It is never enlarged, repeated or laid behind anything.

**The Staged Is Said Rule.** A capture or an example with staged content says so in a visible line under it.

**The Reads Whole Rule.** Without the script, with Reduce Motion on, and in a short window, every word, picture and link is still there and still true: the story is the still layout, the hiker stands or walks his trail, and every download link leads to the latest release.

## Do's and Don'ts

### Do:
- **Do** set display type in Bricolage Grotesque at 600, upright, in one colour, and end a home-page headline with a full stop.
- **Do** use a photograph for decorative art, or use nothing.
- **Do** show a window capture at real size, a 2x file at half its pixel size, and crop it on a narrow screen.
- **Do** ship every picture in Paper and in Night, and define every colour in both.
- **Do** say in a visible line under a capture or an example when its content is staged.
- **Do** keep the hiker on the story's dotted trail, on his own clock.
- **Do** keep text at 4.5:1 or better against its ground. On Paper, write a state in its ink shade.
- **Do** keep the page whole without its script, with Reduce Motion on, and in a short window.
- **Do** keep the site's words few, and within what the repository's README says.
- **Do** write the origin of every new or retaken picture into the file or into the `.json` file beside it.
- **Do** check a new mark against competitors' marks and popular app icons before it is shown.
- **Do** serve type, pictures and script from this domain. The only request elsewhere is to GitHub's API for the latest release.

### Don't:
- **Don't** put the hiker in the first screen, or anywhere but the story.
- **Don't** add a pause button for the hiker. Pressing the hiker stops him.
- **Don't** set the hiker's place by scroll.
- **Don't** use the mark as scenery.
- **Don't** draw scenery.
- **Don't** use a letter as the mark.
- **Don't** make the mark an orange tile with a white figure.
- **Don't** copy a competitor's site or mark, or leave room for anyone to say so.
- **Don't** set a headline lighter than 500 or heavier than 700.
- **Don't** use italics, bold lead-ins, a two-tone headline, or a pill or label above a headline.
- **Don't** use a colon or a semicolon in display copy, or write a balanced "X, and Y" line. The copy must not read as machine-written.
- **Don't** credit the fonts in the footer, or add words to it.
- **Don't** use ember, amber or emerald in the page's own drawing for anything but the action and a thread's state.
- **Don't** use a mock-up, a generated image or a retouched frame as a picture of the app. The one retouch is the three title-bar buttons in the window captures.
- **Don't** scale a window capture down to fit. The story's iPhone is the one reduced capture.
- **Don't** add a third soft shadow, a pill shape or a coloured side stripe.
- **Don't** add trackers, analytics, cookies, or type or script from another domain.

## Provenance

Everything on the site is the project's own or under an open licence. No picture was made by an image model.

### What it is built from

| On the site | Where it comes from |
|---|---|
| Paper's ground (#F3EEE6) | The first sky stop of the Paper backdrop in `assets/backgrounds/make_backgrounds.py`. |
| Ink (#1A1A1C) | The foreground of Trek Paper in `crates/trek-assets/assets/themes/trek.json`. |
| Ember, amber and emerald | The primary, warning and success colours of Trek Paper and Trek Night in the same theme file. |
| Deep ember, the state inks and the other neutrals | The site's own. They are not in the app. |
| The two sky colours | The top row of the day picture and of the Night picture. On 12 November 2026 each token equalled the mean colour of that row. |
| The photograph | "Dried grass field under blue sky during daytime" by Chris Mongeau, from Unsplash under the Unsplash License (https://unsplash.com/photos/ZRdSZeVKBaE). `script/site/make_hero.py` composes the day picture and grades the Night one from it. |
| The hiker and his walk | `crates/trek-mascot/src/lib.rs`: the five poses, the eight colours for each theme, and `Walk::step`, ported into `assets/site.js` with the same constants (16 frames a second, 16 art pixels a second, a 0.35 s ramp, a 1.2 s rest, a pose every 4 pixels). The site draws him at twice the app's size. |
| The mark | Drawn as SVG paths in `script/site/make_icons.py`. It is not a copy of `assets/brand/trek-mark.svg`, which still held the earlier mark on 12 November 2026. |

### Pictures

| File | What it is | Origin |
|---|---|---|
| `assets/img/hero-sky-day-2880.webp`, `hero-sky-day-1600.webp`, `hero-land-day-2880.webp`, `hero-land-day-1600.webp` | The first screen's photograph by day, as two planes. 2880 by 1800 px and 1600 by 1000 px; the land has alpha. | The photograph above, scaled to 2880 px wide. 250 rows of sky are added above it from its own top row, the wisps of cloud behind the headline are softened, the colour is lifted slightly, and the foot is cropped so the horizon sits near 70% of the height (`make_hero.py`). `make_hero_layers.py` then cuts it along the ridge by the warmth of the grass: the land keeps the photograph's pixels with the sky made transparent, and the sky's strip behind the ridge is filled from the sky just above it. Nothing is painted in. |
| `assets/img/hero-sky-night-*.webp`, `hero-land-night-*.webp` | The first screen's picture at Night, as the same two planes in the same two sizes. | A dusk grade of the day picture, cut along the same ridge. Every pixel is the photograph's, multiplied by a colour that changes with height. It is not a photograph taken at dusk. |
| `assets/img/window-paper.webp`, `window-night.webp` | The window under "This is Trek." 2172 by 1492 px, shown at 1086 by 746. | A real capture of the isolated test app in its demo dress: believable names and staged threads. |
| `assets/img/simulator-paper.webp`, `simulator-night.webp` | The window with the Simulator tool open. 2452 by 1612 px, shown at 1226 by 806. | The same app and scene, with a staged thread whose agent checks Trek's own iPhone app in the iOS Simulator. |
| `assets/img/iphone-paper.webp`, `iphone-night.webp` | Trek for iPhone with an approval waiting. 1170 by 2532 px, the phone's own 3x. Shown reduced in the story. | A real capture of the iPhone app in its demo mode, in the iOS Simulator, in light and dark. |
| `assets/img/hiker-stand-paper.png`, `hiker-stand-night.png` | 12 by 16 px. Shown only when the script has not run. | The sprite's standing pose in its Paper and Night colours. |
| `assets/img/og.jpg` | 1200 by 630 px, for link previews. | The day photograph with the page's headline and the mark, rendered from an HTML page in headless Chrome. |
| `assets/img/trek-mark.svg`, `trek-mark-paper.svg`, `favicon.svg`, `favicon.ico`, `apple-touch-icon.png`, `assets/img/icon-192.png`, `icon-512.png`, `icon-maskable-512.png` | The mark, in its two dresses and at each icon size. | Written by `script/site/make_icons.py`. |

The window captures are regenerated from the app and are not edited by hand. One script stages the scene in the test app and takes the masters at 2x, and a second converts the masters into the files here. There is one retouch. The three title-bar buttons are painted in, in macOS's own colours and places, because the test window is never the key window and macOS draws them grey.

Each PNG and the JPEG carries its origin inside the file, and each WebP has it in a `.json` file of the same name beside it. From the `site` folder, the design skill's `impeccable embed-prompt --read <file>` prints it, and `impeccable embed-prompt --scan .` lists any picture without one. On 12 November 2026 the scan found 17 pictures and none without an origin.

### Type

| Face | Use | Licence |
|---|---|---|
| Bricolage Grotesque, variable, Latin | Headlines, captions and the wordmark | SIL OFL 1.1, no reserved font name. `assets/fonts/OFL-BricolageGrotesque.txt` |
| Hanken Grotesk, variable, Latin | Text | SIL OFL 1.1, no reserved font name. `assets/fonts/OFL-HankenGrotesk.txt` |
| JetBrains Mono Regular, subset | Commands and code | SIL OFL 1.1. `assets/fonts/OFL-JetBrainsMono.txt` |

## Where the build departs from its direction

The direction for the home page is in `.impeccable/surfaces/index-html.md`. The build differs from it in three places. None is a rule for new work.

- **States have three colours.** The direction gives ember to the action and the states. The build follows the app and uses amber for waiting and emerald for finished.
- **A little more moves than the direction lists.** Its list of what moves does not name the question card or the pressed Allow. The build brings the card in and out and presses Allow, and buttons and links answer a hover or a press.
- **The first screen moves.** The direction has the photograph still. At the owner's request (13 November 2026, after he compared the cut from the photograph to the story with sites whose scroll flows) its two planes move with the mouse, it is held while the dusk closes on its horizon, and the story's trail begins on that line. The direction's motion grammar was brought up to date in the same change.
