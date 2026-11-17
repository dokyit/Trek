Ask an agent: "Add a focus timer to my composer where the hiker walks round it while I work, and offers me a break when time's up."

# Focus walk

A chip on the composer's top edge. Click its time to pick 15, 25, 45 or 60 minutes, then ▶ (or
⌘⌥F, Ctrl+Alt+F on Windows). The hiker sets off round the composer's edge, leaving a trail in
your theme's accent; you can pause, carry on or stop from the chip or the palette. Back at the
chip, Trek says so (a toast and the alert sound) and offers a five-minute break, walked in
green, or another walk. Quit Trek mid-walk and it picks up where time has got to.

What it shows: a `[[slot]]` on `composer.border` with Trek's own `trek:focus-timer`, and
`[[command]]`s with a key (`trek:focus-timer.toggle`, `.break`, `.stop`).

Make it yours: change `minutes` and `break` in `plugin.toml`, or the key.
