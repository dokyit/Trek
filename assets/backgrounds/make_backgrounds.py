"""Trek's built-in background art: dithered skies over switchback ridges.
Pure Python (zlib PNG writer) so it runs anywhere. Output: dawn.png, night.png, paper.png (1600x1000)."""
import math, zlib, struct, random

W, H = 1600, 1000
BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]

def png(path, rows):
    raw = b"".join(b"\x00" + bytes(r) for r in rows)
    def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

def lerp(a, b, t): return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))
def ramp(stops, t):
    t = min(max(t, 0), 1)
    for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
        if t <= t1: return lerp(c0, c1, (t - t0) / (t1 - t0 or 1))
    return stops[-1][1]
def hexc(h): return tuple(int(h[i:i+2], 16) for i in (1, 3, 5))

def ridge(x, seed, base, amp):
    r = random.Random(seed); y = base
    for k in range(1, 7):
        f = 0.0016 * 2 ** k; p = r.random() * 6.28
        y += amp / (k ** 1.15) * math.sin(x * f + p)
    return y

def render(name, sky, ridges, glow, levels=6, stars=False, cell=3):
    random.seed(7)
    star_set = {(random.randrange(W), random.randrange(int(H * 0.55))) for _ in range(900)} if stars else set()
    rows = []
    q = 255 / (levels - 1)
    for y in range(H):
        row = bytearray()
        ty = y / H
        for x in range(W):
            c = ramp(sky, ty)
            # warm glow around the summit point
            gx, gy, gr, gc = glow
            d = math.hypot(x - gx, (y - gy) * 1.15) / W
            g = max(0.0, 1 - d / gr) ** 2.2
            c = lerp(c, gc, g * 0.85)
            for (seed, base, amp, col, haze) in ridges:
                ry = ridge(x, seed, base * H, amp * H)
                if y > ry:
                    depth = min(1, (y - ry) / (H * 0.35))
                    c = lerp(lerp(c, col, 1 - haze), col, depth * 0.6)
            if (x, y) in star_set: c = (240, 236, 225)
            # ordered dither on a coarse cell grid for the halftone look
            b = BAYER[(y // cell) % 4][(x // cell) % 4] / 16 - 0.5
            row += bytes(min(255, max(0, int(round((ch + b * q) / q) * q))) for ch in c)
        rows.append(row)
    png(name, rows)

import sys
only = sys.argv[1:] 
if not only or "dawn" in only: render("dawn.png",
       sky=[(0, hexc("#0B1222")), (0.35, hexc("#1B2340")), (0.58, hexc("#7A3B3A")), (0.70, hexc("#FF8A3D")), (1, hexc("#0A0A0C"))],
       ridges=[(3, 0.62, 0.05, hexc("#2A1E2A"), 0.55), (5, 0.70, 0.06, hexc("#16121A"), 0.35), (9, 0.80, 0.05, hexc("#0B0A0D"), 0.1)],
       glow=(W * 0.5, H * 0.63, 0.32, hexc("#FFC56B")))
if not only or "night" in only: render("night.png",
       sky=[(0, hexc("#05070D")), (0.5, hexc("#0E1830")), (0.72, hexc("#1F3157")), (1, hexc("#07080B"))],
       ridges=[(11, 0.64, 0.05, hexc("#141B2C"), 0.5), (13, 0.74, 0.06, hexc("#0B0F19"), 0.3), (17, 0.84, 0.04, hexc("#060709"), 0.1)],
       glow=(W * 0.72, H * 0.22, 0.05, hexc("#E8ECF3")), stars=True)
if not only or "paper" in only: render("paper.png",
       sky=[(0, hexc("#F3EEE6")), (0.55, hexc("#F6E2CF")), (0.72, hexc("#F2B98E")), (1, hexc("#FAF7F2"))],
       ridges=[(21, 0.63, 0.05, hexc("#E7D6C6"), 0.6), (23, 0.72, 0.06, hexc("#D9C3B0"), 0.45), (29, 0.82, 0.05, hexc("#C9AE98"), 0.3)],
       glow=(W * 0.5, H * 0.62, 0.3, hexc("#FFD8A8")))
print("ok")
