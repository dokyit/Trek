---
name: map-card
description: A live map card for an answer that recommends a few places (cafés, sights, stops, or hotels with prices) the user could go to. Pins and a "1 / N" pager share one selection, a route redraws to the chosen place, a swatch picker re-themes the map, and "Send to iPhone" hands the thread to the phone; in stays mode it shows price pins, a landmark, "View rooms", a photo strip and filter chips. Use when places, distances and a route matter more than a list.
---

# Map card

A `trek-html` card for an answer about **where to go**: a few places near the user (or near a
landmark), one of them your pick. The user compares them on the map: the pins and the pager in the
panel share one selection, the route redraws to whichever is chosen, the swatch at the top right
re-themes the map (Vivid, Earth, Autumn, Paper, Night), and **Send to iPhone** asks Trek to put
the thread on the user's phone.

It has two looks, from the same card:

- **Places** (the default): cafés, sights, stops. Pins show the name and the travel time, the
  user's position is a blue dot, and the panel shows a rating, a subtitle and up to three facts.
- **Stays** (`mode: "stays"`): hotels or flats from a search. Your picks are price pins with a blue
  dot, the other results plain price pins, the landmark the search is near (the origin) a blue
  pill, and the panel says why each pick is there, the nightly price and the distance, with
  **View rooms** linking to the listing. Optional: a line above the map ("1,624 stays in Paris ·
  14–19 Nov · 2 adults"), a strip of photos under it, and filter chips that ask you for a new search.

Use it when the places have real coordinates and walking or travel time matters. Don't use it for
one place (say where it is), for places without coordinates, or for a list where location doesn't
matter (a table is better).

## What you fill in

Copy the card below into your answer as one fenced `trek-html` block, change the `<title>`, and
replace the `DATA` object. Keep everything else as it is.

- `mode`: `"places"` (the default; leave it out) or `"stays"`.
- `origin`: where the user is, or the landmark: `{ name, lat, lng }`. A blue dot with a halo in
  places mode; in stays mode a blue pill with the name (add `landmark: false` for a plain dot).
- `places`: one object per place, your pick first:
  - `name`, `lat`, `lng` (decimal degrees, real ones: look them up, never guess),
  - `minutes` (travel time, a number) and `mode` (`"walk"`, `"bike"`, `"drive"` or `"transit"`),
  - `kind` ("Coffee shop", "Bakery café"), `rating` (number or `null`),
  - `tags`: up to three short facts ("Fast Wi-Fi", "Outlets", "Open till 9"). Each gets an icon from
    its words (Wi-Fi, outlets, quiet, view, hours, coffee, bright, food); write `{ label, icon }`
    to choose one of `wifi`, `plug`, `quiet`, `view`, `moon`, `cup`, `sun`, `food`, `pin`,
  - `photo`: an https image URL or `null` (a plain tile shows instead),
  - `route`: the path from `origin` as `[[lat, lng], …]` along real streets, or `[]` for a straight line.
  - In stays mode also `price` (the nightly price as shown, "£148.10"; pins drop the pence),
    `pick` (`true` for your picks, `false` for other results: plain price pins, not in the pager),
    `label` (why it's a pick: "Closest to Eiffel Tower", "Best value"), `distance` (line three,
    "1.6 km to Eiffel Tower"; by default the minutes to the origin) and `url` (the https listing
    **View rooms** opens, or `null`).
- `selected`: the index of the place shown first (normally `0`).
- `send`: what **Send to iPhone** sends. Keep `"Open this on my iPhone"`: Trek knows it.
- Stays mode only, all optional: `header` (`{ title: "1,624 stays in Paris", detail: "14–19 Nov ·
  5 nights · 2 adults" }`), `photos` (`true` for the strip of picks with photos under the map) and
  `chips` (`[{ label: "Under £150 a night", count: 25, icon: "tag" | "star" | "sort", send: "Show
  stays under £150 a night" }]`: a chip with `send` posts that as the user's next message).

Write the takeaway in prose under the card ("My pick is …, five minutes away"), so the answer
reads without it.

## Rules

- One card per answer. Keep the page under 200 KB: no inlined images (link https photos), at most
  eight places in places mode and forty in stays mode (at most eight picks), routes of at most 60
  points each.
- The map is Leaflet from unpkg with OpenFreeMap's vector tiles drawn by MapLibre in the card's
  own colours; where WebGL is missing it falls back to OpenStreetMap's tiles, tinted. All over
  https. Keep the attribution.
- The card styles itself with Trek's `--trek-*` variables, so it fits Night and Paper; the map's
  own palette starts as Night in a dark theme and Vivid in a light one.
- **Send to iPhone** calls `trek.send(DATA.send)`: Trek posts that text as the user's next message
  in this thread, marked as from the card. Act on it when it arrives: say the thread is on their
  phone (Trek shows it there). A filter chip's `send` arrives the same way: run that search and
  answer with a new card. Nothing else on the card reaches Trek.

## The card

```trek-html
<!doctype html>
<html lang="en">
<head>
<title>Quiet cafés near Linden Park</title>
<link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css">
<link rel="stylesheet" href="https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.css">
<script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
<script src="https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.js"></script>
<script src="https://unpkg.com/@maplibre/maplibre-gl-leaflet@0.1.4/leaflet-maplibre-gl.js"></script>
<style>
  body { padding: 4px 0 8px; font-family: var(--trek-font, system-ui); color: var(--trek-fg, #1c1c1c); }
  .card { position: relative; height: 368px; border: 1px solid var(--trek-border, #e8e8e8); border-radius: 16px; overflow: hidden; background: var(--land); isolation: isolate; -webkit-font-smoothing: antialiased; }
  .stays .card { height: 360px; box-shadow: 0 6px 20px rgba(0,0,0,.07); }
  #map { position: absolute; inset: 0; background: var(--land); }
  .leaflet-container { font-family: inherit; background: var(--land); }
  .leaflet-tile-pane { filter: var(--tiles, none); }
  .leaflet-control-attribution { font-size: 9.5px; line-height: 16px; padding: 0 7px !important; margin: 8px 0 0 8px !important; border-radius: 999px; background: color-mix(in srgb, var(--panel) 78%, transparent) !important; color: var(--faint) !important; }
  .leaflet-control-attribution a { color: inherit !important; }
  .stays .leaflet-control-attribution { margin: 12px 12px 0 0 !important; }
  .leaflet-control-attribution.min { width: 16px; height: 16px; padding: 0 !important; overflow: hidden; color: transparent !important; position: relative; cursor: pointer; }
  .leaflet-control-attribution.min::after { content: "i"; position: absolute; inset: 0; text-align: center; font: italic 600 10px/16px Georgia, serif; color: var(--sub); }
  .pin { position: absolute; display: flex; flex-direction: column; align-items: center; gap: 4px; transform: translate(-50%, calc(-100% + 3.75px)); white-space: nowrap; cursor: pointer; font-size: 12px; line-height: 22px; }
  .pill { display: flex; align-items: center; gap: 4px; height: 22px; padding: 0 9.5px; border-radius: 11px; background: var(--pin); color: var(--pin-fg); box-shadow: 0 1px 3px rgba(0,0,0,.13); transition: background .2s, color .2s; }
  .pill b { font-weight: 500; }
  .pill span { color: var(--pin-min); transition: color .2s; }
  .pin > i { width: 3px; height: 3px; border: 1.75px solid var(--ring-w); border-radius: 50%; background: #adabac; box-shadow: 0 0 2px rgba(0,0,0,.25); }
  .pin.on .pill { background: var(--sel); color: var(--sel-fg); }
  .pin.on .pill span { color: var(--sel-min); }
  .pin.on > i { width: 4px; height: 4px; background: #1c1f1e; }
  .stays .pin { font-size: 12px; }
  .stays .pill { height: 24px; padding: 0 10px 0 8px; gap: 0; box-shadow: 0 2px 7px rgba(0,0,0,.14); }
  .stays .pill em { width: 6px; height: 6px; margin-right: 5px; border-radius: 50%; background: #2898f5; }
  .stays .pill b { max-width: 0; overflow: hidden; text-overflow: ellipsis; opacity: 0; transition: max-width .25s, opacity .25s, margin .25s; }
  .stays .pill span { color: var(--pin-fg); font-weight: 650; }
  .stays .pin:hover, .stays .pin.on { z-index: 2; }
  .stays .pin:hover .pill b, .stays .pin.on .pill b { max-width: 150px; opacity: 1; margin-right: 6px; }
  .stays .pin:hover .pill span, .stays .pin.on .pill span { font-weight: 400; color: var(--pin-min); }
  .stays .pin.on .pill span { color: var(--sel-min); }
  .stays .pin.on .pill em { background: #7ba7f7; }
  .stays .pin > i { width: 6px; height: 6px; border-width: 2px; background: #1a1a18; box-shadow: none; }
  .night.stays .pin > i { background: #e9ebee; border-color: #2c3134; }
  .stays .pin.other { font-size: 10.5px; }
  .stays .pin.other .pill { height: 21px; padding: 0 9px; }
  .stays .pin.other:not(:hover):not(.on) .pill span { color: var(--sub); }
  .stays .pin.other > i { width: 4px; height: 4px; border-width: 1px; background: #999a98; }
  .me { position: absolute; width: 7.5px; height: 7.5px; transform: translate(-50%, -50%); border-radius: 50%; background: var(--accent); border: 1.7px solid #fff; box-shadow: 0 0 0 5px color-mix(in srgb, var(--accent) 16%, transparent), 0 1px 2px rgba(0,0,0,.2); transition: background .3s; }
  .lm { position: absolute; display: flex; align-items: center; gap: 7px; transform: translate(-7px, -50%); white-space: nowrap; }
  .lm i { width: 11px; height: 11px; border-radius: 50%; background: #3a70e0; border: 1.5px solid #fff; box-shadow: 0 1px 3px rgba(0,0,0,.25); }
  .lm div { display: flex; align-items: center; gap: 5px; height: 24px; padding: 0 11px 0 9px; border-radius: 12px; background: #2861c6; color: #fff; font-size: 13px; font-weight: 500; box-shadow: 0 2px 7px rgba(0,0,0,.18); }
  path.route { stroke: var(--accent); transition: stroke .3s; }
  .picker { position: absolute; top: 12.5px; right: 10.5px; z-index: 600; width: 28px; height: 28px; border-radius: 14px; background: var(--picker); box-shadow: 0 0 0 1px var(--picker-line), 0 2px 6px rgba(0,0,0,.12); overflow: hidden; cursor: pointer; transition: width .16s cubic-bezier(.3,1.25,.5,1), background .15s; view-transition-name: picker; }
  .picker.open { width: 188px; cursor: default; }
  .stays .picker { display: none; }
  .picker .name { position: absolute; left: 12px; top: 0; line-height: 28px; font-size: 12px; color: var(--picker-fg); opacity: 0; transition: opacity .08s; }
  .picker.open .name { opacity: 1; transition-delay: .07s; }
  .sw { position: absolute; top: 4.5px; right: calc(6px + var(--k) * 26.5px); width: 19px; height: 19px; padding: 0; border: 0; border-radius: 50%; cursor: pointer; transition: right .14s cubic-bezier(.3,1.2,.5,1), transform .14s, opacity .14s, box-shadow .1s; }
  .picker:not(.open) .sw { right: 4.5px; }
  .picker:not(.open) .sw:not(.on) { transform: scaleX(.3); opacity: 0; pointer-events: none; }
  .picker.open .sw.on { box-shadow: 0 0 0 1.7px var(--picker), 0 0 0 3.1px var(--ring); transition-delay: 0s, 0s, 0s, .03s; }
  .zoom { position: absolute; left: 10.5px; top: 10.5px; z-index: 600; display: flex; flex-direction: column; width: 30px; border-radius: 7px; background: var(--panel); box-shadow: 0 2px 8px rgba(0,0,0,.12); }
  .zoom button { width: 30px; height: 30px; border: 0; padding: 0; background: none; color: var(--fg); opacity: .75; font-size: 15px; cursor: pointer; }
  .zoom button + button { border-top: 1px solid var(--line); }
  .info { position: absolute; left: 10.5px; right: 10.5px; bottom: 10.5px; z-index: 600; display: flex; gap: 14px; height: 86px; box-sizing: border-box; padding: 10px; border: 1px solid var(--line); border-radius: 14px; background: var(--panel); box-shadow: 0 2px 6px rgba(0,0,0,.1); color: var(--fg); transition: background .3s, color .3s; }
  .stays .info { height: 98px; gap: 12px; }
  .thumb, .body, .send { transition: opacity .1s, filter .1s; }
  .info.swap .thumb, .info.swap .body, .info.swap .send { opacity: 0; filter: blur(4px); }
  .thumb { flex: none; width: 64px; height: 64px; border-radius: 7px; background: var(--blank) center / cover no-repeat; display: grid; place-items: center; color: var(--faint); }
  .stays .thumb { width: 76px; height: 76px; border-radius: 10px; color: var(--sub); }
  .body { flex: 1; min-width: 0; margin-top: -2px; }
  .title { display: flex; align-items: baseline; gap: 7px; height: 20px; padding-right: 76px; white-space: nowrap; }
  .title b { font-size: 15px; line-height: 20px; font-weight: 600; letter-spacing: -.1px; overflow: hidden; text-overflow: ellipsis; }
  .title span { flex: none; font-size: 13px; color: var(--sub); }
  .title svg { width: 11px; height: 11px; margin-right: 3px; vertical-align: -.5px; fill: currentColor; }
  .sub, .tags { font-size: 12.5px; line-height: 20px; color: var(--sub); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sub em { font-style: normal; color: var(--faint); }
  .stays .sub { margin-top: 2px; font-size: 13px; color: color-mix(in srgb, var(--fg) 82%, transparent); }
  .stays .title { padding-right: 112px; }
  .tags { display: flex; gap: 12px; margin-top: 6px; padding-right: 128px; }
  .stays .tags { margin-top: 2px; font-size: 12px; color: var(--fg); opacity: .85; }
  .tags span { display: inline-flex; align-items: center; gap: 6px; }
  .tags svg { flex: none; width: 12px; height: 12px; color: var(--faint); }
  .stays .tags svg { color: inherit; }
  .pager { position: absolute; top: 8px; right: 10px; display: flex; align-items: center; height: 20px; font-size: 12px; color: var(--faint); font-variant-numeric: tabular-nums; white-space: nowrap; }
  .pager button { display: grid; place-items: center; width: 22px; height: 22px; border: 0; padding: 0; border-radius: 6px; background: none; color: var(--sub); cursor: pointer; }
  .pager button:hover { background: color-mix(in srgb, var(--fg) 7%, transparent); }
  .pager span { padding: 0 7px; letter-spacing: .5px; }
  .pager .back { display: flex; width: auto; gap: 4px; padding: 0 6px 0 2px; font: inherit; color: var(--sub); }
  .send { position: absolute; right: 11px; bottom: 10px; display: flex; align-items: center; gap: 7px; min-width: 121px; height: 27px; box-sizing: border-box; padding: 0 12px; border: 0; border-radius: 14px; background: var(--btn); color: var(--btn-fg); font: 500 12.5px/1 var(--trek-font, system-ui); cursor: pointer; text-decoration: none; white-space: nowrap; transition: background .15s, opacity .1s, filter .1s; }
  .send:hover { background: color-mix(in srgb, var(--btn-fg) 8%, var(--btn)); }
  .send:disabled { cursor: default; }
  .send[hidden] { display: none; }
  .send.busy { justify-content: center; padding: 0; }
  .send.busy svg { animation: spin 1s linear infinite; }
  .send .check path { stroke-dasharray: 20; animation: draw .12s ease-out both; }
  .stays .send { width: auto; height: 26px; padding: 0 13px; right: 11px; bottom: 12px; font-size: 13px; background: #e5f3fd; color: #1f98ed; }
  .night.stays .send { background: #23384c; color: #7cc0f7; }
  @keyframes spin { to { transform: rotate(360deg); } }
  @keyframes draw { from { stroke-dashoffset: 20; } }
  .head { display: flex; align-items: baseline; gap: 6px; margin: 0 2px 8px; font-size: 13px; line-height: 20px; white-space: nowrap; overflow: hidden; }
  .head b { font-weight: 500; color: var(--trek-fg, #3b3b3b); }
  .head span { color: var(--trek-muted, #8c8c8c); overflow: hidden; text-overflow: ellipsis; }
  .strip { display: flex; gap: 8px; margin-top: 10px; overflow-x: auto; scrollbar-width: none; }
  .strip::-webkit-scrollbar { display: none; }
  .tile { flex: none; width: 184px; border: 0; padding: 0; background: none; text-align: left; font: inherit; color: var(--trek-fg, #1a1a1a); cursor: pointer; }
  .tile div { position: relative; height: 138px; border-radius: 13px; background: color-mix(in srgb, var(--trek-fg, #000) 7%, transparent) center / cover no-repeat; display: grid; place-items: center; color: var(--trek-muted, #545454); }
  .tile .r { position: absolute; top: 6px; left: 6px; display: flex; align-items: center; gap: 3px; height: 21px; padding: 0 8px 0 7px; border-radius: 11px; background: #f9f9f9; color: #000; font-size: 12px; font-weight: 600; box-shadow: 0 1px 4px rgba(0,0,0,.12); }
  .tile .r svg { width: 11px; height: 11px; fill: #e99f21; }
  .tile b { display: block; margin: 9px 2px 0; font-size: 13px; font-weight: 600; line-height: 17px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tile span { display: block; margin: 0 2px; font-size: 12px; line-height: 17px; color: var(--trek-muted, #6d6d6d); }
  .tile span b { display: inline; margin: 0; font-size: 12px; color: var(--trek-fg, #1a1a1a); }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 10px; }
  .chip { display: inline-flex; align-items: center; gap: 4px; height: 24px; padding: 0 8px; border: 0; border-radius: 12px; background: var(--trek-surface, #fbfbfb); box-shadow: inset 0 0 0 1px var(--trek-border, #eee); color: var(--trek-fg, #3c3c3c); font: 12px var(--trek-font, system-ui); cursor: pointer; }
  .chip:hover { background: color-mix(in srgb, var(--trek-fg, #000) 6%, transparent); }
  .chip svg { width: 10px; height: 10px; color: var(--trek-muted, #555); }
  .chip em { font-style: normal; color: var(--trek-muted, #888); margin-left: 3px; }
  ::view-transition-group(*), ::view-transition-old(*), ::view-transition-new(*) { animation-duration: .12s; }
  ::view-transition-old(root), ::view-transition-new(root) { animation: none; mix-blend-mode: normal; }
  @property --reveal { syntax: "<length>"; inherits: true; initial-value: 0px; }
  @media (max-width: 440px) { .card { height: 420px; } .info { height: auto; flex-wrap: wrap; } .tags { padding-right: 0; flex-wrap: wrap; row-gap: 0; } .send { position: static; margin-left: auto; } .title { padding-right: 72px; } }
</style>
</head>
<body>
<style id="reveal"></style>
<div id="head"></div>
<div class="card" id="card" tabindex="0" aria-label="Map of the places">
  <div id="map"></div>
  <div class="picker" id="picker" role="radiogroup" aria-label="Map colours"><span class="name" id="picker-name"></span></div>
  <div class="info" id="info">
    <div class="thumb" id="thumb"></div>
    <div class="body"><div class="title" id="title"></div><div class="sub" id="sub"></div><div class="tags" id="tags"></div></div>
    <div class="pager" id="pager"></div>
    <button class="send" id="send"></button>
  </div>
</div>
<div id="below"></div>
<script>
const DATA = {
  origin: { name: "You", lat: 52.3566, lng: 4.8643 },
  places: [
    { name: "The Boathouse", lat: 52.3601, lng: 4.8722, minutes: 5, mode: "walk", kind: "Lakeside café", rating: 4.8, tags: ["Quiet mornings", "Outlets", "Lake view"], photo: null, route: [[52.3566, 4.8643], [52.3572, 4.8668], [52.3586, 4.8697], [52.3601, 4.8722]] },
    { name: "Fern & Fig", lat: 52.3622, lng: 4.8627, minutes: 4, mode: "walk", kind: "Bakery café", rating: 4.6, tags: ["Bright tables", "Outlets", "Good espresso"], photo: null, route: [] },
    { name: "Lumen Coffee", lat: 52.3629, lng: 4.8781, minutes: 9, mode: "walk", kind: "Coffee shop", rating: 4.7, tags: ["Fast Wi-Fi", "Outlets", "Open till 9"], photo: null, route: [] }
  ],
  selected: 0,
  send: "Open this on my iPhone"
};

// The palettes: the map's colours (vector tiles), the card's chrome to match, and a CSS filter
// for when only raster tiles can be shown.
const LIGHT = { panel: "#fdfdfd", fg: "#1c1c1c", sub: "#6e6e6e", faint: "#a0a0a0", line: "#e4e4e4", blank: "#efefed", pin: "#fdfdfd", pinFg: "#1e1e1e", pinMin: "#a0a0a0", sel: "#1f1f1f", selFg: "#fff", selMin: "#b0b0b2", ringW: "#fffefb", picker: "#fcfcfc", pickerLine: "#e6e4e5", pickerFg: "#6e6e6e", ring: "#1c1c1f", btn: "#edf3fc", btnFg: "#4178c8", accent: "#488dec" };
const PALETTES = [
  { name: "Vivid", sw: ["#b2e494", "#9ed0f3"], map: ["#efeeeb", "#fdfdfd", "#cdf3b9", "#ddf4cc", "#aee691", "#9dd1f0", "#468b2f", "#286993"], tiles: "saturate(1.15)" },
  { name: "Earth", sw: ["#b3c384", "#b6d2cf"], map: ["#ede5d7", "#faf6f0", "#d6e2ba", "#e4e9ca", "#b0c585", "#b0d4cf", "#697b36", "#4a7c7a"], tiles: "sepia(.45) saturate(.7) hue-rotate(-12deg) brightness(1.03)", accent: "#be643a", btn: "#f9efe8", btnFg: "#a1583c", panel: "#fcfbf7", fg: "#272620", sel: "#2a2a21" },
  { name: "Autumn", sw: ["#e1a56b", "#add7e0"], map: ["#f0ece2", "#fdfaf4", "#f5e9cb", "#efe0b0", "#e5b66d", "#abd9e7", "#94641b", "#368996"], tiles: "sepia(.35) saturate(1.5) hue-rotate(-18deg)", accent: "#4a8cee", btn: "#eef4fd", sel: "#212121" },
  { name: "Paper", sw: ["#e1e2e0", "#e4e9ec"], map: ["#f2f3f0", "#fdfdfd", "#ebeee8", "#f3f4f2", "#dde4da", "#e0e9ed", "#84837d", "#82919a"], tiles: "grayscale(1) contrast(.9) brightness(1.06)", accent: "#1c1c1c", btn: "#ebebeb", btnFg: "#1a1a1a", sel: "#212121" },
  { name: "Night", sw: ["#386b43", "#194153"], map: ["#212327", "#33343a", "#294b33", "#213e2a", "#366b42", "#1c3e5e", "#98d1a7", "#83b2dd"], tiles: "invert(1) hue-rotate(180deg) brightness(.82) contrast(.92) saturate(.6)", dark: true, accent: "#5897f6", btn: "#343d4f", btnFg: "#83a8f5", panel: "#2e2f35", fg: "#f5f7fb", sub: "#b2b3b9", faint: "#828389", line: "#3a3b42", blank: "#3a3b42", pin: "#2c3134", pinFg: "#f4f6fa", pinMin: "#83848a", sel: "#eff5f5", selFg: "#2c2e2f", selMin: "#7c7e82", ringW: "#e9ebee", picker: "#2e2f35", pickerLine: "#3d3e45", pickerFg: "#b2b3b9", ring: "#f5f5fa" }
];

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const stays = DATA.mode === "stays";
const places = DATA.places.slice(0, stays ? 40 : 8);
const picks = stays ? places.map((p, i) => (p.pick === false ? -1 : i)).filter((i) => i >= 0).slice(0, 8) : places.map((_, i) => i);
let sel = Math.min(Math.max(DATA.selected | 0, 0), places.length - 1);
let palette = null, chosen = false;
if (stays) document.body.classList.add("stays");

const S = (d, w) => '<svg viewBox="0 0 24 24" width="' + (w || 12) + '" height="' + (w || 12) + '" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
const ICON = {
  wifi: S('<path d="M2 9a15 15 0 0 1 20 0M5.5 12.5a10 10 0 0 1 13 0M9 16a5 5 0 0 1 6 0"/><circle cx="12" cy="19.5" r=".8"/>'),
  plug: S('<path d="M9 2v5M15 2v5M6 7h12v4a6 6 0 0 1-12 0zM12 17v5"/>'),
  quiet: S('<path d="M4 9h4l5-4v14l-5-4H4zM17 9.5a3.5 3.5 0 0 1 0 5"/>'),
  view: S('<path d="M2 9c3-2.5 5-2.5 7 0s4 2.5 6.5 0 4.5-2.5 6.5 0M2 15c3-2.5 5-2.5 7 0s4 2.5 6.5 0 4.5-2.5 6.5 0"/>'),
  moon: S('<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/>'),
  cup: S('<path d="M4 8h13v5a6 6 0 0 1-6 6h-1a6 6 0 0 1-6-6zM17 10h1.5a2.5 2.5 0 0 1 0 5H17M8 2v3M12 2v3"/>'),
  sun: S('<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5"/>'),
  food: S('<path d="M7 2v20M4 2v6a3 3 0 0 0 6 0V2M17 22V2c-2.5 1-4 4-4 8h4"/>'),
  pin: S('<path d="M12 21s-7-6.2-7-12a7 7 0 0 1 14 0c0 5.8-7 12-7 12z"/><circle cx="12" cy="9" r="2.5"/>'),
  phone: S('<rect x="6.5" y="2.5" width="11" height="19" rx="2.5"/><path d="M11 18.5h2"/>', 13),
  check: '<svg class="check" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  spin: '<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke-width="3"><circle cx="12" cy="12" r="9" stroke="currentColor" opacity=".25"/><path d="M12 3a9 9 0 0 1 9 9" stroke="currentColor" stroke-linecap="round"/></svg>',
  prev: S('<path d="M15 5l-7 7 7 7"/>', 13),
  next: S('<path d="M9 5l7 7-7 7"/>', 13),
  bed: S('<path d="M3 18V7M3 14h18v4M21 14v-2a3 3 0 0 0-3-3h-7v5M7 11.5h.01"/>', 22),
  place: S('<path d="M12 21s-7-6.2-7-12a7 7 0 0 1 14 0c0 5.8-7 12-7 12z"/><circle cx="12" cy="9" r="2.5"/>', 20),
  landmark: S('<path d="M3 21h18M4 10h16M12 3l8 5H4zM6 10v8M10 10v8M14 10v8M18 10v8"/>'),
  tag: S('<path d="M3 12V4a1 1 0 0 1 1-1h8l9 9-9 9z"/><circle cx="7.5" cy="7.5" r="1.2"/>'),
  sort: S('<path d="M4 6h10M4 12h7M4 18h4M17 4v16l3-3"/>'),
  star: '<svg viewBox="0 0 24 24"><path d="M12 2.5l2.9 6 6.6.8-4.9 4.6 1.3 6.6L12 17.2l-5.9 3.3 1.3-6.6-4.9-4.6 6.6-.8z"/></svg>'
};
const WORDS = [["wifi", /wi-?fi|internet/i], ["plug", /outlet|plug|socket|power/i], ["quiet", /quiet|calm|silent/i], ["view", /view|lake|river|sea|water|canal/i], ["moon", /open|late|till|hours|night/i], ["cup", /espresso|coffee|tea|latte/i], ["sun", /bright|sun|light|terrace|garden/i], ["food", /food|lunch|brunch|cake|pastr|bread|kitchen|breakfast/i]];
const tagIcon = (t) => ICON[(typeof t === "object" && t.icon) || ((WORDS.find(([, re]) => re.test(t.label || t)) || ["pin"])[0])] || ICON.pin;

// The map: Leaflet, with OpenFreeMap's vector tiles drawn by MapLibre in the palette's colours,
// or OpenStreetMap's raster tiles under a CSS filter where WebGL isn't there.
const map = L.map("map", { zoomControl: false, attributionControl: false, scrollWheelZoom: false, dragging: !L.Browser.mobile, tap: true, zoomSnap: 0.25, fadeAnimation: false });
map.setView([DATA.origin.lat, DATA.origin.lng], 15);
const credit = L.control.attribution({ position: stays ? "topright" : "topleft", prefix: false }).addTo(map);
const gl = (() => { try { const c = document.createElement("canvas"); return !!(window.maplibregl && L.maplibreGL && (c.getContext("webgl2") || c.getContext("webgl"))); } catch (_) { return false; } })();
let vector = null;
function style(m) {
  const [land, road, grass, meadow, trees, water, parkLabel, waterLabel] = m;
  const src = { source: "o" };
  const font = ["Noto Sans Regular"];
  const halo = { "text-halo-color": land, "text-halo-width": 1.2 };
  return { version: 8, glyphs: "https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf", sources: { o: { type: "vector", url: "https://tiles.openfreemap.org/planet" } }, layers: [
    { id: "land", type: "background", paint: { "background-color": land } },
    { id: "park", type: "fill", ...src, "source-layer": "park", paint: { "fill-color": grass } },
    { id: "cover", type: "fill", ...src, "source-layer": "landcover", filter: ["in", ["get", "class"], ["literal", ["grass", "wood", "farmland", "wetland"]]], paint: { "fill-color": ["match", ["get", "class"], "wood", trees, "grass", grass, meadow] } },
    { id: "use", type: "fill", ...src, "source-layer": "landuse", filter: ["in", ["get", "class"], ["literal", ["cemetery", "pitch", "playground", "garden", "stadium"]]], paint: { "fill-color": meadow } },
    { id: "water", type: "fill", ...src, "source-layer": "water", paint: { "fill-color": water } },
    { id: "river", type: "line", ...src, "source-layer": "waterway", paint: { "line-color": water, "line-width": ["interpolate", ["linear"], ["zoom"], 12, 1, 17, 6] } },
    { id: "path", type: "line", ...src, "source-layer": "transportation", filter: ["==", ["get", "class"], "path"], minzoom: 14, layout: { "line-cap": "round" }, paint: { "line-color": road, "line-opacity": 0.85, "line-width": ["interpolate", ["linear"], ["zoom"], 14, 0.8, 18, 2.5] } },
    { id: "road", type: "line", ...src, "source-layer": "transportation", filter: ["!", ["in", ["get", "class"], ["literal", ["path", "rail", "transit", "ferry", "aerialway", "track"]]]], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": road, "line-width": ["interpolate", ["exponential", 1.6], ["zoom"], 12, ["match", ["get", "class"], ["motorway", "trunk", "primary"], 2.5, ["secondary", "tertiary"], 1.6, 0.6], 17, ["match", ["get", "class"], ["motorway", "trunk", "primary"], 18, ["secondary", "tertiary"], 13, ["service"], 5, 9]] } },
    { id: "street", type: "symbol", ...src, "source-layer": "transportation_name", minzoom: 15, layout: { "symbol-placement": "line", "text-field": ["get", "name"], "text-font": font, "text-size": 9, "text-letter-spacing": 0.02 }, paint: { "text-color": m === PALETTES[4].map ? "#6f7177" : "#9a9a9a", ...halo } },
    { id: "lake", type: "symbol", ...src, "source-layer": "water_name", filter: ["==", ["geometry-type"], "Point"], layout: { "text-field": ["get", "name"], "text-font": font, "text-size": 11 }, paint: { "text-color": waterLabel } },
    { id: "parkname", type: "symbol", ...src, "source-layer": "park", filter: ["==", ["geometry-type"], "Point"], layout: { "text-field": ["get", "name"], "text-font": font, "text-size": 11, "text-max-width": 8 }, paint: { "text-color": parkLabel } }
  ] };
}
const ATTR = stays ? "OpenFreeMap © OpenMapTiles © OpenStreetMap" : "© OpenFreeMap © OpenStreetMap contributors";
if (gl) {
  vector = L.maplibreGL({ style: style(PALETTES[0].map), interactive: false, attributionControl: { customAttribution: ATTR } }).addTo(map);
} else {
  L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", { maxZoom: 19, attribution: "© OpenStreetMap contributors" }).addTo(map);
}
if (stays) {
  const ctl = document.createElement("div");
  ctl.className = "zoom";
  ctl.innerHTML = '<button aria-label="Zoom in">+</button><button aria-label="Zoom out">−</button>';
  ctl.children[0].onclick = () => map.zoomIn(0.5);
  ctl.children[1].onclick = () => map.zoomOut(0.5);
  $("card").appendChild(ctl);
}
// The credit folds into an "i" a few seconds in (a click shows it again).
const at = credit.getContainer();
at.onclick = () => at.classList.toggle("min");
setTimeout(() => at.classList.add("min"), 4000);

const here = [DATA.origin.lat, DATA.origin.lng];
const landmark = stays && DATA.origin.landmark !== false;
const icon = (html) => L.divIcon({ className: "", html, iconSize: [0, 0] });
L.marker(here, { icon: icon(landmark ? '<div class="lm"><i></i><div>' + ICON.landmark + esc(DATA.origin.name) + "</div></div>" : '<div class="me" title="' + esc(DATA.origin.name) + '"></div>'), keyboard: false, zIndexOffset: landmark ? 500 : 0 }).addTo(map);

const unit = { walk: "walk", bike: "by bike", drive: "drive", transit: "by transit" };
const short = (price) => String(price || "").replace(/[.,]\d{2}(?=\D*$)/, "");
const pins = places.map((p, i) => {
  const other = stays && !picks.includes(i);
  const html = stays
    ? '<div class="pin' + (other ? " other" : "") + '"><div class="pill">' + (other ? "" : "<em></em>") + "<b>" + esc(p.name) + "</b><span>" + esc(short(p.price)) + "</span></div><i></i></div>"
    : '<div class="pin"><div class="pill"><b>' + esc(p.name) + "</b><span>" + esc(p.minutes) + " min</span></div><i></i></div>";
  const m = L.marker([p.lat, p.lng], { icon: icon(html), title: p.name, keyboard: false }).addTo(map);
  m.on("click", () => select(i));
  return m;
});

let route = null;
function drawRoute(p) {
  if (route) route.remove();
  const path = p.route && p.route.length > 1 ? p.route : [here, [p.lat, p.lng]];
  route = L.polyline(path, { className: "route", weight: 3, opacity: 1, lineCap: "round", lineJoin: "round", interactive: false }).addTo(map);
  const el = route.getElement();
  if (el && el.getTotalLength) {
    const len = el.getTotalLength();
    el.style.strokeDasharray = len;
    el.style.strokeDashoffset = len;
    el.getBoundingClientRect();
    el.style.transition = "stroke-dashoffset .3s ease-out, stroke .3s";
    el.style.strokeDashoffset = 0;
    setTimeout(() => { el.style.strokeDasharray = ""; }, 350);
  }
  return path;
}

// The camera keeps the origin, the picks and the route to the chosen place in view.
const everywhere = [here].concat(picks.map((i) => [places[i].lat, places[i].lng]));
function frame(points, animate) {
  const pad = { paddingTopLeft: [44, 50], paddingBottomRight: [44, stays ? 124 : 112], maxZoom: 17 };
  const b = L.latLngBounds(points);
  if (animate) map.flyToBounds(b, Object.assign({ duration: 0.45, easeLinearity: 0.5 }, pad)); else map.fitBounds(b, pad);
}

function fill(p) {
  const pick = picks.indexOf(sel);
  const star = p.rating ? "<span>" + ICON.star + esc(p.rating) + "</span>" : "";
  $("title").innerHTML = "<b>" + esc(p.name) + "</b>" + star;
  const t = $("thumb");
  t.style.backgroundImage = /^https:\/\//.test(p.photo || "") ? 'url("' + encodeURI(p.photo) + '")' : "";
  t.innerHTML = t.style.backgroundImage ? "" : stays ? ICON.bed : ICON.place;
  if (stays) {
    $("sub").innerHTML = esc(p.label || (pick < 0 ? "In this search" : p.kind || "")) + (p.price ? "<em> · " + esc(p.price) + " a night</em>" : "");
    const far = p.distance || (p.minutes != null ? p.minutes + " min to " + DATA.origin.name : "");
    $("tags").innerHTML = far ? "<span>" + (landmark ? ICON.landmark : ICON.pin) + esc(far) + "</span>" : "";
    $("pager").innerHTML = pick < 0 && picks.length ? '<button class="back" id="back">' + ICON.prev + "Back to picks</button>" : pager(pick);
    const s = $("send");
    s.hidden = !/^https:\/\//.test(p.url || "");
    if (!s.hidden) s.setAttribute("data-url", p.url);
  } else {
    $("sub").textContent = [p.kind, p.minutes != null ? p.minutes + " min " + (unit[p.mode] || "away") : ""].filter(Boolean).join(" · ");
    $("tags").innerHTML = (p.tags || []).slice(0, 3).map((g) => "<span>" + tagIcon(g) + esc(g.label || g) + "</span>").join("");
    $("pager").innerHTML = pager(sel);
  }
  const back = $("back");
  if (back) back.onclick = () => select(picks[0]);
  if ($("prev")) { $("prev").onclick = () => step(-1); $("next").onclick = () => step(1); }
  document.querySelectorAll(".tile").forEach((b) => b.classList.toggle("on", +b.dataset.i === sel));
}
function pager(k) { return picks.length > 1 ? '<button id="prev" aria-label="Previous place">' + ICON.prev + "</button><span>" + (k + 1) + " / " + picks.length + '</span><button id="next" aria-label="Next place">' + ICON.next + "</button>" : ""; }
function step(d) { const k = picks.indexOf(sel); select(picks[((k < 0 ? 0 : k + d) + picks.length) % picks.length]); }

// `camera`: "fly" to the selection (a click), or "none" (the first draw).
function select(i, camera) {
  sel = i;
  const p = places[sel];
  pins.forEach((m, j) => {
    const el = m.getElement();
    if (!el) return;
    el.firstChild.classList.toggle("on", j === sel);
    m.setZIndexOffset(j === sel ? 1000 : picks.includes(j) ? 100 : 0);
  });
  const info = $("info");
  if (camera === "none") fill(p);
  else { info.classList.add("swap"); setTimeout(() => { fill(p); info.classList.remove("swap"); }, 110); }
  const path = drawRoute(p);
  if (camera !== "none" && !stays) frame(everywhere.concat(path), true);
}
$("card").addEventListener("keydown", (e) => {
  if (e.key === "ArrowRight") { step(1); e.preventDefault(); }
  if (e.key === "ArrowLeft") { step(-1); e.preventDefault(); }
});

// The palette: CSS variables for the chrome, a new style for the vector map, a filter for raster.
function apply(p) {
  palette = p;
  const c = Object.assign({}, LIGHT, p);
  const s = $("card").style;
  const set = (k, v) => s.setProperty("--" + k.replace(/[A-Z]/g, (x) => "-" + x.toLowerCase()), v);
  ["panel", "fg", "sub", "faint", "line", "blank", "pin", "pinFg", "pinMin", "sel", "selFg", "selMin", "ringW", "picker", "pickerLine", "pickerFg", "ring", "btn", "btnFg", "accent"].forEach((k) => set(k, c[k]));
  set("land", p.map[0]);
  set("tiles", vector ? "none" : p.tiles);
  document.body.classList.toggle("night", !!p.dark);
  if (vector && vector.getMaplibreMap()) vector.getMaplibreMap().setStyle(style(p.map));
  const pk = $("picker");
  if (pk) {
    pk.querySelectorAll(".sw").forEach((b) => { const on = b.dataset.name === p.name; b.classList.toggle("on", on); b.setAttribute("aria-checked", on); });
    $("picker-name").textContent = p.name;
  }
}
// A swatch re-themes the card in a circle that grows from the middle of the map.
function reveal(p) {
  if (!document.startViewTransition || matchMedia("(prefers-reduced-motion: reduce)").matches) return apply(p);
  const r = $("card").getBoundingClientRect();
  const x = r.left + r.width * 0.41, y = r.top + r.height * 0.4;
  const de = document.documentElement;
  const mask = "radial-gradient(circle at " + x + "px " + y + "px, #000 calc(var(--reveal) - 14px), transparent var(--reveal))";
  $("reveal").textContent = "::view-transition-new(root) { -webkit-mask-image: " + mask + "; mask-image: " + mask + "; }";
  const far = Math.hypot(Math.max(x - r.left, r.right - x), Math.max(y - r.top, r.bottom - y)) + 20;
  // The map redraws at once, so the new picture has its new colours too.
  const t = document.startViewTransition(() => {
    apply(p);
    const mm = vector && vector.getMaplibreMap();
    if (mm && mm.isStyleLoaded()) mm.redraw();
  });
  t.ready.then(() => de.animate({ "--reveal": ["0px", far + "px"] }, { duration: 480, easing: "cubic-bezier(.45,0,.3,1)", fill: "forwards", pseudoElement: "::view-transition-new(root)" })).catch(() => {});
}
const picker = $("picker");
let shut = 0;
PALETTES.forEach((p, k) => {
  const b = document.createElement("button");
  b.className = "sw";
  b.dataset.name = p.name;
  b.title = p.name;
  b.setAttribute("role", "radio");
  b.style.setProperty("--k", PALETTES.length - 1 - k);
  b.style.background = "linear-gradient(135deg, " + p.sw[0] + " 50%, " + p.sw[1] + " 50%)";
  b.onmouseenter = () => { $("picker-name").textContent = p.name; };
  b.onclick = (e) => {
    if (!picker.classList.contains("open")) return;
    e.stopPropagation();
    chosen = true;
    if (p !== palette) reveal(p);
  };
  picker.appendChild(b);
});
picker.onclick = () => picker.classList.add("open");
picker.onmouseenter = () => clearTimeout(shut);
picker.onmouseleave = () => { $("picker-name").textContent = palette.name; shut = setTimeout(() => picker.classList.remove("open"), 400); };
document.addEventListener("click", (e) => { if (!picker.contains(e.target)) picker.classList.remove("open"); });
function themePalette() {
  if (chosen) return;
  const dark = getComputedStyle(document.documentElement).colorScheme.includes("dark");
  apply(PALETTES[dark ? 4 : 0]);
}
themePalette();
const themeSheet = document.getElementById("trek-theme");
if (themeSheet) new MutationObserver(themePalette).observe(themeSheet, { childList: true, characterData: true, subtree: true });

// Send to iPhone: Trek posts the text as the user's next message here. In stays mode the button
// opens the listing instead.
const send = $("send");
const rest = () => { send.className = "send"; send.innerHTML = ICON.phone + "Send to iPhone"; };
if (stays) {
  send.textContent = "View rooms";
  send.onclick = () => { const u = send.getAttribute("data-url"); if (u) { const a = document.createElement("a"); a.href = u; a.target = "_blank"; a.rel = "noopener"; a.click(); } };
} else {
  rest();
  if (!window.trek) send.hidden = true;
  send.onclick = () => {
    if (send.disabled || !window.trek || !window.trek.send(DATA.send)) return;
    send.disabled = true;
    send.style.width = send.offsetWidth + "px";
    send.className = "send busy";
    send.innerHTML = ICON.spin + "Sending…";
    setTimeout(() => { send.className = "send"; send.innerHTML = ICON.check + "Sent to iPhone"; }, 800);
  };
}

// Stays: the line above the map, the photo strip and the filter chips under it.
if (stays && DATA.header) $("head").innerHTML = '<div class="head"><b>' + esc(DATA.header.title) + "</b><span>" + (DATA.header.detail ? "· " + esc(DATA.header.detail) : "") + "</span></div>";
if (stays && DATA.photos) {
  const strip = document.createElement("div");
  strip.className = "strip";
  strip.innerHTML = picks.map((i) => {
    const p = places[i], ok = /^https:\/\//.test(p.photo || "");
    return '<button class="tile" data-i="' + i + '"><div' + (ok ? ' style="background-image:url(&quot;' + esc(encodeURI(p.photo)) + '&quot;)"' : "") + ">" + (p.rating ? '<span class="r">' + ICON.star + esc(p.rating) + "</span>" : "") + (ok ? "" : ICON.bed) + "</div><b>" + esc(p.name) + "</b>" + (p.price ? "<span><b>" + esc(p.price) + "</b> a night</span>" : "") + "</button>";
  }).join("");
  strip.querySelectorAll(".tile").forEach((b) => { b.onclick = () => select(+b.dataset.i); });
  $("below").appendChild(strip);
}
if (stays && DATA.chips && DATA.chips.length) {
  const row = document.createElement("div");
  row.className = "chips";
  DATA.chips.forEach((c) => {
    const b = document.createElement("button");
    b.className = "chip";
    b.innerHTML = (c.icon === "star" ? ICON.star.replace("<svg", '<svg style="fill:#e99f21"') : ICON[c.icon] || ICON.tag) + esc(c.label) + (c.count != null ? "<em>" + esc(c.count) + "</em>" : "");
    b.onclick = () => { if (c.send && window.trek) window.trek.send(c.send); };
    row.appendChild(b);
  });
  $("below").appendChild(row);
}

// Open on the places, then ease in on the pick and the route to it.
frame(everywhere.concat(places[sel].route || []), false);
select(sel, "none");
</script>
</body>
</html>
```
