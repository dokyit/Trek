// record.mjs OUT_DIR URL WIDTH HEIGHT SCALE SECONDS [light|dark] [css selector to bring into view]
// A real-time recording of a page in headless Chrome, over the DevTools protocol's screencast:
// every frame the browser really drew, with its time. Nothing appears on screen.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [out, url, w, h, scale, secs, scheme = "light", into = "", tour = ""] = process.argv.slice(2);
// A walk down the page for a recording: a pause at the top, then each section in turn, with time
// for the Inbox example to play.
const TOUR = `(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const glide = (to, ms) => new Promise((done) => { const from = scrollY, t0 = performance.now();
    (function f(t) { const k = Math.min(1, (t - t0) / ms), e = k < 0.5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
      scrollTo({ top: from + (to - from) * e, behavior: "instant" }); k < 1 ? requestAnimationFrame(f) : done(); })(t0); });
  const y = (sel, off) => document.querySelector(sel).getBoundingClientRect().top + scrollY + off;
  await sleep(3500);
  await glide(y("#inbox", -40), 2600); await sleep(10500);
  await glide(y("#ios", -30), 2200); await sleep(2600);
  await glide(y("#iphone", -30), 2200); await sleep(2400);
  await glide(y("#install", -30), 2000); await sleep(2500);
})()`;
rmSync(out, { recursive: true, force: true }); mkdirSync(out, { recursive: true });
const profile = mkdtempSync(join(tmpdir(), "trek-rec-"));
const port = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", [
  "--headless=new", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions",
  "--hide-scrollbars", `--remote-debugging-port=${port}`, `--window-size=${w},${h}`, "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try { target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page"); } catch {}
}
if (!target) { chrome.kill(); throw new Error("no page target"); }
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const waiting = new Map(); const frames = []; let loaded;
const onLoad = new Promise((r) => (loaded = r));
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m.result); waiting.delete(m.id); }
  if (m.method === "Page.loadEventFired") loaded();
  if (m.method === "Page.screencastFrame") {
    const n = String(frames.length).padStart(5, "0");
    writeFileSync(join(out, `f${n}.png`), Buffer.from(m.params.data, "base64"));
    frames.push(m.params.metadata.timestamp);
    send("Page.screencastFrameAck", { sessionId: m.params.sessionId });
  }
};
function send(method, params = {}) { const n = ++id; ws.send(JSON.stringify({ id: n, method, params })); return new Promise((r) => waiting.set(n, r)); }

await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: +w, height: +h, deviceScaleFactor: +scale, mobile: false });
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
await send("Page.navigate", { url });
await onLoad;
if (/^\d+$/.test(into)) await send("Runtime.evaluate", { expression: `scrollTo(0, ${into})` });
else if (into) await send("Runtime.evaluate", { expression: `document.querySelector(${JSON.stringify(into)}).scrollIntoView({ block: "center" })` });
await send("Runtime.evaluate", { expression: "document.fonts.ready.then(() => 1)", awaitPromise: true });
await send("Page.startScreencast", { format: "png", everyNthFrame: 1 });
if (tour) send("Runtime.evaluate", { expression: TOUR });
await sleep(+secs * 1000);
await send("Page.stopScreencast");
// ffmpeg's concat list: each frame held until the next one's time
const t0 = frames[0];
let list = "";
frames.forEach((t, i) => { const d = (i + 1 < frames.length ? frames[i + 1] - t : 1 / 30); list += `file 'f${String(i).padStart(5, "0")}.png'\nduration ${d.toFixed(4)}\n`; });
if (frames.length) list += `file 'f${String(frames.length - 1).padStart(5, "0")}.png'\n`;
writeFileSync(join(out, "list.txt"), list);
console.log(`${frames.length} frames over ${(frames.at(-1) - t0).toFixed(2)} s`);
ws.close(); chrome.kill();
await sleep(600);
try { rmSync(profile, { recursive: true, force: true }); } catch {}
process.exit(0);
