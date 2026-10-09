"""Trek's brand: a cairn, the stones hikers stack so the next person knows the way.

Writes, next to this script:
  trek-icon-ember.svg  the app icon (the default): porcelain stones on a Trek-orange tile
  trek-icon-night.svg  alternative: Trek-orange stones on the Night theme's charcoal
  trek-icon-glass.svg  alternative: frosted glass stones over a sunset (Liquid Glass)
  trek-icon-*-ios.svg  the same three, full bleed, for iOS
  trek-mark.svg        the stones alone, for in-app use (title bar, About, welcome screens)
  menubar-{idle,working,attention}.svg   44 pt template glyphs for the menu bar

Run: python3 trek_icon.py. script/bundle.sh renders the .icns from trek-icon-ember.svg; the app
switches to the others at run time (Settings › Appearance › App icon).
"""
import math, random, sys, os

def squircle(cx=512, cy=512, half=412, n=4.2, steps=240):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + half * math.copysign(abs(c) ** (2 / n), c)
        y = cy + half * math.copysign(abs(s) ** (2 / n), s)
        pts.append((x, y))
    return "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in pts) + "Z"

TILE = squircle()

def smooth(pts):
    """Closed Catmull-Rom through pts as cubic beziers."""
    n = len(pts)
    d = f"M{pts[0][0]:.1f} {pts[0][1]:.1f}"
    for i in range(n):
        p0, p1, p2, p3 = pts[(i - 1) % n], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        d += f" C{c1[0]:.1f} {c1[1]:.1f} {c2[0]:.1f} {c2[1]:.1f} {p2[0]:.1f} {p2[1]:.1f}"
    return d + "Z"

def stone_pts(cx, cy, w, h, seed=1, taper=0.12, n=2.6, wobble=0.035, tilt=0.0, flat_bottom=0.25, k=36):
    rnd = random.Random(seed)
    ph = [rnd.uniform(0, 6.28) for _ in range(3)]
    pts = []
    for i in range(k):
        t = 2 * math.pi * i / k
        c, s = math.cos(t), math.sin(t)
        x = math.copysign(abs(c) ** (2 / n), c)
        y = math.copysign(abs(s) ** (2 / n), s)
        if y > 0:  # lower half flatter
            y = y * (1 - flat_bottom) + (abs(y) ** 0.35) * flat_bottom * (1 if y > 0 else -1)
        # top narrower
        x *= 1 - taper * (-(y) if y < 0 else 0) - taper * 0.25 * (y if y > 0 else 0)
        r = 1 + wobble * (math.sin(2 * t + ph[0]) + 0.6 * math.sin(3 * t + ph[1]) + 0.3 * math.sin(5 * t + ph[2]))
        X, Y = x * w / 2 * r, y * h / 2 * r
        Xr = X * math.cos(tilt) - Y * math.sin(tilt)
        Yr = X * math.sin(tilt) + Y * math.cos(tilt)
        pts.append((cx + Xr, cy + Yr))
    return pts

def stone(cx, cy, w, h, **kw):
    return smooth(stone_pts(cx, cy, w, h, **kw))

def lin(id, stops, x1=0, y1=0, x2=0, y2=1, units=None):
    u = f' gradientUnits="{units}"' if units else ""
    s = "".join(f'<stop offset="{o}" stop-color="{c}"' + (f' stop-opacity="{a}"' if a is not None else "") + "/>" for o, c, a in [(st + (None,))[:3] for st in stops])
    return f'<linearGradient id="{id}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"{u}>{s}</linearGradient>'

def rad(id, stops, cx=0.5, cy=0.5, r=0.5, fx=None, fy=None):
    f = (f' fx="{fx}"' if fx is not None else "") + (f' fy="{fy}"' if fy is not None else "")
    s = "".join(f'<stop offset="{o}" stop-color="{c}"' + (f' stop-opacity="{a}"' if a is not None else "") + "/>" for o, c, a in [(st + (None,))[:3] for st in stops])
    return f'<radialGradient id="{id}" cx="{cx}" cy="{cy}" r="{r}"{f}>{s}</radialGradient>'

COMMON = (
    '<filter id="blur8" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="8"/></filter>'
    '<filter id="blur16" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="16"/></filter>'
    '<filter id="blur28" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="28"/></filter>'
    '<filter id="blur4" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="4"/></filter>'
    + '<filter id="grain" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency="1.6" numOctaves="2" seed="7"/><feColorMatrix values="0 0 0 0 .35  0 0 0 0 .25  0 0 0 0 .15  0 0 0 .5 0"/><feComposite in2="SourceGraphic" operator="in"/></filter>'
    + lin("rim", [(0, "#fff", .55), (.18, "#fff", .08), (.6, "#fff", 0), (1, "#000", .25)])
    + lin("sheen", [(0, "#fff", .22), (.5, "#fff", 0)])
)

def tile(fill_defs, fill, extra_bg=""):
    return fill_defs, (
        f'<path d="{TILE}" fill="{fill}"/>' + extra_bg +
        f'<path d="{TILE}" fill="url(#sheen)"/>'
    )

def finish():
    return f'<path d="{TILE}" fill="none" stroke="url(#rim)" stroke-width="5" opacity=".9"/>'

def svg(defs, body):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><defs>{COMMON}{defs}'
            f'<clipPath id="tileclip"><path d="{TILE}"/></clipPath></defs>'
            f'<g filter="url(#tileshadow)"></g>{body}</svg>')

def porcelain_stack(stones, light, mid, dark, idp, shadow_col="#5A1E05", shadow_op=.35, spec=.75, side=None, thick=.16):
    """stones: (cx, cy, w, h, kw) bottom->top, drawn as slabs with a visible side edge."""
    side = side or dark
    defs = lin(f"{idp}fill", [(0, light), (.6, mid), (1, dark)]) + lin(f"{idp}side", [(0, side), (1, shadow_col)])
    body = ""
    b = stones[0]
    body += f'<ellipse cx="{b[0]+12}" cy="{b[1]+b[3]*0.62}" rx="{b[2]*0.5}" ry="{b[3]*0.2}" fill="{shadow_col}" opacity="{shadow_op}" filter="url(#blur16)"/>'
    for i, (cx, cy, w, h, kw) in enumerate(stones):
        t = h * thick
        top = stone(cx, cy - t / 2, w, h - t, **kw)
        under = stone(cx, cy + t / 2, w, h - t, **kw)
        body += f'<path d="{under}" fill="url(#{idp}side)"/>'
        body += f'<path d="{top}" fill="url(#{idp}fill)"/>'
        body += f'<clipPath id="{idp}c{i}"><path d="{top}"/></clipPath><g clip-path="url(#{idp}c{i})">'
        body += f'<path d="{top}" fill="#fff" filter="url(#grain)" opacity=".22"/>'
        body += f'<ellipse cx="{cx-w*0.12}" cy="{cy-h*0.5}" rx="{w*0.42}" ry="{h*0.3}" fill="#fff" opacity="{spec*0.6}" filter="url(#blur8)"/>'
        if i + 1 < len(stones):
            nx, ny, nw, nh, _ = stones[i + 1]
            body += f'<ellipse cx="{nx+8}" cy="{ny+nh*0.48}" rx="{nw*0.5}" ry="{nh*0.24}" fill="{shadow_col}" opacity=".5" filter="url(#blur8)"/>'
        body += "</g>"
    return defs, body

concepts = []

def add(slug, name, blurb, defs, body):
    """Write the macOS icon (`trek-icon-<slug>.svg`, the tile on Apple's grid) and its iOS twin
    (`trek-icon-<slug>-ios.svg`, full bleed: iOS masks the corners itself)."""
    s = svg(defs, body)
    open(f"trek-icon-{slug}.svg", "w").write(s)
    ios = s.replace(finish(), "").replace(f'<path d="{TILE}" fill="url(#sheen)"/>', "").replace(TILE, "M0 0H1024V1024H0Z")
    open(f"trek-icon-{slug}-ios.svg", "w").write(ios)

# Tile backgrounds -----------------------------------------------------------
EMBER = lin("ember", [(0, "#FF9455"), (.5, "#F4692B"), (1, "#D9481A")]) + rad("emberlight", [(0, "#FFC08A", .55), (1, "#FFC08A", 0)], cx=.3, cy=.15, r=.75)
NIGHT = lin("night", [(0, "#24252B"), (1, "#0B0B0D")]) + rad("nightlight", [(0, "#FF7A3D", .16), (1, "#FF7A3D", 0)], cx=.5, cy=.85, r=.7)
PAPER = lin("paper", [(0, "#FFFCF7"), (1, "#EEE4D6")])
DUSK = lin("dusk", [(0, "#2B3550"), (1, "#0E1220")])

def ember_tile():
    return f'<path d="{TILE}" fill="url(#ember)"/><path d="{TILE}" fill="url(#emberlight)"/>'
def night_tile():
    return f'<path d="{TILE}" fill="url(#night)"/><path d="{TILE}" fill="url(#nightlight)"/>'
def paper_tile():
    return f'<path d="{TILE}" fill="url(#paper)"/>'

CAIRN = [
    (506, 676, 500, 168, dict(seed=3, taper=.08, tilt=-0.035, n=3.2, wobble=.06)),
    (530, 538, 356, 138, dict(seed=7, taper=.12, tilt=0.06, n=3.0, wobble=.07)),
    (494, 418, 232, 116, dict(seed=11, taper=.16, tilt=-0.08, n=2.8, wobble=.07)),
    (518, 322, 120, 82, dict(seed=5, taper=.1, tilt=0.12, n=2.4, wobble=.06)),
]

# 1 Ember Cairn
d, b = porcelain_stack(CAIRN, "#FFFBF4", "#F7EBDD", "#E6D2BA", "c1", side="#D8BE9F", shadow_col="#7A3510")
add("ember", "Ember Cairn",
    "The cairn you picked, made solid: four porcelain stones with real weight, contact shadows and a lit top edge, on a warm Trek-orange tile. The safest evolution, and the most \"Apple\" of the set.",
    EMBER + d, ember_tile() + b + finish())

# 2 Night Cairn
d, b = porcelain_stack(CAIRN, "#FFC08A", "#FF7A3D", "#D9531E", "c2", side="#B8410F", shadow_col="#000", shadow_op=.6, spec=.6)
add("night", "Night Cairn",
    "The same stack in Trek's own orange on the Night theme's charcoal. Sits naturally next to Cursor, Zed and Terminal in a developer's Dock, and matches the app's dark UI.",
    NIGHT + d, night_tile() + b + finish())

# 3 Glass Cairn
gdefs = lin("glassfill", [(0, "#FFFFFF", .55), (.6, "#FFF1E6", .28), (1, "#FFFFFF", .16)]) + lin("glassedge", [(0, "#FFFFFF", 1), (.45, "#FFFFFF", .2), (1, "#FFFFFF", .55)])
gb = f'<ellipse cx="520" cy="790" rx="260" ry="40" fill="#4A1200" opacity=".45" filter="url(#blur16)"/>'
for i, (cx, cy, w, h, kw) in enumerate(CAIRN):
    dd = stone(cx, cy, w, h, **kw)
    gb += f'<path d="{dd}" fill="#4A1200" opacity=".28" transform="translate(6 16)" filter="url(#blur8)"/>'
    gb += f'<path d="{dd}" fill="url(#glassfill)"/>'
    gb += f'<clipPath id="g{i}"><path d="{dd}"/></clipPath><g clip-path="url(#g{i})">'
    gb += f'<ellipse cx="{cx-w*.18}" cy="{cy-h*.42}" rx="{w*.36}" ry="{h*.2}" fill="#fff" opacity=".7" filter="url(#blur8)"/>'
    gb += f'<ellipse cx="{cx+w*.2}" cy="{cy+h*.5}" rx="{w*.4}" ry="{h*.2}" fill="#FFB27A" opacity=".55" filter="url(#blur8)"/></g>'
    gb += f'<path d="{dd}" fill="none" stroke="url(#glassedge)" stroke-width="4"/>'
add("glass", "Glass Cairn",
    "Liquid Glass: frosted, translucent stones over a deep sunset gradient, with lit edges and warm light passing through. It reads as made for macOS 26, like Freeform or Journal.",
    lin("sun3", [(0, "#FF9450"), (.5, "#E9531E"), (1, "#9E2A0C")]) + rad("sun3b", [(0, "#FFD9A8", .75), (1, "#FFD9A8", 0)], cx=.78, cy=.08, r=.55) + gdefs,
    f'<path d="{TILE}" fill="url(#sun3)"/><path d="{TILE}" fill="url(#sun3b)"/>' + gb + finish())


# The mark: the cairn alone in Trek's orange, nearest the ground the deepest.
def mark():
    colors = [("#E8541E", "#9E2F0C"), ("#FF6A2B", "#B23C12"), ("#FF8A4C", "#C24A18"), ("#FFB062", "#D5621F")]
    body = ""
    for (cx, cy, w, h, kw), (top, side) in zip(CAIRN, colors):
        t = h * .26
        body += f'<path d="{stone(cx, cy + t / 2, w, h - t, **kw)}" fill="{side}"/><path d="{stone(cx, cy - t / 2, w, h - t, **kw)}" fill="{top}"/>'
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="226 250 580 540">{body}</svg>'
open("trek-mark.svg", "w").write(mark())

# Menu bar template glyphs (black on clear; macOS tints them): four stacked stones. "working"
# shows the top stone as a ring (the tray blinks between the two); "attention" adds a dot.
GLYPH = [(22, 36.5, 30, 9), (23, 27, 22, 8), (21.5, 18.5, 15, 7), (22.5, 10.5, 9, 6)]
def menubar(state):
    shapes = ""
    for i, (cx, cy, w, h) in enumerate(GLYPH):
        if state == "working" and i == len(GLYPH) - 1:
            shapes += f'<ellipse cx="{cx}" cy="{cy}" rx="{w/2-1}" ry="{h/2-1}" fill="none" stroke="#000" stroke-width="2"/>'
        else:
            shapes += f'<ellipse cx="{cx}" cy="{cy}" rx="{w/2}" ry="{h/2}" fill="#000"/>'
    if state == "attention":
        return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 44 44"><mask id="m"><rect width="44" height="44" fill="#fff"/>'
                f'<circle cx="37" cy="8" r="8" fill="#000"/></mask><g mask="url(#m)">{shapes}</g><circle cx="37" cy="8" r="5" fill="#000"/></svg>')
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 44 44">{shapes}</svg>'
for state in ("idle", "working", "attention"):
    open(f"menubar-{state}.svg", "w").write(menubar(state))
