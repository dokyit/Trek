"""The website's own icons and share picture, from the signpost in site/assets/img/mark.svg (the only
copy of its shape): favicon.svg, favicon.ico, the touch icon, the manifest's icons and og.jpg.
Needs Google Chrome for rendering and Pillow for the .ico and the .jpg:

    uv run --with pillow python -I script/site/make_icons.py site SCRATCH_DIR
"""
import os, re, shutil, subprocess, sys, tempfile
from PIL import Image

site, scratch = os.path.abspath(sys.argv[1]), sys.argv[2]
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

mark = open(f"{site}/assets/img/mark.svg").read()
paths = {m.group(1): m.group(0) for m in re.finditer(r'<path id="([\w-]+)"[^>]*/>', mark)}

DAY = dict(tile="#C9C1DE", post="#1F1714", board="#EE5A24")
NIGHT = dict(tile="#1D1A2C", post="#F6F1EA", board="#FF7A3D")


def art(c, cls=False):
    """The mark's three paths, coloured, in its own 1024 space (viewBox 212 236 600 600)."""
    def one(key, colour, board):
        p = paths[key].replace(f' id="{key}"', "")
        paint = f' class="{"b" if board else "p"}"' if cls else (f' fill="{colour}"' + (f' stroke="{colour}"' if board else ""))
        return p.replace("<path", "<path" + paint, 1)
    return one("post", c["post"], False) + one("board-a", c["board"], True) + one("board-b", c["board"], True)


def tile(c, size=1024, scale=0.62, rounded=True, cls=False):
    """The mark on its lavender (or night) tile, as the app icon has it."""
    s = 600 / scale  # how much of the mark's space the tile spans
    x, y = 512 - s / 2, 536 - s / 2
    r = s * 0.225 if rounded else 0
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
style = ("<style>.t{fill:#C9C1DE}.p{fill:#1F1714}.b{fill:#EE5A24;stroke:#EE5A24}"
         "@media (prefers-color-scheme:dark){.t{fill:#1D1A2C}.p{fill:#F6F1EA}.b{fill:#FF7A3D;stroke:#FF7A3D}}</style>")
open(f"{site}/favicon.svg", "w").write(tile(DAY, 96, 0.66, cls=True).replace("%STYLE%", style) + "\n")

# The .ico: the mark a little larger on its tile at the small sizes, where every pixel counts.
for px, sc in ((16, 0.82), (32, 0.74), (48, 0.7)):
    render(tile(DAY, px, sc), px, px, f"{scratch}/ico-{px}.png")
ims = [Image.open(f"{scratch}/ico-{px}.png") for px in (48, 32, 16)]
ims[0].save(f"{site}/favicon.ico", format="ICO", append_images=ims[1:], sizes=[(48, 48), (32, 32), (16, 16)])

# Full-bleed squares: iOS rounds the touch icon itself, and a maskable icon keeps its art in the middle.
render(tile(DAY, 180, 0.56, rounded=False), 180, 180, f"{site}/apple-touch-icon.png")
render(tile(DAY, 192, 0.58), 192, 192, f"{site}/assets/img/icon-192.png")
render(tile(DAY, 512, 0.58), 512, 512, f"{site}/assets/img/icon-512.png")
render(tile(DAY, 512, 0.5, rounded=False), 512, 512, f"{site}/assets/img/icon-maskable-512.png")

# The share picture: the lavender sky, the signpost and the line, in the site's own type.
font = f"file://{site}/assets/fonts/BricolageGrotesque.woff2"
og = f"""<style>@font-face{{font-family:B;src:url({font})}}
.og{{width:1200px;height:630px;box-sizing:border-box;padding:0 96px;display:flex;align-items:center;gap:56px;
background:linear-gradient(180deg,#C9C1DE,#DCD6EA);font-family:B;color:#1F1714}}
.og svg{{width:250px;height:250px;flex:none}} .og h1{{margin:0;font-weight:680;font-size:96px;line-height:.95;letter-spacing:-.04em}}
.og p{{margin:22px 0 0;font-weight:500;font-size:38px;letter-spacing:-.02em;color:#463C47}}</style>
<div class="og"><svg viewBox="212 236 600 600">{art(DAY)}</svg><div><h1>Every agent.<br>One trail.</h1><p>trekdesktop.com</p></div></div>"""
render(og, 1200, 630, f"{scratch}/og.png")
Image.open(f"{scratch}/og.png").convert("RGB").save(f"{site}/assets/img/og.jpg", quality=90, optimize=True)
print("icons written")
