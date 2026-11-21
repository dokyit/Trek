/* trekdesktop.com
   1. The download: which build is the visitor's, and where the latest release's files are.
   2. The bar, which gains a ground once the page has moved under it.
   3. The first screen's window, which lies back and rises flat as the page scrolls.
   4. Things that come in as they are reached, and the app's recordings, which play only while seen.
   The page reads whole without any of this. The only request it makes is to GitHub's API. */
(() => {
  "use strict";

  const all = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
  document.documentElement.classList.add("js");

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
    "linux-appimage": /^Trek-[\w.+-]+-linux-x86_64\.AppImage$/,
    "linux-arm": /^Trek-[\w.+-]+-linux-aarch64\.AppImage$/,
    "linux-deb": /^trek_[\w.+~-]+_amd64\.deb$/,
  };
  // Where each link goes until the API has answered, or if it never does.
  const FALLBACK = { "mac-arm": LATEST, "mac-intel": LATEST, "win-setup": `${LATEST}/download/Trek-Setup-x86_64.exe`, "win-zip": LATEST, "linux-appimage": LATEST, "linux-arm": LATEST, "linux-deb": LATEST };
  const CHIP = { "mac-arm": "Apple silicon", "mac-intel": "Intel", "win-setup": "Windows 11, x64", "linux-appimage": "x86_64", "linux-arm": "Arm" };
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
    // Linux: offered only once a release has Linux files (`paint`), Arm where the browser says so.
    if (/Linux|X11|CrOS/i.test(`${platform} ${ua}`)) {
      const arm = /aarch64|arm64|armv8/i.test(`${platform} ${ua}`);
      return { os: "linux", build: null, linux: arm ? "linux-arm" : "linux-appimage", sure: true };
    }
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

  const link = (href, text) => {
    const a = document.createElement("a");
    a.href = href;
    a.className = "text-link";
    a.textContent = text;
    return a;
  };

  function paint(who, release) {
    const assets = (release && release.assets) || {};
    const href = (key) => (assets[key] && assets[key].url) || FALLBACK[key];

    // The Linux card, only once the latest release has a Linux build.
    const linux = Boolean(assets["linux-appimage"] || assets["linux-arm"]);
    for (const card of all("[data-linux]")) {
      card.hidden = !linux;
      card.parentElement.classList.toggle("has-linux", linux);
    }
    if (who.os === "linux" && linux && assets[who.linux]) who = { ...who, build: who.linux };

    for (const a of all("[data-build]")) {
      const key = a.dataset.build;
      a.href = href(key);
      const size = a.querySelector("[data-size]");
      if (size && assets[key] && assets[key].size) size.textContent = megabytes(assets[key].size);
      a.classList.toggle("is-yours", key === who.build);
    }

    const primary = document.querySelector("[data-dl-primary]");
    const label = document.querySelector("[data-dl-label]");
    const other = document.querySelector("[data-dl-other]");
    const fine = document.querySelector("[data-dl-fine]");
    if (!primary || !label || !fine) return;
    if (who.build && who.os === "linux") {
      primary.href = href(who.build);
      label.textContent = "Download for Linux";
      if (other) {
        other.hidden = false;
        other.href = href("linux-deb");
        other.textContent = ".deb for Ubuntu";
      }
      const size = assets[who.build].size ? `, ${megabytes(assets[who.build].size)}` : "";
      fine.textContent = `Free and open source. Version ${release.version}, ${CHIP[who.build]}${size}. Make it executable, then run it.`;
    } else if (who.build) {
      const windows = who.os === "windows";
      primary.href = href(who.build);
      label.textContent = windows ? "Download for Windows" : "Download for macOS";
      if (other) {
        other.href = windows ? href("mac-arm") : href("win-setup");
        other.textContent = windows ? "For macOS" : "For Windows";
      }
      const size = assets[who.build] && assets[who.build].size ? `, ${megabytes(assets[who.build].size)}` : "";
      const version = release && release.version ? ` Version ${release.version}, ${CHIP[who.build]}${size}.` : "";
      fine.textContent = `Free and open source.${version} The first launch asks you to `;
      fine.append(link("#install", "allow it once"), ".");
      if (who.os === "mac" && !who.sure) fine.append(" On an Intel Mac? ", link(href("mac-intel"), "Take the Intel build"), ".");
    } else if (who.os === "mobile" || who.os === "linux") {
      primary.href = "#install";
      label.textContent = "See the downloads";
      if (other) other.hidden = true;
      if (who.os === "mobile") {
        fine.textContent = "Trek is a free, open-source desktop app. Open this page on your Mac or PC to download it. On a phone, use ";
        fine.append(link("/remote/", "the\u00a0remote"), " to answer your agents.");
      } else {
        fine.textContent = "There is no Linux build yet. Trek runs on macOS and Windows.";
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

  /* ---------- 2. The bar ---------- */

  const bar = document.querySelector(".bar");

  /* ---------- 3. The first screen's window ----------
     It lies back a little, as if standing on the ground line, and comes up flat and a touch nearer
     over the first screen's height of scrolling. Where there is a mouse it turns a degree or two
     towards it. Transforms only; it runs backwards as readily. */

  const hero = document.querySelector("[data-hero]");
  const win = hero && hero.querySelector("[data-window] picture");
  const mouse = matchMedia("(hover: hover) and (pointer: fine)");
  const aim = { x: 0 }, look = { x: 0 };

  /* ---------- 4. Drift ----------
     A panel's picture moves a few pixels against the scroll, so the page has depth as it passes. */

  const drifters = new Set();
  const driftable = all("[data-drift]");

  let raf = 0, painted = "";
  function frame() {
    raf = 0;
    const y = scrollY, h = innerHeight;
    if (bar) bar.classList.toggle("is-stuck", y > 8);
    if (still.matches) return;

    let moving = false;
    if (win) {
      look.x += (aim.x - look.x) * 0.12;
      if (Math.abs(aim.x - look.x) > 0.001) moving = true; else look.x = aim.x;
      const p = clamp(y / (h * 0.75));
      const e = 1 - (1 - p) ** 3;
      const key = `${e.toFixed(4)} ${look.x.toFixed(4)}`;
      if (key !== painted) {
        painted = key;
        win.style.setProperty("--tilt", `${(10 * (1 - e)).toFixed(3)}deg`);
        win.style.setProperty("--lift", `${(-40 * e).toFixed(2)}px`);
        win.style.setProperty("--zoom", (0.94 + 0.06 * e).toFixed(4));
        win.style.rotate = `0 1 0 ${(look.x * 2.2).toFixed(3)}deg`;
      }
    }
    for (const el of drifters) {
      const box = el.getBoundingClientRect();
      const k = Number(el.dataset.drift) || 1;
      const t = clamp((box.top + box.height / 2) / h, -0.2, 1.2) - 0.5; // -0.5 at the top, 0.5 at the bottom
      const target = el.matches(".phone") ? el : el.querySelector(".shot");
      if (target) target.style.setProperty("--drift", `${(t * 36 * k).toFixed(1)}px`);
    }
    if (moving) go();
  }
  const go = () => { if (!raf) raf = requestAnimationFrame(frame); };
  addEventListener("scroll", go, { passive: true });
  addEventListener("resize", go);
  addEventListener("pointermove", (e) => {
    if (e.pointerType !== "mouse" || !mouse.matches || scrollY > innerHeight) return;
    aim.x = (e.clientX / innerWidth) * 2 - 1;
    go();
  }, { passive: true });
  document.documentElement.addEventListener("pointerleave", () => { aim.x = 0; go(); });
  go();

  if ("IntersectionObserver" in window) {
    const near = new IntersectionObserver((entries) => {
      for (const entry of entries) entry.isIntersecting ? drifters.add(entry.target) : drifters.delete(entry.target);
      go();
    }, { rootMargin: "20% 0px" });
    driftable.forEach((el) => near.observe(el));

    // Things come in once, a beat apart when they arrive together, and never more than two beats.
    const seen = new IntersectionObserver((entries) => {
      let n = 0;
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.style.setProperty("--delay", `${(Math.min(n++, 2) * 0.1).toFixed(2)}s`);
        entry.target.classList.add("is-in");
        seen.unobserve(entry.target);
      }
    }, { rootMargin: "0px 0px -12% 0px" });
    all(".reveal").forEach((el) => seen.observe(el));

    // The phones come together as their section arrives. Where the browser ties that to the scroll
    // the stylesheet does it alone; elsewhere they start apart and meet once, here.
    const phones = document.querySelector(".home .phones");
    if (phones && !still.matches && !(window.CSS && CSS.supports("animation-timeline: view()"))) {
      phones.classList.add("is-apart");
      const meet = new IntersectionObserver((entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        requestAnimationFrame(() => phones.classList.add("is-met"));
        meet.disconnect();
      }, { threshold: 0.3 });
      meet.observe(phones);
    }

    // Each recording is a still picture in the page (its poster). As one comes near, and only when
    // motion is welcome, the picture becomes the recording, which plays while it is on screen.
    // With Reduce Motion on, or without this script, the still stays.
    const narrow = matchMedia("(max-width: 600px)");
    const play = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const video = entry.target;
        if (entry.isIntersecting && !still.matches) { const p = video.play(); if (p) p.catch(() => {}); } else video.pause();
      }
    }, { threshold: 0.25 });
    // A recording made in each theme names its Night copy in data-loop-dark; an empty one means
    // there is no Night recording, and the Night still stays.
    const dark = matchMedia("(prefers-color-scheme: dark)");
    const source = (img) => (dark.matches && "loopDark" in img.dataset ? img.dataset.loopDark : img.dataset.loop);
    function upgrade(img) {
      if (!source(img)) return;
      const video = document.createElement("video");
      video.className = img.className;
      video.width = img.width; video.height = img.height;
      video.muted = true; video.loop = true; video.playsInline = true;
      video.setAttribute("muted", ""); video.setAttribute("playsinline", "");
      video.poster = img.currentSrc || img.src;
      video.src = (narrow.matches && img.dataset.loopNarrow) || source(img);
      video.setAttribute("aria-label", img.alt);
      (img.parentElement.matches("picture") ? img.parentElement : img).replaceWith(video);
      play.observe(video);
    }
    const posters = all("img[data-loop]");
    const coming = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting || still.matches) continue;
        coming.unobserve(entry.target);
        upgrade(entry.target);
      }
    }, { rootMargin: "50% 0px" });
    posters.forEach((img) => coming.observe(img));
    // The loops have no button: they stop with Reduce Motion, off screen and in a hidden tab.
    still.addEventListener("change", () => { if (still.matches) all("video").forEach((v) => v.pause()); });
    document.addEventListener("visibilitychange", () => {
      for (const v of all("video")) {
        if (document.hidden) v.pause(); else { play.unobserve(v); play.observe(v); } // asks again whether it is seen
      }
    });
  } else {
    all(".reveal").forEach((el) => el.classList.add("is-in"));
  }
})();
