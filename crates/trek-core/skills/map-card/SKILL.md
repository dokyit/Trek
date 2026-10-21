---
name: map-card
description: A live map card for an answer that recommends a few places (cafés, hotels, sights, stops) the user could go to. Pins and a "1 / N" pager share one selection, a route redraws to the chosen place, a swatch bar re-themes the map, and "Send to iPhone" hands the thread to the phone. Use when places, distances and a route matter more than a list.
---

# Map card

A `trek-html` card for an answer about **where to go**: three to six places near the user (or near a
landmark), one of them your pick. The user compares them on the map: the pins and the pager under
the map share one selection, the route redraws to whichever is chosen, the swatch at the top right
re-themes the map, and **Send to iPhone** asks Trek to put the thread on the user's phone.

Use it when the places have real coordinates and walking or travel time matters. Don't use it for
one place (say where it is), for places without coordinates, or for a list where location doesn't
matter (a table is better).

## What you fill in

Copy the card below into your answer as one fenced `trek-html` block, change the `<title>`, and
replace the `DATA` object. Keep everything else as it is.

- `origin`: where the user is (or the landmark): `{ name, lat, lng }`. Shown as a dot with a halo.
- `places`: one object per place, your pick first:
  - `name`, `lat`, `lng` (decimal degrees, real ones: look them up, never guess),
  - `minutes` (travel time, a number) and `mode` (`"walk"`, `"bike"`, `"drive"` or `"transit"`),
  - `kind` ("Coffee shop", "Bakery café", "Hotel"), `rating` (number or `null`),
  - `tags`: up to three short facts ("Fast Wi-Fi", "Outlets", "Open till 9"),
  - `photo`: an https image URL or `null` (a tile with the initials shows instead),
  - `route`: the path from `origin` as `[[lat, lng], …]` along real streets, or `[]` for a straight line.
- `selected`: the index of the place shown first (normally `0`).
- `send`: what **Send to iPhone** sends. Keep `"Open this on my iPhone"`: Trek knows it.

Write the takeaway in prose under the card ("My pick is …, five minutes away"), so the answer
reads without it.

## Rules

- One card per answer. Keep the page under 200 KB: no inlined images (link https photos), at most
  eight places, routes of at most 60 points each.
- Leaflet comes from unpkg and the map tiles from OpenStreetMap (CARTO's if those don't load), all
  over https. Keep the attribution.
- The card styles itself with Trek's `--trek-*` variables, so it fits Night and Paper; the map's
  own palette starts as Night in a dark theme and Vivid in a light one.
- **Send to iPhone** calls `trek.send(DATA.send)`: Trek posts that text as the user's next message
  in this thread, marked as from the card. Act on it when it arrives: say the thread is on their
  phone (Trek shows it there). Nothing else on the card reaches Trek.

## The card

```trek-html
<!doctype html>
<html lang="en">
<head>
<title>Quiet cafés near Linden Park</title>
<link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css">
<script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
<style>
  body { padding: 4px 0 8px; }
  .card { position: relative; height: 380px; border: 1px solid var(--trek-border, #e3e3e3); border-radius: 18px; overflow: hidden; background: var(--trek-surface, #f6f6f6); font-family: var(--trek-font, system-ui); color: var(--trek-fg, #1d1d1f); }
  #map { position: absolute; inset: 0; background: var(--paper, #eef0ea); }
  .leaflet-tile-pane { filter: var(--tiles, none); transition: filter .45s ease; }
  .leaflet-container { font-family: inherit; }
  .leaflet-control-attribution { font-size: 10px; background: color-mix(in srgb, var(--trek-surface, #fff) 80%, transparent) !important; color: var(--trek-muted, #777); border-radius: 0 0 8px 0; }
  .leaflet-control-attribution a { color: inherit; }
  .pin { position: relative; transform: translate(-50%, calc(-100% - 7px)); display: inline-flex; align-items: baseline; gap: 6px; white-space: nowrap; padding: 5px 10px; border-radius: 999px; background: var(--pin, #fff); color: var(--pin-fg, #1d1d1f); font-size: 12.5px; font-weight: 600; box-shadow: 0 1px 2px rgba(0,0,0,.18), 0 4px 12px rgba(0,0,0,.12); cursor: pointer; transition: background .3s, color .3s, transform .2s; }
  .pin span { font-weight: 500; opacity: .6; }
  .pin::after { content: ""; position: absolute; left: 50%; bottom: -9px; width: 7px; height: 7px; margin-left: -3.5px; border-radius: 50%; background: var(--pin-on, #111); border: 1.5px solid #fff; }
  .pin:hover { transform: translate(-50%, calc(-100% - 9px)); }
  .pin.on { background: var(--pin-on, #111); color: var(--pin-on-fg, #fff); z-index: 2; }
  .leaflet-marker-icon.chosen { z-index: 1000 !important; }
  .me { width: 14px; height: 14px; margin: -7px 0 0 -7px; border-radius: 50%; background: var(--route, #2f6be8); border: 2.5px solid #fff; box-shadow: 0 0 0 7px color-mix(in srgb, var(--route, #2f6be8) 22%, transparent); transition: background .45s; }
  path.route { stroke: var(--route, #2f6be8); transition: stroke .45s; }
  .swatch { position: absolute; top: 12px; right: 12px; z-index: 500; display: flex; align-items: center; gap: 6px; padding: 4px; border-radius: 999px; background: var(--trek-surface, #fff); box-shadow: 0 2px 10px rgba(0,0,0,.16); }
  .swatch .name { font-size: 12px; font-weight: 600; padding: 0 4px 0 8px; min-width: 46px; display: none; }
  .swatch button { width: 24px; height: 24px; border-radius: 50%; border: 0; padding: 0; cursor: pointer; outline: 2px solid transparent; outline-offset: 1px; transition: outline-color .2s, transform .15s; }
  .swatch button:hover { transform: scale(1.1); }
  .swatch button.on { outline-color: var(--trek-fg, #111); }
  .swatch:not(.open) button:not(.on) { display: none; }
  .swatch.open .name { display: block; }
  .info { position: absolute; left: 12px; right: 12px; bottom: 12px; z-index: 500; display: grid; grid-template-columns: 56px 1fr auto; gap: 4px 12px; align-items: center; padding: 12px; border-radius: 14px; background: var(--card, var(--trek-surface, #fff)); box-shadow: 0 2px 14px rgba(0,0,0,.16); transition: background .45s; }
  .thumb { grid-row: span 2; width: 56px; height: 56px; border-radius: 10px; background: color-mix(in srgb, var(--trek-fg, #000) 8%, transparent) center / cover no-repeat; display: grid; place-items: center; font-weight: 700; color: var(--trek-muted, #888); }
  .body { min-width: 0; transition: opacity .2s, filter .2s; }
  .body.swap { opacity: 0; filter: blur(3px); }
  .title { font-weight: 650; font-size: 15px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .title .star { color: var(--trek-warning, #e0a100); font-weight: 600; font-size: 13px; margin-left: 6px; }
  .sub, .tags { font-size: 12.5px; color: var(--trek-muted, #777); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .tags { grid-column: 2; }
  .pager { grid-column: 3; grid-row: 1; justify-self: end; display: flex; align-items: center; gap: 2px; font-size: 12.5px; color: var(--trek-muted, #777); font-variant-numeric: tabular-nums; }
  .pager button { border: 0; background: none; color: inherit; font-size: 17px; line-height: 1; width: 24px; height: 24px; border-radius: 6px; cursor: pointer; }
  .pager button:hover { background: color-mix(in srgb, var(--trek-fg, #000) 8%, transparent); color: var(--trek-fg, #111); }
  .send { grid-column: 3; grid-row: 2 / span 2; align-self: end; border: 0; border-radius: 999px; padding: 7px 13px; font: inherit; font-size: 12.5px; font-weight: 600; cursor: pointer; color: var(--route, #2f6be8); background: color-mix(in srgb, var(--route, #2f6be8) 14%, transparent); transition: background .45s, color .45s; }
  .send:hover { background: color-mix(in srgb, var(--route, #2f6be8) 22%, transparent); }
  .send:disabled { cursor: default; opacity: .7; }
  @media (max-width: 420px) { .card { height: 420px; } .info { grid-template-columns: 48px 1fr; } .thumb { width: 48px; height: 48px; } .pager { grid-column: 2; grid-row: auto; justify-self: start; } .send { grid-column: 1 / -1; grid-row: auto; justify-self: stretch; } }
</style>
</head>
<body>
<div class="card" id="card" tabindex="0" aria-label="Map of the places">
  <div id="map"></div>
  <div class="swatch" id="swatch" role="radiogroup" aria-label="Map colours"><span class="name" id="swatch-name"></span></div>
  <div class="info">
    <div class="thumb" id="thumb"></div>
    <div class="body" id="body"><div class="title" id="name"></div><div class="sub" id="sub"></div></div>
    <div class="pager"><button id="prev" aria-label="Previous place">‹</button><span id="count"></span><button id="next" aria-label="Next place">›</button></div>
    <div class="tags" id="tags"></div>
    <button class="send" id="send">Send to iPhone</button>
  </div>
</div>
<script>
const DATA = {
  origin: { name: "You", lat: 52.3566, lng: 4.8643 },
  places: [
    { name: "The Boathouse", lat: 52.3601, lng: 4.8722, minutes: 5, mode: "walk", kind: "Café on the lake", rating: 4.8, tags: ["Big windows", "Plenty of tables", "Quiet"], photo: null, route: [[52.3566, 4.8643], [52.3572, 4.8668], [52.3586, 4.8697], [52.3601, 4.8722]] },
    { name: "Fern & Fig", lat: 52.3622, lng: 4.8627, minutes: 4, mode: "walk", kind: "Bakery café", rating: 4.6, tags: ["Bright tables", "Outlets", "Good espresso"], photo: null, route: [] },
    { name: "Lumen Coffee", lat: 52.3629, lng: 4.8781, minutes: 9, mode: "walk", kind: "Coffee shop", rating: 4.7, tags: ["Fast Wi-Fi", "Outlets", "Open till 9"], photo: null, route: [] }
  ],
  selected: 0,
  send: "Open this on my iPhone"
};

// The map's palettes: tiles re-coloured by a filter, and the route, pins and card to match.
const PALETTES = [
  { name: "Vivid", a: "#4caf50", b: "#2f6be8", tiles: "saturate(1.2)", route: "#2f6be8", pinOn: "#111", pinOnFg: "#fff", pin: "#fff", pinFg: "#1d1d1f", card: "", paper: "#eef0ea" },
  { name: "Earth", a: "#8a8a4a", b: "#d9c7a5", tiles: "sepia(.45) saturate(.7) hue-rotate(-12deg) brightness(1.03)", route: "#c0643b", pinOn: "#4a3527", pinOnFg: "#fff", pin: "#fffaf0", pinFg: "#3b2d22", card: "#fbf5ea", paper: "#efe7d6" },
  { name: "Autumn", a: "#e08a2c", b: "#5b8fd6", tiles: "sepia(.35) saturate(1.7) hue-rotate(-22deg)", route: "#d2552b", pinOn: "#3a2a1c", pinOnFg: "#fff", pin: "#fff", pinFg: "#3a2a1c", card: "", paper: "#f3e6cf" },
  { name: "Paper", a: "#e6e6e6", b: "#bdbdbd", tiles: "grayscale(1) contrast(.92) brightness(1.06)", route: "#333", pinOn: "#222", pinOnFg: "#fff", pin: "#fff", pinFg: "#222", card: "", paper: "#f1f1f1" },
  { name: "Night", a: "#1f3b33", b: "#1b2a4a", tiles: "invert(1) hue-rotate(180deg) brightness(.82) contrast(.92) saturate(.6)", route: "#5ea0ff", pinOn: "#f2f2f2", pinOnFg: "#111", pin: "#2a2d33", pinFg: "#eee", card: "", paper: "#1c1f24" }
];

const $ = (id) => document.getElementById(id);
const places = DATA.places;
let sel = Math.min(Math.max(DATA.selected | 0, 0), places.length - 1);
let chosenPalette = null;

const map = L.map("map", { zoomControl: false, attributionControl: false, scrollWheelZoom: false, dragging: !L.Browser.mobile, tap: true, zoomSnap: 0.25 });
L.control.attribution({ position: "topleft", prefix: false }).addTo(map);
const osm = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
const carto = "https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png";
const tiles = L.tileLayer(osm, { maxZoom: 19, attribution: "© OpenStreetMap contributors" }).addTo(map);
let failed = 0;
tiles.on("tileerror", () => { if (++failed === 3) { tiles.setUrl(carto); tiles.options.attribution = "© OpenStreetMap contributors © CARTO"; } });

const here = [DATA.origin.lat, DATA.origin.lng];
// The camera keeps the user and every place in view, and the route to the chosen one.
const everywhere = [here].concat(places.map((p) => [p.lat, p.lng]));
L.marker(here, { icon: L.divIcon({ className: "", html: '<div class="me" title="' + esc(DATA.origin.name) + '"></div>', iconSize: [0, 0] }), keyboard: false }).addTo(map);

const unit = { walk: "walk", bike: "by bike", drive: "drive", transit: "by transit" };
const pins = places.map((p, i) => {
  const m = L.marker([p.lat, p.lng], { icon: L.divIcon({ className: "", html: '<div class="pin">' + esc(p.name) + "<span>" + p.minutes + " min</span></div>", iconSize: [0, 0] }), title: p.name, riseOnHover: true }).addTo(map);
  m.on("click", () => select(i));
  return m;
});

let route = null;
function drawRoute(p) {
  if (route) route.remove();
  const path = p.route && p.route.length > 1 ? p.route : [here, [p.lat, p.lng]];
  route = L.polyline(path, { className: "route", weight: 3.5, opacity: 0.95, lineCap: "round", lineJoin: "round", interactive: false }).addTo(map);
  const el = route.getElement();
  if (el && el.getTotalLength) {
    const len = el.getTotalLength();
    el.style.strokeDasharray = len;
    el.style.strokeDashoffset = len;
    el.getBoundingClientRect();
    el.style.transition = "stroke-dashoffset .7s ease, stroke .45s";
    el.style.strokeDashoffset = 0;
    setTimeout(() => { el.style.strokeDasharray = ""; }, 750);
  }
  return path;
}

function frame(points, animate) {
  const pad = { paddingTopLeft: [40, 56], paddingBottomRight: [40, 140], maxZoom: 17 };
  const b = L.latLngBounds(points);
  if (animate) map.flyToBounds(b, Object.assign({ duration: 0.6 }, pad)); else map.fitBounds(b, pad);
}

// `camera`: "fly" to the selection (a click), or "none" (the first draw).
function select(i, camera) {
  sel = (i + places.length) % places.length;
  const p = places[sel];
  pins.forEach((m, j) => {
    const el = m.getElement();
    if (!el) return;
    el.classList.toggle("chosen", j === sel);
    el.firstChild.classList.toggle("on", j === sel);
  });
  const body = $("body");
  body.classList.add("swap");
  setTimeout(() => {
    $("name").innerHTML = esc(p.name) + (p.rating ? '<span class="star">★ ' + p.rating + "</span>" : "");
    $("sub").textContent = [p.kind, p.minutes + " min " + (unit[p.mode] || "away")].filter(Boolean).join(" · ");
    $("tags").textContent = (p.tags || []).slice(0, 3).join(" · ");
    const t = $("thumb");
    t.style.backgroundImage = p.photo ? 'url("' + encodeURI(p.photo) + '")' : "";
    t.textContent = p.photo ? "" : p.name.split(/\s+/).filter((w) => /\w/.test(w)).slice(0, 2).map((w) => w[0]).join("").toUpperCase();
    body.classList.remove("swap");
  }, camera === "none" ? 0 : 140);
  $("count").textContent = sel + 1 + " / " + places.length;
  const path = drawRoute(p);
  if (camera !== "none") frame(everywhere.concat(path), true);
}

$("prev").onclick = () => select(sel - 1);
$("next").onclick = () => select(sel + 1);
$("card").addEventListener("keydown", (e) => {
  if (e.key === "ArrowRight") { select(sel + 1); e.preventDefault(); }
  if (e.key === "ArrowLeft") { select(sel - 1); e.preventDefault(); }
});

// The swatch: one dot until clicked, then the five palettes with the hovered one's name.
const swatch = $("swatch"), label = $("swatch-name");
function applyPalette(p) {
  const s = document.documentElement.style;
  s.setProperty("--tiles", p.tiles);
  s.setProperty("--route", p.route);
  s.setProperty("--pin", p.pin);
  s.setProperty("--pin-fg", p.pinFg);
  s.setProperty("--pin-on", p.pinOn);
  s.setProperty("--pin-on-fg", p.pinOnFg);
  s.setProperty("--paper", p.paper);
  if (p.card) s.setProperty("--card", p.card); else s.removeProperty("--card");
  swatch.querySelectorAll("button").forEach((b) => { const on = b.dataset.name === p.name; b.classList.toggle("on", on); b.setAttribute("aria-checked", on); });
  label.textContent = p.name;
}
PALETTES.forEach((p) => {
  const b = document.createElement("button");
  b.dataset.name = p.name;
  b.title = p.name;
  b.setAttribute("role", "radio");
  b.style.background = "linear-gradient(135deg, " + p.a + " 50%, " + p.b + " 50%)";
  b.onmouseenter = () => { label.textContent = p.name; };
  b.onclick = (e) => {
    e.stopPropagation();
    if (!swatch.classList.contains("open")) { swatch.classList.add("open"); return; }
    chosenPalette = p;
    applyPalette(p);
    setTimeout(() => { if (!swatch.matches(":hover")) swatch.classList.remove("open"); }, 1200);
  };
  swatch.appendChild(b);
});
swatch.onmouseleave = () => { const on = swatch.querySelector("button.on"); label.textContent = on ? on.dataset.name : ""; };
document.addEventListener("click", (e) => { if (!swatch.contains(e.target)) swatch.classList.remove("open"); });
function themePalette() {
  if (chosenPalette) return;
  const dark = getComputedStyle(document.documentElement).colorScheme.includes("dark");
  applyPalette(PALETTES.find((p) => p.name === (dark ? "Night" : "Vivid")));
}
themePalette();
const themeSheet = document.getElementById("trek-theme");
if (themeSheet) new MutationObserver(themePalette).observe(themeSheet, { childList: true, characterData: true, subtree: true });

// Send to iPhone: Trek posts the text as the user's next message here.
const send = $("send");
if (!window.trek) send.hidden = true;
send.onclick = () => {
  if (!window.trek || !window.trek.send(DATA.send)) return;
  send.disabled = true;
  send.textContent = "Sent ✓";
  setTimeout(() => { send.disabled = false; send.textContent = "Send to iPhone"; }, 2000);
};

function esc(s) { return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]); }

// Open wide, then ease in on the places and the route to the pick.
map.setView(here, 13);
select(sel, "none");
setTimeout(() => frame(everywhere.concat(places[sel].route || []), true), 500);
</script>
</body>
</html>
```
