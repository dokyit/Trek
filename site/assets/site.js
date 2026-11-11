/* Trek: trekdesktop.com
   1. The download block: which build is the visitor's, and where the latest release's files are.
   2. The hiker on the trail down the page, and the stretch of trail that explains how work moves.
   The page reads whole without any of this. The only request it makes is to GitHub's API. */
(() => {
  "use strict";

  const REPO = "dokyit/Trek";
  const LATEST = `https://github.com/${REPO}/releases/latest`;
  const API = `https://api.github.com/repos/${REPO}/releases/latest`;
  const DOWNLOADS = `https://github.com/${REPO}/releases/download/`;

  /* ---------- 1. Download ---------- */

  // A release's assets, by what they are. The Mac disk images carry the version in their names.
  const BUILDS = {
    "mac-arm": /^Trek-[\w.+-]+-darwin-aarch64\.dmg$/,
    "mac-intel": /^Trek-[\w.+-]+-darwin-x86_64\.dmg$/,
    "win-setup": /^Trek-Setup-x86_64\.exe$/,
    "win-zip": /^Trek-[\w.+-]+-windows-x86_64\.zip$/,
  };
  // Where each link goes until the API has answered, or if it never does.
  const FALLBACK = {
    "mac-arm": LATEST,
    "mac-intel": LATEST,
    "win-setup": `${LATEST}/download/Trek-Setup-x86_64.exe`,
    "win-zip": LATEST,
  };
  const TRUST = {
    mac: 'Signed, not yet notarized: the first time, macOS asks you to choose <b>Open Anyway</b> in Privacy &amp; Security. <a href="#install">What to click</a>.',
    windows: 'No code signature yet: SmartScreen asks once. Choose <b>More info</b>, then <b>Run anyway</b>. <a href="#install">What to click</a>.',
    linux: 'There is no Linux build yet. Trek runs on macOS and Windows. <a href="#install">Every build</a>.',
    mobile: 'Trek is a desktop app. Open this page on your Mac or PC to download it. <a href="#install">Every build</a>.',
  };

  const all = (sel, root = document) => Array.from(root.querySelectorAll(sel));
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

  const CHIP = { "mac-arm": "Apple silicon", "mac-intel": "Intel", "win-setup": "Windows 11, x64" };

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

    for (const block of all("[data-download]")) {
      const primary = block.querySelector("[data-dl-primary]");
      const label = block.querySelector("[data-dl-label]");
      const version = block.querySelector("[data-dl-version]");
      const trust = block.querySelector("[data-dl-trust]");
      if (who.build && primary && label) {
        primary.href = href(who.build);
        label.textContent = who.os === "windows" ? "Download for Windows" : "Download for macOS";
      }
      if (version && who.build) {
        const chip = CHIP[who.build];
        const size = assets[who.build] && assets[who.build].size ? `, ${megabytes(assets[who.build].size)}` : "";
        version.textContent = release && release.version ? `Version ${release.version} for ${chip}${size}.` : `For ${chip}.`;
        if (who.os === "mac" && !who.sure) {
          const other = document.createElement("a");
          other.href = href("mac-intel");
          other.textContent = "On an Intel Mac? Take the Intel build";
          version.append(" ", other, ".");
        }
      }
      if (trust && TRUST[who.os]) trust.innerHTML = TRUST[who.os];
    }

    if (release && release.version) {
      for (const el of all("[data-version]")) el.textContent = release.version;
      const dmg = assets["mac-arm"];
      if (dmg && dmg.size) for (const el of all('[data-stat="dmg-size"]')) el.textContent = String(Math.round(dmg.size / 1e6));
      const when = release.published ? new Date(release.published) : null;
      for (const el of all("[data-release]")) {
        const date = when && !Number.isNaN(when.getTime())
          ? `, released ${when.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" })}`
          : "";
        el.textContent = `The latest release is ${release.version}${date}.`;
      }
    }
  }

  detect().then((who) => {
    paint(who, null); // the right button at once, on the addresses that never change
    latestRelease().then((release) => { if (release) paint(who, release); });
  });

  /* ---------- 2. The trail ---------- */

  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const clamp = (v, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));

  // The hiker on the line down the page: it keeps its place on the screen while the trail's
  // switchbacks pass under its boots, so it steps left and right as the page scrolls.
  const rail = document.querySelector(".rail");
  const guide = rail && rail.querySelector("[data-rail-hiker]");
  const dusk = document.querySelector(".dusk");
  const ZIG = 96; // the switchback's height; assets/img/trail-zig.svg runs 8 → 36 → 8 across it

  function walkRail(moving) {
    if (!guide || !rail.offsetParent) return;
    const box = rail.getBoundingClientRect();
    const feet = guide.getBoundingClientRect().bottom;
    const along = feet - box.top;
    const turn = (((along % ZIG) + ZIG) % ZIG) / ZIG;
    const down = turn < 0.5;
    const x = 8 + 28 * (down ? turn * 2 : (1 - turn) * 2);
    guide.style.setProperty("translate", `${(x - 22).toFixed(1)}px 0`);
    guide.dataset.face = down ? "right" : "left";
    guide.dataset.pose = moving ? String(1 + (Math.floor(Math.abs(along) / 9) % 4)) : "0";
    if (dusk) guide.classList.toggle("on-dusk", feet > dusk.getBoundingClientRect().top);
  }

  // The stretch that explains how work moves: three agents on three trails. One stops at a cairn
  // because it needs you, you answer, it carries on, and all three come in at the same place.
  const route = document.querySelector("[data-route]");
  const braid = route && route.querySelector("[data-braid]");
  const lanes = braid ? {
    a: braid.querySelector(".lane-a"),
    b: braid.querySelector(".lane-b"),
    c: braid.querySelector(".lane-c"),
  } : null;
  const steps = route ? all(".step", route) : [];
  const WAIT_AT = 43; // where lane b's hiker stands, just short of its cairn
  let last = { a: -1, b: -1, c: -1 };

  function walkRoute(moving) {
    if (!braid) return;
    const box = braid.getBoundingClientRect();
    const view = window.innerHeight;
    // 0 as the trail comes up into the lower part of the screen, 1 as it nears the top.
    const t = clamp((view * 0.82 - (box.top + box.height / 2)) / (view * 0.56));
    const at = {
      a: clamp(t / 0.86) * 100,
      c: clamp(t / 0.6) * 100,
      b: t < 0.26 ? (t / 0.26) * WAIT_AT : t < 0.56 ? WAIT_AT : WAIT_AT + clamp((t - 0.56) / 0.36) * (100 - WAIT_AT),
    };
    const waiting = t >= 0.26 && t < 0.56;
    const state = {
      a: at.a >= 100 ? "done" : "working",
      b: waiting ? "needs" : at.b >= 100 ? "done" : "working",
      c: at.c >= 100 ? "done" : "working",
    };
    const width = lanes.a.getBoundingClientRect().width;
    for (const key of ["a", "b", "c"]) {
      const lane = lanes[key];
      const hiker = lane.querySelector(".hiker");
      hiker.style.setProperty("--at", at[key].toFixed(2));
      lane.dataset.state = state[key];
      const walking = moving && at[key] !== last[key] && at[key] > 0 && at[key] < 100;
      hiker.dataset.pose = walking ? String(1 + (Math.floor((at[key] / 100) * width / 9) % 4)) : "0";
    }
    last = at;
    if (t >= 0.56 && t < 0.74) lanes.b.dataset.flag = "answered"; else delete lanes.b.dataset.flag;
    const now = t < 0.26 ? 1 : t < 0.56 ? 2 : t < 0.8 ? 3 : 4;
    for (const step of steps) step.classList.toggle("is-now", Number(step.dataset.step) === now);
  }

  let queued = false;
  let rest = 0;
  function frame() {
    queued = false;
    walkRail(true);
    walkRoute(true);
    clearTimeout(rest);
    rest = setTimeout(() => { walkRail(false); walkRoute(false); }, 170); // standing once the page is
  }
  function onScroll() {
    if (!queued) { queued = true; requestAnimationFrame(frame); }
  }
  function start() {
    if (still.matches) return; // Reduce Motion: the trail and its hikers stay as the page drew them
    if (route) route.classList.add("is-live");
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll, { passive: true });
    walkRail(false);
    walkRoute(false);
  }
  if (guide || braid) start();
})();
