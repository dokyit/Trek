/* The character field: the terminal's own material, rows of characters like agents' output
   streaming side by side. At the top of the page the rows run (every agent at work) and thin out
   toward the desk, where the one line takes over; one row carries an ember caret, the thread
   waiting on you. At the end of the page the same rows lie still, settled, under the download.

   Built once (and again only when the width changes) as rows of real text. Each row holds its
   pattern twice and slides by exactly one pattern's width on the compositor (a CSS transform), so
   the loop has no seam and no script runs per frame. Rows pause while the field is off screen and
   stand still under Reduce Motion. Nothing here is measured from the font: the cell is fixed in
   CSS (`ch` of a monospace face), so there is no loop that can wait on a size that never comes.

   <div data-field="top|end"> fills its box. */
(() => {
  "use strict";
  const RAMP = " .,:;-=+*%#";
  const CODE = "{}()[]<>/=+-*;:._|";
  const MAX_ROWS = 90, MAX_COLS = 260;   // hard caps: the field never builds more than this

  function hash(n) {
    n |= 0; n = (n ^ 61) ^ (n >>> 16); n = Math.imul(n, 9); n ^= n >>> 4; n = Math.imul(n, 0x27d4eb2d); n ^= n >>> 15;
    return ((n >>> 0) / 4294967295) * 2 - 1;
  }
  function noise(x, seed) {
    const i = Math.floor(x), f = x - i, u = f * f * (3 - 2 * f);
    const a = hash(i * 57 + seed * 131), b = hash((i + 1) * 57 + seed * 131);
    return 0.5 + 0.5 * (a + (b - a) * u);
  }
  const smooth = (a, b, v) => { const t = Math.min(1, Math.max(0, (v - a) / (b - a))); return t * t * (3 - 2 * t); };

  // One row's pattern: runs of output separated by quiet, the weight of each character from noise.
  function pattern(len, seed, top) {
    let s = "";
    for (let c = 0; c < len; c++) {
      const u = c * 0.09;
      const run = smooth(top ? 0.48 : 0.42, top ? 0.66 : 0.6, noise(u * (top ? 0.45 : 0.3), seed));
      const w = noise(u * 2.3 + 40, seed + 5) * run;
      if (run <= 0.02 || w < 0.1) { s += " "; continue; }
      s += noise(u * 5 + 99, seed) > 0.8 ? CODE[Math.floor(noise(u * 7, seed + 9) * CODE.length) % CODE.length]
        : RAMP[Math.min(RAMP.length - 1, 1 + Math.floor(w * (RAMP.length - 1)))];
    }
    return s;
  }

  function field(box) {
    const top = box.dataset.field === "top";
    let built = "";
    function build() {
      const r = box.getBoundingClientRect();
      const W = r.width, H = r.height;
      if (!(W > 0) || !(H > 0) || !Number.isFinite(W) || !Number.isFinite(H)) return; // nothing to fill yet
      const narrow = W < 720;
      const cw = narrow ? 6.6 : 7.8, ch = narrow ? 13 : 16;   // the CSS cell: 11 or 13 px JetBrains Mono, 0.6em wide
      const cols = Math.min(MAX_COLS, Math.ceil(W / cw) + 2);
      const rows = Math.min(MAX_ROWS, Math.ceil(H / ch));
      const key = `${cols}x${rows}`;
      if (key === built) return;
      built = key;
      const frag = document.createDocumentFragment();
      let caretDone = false;
      for (let i = 0; i < rows; i++) {
        const fy = (i * ch) / H;
        // How much a row carries: dense at the top of the page, thinning to nothing at the desk; at
        // the end, the reverse.
        const env = top ? (1 - smooth(0.32, 0.92, fy)) * smooth(0.03, 0.13, fy) : smooth(-0.1, 0.55, fy);
        if (env < 0.06) { frag.appendChild(document.createElement("div")).className = "fr"; continue; }
        const seed = i * 17 + (top ? 1 : 900);
        const p = pattern(cols, seed, top);
        const row = document.createElement("div");
        row.className = "fr";
        const line = document.createElement("span");
        line.className = "fl";
        line.textContent = p + p;
        // A few characters a second at most: one pattern's width takes the row 40 to 110 seconds.
        const secs = top ? 40 + 70 * (0.5 + 0.5 * hash(i * 7 + 3)) : 400;
        line.style.setProperty("--d", `${secs.toFixed(1)}s`);
        line.style.setProperty("--o", (Math.min(1, env * 1.1)).toFixed(3));
        line.style.setProperty("--n", String(cols));
        // The one colour: the caret, once, in a row a third of the way down, after a run.
        if (top && !caretDone && fy > 0.22 && fy < 0.34) {
          const at = p.search(/[^ ] {3}/);
          if (at > cols * 0.3) {
            const caret = document.createElement("i");
            caret.className = "caret";
            line.textContent = "";
            line.append(p.slice(0, at + 1), caret, p.slice(at + 2), p.slice(0, at + 1));
            const c2 = caret.cloneNode();
            line.append(c2, p.slice(at + 2));
            caretDone = true;
          }
        }
        row.appendChild(line);
        frag.appendChild(row);
      }
      box.textContent = "";
      box.appendChild(frag);
    }
    // Pause the rows while the field is off screen.
    let seen = true;
    new IntersectionObserver((e) => { seen = e[0].isIntersecting; box.classList.toggle("is-off", !seen); }).observe(box);

    /* A touch: a ring of scrambled, brighter characters runs out from the spot and settles, with
       an ember spark where it began; a moving mouse leaves smaller, softer rings. Two text layers
       per ring, rewritten a frame at a time only while a ring lives (at most five, each at most
       about 50 by 25 cells); nothing is measured, nothing else moves, and the page's own clicks
       pass through untouched (the field takes no pointer events; its section listens). */
    const host = top ? box.closest("section") : box.closest(".get") || box.parentElement;
    // Only real controls and the pictures of Trek keep their touches to themselves; words over the
    // field are part of it.
    const words = "a, button, input, .btn, .screen, .phones, .links";
    const rings = [];
    let raf = 0, lastHover = 0;
    const still = matchMedia("(prefers-reduced-motion: reduce)");
    const BRIGHT = "#%*+=@&", MID = ".:-=+~";
    function cell() { const narrow = box.clientWidth < 720; return narrow ? [6.6, 13] : [7.8, 16]; }
    function ring(x, y, R, dur, spark) {
      const [cw, ch] = cell();
      const n = Math.min(60, Math.ceil((2 * R) / cw) + 1), m = Math.min(30, Math.ceil((2 * R) / ch) + 1);
      const gx = Math.round((x - R) / cw), gy = Math.round((y - R) / ch);
      const layers = ["b", "m"].map((k) => {
        const pre = document.createElement("pre");
        pre.className = `ripple ${k}`;
        pre.style.left = `${gx * cw}px`; pre.style.top = `${gy * ch}px`;
        box.appendChild(pre);
        return pre;
      });
      if (spark) {
        const s = document.createElement("i");
        s.className = "spark";
        s.style.left = `${Math.round(x / cw) * cw}px`; s.style.top = `${Math.round(y / ch) * ch + 1}px`;
        box.appendChild(s);
        setTimeout(() => s.remove(), 900);
      }
      rings.push({ x: x - gx * cw, y: y - gy * ch, R, dur, t0: performance.now(), n, m, cw, ch, layers });
      while (rings.length > 5) drop(rings[0]);
      if (still.matches) { paint(rings[rings.length - 1], 0.55); const r = rings[rings.length - 1]; setTimeout(() => drop(r), 650); return; }
      if (!raf) raf = requestAnimationFrame(frame);
    }
    function drop(r) { const i = rings.indexOf(r); if (i >= 0) rings.splice(i, 1); r.layers.forEach((l) => l.remove()); }
    function paint(r, k) {
      const rad = r.R * (1 - Math.pow(1 - k, 3));
      const band = r.cw * 2.2;
      let b = "", m = "";
      for (let j = 0; j < r.m; j++) {
        const cy = j * r.ch + r.ch / 2;
        for (let i = 0; i < r.n; i++) {
          const d = Math.hypot(i * r.cw + r.cw / 2 - r.x, cy - r.y);
          const off = Math.abs(d - rad);
          if (d > r.R) { b += " "; m += " "; }
          else if (off < band) { b += BRIGHT[(Math.random() * BRIGHT.length) | 0]; m += " "; }
          else if (off < band * 2.2 && d > rad) { b += " "; m += MID[(Math.random() * MID.length) | 0]; }
          else { b += " "; m += " "; }
        }
        b += "\n"; m += "\n";
      }
      r.layers[0].textContent = b; r.layers[1].textContent = m;
      const fade = String(Math.max(0, 1 - k * k).toFixed(3));
      r.layers[0].style.opacity = fade; r.layers[1].style.opacity = fade;
    }
    function frame(now) {
      raf = 0;
      for (const r of rings.slice()) {
        const k = (now - r.t0) / r.dur;
        if (k >= 1 || !seen) drop(r); else paint(r, k);
      }
      if (rings.length) raf = requestAnimationFrame(frame);
    }
    const at = (e) => { const b = box.getBoundingClientRect(); return [e.clientX - b.left, e.clientY - b.top, b]; };
    const inside = (x, y, b) => x >= 0 && y >= 0 && x <= b.width && y <= b.height;
    if (host) {
      host.addEventListener("pointerdown", (e) => {
        if (e.target.closest && e.target.closest(words)) return;
        const [x, y, b] = at(e);
        if (inside(x, y, b)) ring(x, y, b.width < 720 ? 120 : 180, 1100, true);
      }, { passive: true });
      host.addEventListener("pointermove", (e) => {
        if (e.pointerType !== "mouse" || still.matches) return;
        const now = performance.now();
        if (now - lastHover < 120) return;
        if (e.target.closest && e.target.closest(words)) return;
        const [x, y, b] = at(e);
        if (!inside(x, y, b)) return;
        lastHover = now;
        ring(x, y, 64, 700, false);
      }, { passive: true });
    }
    let t = 0;
    const later = () => { clearTimeout(t); t = setTimeout(build, 150); };
    addEventListener("resize", later);
    build();
    if (!built) requestAnimationFrame(build);
  }

  document.querySelectorAll("[data-field]").forEach(field);
})();
