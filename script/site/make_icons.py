"""The website's own icons and share picture, from the signpost in site/assets/img/mark.svg (the only
copy of its shape on the site, the app's own geometry): favicon.svg, favicon.ico, the touch icon,
the manifest's icons (the share picture, og.jpg, is make_og.mjs's). The .ico's 16 and 32 px frames are the app icon's own
pixel-by-pixel art (assets/brand/trek-icon-paper-{16,32}.svg). Needs Google Chrome for rendering
and Pillow for the .ico and the .jpg:

    uv run --with pillow python -I script/site/make_icons.py site SCRATCH_DIR
"""
import os, re, shutil, subprocess, sys, tempfile
from PIL import Image

site, scratch = os.path.abspath(sys.argv[1]), sys.argv[2]
BRAND = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "assets", "brand")
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

mark = open(f"{site}/assets/img/mark.svg").read()
paths = {m.group(1): m.group(0) for m in re.finditer(r'<path id="([\w-]+)"[^>]*/>', mark)}

DAY = dict(tile="#C9C1DE", post="#1F1714", board="#EE5A24")
NIGHT = dict(tile="#1D1A2C", post="#F6F1EA", board="#FF7A3D")


def art(c, cls=False):
    """The mark's three paths, coloured, in the app icon's 96-unit space (the mark's box is 13 13 70 70)."""
    def one(key, colour, board):
        p = paths[key].replace(f' id="{key}"', "")
        paint = f' class="{"b" if board else "p"}"' if cls else f' fill="{colour}"'
        return p.replace("<path", "<path" + paint, 1)
    return one("post", c["post"], False) + one("board-a", c["board"], True) + one("board-b", c["board"], True)


def tile(c, size=96, scale=1.0, rounded=True, cls=False):
    """The mark on its lavender (or night) tile. At scale 1 it sits as on the app icon (a 96-unit
    tile, corners of 22); a smaller scale leaves more tile around it."""
    s = 96 / scale  # how much of the mark's space the tile spans
    x = y = 48 - s / 2
    r = 22 / scale if rounded else 0
    bg = f'<rect class="t" x="{x}" y="{y}" width="{s}" height="{s}" rx="{r}"/>' if cls else f'<rect x="{x}" y="{y}" width="{s}" height="{s}" rx="{r}" fill="{c["tile"]}"/>'
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="{x} {y} {s} {s}">{"%STYLE%" if cls else ""}{bg}{art(c, cls)}</svg>'


def render(html, w, h, out):
    d = tempfile.mkdtemp(prefix="trek-icons-")
    page = os.path.join(d, "i.html")
    open(page, "w").write(f"<!doctype html><meta charset=utf-8><style>html,body{{margin:0;background:transparent}}svg{{display:block}}</style>{html}")
    shot = os.path.join(d, "s.png")
    subprocess.run([CHROME, "--headless=new", f"--user-data-dir={d}/p", "--hide-scrollbars", "--force-device-scale-factor=1", "--default-background-color=00000000",
                    f"--window-size={max(w, 500)},{max(h, 500)}", f"--screenshot={shot}", "file://" + page], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90)
    Image.open(shot).convert("RGBA").crop((0, 0, w, h)).save(out)
    shutil.rmtree(d, ignore_errors=True)


os.makedirs(scratch, exist_ok=True)

# The favicon follows the system's light or dark setting, as the page does.
style = ("<style>.t{fill:#C9C1DE}.p{fill:#1F1714}.b{fill:#EE5A24}"
         "@media (prefers-color-scheme:dark){.t{fill:#1D1A2C}.p{fill:#F6F1EA}.b{fill:#FF7A3D}}</style>")
open(f"{site}/favicon.svg", "w").write(tile(DAY, 96, cls=True).replace("%STYLE%", style) + "\n")

# The .ico: at 16 and 32 px the app's own pixel-tuned icons, at 48 the drawing.
for px in (16, 32):
    render(open(os.path.join(BRAND, f"trek-icon-paper-{px}.svg")).read().replace("<svg ", f'<svg width="{px}" height="{px}" ', 1), px, px, f"{scratch}/ico-{px}.png")
render(tile(DAY, 48), 48, 48, f"{scratch}/ico-48.png")
ims = [Image.open(f"{scratch}/ico-{px}.png") for px in (48, 32, 16)]
ims[0].save(f"{site}/favicon.ico", format="ICO", append_images=ims[1:], sizes=[(48, 48), (32, 32), (16, 16)])

# Full-bleed squares: iOS rounds the touch icon itself, and a maskable icon keeps its art in the middle.
render(tile(DAY, 180, 0.9, rounded=False), 180, 180, f"{site}/apple-touch-icon.png")
render(tile(DAY, 192), 192, 192, f"{site}/assets/img/icon-192.png")
render(tile(DAY, 512), 512, 512, f"{site}/assets/img/icon-512.png")
render(tile(DAY, 512, 0.76, rounded=False), 512, 512, f"{site}/assets/img/icon-maskable-512.png")

# The share picture (og.jpg) is drawn from the home page itself: script/site/make_og.mjs.
print("icons written")
