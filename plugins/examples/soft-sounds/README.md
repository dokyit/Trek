Ask an agent: "Play a soft chime when an agent finishes and a gentle knock when one needs me, and let me pick other sounds."

# Soft sounds

Sounds of its own for the two moments that matter: an agent finishing a turn (a two-note chime)
and an agent needing you (two gentle knocks). Choose others, or nothing, in its Sounds panel
beside the thread (palette: "Soft sounds: choose sounds"), where each plays as a preview. ⌘⌥S
(Ctrl+Alt+S on Windows) mutes and unmutes it.

What it shows: `main.js` hearing `turn.end` and `thread.needs-you` and playing a file of its own
with `trek.sound("sounds/chime.wav")` (a `.wav` in the plugin's folder: both systems play one);
a `[[panel]]` of its own HTML that keeps the choices in `trek.storage`; `[[command]]`s, one with
a key and one opening the panel. It asks for no permissions.

Make it yours: put your own `.wav` files in `sounds/` and add them to the list in
`panels/sounds/app.js`.
