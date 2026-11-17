Ask an agent: "Show what my agents are working on in the title bar, like a music player's now playing, and open the thread when I click it."

# Now playing

A small pill at the title bar's right end while an agent is at work, wherever you are in Trek:
the thread's title, its agent and how long the turn has run. Click it to open the thread. With
more than one at work, "+2 ▶" skips to the next, as does ⌘⌥N (Ctrl+Alt+N on Windows). When
nothing's running it isn't there.

What it shows: a `[[slot]]` on `status` drawn from the plugin's state; `main.js` hearing
`turn.start`, `turn.end`, `thread.needs-you` and `app.start` and reading the threads with
`trek.threads()`; an `open:` action and a `[[command]]` with a key. It asks for two permissions,
`threads:read` and `threads:open`, which you allow in Settings › Plugins when you turn it on.

Make it yours: change the icon, or show the project instead of the agent
(`{state.playing.project}` after adding it in `main.js`).
