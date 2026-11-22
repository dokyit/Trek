# trekdesktop.com

Trek's website: one page, a privacy page, a short terms page and a not-found page. Plain HTML, CSS and a little
JavaScript. There is no framework and no build step; the folder is served as it is.

```
site/
  index.html            the page, "One line": one ink line runs the whole page from the desk under
                        Trek's window to the download, through a field of characters at the top and
                        the bottom; its ember dot is the thread that needs you, reaching the phone
                        on the way; everything Trek does, a private-by-default box, the downloads
  privacy/index.html    docs/PRIVACY.md in plain words, and what this site itself does
  terms/index.html      the licence and terms of use in plain words (what Apache-2.0 already says)
  remote/               the web remote: Trek's remote for a phone's browser (an Android phone first of
                        all), which pairs from Trek's QR code and talks to Trek on the user's computer
                        over WebTransport. A web app of its own; docs/MOBILE.md, "The web remote"
  404.html
  assets/home/          the home page's own files: home.css (its styles), home.js (the line, the
                        dot, the phones coming together), field.js (the character field and its
                        ripples), inbox.css and inbox.js (Trek's window, redrawn so it can act),
                        dl.js (the download for the visitor's computer, from GitHub's API)
  assets/site.css       the privacy, terms and not-found pages' styles (no inline styles: the CSP
                        forbids them)
  assets/site.js        those pages' bar and download links
  assets/fonts/         Bricolage Grotesque, Hanken Grotesk and JetBrains Mono, with their OFL licences
  assets/img/           mark.svg (the shipping mark, the only copy of its shape), home/paper.webp
                        (the page's paper), features/ (the iPhone app's captures), icons, the
                        share image
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

Or `node script/site/serve.mjs 8731`, which also sends each page the headers `_headers` gives it, as
Cloudflare does: the web remote (`/remote/`) needs its own security policy to connect to Trek.

Any static server will do for the rest. Addresses in the pages start at `/`, so serve the `site` folder as the
root; opening `index.html` from the disk won't find its styles. `_headers` and `_redirects` only
take effect on Cloudflare.

Three views are worth a look before a change goes out:

- **Paper and Night.** The page follows the system's light or dark setting and has no switch of
  its own. Change the system setting, or the browser's emulation of it, to see the other theme.
- **Without the script, and with Reduce Motion on.** Nothing is hidden waiting for the script:
  the window lies flat, every section is there, and each loop stays a still picture (its poster). Every download link leads to the latest release. The page must say the
  same things in both.
- **Narrow, under 768 px.** The first screen's window keeps a near-real size and runs off the right
  edge, so the sidebar and its Inbox stay readable.

## How it is deployed

Cloudflare Pages serves the `site` folder of this repository's `main` branch.

- Framework preset: **None**. Build command: **none** (leave it empty).
- Build output directory: `site`.
- Production branch: `main`. Custom domain: `trekdesktop.com`.

Every push to `main` that changes `site/` is live a minute or so later. `www` to the bare domain,
and HTTP to HTTPS, are Cloudflare settings (a redirect rule and "Always Use HTTPS"), not files here.

Before a push, run `python3 script/site/stamp.py site`. It writes `?v=<hash of the file>` on every
link to the site's own stylesheet, script, pictures and fonts. Without it Cloudflare and browsers
keep serving the copy they hold under the same name, and a new page arrives with the last
version's stylesheet: that happened on 12 November 2026. The pre-push hook refuses a push whose
stamps are out of date.

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
- `index.html`, the iPhone row of the download list: the iPhone app is built from source. Change
  it when it is in TestFlight or the App Store.
- `index.html`, the download list, the page's description and its structured data: the systems
  Trek needs (macOS 13 or later, Windows 11, iOS 26).
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
| `features/iphone-poster.webp`, `features/iphone-night.webp` | The iPhone app answering an approval for `./migrate`, Paper and Night, 590 by 1276 px | The front phone in "Walk away", at its own pixels (295 points) |
| `features/iphone-threads-paper.webp`, `features/iphone-threads-night.webp` | The iPhone app's Threads list, Paper and Night | The phone behind it |
| `home/paper.webp` | A 512 px seamless paper tile, written by `script/site/make_paper.py` | Under the whole home page, soft-light |
| `og.jpg` | The share image, 1200 by 630 px: the headline over the character field, one line ending in the ember dot, drawn from the page itself by `script/site/make_og.mjs` | In link previews |
| `mark.svg`, the icons, `favicon.*`, `apple-touch-icon.png` | The shipping mark. `mark.svg` is the only copy of its shape; `script/site/make_icons.py` draws the icons from it | The bar, the footer, the browser and the web manifest |
| `backdrop/grain.svg` | A 240 px noise tile | The privacy, terms and not-found pages |

**Trek's window on the home page is redrawn**, in HTML (`assets/home/inbox.*`), so it can show one
thread stop, rise to the top and be answered; its threads are staged and the page says it is
redrawn. Its colours and type follow the real app's Paper and Night captures.

**The iPhone pictures** come from the iPhone app's demo mode (`-TrekDemo YES`, see
`ios/README.md`) through `xcrun simctl io … screenshot` (`iphone_masters.sh`). Each has its origin
in a `.json` file of the same name beside it. Keep a picture's name when replacing it and run
`script/site/stamp.py`, which gives its links a new address.

**The drawings** (the laptop with Trek's notification beside Native, the computer and phone in the
privacy box) are inline SVG in `index.html`, ink lines on the page's paper with one ember part, and
move by CSS alone.

The scripts for the earlier home page's photographs, window captures and feature loops
(`make_backdrops.py`, `make_illustrations.py`, `masters.sh`, `feature_media.sh`,
`themes_loop.py`) are kept, but their files are no longer on the page.

## Rules for the look

`DESIGN.md` is the record. The ones most easily broken:

- Headlines at a weight of 500 to 700, never light. No italics, no bold lead-ins, no two-tone
  headlines, no pill or label above a heading.
- One line, one ember dot. The line is drawn to the dot and no further; scrolled to the end, it
  is whole and the dot rests at Download. Ember is the dot, the one button and the drawings' one
  part that needs you.
- Pictures of the iPhone app are real captures; the window on the desk is redrawn and says so.
- The mark is never scenery: it is in the bar and the footer only.
- Nothing outdoors: no hills, trails or mountains anywhere on the page.
- The page reads whole without its script and with Reduce Motion on.
- No trackers, no analytics, no cookies, and no type or script from another domain.

## Rules for the words

Every claim on the home page is true today and stays within what this repository's `README.md`
says. The privacy page follows `docs/PRIVACY.md`. No testimonials, no logos of other companies on the home page (the web remote shows the agents'
own marks, as the app does, with the trademark note in `NOTICE`), no competitors' names, and
nothing that isn't shipped.
