# The look of trekdesktop.com

One idea, carried through: **a day on the trail.** The page starts in the morning, in Trek's Paper,
and ends at camp at dusk, in Night. Those are the two themes the app really has. A dotted trail runs
down the page past cairn waypoints, the pixel hiker from the app walks it as you scroll, and the
download waits at camp.

Everything is built from things Trek already owns. Nothing here is stock, and nothing imitates
another product's site.

## What it is built from

| On the site | Where it comes from in the repository |
|---|---|
| The ridges behind the hero, the golden hour and camp | `assets/backgrounds/make_backgrounds.py`: the same ridge function, seeds, bases, amplitudes and colours as the app's Dawn and Paper backdrops. Drawn as smooth vector silhouettes (`assets/img/ridges-*.svg`), not as the app's dithered pixels. The script is the project's own, so the art is the owner's to use. |
| Morning and dusk colours | `crates/trek-assets/assets/themes/trek.json` (Trek Paper, Trek Night) and the backdrop's sky stops: cream `#F3EEE6`, peach `#F6E2CF` and `#F2B98E`; dusk `#17131F`, with the dawn sky's `#1B2340`, `#7A3B3A` and ember horizon. |
| Ember, and the state colours | `crates/trek-palette/src/lib.rs`: ember for an agent at work, amber for one that needs you, emerald for finished, red for failed. On the site ember is the one accent, kept for the download and for those states. Links and the button use the palette's deeper Paper ember (`#B53C0B`) so white text on it passes 4.5:1. |
| The cairn | `assets/brand/trek-mark.svg`, and the small four-stone mark of `crates/trek-brand` (`small_mark`) for the menu bar and waypoints. The app icon (`assets/brand/trek-icon-ember.svg`) is shown as an object beside the install heading. |
| The hiker | `crates/trek-mascot/src/lib.rs`: its five poses and eight colours, as an SVG sprite (`assets/img/hiker-*.svg`). It walks at four poses a stride, as it does above the composer. |
| The dotted trail | The mascot's own trail: 2-point dots, 7 to 8 points apart. Used for section rules, for the switchback line down the page (wide screens) and for the explainer. |
| The app in the hero | A real frame of the app in its glass look (`control glass on`), taken twice by the verify driver, over black and over white, so its true alpha could be recovered. The sidebar and title bar are see-through in the picture exactly where they are in the app, and the page's own hills show through them. No UI is redrawn in HTML. |
| Type | Inter and JetBrains Mono, the app's faces (`crates/trek-assets/assets/fonts`), subset to Latin and served from this domain. Both are SIL OFL 1.1 with no reserved font name; the licence texts are beside the files. |

## The rules it follows

- **Paper first.** Light is the default; dark mode puts the whole page at dusk. In light mode the
  page travels: Paper from the hero to the permission rules, a golden hour behind the feature cards,
  a sunset over the ridges, then dusk for install, questions and camp.
- **One accent.** Ember marks the action and the states. Nothing else is coloured.
- **Two-tone headlines.** The first clause at full strength, the second muted. Display type is Inter
  Medium, tight (-0.035 to -0.042em), never above 5.5rem.
- **Waypoints.** Each section opens with a small pill holding the cairn and the section's name, on
  a dotted rule. On wide screens the rule runs back to the trail in the margin.
- **Real pictures, honestly labelled.** Every app picture is a real frame from the app. Each one
  that shows staged content carries an "Example" tag and says what was staged.
- **No "Mock" in any picture.** The test app's scripted agent names itself in the composer and under
  it. Every picture is cut above that, or taken from a view without it.
- **One retouch.** In the window pictures the three title-bar buttons are painted in macOS's own
  colours, where and as large as they were captured: the test window is never the key window, so
  macOS draws them grey. Nothing else in any picture is retouched.
- **Motion is the trail.** The only moving things are the hiker on the line down the page and the
  three hikers of "How it works", both driven by scrolling, never by a timer, so there is nothing to
  pause. With Reduce Motion on, or with JavaScript off, they stand still and the page reads the same.
- **Fast and plain.** No framework, no build, no web font or script from elsewhere. One request
  leaves the page: the GitHub API call that finds the latest release.

## Influences: what was taken, and what was not

The owner supplied these. Each gave a quality, not a layout.

| Influence | Taken | Left |
|---|---|---|
| A designer's concept (cream and maroon fintech page) | A warm ground instead of white or black; two-tone headlines; one large statement with its second half dimmed; a stat row with dividers; cards on a rich backdrop; thin line-work | Its maroon, its circuit-board lines, its centred everything, its pricing and logo rows |
| macapp.supply (a gallery of Mac apps) | The app icon shown as an object; a product shot on a rich ground | Its banner-card grid |
| A hero exploration (landscape, glass panel rising from the bottom) | A full-bleed landscape hero; the product as a translucent panel rising over the hills; hairlines through the composition; a small pill above the headline that states the loop; two pill buttons, one solid and one translucent | Its photographed red hills and sky (Trek's hills are its own ridges), its light-weight white type, its plain menu bar |
| A component library's reel | Folder tabs on the three feature cards, where the tab says where the thing lives in the app | The arc and spotlight carousels and the 3D rings |
| Realm (a competitor) | The real app as the hero at large size; short, confident copy | Its near-black page, its bordered frame with a strip of tabs under the window, its white sans on black |
| GOAT (a competitor) | An explainer that makes the idea clear without a video; a menu bar that keeps Download in reach; licence and platforms in one line under the button, other builds as quiet links; one install tile per platform; honest "Example" captions | Its dot-matrix backdrop, its grey-white look, its hub-and-spokes diagram, its section order |
| Aria (a competitor; described to me, since a headless capture only catches its loader) | A world behind the page that changes from top to bottom; orderly copy: steps, grouped features, plain privacy, FAQ, a last download | Its dithered pixel sky and sun, its giant wordmark hero, its auto-advancing segmented showcase |

## How Trek's site differs from the others

Captured on 11 November 2026 with headless Chrome at 1440 px (Aria as described above).

| Site | What it looks like | How Trek's differs |
|---|---|---|
| T3 Code (t3.codes) | Black page, centred white headline, vendor logos floating around it, a dark app shot, a wall of posts | Paper and dusk, no vendor logos, no quotes; the app is glass over Trek's own hills |
| Synara (trysynara.com) | Black, an ASCII-art temple behind a centred headline, a dark app shot | A coloured landscape in smooth vector, not character art; light by default |
| Orca (onorca.dev) | Black, centred headline, a tab strip over a dark app mock-up, a logo wall | No mock-up, no tab strip, no logo wall; one real frame |
| Paseo (paseo.sh) | Black, centred headline, tilted phone renders | Flat real screenshots; the phone is shown in its own section, with what it takes to get it |
| Paradev (paradev.com) | Black with a dotted noise field, pixel wordmark, a dark workspace mock-up | No noise field or pixel lettering; warm ground |
| Conductor (conductor.build) | Black with a dithered starfield, a left-aligned headline, a dark app shot, a logo wall | No dithering, no stars; ridges and a horizon instead |
| Waku (waku.sh) | Black column, left-aligned headline, one screenshot on a blue sky panel, a spec grid | The landscape is the whole hero and runs the length of the page; the window is see-through, not pasted on a panel |
| Emdash (emdash.com) | Black, text only, an email box | A download that works, and pictures |
| Cursor (cursor.com) | Warm black, small left headline, app windows over a painted landscape in a frame | Trek's land is generated ridges, unframed and full-bleed, light first, and it changes down the page |
| Zed (zed.dev) | Navy with a hairline grid, an italic serif headline, blue buttons | Sans throughout, ember instead of blue, a landscape instead of a grid |
| Warp (warp.dev) | White with a square grid, monospace headlines | Warm paper, proportional type; monospace only for commands |
| GOAT (trygoat.ai) | Grey-white, dot-matrix mountains, heavy black headline, a hub diagram | Colour, smooth ridges, a medium-weight two-tone headline, a trail instead of a hub |
| Realm (realm.computer) | Near-black, white sans, the app in a bordered frame with a strip that switches views | Paper first; the app rises out of the scene with no frame or switcher |
| Aria (joinariapp.com) | A dithered pixel sky with a sun, a huge wordmark, the scene shifting from day to dusk | Vector ridges with no pixel sky; the headline says what Trek does; the real app is in the first screen and a half |

What only Trek has, and so what the page leans on: Paper as a default, the cairn, the switchback
ridges, the hiker, and the trail words.

## What is a first version

- The hiker on the line down the page shows on screens 1320 px and wider; narrower screens keep
  the waypoints and dotted rules without it.
- "How it works" is one stretch of trail with four captions. Scrolling moves its hikers; it has no
  pager and no separate frames per step.
- The hero shows one view of the app. There is no switcher.
- The sky's change down the page is three settings (morning, golden hour, dusk), not a continuous
  blend tied to the scroll position.
