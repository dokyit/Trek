"""Trek's background collections beyond the classic three (make_backgrounds.py): Paintings, ASCII
and Light. Each piece comes in a Night and a Paper version, with a small thumbnail of each for
Settings › Appearance.

  Paintings  public-domain landscapes (SOURCES.md), dithered into Trek's palette: near-black,
             greys and ember on Night; cream, warm greys and ember on Paper.
  ASCII      landscapes drawn in text styles (ASCII ramp, braille, halftone, blocks, LED
             matrix, contour lines), tall, for the sidebar.
  Light      long-exposure light whose bright parts are made of glyphs, made here from code.

Needs Pillow and numpy, and the paintings downloaded as SOURCES.md lists them:

  uv run --with pillow --with numpy python assets/backgrounds/make_collections.py <sources> [piece…]

Writes crates/trek-assets/assets/backgrounds/<collection>/<piece>-<night|paper>.png|jpg and
…-thumb.jpg. The palettes' extremes are mirrored in crates/trek-app/src/backgrounds.rs, whose
tests read every file back and hold it to them (the scrim over the transcript is worked out from
them, so text keeps 4.5:1).
"""
import math, os, random, sys
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(ROOT, "crates", "trek-assets", "assets", "backgrounds")
FONT = os.path.join(ROOT, "crates", "trek-assets", "assets", "fonts", "JetBrainsMonoNL-Regular.ttf")
FONT_BOLD = os.path.join(ROOT, "crates", "trek-assets", "assets", "fonts", "JetBrainsMonoNL-Bold.ttf")

# The main pane at 2x, and the sidebar column at 2x (272 pt wide, tall windows).
WIDE = (2400, 1500)
TALL = (720, 2160)
THUMB = (200, 132)


def hexc(h):
    return np.array([int(h[i:i + 2], 16) for i in (1, 3, 5)], dtype=np.float64)


def bayer(n):
    m = np.array([[0]])
    while m.shape[0] < n:
        m = np.block([[4 * m, 4 * m + 2], [4 * m + 3, 4 * m + 1]])
    return (m + 0.5) / m.size


B8 = bayer(8)


def smooth(e0, e1, x):
    t = np.clip((x - e0) / (e1 - e0), 0, 1)
    return t * t * (3 - 2 * t)


def crop_to(im, aspect, fx=0.5, fy=0.5, zoom=1.0, inset=0.0):
    """The largest `aspect` (w/h) window of `im` around (fx, fy), shrunk by `zoom`."""
    w, h = im.size
    if inset:
        im = im.crop((int(w * inset), int(h * inset), int(w * (1 - inset)), int(h * (1 - inset))))
        w, h = im.size
    if w / h > aspect:
        cw, ch = h * aspect, h
    else:
        cw, ch = w, w / aspect
    cw, ch = cw * zoom, ch * zoom
    x0 = min(max(fx * w - cw / 2, 0), w - cw)
    y0 = min(max(fy * h - ch / 2, 0), h - ch)
    return im.crop((int(x0), int(y0), int(x0 + cw), int(y0 + ch)))


def luma(rgb):
    return rgb[..., 0] * 0.2126 + rgb[..., 1] * 0.7152 + rgb[..., 2] * 0.0722


def tone(rgb, lo_pct=1.0, hi_pct=99.6, gamma=1.0, contrast=0.35, lift=0.0):
    """Luminance, levelled to the painting's own range and given some bite."""
    L = luma(rgb)
    lo, hi = np.percentile(L, lo_pct), np.percentile(L, hi_pct)
    L = np.clip((L - lo) / max(hi - lo, 1e-3), 0, 1) ** gamma
    L = L * (1 - contrast) + contrast * L * L * (3 - 2 * L)
    return np.clip(L + lift, 0, 1)


def warmth(im, gain=1.0, lo=0.16, hi=0.55):
    """How much of a pixel is sunset: saturated reds, oranges and golds."""
    hsv = np.asarray(im.convert("HSV"), dtype=np.float64) / 255.0
    h, s, v = hsv[..., 0] * 360, hsv[..., 1], hsv[..., 2]
    hue = np.where(h < 55, 1.0, np.where(h < 75, 1 - (h - 55) / 20, 0.0))
    hue = np.maximum(hue, np.where(h > 335, (h - 335) / 25, 0.0))
    return smooth(lo, hi, s * hue * gain * np.clip(v * 1.6, 0, 1))


# ---------------------------------------------------------------- palettes
# Index 0 is the ground; each ramp runs from the ground to its brightest (Night) or from its
# darkest ink to the ground (Paper), so a ramp's index follows the painting's light either way.
NIGHT = {
    "grey": ["#0A0A0B", "#1E1E20", "#3A383C", "#646165", "#A19D99"],
    "ember": ["#0A0A0B", "#3A190C", "#7A3315", "#C8521F", "#FF7A3D"],
    # Ember only where the painting is lit as well as warm: a sunset, not a dark autumn wood.
    "warm_light": (0.08, 0.35),
}
PAPER = {
    "grey": ["#6A6058", "#9A8F83", "#C4BAAD", "#E2DACF", "#F3EEE6"],
    "ember": ["#7E4A33", "#B0603C", "#E2733F", "#F2B08C", "#F3EEE6"],
    "warm_light": (0.15, 0.55),
}


def dither_painting(im, size, cell, pal, warm_gain=1.0, **tone_args):
    gw, gh = size[0] // cell, size[1] // cell
    small = im.convert("RGB").resize((gw, gh), Image.LANCZOS)
    rgb = np.asarray(small, dtype=np.float64) / 255.0
    L = tone(rgb, **tone_args)
    W = warmth(small, gain=warm_gain)
    ys, xs = np.mgrid[0:gh, 0:gw]
    t_tone = B8[ys % 8, xs % 8]
    t_warm = B8[(ys + 3) % 8, (xs + 5) % 8]
    n = len(pal["grey"])
    v = L * (n - 1)
    idx = np.clip(np.floor(v) + (v - np.floor(v) > t_tone), 0, n - 1).astype(np.int64)
    use_ember = W * smooth(*pal["warm_light"], L) > t_warm
    colours = np.stack([np.array([hexc(c) for c in pal["grey"]]), np.array([hexc(c) for c in pal["ember"]])])
    out = colours[use_ember.astype(np.int64), idx]
    img = Image.fromarray(out.astype(np.uint8), "RGB").resize((gw * cell, gh * cell), Image.NEAREST)
    return img


PAINTINGS = {
    # name: (source file, crop, tone, warmth gain)
    "matterhorn": ("matterhorn.jpg", dict(fx=0.5, fy=0.30, inset=0.012), dict(gamma=1.05), 0.8),
    "kauterskill": ("kauterskill.jpg", dict(fx=0.5, fy=0.36, inset=0.012), dict(gamma=1.25, contrast=0.5), 1.1),
    "oxbow": ("oxbow.jpg", dict(fx=0.5, fy=0.5, inset=0.015), dict(gamma=1.1), 1.2),
    "notch": ("notch.jpg", dict(fx=0.5, fy=0.5, inset=0.01), dict(gamma=1.1), 1.0),
    "green-river": ("green-river.jpg", dict(fx=0.62, fy=0.5, inset=0.01), dict(gamma=1.15), 1.1),
    "aosta": ("aosta.jpg", dict(fx=0.5, fy=0.42, inset=0.012), dict(gamma=1.15, contrast=0.45), 1.3),
    "keelmen": ("keelmen.jpg", dict(fx=0.5, fy=0.42, inset=0.012), dict(gamma=1.3, contrast=0.45), 1.6),
    "merced": ("merced.jpg", dict(fx=0.5, fy=0.5, inset=0.012), dict(gamma=1.1), 1.1),
}


def paintings(src, only):
    os.makedirs(os.path.join(OUT, "paintings"), exist_ok=True)
    for name, (file, crop, tone_args, gain) in PAINTINGS.items():
        if only and name not in only:
            continue
        im = crop_to(Image.open(os.path.join(src, file)).convert("RGB"), WIDE[0] / WIDE[1], **crop)
        for variant, pal in (("night", NIGHT), ("paper", PAPER)):
            img = dither_painting(im, WIDE, 5, pal, warm_gain=gain, **tone_args)
            save(img, "paintings", name, variant, png=True)


# ---------------------------------------------------------------- glyph helpers
def font(px, bold=False):
    return ImageFont.truetype(FONT_BOLD if bold else FONT, px)


def mix(a, b, t):
    return a + (b - a) * t


# ---------------------------------------------------------------- ASCII (sidebar)
# The sidebar column: the threads are listed from its top, so each piece rises from the bottom
# and leaves the top to the ground. A piece is a `density` field (0..1, how much is drawn where)
# with `warm` (which marks are ember) — on Night density is light, on Paper it's ink.
ASCII_NIGHT = {"ground": "#111113", "ink": ["#29282B", "#3D3B40", "#57555A", "#77747A", "#9A9693"], "ember": ["#4A200E", "#7E3415", "#B84A1D", "#E8642B", "#FF7A3D"]}
ASCII_PAPER = {"ground": "#F6F5F3", "ink": ["#E2DDD6", "#CCC4BA", "#B0A69A", "#918679", "#74695E"], "ember": ["#F2C9AE", "#EEA27A", "#E9814C", "#E85D1F", "#B9461A"]}


def ascii_colour(pal, level, warm):
    """`level` 0..1 picks a step of the ink or ember ramp."""
    ramp = pal["ember"] if warm else pal["ink"]
    return tuple(int(c) for c in hexc(ramp[int(np.clip(level, 0, 0.999) * len(ramp))]))


def rgb(h):
    return tuple(int(c) for c in hexc(h))


def cells(a, cols, rows):
    return np.asarray(Image.fromarray((np.clip(a, 0, 1) * 255).astype(np.uint8)).resize((cols, rows), Image.BOX), dtype=np.float64) / 255


def draw_ramp(D, W, size, pal, chars=" .'`:-=+*%#@", px=18, lift=0.5, cut=0.0):
    """Classic ASCII: one character per cell, sparser to denser."""
    f = font(px)
    cw, ch = int(f.getlength("M")), int(px * 1.25)
    cols, rows = size[0] // cw, size[1] // ch
    d, w = cells(D, cols, rows) ** lift, cells(W, cols, rows)
    img = Image.new("RGB", size, rgb(pal["ground"]))
    dr = ImageDraw.Draw(img)
    ox = (size[0] - cols * cw) // 2
    for r in range(rows):
        for c in range(cols):
            v = d[r, c]
            k = int(v * (len(chars) - 1) + 0.5)
            if k == 0 or v < cut:
                continue
            dr.text((ox + c * cw, r * ch), chars[k], font=f, fill=ascii_colour(pal, 0.3 + v * 0.8, w[r, c] > 0.5))
    return img


def draw_braille(D, W, size, pal, pitch=7, gap=6):
    """Braille: characters of 2×4 dots, each dot on where the dithered light is."""
    cell_w, cell_h = 2 * pitch + gap, 4 * pitch + gap
    cols, rows = size[0] // cell_w, size[1] // cell_h
    dots = cells(D, cols * 2, rows * 4)
    w = cells(W, cols, rows)
    img = Image.new("RGB", (size[0] * 2, size[1] * 2), rgb(pal["ground"]))
    dr = ImageDraw.Draw(img)
    ox = (size[0] - cols * cell_w) // 2
    for y in range(rows * 4):
        for x in range(cols * 2):
            v = dots[y, x]
            if v <= B8[y % 8, x % 8] * 0.8 + 0.16:
                continue
            gx = ox + (x // 2) * cell_w + (x % 2) * pitch + pitch / 2
            gy = (y // 4) * cell_h + (y % 4) * pitch + pitch / 2
            r = pitch * 0.34
            col = ascii_colour(pal, 0.35 + v * 0.75, w[y // 4, x // 2] > 0.45)
            dr.ellipse((2 * (gx - r), 2 * (gy - r), 2 * (gx + r), 2 * (gy + r)), fill=col)
    return img.resize(size, Image.LANCZOS)


def draw_halftone(D, W, size, pal, step=13, angle=0.26):
    """Halftone: dots on a rotated screen, each as large as the light under it."""
    big = Image.new("RGB", (size[0] * 2, size[1] * 2), rgb(pal["ground"]))
    dr = ImageDraw.Draw(big)
    h, w = D.shape
    ca, sa = math.cos(angle), math.sin(angle)
    span = int(max(size) * 1.5 / step)
    for i in range(-span, span):
        for j in range(-span, span):
            x = size[0] / 2 + (i * ca - j * sa) * step
            y = size[1] / 2 + (i * sa + j * ca) * step
            if not (0 <= x < size[0] and 0 <= y < size[1]):
                continue
            v = D[int(y * h / size[1]), int(x * w / size[0])]
            r = step * 0.58 * math.sqrt(v)
            if r < 0.8:
                continue
            col = ascii_colour(pal, 0.3 + v * 0.8, W[int(y * h / size[1]), int(x * w / size[0])] > 0.5)
            dr.ellipse((2 * (x - r), 2 * (y - r), 2 * (x + r), 2 * (y + r)), fill=col)
    return big.resize(size, Image.LANCZOS)


def draw_blocks(D, W, size, pal, px=20):
    """Blocks: ░▒▓█ shading, one character per cell."""
    return draw_ramp(D, W, size, pal, chars=" ░░▒▒▓▓█", px=px, lift=1.0)


def draw_led(D, W, size, pal, pitch=12):
    """LED matrix: every light on the board, lit or dark."""
    cols, rows = size[0] // pitch, size[1] // pitch
    d, w = cells(D, cols, rows), cells(W, cols, rows)
    big = Image.new("RGB", (size[0] * 2, size[1] * 2), rgb(pal["ground"]))
    dr = ImageDraw.Draw(big)
    dark = tuple(int(c) for c in mix(hexc(pal["ground"]), hexc(pal["ink"][0]), 0.75))
    ox = (size[0] - cols * pitch) // 2
    for y in range(rows):
        for x in range(cols):
            v = d[y, x]
            cx_, cy_ = 2 * (ox + x * pitch + pitch / 2), 2 * (y * pitch + pitch / 2)
            lit = v > B8[y % 8, x % 8] * 0.5 + 0.12
            r = 2 * pitch * (0.36 if lit else 0.30)
            col = ascii_colour(pal, 0.25 + v * 0.9, w[y, x] > 0.4) if lit else dark
            dr.ellipse((cx_ - r, cy_ - r, cx_ + r, cy_ + r), fill=col)
    return big.resize(size, Image.LANCZOS)


def draw_contours(D, W, size, pal, px=16, levels=16, Z=None):
    """Lines: a trail map's contour lines in box-drawing strokes, the trail in ember."""
    f = font(px)
    cw, ch = int(f.getlength("M")), int(px * 1.2)
    cols, rows = size[0] // cw, size[1] // ch
    z = cells(Z, cols, rows)
    p = cells(D, cols, rows)
    w = cells(W, cols, rows)
    q = np.floor(z * levels)
    img = Image.new("RGB", size, rgb(pal["ground"]))
    dr = ImageDraw.Draw(img)
    ox = (size[0] - cols * cw) // 2
    for r in range(1, rows - 1):
        for c in range(1, cols - 1):
            if p[r, c] < 0.05:
                continue
            at = (ox + c * cw, r * ch)
            if w[r, c] > 0.18:
                dr.text(at, "•" if (r + c) % 3 else "·", font=f, fill=ascii_colour(pal, 0.75 + 0.25 * p[r, c], True))
                continue
            if q[r, c] == q[r, c + 1] and q[r, c] == q[r + 1, c]:
                continue
            gx = z[r, c + 1] - z[r, c - 1]
            gy = z[r + 1, c] - z[r - 1, c]
            a = math.degrees(math.atan2(gy, gx)) % 180
            g = "│" if a < 22 or a >= 158 else "╱" if a < 68 else "─" if a < 112 else "╲"
            major = int(q[r, c]) % 4 == 0
            dr.text(at, g, font=f, fill=ascii_colour(pal, (0.95 if major else 0.55) * p[r, c] + 0.05, False))
    return img


def fbm(w, h, seed, octaves=6, base=4):
    rnd = np.random.default_rng(seed)
    out = np.zeros((h, w))
    amp = 1.0
    for o in range(octaves):
        n = base * 2 ** o
        g = rnd.random((n + 1, n + 1))
        out += amp * np.asarray(Image.fromarray((g * 255).astype(np.uint8)).resize((w, h), Image.BICUBIC), dtype=np.float64) / 255
        amp *= 0.5
    return (out - out.min()) / (out.max() - out.min())


def topo(w, h, seed=11):
    """A mountain for a trail map: a summit, its spurs and some ground roughness; the trail
    climbs it in switchbacks."""
    ys, xs = np.mgrid[0:h, 0:w] / np.array([h, w])[:, None, None]
    ax = w / h
    z = 1.0 * np.exp(-(((xs - 0.55) * ax) ** 2 / 0.05 + (ys - 0.56) ** 2 / 0.03))
    z += 0.55 * np.exp(-(((xs - 0.2) * ax) ** 2 / 0.03 + (ys - 0.72) ** 2 / 0.02))
    z += 0.45 * np.exp(-(((xs - 0.85) * ax) ** 2 / 0.02 + (ys - 0.80) ** 2 / 0.03))
    n = fbm(w, h, seed)
    z += 0.30 * n + 0.18 * (1 - np.abs(2 * fbm(w, h, seed + 1) - 1))
    z = (z - z.min()) / (z.max() - z.min())
    trail = np.zeros((h, w))
    pts = [(0.30, 0.99), (0.70, 0.93), (0.38, 0.86), (0.66, 0.80), (0.45, 0.73), (0.60, 0.67), (0.53, 0.60)]
    tr = Image.new("L", (w, h), 0)
    ImageDraw.Draw(tr).line([(x * w, y * h) for x, y in pts], fill=255, width=max(2, w // 80), joint="curve")
    trail = np.asarray(tr, dtype=np.float64) / 255
    P = smooth(0.34, 0.58, ys)
    return z, trail, P


def ridge_profile(x, seed, base, amp, octaves=6):
    r = random.Random(seed)
    y = np.full_like(x, base, dtype=np.float64)
    for k in range(1, octaves + 1):
        f, p = 2.2 ** k, r.random() * 6.28
        y += amp / (k ** 1.25) * np.sin(x * f + p)
    return y


def ridge_line(w, seed, base, amp, rough=1.0, peaks=()):
    """A ridge's height (fraction of the field, from the top) across `w` columns: rolling noise,
    plus sharp `peaks` given as (x, height, width)."""
    x = np.linspace(0, 1, w)
    y = ridge_profile(x * 2.2, seed, base, amp * 0.5 * rough)
    for px_, ph, pw in peaks:
        y = np.minimum(y, base - ph * np.clip(1 - np.abs(x - px_) / pw, 0, 1) ** 1.25 + 0.012 * np.sin(x * 90 + seed))
    return y


def scene(kind, w, h, seed=1):
    """A tall mountain scene for the sidebar, as (night, paper) pairs of (density, warm).

    `kind`: peak, ranges, lake, forest, trail. Everything sits in the bottom half or so; above it
    the ground stays clear for the thread list."""
    ys = (np.arange(h)[:, None] / h) * np.ones((1, w))
    xs = (np.arange(w)[None, :] / w) * np.ones((h, 1))
    rnd = np.random.default_rng(seed)
    glow = np.zeros((h, w))
    warm = np.zeros((h, w))
    mass = np.zeros((h, w))
    haze = np.zeros((h, w))
    sun = None
    if kind == "peak":
        layers = [
            (ridge_line(w, seed, 0.60, 0.02, peaks=[(0.58, 0.20, 0.42)]), 0.2, True),
            (ridge_line(w, seed + 1, 0.74, 0.05), 0.55, False),
            (ridge_line(w, seed + 2, 0.86, 0.04), 0.9, False),
        ]
        sun = (0.22, 0.36, 0.022)
    elif kind == "ranges":
        layers = [(ridge_line(w, seed + k, 0.56 + k * 0.075, 0.07 - k * 0.006, peaks=[(rnd.uniform(0.2, 0.8), 0.05, 0.3)]), k / 5, False) for k in range(6)]
        sun = (0.62, 0.55, 0.06)
    elif kind == "lake":
        layers = [
            (ridge_line(w, seed, 0.62, 0.02, peaks=[(0.40, 0.16, 0.38), (0.82, 0.09, 0.25)]), 0.25, True),
            (ridge_line(w, seed + 3, 0.70, 0.03), 0.5, False),
        ]
        sun = (0.66, 0.44, 0.03)
    elif kind == "forest":
        layers = [
            (ridge_line(w, seed, 0.62, 0.03, peaks=[(0.65, 0.12, 0.45)]), 0.15, True),
            (ridge_line(w, seed + 1, 0.74, 0.03), 0.45, False),
        ]
        sun = (0.30, 0.56, 0.05)
    else:  # trail
        layers = [
            (ridge_line(w, seed, 0.58, 0.02, peaks=[(0.48, 0.14, 0.5)]), 0.3, True),
            (ridge_line(w, seed + 1, 0.80, 0.04), 0.75, False),
        ]
    # sky: a band of dawn light low down, stars high up (night only)
    horizon = float(np.median(layers[0][0]))
    dawn = np.exp(-((ys - horizon) / 0.06) ** 2) * (ys < horizon + 0.05)
    haze += dawn * 0.22
    warm += dawn * 0.9
    stars = np.zeros((h, w))
    for _ in range(int(w * h / 1400)):
        sx, sy = rnd.integers(0, w), int(rnd.uniform(0.18, 0.55) * h)
        stars[sy, sx] = rnd.uniform(0.25, 0.7) * smooth(0.18, 0.4, sy / h)
    if sun:
        sx_, sy_, sr = sun
        d = np.hypot((xs - sx_) * w / h, ys - sy_)
        disc = (d < sr * 0.45).astype(float)
        glow = np.maximum(glow, disc)
        haze += np.exp(-(d / (sr * 1.8)) ** 2) * 0.45
        warm = np.maximum(warm, (d < sr * 1.5) * 1.0)
    # the ridges, far to near; each later one covers what's behind
    for top, depth, lit in layers:
        under = ys >= top[None, :]
        far = 1 - depth
        mass = np.where(under, 0.35 + 0.65 * depth, mass)
        haze = np.where(under, 0.07 + 0.22 * far * far, haze)
        glow = np.where(under, 0, glow)
        warm = np.where(under, 0, warm)
        rim = np.exp(-((ys - top[None, :]) / 0.009) ** 2) * (0.55 + 0.4 * far) * (ys > top[None, :] - 0.004)
        glow = np.maximum(glow, rim)
        warm = np.maximum(warm, rim * (0.9 if depth < 0.5 else 0.0))
        if lit:
            # the sunlit face: left of the summit, fading down the slope
            peak_x = xs[0][np.argmin(top)]
            face = under * (xs < peak_x) * np.exp(-((ys - top[None, :]) / 0.16)) * smooth(-0.35, 0.0, xs - peak_x)
            ribs = 0.65 + 0.35 * np.sin((xs * 40 + ys * 70))
            glow = np.maximum(glow, face * 0.85 * ribs)
            warm = np.maximum(warm, (face > 0.15) * 1.0)
    if kind == "lake":
        water = ys > 0.76
        mirror = np.clip(2 * 0.76 - ys, 0, 1)
        iy = np.clip((mirror * h).astype(int), 0, h - 1)
        ix = np.arange(w)[None, :].repeat(h, 0)
        ripple = 0.55 + 0.45 * (np.sin(ys * 600 + np.sin(xs * 9) * 3) > 0)
        glow = np.where(water, glow[iy, ix] * 0.6 * ripple, glow)
        haze = np.where(water, haze[iy, ix] * 0.7, haze)
        warm = np.where(water, warm[iy, ix], warm)
        mass = np.where(water, mass[iy, ix] * 0.45, mass)
    if kind == "forest":
        for k in range(26):
            tx, base = rnd.uniform(0, 1), rnd.uniform(0.80, 1.0)
            th = rnd.uniform(0.10, 0.22) * (base - 0.6) / 0.4 + 0.04
            tw = th * rnd.uniform(0.16, 0.22) * h / w
            tree = (ys < base) & (ys > base - th) & (np.abs(xs - tx) < tw * (ys - (base - th)) / th * (0.75 + 0.25 * np.sin(ys * 300)))
            mass = np.where(tree, 1.0, mass)
            glow = np.where(tree, 0, glow)
            haze = np.where(tree, 0.0, haze)
            warm = np.where(tree, 0, warm)
        mist = np.exp(-((ys - 0.80) / 0.035) ** 2) * (0.6 + 0.4 * np.sin(xs * 13))
        haze = np.maximum(haze, mist * 0.4 * (mass < 1))
    if kind == "trail":
        pts = [(0.25, 1.0), (0.78, 0.94), (0.30, 0.87), (0.70, 0.80), (0.40, 0.73), (0.58, 0.67), (0.49, 0.61)]
        tr = Image.new("L", (w, h), 0)
        ImageDraw.Draw(tr).line([(x * w, y * h) for x, y in pts], fill=255, width=max(2, w // 70), joint="curve")
        t = np.asarray(tr, dtype=np.float64) / 255
        glow = np.maximum(glow, t)
        warm = np.maximum(warm, t)
    clear = smooth(0.30, 0.52, ys)  # the top stays clear for the thread list
    night = np.clip(np.maximum(glow, haze) + stars, 0, 1) * clear
    paper = np.clip(mass * 0.75 + haze * 0.35 - glow * 0.25 + (glow > 0.6) * warm * 0.7, 0, 1) * clear
    return (night, warm * clear), (paper, warm * clear * (paper > 0.05))


def ascii_pieces(only):
    os.makedirs(os.path.join(OUT, "ascii"), exist_ok=True)
    w, h = TALL
    hw, hh = w // 2, h // 2
    pieces = {
        "ramp": ("peak", lambda *a: draw_ramp(*a, cut=0.52), 3),
        "braille": ("ranges", draw_braille, 5),
        "halftone": ("lake", draw_halftone, 7),
        "blocks": ("forest", draw_blocks, 9),
        "led": ("trail", draw_led, 4),
    }
    for name, (kind, draw, seed) in pieces.items():
        if only and name not in only:
            continue
        if kind:
            night, paper = scene(kind, hw, hh, seed)
        for variant, pal in (("night", ASCII_NIGHT), ("paper", ASCII_PAPER)):
            if kind is None:
                Z, trail, P = topo(hw, hh, seed)
                img = draw_contours(P, trail, TALL, pal, Z=Z)
            else:
                D, W = night if variant == "night" else paper
                img = draw(D, W, TALL, pal)
            save(img, "ascii", name, variant, png=True)


# ---------------------------------------------------------------- Light (chat area)
LIGHT_GLYPHS = " .:+x*X8S#$0"


def glyph_light(F, size, cell_px=18, threshold=0.22, glow_gain=0.40, seed=5):
    """`F` an HDR light field (h, w, 3). Where it's bright, the light is drawn as glyphs on a grid;
    around them it glows, smooth."""
    w, h = size
    rnd = random.Random(seed)
    Fimg = Image.fromarray(np.clip(F / F.max() * 255, 0, 255).astype(np.uint8))
    scale = F.max()
    blur = np.asarray(Fimg.resize(size, Image.BILINEAR).filter(ImageFilter.GaussianBlur(w / 90)), dtype=np.float64) / 255 * scale
    sharp = np.asarray(Fimg.resize(size, Image.BILINEAR), dtype=np.float64) / 255 * scale
    glow = blur * glow_gain + sharp * 0.10
    f = font(cell_px, bold=True)
    cw, ch = int(f.getlength("M")) + 1, int(cell_px * 1.05)
    cols, rows = w // cw, h // ch
    cellF = np.asarray(Image.fromarray(np.clip(F / scale * 255, 0, 255).astype(np.uint8)).resize((cols, rows), Image.BOX), dtype=np.float64) / 255 * scale
    mask = Image.new("L", size, 0)
    colour = np.zeros((h, w, 3))
    md = ImageDraw.Draw(mask)
    for r in range(rows):
        for c in range(cols):
            v = luma(cellF[r, c])
            if v < threshold:
                continue
            t = min(1, (v - threshold) / (1.1 - threshold))
            k = int(t * (len(LIGHT_GLYPHS) - 1))
            if rnd.random() < 0.08:
                k = max(1, k - 2)
            md.text((c * cw, r * ch), LIGHT_GLYPHS[k], font=f, fill=int(110 + 145 * min(1, t * 1.4)))
    m = np.asarray(mask, dtype=np.float64)[..., None] / 255
    hot = sharp * 2.2 + 0.12
    out = glow * (1 - m * 0.7) + hot * m
    return out


def tonemap_night(out, cap):
    """Film-like roll-off per channel (hot light goes to white, keeping its hue at the edges),
    then held under `cap` luma (the brightest the transcript's scrim allows for)."""
    v = 1 - np.exp(-out * 1.35)
    ground = hexc("#0A0A0B") / 255
    v = ground + v * (1 - ground)
    lum = luma(v)[..., None]
    v = np.where(lum > cap, ground + (v - ground) * (cap - luma(ground)) / np.maximum(lum - luma(ground), 1e-6), v)
    return Image.fromarray(np.clip(v * 255, 0, 255).astype(np.uint8))


def tonemap_paper(out):
    """The same light as ink on cream: where it shines, the paper takes ember and grey."""
    v = 1 - np.exp(-out * 1.15)
    cream = hexc("#F3EEE6") / 255
    ember = hexc("#E85D1F") / 255
    ink = hexc("#7A6E63") / 255
    lum = luma(v)[..., None]
    sat = np.clip((v.max(axis=-1) - v.min(axis=-1))[..., None] * 1.6, 0, 1)
    tint = ink + (ember - ink) * sat
    res = cream + (tint - cream) * np.clip(lum * 1.6, 0, 0.9)
    return Image.fromarray(np.clip(res * 255, 0, 255).astype(np.uint8))


def field_streaks(w, h, seed=2):
    """Light streaks across the frame, as a long exposure of lights going by."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float64)
    rnd = np.random.default_rng(seed)
    ang = math.radians(-34)
    u = (xs - w * 0.5) * math.cos(ang) + (ys - h * 0.5) * math.sin(ang)  # along
    v = -(xs - w * 0.5) * math.sin(ang) + (ys - h * 0.5) * math.cos(ang)  # across
    F = np.zeros((h, w, 3))
    cols = [hexc("#FF7A3D"), hexc("#FF4A1C"), hexc("#FFB27A"), hexc("#3A7BD5"), hexc("#2C5F9E"), hexc("#E8642B"), hexc("#FFE6CC")]
    for i in range(70):
        off = rnd.normal(0, h * 0.20)
        width = rnd.uniform(1.5, 14) * (2.6 if abs(off) < h * 0.06 else 1)
        c = cols[rnd.integers(0, len(cols))] / 255
        if abs(off) < h * 0.08:
            c = (cols[rnd.integers(0, 3)]) / 255 * 0.8
        amp = rnd.uniform(0.2, 1.0) * np.exp(-(off / (h * 0.28)) ** 2)
        start, length = rnd.uniform(-w * 0.9, w * 0.2), rnd.uniform(w * 0.5, w * 1.6)
        along = smooth(start, start + w * 0.15, u) * (1 - smooth(start + length - w * 0.2, start + length, u))
        F += amp * np.exp(-((v - off) / width) ** 2)[..., None] * along[..., None] * c
    core = np.exp(-(v / (h * 0.07)) ** 2) * smooth(-w * 0.7, w * 0.1, u)
    F += (core * 0.6)[..., None] * (hexc("#FFB27A") / 255)
    return F


def field_aurora(w, h, seed=4):
    """Curtains of light hanging over a dark ridge: bright along their lower hem, rayed, fading up."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float64) / np.array([h, w])[:, None, None]
    rnd = np.random.default_rng(seed)
    F = np.zeros((h, w, 3))
    x = xs[0]
    for k, (hem_c, top_c, amp) in enumerate([("#FF8A4A", "#B0306A", 1.0), ("#FFC27A", "#E8642B", 0.75), ("#9AD0C8", "#3E6E9E", 0.45)]):
        hem = 0.50 + 0.05 * k + 0.07 * np.sin(x * (4.2 + k) + rnd.uniform(0, 6)) + 0.025 * np.sin(x * 17 + rnd.uniform(0, 6))
        height = 0.30 + 0.10 * np.sin(x * 3 + rnd.uniform(0, 6)) ** 2
        rays = 0.45 + 0.55 * np.clip(sum(np.sin(x * f + rnd.uniform(0, 6)) for f in (90, 157, 233)) / 3 + 0.5, 0, 1) ** 2
        above = np.clip(hem[None, :] - ys, 0, None)
        body = np.exp(-above / (height[None, :] * 0.45)) * (ys < hem[None, :] + 0.004)
        edge = np.exp(-((ys - hem[None, :]) / 0.012) ** 2)
        t = np.clip(above / height[None, :], 0, 1)[..., None]
        col = (hexc(hem_c) + (hexc(top_c) - hexc(hem_c)) * t) / 255
        F += ((body * 0.75 + edge * 0.8) * rays[None, :] * amp)[..., None] * col
    ridge = ridge_profile(x * 3, seed, 0.80, 0.06)
    F *= (ys < ridge[None, :])[..., None]
    rim = np.exp(-((ys - ridge[None, :]) / 0.004) ** 2) * (ys < ridge[None, :] + 0.002)
    F += (rim * 0.3)[..., None] * (hexc("#FFB27A") / 255)
    # its reflection in a still lake below the ridge
    lake = ys > 0.88
    iy = np.clip(((2 * 0.88 - ys) * h).astype(int), 0, h - 1)
    F = np.where(lake[..., None], F[iy, np.arange(w)[None, :].repeat(h, 0)] * 0.35, F)
    return F


def field_window(w, h, seed=6):
    """A window's light thrown across a dark room, through a blind, and a lamp."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float64)
    F = np.zeros((h, w, 3))
    # the patch on the wall: a skewed quad, with mullions and slats
    u = (xs - w * 0.18) / (w * 0.42) - (ys - h * 0.2) / h * 0.25
    v = (ys - h * 0.14) / (h * 0.62) + (xs - w * 0.18) / w * 0.18
    inside = smooth(-0.02, 0.02, u) * (1 - smooth(0.98, 1.02, u)) * smooth(-0.02, 0.02, v) * (1 - smooth(0.98, 1.02, v))
    mullion = 1 - np.exp(-((u - 0.5) / 0.012) ** 2) - np.exp(-((v - 0.48) / 0.012) ** 2)
    slats = 0.55 + 0.45 * (np.sin(v * math.pi * 26) > -0.35)
    falloff = 0.55 + 0.45 * np.exp(-((u - 0.62) ** 2 + (v - 0.45) ** 2) / 0.18)
    patch = np.clip(inside * mullion * slats * falloff, 0, None)
    F += patch[..., None] * (hexc("#9EC2E8") / 255) * 1.6
    F += (patch * np.exp(-((u - 0.7) ** 2) / 0.05))[..., None] * (hexc("#FFF1E0") / 255) * 0.6
    # the lamp, to the right, warm
    lx, ly = w * 0.78, h * 0.55
    d = np.hypot(xs - lx, (ys - ly) * 1.2) / w
    shade = (np.abs(xs - lx) < (w * 0.035 + (ys - (ly - h * 0.09)) * 0.35)) & (ys > ly - h * 0.09) & (ys < ly)
    F += (shade * 1.4)[..., None] * (hexc("#FFB27A") / 255)
    F += (np.exp(-d / 0.05) * 0.9)[..., None] * (hexc("#FF7A3D") / 255)
    F += (np.exp(-d / 0.18) * 0.18)[..., None] * (hexc("#FF7A3D") / 255)
    # light pooling on the sill/table
    F += (np.exp(-(((xs - w * 0.62) / (w * 0.25)) ** 2 + ((ys - h * 0.86) / (h * 0.03)) ** 2)) * 0.35)[..., None] * (hexc("#7FA4C8") / 255)
    return F


def field_switchbacks(w, h, seed=8):
    """A long exposure of headlamps on a switchback trail up a dark mountain, under stars."""
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float64)
    rnd = np.random.default_rng(seed)
    F = np.zeros((h, w, 3))
    ridge = ridge_profile(xs[0] / w * 3, seed, 0.30, 0.07) * h
    sky = ys < ridge[None, :]
    F += (sky * np.exp(-((ys - ridge[None, :]) / (h * 0.08)) ** 2) * 0.10)[..., None] * (hexc("#3E6E9E") / 255)
    for _ in range(500):
        sx, sy = rnd.integers(0, w), rnd.integers(0, h)
        if sy < ridge[sx] - 4:
            F[sy, sx] += rnd.uniform(0.3, 1.0) * hexc("#E6ECF5") / 255
    # the trail: zigzags from the bottom left to the col
    pts = [(0.06, 1.02), (0.64, 0.90), (0.18, 0.78), (0.58, 0.67), (0.30, 0.57), (0.52, 0.48), (0.42, 0.40), (0.47, 0.33)]
    trail = np.zeros((h, w))
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        n = int(math.hypot((x1 - x0) * w, (y1 - y0) * h))
        for t in np.linspace(0, 1, n):
            px = (x0 + (x1 - x0) * t) * w + math.sin(t * 9 + x0 * 30) * 6
            py = (y0 + (y1 - y0) * t) * h
            ix, iy = int(px), int(py)
            if 0 <= ix < w and 0 <= iy < h:
                trail[iy, ix] = 1.0
    trail_img = Image.fromarray((trail * 255).astype(np.uint8))
    near = np.asarray(trail_img.filter(ImageFilter.GaussianBlur(9)), dtype=np.float64) / 255
    wide = np.asarray(trail_img.filter(ImageFilter.GaussianBlur(40)), dtype=np.float64) / 255
    depth = np.clip((ys / h - 0.3) / 0.7, 0.25, 1)  # nearer the bottom, brighter and wider
    F += (near * 12 * depth)[..., None] * (hexc("#FFD2B0") / 255)
    F += (wide * 10 * depth)[..., None] * (hexc("#FF7A3D") / 255)
    # the slope catches a little of it
    F += ((~sky) * 0.03)[..., None] * (hexc("#5A585C") / 255)
    return F


LIGHT = {
    "streaks": (field_streaks, dict(threshold=0.26)),
    "aurora": (field_aurora, dict(threshold=0.30)),
    "window": (field_window, dict(threshold=0.28)),
    "switchbacks": (field_switchbacks, dict(threshold=0.30)),
}
# The brightest a Light piece gets on Night, so the transcript's scrim stays light.
LIGHT_CAP = 0.58


def light_pieces(only):
    os.makedirs(os.path.join(OUT, "light"), exist_ok=True)
    w, h = WIDE
    for name, (make, args) in LIGHT.items():
        if only and name not in only:
            continue
        F = make(w // 2, h // 2)
        F = np.asarray(Image.fromarray(np.clip(F / max(F.max(), 1e-6) * 255, 0, 255).astype(np.uint8)).resize(WIDE, Image.BICUBIC), dtype=np.float64) / 255 * F.max()
        F = F / np.percentile(luma(F), 99.7) * 1.1
        out = glyph_light(F, WIDE, **args)
        save(tonemap_night(out, LIGHT_CAP), "light", name, "night", png=False)
        save(tonemap_paper(out), "light", name, "paper", png=False)


# ---------------------------------------------------------------- output
def save(img, collection, name, variant, png):
    base = os.path.join(OUT, collection, f"{name}-{variant}")
    if png:
        q = img.quantize(colors=32, method=Image.MEDIANCUT, dither=Image.NONE)
        q.save(base + ".png", optimize=True)
    else:
        img.save(base + ".jpg", quality=86, optimize=True, progressive=True, subsampling=0)
    thumb = img.convert("RGB")
    # The gallery's tiles: 100x66 pt, or 56x92 for a sidebar piece (its lower part, the scene).
    size, fy = ((112, 184), 0.78) if collection == "ascii" else (THUMB, 0.5)
    thumb = crop_to(thumb, size[0] / size[1], fy=fy).resize(size, Image.LANCZOS)
    thumb.save(base + "-thumb.jpg", quality=86, optimize=True)
    print(collection, name, variant, os.path.getsize(base + (".png" if png else ".jpg")))


if __name__ == "__main__":
    src, *only = sys.argv[1:] or [""]
    which = {a.split(":", 1)[0] for a in only if ":" in a}
    names = [n for n in [a.split(":", 1)[1] for a in only if ":" in a] + [a for a in only if ":" not in a] if n]
    if not which or "paintings" in which:
        paintings(src, names)
    if not which or "ascii" in which:
        ascii_pieces(names)
    if not which or "light" in which:
        light_pieces(names)
