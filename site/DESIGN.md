---
name: trekdesktop.com
description: "Trek's website: one afternoon while the agents work, from a grass hill in daylight to dusk in the grass, with the real app doing each job on the way."
colors:
  sky: "#C9C1DE"
  sky-2: "#DCD6EA"
  ground: "#F1EFF6"
  panel: "#E4DFEF"
  panel-2: "#D6CFE6"
  raised: "#FBFAFD"
  ink: "#1F1714"
  ink-2: "#463C47"
  mute: "#5E5466"
  line: "rgb(70 50 100 / 0.14)"
  board: "#EE5A24"
  board-hover: "#F26A37"
  board-ink: "#1F1714"
  post: "#1F1714"
  shade: "rgb(31 23 20)"
  sky-night: "#1D1A2C"
  sky-2-night: "#25213A"
  ground-night: "#13111C"
  panel-night: "#1F1B2E"
  panel-2-night: "#2A2540"
  raised-night: "#1B1828"
  ink-night: "#F4EFF7"
  ink-2-night: "#D2C9DD"
  mute-night: "#A89FB6"
  line-night: "rgb(214 200 255 / 0.13)"
  board-night: "#FF7A3D"
  board-hover-night: "#FF8A52"
  board-ink-night: "#1D1A2C"
  post-night: "#F6F1EA"
  shade-night: "rgb(0 0 0)"
typography:
  display:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.9rem, 7.4vw, 5.75rem)"
    fontWeight: 680
    lineHeight: 1.02
    letterSpacing: "-0.022em"
  headline:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.3rem, 4.6vw, 3.9rem)"
    fontWeight: 650
    lineHeight: 1.04
    letterSpacing: "-0.02em"
  title-large:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2rem, 3.4vw, 2.9rem)"
    fontWeight: 650
    lineHeight: 1.1
    letterSpacing: "-0.02em"
  title:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.55rem, 2.3vw, 2rem)"
    fontWeight: 650
    lineHeight: 1.1
    letterSpacing: "-0.02em"
  title-small:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.4rem"
    fontWeight: 650
    lineHeight: 1.08
    letterSpacing: "-0.02em"
  wordmark:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "27px"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.035em"
  lede:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(18px, 1.6vw, 21px)"
    fontWeight: 400
    lineHeight: 1.45
  body:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.55
  label:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "16px"
    fontWeight: 600
    lineHeight: 1
  caption:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.55
  mono:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "0.88em"
    fontWeight: 400
rounded:
  code: "5px"
  focus: "8px"
  small: "10px"
  button-small: "11px"
  shot: "12px"
  button: "14px"
  tile: "16px"
  panel: "28px"
spacing:
  gutter: "clamp(16px, 5vw, 72px)"
  wide: "1180px"
  panel-pad: "clamp(24px, 4vw, 56px)"
  row-gap: "clamp(28px, 4vw, 64px)"
  feature: "clamp(72px, 9vw, 120px)"
  section: "clamp(96px, 12vw, 160px)"
components:
  button-ember:
    backgroundColor: "{colors.board}"
    textColor: "{colors.board-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.button}"
    height: "56px"
    padding: "0 24px 0 20px"
  button-ember-hover:
    backgroundColor: "{colors.board-hover}"
    textColor: "{colors.board-ink}"
  button-ink:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.ground}"
    typography: "{typography.label}"
    rounded: "{rounded.button-small}"
    height: "40px"
    padding: "0 16px"
  button-quiet:
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.button}"
    height: "56px"
    padding: "0 20px"
  button-line:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.button-small}"
    height: "40px"
    padding: "0 13px"
  panel:
    backgroundColor: "{colors.panel}"
    rounded: "{rounded.panel}"
    padding: "{spacing.panel-pad}"
  shot:
    backgroundColor: "{colors.raised}"
    rounded: "{rounded.shot}"
  menubar-tile:
    backgroundColor: "{colors.raised}"
    rounded: "{rounded.tile}"
    padding: "14px"
    size: "44px"
---

# Design System: trekdesktop.com

## Overview

**Creative North Star: "The Signpost"**

The site is the place a visitor meets Trek before installing it. The home page is one afternoon passing while the agents work: it opens in daylight on a grass hill (a photograph), the light warms to gold and then rose as you read down, and it closes at dusk down in the grass (a second photograph), the hour the work is handed over and left running; at Night it runs from blue hour to full night. The first screen has one brand moment: the signpost (one post, two pointed boards) over the hill, its boards swinging in and settling once above the line "Every agent. One trail." Under it the real app window stands on the hill, tilted back, and comes up flat as the page scrolls. After that the page is the app doing each job, one feature to a row, and the rows change their arrangement from one to the next: a split, a wide band, a pair, a strip of three, an iPhone band, a flipped split, a wide band again. Then where each build is, and the end.

The world is soft and lit from above. A lavender sky field runs into a pale lilac ground by day; by night it is a deep indigo field. Ember belongs to the signpost's boards and the one action that matters. Everything else is ink on lilac, with hairlines tinted lilac and never grey. Panels are pieces of the sky the app is shown against; the captures inside them carry their own colour. Type is two grotesques, upright throughout, at a confident display weight.

The page refuses buttons or feature rows drawn as boards on a stick, a dark page with a shrunken window under the headline, and a grid of same-size feature cards.

**Key Characteristics:**
- One signpost moment in the first screen; the mark is brand, never scenery or a layout device.
- Real captures of the app, large and sharp, in soft lifted frames with the app's own corner radius.
- A row rhythm that changes from row to row on a 12-part grid.
- Ember only on the boards, the main button and the visitor's own build.
- Every colour defined for day and night; every picture shipped as Paper and Night where the app has both.
- Reads whole without script and with Reduce Motion on.

## Colors

A cool lavender-and-lilac field with warm ink type and one ember accent.

### Primary
- **Signpost Ember** (board, board-night): the signpost's two boards, the filled Download button, the ring around the visitor's own build, and a 30% tint for text selection. Its hover (board-hover) is a step lighter. Text on it is the board ink: warm ink by day, the indigo sky by night.

### Neutral
- **The afternoon** (home page only, `.home`): one fall of light from the top of the page to its end, under a grain (`assets/img/backdrop/grain.svg`, soft-light) that also lies over both photographs, so no field is ever flat and the texture never changes at a join. Paper: #EFEDF4, #F2EEF1, #F6EBE1, #F5DFCF, #EBCDC5, #C9A3AE (at 0, 24, 46, 70, 88 and 100% of the page). Night: #15132A, #1A1C3A, #141936, #0D1227, #080B19, #05070F. Panels on it are tinted glass (white at 42% to the ink at 7% by day, lilac at 7% to 2.5% at night), not slabs of colour. The theme colour is the hill's sky: #B3B3C7, and #0E1227 at night.
- **Lavender Sky** (sky, sky-2): the privacy and not-found pages' field.
- **Lilac Ground** (ground): the page itself below the first screen's ground line; also the text colour on ink buttons.
- **Panel Lilac** (panel, panel-2): the feature panels, as a 170 degree gradient from panel-2 into panel.
- **Raised White** (raised): the face of a capture frame, the line button and the menu bar tiles.
- **Warm Ink** (ink): headlines, body text, the post of the mark by day, the dark Download button in the bar, the focus outline.
- **Second Ink** (ink-2): ledes, paragraphs under headings, nav links at rest, the footer.
- **Muted Ink** (mute): captions such as the staged-content line, the "Last updated" line and the copyright.
- **Lilac Hairline** (line): every border and inset ring; the bar's bottom edge once it is stuck.
- **Shade** (shade): the tint inside every soft shadow, the ink by day and black by night, always at a stated opacity.
- **Cream Post** (post-night): the post of the mark at night.

### Named Rules
**The Ember Is Spoken Once Rule.** Ember is the boards, the one primary button on a screen, and the visitor's own build. It is never a background band, a heading colour or a decorative stroke.

**The Lilac Line Rule.** Borders are the lilac hairline or ink mixed into transparency (10% on the ground line, 22% over a build). Never a neutral grey.

**The Two Dresses Rule.** Every token has a night value under `prefers-color-scheme: dark`. A new colour is not done until it has both.

## Typography

**Display Font:** Bricolage Grotesque (with ui-sans-serif, system-ui)
**Body Font:** Hanken Grotesk (with ui-sans-serif, system-ui)
**Label/Mono Font:** JetBrains Mono, for inline code on the privacy page only

**Character:** A characterful display grotesque at 650 to 700, tracked in just enough that no two letters touch (-0.022em on the display line, -0.02em on every other heading), over a calm, clear text grotesque. Both upright; no italics anywhere.

### Hierarchy
- **Display** (680, clamp to 5.75rem, 1.02): the first screen's headline only.
- **Headline** (650, clamp to 3.9rem, 1.04): a section's headline: the features head, Trek for iPhone, Get Trek.
- **Title large** (650, clamp to 2.9rem): a feature that has a split row to itself.
- **Title** (650, clamp to 2rem, 1.1): other feature headings. Title small (1.4rem) in the strip, 1.35rem in the build list.
- **Wordmark** (700, 27 px, -0.035em): the lowercase "trek" in the lockup; 22 px in the footer.
- **Lede** (400, clamp 18 to 21 px, 1.45): the sentence under the display headline, at most 30ch. Section ledes are 19 px, feature text 17 to 18 px at most 40 to 48ch, all in second ink.
- **Body** (400, 17 px, 1.55): everything else.
- **Label** (600, 16 px; 17 px on the big button, 15 px on the small): buttons. A file size inside a button is 500, 13 px, tabular, at 72% opacity.
- **Caption** (14 px; 13 px under the hero window): fine print and captions, in second or muted ink. The line under the first screen's buttons sets its numbers tabular and keeps room for two lines, so the version and size GitHub sends move nothing.

The page and privacy headline is 650 at clamp(2.25rem, 4vw, 3.25rem), 1.05. Privacy paragraphs stop at 70ch. Headings use `text-wrap: balance` and paragraphs `text-wrap: pretty`. Apostrophes in the page's words are typographic (’). `font-synthesis: none` is set, so the browser never fakes a weight or a slant.

### Named Rules
**The Plain Headline Rule.** A headline is a plain statement in one colour, in short words. Nothing sits above it: no label, no pill, no eyebrow. Display copy has no colon and no semicolon. No paragraph opens with a bold lead-in.

**The Own Type Rule.** The three faces are variable or subset woff2 files served from this domain, each under the SIL Open Font License 1.1 with its licence text beside it in `assets/fonts/`. Bricolage and Hanken are preloaded on every page. No font from another domain, and no system face for display.

## Layout

Content sits in a column of at most 1180 px plus a fluid gutter (16 to 72 px) on each side; the bar, the sections and the footer all use the same column. Feature rows use fractions of twelve: a split is 4 to 8 (text, then panel), a flipped split 7 to 5 (panel, then text), a pair of cards 7 to 5 or 5 to 7 with 24 px between, a wide feature puts its heading and paragraph on one 4 to 8 baseline row above a full-width panel, and the strip is three equal columns on the ground itself. Sections are clamp(96px, 12vw, 160px) apart; features within a section clamp(72px, 9vw, 120px). The first screen is centred; everything after it is left-aligned.

The first screen stands on the hill: the photograph fills it down to a fixed depth above the section's foot (clamp(240px, 30vw, 400px), 120 px on a phone), framed wide on a desktop and tall on a phone, and its land fades out along the hill's fall (two overlapping mask ramps, no level line). The window (1086 px) stands in front of it. The footer stands on the dark ground of the dusk photograph, which rises out of the page's light behind the last section.

Under 1024 px splits become one column (a flipped split puts its text first), the strip becomes two columns, the iPhone band stacks, and the build list goes to two columns. Under 768 px the bar drops its links, pairs and the strip go to one column, panels keep their 28 px corners with 16 px padding (the capture's 12 px plus the padding, so the corners are concentric), the hero window stays 680 px wide from the left gutter and runs off the right edge, and the build list is one column. Under 421 px the first screen's buttons fill the width. Under 601 px wide recordings switch to their narrow cut.

## Elevation & Depth

Depth is soft and lifted, from light above: low-opacity shade with a long negative-spread blur, never a hard offset. Panels are flat at rest with an inset hairline; what floats is the app.

### Shadow Vocabulary
- **Capture frame** (`var(--ring), 0 2px 4px rgb(var(--shade) / 0.06), 0 24px 48px -20px rgb(var(--shade) / 0.42)`): every capture in a panel or the strip. The ring is `0 0 0 1px rgb(0 0 0 / 0.06)` by day and `rgb(255 255 255 / 0.08)` at night, where a dark ring vanishes.
- **Ember button** (`0 1px 0 rgb(255 255 255 / 0.28) inset, 0 10px 24px -12px rgb(var(--shade) / 0.55)`), deepening to `0 16px 30px -14px` at 0.6 on hover.
- **Line button** (inset 1 px line plus `0 1px 2px` at 0.06), lifting to `0 6px 14px -8px` at 0.3 on hover.
- **Ground contact** (a blurred radial ellipse of shade at 0.32): under the hero window's foot (on the picture, so it tilts with it). The window's own shadow is in the capture.
- **Menu bar tile** (inset line plus `0 10px 24px -16px` at 0.5).
- **iPhone** (`0 40px 80px -30px rgb(0 0 0 / 0.45)` on the front phone; a `drop-shadow(0 30px 40px rgb(0 0 0 / 0.28))` on the back one).
- **Stuck bar** (`0 1px 0` line, with a 14 px backdrop blur over the ground at 82%).

In forced-colours mode, which drops every shadow, panels and every button get a 1 px CanvasText border, and captures, phones and the menu bar tiles a 1 px outline drawn inside their edge (an outline, so a capture is never resized by a border).

### Named Rules
**The App Floats Rule.** Shadows belong to the things that are the app (captures, phones, the window) and to buttons. Panels, the strip and the build list do not cast shadows.

## Shapes

Corners are gently rounded and grow with the thing: 5 px inline code, 8 px the focus outline, 10 px the skip link, 11 px a small button, 12 px a capture (the app's own window radius), 14 px a big button, 16 px a menu bar tile, 28 px a panel. A capture that bleeds out of its panel's foot keeps its top corners and loses its bottom ones; a cropped tall capture fades to nothing over its last 28%, and a strip capture fades out to the right over its last 22%. Phones use their device radii (54 and 45 px, 42 and 37 px on a phone). Nothing is a pill.

The mark lives in one file, `assets/img/mark.svg`: a rounded post and two pointed boards with rounded joins, each a path with an id. Pages draw it with `<use>` so the CSS colours the post and the boards and can move each board.

## Components

### Bar and lockup
Fixed at the top, transparent over the sky, gaining a ground (82% ground, blurred) and a hairline once the page has moved 8 px. Lockup on the left: the mark at 30 px and the lowercase "trek" wordmark. Two nav links in second ink (500, 15 px) brighten to ink on hover. A small ink Download button at the right. The lockup's target is 44 px tall without making the bar taller. The privacy and not-found pages start with the bar already stuck.

### Buttons
- **Shape:** 14 px corners on the 56 px big button, 11 px on the 40 px small button (44 px on a touch screen). A stroked 20 px download arrow (2 px as drawn, `vector-effect: non-scaling-stroke`, round caps) sits before the label where there is one.
- **Ember (primary):** board on board ink, with an inner top highlight and a soft shade below. Hover lightens one step, rises 1 px and deepens the shadow; press returns it. One per screen.
- **Ink:** ink on ground; hover mixes 14% ember into the ink. The bar's Download.
- **Quiet:** ink text on raised white at 42%, with an inset hairline; hover raises the fill to 75%. The first screen's second choice.
- **Line:** ink on raised white with an inset hairline and a faint drop; hover darkens the ring and lifts. The visitor's own build gets a 2 px inset ember ring.
- **Transitions:** colours 180 ms, shadow and transform 220 ms, all on the site's ease-out.
- **Hover** styling sits under `@media (hover: hover)`, so a tap on a touch screen never leaves a button looking hovered; buttons also drop the browser's grey tap flash.
- **Press:** every button scales to 0.96 over 150 ms ease-out while pressed (the `scale` property, apart from the hover's lift), never when disabled.

### Panels and captures
A panel is a 28 px rounded piece of the sky (panel-2 into panel) with an inset hairline, holding one real capture centred. Variants: tall (420 to 560 px high), mid (280 to 380 px), wide (more padding top and bottom), bleed (no bottom padding; the capture rises from the panel's foot with square bottom corners), crop (a fixed height, the capture starts at the top and fades out). A capture is shown at the size it was cut for, not stretched to fill.

### The strip
Three small features in a row on the ground itself, each under a top hairline, with a 168 px media slot, a small title and two short lines. It is the quiet passage between panels. One slot holds the menu bar's three icons on 44 px raised tiles with their names (at night the icons are inverted on a light tile).

### The iPhone band
A full-width band in the page's light, with no field of its own. Text on the left (5 parts), two phones on the right (7 parts): a front phone in a near-black body, its screen a recording, and a back phone tilted -6 degrees, behind and to the left, drifting against the scroll. The phones stand whole. As the band arrives they come together (see Motion).

### Build list
Four builds in a row (two under 1024 px, one under 768 px), each under an ink hairline at 22%: a title, a sentence that says what the first launch asks, and line buttons that carry the file size once GitHub has answered.

### Footer
One line, standing on the dark ground of the dusk photograph (light inks: #F4EFF7, #D9D1E3, #BDB4C9): the lockup at 22 px, the copyright, five text links (GitHub, Releases, Privacy, Terms, support). Nothing else. On a touch screen each link is a 44 px target.

### Drawings
A small object under the words of a feature, where the page would otherwise be empty, in one language (`script/site/make_illustrations.py`, inline SVG): line drawings of blocks in isometric projection, one ink line (#3B3040, #C9C0DD at night, 1.3 px) on the page's own light, and one ember line for the thing that needs you. Each face is filled with the light measured where the drawing stands, so hidden edges stay hidden and it reads as line alone. No shading, shadows, fills of colour, icons or people. Each carries one thought:
- **One inbox:** a quiet stack of threads, and the one that needs you lifted clear of it, tipped toward you, in ember.
- **Worktrees:** the branch leaves the main beam's side at an angle and runs beside it, one piece, in ember.
- **Study:** two solid blocks, and the next one only drawn in, as a dashed ember cube.
On a phone each stands small (88 px high) beside its heading. A drawing that doesn't carry a thought of its own is left out: the page's light is better than a weak drawing.

### Text pages
Privacy and not-found sit on a sky that fades into the ground over 420 px, once (a short page shows ground below it, not the sky again), in a 760 px column. Privacy: document headings 1.5rem with 52 px above, paragraphs and lists 14 px apart in second ink, a hairline-ruled table that scrolls sideways when narrow, a muted "Last updated" line. Not-found: the mark at 64 px, a headline, one sentence and one ember button, left-aligned in the middle of the screen.

### Motion
One ease, a steep exponential ease-out (`cubic-bezier(0.16, 1, 0.3, 1)`); the press uses plain ease-out. Transforms, opacity, blur and clip only. Nothing loops but the app's own recordings. One entrance token: `--enter` 400 ms, `--rise` 12 px, `--blur` 4 px.
- **Signpost settle:** on load the post grows up from its foot (0.45 s), then each board swings round the post from a sliver, overshoots and settles (1.1 s, at 0.15 s and 0.27 s). Once. The first screen is still by about 1.4 s.
- **Hover lean:** while the ember button is hovered (with a pointer that hovers) or focused the left-pointing board turns 5 degrees; while the quiet button is, the right-pointing board turns -5 degrees (0.6 s).
- **First-screen entrance:** headline (450 ms), lede and buttons (400 ms) rise 12 px out of a 4 px blur, 100 ms apart from 0.1 s; the fine line only fades (0.4 s); the window rises 32 px over 600 ms from 0.4 s.
- **Window tilt:** the window lies back 10 degrees at 94% scale and comes up flat, full size and 40 px higher over three quarters of a screen of scrolling, cubic ease-out, reversible. With a fine pointer, inside the first screen, it turns up to 2.2 degrees towards the mouse.
- **Drift:** panel captures and the back phone move up to 18 px against the scroll while near the screen.
- **The phones come together:** as the iPhone band arrives the back phone comes in from the left and the front one from the right (90 and 110 px apart and a little low on a desktop, 80 and 90 px and mostly sideways on a phone), each turning 5 to 6 degrees, and settle in their place by the time the band is mid-screen. Tied to the scroll (`animation-timeline: view()`, a gentle ease-out) where the browser can; elsewhere played once as they arrive (650 ms, 90 ms apart, from site.js). Their place at rest is the layout, so nothing moves after.
- **Drawings:** each draws its lines in once as its row arrives (450 ms, after 150 ms); the dashed cube fades in after.
- **The dusk grass** drifts 24 px against the scroll as the page ends (scroll-driven only).
- **Reveals:** one entrance per row: the features head, each feature row or card (its words arrive with its picture), the iPhone band, the strip as a whole and the build list as a whole rise 12 px out of a 4 px blur over 400 ms, once, as they arrive; 0.1 s apart when two or three arrive together, never more. The staged-threads line just sits there.
- **Loops:** each recording is a still poster in the page. As it comes within half a screen, and only when motion is welcome, the script swaps it for a muted, looping, inline video (the narrow cut under 601 px), which plays while a quarter of it is visible and pauses otherwise, and pauses while the tab is hidden. There is no visible pause control: the loops stop under Reduce Motion and off screen.
- **Reduce Motion:** no smooth scroll, no settle, no lean, no tilt, no drift, the phones and the drawings are simply in place, and the posters stay still; the first screen and each reveal only fade (300 ms and 200 ms); videos already playing pause when the setting turns on.
- **No script:** reveals are gated on a `js` class the script sets, so the page shows whole; posters stay as pictures; the Download button points at the latest release.

## Do's and Don'ts

### Do:
- **Do** show only real captures and real recordings of the app, cut from its isolated test app in demo dress, and say in a visible line when threads are staged.
- **Do** replace a feature picture or loop by writing a new capture under the same file name in `assets/img/features/` (`<name>-paper.webp`, `<name>-night.webp`, `<name>.mp4`, `<name>-poster.webp`); `script/site/feature_media.sh` makes them. Sizes come from the CSS.
- **Do** write each picture's origin into the `.json` file of the same name beside it.
- **Do** change the mark only in `assets/img/mark.svg`, then run `script/site/make_icons.py` to redraw the favicon, touch and manifest icons and the share picture.
- **Do** run `python3 script/site/stamp.py site` before every push: every local link carries `?v=<hash>`, and a stale stamp serves old files.
- **Do** keep all styling in `assets/site.css`; the CSP forbids inline styles and scripts. Script may set custom properties through the CSSOM.
- **Do** define every new colour for day and night, and keep text at 4.5:1 against its ground.
- **Do** change the row arrangement from one feature to the next.
- **Do** keep the page whole without script and with Reduce Motion on.
- **Do** keep words few, in short plain sentences, claiming only what the README says.

### Don't:
- **Don't** draw buttons or feature rows as boards on a stick, or use the signpost as scenery, a bullet, a divider or a layout device.
- **Don't** put the pixel hiker on the home page.
- **Don't** draw a picture that is only its heading turned into a symbol (a bar for an agent, a gate for permission, swatches for themes). Draw one object with a thought, in the drawings' language, or leave the field to the light.
- **Don't** let the hill's footpath show above the window: the photograph's own grass is laid over its top (`make_backdrops.py`, `hide_path`), and a trail beside the signpost reads as a hiking app.
- **Don't** use a dark page with a shrunken window under the headline, or a grid of same-size feature cards.
- **Don't** use a mock-up, a generated image or a hand-retouched frame as a picture of the app. The one retouch is the hero window's three title-bar buttons, painted in because the test window is never the key window.
- **Don't** put ember on anything but the boards, the primary button and the visitor's build.
- **Don't** put a label, pill or eyebrow above a headline, use italics or bold lead-ins, or put a colon or semicolon in display copy.
- **Don't** use a grey border or a hard offset shadow.
- **Don't** add trackers, analytics, cookies, or type or script from another domain. The only request off the domain is to GitHub's API for the latest release.
- **Don't** loop anything that is not the app's own recording.
- **Don't** credit the fonts in the footer or add words to it.

## Where the photographs come from

Both are Unsplash photographs, free to use under the Unsplash License (no credit is required; it is given here and in `NOTICE`): the first screen's "Dried grass field under blue sky during daytime" by Chris Mongeau (unsplash.com/photos/ZRdSZeVKBaE), and the end's "The sun is setting over a field of tall grass" by Johannes Plenio (unsplash.com/photos/X7RUQSjQv0k). `script/site/make_backdrops.py` frames and grades them; `site/README.md` lists each file.
