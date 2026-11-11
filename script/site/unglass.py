"""Recover a frame's real alpha from the same frame laid over black and over white.

over_black = p, over_white = p + 255 (1 - a)   (p is premultiplied)
so a = 1 - (over_white - over_black) / 255, and the straight colour is p / a.

Where the two frames differ in what they show (a clock's digit, a spinner's arc), the channels
disagree. Those pixels take the black frame's own content: opaque where it shows a glyph, else the
glass beside them.

unglass.py BLACK.png WHITE.png OUT.png [crop_height_px]
"""
import sys

from PIL import Image, ImageChops

black = Image.open(sys.argv[1]).convert("RGB")
white = Image.open(sys.argv[2]).convert("RGB")
out = sys.argv[3]
if len(sys.argv) > 4:
    h = int(sys.argv[4])
    black = black.crop((0, 0, black.width, h))
    white = white.crop((0, 0, white.width, h))
W, H = black.size

diff = ImageChops.subtract(white, black)          # 255 (1 - a) per channel
r, g, b = diff.split()
lo = ImageChops.darker(ImageChops.darker(r, g), b)
hi = ImageChops.lighter(ImageChops.lighter(r, g), b)
spread = ImageChops.subtract(hi, lo).tobytes()
alpha = ImageChops.invert(Image.merge("RGB", (r, g, b)).convert("L"))

bp = black.load()
ap = alpha.load()
result = Image.new("RGBA", (W, H))
rp = result.load()
flagged = []
for y in range(H):
    row = y * W
    for x in range(W):
        if spread[row + x] > 24:
            flagged.append((x, y))
            continue
        a = ap[x, y]
        pr, pg, pb = bp[x, y]
        if 0 < a < 255:
            pr, pg, pb = (min(255, round(c * 255 / a)) for c in (pr, pg, pb))
        rp[x, y] = (pr, pg, pb, a)
marks = set(flagged)
for x, y in flagged:
    nx = x - 1
    while nx > 0 and (nx, y) in marks:
        nx -= 1
    glass = bp[nx, y]
    here = bp[x, y]
    if sum(abs(here[i] - glass[i]) for i in range(3)) > 36:
        rp[x, y] = (*here, 255)                   # a glyph in the black frame: keep it, solid
    else:
        rp[x, y] = rp[nx, y]                      # glass in the black frame: the glass beside it
result.save(out)
print(out, (W, H), "patched pixels:", len(flagged), "alpha range:", alpha.getextrema())
