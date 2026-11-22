/* trekdesktop.com's home page, "One line".
   1. The trail: one ink line, laid out from the page's own boxes. It is the desk under Trek's
      window, turns down the right edge, becomes the ground the phone stands on, runs down the left
      edge past everything Trek does (a waymark at each row, and the ground of the drawing beside
      Native), closes round your computer and phone, and stops at the download. It draws itself as
      the page scrolls. The ember bead is the thread that needs you: it leaves the window's stand
      and travels the line at a steady pace to the phone, where it is answered.
   2. Trek's window on the desk goes through its steps while it is on screen.
   3. The bar. */
(() => {
  "use strict";
  const NS = "http://www.w3.org/2000/svg";
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
  const f = (n) => (Math.round(n * 10) / 10).toString();
  const el = (tag, attrs = {}, parent) => {
    const n = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
    if (parent) parent.appendChild(n);
    return n;
  };

  const page = document.querySelector("[data-page]");
  const svg = document.querySelector("[data-trail]");
  const narrowQ = matchMedia("(max-width: 720px)");

  /* ---------- 2. The window ---------- */
  const box = document.querySelector("[data-inbox]");
  const inbox = window.TrekInbox && box ? window.TrekInbox.mount(box, { compact: narrowQ.matches }) : null;
  // The window's height for the line, measured once per width: on a phone the browser's toolbar
  // grows and shrinks the window as you scroll, and the line must not jump when it does.
  const probe = document.createElement("div");
  probe.setAttribute("aria-hidden", "true");
    probe.className = "vh-probe";
  document.body.appendChild(probe);
  let VH = innerHeight, lastW = innerWidth;
  const measureVH = () => { VH = probe.getBoundingClientRect().height || innerHeight; };
  measureVH();
  if (inbox) {
    const PLAN = [[0, 1800], [1, 3800], [2, 2000], [3, 3000]];
    let i = 0, timer = 0, seen = false;
    const tick = () => {
      if (!seen || still.matches || document.hidden) return;
      i = (i + 1) % PLAN.length;
      inbox.set(PLAN[i][0]);
      timer = setTimeout(tick, PLAN[i][1]);
    };
    inbox.set(still.matches ? 1 : 0, true);
    new IntersectionObserver((e) => {
      seen = e[0].isIntersecting;
      clearTimeout(timer);
      if (seen && !still.matches) timer = setTimeout(tick, PLAN[i][1]);
    }, { threshold: 0.35 }).observe(box);
  }

  /* ---------- 1. The trail ---------- */
  let boxNode = null, natTick = null, natJoin = null, endAt = null;
  let paths = [];      // { node, lut } drawn as the page reaches them, or { node, at } shown past a height
  let main = null, bead = null, tag = null, parkAt = 0, knots = [];
  let intro = null;

  function rect(node) {
    const r = node.getBoundingClientRect(), p = page.getBoundingClientRect();
    return { x: r.left - p.left, y: r.top - p.top, w: r.width, h: r.height, r: r.right - p.left, b: r.bottom - p.top };
  }

  // For each length along a path, the lowest point reached so far: a part is drawn once all of
  // it is above the height the page has scrolled to.
  // At most 400 samples a path, whatever its length (getPointAtLength is slow in WebKit).
  function lut(node) {
    const total = node.getTotalLength();
    const len = [], top = [], xs = [], ys = [];
    if (!(total > 0) || !Number.isFinite(total)) return { len: [0], top: [Infinity], xs: [0], ys: [0], total: 0 };
    const step = Math.max(6, total / 400);
    let low = -Infinity;
    for (let s = 0; ; s = Math.min(total, s + step)) {
      const p = node.getPointAtLength(s);
      low = Math.max(low, p.y);
      len.push(s); top.push(low); xs.push(p.x); ys.push(p.y);
      if (s >= total) break;
    }
    return { len, top, xs, ys, total };
  }
  function reach(l, t) {
    let a = 0, b = l.top.length - 1;
    if (l.top[0] > t) return 0;
    while (a < b) { const m = (a + b + 1) >> 1; if (l.top[m] <= t) a = m; else b = m - 1; }
    return l.len[a];
  }
  const add = (node, drawn = true) => { const p = drawn ? { node, lut: lut(node) } : { node }; paths.push(p); return p; };

  // A point in an inline drawing's own coordinates, on the page.
  function fromSvg(el, x, y) {
    const r = rect(el), vb = (el.getAttribute("viewBox") || "0 0 1 1").split(/\s+/).map(Number);
    const k = r.w / vb[2];
    return [r.x + (x - vb[0]) * k, r.y + (y - vb[1]) * k];
  }

  function build() {
    try { layoutTrail(); } catch (e) { /* the page reads whole without its line */ }
  }
  function layoutTrail() {
    if (!svg || !page) return;
    const narrow = narrowQ.matches;
    const natSvg = document.querySelector('[data-ill="native"]');
    if (natSvg && natSvg.dataset.join && natSvg.clientWidth) {
      const vb = natSvg.getAttribute("viewBox").split(/\s+/).map(Number), jy = Number(natSvg.dataset.join.split(" ")[1]);
      natSvg.style.setProperty("--below", `${f((vb[1] + vb[3] - jy) * (natSvg.clientWidth / vb[2]))}px`);
    }
    const W = page.clientWidth, H = page.scrollHeight;
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svg.textContent = "";
    paths = [];
    const q = (s) => document.querySelector(s);
    const head = rect(q(".every-head h2"));
    const rowsBox = rect(q(".rows"));
    const sL = head.x - (narrow ? 22 : 40);
    const sR = narrow ? W - 14 : Math.min(W - 24, rowsBox.r + 40);
    const R = narrow ? 16 : 26;
    const screen = rect(q("[data-screen]"));
    const deskY = screen.b + (narrow ? 22 : 58);
    // The front phone where it comes to rest, not where its arrival has it now.
    const frontEl = q("[data-phone]");
    const held = frontEl.style.transform;
    frontEl.style.transform = "none";
    const phone = rect(frontEl);
    frontEl.style.transform = held;
    const groundY = phone.b + 1;
    const end = rect(q("[data-end]"));
    const endY = end.y + end.h / 2, endX = end.x - 14;
    const priv = rect(q(".private-in"));
    const cx = screen.x + screen.w / 2;

    // The desk, from the page's edge to the stand's foot; the line runs on from there.
    add(el("path", { class: "ln", d: `M-12 ${f(deskY)}H${f(cx)}` }, svg));
    const d = [
      `M${f(cx)} ${f(deskY)}`, `H${f(sR - R)}`, `A${R} ${R} 0 0 1 ${f(sR)} ${f(deskY + R)}`,
      `V${f(groundY - R)}`, `A${R} ${R} 0 0 1 ${f(sR - R)} ${f(groundY)}`,
      `H${f(sL + R)}`, `A${R} ${R} 0 0 0 ${f(sL)} ${f(groundY + R)}`,
      `V${f(endY - R)}`, `A${R} ${R} 0 0 0 ${f(sL + R)} ${f(endY)}`, `H${f(endX)}`,
    ].join("");
    main = el("path", { class: "ln", d }, svg);
    const mainP = add(main);

    // The screen's stand: a neck, and a foot that rests on the desk.
    const nw = narrow ? 12 : 22, fw = narrow ? 64 : 120, fh = narrow ? 7 : 9;
    if (!narrow) add(el("path", { class: "stand", d: `M${f(cx - nw)} ${f(screen.b - 2)}L${f(cx - nw * 1.25)} ${f(deskY - fh)}H${f(cx + nw * 1.25)}L${f(cx + nw)} ${f(screen.b - 2)}` }, svg), false);
    if (!narrow) add(el("rect", { class: "stand", x: f(cx - fw), y: f(deskY - fh), width: f(fw * 2), height: f(fh), rx: f(fh / 2) }, svg), false);

    // Your computer and phone: a closed outline, drawn whole; the spine runs down its left side.
    const BR = narrow ? 18 : 30;
    const bd = `M${f(sL)} ${f(priv.y + BR)}A${BR} ${BR} 0 0 1 ${f(sL + BR)} ${f(priv.y)}H${f(sR - BR)}A${BR} ${BR} 0 0 1 ${f(sR)} ${f(priv.y + BR)}V${f(priv.b - BR)}A${BR} ${BR} 0 0 1 ${f(sR - BR)} ${f(priv.b)}H${f(sL + BR)}A${BR} ${BR} 0 0 1 ${f(sL)} ${f(priv.b - BR)}Z`;
    boxNode = el("path", { class: "box", d: bd }, svg);
    const lx = sL + BR + 14, ly = priv.y;
    const lbg = el("rect", { class: "label-bg", x: f(lx - 8), y: f(ly - 10), width: narrow ? 176 : 192, height: 20 }, svg);
    const lab = el("text", { class: "label", x: f(lx), y: f(ly + 4.5) }, svg);
    lab.textContent = "Your computer and phone";


    // A waymark at each row of what Trek does; at Native the mark runs on to the drawing's ground.
    const marks = [...document.querySelectorAll(".every-head, .row, .builds li:first-child")].map(rect).filter((r) => Math.abs(r.x - head.x) < 4);
    for (const r of marks) el("path", { class: "tick", d: `M${f(sL)} ${f(r.y)}H${f(r.x - 10)}` }, svg);
    const nat = q('[data-ill="native"]');
    if (nat && nat.dataset.join) {
      const [jx, jy] = nat.dataset.join.split(" ").map(Number);
      const [x, y] = fromSvg(nat, jx, jy);
      natTick = el("path", { class: "tick", d: `M${f(sL)} ${f(y)}H${f(x)}` }, svg);
      natJoin = [x, y];
    }

    // The stop at the end, and the bead with its question.
    tag = el("text", { class: "label tag" }, svg);
    tag.textContent = "Run ./migrate?";
    bead = el("circle", { class: "bead", r: narrow ? 7 : 8 }, svg);

    // Where the bead reaches the phone: on the ground, under the phone's middle.
    const L = mainP.lut, gx = phone.x + phone.w / 2;
    let best = 0, dist = Infinity;
    for (let i = 0; i < L.len.length; i++) {
      const dd = Math.hypot(L.xs[i] - gx, L.ys[i] - groundY);
      if (dd < dist) { dist = dd; best = L.len[i]; }
    }
    parkAt = best;
    // The bead moves at a steady pace with the scroll: from the stand's foot at the top of the
    // page, to the phone as the phone sits low on the screen, then on to the end.
    const top = page.getBoundingClientRect().top + scrollY;
    const maxScroll = Math.max(1, document.documentElement.scrollHeight - VH);
    const keys = [];
    let prev = 0;
    for (let i = 0; i < L.len.length; i++) {
      const level = L.top[i] + top - VH * 0.6;               // when this sample meets the reading line
      const pace = i ? prev + (L.len[i] - L.len[i - 1]) * 0.45 : 0;   // across, at a steady pace
      prev = Math.max(level, pace, i ? prev : 0);
      keys.push(prev);
    }
    const last = keys[keys.length - 1];
    if (last > maxScroll - 2) { const k = (maxScroll - 2) / last; for (let i = 0; i < keys.length; i++) keys[i] *= k; }
    knots = keys.map((y, i) => [y, L.len[i]]);
    knots.push([maxScroll, L.total]);
    endAt = [endX, endY];
    for (const p of paths) if (p.lut) p.node.style.strokeDasharray = `${p.lut.total} ${p.lut.total}`;
    paint();
  }

  function beadAt() {
    const y = scrollY;
    if (!knots.length || y <= knots[0][0]) return 0;
    let a = 0, b = knots.length - 1;
    if (y >= knots[b][0]) return knots[b][1];
    while (b - a > 1) { const m = (a + b) >> 1; if (knots[m][0] <= y) a = m; else b = m; }
    const [y0, s0] = knots[a], [y1, s1] = knots[b];
    return s0 + (s1 - s0) * clamp((y - y0) / Math.max(1e-6, y1 - y0));
  }

  function placeBead(s) {
    const p = main.getPointAtLength(s), n = main.getPointAtLength(Math.min(s + 3, paths[1].lut.total));
    bead.setAttribute("cx", f(p.x)); bead.setAttribute("cy", f(p.y));
    const tw = tag.getComputedTextLength ? tag.getComputedTextLength() : 90;
    const W = page.clientWidth;
    const flat = Math.abs(n.x - p.x) >= Math.abs(n.y - p.y);
    // Under the line where it runs flat, beside it where it runs down: never on it.
    const x = flat ? clamp(p.x - tw / 2, 12, W - tw - 12) : p.x - 16 - tw;
    const y = flat ? p.y + 25 : p.y + 4;
    tag.setAttribute("x", f(x)); tag.setAttribute("y", f(y));
    tag.classList.toggle("off", narrowQ.matches && !flat);
  }

  function paint() {
    if (!main) return;
    const top = page.getBoundingClientRect().top;
    const t = still.matches ? Infinity : -top + VH * 0.93;
    const s = still.matches ? paths[1].lut.total : beadAt();
    for (const p of paths) {
      if (p.lut) {
        let drawn = p.node === main ? s : reach(p.lut, t);
        if (intro !== null && p === paths[0]) drawn = Math.min(drawn, intro * p.lut.total);
        if (intro !== null && p.node === main) drawn = Math.min(drawn, Math.max(0, intro * 2 - 1) * drawn);
        p.node.style.strokeDashoffset = String(p.lut.total - drawn);
      } else if (p.at !== undefined) {
        p.node.style.opacity = t >= p.at ? "1" : "0";
      }
    }
    // Its words ride with it: the question until the phone, "Allowed" for a while after, then
    // nothing as it runs on to rest at Download.
    const words = s < parkAt - 1 ? "Run ./migrate?" : s < parkAt + 700 ? "Allowed on your phone" : "";
    if (tag.textContent !== words) tag.textContent = words;
    placeBead(s);
    tag.classList.toggle("gone", !words || (intro !== null && intro < 0.9));
    // The phone's Allow is tapped as the dot arrives under it.
    const front = document.querySelector("[data-phone]");
    if (front && !still.matches) {
      if (s >= parkAt - 1 && !front.classList.contains("tapped")) front.classList.add("tapped");
      else if (s < parkAt - 80) front.classList.remove("tapped");
    }
  }

  window.__trailCheck = () => {
    if (!main) return null;
    const total = paths[1].lut.total, off = Number(main.style.strokeDashoffset || 0);
    const head = main.getPointAtLength(total - off);
    const dot = [Number(bead.getAttribute("cx")), Number(bead.getAttribute("cy"))];
    const nat = document.querySelector('[data-ill="native"]');
    let natGap = null;
    if (nat && natJoin) { const [jx, jy] = nat.dataset.join.split(" ").map(Number); const [x, y] = fromSvg(nat, jx, jy); natGap = Math.hypot(x - natJoin[0], y - natJoin[1]); }
    const fe = document.querySelector("[data-phone]"), hold = fe.style.transform;
    fe.style.transform = "none"; const ph = rect(fe); fe.style.transform = hold;
    const ground = main.getPointAtLength(parkAt);
    return { drawn: total - off, total, dotToEnd: Math.hypot(dot[0] - endAt[0], dot[1] - endAt[1]), headToDot: Math.hypot(head.x - dot[0], head.y - dot[1]),
      natGap, phoneGap: Math.abs(ground.y - (ph.b + 1)), boxClosed: /Z$/.test(boxNode.getAttribute("d")), natJoin, endAt };
  };

  // The first screen draws its stretch of the line once, as the page opens.
  function start() {
    build();
    if (still.matches || !main) return;
    const t0 = performance.now(), ms = 1500;
    intro = 0;
    const step = (now) => {
      intro = clamp((now - t0) / ms);
      intro = 1 - Math.pow(1 - intro, 3);
      paint();
      if (now - t0 < ms) requestAnimationFrame(step); else { intro = null; paint(); }
    };
    requestAnimationFrame(step);
  }

  const phones = document.querySelector("[data-phones]");
  const back = phones && phones.querySelector(".phone-back"), frontP = phones && phones.querySelector(".phone-front");
  function together() {
    if (!phones || !back || !frontP) return;
    const r = phones.getBoundingClientRect();
    const k = still.matches ? 1 : clamp((VH - r.top) / (VH * 0.7));
    const e = 1 - Math.pow(1 - k, 3), a = 1 - e;
    const narrow = narrowQ.matches;
    if (narrow) {
      back.style.setProperty("--bx", "0px"); back.style.setProperty("--by", `${f(40 * a)}px`); back.style.setProperty("--br", "0deg");
      frontP.style.setProperty("--fx", "0px"); frontP.style.setProperty("--fy", `${f(28 * a)}px`); frontP.style.setProperty("--fr", "0deg");
      phones.style.setProperty("--fade", f(0.35 + 0.65 * e));
    } else {
      back.style.setProperty("--bx", `${f(-90 * a)}px`); back.style.setProperty("--by", `${f(46 * a)}px`); back.style.setProperty("--br", `${f(1 * a)}deg`);
      frontP.style.setProperty("--fx", `${f(110 * a)}px`); frontP.style.setProperty("--fy", `${f(70 * a)}px`); frontP.style.setProperty("--fr", `${f(6 * a)}deg`);
      phones.style.setProperty("--fade", "1");
    }
  }
  // Drawings and phones pause their loops while off screen.
  const pause = new IntersectionObserver((es) => { for (const e of es) e.target.classList.toggle("is-off", !e.isIntersecting); });
  document.querySelectorAll("svg[data-ill], [data-phones]").forEach((n) => pause.observe(n));

  let raf = 0;
  addEventListener("scroll", () => { if (!raf) raf = requestAnimationFrame(() => { raf = 0; paint(); together(); }); }, { passive: true });
  together();
  let rt = 0;
  // A resize that only changes the height (a phone's toolbar) changes nothing here.
  addEventListener("resize", () => {
    if (innerWidth === lastW) return;
    lastW = innerWidth; measureVH();
    clearTimeout(rt); rt = setTimeout(build, 120);
  });
  addEventListener("load", start);
  const relay = () => { clearTimeout(rt); rt = setTimeout(build, 120); };
  document.querySelectorAll("img").forEach((im) => { if (!im.complete) im.addEventListener("load", relay, { once: true }); });
  if ("ResizeObserver" in window) {
    const watch = new ResizeObserver(() => { if (main) relay(); });
    ["[data-screen]", "[data-phones]", ".rows", '[data-ill="native"]', ".private-in", "[data-end]", ".builds"].forEach((s) => { const n = document.querySelector(s); if (n) watch.observe(n); });
  }
  if (document.fonts) document.fonts.ready.then(() => { if (main) build(); });
  // Anything that changes the page's height (the release's version arriving, the fonts) moves
  // the boxes the line is laid out from: lay it out again.
  // Rebuilt only when the page really changes height, and at most every half second.
  if (page) {
    let last = 0, at = 0;
    new ResizeObserver(() => {
      const h = page.scrollHeight;
      if (!main || Math.abs(h - last) < 4) return;
      last = h;
      clearTimeout(rt);
      rt = setTimeout(build, Math.max(120, 500 - (performance.now() - at)));
      at = performance.now();
    }).observe(page);
  }

  /* ---------- 3. The bar ---------- */
  const bar = document.querySelector(".bar");
  const stuck = () => bar && bar.classList.toggle("is-stuck", scrollY > 8);
  addEventListener("scroll", stuck, { passive: true });
  stuck();
})();
