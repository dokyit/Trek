/* A small working copy of Trek's window (see inbox.css). TrekInbox.mount(box, options) builds it in
   `box` and returns { set(step), step }. Steps: 0 at work, 1 one stops to ask and goes to the top,
   2 answered and back at work, 3 finished. The page decides when a step happens (scroll, a timer);
   this file only draws them. The threads are staged, as the page says.
   The window keeps one shape at every size. In a box under 560 px wide (a phone) it is laid out
   closer, at 540 px instead of 760, so its words are a third larger there: the same sidebar, thread
   and card, less the parts marked tk-x (a word, a chip, a hint), which inbox.css leaves out. */
(() => {
  "use strict";
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const I = {
    search: '<svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>',
    camp: '<svg viewBox="0 0 16 16"><path d="M8 2.5 2 13.5h12z"/><path d="M8 7.5 5.8 13.5M8 7.5l2.2 6"/></svg>',
    notes: '<svg viewBox="0 0 16 16"><path d="M3.5 2.5h6l3 3v8h-9z"/><path d="M6 8h4M6 10.5h3"/></svg>',
    study: '<svg viewBox="0 0 16 16"><path d="M1.5 6 8 3l6.5 3L8 9z"/><path d="M4.5 7.5v3.2C6.5 12.5 9.5 12.5 11.5 10.7V7.5"/></svg>',
    inbox: '<svg viewBox="0 0 16 16"><path d="M2 9.5 4 3.5h8l2 6v3.5H2z"/><path d="M2 9.5h3.5l1 1.5h3l1-1.5H14"/></svg>',
    shield: '<svg viewBox="0 0 16 16"><path d="M8 1.8 13 3.8v3.6c0 3.2-2.2 5.4-5 6.8-2.8-1.4-5-3.6-5-6.8V3.8z"/><path d="m5.8 8 1.6 1.6 3-3.2"/></svg>',
    check: '<svg viewBox="0 0 16 16"><path d="m3.5 8.5 3 3 6-7"/></svg>',
    read: '<svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>',
    branch: '<svg viewBox="0 0 16 16"><circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="5.5" r="1.5"/><path d="M4.5 5v6M11.5 7c0 3-7 2-7 4"/></svg>',
    chip: '<svg viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1.5"/><path d="M6 1.5V4M10 1.5V4M6 12v2.5M10 12v2.5M1.5 6H4M1.5 10H4M12 6h2.5M12 10h2.5"/></svg>',
  };
  const ROWS = {
    mig: { t: "Migrate the schema", p: "mobile", base: 48 },
    auth: { t: "Require session auth", p: "atlas-web", base: 124 },
    test: { t: 'Run the <span class="tk-x">full </span>test suite', p: "mobile", base: 37 },
    sync: { t: "Review the sync code", p: "mobile", base: 190 },
  };
  // Each step: the order of the rows, and each row's state ("working", "waiting", "done").
  const STEPS = [
    { order: ["auth", "mig", "test", "sync"], s: { mig: "working", auth: "working", test: "working", sync: "working" } },
    { order: ["mig", "auth", "test", "sync"], s: { mig: "waiting", auth: "working", test: "working", sync: "working" } },
    { order: ["mig", "auth", "test", "sync"], s: { mig: "working", auth: "working", test: "working", sync: "working" } },
    { order: ["mig", "sync", "auth", "test"], s: { mig: "done", auth: "working", test: "working", sync: "done" } },
  ];
  const fmt = (s) => (s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`);

  function html(narrow) {
    const rows = Object.entries(ROWS).map(([id, r]) =>
      `<li class="tk-row" data-id="${id}" data-state="working"><span class="tk-dot"><i></i></span><span class="t">${r.t}</span><span class="w">now</span><span class="s"><b></b><span class="p"> · ${r.p}</span></span></li>`).join("");
    return `<div class="tk-scale"><div class="tk-win">
  <div class="tk-title"><span class="tk-lights"><i></i><i></i><i></i></span><b>Trek</b>
    <span class="tk-crumb"><span class="tk-tag mo">MO</span>mobile <span>/</span> <strong>Migrate the schema</strong></span></div>
  <div class="tk-body">
    <div class="tk-side">
      <div class="tk-nav search">${I.search}<span>Search</span></div>
      <div class="tk-nav">${I.camp}<span>Basecamp</span><span class="n">4</span></div>
      <div class="tk-nav">${I.notes}<span>Notes</span></div>
      <div class="tk-nav">${I.study}<span>Study</span></div>
      <div class="tk-inbox"><div class="tk-inbox-h">${I.inbox}Inbox <em>4</em><small class="tk-x">Mark all read</small></div><ol class="tk-rows">${rows}</ol></div>
      <div class="tk-proj"><p>Projects 2</p><div><span class="tk-tag mo">MO</span>mobile<span class="n">3</span></div><div><span class="tk-tag">AW</span>atlas-web<span class="n">1</span></div></div>
    </div>
    <div class="tk-pane">
      <div class="tk-tabs"><span class="tk-tab">${I.chip}Tour the startup code</span><span class="tk-tab on">${I.chip}Migrate the schema <span class="tk-dot" data-tabdot hidden><i></i></span></span></div>
      <div class="tk-log">
        <div class="tk-you">Add a streak count to each user</div>
        <div class="tk-act">${I.read}Read 3 files, edited 2</div>
        <p class="tk-msg">Migration <code>0042</code> adds the column and backfills it from the activity log.</p>
        <div class="tk-card" data-card>
          <h4>${I.shield}Claude Code wants to run a command</h4>
          <pre>./migrate</pre>
          <div class="btns"><span class="tk-x">Applies 0042 and nothing else.</span><span class="tk-btn">Deny</span><span class="tk-btn go" data-allow>Allow</span></div>
        </div>
        <div class="tk-allowed" data-allowed hidden>${I.check}Allowed <code>./migrate</code></div>
        <p class="tk-msg tk-after hide" data-after></p>
      </div>
      <div class="tk-comp" data-comp><p>Message Claude Code</p><div><span class="tk-chip">${I.branch}main</span><span class="tk-chip">Sonnet 5.5 · High</span><span class="tk-chip tk-x">Auto-accept edits</span><span class="tk-stop"><i></i><svg viewBox="0 0 16 16"><path d="M8 13V3M4 7l4-4 4 4"/></svg></span></div></div>
    </div>
  </div>
</div></div>`;
  }

  function mount(box, opts = {}) {
    if (!box) return null;
    const narrow = Boolean(opts.narrow);
    let compact = false;
    box.classList.add("tk-box");
    box.classList.toggle("tk-narrow", narrow);
    box.innerHTML = html(narrow);
    const scaler = box.querySelector(".tk-scale");
    const list = box.querySelector(".tk-rows");
    const rows = Object.fromEntries([...list.children].map((li) => [li.dataset.id, li]));
    const card = box.querySelector("[data-card]");
    const allowed = box.querySelector("[data-allowed]");
    const after = box.querySelector("[data-after]");
    const comp = box.querySelector("[data-comp]");
    const allow = box.querySelector("[data-allow]");
    const tabdot = box.querySelector("[data-tabdot]");
    // The layout's own width, and the scale that fits it to the box. The box's shape does not
    // depend on which layout it holds, so choosing by its width cannot go back and forth.
    const scale = () => box.clientWidth / (compact ? 540 : narrow ? 300 : 760);
    const fit = () => {
      const w = box.clientWidth;
      const small = !narrow && w > 0 && w < 560;
      if (small !== compact) { compact = small; box.classList.toggle("tk-compact", compact); }
      scaler.style.setProperty("--tk-k", String(scale()));
    };
    new ResizeObserver(fit).observe(box);
    fit();

    const secs = Object.fromEntries(Object.entries(ROWS).map(([id, r]) => [id, r.base]));
    let current = -1;
    let visible = false;

    function label(id, state) {
      const b = rows[id].querySelector(".s b");
      if (state === "waiting") b.textContent = "Waiting: run ./migrate?";
      else if (state === "done") b.textContent = "Finished";
      else b.textContent = `Working ${fmt(secs[id])}`;
    }

    function set(step, instant = false) {
      step = Math.max(0, Math.min(STEPS.length - 1, step));
      if (step === current) return;
      const was = current;
      current = step;
      const st = STEPS[step];
      const quick = instant || still.matches || was < 0;
      // Rows: state, then the order, moved as one (FLIP).
      const before = Object.fromEntries(Object.entries(rows).map(([id, li]) => [id, li.getBoundingClientRect().top]));
      for (const [id, li] of Object.entries(rows)) {
        const prev = li.dataset.state;
        li.dataset.state = st.s[id];
        label(id, st.s[id]);
        li.querySelector(".w").textContent = st.s[id] === "working" ? "" : "now";
        if (!quick && prev !== "waiting" && st.s[id] === "waiting") {
          li.classList.add("flash");
          requestAnimationFrame(() => requestAnimationFrame(() => li.classList.remove("flash")));
        }
      }
      for (const id of st.order) list.appendChild(rows[id]);
      if (!quick) {
        for (const [id, li] of Object.entries(rows)) {
          const dy = before[id] - li.getBoundingClientRect().top;
          if (!dy) continue;
          li.animate([{ transform: `translateY(${dy / (scale() || 1)}px)` }, { transform: "none" }],
            { duration: id === "mig" ? 700 : 550, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
        }
      }
      // The thread on screen.
      tabdot.hidden = step !== 1;
      const answered = step >= 2;
      card.hidden = step !== 1;
      card.classList.toggle("gone", false);
      allowed.hidden = !answered;
      after.textContent = step === 3 ? "Applied 0042. Every user has a streak, and the tests pass." : step === 2 ? "Applying 0042…" : step === 0 ? "Writing the backfill…" : "";
      after.classList.toggle("hide", step === 1);
      comp.classList.toggle("idle", step === 3);
      comp.querySelector("p").textContent = step === 1 ? "Reply, or answer above" : "Message Claude Code";
      if (!quick && was === 1 && step === 2) {
        // Allow is pressed, then the card gives way.
        card.hidden = false;
        allowed.hidden = true;
        allow.classList.add("press");
        setTimeout(() => { allow.classList.remove("press"); card.classList.add("gone"); }, 220);
        after.classList.add("hide");
        setTimeout(() => { card.hidden = true; card.classList.remove("gone"); allowed.hidden = false; after.classList.remove("hide"); }, 560);
      }
    }

    // The clocks on working rows run only while the window is on screen and motion is welcome.
    new IntersectionObserver((e) => { visible = e[0].isIntersecting; }, { threshold: 0.1 }).observe(box);
    setInterval(() => {
      if (!visible || still.matches || document.hidden) return;
      for (const id of Object.keys(rows)) {
        if (rows[id].dataset.state !== "working") continue;
        secs[id] += 1;
        label(id, "working");
      }
    }, 1000);

    set(opts.step || 0, true);
    return { set, get step() { return current; }, steps: STEPS.length };
  }

  window.TrekInbox = { mount };
})();
