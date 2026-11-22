/* The one action: Download, for the visitor's own computer (as the live site works it out).
   Every [data-dl] link becomes the visitor's build; [data-dl-label] says which; [data-dl-fine]
   says the version and size once GitHub's API has answered; [data-build] links in the list point
   at their own files and the visitor's own is marked. The only request elsewhere is to GitHub. */
(() => {
  "use strict";
  const all = (s) => Array.from(document.querySelectorAll(s));
  const REPO = "dokyit/Trek";
  const LATEST = `https://github.com/${REPO}/releases/latest`;
  const API = `https://api.github.com/repos/${REPO}/releases/latest`;
  const DOWNLOADS = `https://github.com/${REPO}/releases/download/`;
  const BUILDS = {
    "mac-arm": /^Trek-[\w.+-]+-darwin-aarch64\.dmg$/,
    "mac-intel": /^Trek-[\w.+-]+-darwin-x86_64\.dmg$/,
    "win-setup": /^Trek-Setup-x86_64\.exe$/,
    "win-zip": /^Trek-[\w.+-]+-windows-x86_64\.zip$/,
    "linux-appimage": /^Trek-[\w.+-]+-linux-x86_64\.AppImage$/,
    "linux-arm": /^Trek-[\w.+-]+-linux-aarch64\.AppImage$/,
    "linux-deb": /^trek_[\w.+~-]+_amd64\.deb$/,
    "linux-deb-arm": /^trek_[\w.+~-]+_arm64\.deb$/,
  };
  // Each pattern is anchored at both ends, so symbols (.dSYM.zip, .pdb.zip, .debug.tar.gz),
  // tarballs and signatures (.minisig) never match, nor does one architecture's file another's.
  const FALLBACK = { "mac-arm": LATEST, "mac-intel": LATEST, "win-setup": `${LATEST}/download/Trek-Setup-x86_64.exe`, "win-zip": LATEST, "linux-appimage": LATEST, "linux-arm": LATEST, "linux-deb": LATEST, "linux-deb-arm": LATEST };
  const CHIP = { "mac-arm": "Apple silicon", "mac-intel": "Intel", "win-setup": "Windows 11, x64", "linux-appimage": "Linux x86_64", "linux-arm": "Linux Arm" };
  const mb = (b) => `${Math.round(b / 1e6)} MB`;

  function chipFromGpu() {
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      if (!gl) return null;
      const info = gl.getExtension("WEBGL_debug_renderer_info");
      const name = String(gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER) || "");
      if (/Apple M\d/i.test(name)) return "mac-arm";
      if (/Intel|AMD|Radeon|NVIDIA|GeForce/i.test(name)) return "mac-intel";
    } catch (_) { /* undecided */ }
    return null;
  }

  async function detect() {
    const ua = navigator.userAgent || "";
    const data = navigator.userAgentData;
    const platform = (data && data.platform) || navigator.platform || "";
    const touchMac = /Mac/i.test(platform) && navigator.maxTouchPoints > 1;
    if ((data && data.mobile) || /iPhone|iPad|iPod|Android/i.test(ua) || touchMac) return { os: "mobile", build: null };
    if (/Win/i.test(platform) || /Windows/i.test(ua)) return { os: "windows", build: "win-setup", sure: true };
    if (/Mac/i.test(platform) || /Macintosh/i.test(ua)) {
      let build = null;
      if (data && data.getHighEntropyValues) {
        try {
          const { architecture } = await data.getHighEntropyValues(["architecture"]);
          if (architecture === "arm") build = "mac-arm"; else if (architecture === "x86") build = "mac-intel";
        } catch (_) { /* refused */ }
      }
      if (!build) build = chipFromGpu();
      return { os: "mac", build: build || "mac-arm", sure: Boolean(build) };
    }
    // Linux: offered only once a release has Linux files (paint), Arm where the browser says so.
    if (/Linux|X11|CrOS/i.test(`${platform} ${ua}`)) {
      const arm = /aarch64|arm64|armv8/i.test(`${platform} ${ua}`);
      return { os: "linux", build: null, linux: arm ? "linux-arm" : "linux-appimage", sure: arm };
    }
    return { os: "other", build: null };
  }

  async function latest() {
    const stop = new AbortController();
    const timer = setTimeout(() => stop.abort(), 6000);
    try {
      const r = await fetch(API, { headers: { Accept: "application/vnd.github+json" }, signal: stop.signal });
      if (!r.ok) return null;
      const rel = await r.json();
      if (!rel || rel.draft || rel.prerelease || !Array.isArray(rel.assets)) return null;
      const assets = {};
      for (const a of rel.assets) {
        const url = String(a.browser_download_url || "");
        if (!url.startsWith(DOWNLOADS)) continue;
        for (const [k, re] of Object.entries(BUILDS)) if (re.test(String(a.name))) assets[k] = { url, size: Number(a.size) || 0 };
      }
      return { version: String(rel.tag_name || "").replace(/^v/, ""), assets };
    } catch (_) { return null; } finally { clearTimeout(timer); }
  }

  function paint(who, rel) {
    const assets = (rel && rel.assets) || {};
    const href = (k) => (assets[k] && assets[k].url) || FALLBACK[k];
    // A Linux visitor gets the AppImage for their processor when the browser says which, else the
    // x86_64 one with the Arm one named beside it.
    if (who.os === "linux" && assets[who.linux]) who = { ...who, build: who.linux };
    for (const a of all("[data-build]")) {
      const k = a.dataset.build;
      a.href = href(k);
      const s = a.querySelector("[data-size]");
      if (s && assets[k] && assets[k].size) s.textContent = mb(assets[k].size);
      a.classList.toggle("is-yours", k === who.build);
    }
    const label = who.build ? (who.os === "windows" ? "Download for Windows" : who.os === "linux" ? "Download for Linux" : "Download for macOS") : "See the downloads";
    for (const a of all("[data-dl]")) a.href = who.build ? href(who.build) : "#install";
    for (const el of all("[data-dl-label]")) el.textContent = el.dataset.dlShort !== undefined ? "Download" : label;
    for (const el of all("[data-dl-fine]")) {
      if (who.build) {
        const size = assets[who.build] && assets[who.build].size ? `, ${mb(assets[who.build].size)}` : "";
        const v = rel && rel.version ? ` Version ${rel.version}, ${CHIP[who.build]}${size}.` : "";
        el.textContent = `Free and open source.${v}`;
        if (who.os === "linux") {
          el.append(" A preview. Make it executable, then run it.");
          if (!who.sure && who.build === "linux-appimage" && assets["linux-arm"]) {
            const a = document.createElement("a");
            a.className = "text-link"; a.href = assets["linux-arm"].url; a.textContent = "the Arm AppImage";
            el.append(" On Arm? Take ", a, ".");
          }
        }
      } else if (who.os === "mobile") {
        el.textContent = "Trek is a desktop app for Mac, Windows and Linux. Open this page on your computer to get it.";
      } else if (who.os === "linux") {
        el.textContent = "Trek runs on Linux as a preview: an AppImage or a .deb, under Get Trek.";
      }
    }
    if (rel && rel.version) for (const el of all("[data-release]")) el.textContent = `Version ${rel.version}.`;
  }

  detect().then((who) => {
    paint(who, null);
    latest().then((rel) => { if (rel) paint(who, rel); });
  });
})();
