#!/usr/bin/env python3
"""Pixelate the parts of each as-launched screenshot that show the owner's own thread, project and file
names, then save a web-sized copy for the public report. Regions are fractions of the image
(x0, y0, x1, y1), chosen by looking at each capture. Run with: uv run --with pillow python -I sanitize_shots.py <in_dir> <out_dir>"""
import sys
from pathlib import Path
from PIL import Image

# Sidebars, transcripts, titles and terminals that carry private text.
REGIONS = {
    "trek-home": [(0.74, 0.09, 0.97, 0.17)],
    "codex-app": [(0.03, 0.06, 0.21, 1.0), (0.44, 0.46, 0.78, 0.52), (0.47, 0.84, 0.56, 0.88)],
    "cursor": [(0.0, 0.03, 0.21, 1.0), (0.21, 0.40, 0.60, 0.48), (0.21, 0.0, 1.0, 0.05)],
    "t3-code": [(0.0, 0.18, 0.23, 0.75), (0.23, 0.0, 0.45, 0.06), (0.40, 0.33, 0.80, 0.40)],
    "synara": [(0.0, 0.08, 0.16, 1.0), (0.16, 0.0, 0.58, 0.92), (0.58, 0.04, 1.0, 1.0)],
    "orca": [(0.0, 0.05, 0.25, 1.0), (0.25, 0.0, 1.0, 1.0)],
    "zed": [(0.33, 0.58, 0.67, 0.70), (0.0, 0.0, 0.3, 0.03)],
    "opencode-desktop": [(0.0, 0.0, 1.0, 0.06), (0.08, 0.2, 0.32, 0.5), (0.33, 0.25, 0.92, 0.45)],
    "antigravity": [(0.0, 0.05, 0.21, 1.0), (0.35, 0.44, 0.80, 0.56), (0.3, 0.84, 0.75, 0.90)],
    "vs-code": [(0.0, 0.0, 1.0, 1.0)],
    "capy": [(0.0, 0.05, 0.25, 1.0)],
}


def pixelate(im, box, block=28):
    w, h = im.size
    x0, y0, x1, y1 = int(box[0] * w), int(box[1] * h), int(box[2] * w), int(box[3] * h)
    if x1 - x0 < 2 or y1 - y0 < 2:
        return
    part = im.crop((x0, y0, x1, y1))
    small = part.resize((max(1, (x1 - x0) // block), max(1, (y1 - y0) // block)), Image.BILINEAR)
    im.paste(small.resize(part.size, Image.NEAREST), (x0, y0))


SKIP = {"synara", "orca", "vs-code", "calculator-baseline", "textedit-baseline"}  # nearly all private content: kept out of the public report


def main():
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    dst.mkdir(parents=True, exist_ok=True)
    for f in sorted(src.glob("*.png")):
        if f.stem in SKIP:
            continue
        im = Image.open(f).convert("RGB")
        for box in REGIONS.get(f.stem, []):
            pixelate(im, box)
        im.thumbnail((1400, 1400))
        im.save(dst / f.name, optimize=True)
        print(f.name, im.size)


if __name__ == "__main__":
    main()
