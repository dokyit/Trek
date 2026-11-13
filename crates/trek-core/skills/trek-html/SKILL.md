---
name: trek-html
description: How to write a trek-html block, the interactive page Trek runs inline in an answer (a 3D model, a simulation, an explorable diagram, a tabbed report). Its size, what it may load, Trek's theme variables, and the two calls a page can make back to Trek. Use when an interactive or rich visual genuinely helps; prefer trek-viz for a simple chart.
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

- Inline your CSS and JS.
- Scripts, styles, images and fonts may load over https from CDNs such as cdn.jsdelivr.net or
  unpkg.com. Never over http, and never from local files: the page's policy blocks both.
- Links open in the user's browser, so the page can't navigate away.

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
