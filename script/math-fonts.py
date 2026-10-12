#!/usr/bin/env python3
"""Make the KaTeX fonts GPUI won't load loadable, as crates/trek-app/assets/fonts/TrekMath_*.ttf.

GPUI's macOS text system refuses any font without an `m` (it measures with one), and six KaTeX
faces have none: AMS, Caligraphic, Script and Size1–4 (big operators and delimiters). This maps
`m` to the font's space glyph — nothing Trek draws in these faces is an `m` — and, as the SIL
Open Font License asks of a modified font, renames the family (KaTeX_Size1 → TrekMath_Size1).
Everything else in the font is untouched.

    python3 -m venv /tmp/ft && /tmp/ft/bin/pip install fonttools
    /tmp/ft/bin/python script/math-fonts.py /tmp/katex/package/dist/fonts crates/trek-app/assets/fonts
"""

import os
import sys

from fontTools.ttLib import TTFont

FACES = ["AMS-Regular", "Caligraphic-Regular", "Script-Regular", "Size1-Regular", "Size2-Regular", "Size3-Regular", "Size4-Regular"]


def main(src, dst):
    for face in FACES:
        font = TTFont(os.path.join(src, f"KaTeX_{face}.ttf"))
        cmap = font.getBestCmap()
        blank = cmap.get(0x20) or cmap.get(0xA0)
        for table in font["cmap"].tables:
            if table.isUnicode() and ord("m") not in table.cmap:
                table.cmap[ord("m")] = blank
        family = face.split("-")[0]
        for record in font["name"].names:
            text = record.toUnicode()
            if "KaTeX" in text and record.nameID in (1, 3, 4, 6, 16, 17, 18, 21):
                record.string = text.replace("KaTeX_", "TrekMath_").replace("KaTeX", "TrekMath")
        font.save(os.path.join(dst, f"TrekMath_{face}.ttf"))
        print(f"TrekMath_{face}.ttf ({family})")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
