"""The website's own icons, from the mark's geometry: favicon.svg, favicon.ico, the touch icon and
the manifest's icons. The small sizes are drawn for their size (no trail dots at 16 px, fewer and
larger at 32), as the app's are. Needs Google Chrome for rendering and Pillow for the .ico.
make_icons.py SITE_DIR SCRATCH_DIR"""
import os, subprocess, sys, shutil, tempfile
from PIL import Image

site, scratch = sys.argv[1], sys.argv[2]
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
OUT = "M9.5 78.5 32.8 46.6 43 43.4 56 21 86.5 78.5Z"
RIDGE = [(56, 21), (60.6, 39.6), (55, 51.6), (62.6, 64.8), (58.8, 78.5)]
CLIP = "M56 4 " + " ".join(f"L{x} {y}" for x, y in RIDGE) + " L58.8 96H100V4Z"
FULL = [(29, 74.2, 2.4), (36, 69.2, 2.4), (43, 66, 2.4), (47.2, 60, 2.4), (44, 53.2, 2.4), (48, 47, 2.4), (51.6, 40.4, 2.4), (53.6, 34, 2.4), (54.6, 28.2, 2)]
FEW = [(31, 73, 3.6), (43.5, 64.5, 3.6), (45.5, 51, 3.6), (52.5, 38, 3.4)]
NIGHT = dict(tile="#1A1A1C", lit="#FF7A3D", shade="#C9551F", flag="#FFC7A0")
PAPER = dict(tile="#FBF8F3", lit="#E85D1F", shade="#B8461A", flag="#B8461A")

def art(c, dots, flag=1.0):
    body = lambda col: f'<path d="{OUT}" fill="{col}" stroke="{col}" stroke-width="4.5" stroke-linejoin="round"/>'
    pole_top = 24 - 15.4 * flag
    cloth = (f'<path d="M56.4 {pole_top} C{56.4+2.9*flag} {pole_top-1.4*flag} {56.4+5.1*flag} {pole_top+1.6*flag} {56.4+9.3*flag} {pole_top+0.2*flag} '
             f'V{pole_top+6.6*flag} C{56.4+5.1*flag} {pole_top+8*flag} {56.4+2.9*flag} {pole_top+5*flag} 56.4 {pole_top+6.4*flag} Z" fill="{c["flag"]}"/>')
    return (f'<path d="M56 24V{pole_top}" stroke="{c["flag"]}" stroke-width="{2*flag}" stroke-linecap="round"/>{cloth}'
            f'{body(c["lit"])}<clipPath id="r"><path d="{CLIP}"/></clipPath><g clip-path="url(#r)">{body(c["shade"])}</g>'
            f'<g fill="{c["tile"]}">' + "".join(f'<circle cx="{x}" cy="{y}" r="{r}"/>' for x, y, r in dots) + "</g>")

def svg(c, dots=FULL, flag=1.0, tile="round", scale=1.0):
    bg = f'<rect width="96" height="96" rx="22" fill="{c["tile"]}"/>' if tile == "round" else f'<rect width="96" height="96" fill="{c["tile"]}"/>'
    inner = art(c, dots, flag)
    if scale != 1.0: inner = f'<g transform="translate(48 48) scale({scale}) translate(-48 -48)">{inner}</g>'
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96">{bg}{inner}</svg>\n'

def render(markup, px, out):
    d = tempfile.mkdtemp(prefix="trek-icons-")
    page = os.path.join(d, "i.html")
    open(page, "w").write(f'<!doctype html><style>html,body{{margin:0;background:transparent}}svg{{display:block;width:{px}px;height:{px}px}}</style>{markup}')
    shot = os.path.join(d, "s.png")
    side = max(px, 600)
    subprocess.run([CHROME, "--headless=new", f"--user-data-dir={d}/p", "--hide-scrollbars", "--force-device-scale-factor=1", "--default-background-color=00000000",
                    f"--window-size={side},{side}", f"--screenshot={shot}", "file://" + page], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90)
    Image.open(shot).convert("RGBA").crop((0, 0, px, px)).save(out)
    shutil.rmtree(d, ignore_errors=True)

os.makedirs(scratch, exist_ok=True)
open(f"{site}/favicon.svg", "w").write(svg(NIGHT, FEW, 1.2))
open(f"{site}/assets/img/trek-mark.svg", "w").write(svg(NIGHT))
open(f"{site}/assets/img/trek-mark-paper.svg", "w").write(svg(PAPER))
# the .ico: 16 without dots, 32 with a few, 48 with the whole trail
sizes = {16: svg(NIGHT, [], 1.3), 32: svg(NIGHT, FEW, 1.2), 48: svg(NIGHT)}
for px, markup in sizes.items(): render(markup, px, f"{scratch}/ico-{px}.png")
ims = [Image.open(f"{scratch}/ico-{px}.png") for px in (48, 32, 16)]
ims[0].save(f"{site}/favicon.ico", format="ICO", append_images=ims[1:], sizes=[(48, 48), (32, 32), (16, 16)])
# full-bleed squares: iOS rounds the touch icon itself, and a maskable icon keeps its art in the middle
render(svg(NIGHT, tile="square", scale=0.86), 180, f"{site}/apple-touch-icon.png")
render(svg(NIGHT), 192, f"{site}/assets/img/icon-192.png")
render(svg(NIGHT), 512, f"{site}/assets/img/icon-512.png")
render(svg(NIGHT, tile="square", scale=0.70), 512, f"{site}/assets/img/icon-maskable-512.png")
print("icons written")
