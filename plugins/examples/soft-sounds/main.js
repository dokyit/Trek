// Soft sounds: a sound of the plugin's own when an agent finishes a turn or comes to need you.
// Which sound for which, and whether it's muted, are kept in its storage; the Sounds panel
// changes them.
const DEFAULTS = { finish: "chime", needs: "knock", muted: false };
const settings = () => ({ ...DEFAULTS, ...trek.storage.get() });

function play(which) {
  const s = settings();
  if (!s.muted && s[which] && s[which] !== "none") trek.sound(`sounds/${s[which]}.wav`);
}

trek.on("turn.end", () => play("finish"));
trek.on("thread.needs-you", () => play("needs"));

// ⌘⌥S (Ctrl+Alt+S): quiet, or not.
function mute() {
  const s = settings();
  trek.storage.set({ ...s, muted: !s.muted });
  trek.toast(s.muted ? "Soft sounds are on" : "Soft sounds are muted");
}
