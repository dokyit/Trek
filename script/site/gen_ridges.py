"""Trek's backdrop art for the site, from the app's own generator (assets/backgrounds/make_backgrounds.py):
the same ridge function, seeds, colours, glow and 4x4 Bayer step on 3-pixel cells, drawn wider and
split into a back layer (sky and the two far ridges) and a front layer (the near ridge, on transparency).
gen_ridges.py OUT_DIR [WIDTH]"""
import math, random, sys
import numpy as np
from PIL import Image

out = sys.argv[1]
W = int(sys.argv[2]) if len(sys.argv) > 2 else 2880
H, REF, TALL = 1000, 1600, 1500
BAYER = np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) / 16 - 0.5

def hexc(h): return np.array([int(h[i:i + 2], 16) for i in (1, 3, 5)], float)

def ramp(stops, t):
    t = np.clip(t, 0, 1)
    c = np.zeros(t.shape + (3,))
    done = np.zeros(t.shape, bool)
    for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
        m = (~done) & (t <= t1)
        k = ((t - t0) / ((t1 - t0) or 1))[..., None]
        c = np.where(m[..., None], c0 + (c1 - c0) * k, c)
        done |= m
    return np.where(done[..., None], c, stops[-1][1])

def ridge(x, seed, base, amp):
    r = random.Random(seed); y = np.full(x.shape, float(base))
    for k in range(1, 7):
        f = 0.0016 * 2 ** k; p = r.random() * 6.28
        y += amp / (k ** 1.15) * np.sin(x * f + p)
    return y

def render(name, sky, ridges, glow, levels=6, cell=3):
    q = 255 / (levels - 1)
    ys, xs = np.mgrid[0:TALL, 0:W].astype(float)
    xa = xs - (W - REF) / 2            # the app's picture sits in the middle of the wider one
    c = ramp([(t, hexc(h)) for t, h in sky], ys / H)
    gx, gy, gr, gc = glow
    d = np.hypot(xa - gx, (ys - gy) * 1.15) / REF
    g = np.maximum(0.0, 1 - d / gr) ** 2.2
    c = c + (hexc(gc) - c) * (g * 0.85)[..., None]
    near = None
    for i, (seed, base, amp, col, haze) in enumerate(ridges):
        ry = ridge(xa, seed, base * H, amp * H)
        inside = ys > ry
        depth = np.minimum(1, (ys - ry) / (H * 0.35))
        col = hexc(col)
        a = c + (col - c) * (1 - haze)
        b = a + (col - a) * (depth * 0.6)[..., None]
        if i == len(ridges) - 1:
            near = (inside, b)
        else:
            c = np.where(inside[..., None], b, c)
    bay = BAYER[(ys.astype(int) // cell) % 4, (np.floor(xa).astype(int) // cell) % 4][..., None]
    def dith(v): return np.clip(np.round((v + bay * q) / q) * q, 0, 255).astype(np.uint8)
    Image.fromarray(dith(c), "RGB").save(f"{out}/ridges-{name}-back.png", optimize=True)
    inside, b = near
    rgba = np.dstack([dith(b), np.where(inside, 255, 0).astype(np.uint8)])
    Image.fromarray(rgba, "RGBA").save(f"{out}/ridges-{name}-front.png", optimize=True)

render("dawn",
       sky=[(0, "#0B1222"), (0.35, "#1B2340"), (0.58, "#7A3B3A"), (0.70, "#FF8A3D"), (1, "#0A0A0C")],
       ridges=[(3, 0.62, 0.05, "#2A1E2A", 0.55), (5, 0.70, 0.06, "#16121A", 0.35), (9, 0.80, 0.05, "#0B0A0D", 0.1)],
       glow=(REF * 0.5, H * 0.63, 0.32, "#FFC56B"))
render("paper",
       sky=[(0, "#F3EEE6"), (0.55, "#F6E2CF"), (0.72, "#F2B98E"), (1, "#FAF7F2")],
       ridges=[(21, 0.63, 0.05, "#E7D6C6", 0.6), (23, 0.72, 0.06, "#D9C3B0", 0.45), (29, 0.82, 0.05, "#C9AE98", 0.3)],
       glow=(REF * 0.5, H * 0.62, 0.3, "#FFD8A8"))
print("ok")
