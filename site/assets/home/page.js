/* trekdesktop.com's reading pages: the bar gains its ground once the page moves, and the line down
   the column is drawn to the ember dot, as far as you have read (whole at the end of the page).
   Without script, or with Reduce Motion, the line is whole and the dot at its end. */
(() => {
  "use strict";
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const clamp = (v) => Math.min(1, Math.max(0, v));
  const bar = document.querySelector(".bar");
  const read = document.querySelector("[data-read]");
  let raf = 0;
  function paint() {
    raf = 0;
    if (bar) bar.classList.toggle("is-stuck", scrollY > 8);
    if (!read) return;
    if (still.matches) { read.style.setProperty("--k", "1"); return; }
    const r = read.getBoundingClientRect();
    const top = r.top + scrollY, h = Math.max(1, r.height - 96);
    const max = Math.max(1, document.documentElement.scrollHeight - innerHeight);
    const at = (y) => (y + innerHeight * 0.6 - top) / h;
    // Scaled so the bottom of the page is the end of the line, however short the page.
    const end = Math.max(at(max), 1e-6);
    const k = end < 1 ? at(scrollY) / end : at(scrollY);
    read.style.setProperty("--k", clamp(k).toFixed(4));
  }
  const go = () => { if (!raf) raf = requestAnimationFrame(paint); };
  addEventListener("scroll", go, { passive: true });
  addEventListener("resize", go);
  still.addEventListener("change", go);
  paint();
})();
