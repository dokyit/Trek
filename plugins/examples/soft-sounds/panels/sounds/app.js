// Which sound plays when, kept in the plugin's storage (its logic reads it when a turn ends or a
// thread needs you). Each sound plays here as a preview.
const SOUNDS = [
  ["chime", "Chime", "two soft notes"],
  ["knock", "Knock", "two gentle taps"],
  ["drop", "Drop", "a drop of water"],
  ["bell", "Bell", "a small singing bowl"],
  ["none", "Nothing", ""],
];
const DEFAULTS = { finish: "chime", needs: "knock", muted: false };
let settings = { ...DEFAULTS };
const on = document.getElementById("on");

function preview(name) {
  if (name === "none") return;
  const a = new Audio(`../../sounds/${name}.wav`);
  a.volume = 0.8;
  a.play().catch(() => {});
}

function draw() {
  on.checked = !settings.muted;
  document.body.classList.toggle("off", settings.muted);
  for (const section of document.querySelectorAll("section")) {
    const when = section.dataset.when;
    const box = section.querySelector(".choices");
    box.replaceChildren();
    for (const [id, name, about] of SOUNDS) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "choice" + (settings[when] === id ? " on" : "");
      const label = document.createElement("span");
      label.className = "name";
      label.textContent = name;
      const note = document.createElement("span");
      note.className = "about";
      note.textContent = about;
      row.append(label, note);
      if (id !== "none") {
        const play = document.createElement("span");
        play.className = "play";
        play.title = "Play it";
        play.textContent = "▶";
        play.onclick = (e) => { e.stopPropagation(); preview(id); };
        row.append(play);
      }
      row.onclick = () => { settings[when] = id; preview(id); save(); };
      box.append(row);
    }
  }
}

async function save() {
  draw();
  try { await trek.storage.set(settings); } catch (_) {}
}

on.onchange = () => { settings.muted = !on.checked; save(); };

(async () => {
  try { settings = { ...DEFAULTS, ...(await trek.storage.get()) }; } catch (_) {}
  draw();
})();
