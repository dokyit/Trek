# Tools for the website's pictures

These make and check what `site/` shows. They were written while the site was built and moved here
so the pictures can be made again. Since the move only `shoot_site.sh` (with `shoot.sh` and
`shoot_phone.sh`) has been run from this folder; the others have been syntax-checked, not re-run.

Set `TREK_SITE_SCRATCH` to a scratch folder first. Masters, frames and stills all go there, never
into the repository. `TREK_SITE_PYTHON` names a Python with Pillow and numpy (default `python3`).

| Script | What it does |
|---|---|
| `masters.sh` | Stages the isolated test app in its demo dress on the invisible test display and takes the Mac window masters at 2x (`all`, `stage`, `shoot W H`, `loop` for the Simulator scene, `stop`). |
| `iphone_masters.sh` | Takes the iPhone app's demo frames from the simulator with `simctl` only. |
| `site_assets.sh` | Converts the masters into `site/assets/img`. |
| `make_hero.py` | Composes the first screen's photograph (`PHOTO OUT_DIR`): the day picture and its dusk grade, as `hero-day.png` and `hero-night.png` at 2880 px. The photograph and its licence are named in `site/README.md`. |
| `make_hero_layers.py` | Cuts those two pictures into the planes the page moves (`HERO_DIR OUT_DIR`): `sky-*.png` and `land-*.png`, along the ridge, by the warmth of the grass. Then, for each of the four: `sips -Z 1600` for the small size, and `cwebp -q 80 -m 6 -sharp_yuv -alpha_q 90` into `site/assets/img/hero-{sky,land}-{day,night}-{1600,2880}.webp`, and write each origin with `impeccable embed-prompt`. Run it with `TREK_SITE_PYTHON` (it needs only Pillow and numpy). |
| `make_icons.py` | Draws the mark into the site's favicons, touch and manifest icons and the two `trek-mark` SVGs (`SITE_DIR SCRATCH_DIR`). |
| `shoot_site.sh` | Serves `site/` for the length of the run and takes stills: desktop and phone, light and dark. |
| `rec.sh`, `record.mjs` | A real-time recording of a page from headless Chrome. |
| `check_links.py` | Checks every link in the site; `--online` asks outside addresses too. |
| `stamp.py` | Writes `?v=<hash of the file>` on every link to the site's own files (`SITE_DIR`), so a changed file is fetched at once and never served from an old copy. Run it after any change under `site/`; `--check` is what the pre-push hook runs. |

Nothing here opens a window: Chrome runs headless, the test app stays on its own display, and
Simulator.app is never started.
