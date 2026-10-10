#!/usr/bin/env python3
"""Render Trek's Windows icons from the brand SVGs (the Windows twin of the icon steps in bundle.sh).

Writes:
  assets/brand/trek.ico                                 the app icon (ember, as the Mac bundle's
                                                        .icns), 16 20 24 32 48 64 128 256 px,
                                                        embedded in trek.exe by crates/trek-app/build.rs
  crates/trek-app/assets/brand/tray/{state}-{theme}-{px}.png
                                                        the system tray glyphs, state is idle |
                                                        working | attention, px is 16 20 24 32 (the
                                                        tray's size at 100% 125% 150% 200% display
                                                        scale). `light` is for a light taskbar
                                                        (black glyph), `dark` for a dark one (white).

Needs `resvg` on PATH (cargo install resvg; the same renderer bundle.sh uses) and nothing else:
Python's standard library packs the .ico. Run from anywhere: python script/windows-icons.py
"""
import os
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
BRAND = os.path.join(ROOT, "assets", "brand")
TRAY_OUT = os.path.join(ROOT, "crates", "trek-app", "assets", "brand", "tray")
ICO_SIZES = [16, 20, 24, 32, 48, 64, 128, 256]
TRAY_SIZES = [16, 20, 24, 32]
STATES = ["idle", "working", "attention"]
# The Mac bundle's icon (script/bundle.sh): Trek orange with porcelain stones.
APP_ICON_SVG = "trek-icon-ember.svg"


def resvg(svg, out, px):
    subprocess.run([RESVG, "-w", str(px), "-h", str(px), svg, out], check=True)


def whitened(svg_text):
    """The glyph SVGs draw black; a dark taskbar wants white. A filter on a wrapping group turns
    everything white and keeps the alpha (recolouring the fills would break the attention
    glyph's mask, which cuts its hole with black)."""
    head, rest = svg_text.split(">", 1)
    body, tail = rest.rsplit("</svg>", 1)
    return f'{head}><g filter="brightness(0) invert(1)">{body}</g></svg>{tail}'


def ico(frames):
    """A .ico of PNG frames (Vista and later read PNG at every size). `frames` is [(px, bytes)]."""
    out = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    for px, data in frames:
        # Width and height 0 mean 256; 0 colours and planes/bit count as for 32-bit colour.
        out += struct.pack("<BBBBHHII", px % 256, px % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(data for _, data in frames)


def main():
    global RESVG
    RESVG = shutil.which("resvg")
    if not RESVG:
        sys.exit("resvg isn't on PATH: cargo install resvg --locked")
    tmp = tempfile.mkdtemp()
    try:
        frames = []
        for px in ICO_SIZES:
            png = os.path.join(tmp, f"app-{px}.png")
            resvg(os.path.join(BRAND, APP_ICON_SVG), png, px)
            with open(png, "rb") as f:
                frames.append((px, f.read()))
        with open(os.path.join(BRAND, "trek.ico"), "wb") as f:
            f.write(ico(frames))

        os.makedirs(TRAY_OUT, exist_ok=True)
        for state in STATES:
            with open(os.path.join(BRAND, f"menubar-{state}.svg")) as f:
                black = f.read()
            for theme, text in (("light", black), ("dark", whitened(black))):
                svg = os.path.join(tmp, f"{state}-{theme}.svg")
                with open(svg, "w") as f:
                    f.write(text)
                for px in TRAY_SIZES:
                    resvg(svg, os.path.join(TRAY_OUT, f"{state}-{theme}-{px}.png"), px)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("wrote assets/brand/trek.ico and", os.path.relpath(TRAY_OUT, ROOT))


if __name__ == "__main__":
    main()
