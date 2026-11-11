# trekdesktop.com

Trek's website: one page, a privacy page and a not-found page. Plain HTML, CSS and a little
JavaScript. There is no framework and no build step; the folder is served as it is.

```
site/
  index.html            the page
  privacy/index.html    docs/PRIVACY.md in plain words, and what this site itself does
  404.html
  assets/site.css       every style (no inline styles: the CSP forbids them)
  assets/site.js        the download button, and the hikers on the trail
  assets/fonts/         Inter and JetBrains Mono, subset, with their OFL licences
  assets/img/           screenshots, the ridges, the hiker, the cairn, icons, the share image
  _headers  _redirects  Cloudflare Pages: security headers, caching, short addresses
  robots.txt  sitemap.xml  site.webmanifest  favicon.ico  favicon.svg  apple-touch-icon.png
  DESIGN.md             what the look is built from, and how it differs from other sites
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
browser and points at its files; the Windows installer's address never changes; and with
JavaScript off, or if the API doesn't answer, every link leads to the latest release page.

Three places say a version or a state in words, and should be read again when it changes:

- `index.html`, the small print under the stat row: the fallback download size ("38") and version
  ("0.4.49") shown before the API answers, and the measurements from `docs/PERFORMANCE.md`.
- `index.html`, the install tiles, the hero's trust line and `assets/site.js` (`TRUST`): the first
  launch. Today the Mac build is signed but not notarized and the Windows build has no code
  signature. Change the words the day that changes, and not before.
- `index.html`, the iPhone section, the install tile and the FAQ: the iPhone app is built from
  source. Change it when it is in TestFlight or the App Store.

## Pictures

Every app picture is a real frame from the app, taken with the verify-trek skill
(`.agents/skills/verify-trek`) in its isolated profile on the invisible test display: a demo
project, the scripted agent, example names. The test app names its scripted agent in the composer
and under it, so every picture is cut above the composer or taken from a view without one. Check
that again whenever a picture is replaced: no picture may say "Mock agent" or "Mock Swift".

- **The hero** is the app in its glass look. The frame was taken twice, over black and over white
  (`TREK_SHOT_UNDER`), and its real alpha recovered from the pair, so the sidebar and title bar are
  see-through in the WebP and the page's hills show through them.
- **The iPhone pictures** are the repository's own demo-mode screenshots
  (`docs/assets/screenshots/iphone`).
- **The ridges** (`ridges-*.svg`) use the ridge function, seeds and colours of
  `assets/backgrounds/make_backgrounds.py`, drawn as vector outlines.
- **The hiker** (`hiker-*.svg`) is the sprite of `crates/trek-mascot`, five poses side by side.
- **The fonts** are subsets of `crates/trek-assets/assets/fonts` (`pyftsubset`, Latin, WOFF2).

Pictures are WebP at twice their shown size, with `width` and `height` set, and lazy below the
hero. Keep a picture's name when replacing it; `_headers` caches pictures for a day.

## Rules for the words

Every claim on the page is true today and can be found in this repository: `README.md`,
`docs/PERFORMANCE.md`, `docs/PRIVACY.md`, `docs/MOBILE.md` and the feature pages under
`.agents/skills/verify-trek/references/features`. Numbers carry their conditions. No testimonials,
no logos of other companies, no competitors' names, and nothing that isn't shipped.
