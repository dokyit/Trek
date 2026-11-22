// make_og.mjs [SITE_DIR] [OUT.png]
// The share picture (1200 x 630) drawn from the home page itself, in headless Chrome: Night, the
// headline over the character field, and the desk line ending in the ember dot (the window, the
// buttons and the bar are set aside for it). Nothing appears on screen. Then:
//   sips -s format jpeg -s formatOptions 88 OUT.png --out site/assets/img/og.jpg
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { serve } from "./serve.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const site = resolve(process.argv[2] || join(here, "../../site"));
const out = resolve(process.argv[3] || "og.png");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const server = await serve(0, site);
const port = server.address().port;
const profile = mkdtempSync(join(tmpdir(), "trek-og-"));
const dev = 9500 + Math.floor(Math.random() * 400);
const chrome = spawn("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", [
  "--headless=new", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions",
  "--hide-scrollbars", `--remote-debugging-port=${dev}`, "--window-size=1200,630", "about:blank",
], { stdio: "ignore" });
let target;
for (let i = 0; i < 60 && !target; i++) { await sleep(200); try { target = (await (await fetch(`http://127.0.0.1:${dev}/json`)).json()).find((t) => t.type === "page"); } catch {} }
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pend = new Map();
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result); pend.delete(m.id); } };
const send = (method, params = {}) => new Promise((res) => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
await send("Page.enable");
await send("Page.setBypassCSP", { enabled: true });
await send("Emulation.setDeviceMetricsOverride", { width: 1200, height: 630, deviceScaleFactor: 1, mobile: false });
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }, { name: "prefers-reduced-motion", value: "reduce" }] });
await send("Page.navigate", { url: `http://127.0.0.1:${port}/` });
await sleep(3000);
await send("Runtime.evaluate", { expression: `(() => {
  const s = document.createElement("style");
  s.textContent = \`
    .bar, .cta, .fine, .lede, .staged, .desk, .trail { display: none !important; }
    .hero { min-height: 630px !important; padding: 0 72px !important; grid-template-columns: 1fr !important; align-items: center !important; }
    .hero h1 { font-size: 112px !important; margin-top: -40px; }
    .og-line { position: absolute; left: 0; top: 0; width: 1200px; height: 630px; z-index: 2; }
    .og-line path { fill: none; stroke: var(--stroke); stroke-width: 3; stroke-linecap: round; }
    .og-line circle { fill: var(--ember); stroke: var(--paper); stroke-width: 4; }
    .og-mark { position: absolute; left: 72px; top: 56px; display: flex; align-items: center; gap: 10px; font: 700 34px/1 var(--display); letter-spacing: -0.035em; color: var(--ink); z-index: 3; }
    .og-mark svg { width: 38px; height: 38px; }\`;
  document.head.appendChild(s);
  const m = document.querySelector(".lockup").cloneNode(true);
  m.className = "og-mark";
  document.querySelector(".hero").appendChild(m);
  // One line across the foot of the card, ending in the ember dot.
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", "og-line"); svg.setAttribute("viewBox", "0 0 1200 630");
  const p = document.createElementNS(ns, "path"); p.setAttribute("d", "M-10 520H1012"); svg.appendChild(p);
  const c = document.createElementNS(ns, "circle"); c.setAttribute("cx", "1024"); c.setAttribute("cy", "520"); c.setAttribute("r", "12"); svg.appendChild(c);
  document.querySelector(".hero").appendChild(svg);
  dispatchEvent(new Event("resize"));
  scrollTo(0, 0);
})()` });
await sleep(1500);
const shot = await send("Page.captureScreenshot", { format: "png", clip: { x: 0, y: 0, width: 1200, height: 630, scale: 1 } });
writeFileSync(out, Buffer.from(shot.data, "base64"));
ws.close(); chrome.kill(); server.close();
setTimeout(() => rmSync(profile, { recursive: true, force: true }), 500);
console.log("wrote", out);
