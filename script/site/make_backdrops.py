"""The home page's backdrop, one afternoon passing as you read down: the first screen stands on a
grass hill in daylight (Night: blue hour), and the page closes at dusk down in the grass (Night:
later, full night). Each photograph is framed wide for a desktop and tall for a phone, graded for
Paper and Night by multiplying (never repainting), and encoded for the page (AVIF, WebP and a JPEG
fallback, at the widths the page asks for).

make_backdrops.py hill PHOTO OUT_DIR    the hill: {wide,tall}-{day,night}.png masters
make_backdrops.py dusk PHOTO OUT_DIR    the dusk grass: {wide,tall}-{dusk,night}.png masters
make_backdrops.py encode MASTER_DIR SITE_IMG_DIR NAME
                                        NAME-{wide,tall}-<grade>-<width>.{avif,webp,jpg}

The photographs (Unsplash License; download the originals, the site never serves them):
  hill  Chris Mongeau, "Dried grass field under blue sky during daytime",
        https://unsplash.com/photos/ZRdSZeVKBaE (the November hero's photograph)
  dusk  Johannes Plenio, "The sun is setting over a field of tall grass",
        https://unsplash.com/photos/X7RUQSjQv0k
The hill's footpath is grassed over near the crest from the photograph's own grass beside it
(hide_path), so no trail shows above the app's window.

Run with a Python that has Pillow (with AVIF) and numpy:
  uv run --with pillow --with numpy python -I script/site/make_backdrops.py ...
"""
import sys
import numpy as np
from PIL import Image, ImageFilter

FRAMES = {"wide": (2560, 1600), "tall": (1080, 1920)}
# Where the horizon sits in each frame: just under the line below the buttons, where the page
# places the top of the app's window, so land shows between the words and the window.
HORIZON = {"wide": 0.55, "tall": 0.57}
WIDTHS = {"wide": (1280, 1920, 2560), "tall": (720, 1080)}
LUMA = np.array([0.299, 0.587, 0.114], np.float32)


def arr(im):
    return np.asarray(im.convert("RGB")).astype(np.float32)


def img(a):
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))


def blur(a, r):
    return arr(img(a).filter(ImageFilter.GaussianBlur(r)))


def ramp(v, stops):
    """Colour stops along v (0..1), per pixel."""
    c = np.zeros(v.shape[:2] + (3,), np.float32) + np.array(stops[0][1], np.float32)
    for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
        k = np.clip((v - t0) / (t1 - t0), 0, 1)
        c = np.where(v >= t0, np.array(c0, np.float32) * (1 - k) + np.array(c1, np.float32) * k, c)
    return c


def frame(photo, w, h, horizon_src, horizon_dst, cx=0.5):
    """Scale and crop the photograph to w x h so its horizon (a fraction of its height) lands at
    horizon_dst of the frame; where that needs more sky than the photograph has, the sky is
    extended from its own softened top rows with the photograph's grain."""
    pw, ph = photo.size
    s = max(w / pw, 1.0 * h / ph)
    s = max(s, w / pw)
    for _ in range(3):  # scale up until the land below the horizon fills the frame
        if (1 - horizon_src) * ph * s >= (1 - horizon_dst) * h:
            break
        s = (1 - horizon_dst) * h / ((1 - horizon_src) * ph)
    p = photo.resize((round(pw * s), round(ph * s)), Image.LANCZOS)
    a = arr(p)
    x0 = int(np.clip(cx * a.shape[1] - w / 2, 0, a.shape[1] - w))
    top = round(horizon_src * a.shape[0] - horizon_dst * h)
    if top >= 0:
        return a[top : top + h, x0 : x0 + w]
    # extend the sky upward
    need = -top
    body = a[: h - need, x0 : x0 + w]
    soft = blur(body, 60)
    row = soft[0:8].mean(axis=0)
    deep = row.mean(axis=0) + np.array([-8, -7, -3], np.float32)
    t = (np.arange(need, dtype=np.float32) / max(need, 1))[:, None, None] ** 1.3
    sky = deep[None, None, :] * (1 - t) + row[None, :, :] * t
    sky += np.random.default_rng(7).normal(0, 1.2, sky.shape).astype(np.float32)
    y = np.arange(body.shape[0], dtype=np.float32)[:, None, None]
    f = np.clip((80 - y) / 80, 0, 1)
    body = body * (1 - f) + soft * f
    return np.vstack([sky, body])


def calm_sky(a, until, fade=160, amount=0.85, r=40):
    """Soften the sky's fine marks (contrails, wisps) above row `until`, where the headline sits."""
    soft = blur(a, r)
    y = np.arange(a.shape[0], dtype=np.float32)[:, None, None]
    k = amount * np.clip((until - y) / fade, 0, 1)
    return a * (1 - k) + soft * k


# ---------- The hill (the first screen) and the dusk grass (the end of the page) ----------

def hide_path(a):
    """Grass over the top of the photograph's footpath, where it can show above the app's window
    (a trail beside the signpost reads as a hiking app). The grass is the photograph's own, taken
    from beside the path at the same height, so its light and grain match; the lower path stays
    and is always behind the window."""
    h, w, _ = a.shape
    y = np.arange(h, dtype=np.float32)[:, None] / h
    x = np.arange(w, dtype=np.float32)[None, :] / w
    # the path's centre line, from the crest down (fractions of the photograph)
    cx = np.interp(y, [0.47, 0.55, 0.62, 0.70, 0.78], [0.412, 0.468, 0.515, 0.545, 0.56])
    half = np.interp(y, [0.47, 0.6, 0.78], [0.02, 0.035, 0.05])
    m = np.clip((half - np.abs(x - cx)) / 0.012, 0, 1) * np.clip((y - 0.455) / 0.01, 0, 1) * np.clip((0.80 - y) / 0.04, 0, 1)
    shift = int(0.13 * w)
    src = np.roll(a, -shift, axis=1)  # the grass 13% to the right
    m = blur(np.repeat(m[..., None] * 255, 3, axis=2), 6)[..., :1] / 255
    return a * (1 - m) + src * m


def hill(src, out):
    """The November hero's photograph, framed from the original so the grass is never enlarged.
    Day: the frame as shot, a little richer. Night: blue hour, not a filter: the sky deepens to navy
    with one thin warm line on the crest, and the grass loses its colour to a dark blue-grey."""
    photo = img(hide_path(arr(Image.open(src))))
    CREST = 0.46  # the hill's crest at the middle of the photograph (it falls away to the left)
    for name, (w, h) in FRAMES.items():
        hd = HORIZON[name] + (0.05 if name == "tall" else 0.03)  # the warm line stays below the words
        a = frame(photo, w, h, CREST, hd, cx=0.5)
        a = calm_sky(a, until=hd * h - 0.1 * h, fade=0.12 * h, amount=0.6, r=60)
        grey = a.mean(axis=2, keepdims=True)
        day = np.clip(grey + (a - grey) * 1.12, 0, 255) * np.array([0.985, 0.995, 1.02], np.float32)
        img(day).save(f"{out}/{name}-day.png")
        yy = (np.arange(h, dtype=np.float32) / h)[:, None, None]
        lum = (a * LUMA).sum(axis=2, keepdims=True) / 255
        # the sky: bright, and not far below the crest (the hill falls away to the left)
        is_sky = np.clip((lum - 0.60) / 0.12, 0, 1) * np.clip((hd + 0.10 - yy) / 0.03, 0, 1)
        is_sky = blur(np.repeat(is_sky * 255, 3, axis=2), 3)[..., :1] / 255
        gain = ramp(np.broadcast_to(yy / hd, (h, w, 1)), [
            (0.0, (0.075, 0.09, 0.18)), (0.6, (0.12, 0.14, 0.27)), (0.88, (0.22, 0.22, 0.38)),
            (0.97, (0.36, 0.29, 0.40)), (1.0, (0.62, 0.42, 0.40))])
        sky_n = a * gain
        grass = grey + (a - grey) * 0.3                      # most of the ochre gone
        land_n = (np.clip(grass / 255, 0, 1) ** 1.15) * 255 * np.array([0.24, 0.27, 0.37], np.float32)
        rim = np.clip(1 - (yy - hd) / 0.025, 0, 1) * np.clip((yy - hd + 0.005) / 0.01, 0, 1)
        land_n = land_n * (1 + rim[..., :1] * np.array([0.7, 0.35, 0.15], np.float32))
        night = sky_n * is_sky + land_n * (1 - is_sky)
        night += np.random.default_rng(13).normal(0, 1.4, night.shape).astype(np.float32)
        img(night).save(f"{out}/{name}-night.png")
    print("hill ok")


def dusk(src, out):
    """The end of the same afternoon, closer to the ground: grass against the sky after sunset,
    where the page closes and the footer stands. Graded twice by multiplying, never repainting:
    Paper keeps the dusk (the photograph's green-grey upper sky turned to the page's dusty mauve, the
    apricot band kept), Night is later (navy sky, a low ember band, the grass gone black)."""
    photo = Image.open(src).convert("RGB")
    HSRC = 0.85  # the far field's edge
    for name, (w, h) in FRAMES.items():
        hd = 0.80 if name == "wide" else 0.70  # a phone's footer is taller: more ground under it
        a = frame(photo, w, h, HSRC, hd, cx=0.42)
        yy = (np.arange(h, dtype=np.float32) / h)[:, None, None]
        lum = (a * LUMA).sum(axis=2, keepdims=True) / 255
        is_sky = np.clip((lum - 0.18) / 0.10, 0, 1)  # everything bright enough is sky; the grass is near black
        t = np.broadcast_to(yy / hd, (h, w, 1))
        # Paper: graded toward the hill's own soft light so the two read as one afternoon: much less
        # saturated than the photograph, rose and mauve above, the apricot kept only low and quiet
        g_dusk = ramp(t, [(0.0, (1.04, 0.88, 1.08)), (0.45, (1.02, 0.88, 0.98)), (0.8, (1.0, 0.9, 0.9)), (1.0, (0.98, 0.9, 0.88))])
        lift = ramp(t, [(0.0, (40, 26, 46)), (0.6, (18, 10, 16)), (1.0, (6, 2, 4))])
        dusk_ = (a * g_dusk + lift) * is_sky + a * 0.9 * (1 - is_sky)
        grey = dusk_.mean(axis=2, keepdims=True)
        dusk_ = grey + (dusk_ - grey) * 0.56
        dusk_ += np.random.default_rng(17).normal(0, 1.2, dusk_.shape).astype(np.float32)
        img(dusk_).save(f"{out}/{name}-dusk.png")
        # Night: later than the top of the page, so darker and cooler: a navy sky that is dimmer than
        # the hill's blue hour, one thin ember line on the far field, the grass gone black
        g_night = ramp(t, [(0.0, (0.07, 0.08, 0.17)), (0.55, (0.09, 0.09, 0.20)), (0.88, (0.15, 0.12, 0.22)), (0.97, (0.30, 0.18, 0.20)), (1.0, (0.44, 0.24, 0.20))])
        night = a * g_night * is_sky + a * 0.35 * np.array([0.8, 0.85, 1.1], np.float32) * (1 - is_sky)
        grey = night.mean(axis=2, keepdims=True)
        night = grey + (night - grey) * 0.7
        night += np.random.default_rng(19).normal(0, 1.4, night.shape).astype(np.float32)
        img(night).save(f"{out}/{name}-night.png")
    print("dusk ok")


# ---------- Encoding ----------

AVIF_Q = {"hill": 50, "dusk": 64}  # the dusk's smooth sky needs more bits to keep its grain; grass hides its own


def encode(src, dst, name):
    import os
    for fr in FRAMES:
        for grade in ("day", "dusk", "night"):
            if not os.path.exists(f"{src}/{fr}-{grade}.png"):
                continue
            m = Image.open(f"{src}/{fr}-{grade}.png").convert("RGB")
            for w in WIDTHS[fr]:
                im = m.resize((w, round(m.height * w / m.width)), Image.LANCZOS) if w != m.width else m
                base = f"{dst}/{name}-{fr}-{grade}-{w}"
                im.save(base + ".avif", quality=AVIF_Q.get(name, 62), speed=4)
                im.save(base + ".webp", quality=80, method=6)
                im.save(base + ".jpg", quality=82, optimize=True, progressive=True)
    sizes = {}
    for f in sorted(os.listdir(dst)):
        if f.startswith(name + "-"):
            sizes[f] = os.path.getsize(f"{dst}/{f}")
    for f, s in sizes.items():
        print(f"{s/1024:7.1f} KB  {f}")


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "hill":
        hill(sys.argv[2], sys.argv[3])
    elif cmd == "dusk":
        dusk(sys.argv[2], sys.argv[3])
    elif cmd == "encode":
        encode(sys.argv[2], sys.argv[3], sys.argv[4])
