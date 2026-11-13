"""The first screen's photograph in two planes, so it can move with depth: the sky (with the far
hills) and the land (the near hill, its grass and its trail, with a transparent sky).
make_hero_layers.py HERO_DIR OUT_DIR

HERO_DIR holds hero-day.png and hero-night.png, as make_hero.py writes them. Every pixel of both
planes is the photograph's own: the land is the picture cut along the ridge, and the sky is the
picture with the strip behind the ridge filled from the sky just above it, so a slight shift of
the land never shows a hole. Writes sky-day.png, land-day.png, sky-night.png and land-night.png
to OUT_DIR (the land with alpha); script/site/README.md has the WebP step."""
import sys
import numpy as np
from PIL import Image

src, out = sys.argv[1], sys.argv[2]
day = np.asarray(Image.open(f"{src}/hero-day.png").convert("RGB")).astype(np.float32)
night = np.asarray(Image.open(f"{src}/hero-night.png").convert("RGB")).astype(np.float32)
H, W, _ = day.shape


def box(a, k, axis):
    """A running mean of width 2k+1 along one axis, with the edges held."""
    p = [(0, 0)] * a.ndim
    p[axis] = (k + 1, k)
    c = np.cumsum(np.pad(a, p, mode="edge"), axis=axis, dtype=np.float64)
    n = a.shape[axis]
    hi = np.take(c, np.arange(2 * k + 1, 2 * k + 1 + n), axis=axis)
    lo = np.take(c, np.arange(0, n), axis=axis)
    return ((hi - lo) / (2 * k + 1)).astype(np.float32)


def blur(a, r):
    """Close to a Gaussian blur of radius r (its sigma): three box blurs each way."""
    k = max(1, int(round(np.sqrt(12 * r * r / 3 + 1) - 1) // 2))
    a = a.astype(np.float32)
    for _ in range(1 if r < 1 else 3):
        a = box(box(a, k, 0), k, 1)
    return a


def smooth(v, a, b):
    t = np.clip((v - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


# 1. Where the grass is. The sky (pale lavender and pink) and the far hills (blue) are cool or barely
# warm; the dry grass, lit or in shadow, is warm: red well above blue.
warm = day[..., 0] - day[..., 2]
grass = smooth(warm, 27, 44)

# 2. The ridge in each column: the first row from which the next 48 rows are mostly grass. A median
# across neighbouring columns keeps a stray warm wisp of cloud from making a spike.
ys = np.arange(H, dtype=np.float32)
run = np.cumsum(np.vstack([np.zeros((1, W), np.float32), grass]), axis=0)
mostly = (run[48:] - run[:-48]) / 48 > 0.7                     # (H - 47, W)
ridge = np.where(mostly.any(axis=0), mostly.argmax(axis=0), H).astype(np.float32)
pad = 15
ridge = np.array([np.median(ridge[max(0, x - pad): x + pad + 1]) for x in range(W)], np.float32)
below = ys[:, None] - ridge[None, :]

# 3. The land's alpha: solid a little under the ridge, the grass's own tips along it, nothing above.
alpha = np.where(below > 22, 1.0, np.where(below > -40, grass, 0.0)).astype(np.float32)
alpha = np.clip(blur(alpha, 0.7), 0, 1)
alpha[below > 26] = 1.0

# 4. The sky behind the ridge: near it, a normalised blur of the sky around it; further down, each
# column's sky colour just above the ridge, carried straight down and softened sideways.
def sky_plane(img):
    keep = (1 - smooth(alpha, 0.02, 0.25)).astype(np.float32)
    keep[below > -6] = 0
    den = blur(keep, 40)
    near = blur(img * keep[..., None], 40) / np.maximum(den, 1e-3)[..., None]
    at = np.clip(ridge - 10, 0, H - 1).astype(int)
    edge = near[at, np.arange(W)]                              # (W, 3): the sky just above the ridge
    edge = blur(edge[None, :, :], 24)[0]
    down = np.where((ys[:, None] > ridge[None, :] - 10)[..., None], edge[None, :, :], near)
    trust = np.clip((den - 0.02) * 8, 0, 1)[..., None]
    fill = near * trust + down * (1 - trust)
    sky = img * keep[..., None] + fill * (1 - keep[..., None])
    rng = np.random.default_rng(11)
    sky += rng.normal(0, 1.0, sky.shape).astype(np.float32) * (1 - keep[..., None])  # the photograph's grain
    return np.clip(sky, 0, 255)


# 5. The land's colour along the ridge, with the sky's share taken out, so a tip of grass does not
# carry a rim of sky when the land moves over a different part of it.
def land_plane(img, sky):
    a = np.maximum(alpha, 1e-3)[..., None]
    pure = (img - sky * (1 - alpha[..., None])) / a
    edge = (alpha > 0.02) & (alpha < 0.98)
    col = np.where(edge[..., None], np.clip(pure, 0, 255), img)
    return np.dstack([col, alpha * 255])


for name, img in (("day", day), ("night", night)):
    sky = sky_plane(img)
    Image.fromarray(sky.astype(np.uint8)).save(f"{out}/sky-{name}.png")
    Image.fromarray(np.clip(land_plane(img, sky), 0, 255).astype(np.uint8), "RGBA").save(f"{out}/land-{name}.png")

crest = int(np.argmin(ridge[int(W * 0.3): int(W * 0.55)])) + int(W * 0.3)
print("ridge rows: left", int(ridge[40]), "trail", int(ridge[int(W * 0.42)]), "right", int(ridge[W - 40]),
      "| highest point between 30% and 55% across:", crest, int(ridge[crest]))
