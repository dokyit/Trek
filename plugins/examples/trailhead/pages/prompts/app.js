// The quick prompts, kept in the plugin's storage (so they last) and handed to its state (what the
// new-thread screen draws its chips from) on every change.
const MAX = 8;
const list = document.getElementById("list");
const form = document.getElementById("add");
const input = document.getElementById("text");
const note = document.getElementById("note");
let prompts = [];

function draw() {
  list.replaceChildren();
  if (!prompts.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No quick prompts: add one below.";
    list.append(li);
  }
  prompts.forEach((p, i) => {
    const li = document.createElement("li");
    const text = document.createElement("span");
    text.textContent = p;
    const up = button("↑", "Move up", i === 0, () => move(i, -1));
    const remove = button("Remove", "Remove it", false, () => {
      prompts.splice(i, 1);
      save();
    });
    li.append(text, up, remove);
    list.append(li);
  });
  form.querySelector("button").disabled = prompts.length >= MAX;
  input.placeholder = prompts.length >= MAX ? `${MAX} at most: remove one first` : "Something you often ask, like “Review my last commit”";
}

function button(label, title, disabled, onclick) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = label;
  b.title = title;
  b.disabled = disabled;
  b.onclick = onclick;
  return b;
}

function move(i, by) {
  const [p] = prompts.splice(i, 1);
  prompts.splice(i + by, 0, p);
  save();
}

async function save() {
  draw();
  try {
    const kept = await trek.storage.get();
    await trek.storage.set({ ...kept, prompts });
    await trek.state.set({ prompts });
    note.textContent = "Saved: your new-thread screen shows them now.";
  } catch (e) {
    note.textContent = "Couldn't save: " + e.message;
  }
}

form.addEventListener("submit", (e) => {
  e.preventDefault();
  const p = input.value.trim();
  if (!p || prompts.length >= MAX) return;
  if (!prompts.includes(p)) prompts.push(p);
  input.value = "";
  save();
});

(async () => {
  try {
    const state = await trek.state.get();
    const kept = await trek.storage.get();
    prompts = Array.isArray(kept.prompts) ? kept.prompts : Array.isArray(state.prompts) ? state.prompts : [];
  } catch (_) {}
  draw();
})();
