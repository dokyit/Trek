"""The website's own icons, drawn from the app's signpost (assets/brand/trek_icon.py, the one
source of the drawing): favicon.svg, favicon.ico, the touch icon, the manifest's icons and the
two `trek-mark` SVGs the pages' bar shows. The .ico's 16 and 32 px frames are the app icon's own
pixel-by-pixel art. Needs resvg on PATH (cargo install resvg); Python's standard library packs
the .ico.
make_icons.py SITE_DIR SCRATCH_DIR"""
import os, struct, subprocess, sys

site, scratch = sys.argv[1], sys.argv[2]
BRAND = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "assets", "brand")
sys.path.insert(0, BRAND)
import trek_icon as ti  # noqa: E402  (its drawing only: main() doesn't run on import)

SIGN = ti.Signpost(ti.PICK)

def svg(c, tile="round", scale=1.0):
    """The signpost on its tile, on a 96-unit square: a rounded tile (22 of 96) for the page and
    the browser, a square one for iOS and a maskable icon, which round it themselves."""
    bg = (f'<rect width="96" height="96" rx="22" fill="{c["tile"]}"/>' if tile == "round"
          else f'<rect width="96" height="96" fill="{c["tile"]}"/>')
    inner = ti.Drawn(SIGN).flat(**ti.colours(c))
    if scale != 1.0:
        inner = f'<g transform="translate(48 48) scale({scale}) translate(-48 -48)">{inner}</g>'
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96">{bg}{inner}</svg>\n'

def render(src, px, out):
    subprocess.run(["resvg", "-w", str(px), "-h", str(px), src, out], check=True)

def ico(frames):
    """A .ico of PNG frames: [(px, bytes)]."""
    out = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    for px, data in frames:
        out += struct.pack("<BBBBHHII", px % 256, px % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(data for _, data in frames)

def write(path, text):
    with open(path, "w") as f:
        f.write(text)

os.makedirs(scratch, exist_ok=True)
# The page's bar: a navy tile on a light page, the lavender one on a dark page, so the tile
# always stands against the page.
write(f"{site}/assets/img/trek-mark.svg", svg(ti.NIGHT))
write(f"{site}/assets/img/trek-mark-paper.svg", svg(ti.PAPER))
write(f"{site}/favicon.svg", svg(ti.NIGHT))
full = os.path.join(scratch, "full.svg")
write(full, svg(ti.NIGHT))
frames = []
for px in (16, 32, 48):
    src = os.path.join(BRAND, f"trek-icon-night-{px}.svg") if px in (16, 32) else full
    out = os.path.join(scratch, f"ico-{px}.png")
    render(src, px, out)
    with open(out, "rb") as f:
        frames.append((px, f.read()))
with open(f"{site}/favicon.ico", "wb") as f:
    f.write(ico(frames[::-1]))
# Full-bleed squares: iOS rounds the touch icon itself, and a maskable icon keeps its art in
# the middle.
for name, markup, px in (("apple-touch-icon.png", svg(ti.NIGHT, tile="square", scale=0.9), 180),
                         ("assets/img/icon-192.png", svg(ti.NIGHT), 192),
                         ("assets/img/icon-512.png", svg(ti.NIGHT), 512),
                         ("assets/img/icon-maskable-512.png", svg(ti.NIGHT, tile="square", scale=0.76), 512)):
    src = os.path.join(scratch, "icon.svg")
    write(src, markup)
    render(src, px, f"{site}/{name}")
print("icons written")
