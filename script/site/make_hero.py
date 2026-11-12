"""The hero picture for the site, from one photograph: more sky above it, the sky calmed where the
headline sits, and a dusk grade of the same frame for Night.
make_hero.py PHOTO OUT_DIR"""
import sys
import numpy as np
from PIL import Image, ImageFilter

src, out = sys.argv[1], sys.argv[2]
W, H = 2880, 1800            # the picture as served at 2x; the horizon sits near 70% of its height
SHIFT = 250                  # rows of new sky above the photograph

photo = Image.open(src).convert("RGB")
photo = photo.resize((W, round(photo.height * W / photo.width)), Image.LANCZOS)
a = np.asarray(photo).astype(np.float32)[: H - SHIFT]
rows = a.shape[0]

# The sky without its fine structure: a heavy blur, used to extend it and to calm it.
soft = np.asarray(photo.filter(ImageFilter.GaussianBlur(90))).astype(np.float32)[: H - SHIFT]

# 1. Calm the wisps behind the headline: blend toward the soft sky, fading out well above the hill.
y = np.arange(rows, dtype=np.float32)[:, None, None]
calm = 0.55 * np.clip((760 - y) / 140, 0, 1)          # full above row 620, none below row 760
a = a * (1 - calm) + soft * calm

# 2. New sky above: from the photograph's own top row, softened, to a slightly deeper tone.
top = soft[0:6].mean(axis=0)                             # per column
deep = top.mean(axis=0) + np.array([-9, -8, -5], np.float32)
t = (np.arange(SHIFT, dtype=np.float32) / SHIFT)[:, None, None] ** 1.2
sky = deep[None, None, :] * (1 - t) + top[None, :, :] * t
rng = np.random.default_rng(7)
sky += rng.normal(0, 1.1, sky.shape).astype(np.float32)  # the photograph's grain, so the join does not show
# feather the photograph's first rows into the soft sky so nothing is cut at the join
f = np.clip((90 - y) / 90, 0, 1)
a = a * (1 - f) + soft * f

day = np.clip(np.vstack([sky, a]), 0, 255)
# a little more colour in the day frame: the sky a touch bluer, everything slightly more saturated
grey = day.mean(axis=2, keepdims=True)
day = np.clip(grey + (day - grey) * 1.14, 0, 255) * np.array([0.985, 0.995, 1.02], np.float32)
day = np.clip(day, 0, 255)
Image.fromarray(day.astype(np.uint8)).save(f"{out}/hero-day.png")

# 3. Dusk: the same photograph after sunset, by grading it and never by repainting it. Every pixel
# is the day frame multiplied by a colour that changes with height, so the real sky, its wisps,
# the grass and the trail are all still there.
yy = (np.arange(H, dtype=np.float32) / H)[:, None, None]
xx = (np.arange(W, dtype=np.float32) / W)[None, :, None]
lum = (day * np.array([0.299, 0.587, 0.114], np.float32)).sum(axis=2, keepdims=True) / 255
is_sky = np.clip((lum - 0.62) / 0.14, 0, 1)             # the sky is the bright part of this frame
is_sky = np.asarray(Image.fromarray((is_sky[..., 0] * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(3))).astype(np.float32)[..., None] / 255
# what the sky is multiplied by, from the top of the frame to the horizon
stops = [(0.0, (0.075, 0.105, 0.19)), (0.36, (0.16, 0.165, 0.31)), (0.56, (0.36, 0.25, 0.37)), (0.66, (0.74, 0.43, 0.33)), (0.74, (0.92, 0.55, 0.36))]
def ramp(v):
    c = np.zeros(v.shape[:2] + (3,), np.float32) + np.array(stops[0][1], np.float32)
    for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
        k = np.clip((v - t0) / (t1 - t0), 0, 1)
        c = np.where(v >= t0, np.array(c0, np.float32) * (1 - k) + np.array(c1, np.float32) * k, c)
    return c
# the afterglow is strongest where the sun went down (left of the middle) and sinks toward the edges
sink = 0.05 * np.clip(np.abs(xx - 0.36) / 0.64, 0, 1) ** 1.3
gain = ramp(np.clip(np.broadcast_to(yy, (H, W, 1)) - sink, 0, 1))
# the wisps of cloud keep their place, a little stronger than in the calmed day frame
soft_day = np.asarray(Image.fromarray(day.astype(np.uint8)).filter(ImageFilter.GaussianBlur(30))).astype(np.float32)
night_sky = (soft_day + (day - soft_day) * 1.8) * gain
# the land: the same grass and trail, underexposed and a touch warm, with its contrast kept
land = (np.clip(day / 255, 0, 1) ** 1.12) * 255 * np.array([0.37, 0.30, 0.32], np.float32)
rim = np.clip(1 - (yy - 0.70) / 0.12, 0, 1) * np.clip((yy - 0.60) / 0.08, 0, 1)
land = land * (1 + rim * np.array([0.55, 0.22, 0.06], np.float32))
night = np.clip(night_sky * is_sky + land * (1 - is_sky), 0, 255)
Image.fromarray(night.astype(np.uint8)).save(f"{out}/hero-night.png")
print("day top", day[2, W // 2].round(), "mid", day[700, W // 2].round(), "| night top", night[2, W // 2].round(), "land", night[1600, W // 2].round())
