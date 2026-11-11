# trekdesktop.com

Trek's website: one page, a privacy page and a not-found page. Plain HTML, CSS and a little
JavaScript. There is no framework and no build step; the folder is served as it is.

```
site/
  index.html            the page: the hero with the app's window, then the Inbox, iOS, iPhone
                        and download sections
  privacy/index.html    docs/PRIVACY.md in plain words, and what this site itself does
  404.html
  assets/site.css       every style (no inline styles: the CSP forbids them)
  assets/site.js        the download button, the hiker's walk, and the Inbox example's play
  assets/fonts/         Bricolage Grotesque, Hanken Grotesk and JetBrains Mono, with their OFL licences
  assets/img/           captures of the app, the ridges, the standing hiker, the cairn, icons,
                        the share image
  _headers  _redirects  Cloudflare Pages: security headers, caching, short addresses
  robots.txt  sitemap.xml  site.webmanifest  favicon.ico  favicon.svg  apple-touch-icon.png
  DESIGN.md             the design record: colours, type, layout, components, the rules, and
                        where each thing comes from
  PRODUCT.md            who the site is for and what it has to say
  .impeccable/          the design tool's notes: the direction for the home page (surfaces/),
                        the design record as data (design.json), review screenshots (review/)
```

## Preview it

```sh
cd site
python3 -m http.server 8731 --bind 127.0.0.1
# then open http://127.0.0.1:8731/
```

Any static server will do. Addresses in the pages start at `/`, so serve the `site` folder as the
root; opening `index.html` from the disk won't find its styles. `_headers` and `_redirects` only
take effect on Cloudflare.

Three views are worth a look before a change goes out:

- **Paper and Night.** The page follows the system's light or dark setting and has no switch of
  its own. Change the system setting, or the browser's emulation of it, to see the other theme.
- **Without the script, and with Reduce Motion on.** The hiker stands at the start of his trail,
  the Inbox example shows the waiting moment, and every download link leads to the latest release.
  The page must say the same things in both.
- **Narrow, under 860 px.** The window pictures keep their real size and are cropped, not shrunk.

## How it is deployed

Cloudflare Pages serves the `site` folder of this repository's `main` branch.

- Framework preset: **None**. Build command: **none** (leave it empty).
- Build output directory: `site`.
- Production branch: `main`. Custom domain: `trekdesktop.com`.

Every push to `main` that changes `site/` is live a minute or so later. `www` to the bare domain,
and HTTP to HTTPS, are Cloudflare settings (a redirect rule and "Always Use HTTPS"), not files here.

Two things to leave off in Cloudflare, because the pages promise it and the CSP would block them
anyway: **Web Analytics' JavaScript snippet** (and its automatic injection), and anything else
that adds a script, such as Rocket Loader or email obfuscation. Server-side request analytics are
fine. If one is ever wanted, change `_headers` and the privacy page in the same commit.

## What changes with a release

Nothing has to. The download button asks GitHub's API for the latest release in the visitor's
browser and points at its files; the version, the file sizes and the release date on the page come
from that answer. The Windows installer's address never changes; and with JavaScript off, or if
the API doesn't answer, every link leads to the latest release page.

These places say a state in words, and should be read again when it changes:

- `index.html`, the download list: the first launch. Today the Mac build is signed but not
  notarized and the Windows build has no code signature. Change the words the day that changes,
  and not before.
- `index.html`, the iPhone section and the iPhone row of the download list: the iPhone app is
  built from source. Change it when it is in TestFlight or the App Store.
- `index.html`, the line under the hero's buttons, the page's description and its structured
  data: the systems Trek needs (macOS 13 or later, Windows 11, iOS 26).
- `assets/site.js`: the names of a release's files (`BUILDS`), and the line that says there is no
  Linux build.

## Type

| Face | Use | File |
|---|---|---|
| Bricolage Grotesque | Headlines at 600, the wordmark at 500 | `BricolageGrotesque.woff2`, a variable file, Latin |
| Hanken Grotesk | Text at 400 and 500 | `HankenGrotesk.woff2`, a variable file, Latin |
| JetBrains Mono | Commands and code | `JetBrainsMono-Regular.woff2`, a Latin subset of the app's own copy (`crates/trek-assets/assets/fonts`) |

All three are under the SIL Open Font License 1.1, and the first two have no reserved font name.
Each licence text is beside its file as `OFL-*.txt`. The two variable files are the Latin files of
the `@fontsource-variable/bricolage-grotesque` and `@fontsource-variable/hanken-grotesk` packages,
copied as they are. Fonts are served from this domain and cached for a year, so give a changed
font a new file name.

There are no italics: no italic file is shipped, and the stylesheet stops the browser from
slanting or thickening a face itself.

## Pictures

Every picture of the app is a real capture. The Mac pictures are taken from the isolated test app
of the verify-trek skill (`.agents/skills/verify-trek`) in its demo dress: demo projects,
believable names, staged threads, and the scripted agent dressed as Claude Code, so no picture
names a mock agent. Check that again whenever a picture is replaced. Staged content is said to be
staged in a line beside each picture.

| File in `assets/img/` | What it is | How it is shown |
|---|---|---|
| `window-paper.webp`, `window-night.webp` | The app's window, 1040 by 700 points, with its shadow: 2172 by 1492 px | At real size, 1086 by 746, in the hero |
| `simulator-paper.webp`, `simulator-night.webp` | The window at 1180 by 760 points with the Simulator tool open on Trek's own iPhone app: 2452 by 1612 px | At real size, 1226 by 806. Under 860 px, cropped to the Simulator panel |
| `iphone-paper.webp`, `iphone-night.webp` | Trek for iPhone in its demo mode in the iOS Simulator: 1170 by 2532 px, the phone's own 3x | 298 px wide inside a drawn phone body, running off the foot of its section |
| `ridges-paper-back.png`, `ridges-paper-front.png`, `ridges-dawn-back.png`, `ridges-dawn-front.png` | The hero's backdrop, 2880 by 1500 px, in two layers: the sky with the two far ridges, and the near ridge on transparency | At one CSS pixel to the picture's pixel, centred, `image-rendering: pixelated` |
| `hiker-stand-paper.png`, `hiker-stand-night.png` | The hiker's standing pose, 12 by 16 px | Only when the script has not run. With the script he is drawn on a canvas |
| `og.jpg` | The share image, 1200 by 630 px: the page's headline and Paper window over the Paper ridges | In link previews |
| `trek-mark.svg`, the icons, `favicon.*`, `apple-touch-icon.png` | The cairn (`assets/brand/trek-mark.svg`) and the app's ember icon (`assets/brand`) | The bar, the footer, the browser and the web manifest |

**Retaking a capture.** The captures are regenerated from the app, not edited by hand. One script,
`masters.sh`, stages the scene in the test app with the verify-trek driver (the `dress`, `title`
and `theme` controls), sizes the window, and takes each master with `screencapture` at 2x in Paper
and in Night. A second, `site_assets.sh`, converts the masters with
`cwebp -q 92 -alpha_q 100 -m 6 -sharp_yuv`. The iPhone frames come from the iPhone app's demo mode
(`-TrekDemo YES`, see `ios/README.md`) through `xcrun simctl io … screenshot`
(`iphone_masters.sh`). The scripts are in `script/site/`, with a note on how to run them.

**One retouch.** In the window pictures the three title-bar buttons are painted in, in macOS's own
colours and places: the test window is never the key window, so macOS draws them grey. Nothing
else in any picture is changed.

**The ridges** are drawn by the app's backdrop generator (`assets/backgrounds/make_backgrounds.py`):
the same ridge function, seeds, colours, glow and 4 by 4 Bayer step on 3-pixel cells, rendered
2880 px wide with the app's 1600 px picture in the middle, 1500 px tall, and split into two layers.
Paper uses the generator's Paper backdrop and Night uses its Dawn backdrop.

**The hiker** is the sprite and the walk of `crates/trek-mascot/src/lib.rs`, ported line for line
into `assets/site.js`: 16 frames a second, 16 art pixels a second, a pose every 4 pixels, and a
rest and a turn at each end of the trail. If the app's hiker changes, change both.

**Where a picture came from** is written into the picture. Each PNG and the JPEG carries its
origin inside the file, and each WebP has it in a `.json` file of the same name beside it. From
this folder, the design skill's `impeccable embed-prompt --read assets/img/<file>` prints it and
`impeccable embed-prompt --scan assets/img` lists any picture without one. Write the origin of a
new or retaken picture the same way.

Window captures are WebP at twice their shown size, with `width` and `height` set, and lazy below
the hero. Each picture ships in Paper and in Night. Keep a picture's name when replacing it;
`_headers` caches pictures for a day.

## Rules for the look

`DESIGN.md` is the record. The ones most easily broken:

- Headlines at a weight of 500 to 700, never light. No italics, no bold lead-ins, no two-tone
  headlines, no pill or label above a heading.
- Few pictures, each a real capture, and a window capture always at real size.
- No pause button: pressing the hiker stops him and sends him on.
- The page reads whole without its script and with Reduce Motion on.
- No trackers, no analytics, no cookies, and no type or script from another domain.

## Rules for the words

Every claim on the home page is true today and stays within what this repository's `README.md`
says. The privacy page follows `docs/PRIVACY.md`. No testimonials, no logos of other companies, no
competitors' names, and nothing that isn't shipped.
