// Now playing: whichever thread's agent is at work, as a music player shows its track. Each time
// a turn starts or ends, or a thread comes to need you, it looks over the threads (it may read
// their titles, states and agents: `threads:read`) and keeps the ones at work, newest first.
function refresh(keepId) {
  const s = trek.state.get();
  const since = s.since || {};
  const now = Date.now();
  const list = trek.threads()
    .filter((t) => t.state === "working")
    .map((t) => ({ id: t.id, title: t.title || "Untitled", agent: t.agent, since: since[t.id] || now }));
  list.sort((a, b) => b.since - a.since);
  const kept = {};
  for (const t of list) kept[t.id] = t.since;
  let index = list.findIndex((t) => t.id === (keepId || (s.playing && s.playing.id)));
  if (index < 0) index = 0;
  trek.state.set({
    since: kept,
    list,
    index,
    playing: list[index] || null,
    more: Math.max(0, list.length - 1),
  });
}

trek.on("app.start", () => refresh());
trek.on("turn.start", (e) => refresh(e.thread.id));
trek.on("turn.end", () => refresh());
trek.on("thread.needs-you", () => refresh());

// Skip to the next agent at work, as a player's next track.
function next() {
  const s = trek.state.get();
  const list = s.list || [];
  if (list.length < 2) return;
  const index = ((s.index || 0) + 1) % list.length;
  trek.state.set({ index, playing: list[index] });
}
