Ask an agent: "Put a little marmot at the bottom of my sidebar that naps when nothing's running, keeps watch while agents work, and whistles when one needs me."

# Pip the marmot

A companion at the foot of the sidebar, above Settings. Pip naps while nothing's running, sits
up and keeps watch while agents work (it counts them), stands and whistles when one needs you
(with how many), and cheers when a turn finishes. Click Pip, or ⌘⌥P (Ctrl+Alt+P on Windows), to
pat it: a cheering marmot settles back down for a nap.

What it shows: a `[[slot]]` on `sidebar.foot` that switches between four pictures of its own
(`image`, `when`); `main.js` hearing `turn.start`, `turn.end`, `thread.needs-you` and `app.start`
and setting the state the slot reads; a `[[command]]` with a key that runs a function of the
logic. It asks for no permissions: a hook tells it a thread's id, never its title.

Make it yours: draw your own animal in the four SVGs in `art/`, and rename it in
`ui/buddy.json` and `plugin.toml`.
