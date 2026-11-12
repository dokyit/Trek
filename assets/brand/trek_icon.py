"""Trek's brand: one mountain with a lit face and a shaded face, a dotted trail up the lit face
and a small flag planted behind the summit.

Writes, next to this script:
  trek-icon-night.svg  the app icon (the default, and the one in the app bundle): Trek's orange
                       on the Night theme's charcoal
  trek-icon-paper.svg  alternative: the deeper orange on the Paper theme's cream
  trek-icon-glass.svg  alternative: a frosted glass mountain against a dusk sky (Liquid Glass)
  trek-icon-*-ios.svg  the same three, full bleed, for iOS (ios/scripts/app-icons.sh renders them)
  trek-icon-{dark,tinted}-ios.svg  the iOS icon's dark and tinted variants: the mountain on clear
  trek-icon-{night,paper,glass}-{16,32}.svg  the three drawn for 16 and 32 px, on whole pixels,
                       where the full art blurs: no trail at 16, a shorter one of larger dots at 32
  trek-mark.svg        the mountain alone in Night's colours, the trail's dots cut through it so
                       the ground shows, for in-app use (the browser's start page, the iPhone app)
  trek-mark-paper.svg  the same in Paper's colours, for a light ground
  menubar-{idle,working,attention}.svg   44 pt template glyphs for the menu bar

Run: python3 trek_icon.py. With resvg on PATH (cargo install resvg) it also renders the PNGs the
app embeds (crates/trek-assets/assets/brand: icon.png, icon-paper.png, icon-glass.png, mark.png,
mark-paper.png, menubar-*.png), docs/assets/trek-icon.png and the iPhone app's TrekMark image
sets. script/bundle.sh renders the .icns from trek-icon-night.svg (its 16 and 32 px from
trek-icon-night-16.svg and -32.svg); the app switches to the others at run time (Settings ›
Appearance › App icon). script/windows-icons.py makes the Windows .icos the same way, and
ios/scripts/app-icons.sh the iPhone's icons.

crates/trek-brand draws the same mountain in the app (the title bar, About, the welcome screens)
from the numbers below; its tests read this script's SVGs, so change the two together.
"""
import math, os, shutil, subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
os.chdir(HERE)

def squircle(cx=512, cy=512, half=412, n=4.2, steps=240, places=1):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + half * math.copysign(abs(c) ** (2 / n), c)
        y = cy + half * math.copysign(abs(s) ** (2 / n), s)
        pts.append((x, y))
    return "M" + " L".join(f"{x:.{places}f} {y:.{places}f}" for x, y in pts) + "Z"

TILE = squircle()

def num(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("", "-0") else s

def lin(id, stops, x1=0, y1=0, x2=0, y2=1, units=None):
    u = f' gradientUnits="{units}"' if units else ""
    s = "".join(f'<stop offset="{o}" stop-color="{c}"' + (f' stop-opacity="{a}"' if a is not None else "") + "/>" for o, c, a in [(st + (None,))[:3] for st in stops])
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"{u}>{s}</linearGradient>'

def rad(id, stops, cx=0.5, cy=0.5, r=0.5, fx=None, fy=None):
    f = (f' fx="{fx}"' if fx is not None else "") + (f' fy="{fy}"' if fy is not None else "")
    s = "".join(f'<stop offset="{o}" stop-color="{c}"' + (f' stop-opacity="{a}"' if a is not None else "") + "/>" for o, c, a in [(st + (None,))[:3] for st in stops])
    return f'<radialGradient id="{id}" cx="{cx}" cy="{cy}" r="{r}"{f}>{s}</radialGradient>'

def svg(body, defs="", box="0 0 1024 1024"):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{box}">' + (f"<defs>{defs}</defs>" if defs else "") + body + "</svg>"

def write(name, text):
    with open(name, "w") as out:
        out.write(text)

# The mark, on a 96-unit square ---------------------------------------------------------------
# The outline, clockwise from the foot of the lit face: foot, shoulder (two points), summit, the
# shaded face's foot. It's filled and stroked EDGE wide in the same colour with round joins,
# which is what rounds the summit and the feet.
OUTLINE = [(9.5, 78.5), (32.8, 46.6), (43, 43.4), (56, 21), (86.5, 78.5)]
EDGE = 4.5
# The ridge, summit to base. The shaded face is the outline's own shape right of this line, so
# the two faces share one silhouette and meet exactly at the summit. (A second stroked polygon
# for the shade would cover the tip with its stroke.)
RIDGE = [(56, 21), (60.6, 39.6), (55, 51.6), (62.6, 64.8), (58.8, 78.5)]
# The trail up the lit face, from its foot: centre x, y, radius.
DOTS = [(29, 74.2, 2.4), (36, 69.2, 2.4), (43, 66, 2.4), (47.2, 60, 2.4), (44, 53.2, 2.4),
        (48, 47, 2.4), (51.6, 40.4, 2.4), (53.6, 34, 2.4), (54.6, 28.2, 2)]
# The flag, drawn behind the mountain: the pole (x, foot, top, width; round caps) and the cloth
# (a start, then curves and lines: three points are a cubic bezier, one a line).
POLE = (56, 24, 8.6, 2)
CLOTH = [(56.4, 8.6), [(59.3, 7.2), (61.5, 10.2), (65.7, 8.8)], [(65.7, 15.2)],
         [(61.5, 16.6), (59.3, 13.6), (56.4, 15)]]
# The mark's own box on that square (left, top, width, height), a little air included.
MARK_BOX = (7, 7, 82, 74)

def unit(a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    l = math.hypot(dx, dy)
    return dx / l, dy / l

def grown(pts, r):
    """A clockwise outline grown by r with round joins: what filling it and stroking it 2r wide
    covers. [(kind, point)]: "L" a line to it, "A" a clockwise arc of radius r to it."""
    out = []
    for i, cur in enumerate(pts):
        d0, d1 = unit(pts[i - 1], cur), unit(cur, pts[(i + 1) % len(pts)])
        a = (cur[0] + r * d0[1], cur[1] - r * d0[0])
        b = (cur[0] + r * d1[1], cur[1] - r * d1[0])
        turn = d0[0] * d1[1] - d0[1] * d1[0]
        if turn > 0:  # a corner that points out: the stroke's round join
            out += [("L", a), ("A", b)]
        else:  # the shoulder's inside corner: the two grown edges meet
            t = ((b[0] - a[0]) * d1[1] - (b[1] - a[1]) * d1[0]) / turn
            out.append(("L", (a[0] + t * d0[0], a[1] + t * d0[1])))
    return out

class Mountain:
    """The mark's shapes as SVG, scaled by k and moved to (ox, oy). The defaults are the mark
    itself; the small sizes pass their own outline, ridge, trail and flag."""
    def __init__(self, k=1.0, ox=0.0, oy=0.0, outline=OUTLINE, edge=EDGE, ridge=RIDGE, dots=DOTS, pole=POLE, cloth=CLOTH, cap="round"):
        self.k, self.ox, self.oy = k, ox, oy
        self.outline, self.r, self.ridge, self.dots, self.pole, self.cloth = outline, edge / 2, ridge, dots, pole, cloth
        self.cap = cap  # the pole's ends: round on the mark, "butt" where it's drawn on whole pixels
        assert ridge[0] == outline[-2], "the ridge starts at the summit, the outline's last corner but one"

    def at(self, p):
        return f"{num(self.ox + self.k * p[0])} {num(self.oy + self.k * p[1])}"

    def path(self, steps):
        d = ""
        for i, (kind, p) in enumerate(steps):
            if i == 0:
                d += "M" + self.at(p)
            elif kind == "A":
                if self.r > 0:
                    d += f"A{num(self.r * self.k)} {num(self.r * self.k)} 0 0 1 {self.at(p)}"
            else:
                d += "L" + self.at(p)
        return d + "Z"

    def body(self):
        """The whole silhouette."""
        return self.path(grown(self.outline, self.r))

    def holes(self):
        """The trail's dots as subpaths: with fill-rule evenodd they're cut through the body."""
        d = ""
        for x, y, r in self.dots:
            R = num(r * self.k)
            d += f"M{self.at((x - r, y))}A{R} {R} 0 1 0 {self.at((x + r, y))}A{R} {R} 0 1 0 {self.at((x - r, y))}Z"
        return d

    def shade(self):
        """The silhouette right of the ridge: from the top of the summit's round join, round the
        shaded face's foot, along the base to under the ridge's end, and back up the ridge."""
        summit, foot, first = self.outline[-2], self.outline[-1], self.outline[0]
        r = self.r
        down, base = unit(summit, foot), unit(foot, first)
        steps = [("L", (summit[0], summit[1] - r)), ("A", (summit[0] + r * down[1], summit[1] - r * down[0])),
                 ("L", (foot[0] + r * down[1], foot[1] - r * down[0])), ("A", (foot[0] + r * base[1], foot[1] - r * base[0])),
                 ("L", (self.ridge[-1][0], self.ridge[-1][1] + r))]
        return self.path(steps + [("L", p) for p in reversed(self.ridge)])

    def ridge_line(self):
        """The ridge as an open line, for the one-colour mark, which cuts it out of the body."""
        pts = [(self.ridge[0][0], self.ridge[0][1] - self.r)] + list(self.ridge) + [(self.ridge[-1][0], self.ridge[-1][1] + self.r)]
        return "M" + "L".join(self.at(p) for p in pts)

    def pole_d(self):
        x, foot, top, _ = self.pole
        return f"M{self.at((x, foot))}L{self.at((x, top))}"

    def pole_width(self):
        return num(self.pole[3] * self.k)

    def cloth_d(self):
        d = "M" + self.at(self.cloth[0])
        for seg in self.cloth[1:]:
            d += ("C" if len(seg) == 3 else "L") + " ".join(self.at(p) for p in seg)
        return d + "Z"

    def flag(self, color):
        return (f'<path d="{self.pole_d()}" stroke="{color}" stroke-width="{self.pole_width()}" stroke-linecap="{self.cap}"/>'
                f'<path d="{self.cloth_d()}" fill="{color}"/>')

    def flat(self, lit, shade, flag):
        """The mark in flat colour: the flag, then the body with the trail cut through it (so the
        dots are the colour of whatever it stands on), then the shaded face over its right."""
        return (self.flag(flag)
                + f'<path d="{self.body()}{self.holes()}" fill="{lit}" fill-rule="evenodd"/>'
                + f'<path d="{self.shade()}" fill="{shade}"/>')

# Colours ---------------------------------------------------------------------------------
NIGHT = dict(tile="#1A1A1C", lit="#FF7A3D", shade="#C9551F", flag="#FFC7A0")
PAPER = dict(tile="#FBF8F3", lit="#E85D1F", shade="#B8461A", flag="#B8461A")
# Glass's sky: dusk. Deep navy at the top, through plum, to an ember glow low on the horizon,
# behind the mountain's foot. (Not an orange tile: a pale peak on orange is other apps' icon.)
DUSK = (lin("dusk", [(0, "#0B1222"), (.4, "#3A2740"), (.6, "#4A324E"), (.8, "#B0603F"), (.9, "#D6804F"), (1, "#B0603F")])
        + rad("duskglow", [(0, "#D6804F", .55), (1, "#D6804F", 0)], cx=.5, cy=.86, r=.5))

def colours(c):
    return dict(lit=c["lit"], shade=c["shade"], flag=c["flag"])

# The app icons ------------------------------------------------------------------------------
# On the Mac the tile is Apple's grid: 824 of 1024 across, and the mark's 96-unit square is the
# tile's box. On iOS the icon is full bleed (iOS masks the corners itself), the square the whole
# canvas, so the mountain is as large on its tile there as here.
ON_TILE = Mountain(824 / 96, 100, 100)
FULL_BLEED = Mountain(1024 / 96, 0, 0)
SQUARE = "M0 0H1024V1024H0Z"

TILE_CLIP = f'<clipPath id="tileclip"><path d="{TILE}"/></clipPath>'

def hairline(color, opacity):
    """A faint line just inside the Mac tile's edge, so a flat tile keeps its shape on a ground
    of its own colour (Night in a dark Dock, Paper in a white Finder window). 3 px of 1024."""
    return f'<path d="{TILE}" fill="none" stroke="{color}" stroke-opacity="{opacity}" stroke-width="6" clip-path="url(#tileclip)"/>'

def flat_icon(slug, c, edge):
    """`trek-icon-<slug>.svg` and its iOS twin `trek-icon-<slug>-ios.svg`."""
    write(f"trek-icon-{slug}.svg", svg(f'<path d="{TILE}" fill="{c["tile"]}"/>' + edge + ON_TILE.flat(**colours(c)), TILE_CLIP))
    write(f"trek-icon-{slug}-ios.svg", svg(f'<path d="{SQUARE}" fill="{c["tile"]}"/>' + FULL_BLEED.flat(**colours(c))))

flat_icon("night", NIGHT, hairline("#FFFFFF", ".09"))
flat_icon("paper", PAPER, hairline("#3A2A1C", ".12"))

# Glass: the mountain as a slab of frosted glass against the dusk, catching the last light: warm
# where the horizon's glow comes through its foot, a lit edge along the top of the lit face, the
# shaded face the same glass with the night on it. The trail's dots are holes in the glass, and
# the flag is the pale ember of Night's.
BLUR = ('<filter id="blur8" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="8"/></filter>'
        '<filter id="blur16" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="16"/></filter>')
GLASS = (lin("glassfill", [(0, "#FFE2C8", .86), (.5, "#FFC39A", .66), (1, "#FFA76B", .5)])
         + lin("glassedge", [(0, "#FFFFFF", 1), (.45, "#FFE2CC", .3), (1, "#FFD0AC", .7)]))
GLASS_FLAG = "#FFC7A0"

def glass(m):
    k = m.k
    body, shade = m.body(), m.shade()
    top = m.outline[-2][1] - m.r  # the summit's top, and the mountain's height from there to its base
    tall = m.outline[0][1] + m.r - top
    left, top = m.ox + k * MARK_BOX[0], m.oy + k * top
    width, height = k * MARK_BOX[2], k * tall
    cx = left + width / 2
    g = f'<ellipse cx="{num(cx + 8)}" cy="{num(top + height + 4)}" rx="{num(width * .5)}" ry="{num(k * 3.4)}" fill="#3A1408" opacity=".4" filter="url(#blur16)"/>'
    # The flag, behind the mountain.
    g += f'<path d="{m.pole_d()}" stroke="{GLASS_FLAG}" stroke-width="{m.pole_width()}" stroke-linecap="round"/>'
    g += f'<path d="{m.cloth_d()}" fill="{GLASS_FLAG}"/>'
    g += f'<path d="{body}" fill="#1A0D14" opacity=".3" transform="translate(6 14)" filter="url(#blur8)"/>'
    g += f'<path d="{body}{m.holes()}" fill="url(#glassfill)" fill-rule="evenodd"/>'
    g += f'<clipPath id="gm"><path d="{body}{m.holes()}" clip-rule="evenodd"/></clipPath><g clip-path="url(#gm)">'
    g += f'<ellipse cx="{num(left + width * .34)}" cy="{num(top + height * .36)}" rx="{num(width * .3)}" ry="{num(height * .22)}" fill="#FFEAD6" opacity=".55" filter="url(#blur16)"/>'
    g += f'<path d="{shade}" fill="#241633" opacity=".5"/>'
    g += f'<ellipse cx="{num(left + width * .55)}" cy="{num(top + height)}" rx="{num(width * .5)}" ry="{num(height * .2)}" fill="#FF9A5C" opacity=".6" filter="url(#blur8)"/></g>'
    g += f'<path d="{m.ridge_line()}" fill="none" stroke="#FFE7D3" stroke-opacity=".5" stroke-width="{num(k * .42)}" stroke-linejoin="round"/>'
    g += f'<path d="{body}" fill="none" stroke="url(#glassedge)" stroke-width="4"/>'
    for x, y, r in m.dots:
        g += f'<circle cx="{num(m.ox + k * x)}" cy="{num(m.oy + k * y)}" r="{num(k * r)}" fill="none" stroke="#FFE7D3" stroke-opacity=".55" stroke-width="2.5"/>'
    return g

def dusk(shape):
    return f'<path d="{shape}" fill="url(#dusk)"/><path d="{shape}" fill="url(#duskglow)"/>'

# The Mac tile's lit rim, as glass has: bright along the top, dark under the bottom.
RIM = lin("rim", [(0, "#fff", .4), (.18, "#fff", .08), (.6, "#fff", 0), (1, "#000", .25)])
GLASS_RIM = f'<path d="{TILE}" fill="none" stroke="url(#rim)" stroke-width="5" opacity=".9"/>'

write("trek-icon-glass.svg", svg(dusk(TILE) + glass(ON_TILE) + GLASS_RIM, BLUR + DUSK + GLASS + RIM))
write("trek-icon-glass-ios.svg", svg(dusk(SQUARE) + glass(FULL_BLEED), BLUR + DUSK + GLASS))

# iOS 18's dark and tinted Home Screen icons: the mountain alone on a clear ground (iOS draws
# the dark tile, or tints the grey mountain itself). Dark wears Night's colours; tinted is
# greyscale, the shaded face the darker grey.
write("trek-icon-dark-ios.svg", svg(FULL_BLEED.flat(**colours(NIGHT))))
write("trek-icon-tinted-ios.svg", svg(FULL_BLEED.flat(lit="#FFFFFF", shade="#A6A6A6", flag="#E0E0E0")))

# The mark: the mountain alone. The dots are cut through, so the ground it's put on shows.
def mark(c):
    x, y, w, h = MARK_BOX
    return svg(Mountain().flat(**colours(c)), box=f"{x} {y} {w} {h}")
write("trek-mark.svg", mark(NIGHT))
write("trek-mark-paper.svg", mark(PAPER))

# The app icon at 16 and 32 px (Finder's list view, the Dock at its smallest, Windows' small
# icons). Scaled that far the full art blurs: the trail's dots are a third of a pixel across at
# 16 px and the flag one pixel. These are drawn for their size, in pixels: the base, the flag
# and the summit's column on whole pixels, the shaded face starting on a pixel's edge, the flag
# a pole one pixel wide with a cloth of whole pixels (two by one at 16 px, three by two at 32),
# no trail at 16 px and a shorter one of larger dots at 32. script/bundle.sh and script/windows-icons.py use these at 16 and 32 px only: from 64 px
# up the full art reads (the .icos' 20, 24 and 48 px frames still scale it). The title bar's
# 16 pt mark is drawn the same way, without the tile, in crates/trek-brand (SMALL_1X, SMALL_2X).
#
# Per size: the tile's half width, then the mountain in pixels. The full art's tile is 824 of
# 1024 across, 12.9 px at 16 and 25.75 at 32; here 14 px (1 to 15) and 26 (3 to 29).
def rect(x, y, w, h):
    return [(x, y), [(x + w, y)], [(x + w, y + h)], [(x, y + h)]]

SMALL = {
    16: (7, dict(
        outline=[(2.5, 13), (5.6, 8.6), (7.1, 8.1), (9.5, 4), (13.5, 13)], edge=0,
        ridge=[(9.5, 4), (10, 5), (10, 13)],
        dots=[],
        pole=(9.5, 5, 2, 1), cap="butt",
        cloth=rect(10, 2, 2, 1))),
    32: (13, dict(
        outline=[(5.6, 24.4), (11.9, 15.7), (14.7, 14.8), (18.5, 8.6), (26.4, 24.4)], edge=1.2,
        ridge=[(18.5, 8.6), (19.7, 13.7), (18.2, 17), (20.3, 20.6), (19.3, 24.4)],
        dots=[(10.5, 22.5, 1.1), (13.5, 20.5, 1.1), (15.5, 17.5, 1.1), (16.5, 14.5, 1.1)],
        pole=(18.5, 10, 5, 1), cap="butt",
        cloth=rect(19, 5, 3, 2))),
}
# Each icon's tile and colours. Glass keeps its dusk; its mountain is the frosted glass's own
# colours against it, flat (the full art has its lit edges and glow to show it's glass), and
# light enough to read on the plum above and the ember below.
SMALL_TILES = {
    "night": ("", lambda t: f'<path d="{t}" fill="{NIGHT["tile"]}"/>', colours(NIGHT)),
    "paper": ("", lambda t: f'<path d="{t}" fill="{PAPER["tile"]}"/>', colours(PAPER)),
    "glass": (DUSK, dusk, dict(lit="#F6D2BC", shade="#8E6470", flag=GLASS_FLAG)),
}

def small_icon(slug, px):
    """`trek-icon-<slug>-<px>.svg`."""
    defs, tile, c = SMALL_TILES[slug]
    half, shapes = SMALL[px]
    body = tile(squircle(px / 2, px / 2, half, steps=120, places=2)) + Mountain(**shapes).flat(**c)
    write(f"trek-icon-{slug}-{px}.svg", svg(body, defs, box=f"0 0 {px} {px}"))

for slug in SMALL_TILES:
    for px in SMALL:
        small_icon(slug, px)

# Menu bar template glyphs (black on clear; macOS tints them), 44 units for 22 pt: the mountain
# in one colour with its ridge cut out of it, and the flag, on even units (whole pixels at 22 px)
# and larger than its share so it reads there. "working" shows the flag's cloth as an outline
# (the tray blinks between the two); "attention" adds a dot beside the flag.
GLYPH = Mountain(
    outline=[(5, 39), (15, 24.9), (19.4, 23.5), (25, 14), (39, 39)], edge=2,
    ridge=[(25, 14), (27.1, 21.8), (24.6, 27.1), (27.9, 33), (26.3, 39)],
    dots=[],
    pole=(25, 15, 6, 2), cap="butt",
    cloth=rect(26, 6, 6, 4))
GLYPH_DOT = (39.5, 8, 4.25)

def menubar(state):
    m = GLYPH
    if state == "working":
        (x, y), (right, _), (_, bottom) = m.cloth[0], m.cloth[1][0], m.cloth[2][0]
        cloth = f'<rect x="{x + .5}" y="{y + .5}" width="{right - x - 1}" height="{bottom - y - 1}" fill="none" stroke="#000"/>'
    else:
        cloth = f'<path d="{m.cloth_d()}" fill="#000"/>'
    shapes = (f'<path d="{m.pole_d()}" stroke="#000" stroke-width="{m.pole_width()}"/>' + cloth
              + f'<g mask="url(#ridge)"><path d="{m.body()}" fill="#000"/></g>')
    if state == "attention":
        shapes += f'<circle cx="{GLYPH_DOT[0]}" cy="{GLYPH_DOT[1]}" r="{GLYPH_DOT[2]}" fill="#000"/>'
    defs = f'<mask id="ridge"><rect width="44" height="44" fill="#fff"/><path d="{m.ridge_line()}" fill="none" stroke="#000" stroke-width="1.6" stroke-linejoin="round"/></mask>'
    return svg(shapes, defs, box="0 0 44 44")

for state in ("idle", "working", "attention"):
    write(f"menubar-{state}.svg", menubar(state))

# The PNGs drawn from the above that the repository keeps ------------------------------------
def render():
    resvg = shutil.which("resvg")
    if not resvg:
        print("resvg isn't on PATH: wrote the SVGs only (cargo install resvg --locked, then run again for the PNGs)")
        return
    def png(src, out, w, h=None):
        subprocess.run([resvg, "-w", str(w)] + (["-h", str(h)] if h else []) + [src, out], check=True)
    embedded = os.path.join(ROOT, "crates", "trek-assets", "assets", "brand")
    for name, slug in (("icon", "night"), ("icon-paper", "paper"), ("icon-glass", "glass")):
        png(f"trek-icon-{slug}.svg", os.path.join(embedded, f"{name}.png"), 512, 512)
    png("trek-mark.svg", os.path.join(embedded, "mark.png"), 256)
    png("trek-mark-paper.svg", os.path.join(embedded, "mark-paper.png"), 256)
    for state in ("idle", "working", "attention"):
        png(f"menubar-{state}.svg", os.path.join(embedded, f"menubar-{state}.png"), 44, 44)
    png("trek-icon-night.svg", os.path.join(ROOT, "docs", "assets", "trek-icon.png"), 256, 256)
    # The iPhone app shows the mark for its theme (the image set's dark appearance is Night's);
    # the Live Activity's ground is always dark.
    for target, names in (("Trek", ("trek-mark.svg", "trek-mark-paper.svg")), ("TrekActivity", ("trek-mark.svg",))):
        imageset = os.path.join(ROOT, "ios", target, "Assets.xcassets", "TrekMark.imageset")
        for name in names:
            shutil.copyfile(name, os.path.join(imageset, name))
    print("wrote the SVGs here, the PNGs in crates/trek-assets/assets/brand and docs/assets, and ios/*/Assets.xcassets/TrekMark.imageset")

render()
