Ask an agent: "Make my new-thread screen greet me with a landscape for the time of day, and give me a row of quick prompts I can edit myself."

# Trailhead

Your own new-thread screen. Above Trek's composer, a small landscape and a greeting that follow
the day: a sunrise in the morning, green hills in the afternoon, a sunset in the evening, stars
and a tent at night. Under it, your quick prompts: a click puts one in the composer. Edit them on
the Quick prompts page (in the sidebar's Pages group, or "Edit" beside the chips, or the
composer's + menu); they're kept between launches.

Turn it on, then choose it in Settings › Plugins › In place of Trek's own › New-thread screen.

What it shows: a `[[replace]]` of `new-thread` that embeds Trek's own composer (`trek:composer`)
and reads the time of day (`{now.part}`, `{now.weekday}`, `{now.date}`); a `[[page]]` that keeps
the prompts in `trek.storage` and hands them to the screen with `trek.state.set`; logic that
hears `app.start` and puts the kept prompts back after a restart; and a `composer.plus` slot.

Make it yours: swap the four pictures in `art/`, or change the greetings in `ui/screen.json`.
