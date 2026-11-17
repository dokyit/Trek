// Pip, a marmot at the foot of the sidebar. It hears agents start and end their turns and come to
// need you, keeps which threads are at it by their ids (it needs no permission to know that much),
// and sets the mood its picture and line are drawn from.
function moods(s) {
  const working = s.working || [], waiting = s.waitingIds || [];
  if (waiting.length) {
    return { mood: "whistling", waiting: waiting.length, line: waiting.length === 1 ? "Whistling: an agent needs you" : `Whistling: ${waiting.length} agents need you` };
  }
  if (working.length) {
    return { mood: "working", waiting: 0, line: working.length === 1 ? "Keeping watch over 1 agent" : `Keeping watch over ${working.length} agents` };
  }
  return null;
}

function update(change) {
  const s = trek.state.get();
  const working = new Set(s.working || []), waiting = new Set(s.waitingIds || []);
  change(working, waiting);
  const next = { working: [...working], waitingIds: [...waiting] };
  return { next, mood: moods(next) };
}

trek.on("app.start", () => {
  trek.state.set({ mood: "asleep", line: "Napping: nothing's running", working: [], waitingIds: [], done: 0 });
});

trek.on("turn.start", (e) => {
  const { next, mood } = update((w, n) => { w.add(e.thread.id); n.delete(e.thread.id); });
  trek.state.set({ ...next, ...mood });
});

trek.on("thread.needs-you", (e) => {
  const { next, mood } = update((w, n) => { w.delete(e.thread.id); n.add(e.thread.id); });
  trek.state.set({ ...next, ...mood });
});

trek.on("turn.end", (e) => {
  const { next, mood } = update((w, n) => { w.delete(e.thread.id); n.delete(e.thread.id); });
  const done = (trek.state.get().done || 0) + 1;
  // Nothing else on: a cheer for the turn that finished, until something else happens.
  trek.state.set({ ...next, done, ...(mood || { mood: "cheering", waiting: 0, line: done === 1 ? "Cheering: a turn finished" : `Cheering: ${done} turns finished` }) });
});

// A pat: Pip settles down for a nap if nothing needs it.
function pat() {
  const s = trek.state.get();
  if (s.mood === "whistling") {
    trek.toast("Pip won't settle: an agent needs you");
  } else if (s.mood === "working") {
    trek.toast("Pip leans into the pat and keeps watching");
  } else {
    trek.state.set({ mood: "asleep", line: "Napping: nothing's running", done: 0 });
  }
}
