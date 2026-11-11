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
| `site_assets.sh` | Converts the masters into `site/assets/img`; with `ridges`, redraws the backdrop with `gen_ridges.py`. |
| `gen_ridges.py` | The app's backdrop generator (`assets/backgrounds/make_backgrounds.py`), drawn wider and in two layers. |
| `shoot_site.sh` | Serves `site/` for the length of the run and takes stills: desktop and phone, light and dark. |
| `rec.sh`, `record.mjs` | A real-time recording of a page from headless Chrome. |
| `check_links.py` | Checks every link in the site; `--online` asks outside addresses too. |

Nothing here opens a window: Chrome runs headless, the test app stays on its own display, and
Simulator.app is never started.
