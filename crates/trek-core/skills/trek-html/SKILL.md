---
name: trek-html
description: How to write a trek-html block, the interactive page Trek runs inline in an answer (a 3D model, a simulation, an explorable diagram, a tabbed report). Its size, what it may load without asking the user, Trek's theme variables, and the two calls a page can make back to Trek. Use when an interactive or rich visual genuinely helps; prefer trek-viz for a simple chart.
---

# trek-html

A fenced `trek-html` block holds one complete, self-contained HTML document. Trek runs it inline
in the answer, in a sandboxed web view. Write the block, then state its takeaway in prose: the
page shows something, the words say what it means.

## Shape

- One `trek-html` block per answer, with a `<title>` (Trek names the card by it).
- It is as wide as the answer and as tall as its content: 160 to 720 px, and it scrolls past that.
  Lay it out for that width; don't assume a full window.
- Keep it under 200 KB.

## What it may load

The page reaches only what its policy names. A load from anywhere else is stopped, and Trek
asks the user about that host on the card (Allow once, Always allow, Don't allow).

- Inline your CSS and JS. A page that needs nothing from the web always works.
- Without asking: library files under `https://cdnjs.cloudflare.com/ajax/libs/` (three.js, d3,
  Chart.js, Plotly, ECharts, KaTeX, Leaflet and most others are there) and Google Fonts. Take
  your libraries from cdnjs.
- Any other host asks the user first: cdn.jsdelivr.net, unpkg.com, esm.sh, an API, a host of
  images. That goes for scripts, styles, images, fonts, media, `fetch` and WebSocket alike. Use
  one only when the page needs it, say so in your answer, and have the page show a line about
  what it's waiting for instead of breaking (it loads again once allowed).
- Nothing loads over http, and never from local files or this machine. No frames from other
  addresses (write one with `srcdoc`), no forms that submit, no WebRTC.
- A link opens in the user's browser when they click it. The page can't navigate away, and
  `window.open` with no click does nothing.

## Looks

Style it with Trek's CSS variables, on a transparent page background (Trek has light and dark
themes, and the page follows whichever is on):

- `--trek-bg`, `--trek-surface`: backgrounds
- `--trek-fg`, `--trek-muted`: text
- `--trek-accent`: the accent colour
- `--trek-border`: lines and outlines
- `--trek-font`, `--trek-mono`: the fonts
- `--trek-radius`: the corner radius

## Talking back to Trek

- `trek.send(text)` sends `text` as the user's next message in this thread (a button's "Try
  this one").
- `trek.copy(text)` puts `text` on the clipboard.

## Cards Trek ships recipes for

For a map, a pull request or a database migration, follow Trek's built-in skill `map-card`,
`pull-request-card` or `database-migration-card` instead of starting from nothing.
