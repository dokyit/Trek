"""Trek's brand: a trail signpost. A thick post with a chamfered cap, and two ember boards: the
upper one long, pointing right, its tip one long diagonal cut, a trail blaze cut through it; the
lower one shorter, pointing back left, its far end a swallowtail notch.

Three refinements of the drawing are kept here (VARIANTS: "a", "b", "c"); PICK is the one Trek
ships. To switch, change PICK and run the script: everything below, the PNGs and the app's own
drawing (crates/trek-brand/src/signpost.rs) follow.

Writes, next to this script:
  trek-icon-night.svg  the app icon (the default, and the one in the app bundle): the signpost
                       on a near-black navy tile, the post cream
  trek-icon-paper.svg  alternative: the signpost on a lavender tile, the post near-black
  trek-icon-glass.svg  alternative: glass boards and a frosted post against a dusk sky
  trek-icon-*-ios.svg  the same three, full bleed, for iOS (ios/scripts/app-icons.sh renders them)
  trek-icon-{dark,tinted}-ios.svg  the iOS icon's dark and tinted variants: the signpost on clear
  trek-icon-{night,paper,glass}-{16,32}.svg  the three drawn pixel by pixel for 16 and 32 px,
                       where the full art blurs
  trek-mark.svg        the signpost alone in Night's colours (cream post), for a dark ground
  trek-mark-paper.svg  the same in Paper's colours (near-black post), for a light ground
  menubar-{idle,working,attention}.svg   44 pt template glyphs for the menu bar and tray

Run: python3 trek_icon.py. With resvg on PATH (cargo install resvg) it also renders the PNGs the
app embeds (crates/trek-assets/assets/brand: icon.png, icon-paper.png, icon-glass.png, mark.png,
mark-paper.png, menubar-*.png), docs/assets/trek-icon.png and the iPhone app's TrekMark image
sets. script/bundle.sh renders the .icns from trek-icon-night.svg (its 16 and 32 px from
trek-icon-night-16.svg and -32.svg); the app switches to the others at run time (Settings ›
Appearance › App icon). script/windows-icons.py makes the Windows .icos the same way,
ios/scripts/app-icons.sh the iPhone's icons and script/site/make_icons.py the website's.

python3 trek_icon.py --variants DIR writes every variant's SVGs to DIR/<variant>/ instead (and
nothing in the repository), to compare them.

crates/trek-brand draws the same signpost in the app (the title bar, About, the welcome screens)
from signpost.rs, which this script writes; its tests read the SVGs here.
"""
import math, os, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))

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

# Geometry ----------------------------------------------------------------------------------------
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
        else:  # an inside corner (the swallowtail's notch): the two grown edges meet
            t = ((b[0] - a[0]) * d1[1] - (b[1] - a[1]) * d1[0]) / turn
            out.append(("L", (a[0] + t * d0[0], a[1] + t * d0[1])))
    return out

def rotated(p, pivot, degrees):
    """`p` turned `degrees` about `pivot`, clockwise on screen (y down), as SVG's rotate()."""
    a = math.radians(degrees)
    x, y = p[0] - pivot[0], p[1] - pivot[1]
    return (pivot[0] + x * math.cos(a) - y * math.sin(a), pivot[1] + x * math.sin(a) + y * math.cos(a))

def board(x0, x1, cy, h, left="square", right="square", chamfer=8.0, notch=6.0, chamfer_top=True):
    """A board, level, clockwise from its top left: x0 to x1, centred on cy, h tall. An end is
    "square", "point" (one long chamfer: one edge runs on `chamfer` past the other; the top one
    unless chamfer_top is False) or "swallow" (a notch `notch` deep)."""
    t, b = cy - h / 2, cy + h / 2
    top_left = (x0 + chamfer, t) if left == "point" and not chamfer_top else (x0, t)
    pts = [top_left]
    if right == "point":
        pts += [(x1, t), (x1 - chamfer, b)] if chamfer_top else [(x1 - chamfer, t), (x1, b)]
    elif right == "swallow":
        pts += [(x1, t), (x1 - notch, cy), (x1, b)]
    else:
        pts += [(x1, t), (x1, b)]
    if left == "point":
        pts += [(x0 + chamfer, b)] if chamfer_top else [(x0, b)]
    elif left == "swallow":
        pts += [(x0, b), (x0 + notch, cy)]
    else:
        pts += [(x0, b)]
    return pts

# The variants, drawn on a 96-unit square, then scaled about their own centre onto the square's
# (FIT). The post is (left, right, top, bottom, cap): the cap is one chamfer, its left corner
# `cap` lower than its right. A board's angle turns it about its pivot on the post, clockwise
# (so negative raises its right end); its blaze (left, top, width, height) is cut through it.
FIT = 0.84
EDGE = 3.0  # the round joins' diameter, before FIT
UPPER = dict(pts=board(30, 83, 30, 17, right="point", chamfer=8), pivot=(48, 30), angle=-6, blaze=(64.5, 24.5, 4, 11))
VARIANTS = {
    # A: the boards fanned apart (the upper rises to its tip, the lower to its notch), the lower
    # board's far end a swallowtail.
    "a": dict(post=(44, 52, 11, 87, 4), upper=UPPER,
              lower=dict(pts=board(19, 62, 57, 16, left="swallow", notch=6), pivot=(48, 57), angle=8, blaze=None)),
    # B: both boards point, each tip one chamfer (the lower's along its bottom); fanned as A.
    "b": dict(post=(44, 52, 11, 87, 4), upper=UPPER,
              lower=dict(pts=board(19, 62, 57, 16, left="point", chamfer=7, chamfer_top=False), pivot=(48, 57), angle=8, blaze=None)),
    # C (Trek's): both boards rise toward the right, the lower a little more, its far end a
    # swallowtail: one way on, the way back behind it. A heavier post.
    "c": dict(post=(43.5, 52.5, 11, 87, 4.5), upper=UPPER,
              lower=dict(pts=board(19, 62, 55, 16, left="swallow", notch=6), pivot=(48, 55), angle=-8, blaze=None)),
}
PICK = "c"

class Signpost:
    """A variant's shapes on the 96-unit square, after FIT: the post's outline, and each board's
    outline, blaze (an outline, or None) and pivot and angle (for the app's animation)."""
    def __init__(self, slug):
        v = VARIANTS[slug]
        x0, x1, top, bottom, cap = v["post"]
        post = [(x0, top + cap), (x1, top), (x1, bottom), (x0, bottom)]
        boards = []
        for b in (v["upper"], v["lower"]):
            turn = lambda p: rotated(p, b["pivot"], b["angle"])
            blaze = None
            if b["blaze"]:
                bx, by, bw, bh = b["blaze"]
                blaze = [turn(p) for p in [(bx, by), (bx + bw, by), (bx + bw, by + bh), (bx, by + bh)]]
            boards.append(dict(outline=[turn(p) for p in b["pts"]], blaze=blaze, pivot=b["pivot"], angle=b["angle"]))
        everything = post + [p for b in boards for p in b["outline"]]
        xs, ys = [p[0] for p in everything], [p[1] for p in everything]
        cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
        fit = lambda p: (round(48 + FIT * (p[0] - cx), 2), round(48 + FIT * (p[1] - cy), 2))
        self.post = [fit(p) for p in post]
        self.boards = [dict(outline=[fit(p) for p in b["outline"]], blaze=b["blaze"] and [fit(p) for p in b["blaze"]],
                            pivot=fit(b["pivot"]), angle=b["angle"]) for b in boards]
        self.r = EDGE * FIT / 2
        # The mark's own box: square, round the shapes and their joins, on whole units.
        grownx = [p[0] for p in self.post + [q for b in self.boards for q in b["outline"]]]
        growny = [p[1] for p in self.post + [q for b in self.boards for q in b["outline"]]]
        side = math.ceil(max(max(grownx) - min(grownx), max(growny) - min(growny)) + 2 * self.r + 2)
        side += side % 2
        self.box = (48 - side // 2, 48 - side // 2, side, side)

class Drawn:
    """A Signpost's shapes as SVG paths, scaled by k and moved to (ox, oy)."""
    def __init__(self, sign, k=1.0, ox=0.0, oy=0.0):
        self.s, self.k, self.ox, self.oy = sign, k, ox, oy

    def at(self, p):
        return f"{num(self.ox + self.k * p[0])} {num(self.oy + self.k * p[1])}"

    def outline(self, pts):
        r = self.s.r
        d = ""
        for i, (kind, p) in enumerate(grown(pts, r)):
            if i == 0:
                d += "M" + self.at(p)
            elif kind == "A":
                d += f"A{num(r * self.k)} {num(r * self.k)} 0 0 1 {self.at(p)}"
            else:
                d += "L" + self.at(p)
        return d + "Z"

    def hole(self, pts):
        return "M" + "L".join(self.at(p) for p in pts) + "Z"

    def post_d(self):
        return self.outline(self.s.post)

    def board_d(self, b):
        """The board, its blaze cut through it (with fill-rule evenodd)."""
        return self.outline(b["outline"]) + (self.hole(b["blaze"]) if b["blaze"] else "")

    def flat(self, post, board):
        """The signpost in flat colour: the post, then the boards over it."""
        out = f'<path d="{self.post_d()}" fill="{post}"/>'
        for b in self.s.boards:
            out += f'<path d="{self.board_d(b)}" fill="{board}" fill-rule="evenodd"/>'
        return out

# Colours -----------------------------------------------------------------------------------------
NIGHT = dict(tile="#1D1A2C", post="#F6F1EA", board="#FF7A3D")
PAPER = dict(tile="#C9C1DE", post="#1F1714", board="#EE5A24")
# Glass's sky: dusk. Deep navy at the top, through plum, to an ember glow low on the horizon.
DUSK = (lin("dusk", [(0, "#0B1222"), (.4, "#3A2740"), (.6, "#4A324E"), (.8, "#B0603F"), (.9, "#D6804F"), (1, "#B0603F")])
        + rad("duskglow", [(0, "#D6804F", .55), (1, "#D6804F", 0)], cx=.5, cy=.86, r=.5))
GLASS_POST = "#F3E6DC"
GLASS_BOARD = "#FF9A5C"  # the boards' glass, flat, where it's drawn small

def colours(c):
    return dict(post=c["post"], board=c["board"])

# The art ----------------------------------------------------------------------------------------
# The small sizes, pixel by pixel: '#' the post, 'o' a board, '.' the ground. A board is a band of
# rows; each end square, a point (the top row runs on, each row below stops chamfer/(h-1) sooner,
# or the other way up) or a swallowtail (a V notch `depth` deep); a step (col, dy) moves the cells
# left of col dy rows (the boards' rise); a blaze (col, row in the board, width, height) is a hole.
def board_cells(x0, x1, top, h, left="square", right="square", chamfer=0, depth=0, chamfer_top=True, steps=(), blaze=None):
    cells = set()
    mid = (h - 1) / 2
    for r in range(h):
        f = r / (h - 1) if chamfer_top else (h - 1 - r) / (h - 1)
        lo, hi = x0, x1
        if right == "point":
            hi = x1 - round(f * chamfer)
        if left == "point":
            lo = x0 + round(f * chamfer)
        if left == "swallow":
            lo = x0 + round(depth * (1 - abs(r - mid) / mid))
        for c in range(lo, hi + 1):
            if blaze and blaze[0] <= c < blaze[0] + blaze[2] and blaze[1] <= r < blaze[1] + blaze[3]:
                continue
            cells.add((c, top + r + sum(d for col, d in steps if c < col)))
    return cells

def grid(px, post, boards):
    """post: (left, width, top, bottom, cap): the top `cap` rows lose a column on the left each."""
    g = [["."] * px for _ in range(px)]
    x0, w, top, bottom, cap = post
    for r in range(top, bottom + 1):
        cut = max(0, cap - (r - top))
        for c in range(x0 + min(cut, w - 1), x0 + w):
            g[r][c] = "#"
    for cells in boards:
        for c, r in cells:
            g[r][c] = "o"
    return ["".join(row) for row in g]

def small_grids(slug):
    """A variant's small art: on the tile at 16 and 32 px (the tile 1 to 15 and 3 to 29), and the
    bare mark in a 16 pt box a point a pixel (mark16) and two pixels a point (mark32). Level
    boards: a one-row step reads as a kink this small."""
    lower = dict(left="swallow") if slug in ("a", "c") else dict(left="point", chamfer_top=False)
    return {
        "tile16": grid(16, (7, 2, 2, 13, 1), [
            board_cells(5, 12, 4, 3, right="point", chamfer=1),
            board_cells(3, 9, 8, 3, depth=1, chamfer=1, **lower)]),
        "tile32": grid(32, (14, 4, 4, 28, 2), [
            board_cells(9, 25, 8, 7, right="point", chamfer=3, blaze=(20, 2, 2, 3)),
            board_cells(5, 20, 18, 7, depth=3, chamfer=3, **lower)]),
        "mark16": grid(16, (7, 2, 1, 15, 1), [
            board_cells(4, 14, 3, 4, right="point", chamfer=2, blaze=(11, 1, 1, 2)),
            board_cells(1, 10, 8, 4, depth=1, chamfer=2, **lower)]),
        "mark32": grid(32, (14, 4, 1, 31, 2), [
            board_cells(7, 29, 6, 8, right="point", chamfer=4, blaze=(23, 2, 2, 4)),
            board_cells(2, 21, 19, 8, depth=3, chamfer=4, **lower)]),
    }

def runs(rows, ch):
    """The cells of `ch` as horizontal runs: (x, y, width)."""
    out = []
    for y, row in enumerate(rows):
        x = 0
        while x < len(row):
            if row[x] == ch:
                start = x
                while x < len(row) and row[x] == ch:
                    x += 1
                out.append((start, y, x - start))
            else:
                x += 1
    return out

def pixels(rows, post, board):
    """A grid as SVG: the post's runs, then the boards'."""
    return "".join(f'<path d="' + "".join(f"M{x} {y}h{w}v1h-{w}Z" for x, y, w in runs(rows, ch)) + f'" fill="{colour}"/>'
                   for ch, colour in (("#", post), ("o", board)))

# On the Mac the tile is Apple's grid: 824 of 1024 across, and the 96-unit square is the tile's
# box. On iOS the icon is full bleed (iOS masks the corners itself), the square the whole canvas.
SQUARE = "M0 0H1024V1024H0Z"
TILE_CLIP = f'<clipPath id="tileclip"><path d="{TILE}"/></clipPath>'

def hairline(color, opacity):
    """A faint line just inside the Mac tile's edge, so a flat tile keeps its shape on a ground
    of its own colour (Night in a dark Dock, Paper in a white Finder window). 3 px of 1024."""
    return f'<path d="{TILE}" fill="none" stroke="{color}" stroke-opacity="{opacity}" stroke-width="6" clip-path="url(#tileclip)"/>'

# Glass: the boards are slabs of ember glass against the dusk, lit along their top edges, the
# blaze a window through to the sky; the post frosted, pale. No shadows.
GLASS = (lin("glassboard", [(0, "#FFD2B0", .92), (.55, "#FFA36A", .82), (1, "#F07A3C", .78)])
         + lin("glassedge", [(0, "#FFFFFF", .95), (.5, "#FFE2CC", .25), (1, "#FFD0AC", .55)])
         + lin("glasspost", [(0, "#FFF6EE", .92), (1, "#E9D6CA", .72)]))
RIM = lin("rim", [(0, "#fff", .4), (.18, "#fff", .08), (.6, "#fff", 0), (1, "#000", .25)])
GLASS_RIM = f'<path d="{TILE}" fill="none" stroke="url(#rim)" stroke-width="5" opacity=".9"/>'

def dusk(shape):
    return f'<path d="{shape}" fill="url(#dusk)"/><path d="{shape}" fill="url(#duskglow)"/>'

def glass(m):
    g = f'<path d="{m.post_d()}" fill="url(#glasspost)"/>'
    g += f'<path d="{m.post_d()}" fill="none" stroke="#FFFFFF" stroke-opacity=".35" stroke-width="{num(m.k * .3)}"/>'
    for i, b in enumerate(m.s.boards):
        d = m.board_d(b)
        g += f'<path d="{d}" fill="url(#glassboard)" fill-rule="evenodd"/>'
        g += f'<path d="{d}" fill="none" stroke="url(#glassedge)" stroke-width="{num(m.k * .45)}"/>'
    return g

class Writer:
    def __init__(self, slug, out):
        self.slug, self.out = slug, out
        self.sign = Signpost(slug)
        self.grids = small_grids(slug)

    def write(self, name, text):
        with open(os.path.join(self.out, name), "w") as f:
            f.write(text)

    def all(self):
        s = self.sign
        on_tile, full = Drawn(s, 824 / 96, 100, 100), Drawn(s, 1024 / 96, 0, 0)
        for slug, c, edge in (("night", NIGHT, hairline("#FFFFFF", ".09")), ("paper", PAPER, hairline("#2A2140", ".12"))):
            self.write(f"trek-icon-{slug}.svg", svg(f'<path d="{TILE}" fill="{c["tile"]}"/>' + edge + on_tile.flat(**colours(c)), TILE_CLIP))
            self.write(f"trek-icon-{slug}-ios.svg", svg(f'<path d="{SQUARE}" fill="{c["tile"]}"/>' + full.flat(**colours(c))))
        self.write("trek-icon-glass.svg", svg(dusk(TILE) + glass(on_tile) + GLASS_RIM, DUSK + GLASS + RIM))
        self.write("trek-icon-glass-ios.svg", svg(dusk(SQUARE) + glass(full), DUSK + GLASS))
        # iOS 18's dark and tinted Home Screen icons: the signpost alone on clear (iOS draws the
        # dark tile, or tints the grey drawing itself). Dark wears Night's colours; tinted is
        # greyscale, the post the darker grey so the boards stand in front of it.
        self.write("trek-icon-dark-ios.svg", svg(full.flat(**colours(NIGHT))))
        self.write("trek-icon-tinted-ios.svg", svg(full.flat(post="#9E9E9E", board="#FFFFFF")))
        # The mark alone, in its own box.
        x, y, w, h = s.box
        for name, c in (("trek-mark.svg", NIGHT), ("trek-mark-paper.svg", PAPER)):
            self.write(name, svg(Drawn(s).flat(**colours(c)), box=f"{x} {y} {w} {h}"))
        # 16 and 32 px, pixel by pixel on each tile.
        tiles = {"night": ("", lambda t: f'<path d="{t}" fill="{NIGHT["tile"]}"/>', colours(NIGHT)),
                 "paper": ("", lambda t: f'<path d="{t}" fill="{PAPER["tile"]}"/>', colours(PAPER)),
                 "glass": (DUSK, dusk, dict(post=GLASS_POST, board=GLASS_BOARD))}
        for slug, (defs, tile, c) in tiles.items():
            for px in (16, 32):
                half = 7 if px == 16 else 13
                body = tile(squircle(px / 2, px / 2, half, steps=120, places=2)) + pixels(self.grids[f"tile{px}"], c["post"], c["board"])
                self.write(f"trek-icon-{slug}-{px}.svg", svg(body, defs, box=f"0 0 {px} {px}"))
        for state in ("idle", "working", "attention"):
            self.write(f"menubar-{state}.svg", menubar(state))

# Menu bar and tray glyphs ------------------------------------------------------------------------
# Template glyphs (black on clear; macOS tints them, script/windows-icons.py whitens them for a
# dark taskbar), 44 units for 22 pt, every edge on even units (whole pixels at 22 px). The
# signpost level and square-jointed, the post 4 units (a pixel at 1x is too thin to stand).
#   idle       the signpost, filled
#   working    the boards drawn as outlines, the post stopping at them (the tray blinks between
#              this and idle while agents work)
#   attention  the signpost, filled, with a dot at the bottom right, the one corner the boards
#              leave clear
GLYPH_POST = [(20, 6), (24, 3), (24, 42), (20, 42)]
GLYPH_UPPER = [(14, 8), (40, 8), (34, 18), (14, 18)]
GLYPH_LOWER = [(4, 22), (30, 22), (30, 32), (4, 32), (8, 27)]
GLYPH_DOT = (36, 37, 5)

def glyph_path(pts):
    return "M" + "L".join(f"{num(x)} {num(y)}" for x, y in pts) + "Z"

def menubar(state):
    boards = glyph_path(GLYPH_UPPER) + glyph_path(GLYPH_LOWER)
    if state == "working":
        # The boards' outlines, 2 units inside their edges; the post masked out under them.
        defs = (f'<clipPath id="boards"><path d="{boards}"/></clipPath>'
                f'<mask id="post"><rect width="44" height="44" fill="#fff"/><path d="{boards}" fill="#000"/></mask>')
        shapes = (f'<path d="{glyph_path(GLYPH_POST)}" fill="#000" mask="url(#post)"/>'
                  f'<path d="{boards}" fill="none" stroke="#000" stroke-width="4" stroke-linejoin="miter" clip-path="url(#boards)"/>')
        return svg(shapes, defs, box="0 0 44 44")
    shapes = f'<path d="{glyph_path(GLYPH_POST)}" fill="#000"/><path d="{boards}" fill="#000"/>'
    if state == "attention":
        shapes += f'<circle cx="{GLYPH_DOT[0]}" cy="{GLYPH_DOT[1]}" r="{GLYPH_DOT[2]}" fill="#000"/>'
    return svg(shapes, box="0 0 44 44")

# The app's own drawing ---------------------------------------------------------------------------
def rust(sign, grids):
    """crates/trek-brand/src/signpost.rs: the picked variant's numbers for the app to draw."""
    f = lambda v: f"{v:.2f}"
    pts = lambda ps: "&[" + ", ".join(f"({f(x)}, {f(y)})" for x, y in ps) + "]"
    boards = ""
    for b in sign.boards:
        blaze = pts(b["blaze"]) if b["blaze"] else "&[]"
        boards += (f"    Board {{\n        outline: {pts(b['outline'])},\n        blaze: {blaze},\n"
                   f"        pivot: ({f(b['pivot'][0])}, {f(b['pivot'][1])}),\n        angle: {f(b['angle'])},\n    }},\n")
    rows = lambda g: "[\n" + "".join(f'    "{r}",\n' for r in g) + "]"
    x, y, w, h = sign.box
    return (f"//! The signpost's numbers, written by `assets/brand/trek_icon.py` (variant \"{PICK}\"): change\n"
            "//! the drawing there and run it, not here.\n\n"
            "use super::{Board, P};\n\n"
            "/// The round joins' radius.\n"
            f"pub(super) const RADIUS: f32 = {f(sign.r)};\n"
            "/// The post's outline, clockwise from its cap's low corner.\n"
            f"pub(super) const POST: &[P] = {pts(sign.post)};\n"
            "/// The upper board, then the lower.\n"
            f"pub(super) const BOARDS: [Board; 2] = [\n{boards}];\n"
            "/// The mark's own box: left, top, width, height (`trek-mark.svg`'s view box).\n"
            f"pub(super) const BOX: (f32, f32, f32, f32) = ({f(x)}, {f(y)}, {f(w)}, {f(h)});\n"
            "/// The small mark in its 16 pt box, a point a pixel: `#` the post, `o` a board.\n"
            f"pub(super) const SMALL_1X: [&str; 16] = {rows(grids['mark16'])};\n"
            "/// The same, two pixels a point.\n"
            f"pub(super) const SMALL_2X: [&str; 32] = {rows(grids['mark32'])};\n")

def render(out):
    resvg = shutil.which("resvg")
    if not resvg:
        print("resvg isn't on PATH: wrote the SVGs only (cargo install resvg --locked, then run again for the PNGs)")
        return
    def png(src, dst, w, h=None):
        subprocess.run([resvg, "-w", str(w)] + (["-h", str(h)] if h else []) + [os.path.join(out, src), dst], check=True)
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
            shutil.copyfile(os.path.join(out, name), os.path.join(imageset, name))

def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--variants":
        for slug in VARIANTS:
            out = os.path.join(sys.argv[2], slug)
            os.makedirs(out, exist_ok=True)
            Writer(slug, out).all()
        print(f"wrote variants {', '.join(VARIANTS)} to {sys.argv[2]}")
        return
    w = Writer(PICK, HERE)
    w.all()
    path = os.path.join(ROOT, "crates", "trek-brand", "src", "signpost.rs")
    with open(path, "w") as f:
        f.write(rust(w.sign, w.grids))
    if shutil.which("rustfmt"):
        subprocess.run(["rustfmt", "--edition", "2024", path], check=True)
    render(HERE)
    print(f"wrote variant {PICK}: the SVGs here, signpost.rs, the PNGs in crates/trek-assets/assets/brand and docs/assets, and ios/*/Assets.xcassets/TrekMark.imageset")

if __name__ == "__main__":
    main()
