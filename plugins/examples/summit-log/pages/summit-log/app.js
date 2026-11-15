// The Summit log: threads finished per day, from Trek (`trek.data("finished")`, which the
// plugin's threads:read permission allows). The range chosen is kept in the plugin's own storage.
const $ = (id) => document.getElementById(id);
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MAX_STONES = 9;

let weeks = 12;
let finished = null;

const plural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;

function dateOf(start, i) {
  const [y, m, d] = start.split("-").map(Number);
  return new Date(y, m - 1, d + i);
}

function draw() {
  if (!finished) return;
  const all = finished.days;
  const days = all.slice(all.length - weeks * 7);
  const offset = all.length - days.length;
  const sum = (a) => a.reduce((x, y) => x + y, 0);

  $("today").textContent = finished.today;
  $("week").textContent = sum(all.slice(-7));
  $("total").textContent = sum(days);
  $("total-label").textContent = weeks === 4 ? "in four weeks" : "in twelve weeks";
  $("now").textContent = `${finished.working} working · ${finished.waiting} need you`;

  for (const b of document.querySelectorAll(".range button")) {
    b.setAttribute("aria-selected", String(Number(b.dataset.weeks) === weeks));
  }

  const cairns = $("cairns");
  const months = $("months");
  cairns.replaceChildren();
  months.replaceChildren();
  let best = -1;
  let lastMonth = -1;
  days.forEach((n, i) => {
    const date = dateOf(finished.start, offset + i);
    const day = document.createElement("div");
    day.className = "day" + (n === 0 ? " none" : "") + (i === days.length - 1 ? " today" : "")
      + (date.getDay() === 1 && i > 0 ? " monday" : "");
    day.dataset.tip = `${DAYS[date.getDay()]} ${MONTHS[date.getMonth()]} ${date.getDate()} · ${plural(n, "summit")}`;
    for (let s = 0; s < Math.min(n, MAX_STONES); s++) {
      const stone = document.createElement("div");
      stone.className = "stone";
      day.append(stone);
    }
    if (n > MAX_STONES) {
      const more = document.createElement("div");
      more.className = "more";
      more.textContent = `+${n - MAX_STONES}`;
      day.append(more);
    }
    cairns.append(day);
    if (best < 0 || n > days[best]) best = i;
    if (date.getMonth() !== lastMonth) {
      if (i > 0 && i < days.length - 3) {
        const label = document.createElement("span");
        label.textContent = MONTHS[date.getMonth()];
        label.style.left = `${(i / days.length) * 100}%`;
        months.append(label);
      }
      lastMonth = date.getMonth();
    }
  });

  const none = sum(days) === 0;
  $("empty").hidden = !none;
  if (none) {
    $("best").textContent = "";
  } else {
    const date = dateOf(finished.start, offset + best);
    $("best").textContent =
      `Highest cairn: ${DAYS[date.getDay()]} ${MONTHS[date.getMonth()]} ${date.getDate()} · ${plural(days[best], "summit")}`;
  }
  // The streak: days in a row, up to today, with a summit (today may still be empty).
  let streak = 0;
  for (let i = all.length - 1; i >= 0; i--) {
    if (all[i] > 0) streak++;
    else if (i < all.length - 1) break;
  }
  $("foot").textContent = streak > 1 ? `${streak} days in a row with a summit.` : "";
}

async function load() {
  try {
    finished = await trek.data("finished");
    draw();
  } catch (e) {
    $("empty").hidden = false;
    $("empty").textContent = `Trek wouldn't say: ${e.message}`;
  }
}

for (const b of document.querySelectorAll(".range button")) {
  b.addEventListener("click", () => {
    weeks = Number(b.dataset.weeks);
    draw();
    trek.storage.set({ weeks }).catch(() => {});
  });
}

trek.storage.get().then((kept) => {
  if (kept && (kept.weeks === 4 || kept.weeks === 12)) weeks = kept.weeks;
  draw();
}).catch(() => {});
load();
// What's finished changes while the page is open: look again when it's looked at, and now and then.
window.addEventListener("focus", load);
setInterval(() => { if (!document.hidden) load(); }, 60000);
