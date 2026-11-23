---
name: trekdesktop.com
description: "Trek's website, \"One line\": every agent, one trail, drawn as one ink line down the whole page; its ember dot is the thread that needs you, and it finds you on your phone before it rests at the download."
colors:
  paper: "#F2EFF6"
  paper-2: "#E9E4F1"
  wash: "#DAD2EC"
  ink: "#1F1714"
  ink-2: "#463C47"
  line: "rgb(31 23 20 / 0.18)"
  stroke: "#1F1714"
  ember: "#EE5A24"
  ember-hover: "#F26A37"
  on-ember: "#1F1714"
  raised: "#FBFAFD"
  paper-night: "#17151F"
  paper-2-night: "#1D1A28"
  wash-night: "#2A2640"
  ink-night: "#F3EEF6"
  ink-2-night: "#D3CBDD"
  line-night: "rgb(243 238 246 / 0.2)"
  stroke-night: "#EDE6F2"
  ember-night: "#FF7A3D"
  on-ember-night: "#1D1A2C"
  raised-night: "#221F30"
typography:
  display:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(3rem, 5.6vw, 5.2rem)"
    fontWeight: 680
    lineHeight: 0.98
    letterSpacing: "-0.026em"
  headline:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.4rem, 5vw, 4.8rem)"
    fontWeight: 650
    lineHeight: 1.02
    letterSpacing: "-0.02em"
  title:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "22px"
    fontWeight: 650
    lineHeight: 1.2
    letterSpacing: "-0.02em"
  wordmark:
    fontFamily: "Bricolage Grotesque, ui-sans-serif, system-ui, sans-serif"
    fontSize: "27px"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.035em"
  lede:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(17px, 1.45vw, 20px)"
    fontWeight: 400
    lineHeight: 1.45
  body:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.55
  run:
    fontFamily: "Hanken Grotesk, ui-sans-serif, system-ui, sans-serif"
    fontSize: "16.5px"
    fontWeight: 400
    lineHeight: 1.75
  field:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: "16px"
rounded:
  tag: "5px"
  focus: "8px"
  button-small: "11px"
  button: "14px"
  screen: "24px"
  box: "30px"
spacing:
  gutter: "clamp(34px, 6vw, 96px)"
  wide: "1200px"
  line: "2.25px"
components:
  button-ember:
    backgroundColor: "{colors.ember}"
    textColor: "{colors.on-ember}"
    rounded: "{rounded.button}"
    height: "56px"
  button-ink:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.button-small}"
    height: "40px"
  button-line:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.button-small}"
    height: "40px"
---

# Design System: trekdesktop.com

## Overview

**Creative North Star: "One line"**

Every agent, one trail, drawn as one ink line that runs the whole page. The first screen stands in
a field of characters, the agents' own output running in rows, that thins out to nothing at the
desk; there the one line takes over. It is the desk under Trek's window, turns down the right edge,
becomes the ground the two phones stand on, runs down the left edge past everything Trek does,
closes round "your computer and phone", and stops at the Download button, over the same field of
characters lying still. The ember dot at the line's head is the thread that needs you: it carries
"Run ./migrate?" from the desk to the phone, where its Allow is tapped, and rests at Download. The
line is drawn exactly to the dot as the page scrolls, never past it; scrolled to the end, the line
is whole.

The world is paper and ink: a pale lilac paper by day, a deep violet-black at night, a 512 px paper
tooth over both, one ink weight for the line and the drawings, and ember for the one thing that
needs you. Type is two grotesques, upright, and a monospace face for the field only.

**Key Characteristics:**
- One line, one dot, one ember. Ember is the dot, the main button, the caret in the field, and the
  one part of each drawing that needs you.
- The character field is text, not a picture: rows built once and slid by CSS transforms. A click
  or a touch on it sends out a ring of scrambled characters with an ember spark.
- Drawings are ink lines on the page's paper (faces filled with it, so what is behind stays
  hidden), with real product words where words help.
- The product is shown alive: Trek's window redrawn so it can act (and saying so), and real iPhone
  captures.
- Every colour is defined for Paper and Night, following the system.
- Reads whole with Reduce Motion on: the field and drawings stand still, the line is drawn whole
  and the dot rests at Download.

## Colors

Paper (`paper`, `paper-2` toward the end) is the page; `wash` fills the screen's bezel and the
phones' frames. `ink` is headlines and the line's stroke (`stroke`); `ink-2` is text under
headings, the field's characters at 18 to 26% (34 to 42% in the end field), labels and captions.
`line` is every hairline rule. `ember` is the dot, the ember button, the field's caret, the
notification's icon in the Native drawing and the note travelling between computer and phone.
Night uses the `-night` values. Text is 4.5:1 or better against its ground everywhere, including
over the field at its densest.

## Typography

Bricolage Grotesque for headlines (680 for the first screen, 650 elsewhere), the wordmark and the
group names; Hanken Grotesk for everything else; JetBrains Mono, a Latin subset, for the character
field alone. No italics, no bold lead-ins, no two-tone headlines, no label above a heading, no colon
or semicolon in display copy.

## Layout

A column of at most 1200 px plus a fluid gutter. The first screen is two columns (words, then the
window on its stand, 5 to 8), its desk line low in the screen. "Walk away" is two columns (words,
then the two phones). Everything Trek does is rows hanging off the line: the group's name and count
on the left, its items run together as a line of text with middots, the Native row with its
drawing on the right. The privacy box holds the words and a list of what is true on the left and
the drawing on the right. Downloads are four columns: macOS, Windows, Phone, Linux (Preview). Under
1000 px the first screen stacks; under 720 px everything is one column and the line runs in the
side margins. On a phone nothing changes its nature, only its size: the window is still a screen on
a stand (16 px from each edge), the phones still two iPhones.

## Components

### The line (`assets/home/home.js`)
Laid out from the page's own boxes, and again whenever one of them, a late image or the fonts move
it. Stroke 2.25 px, round caps, 26 px corners (16 on a phone). The screen's stand is a neck and a
foot resting on the desk. The dot's place for a scroll position keeps it level with the reading line
(60% down the window) where the line runs down the page, and moves it at a steady pace where it runs
across; the page's end is the line's end. Its words ride under it where it runs flat and beside it
where it runs down. `window.__trailCheck()` reports the drawn length, the dot's distance from
Download and each joint's gap.

### The character field (`assets/home/field.js`)
Rows of characters (a ramp of weights and a few code symbols), each holding its pattern twice and
sliding one pattern's width in 40 to 110 seconds on the compositor; built once per width, at most
90 rows of 260 characters; paused off screen. At the top it is densest high up and gone at the desk,
quieter behind the words; one ember caret breathes at the end of a row. At the end it lies still
and stronger. Clicks and touches anywhere on it (but not on links, buttons, the window or the
phones) send out a ring.

### Trek's window (`assets/home/inbox.*`)
The real app's sidebar, Inbox and one thread, redrawn at 760 by 476 and scaled to its box. It goes
through four steps while on screen: at work, one stops to ask and goes to the top with its question,
answered, finished. Staged threads, and the page says "redrawn". Its shape never changes. In a box
under 560 px wide (a phone) it is laid out at 540 wide instead, so its words are a third larger:
the Inbox and Projects on the left, the thread, its card and the composer on the right, and a few
words and one chip left out (marked `tk-x` in `inbox.js`) so that nothing wraps or is cut short.
The stand and the desk's distance under it are in proportion to the screen's width.

### The phones
The iPhone app's Threads list behind and its approval in front, coming together as their section
arrives (played by the scroll, both ways). The front phone's Allow is tapped, a ring and a press, as
the dot arrives under it. A capture is always drawn in its own proportions (590 by 1276), so a frame
is about 18.7 to 9, as an iPhone is. On a phone both stand upright inside the screen: the approval
in front on the right at two thirds of the width, Threads behind it on the left, smaller, with five
eighths of its width showing; each frame's corners follow its width.

### The drawings
Native: a laptop from the front, a notification from Trek sliding out of the screen's corner
("Migrate the schema", "Wants to run ./migrate") and going again, 5 s. Privacy: your computer and
your phone on one direct line ("Direct, over your Wi-Fi"), a note going over and back along it, and
above it an empty dashed space: "No Trek server, nothing in between". Nothing crosses the box.

### Buttons
Ember (the one main action), ink (the bar's Download), line (each build). 56 px and 40 px, rounded
14 and 11, a pressed scale of 0.96, hover only where there is a hover.

### Text sizes
Nothing a visitor reads is under 12 px at any width; the words inside the two drawings render at
14 px or more on a phone (they scale with the drawing, so their own sizes are set for the narrowest
column). Two things stay smaller on purpose: the redrawn window's text, at the real app's own sizes
(11 to 13 px, scaled with the window; on a phone its 13 px draws at 6.5 px at 320 wide, 8.3 at 393
and 9.4 at 440), because it depicts Trek's interface and is labelled as a picture; and the character
field, which is texture, hidden from assistive technology.

## Motion

One authored story: the line drawing to its dot as the page is read. Around it, only loops that
pause off screen: the field's rows, the caret, the window's steps, the two drawings. Nothing a visitor
needs to read moves by itself: the field is decorative and `aria-hidden`, and its rows drift a few
characters a second. There is no pause button on the page, as on the earlier home page's loops:
every loop stops under Reduce Motion (the system setting is the control) and while off screen. Transforms and
opacity only. Under Reduce Motion nothing moves; the line is whole, the dot at Download, the drawings
at their telling moment.

## Do's and Don'ts

### Do
- Keep the line drawn exactly to the dot, and whole at the end of the page.
- Keep every claim true for the latest release; tag what is Mac only, partial or a preview.
- Run `python3 script/site/stamp.py site` after any change under `site/`.
- Keep all styling in the stylesheets and all script in files: the CSP forbids inline styles and
  scripts. Script may set custom properties through the CSSOM.

### Don't
- Don't draw anything outdoors (hills, trails, mountains), and don't use the mark as scenery.
- Don't let a line leave the privacy box or stop short of a joint.
- Don't load type, script or pictures from another domain; the only request elsewhere is GitHub's
  release API, for the download.
- Don't add trackers, analytics or cookies.
