"""The Mac's background art at phone sizes, for ios/Trek/Backgrounds.xcassets.

Each piece of the Mac's collections (crates/trek-app/src/backgrounds.rs; the files in
crates/trek-assets/assets/backgrounds, made by assets/backgrounds/make_collections.py) is cut to
a phone's tall shape around the part worth seeing, at the pixels it already has: nothing is
scaled, so the dithered pieces keep their own colours and stay small as palette PNGs. Light's
glows stay JPEG. Every piece but Classic's gets a Night and a Paper version (the image set's dark
and light appearances), and a small portrait thumbnail of each for Settings › Appearance.

The scrim the phone lays over the art (Design/Backgrounds.swift) is worked out from each
collection's extreme pixel, as on the Mac; this checks that every colour of every file it writes,
under that scrim, keeps the text 4.5:1 and the grey text 3:1 in Trek's Night and Paper, and
fails naming the colour if not.

  uv run --with pillow python -I ios/scripts/backgrounds.py
"""
import json, os, shutil, sys
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
SRC = os.path.join(ROOT, "crates", "trek-assets", "assets", "backgrounds")
OUT = os.path.join(ROOT, "ios", "Trek", "Backgrounds.xcassets")

# A phone's screen, width over height (19.5:9).
ASPECT = 9 / 19.5
THUMB = (96, 208)
# How much of the ground Light's glows take before they're coded again as JPEG: coding makes
# colours of its own, a shade past the collection's extreme, and this keeps them inside it.
LIFT = 0.1

# (collection, id, where across the piece the phone's cut is centred). Paintings are cut around
# their subject; the rest are centred, as the Mac's Cover fit centres them.
PIECES = [
    ("classic", "dawn", 0.5), ("classic", "night", 0.5), ("classic", "paper", 0.5),
    ("paintings", "matterhorn", 0.64), ("paintings", "kauterskill", 0.22),
    ("paintings", "oxbow", 0.64), ("paintings", "notch", 0.46),
    ("paintings", "green-river", 0.6), ("paintings", "aosta", 0.6),
    ("paintings", "keelmen", 0.42), ("paintings", "merced", 0.36),
    ("ascii", "ramp", 0.5), ("ascii", "braille", 0.5), ("ascii", "halftone", 0.5),
    ("ascii", "dense", 0.5), ("ascii", "led", 0.5),
    ("light", "streaks", 0.5), ("light", "aurora", 0.5), ("light", "window", 0.5),
    ("light", "switchbacks", 0.5),
]

# Collection::extreme in crates/trek-app/src/backgrounds.rs: (Night's brightest, Paper's darkest).
EXTREME = {
    "paintings": (0xFF7A3D, 0x6A6058),
    "ascii": (0xFF8448, 0x655A4E),
    "light": (0xBEAAA0, 0x74604F),
}
# Trek's Night and Paper: (window, text, muted).
NIGHT = (0x0A0A0B, 0xE4E4E7, 0x8B8B94)
PAPER = (0xFFFFFF, 0x1A1A1C, 0x6E6C68)


def rgb(h):
    return [((h >> 16) & 255) / 255, ((h >> 8) & 255) / 255, (h & 255) / 255]


def lum(c):
    lin = lambda v: v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])


def contrast(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def mix(art, ground, a):
    return [art[i] * (1 - a) + ground[i] * a for i in range(3)]


def scrim(art, ground, text, ratio):
    """Backgrounds.swift's `Scrim.alpha`, as backgrounds.rs has it."""
    most = 0.92
    reads = lambda a: contrast(text, mix(art, ground, a)) >= ratio
    if reads(0):
        return 0.0
    if not reads(most):
        return most
    lo, hi = 0.0, most
    for _ in range(24):
        mid = (lo + hi) / 2
        if reads(mid):
            hi = mid
        else:
            lo = mid
    return min(hi + 0.01, most)


def box(size, fx):
    """The phone's cut of a piece `size` big, centred at `fx` across it: (x, y, w, h)."""
    w, h = size
    if w / h > ASPECT:
        cw, ch = round(h * ASPECT), h
    else:
        cw, ch = w, round(w / ASPECT)
    x0 = min(max(round(fx * w - cw / 2), 0), w - cw)
    y0 = (h - ch) // 2
    return x0, y0, cw, ch


def check(path, collection, dark):
    if collection not in EXTREME:
        return
    extreme = rgb(EXTREME[collection][0 if dark else 1])
    ground, text, muted = (rgb(c) for c in (NIGHT if dark else PAPER))
    alpha = max(scrim(extreme, ground, text, 4.5), scrim(extreme, ground, muted, 3.0))
    colours = {c for _, c in Image.open(path).convert("RGB").getcolors(1 << 24)}
    for c in colours:
        px = [v / 255 for v in c]
        for t, ratio in ((text, 4.5), (muted, 3.0)):
            got = contrast(t, mix(px, ground, alpha))
            if got < ratio - 0.01:
                sys.exit(f"{path}: #{c[0]:02X}{c[1]:02X}{c[2]:02X} gives {got:.2f}:1 under {alpha:.2f}, under {ratio}:1")


def image_set(name, files):
    """`files`: [(filename, dark)]."""
    d = os.path.join(OUT, f"{name}.imageset")
    os.makedirs(d, exist_ok=True)
    images = []
    for filename, dark in files:
        entry = {"idiom": "universal", "filename": filename}
        if dark:
            entry["appearances"] = [{"appearance": "luminosity", "value": "dark"}]
        images.append(entry)
    with open(os.path.join(d, "Contents.json"), "w") as f:
        json.dump({"images": images, "info": {"author": "xcode", "version": 1}}, f, indent=2)
    return d


def size_of(d):
    return sum(os.path.getsize(os.path.join(d, f)) for f in os.listdir(d))


def main():
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    os.makedirs(OUT)
    with open(os.path.join(OUT, "Contents.json"), "w") as f:
        json.dump({"info": {"author": "xcode", "version": 1}}, f, indent=2)
    total = 0
    for collection, pid, fx in PIECES:
        name = f"bg-{collection}-{pid}"
        ext = "jpg" if collection == "light" else "png"
        if collection == "classic":
            variants = [(pid, False, os.path.join(SRC, f"{pid}.png"))]
        else:
            variants = [(v, v == "night", os.path.join(SRC, collection, f"{pid}-{v}.{ext}"))
                        for v in ("paper", "night")]
        full, thumbs = [], []
        for stem, dark, src in variants:
            source = Image.open(src)
            if source.mode != "P":
                source = source.convert("RGB")
            x, y, w, h = box(source.size, fx)
            im = source.crop((x, y, x + w, y + h))
            full.append((f"{stem}.{ext}", dark))
            thumbs.append((f"{stem}-thumb.jpg", dark))
            d = image_set(name, full)
            path = os.path.join(d, f"{stem}.{ext}")
            if ext == "png":
                im.save(path, optimize=True)
            else:
                ground = Image.new("RGB", im.size, (10, 10, 11) if dark else (255, 255, 255))
                Image.blend(im, ground, LIFT).save(path, quality=78, optimize=True, progressive=True,
                                                   subsampling=0)
            check(path, collection, dark)
            td = image_set(f"{name}-thumb", thumbs)
            im.convert("RGB").resize(THUMB, Image.BOX).save(os.path.join(td, f"{stem}-thumb.jpg"),
                                                            quality=72, optimize=True)
        total += size_of(os.path.join(OUT, f"{name}.imageset"))
        total += size_of(os.path.join(OUT, f"{name}-thumb.imageset"))
    print(f"{len(PIECES)} pieces, {total / 1024:.0f} KB")


main()
