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
| `site_assets.sh` | Converts the masters into `site/assets/img` (the earlier home page's window pictures; the home page now redraws its window, so nothing on the site uses its output today). |
| `make_icons.py` | Draws the mark from `site/assets/img/mark.svg` into the site's favicons, touch and manifest icons (`uv run --with pillow python -I script/site/make_icons.py site SCRATCH_DIR`). |
| `feature_media.sh` | Cuts pictures and loops into `site/assets/img/features/` from the screenshot atlas and the launch footage, preferring any file of the same name in the newest launch captures. Of its output the home page uses only the iPhone pictures. |
| `make_og.mjs` | Draws the share picture, `og.jpg`, from the home page itself in headless Chrome. |
| `make_paper.py` | Writes the home page's paper tile, `site/assets/img/home/paper.webp`. |
| `shoot_site.sh` | Serves `site/` for the length of the run and takes stills: desktop and phone, light and dark. |
| `rec.sh`, `record.mjs` | A real-time recording of a page from headless Chrome. |
| `check_links.py` | Checks every link in the site; `--online` asks outside addresses too. |
| `stamp.py` | Writes `?v=<hash of the file>` on every link to the site's own files (`SITE_DIR`), so a changed file is fetched at once and never served from an old copy. Run it after any change under `site/`; `--check` is what the pre-push hook runs. |

Nothing here opens a window: Chrome runs headless, the test app stays on its own display, and
Simulator.app is never started.
