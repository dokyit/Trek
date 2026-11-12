/* trekdesktop.com
   1. The download: which build is the visitor's, and where the latest release's files are.
   2. The hiker, walking as he does in the app, on his trail in the story.
   3. The story in the middle of the page, played by scrolling.
   The page reads whole without any of this. The only request it makes is to GitHub's API. */
(() => {
  "use strict";

  const all = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const dark = matchMedia("(prefers-color-scheme: dark)");

  /* ---------- 1. Download ---------- */

  const REPO = "dokyit/Trek";
  const LATEST = `https://github.com/${REPO}/releases/latest`;
  const API = `https://api.github.com/repos/${REPO}/releases/latest`;
  const DOWNLOADS = `https://github.com/${REPO}/releases/download/`;

  // A release's files, by what they are. The Mac disk images carry the version in their names.
  const BUILDS = {
    "mac-arm": /^Trek-[\w.+-]+-darwin-aarch64\.dmg$/,
    "mac-intel": /^Trek-[\w.+-]+-darwin-x86_64\.dmg$/,
    "win-setup": /^Trek-Setup-x86_64\.exe$/,
    "win-zip": /^Trek-[\w.+-]+-windows-x86_64\.zip$/,
  };
  // Where each link goes until the API has answered, or if it never does.
  const FALLBACK = { "mac-arm": LATEST, "mac-intel": LATEST, "win-setup": `${LATEST}/download/Trek-Setup-x86_64.exe`, "win-zip": LATEST };
  const CHIP = { "mac-arm": "Apple silicon", "mac-intel": "Intel", "win-setup": "Windows 11, x64" };
  const megabytes = (bytes) => `${Math.round(bytes / 1e6)} MB`;

  /** The Mac's chip from its GPU's name, where the browser gives one. Chrome and Firefox name it
      ("Apple M2", "Intel Iris"); Safari says only "Apple GPU" on every Mac, which decides nothing. */
  function chipFromGpu() {
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      if (!gl) return null;
      const info = gl.getExtension("WEBGL_debug_renderer_info");
      const name = String(gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER) || "");
      if (/Apple M\d/i.test(name)) return "mac-arm";
      if (/Intel|AMD|Radeon|NVIDIA|GeForce/i.test(name)) return "mac-intel";
    } catch (_) { /* no WebGL: undecided */ }
    return null;
  }

  /** What the visitor is on: { os, build, sure }. `build` is the key of the file to offer first. */
  async function detect() {
    const ua = navigator.userAgent || "";
    const data = navigator.userAgentData;
    const platform = (data && data.platform) || navigator.platform || "";
    const touchMac = /Mac/i.test(platform) && navigator.maxTouchPoints > 1; // an iPad saying it's a Mac
    if ((data && data.mobile) || /iPhone|iPad|iPod|Android/i.test(ua) || touchMac) return { os: "mobile", build: null, sure: true };
    if (/Win/i.test(platform) || /Windows/i.test(ua)) return { os: "windows", build: "win-setup", sure: true };
    if (/Mac/i.test(platform) || /Macintosh/i.test(ua)) {
      let build = null;
      if (data && data.getHighEntropyValues) {
        try {
          const { architecture } = await data.getHighEntropyValues(["architecture"]);
          if (architecture === "arm") build = "mac-arm";
          else if (architecture === "x86") build = "mac-intel";
        } catch (_) { /* refused: undecided */ }
      }
      if (!build) build = chipFromGpu();
      // Undecided (Safari): offer Apple silicon, say so, and put the Intel build one tap away.
      return { os: "mac", build: build || "mac-arm", sure: Boolean(build) };
    }
    if (/Linux|X11|CrOS/i.test(`${platform} ${ua}`)) return { os: "linux", build: null, sure: true };
    return { os: "other", build: null, sure: false };
  }

  /** The latest release from GitHub: { version, published, assets: { key: { url, size } } }, or null. */
  async function latestRelease() {
    const stop = new AbortController();
    const timer = setTimeout(() => stop.abort(), 6000);
    try {
      const answer = await fetch(API, { headers: { Accept: "application/vnd.github+json" }, signal: stop.signal });
      if (!answer.ok) return null;
      const release = await answer.json();
      if (!release || release.draft || release.prerelease || !Array.isArray(release.assets)) return null;
      const assets = {};
      for (const asset of release.assets) {
        const url = String(asset.browser_download_url || "");
        if (!url.startsWith(DOWNLOADS)) continue; // only this repository's own release files
        for (const [key, name] of Object.entries(BUILDS)) {
          if (name.test(String(asset.name))) assets[key] = { url, size: Number(asset.size) || 0 };
        }
      }
      return { version: String(release.tag_name || "").replace(/^v/, ""), published: release.published_at, assets };
    } catch (_) {
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  function paint(who, release) {
    const assets = (release && release.assets) || {};
    const href = (key) => (assets[key] && assets[key].url) || FALLBACK[key];

    for (const link of all("[data-build]")) {
      const key = link.dataset.build;
      link.href = href(key);
      const size = link.querySelector("[data-size]");
      if (size && assets[key] && assets[key].size) size.textContent = megabytes(assets[key].size);
      link.classList.toggle("is-yours", key === who.build);
    }

    const primary = document.querySelector("[data-dl-primary]");
    const label = document.querySelector("[data-dl-label]");
    const fine = document.querySelector("[data-dl-fine]");
    if (primary && label && fine) {
      if (who.build) {
        primary.href = href(who.build);
        label.textContent = who.os === "windows" ? "Download for Windows" : "Download for macOS";
        const size = assets[who.build] && assets[who.build].size ? `, ${megabytes(assets[who.build].size)}` : "";
        const version = release && release.version ? ` Version ${release.version}, ${CHIP[who.build]}${size}.` : "";
        fine.textContent = `Free and open source.${version}`;
        if (who.os === "mac" && !who.sure) {
          const other = document.createElement("a");
          other.href = href("mac-intel");
          other.className = "text-link";
          other.textContent = "Take the Intel build";
          fine.append(" On an Intel Mac? ", other, ".");
        }
      } else if (who.os === "mobile" || who.os === "linux") {
        primary.href = "#install";
        label.textContent = "See the downloads";
        fine.textContent = who.os === "mobile"
          ? "Trek is a desktop app. Open this page on your Mac or PC to download it."
          : "There is no Linux build yet. Trek runs on macOS and Windows.";
      }
    }

    if (release && release.version) {
      const when = release.published ? new Date(release.published) : null;
      const date = when && !Number.isNaN(when.getTime())
        ? `, ${when.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" })}`
        : "";
      for (const el of all("[data-release]")) el.textContent = `Version ${release.version}${date}.`;
    }
  }

  detect().then((who) => {
    paint(who, null); // the right button at once, on the addresses that never change
    latestRelease().then((release) => { if (release) paint(who, release); });
  });

  /* ---------- 2. The hiker ----------
     As in the app (crates/trek-mascot/src/lib.rs): a 16-frames-a-second clock, 16 art pixels a
     second, a pose for every 4 pixels of ground covered, a rest and a turn at each end of the
     trail. Time moves him; scrolling only starts and stops the clock. Pressing him stops him and
     sends him on again. */

  const POSES = [
    ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....", "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....", "....KKK.....", "....KKK.....", "....KKK.....", "....KKK.....", "....bbbb...."],
    ["............", "....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....", "EEEEKKKK....", "EeEEKKKK....", "EeEEsKKK....", ".EEEKKKK....", "....KKKK....", "...kkKKKK...", "..kk...KK...", ".kk.....KK..", ".b......bbb."],
    ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....", "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....", "....KKkk....", "....KK.kk...", "....KK..k...", "....KK..bb..", "....bbb....."],
    ["............", "....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....", "EEEEKKKKK...", "EeEEKKKKKs..", "EeEEKKKK....", ".EEEKKKK....", "....KKKK....", "...KKkkkk...", "..KK...kk...", ".KK.....kk..", ".b......bbb."],
    ["....hhhh....", "..hhhhhhhh..", "....ssss....", "....ssos....", ".EE..ss.....", "EEEEKKKK....", "EEEEKKKKK...", "EeEEKKKKs...", "EeEEKKKK....", ".EEEKKKK....", "....kkKK....", "....kkKK....", "....kk.KK...", "....kk..K...", "....kk..bb..", "....bbb....."],
  ];
  // [night, paper]: the pack is ember, the body is the theme's ink at two strengths.
  const INK = {
    h: ["#C9A47C", "#B58A5C"], s: ["#EBC6A0", "#E9BF95"], o: ["#1A1A1C", "#1A1A1C"], K: ["#9A9AA2", "#55555B"],
    k: ["#5C5C63", "#9C9A95"], b: ["#8A5A2E", "#6B4423"], E: ["#FF7A3D", "#E85D1F"], e: ["#C9551F", "#B8461A"],
  };
  const FPS = 16, WALK = 16, RAMP = 0.35, REST = 1.2, STEP = 4, WIDE = 12;

  // One frame on, along a trail `travel` art pixels long. Line for line the app's Walk::step.
  function step(w, travel) {
    const dt = 1 / FPS;
    if (w.rest > 0) {
      w.rest = Math.max(0, w.rest - dt / REST);
      if (w.rest > 0) return;
    }
    travel = Math.max(travel, 1);
    const left = (w.right ? 1 - w.at : w.at) * travel;
    const accel = WALK / RAMP;
    const v = Math.min(w.speed + accel * dt, WALK, Math.sqrt(2 * accel * left));
    const d = Math.max(v * dt, 0.25);
    if (d >= left) {
      w.at = w.right ? 1 : 0; w.right = !w.right; w.speed = 0; w.rest = 1; w.walked = 0;
    } else {
      w.at += (w.right ? d : -d) / travel; w.speed = v; w.walked += d;
    }
  }
  const pose = (w) => (w.rest > 0 ? 0 : 1 + (Math.floor(w.walked / STEP) % 4));
  const facingRight = (w) => (w.rest > 0.5 ? !w.right : w.right);

  function paintHiker(canvas, p, right, night) {
    const g = canvas.getContext("2d");
    g.clearRect(0, 0, WIDE, 16);
    POSES[p].forEach((row, y) => {
      for (let x = 0; x < WIDE; x++) {
        const ink = INK[row[x]];
        if (!ink) continue;
        g.fillStyle = ink[night ? 0 : 1];
        g.fillRect(right ? x : WIDE - 1 - x, y, 1, 1);
      }
    });
  }

  // One art pixel is a whole number of device pixels, as the app's cell_size has it, also where the
  // story's block has been scaled by `k` to fit a short window.
  const cellSize = (k = 1) => Math.max(1, Math.round(3 * devicePixelRatio * k)) / (devicePixelRatio * k);

  /** Set a hiker walking. `ground(cell)` says how far he may go, in art pixels, and
      `place(canvas, x, cell)` puts him `x` art pixels along it. */
  function walk(canvas, watch, ground, place) {
    const w = { at: 0, right: true, speed: 0, rest: 0.5, walked: 0 };
    let seen = false, paused = false, timer = 0, drawn = "";

    function draw() {
      const cell = cellSize(parseFloat(getComputedStyle(canvas).getPropertyValue("--k")) || 1);
      canvas.style.setProperty("--cell", `${cell}px`);
      const travel = Math.max(1, ground(cell));
      const p = pose(w), right = facingRight(w), night = dark.matches;
      const key = `${p}${right}${night}`;
      if (key !== drawn) { paintHiker(canvas, p, right, night); drawn = key; }
      place(canvas, Math.round(w.at * travel), cell);
      return travel;
    }

    function sync() {
      const go = seen && !paused && !still.matches && document.visibilityState === "visible";
      if (go && !timer) timer = setInterval(() => { step(w, draw()); draw(); }, 1000 / FPS);
      if (!go && timer) { clearInterval(timer); timer = 0; }
      // There is no button for it: pressing the hiker himself stops him and sends him on.
      if (still.matches) {
        canvas.removeAttribute("role"); canvas.removeAttribute("tabindex"); canvas.removeAttribute("title"); canvas.setAttribute("aria-hidden", "true");
      } else {
        const label = paused ? "Let the hiker walk on" : "Stop the hiker";
        canvas.removeAttribute("aria-hidden");
        canvas.setAttribute("role", "button"); canvas.tabIndex = 0;
        canvas.setAttribute("aria-label", label); canvas.title = label;
      }
    }

    const press = () => { if (!still.matches) { paused = !paused; sync(); } };
    canvas.addEventListener("click", press);
    canvas.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); press(); } });
    new IntersectionObserver(([entry]) => { seen = entry.isIntersecting; sync(); }).observe(watch);
    document.addEventListener("visibilitychange", sync);
    still.addEventListener("change", sync);
    dark.addEventListener("change", () => { drawn = ""; draw(); });
    // A frame later, so the story has refitted its block before he is measured against it.
    addEventListener("resize", () => requestAnimationFrame(draw));
    canvas.classList.add("is-live");
    draw();
    sync();
  }

  // On a trail: straight along it.
  for (const trail of all("[data-trail]")) {
    walk(trail.querySelector("canvas"), trail,
      (cell) => Math.floor(trail.clientWidth / cell) - WIDE,
      (canvas, x, cell) => { canvas.style.translate = `${x * cell}px 0`; });
  }

  /* ---------- 3. The story ----------
     One stage, pinned while the page scrolls past it, and the scroll plays it: three threads at
     work, one stops to ask and goes to the top, the phone can answer it too, it goes back to work,
     and finished work waits. It runs backwards as readily as forwards. Without this script, and
     with Reduce Motion on, the page shows the asking moment as a still, with every line under it. */

  const story = document.querySelector("[data-story]");
  // A window too short to hold the stage (a phone on its side) gets the still, as Reduce Motion does.
  if (story && !still.matches && !matchMedia("(max-height: 559px)").matches) {
    const stage = story.querySelector(".stage"), list = story.querySelector(".rows");
    const card = story.querySelector(".ask-card"), yes = story.querySelector(".yes");
    const lines = all(".captions li", story);
    const rows = Object.fromEntries(Array.from(list.children).map((li) => [li.dataset.id, li]));
    const base = ["tests", "migrate", "tour"];
    const rank = { needs: 0, done: 1, work: 2 };
    const project = { tests: "mobile", migrate: "mobile", tour: "atlas-web" };
    const say = { work: "At work", done: "Finished", needs: "Waiting: run ./migrate?" };
    // What each moment of the story shows: [migrate, tour, the question is open].
    const SCENES = [["work", "work", false], ["needs", "work", true], ["needs", "work", true], ["work", "work", false], ["done", "done", false]];
    const ENDS = [0.18, 0.4, 0.6, 0.8]; // where one moment gives way to the next, as a share of the scroll
    const actors = story.querySelector(".actors"), captions = story.querySelector(".captions");
    let shown = -1, said = -2, target = 0, now = 0, near = 0, frame = 0, press = 0, small = 1;

    // Each line of the story comes in a word at a time.
    for (const li of lines) {
      const words = li.textContent.trim().split(/\s+/);
      li.textContent = "";
      words.forEach((word, i) => {
        const span = document.createElement("span");
        span.textContent = word;
        span.style.transitionDelay = `${170 + i * 55}ms`; // after the line before it has gone
        li.append(span, " ");
      });
    }

    function set(id, state) {
      if (rows[id].dataset.state === state) return;
      const why = rows[id].querySelector(".why"), strong = document.createElement("b");
      rows[id].dataset.state = state;
      strong.textContent = say[state];
      why.replaceChildren(strong, ` · ${project[id]}`);
    }

    // What needs you first, then what has finished, then what is still at work. Rows glide to
    // their new places; nothing jumps.
    function sort() {
      const before = new Map(base.map((id) => [id, rows[id].getBoundingClientRect().top]));
      const order = [...base].sort((a, b) => rank[rows[a].dataset.state] - rank[rows[b].dataset.state] || base.indexOf(a) - base.indexOf(b));
      if (order.every((id, i) => list.children[i] === rows[id])) return;
      order.forEach((id) => list.append(rows[id]));
      for (const id of base) {
        const d = before.get(id) - rows[id].getBoundingClientRect().top;
        // Measured on screen, moved inside the scaled block.
        if (d) rows[id].animate([{ translate: `0 ${d / small}px` }, { translate: "0 0" }], { duration: 420, easing: "cubic-bezier(0.23, 1, 0.32, 1)" });
      }
    }

    function show(i) {
      if (i === shown) return;
      const forward = i > shown;
      shown = i;
      clearTimeout(press);
      const [migrate, tour, open] = SCENES[i];
      const settle = () => { set("migrate", migrate); set("tour", tour); sort(); card.toggleAttribute("data-off", !open); yes.removeAttribute("data-down"); };
      // Going forward into "it gets back to work", Allow is pressed first.
      if (i === 3 && forward) { yes.toggleAttribute("data-down", true); press = setTimeout(settle, 220); } else settle();
    }

    // A line is said only once the stage has arrived, so the first one is not over before it is seen.
    function speak() {
      const line = near >= 0.9 ? shown : -1;
      if (line === said) return;
      said = line;
      lines.forEach((li, n) => li.toggleAttribute("data-on", n === line));
    }

    // Shrink the block to fit a short window, so it never meets the caption under it.
    function fit() {
      const tall = actors.offsetHeight; // its own height: scaling does not change it
      const room = stage.clientHeight - captions.offsetHeight - parseFloat(getComputedStyle(captions).marginBottom)
        - parseFloat(getComputedStyle(actors).marginTop) - 48;
      small = Math.min(1, Math.max(0.62, room / tall));
      stage.style.setProperty("--k", small.toFixed(3));
      stage.style.setProperty("--tall", `${tall}px`);
    }

    const clamp = (v) => Math.min(1, Math.max(0, v));
    const ease = (a, b, v) => { const t = clamp((v - a) / (b - a)); return t * t * (3 - 2 * t); };

    function tick() {
      now += (target - now) * 0.16;
      if (Math.abs(target - now) < 0.0004) now = target;
      const i = ENDS.findIndex((end) => now < end);
      show(i < 0 ? ENDS.length : i);
      speak();
      stage.style.setProperty("--enter", near.toFixed(3));
      stage.style.setProperty("--phone", (ease(0.4, 0.47, now) * (1 - ease(0.58, 0.63, now))).toFixed(3));
      frame = now === target ? 0 : requestAnimationFrame(tick);
    }

    function read() {
      const box = story.getBoundingClientRect();
      target = clamp(-box.top / (box.height - innerHeight));
      near = clamp(1 - box.top / (innerHeight * 0.75)); // the rows gather as the stage comes up
      if (!frame) frame = requestAnimationFrame(tick);
    }

    story.classList.add("is-live");
    addEventListener("scroll", read, { passive: true });
    addEventListener("resize", () => { fit(); read(); });
    fit();
    read();
  }
})();
