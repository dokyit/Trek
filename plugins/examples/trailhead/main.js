// Trailhead's logic: when Trek starts, the quick prompts the user kept (their page saves them in
// the plugin's storage) become the state the new-thread screen draws its chips from.
const STARTERS = [
  "Explain this project to me",
  "What changed since yesterday?",
  "Find a bug worth fixing",
  "Tidy up the README",
];

trek.on("app.start", () => {
  const kept = trek.storage.get().prompts;
  trek.state.set({ prompts: Array.isArray(kept) ? kept : STARTERS });
});
