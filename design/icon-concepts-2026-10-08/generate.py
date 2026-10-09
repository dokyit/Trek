import math, random, sys, os
OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)

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
    s = svg(defs, body)
    open(os.path.join(OUT, f"{slug}.svg"), "w").write(s)
    concepts.append((slug, name, blurb))

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
add("01-ember-cairn", "Ember Cairn",
    "The cairn you picked, made solid: four porcelain stones with real weight, contact shadows and a lit top edge, on a warm Trek-orange tile. The safest evolution, and the most \"Apple\" of the set.",
    EMBER + d, ember_tile() + b + finish())

# 2 Night Cairn
d, b = porcelain_stack(CAIRN, "#FFC08A", "#FF7A3D", "#D9531E", "c2", side="#B8410F", shadow_col="#000", shadow_op=.6, spec=.6)
add("02-night-cairn", "Night Cairn",
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
add("03-glass-cairn", "Glass Cairn",
    "Liquid Glass: frosted, translucent stones over a deep sunset gradient, with lit edges and warm light passing through. It reads as made for macOS 26, like Freeform or Journal.",
    lin("sun3", [(0, "#FF9450"), (.5, "#E9531E"), (1, "#9E2A0C")]) + rad("sun3b", [(0, "#FFD9A8", .75), (1, "#FFD9A8", 0)], cx=.78, cy=.08, r=.55) + gdefs,
    f'<path d="{TILE}" fill="url(#sun3)"/><path d="{TILE}" fill="url(#sun3b)"/>' + gb + finish())

# 4 Balance (paper)
d4 = lin("char", [(0, "#55555E"), (1, "#1A1A1E")]) + lin("charside", [(0, "#16161A"), (1, "#0A0A0C")]) + lin("org4", [(0, "#FFA770"), (.6, "#F26A2C"), (1, "#C9451A")]) + lin("ivo4", [(0, "#FFFFFF"), (1, "#E9DCCB")])
base = stone(512, 628, 250, 230, seed=4, taper=.06, n=2.2, flat_bottom=.45, wobble=.04)
slab_top = stone(500, 430, 640, 120, seed=21, taper=.02, tilt=-0.06, n=3.6, wobble=.03)
slab_side = stone(500, 450, 640, 120, seed=21, taper=.02, tilt=-0.06, n=3.6, wobble=.03)
peb_top = stone(640, 332, 132, 78, seed=9, n=2.3, tilt=0.12, wobble=.05)
peb_side = stone(640, 344, 132, 78, seed=9, n=2.3, tilt=0.12, wobble=.05)
b4 = f'<ellipse cx="520" cy="752" rx="210" ry="30" fill="#5A3A1A" opacity=".35" filter="url(#blur16)"/>'
b4 += f'<path d="{base}" fill="url(#org4)"/><clipPath id="b4"><path d="{base}"/></clipPath><g clip-path="url(#b4)"><path d="{base}" fill="#fff" filter="url(#grain)" opacity=".4"/><ellipse cx="470" cy="560" rx="120" ry="80" fill="#fff" opacity=".35" filter="url(#blur16)"/><ellipse cx="500" cy="512" rx="170" ry="34" fill="#000" opacity=".45" filter="url(#blur8)"/></g>'
b4 += f'<path d="{slab_side}" fill="url(#charside)"/><path d="{slab_top}" fill="url(#char)"/><clipPath id="s4"><path d="{slab_top}"/></clipPath><g clip-path="url(#s4)"><path d="{slab_top}" fill="#fff" filter="url(#grain)" opacity=".35"/><ellipse cx="430" cy="392" rx="280" ry="36" fill="#fff" opacity=".2" filter="url(#blur16)"/><ellipse cx="648" cy="372" rx="70" ry="16" fill="#000" opacity=".55" filter="url(#blur8)"/></g>'
b4 += f'<path d="{peb_side}" fill="#CDBBA4"/><path d="{peb_top}" fill="url(#ivo4)"/>'
add("04-balance", "Balance",
    "A long slab balanced on one orange stone, with a pebble on top: many agents held steady by one point. Paper tile, charcoal and orange. Quiet, editorial and unmistakable at 16px.",
    PAPER + d4, paper_tile() + b4 + finish())

# 5 Trail Blaze (granite)
d5 = ('<filter id="granite" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency=".9" numOctaves="3" seed="4"/><feColorMatrix values="0 0 0 0 .5  0 0 0 0 .5  0 0 0 0 .52  0 0 0 .55 0"/><feComposite in2="SourceGraphic" operator="in"/></filter>'
      + lin("gran", [(0, "#A39E98"), (1, "#615C58")]) + lin("paint", [(0, "#FF8A4C"), (1, "#E85D1F")]) + lin("paintw", [(0, "#FFFDF8"), (1, "#EFE6D8")]))
def blaze(y, h, fill, seed):
    rnd = random.Random(seed)
    top = [(300 + i * 42, y + rnd.uniform(-4, 4)) for i in range(11)]
    bot = [(720 - i * 42, y + h + rnd.uniform(-4, 4)) for i in range(11)]
    pts = top + [(728, y + h * .5)] + bot + [(296, y + h * .5)]
    return f'<path d="{smooth(pts)}" fill="{fill}"/>'
b5 = f'<path d="{TILE}" fill="url(#gran)"/><path d="{TILE}" fill="#fff" filter="url(#granite)" opacity=".55"/>'
b5 += '<g transform="rotate(-4 512 512)">'
b5 += f'<rect x="300" y="300" width="424" height="424" rx="18" fill="#000" opacity=".25" filter="url(#blur16)"/>'
b5 += blaze(300, 128, "url(#paint)", 1) + blaze(448, 128, "url(#paintw)", 2) + blaze(596, 128, "url(#paint)", 3)
b5 += '</g>'
add("05-trail-blaze", "Trail Blaze",
    "The painted stripes hikers follow on rocks and trees, here orange, ivory, orange on a granite tile. A real wayfinding symbol, flat and graphic like the trail signs it comes from, and like nothing else in a developer's Dock.",
    d5, b5 + finish())

# 6 Switchback Ribbon (night)
d6 = lin("band", [(0, "#FFC08A"), (.5, "#FF8A4C"), (1, "#F2622A")], 0, 0, 1, 1) + lin("bandside", [(0, "#A8380F"), (1, "#5E1C05")])
def switchback_path():
    levels = [(760, 250), (626, 200), (514, 156), (420, 118), (344, 84)]
    d = f"M{512-levels[0][1]} {levels[0][0]}"
    for i in range(len(levels) - 1):
        (y0, w0), (y1, w1) = levels[i], levels[i + 1]
        s_ = 1 if i % 2 == 0 else -1
        xe = 512 + s_ * w0
        r = (y0 - y1) / 2
        d += f" L{xe - s_*r*0.2:.0f} {y0}"
        d += f" C{xe + s_*r*1.1:.0f} {y0} {512 + s_*w1 + s_*r*1.1:.0f} {y1} {512 + s_*w1 - s_*r*0.2:.0f} {y1}"
    yT, wT = levels[-1]
    d += f" L{512} {yT}"
    return d
sp = switchback_path()
b6 = f'<ellipse cx="512" cy="812" rx="300" ry="30" fill="#000" opacity=".6" filter="url(#blur16)"/>'
b6 += f'<path d="{sp}" fill="none" stroke="url(#bandside)" stroke-width="64" stroke-linecap="round" stroke-linejoin="round" transform="translate(0 16)"/>'
b6 += f'<path d="{sp}" fill="none" stroke="url(#band)" stroke-width="64" stroke-linecap="round" stroke-linejoin="round"/>'
b6 += f'<path d="{sp}" fill="none" stroke="#fff" stroke-opacity=".35" stroke-width="10" stroke-linecap="round" stroke-linejoin="round" transform="translate(-4 -18)" filter="url(#blur4)"/>'
add("06-switchback-ribbon", "Switchback Ribbon",
    "Today's switchback, rebuilt as a thick sculpted ribbon in Trek orange with a lit top and a shadowed underside. Keeps the shipping icon's idea and loses its thin-line look.",
    NIGHT + d6, night_tile() + b6 + finish())

# 7 Stone T
d7 = lin("ivo7", [(0, "#FFFBF4"), (.6, "#F4E6D4"), (1, "#DCC5AA")]) + lin("ivo7s", [(0, "#C9A988"), (1, "#8E5A33")])
post_top = stone(512, 590, 220, 340, seed=31, taper=.1, n=3.4, flat_bottom=.3, wobble=.035)
lint_top = stone(512, 352, 520, 150, seed=32, taper=.04, n=3.6, tilt=-0.04, wobble=.04)
lint_side = stone(512, 372, 520, 150, seed=32, taper=.04, n=3.6, tilt=-0.04, wobble=.04)
b7 = f'<ellipse cx="524" cy="772" rx="190" ry="30" fill="#5A1E05" opacity=".45" filter="url(#blur16)"/>'
b7 += f'<path d="{post_top}" fill="url(#ivo7)"/><clipPath id="p7"><path d="{post_top}"/></clipPath><g clip-path="url(#p7)"><path d="{post_top}" fill="#fff" filter="url(#grain)" opacity=".22"/><ellipse cx="512" cy="446" rx="150" ry="40" fill="#5A1E05" opacity=".55" filter="url(#blur8)"/><rect x="570" y="420" width="90" height="400" fill="#5A1E05" opacity=".16" filter="url(#blur16)"/></g>'
b7 += f'<path d="{lint_side}" fill="url(#ivo7s)"/><path d="{lint_top}" fill="url(#ivo7)"/><clipPath id="l7"><path d="{lint_top}"/></clipPath><g clip-path="url(#l7)"><path d="{lint_top}" fill="#fff" filter="url(#grain)" opacity=".22"/><ellipse cx="460" cy="306" rx="240" ry="44" fill="#fff" opacity=".65" filter="url(#blur16)"/></g>'
add("07-stone-t", "Stone T",
    "A T made of two stones: a heavy lintel balanced on a standing stone. A monogram you can say (\"the T\") that's still a trail marker, not lettering.",
    EMBER + d7, ember_tile() + b7 + finish())

# 8 Stepping Stones (dusk water)
d8 = lin("ivo8", [(0, "#FFFFFF"), (1, "#E6D7C4")]) + lin("ivo8s", [(0, "#B8A288"), (1, "#5E4A36")]) + lin("water", [(0, "#2E3A57"), (1, "#0C111F")]) + rad("dawn8", [(0, "#FF7A3D", .7), (.5, "#FF7A3D", .15), (1, "#FF7A3D", 0)], cx=.8, cy=.0, r=.7)
b8 = f'<path d="{TILE}" fill="url(#water)"/><path d="{TILE}" fill="url(#dawn8)"/>'
steps = [(318, 738, 380, 150, 41), (540, 572, 290, 116, 42), (700, 444, 214, 86, 43), (806, 352, 150, 60, 44)]
for (cx, cy, w, h, sd) in steps:
    t = h * .28
    for k, op in ((1.0, .22), (1.25, .12)):
        b8 += f'<ellipse cx="{cx}" cy="{cy+h*.35}" rx="{w*.55*k}" ry="{h*.42*k}" fill="none" stroke="#A9BEEA" stroke-opacity="{op}" stroke-width="3"/>'
    b8 += f'<ellipse cx="{cx+6}" cy="{cy+h*.4}" rx="{w*.5}" ry="{h*.28}" fill="#000" opacity=".5" filter="url(#blur8)"/>'
    under = stone(cx, cy + t/2, w, h - t, seed=sd, n=2.6, flat_bottom=.4, wobble=.05)
    top = stone(cx, cy - t/2, w, h - t, seed=sd, n=2.6, flat_bottom=.4, wobble=.05)
    b8 += f'<path d="{under}" fill="url(#ivo8s)"/><path d="{top}" fill="url(#ivo8)"/><clipPath id="st{sd}"><path d="{top}"/></clipPath><g clip-path="url(#st{sd})"><path d="{top}" fill="#fff" filter="url(#grain)" opacity=".22"/><ellipse cx="{cx+w*.25}" cy="{cy-h*.3}" rx="{w*.3}" ry="{h*.2}" fill="#FFB27A" opacity=".5" filter="url(#blur8)"/></g>'
add("08-stepping-stones", "Stepping Stones",
    "Four stones leading across still water toward first light: the next step, then the one after. The only cool-toned option, with Trek orange kept to the horizon and the stones' lit edges.",
    d8, b8 + finish())

# 9 Stone Gate
d9 = lin("ivo9", [(0, "#FFFBF4"), (.6, "#F2E3CF"), (1, "#D9C0A2")]) + lin("portal", [(0, "#FFD0A0"), (1, "#FF8A4C")])
b9 = ember_tile()
b9 += f'<ellipse cx="512" cy="776" rx="300" ry="34" fill="#5A1E05" opacity=".4" filter="url(#blur16)"/>'
b9 += f'<path d="M402 420 L622 420 L622 760 L402 760Z" fill="url(#portal)" opacity=".9"/>'
lp = stone(350, 588, 150, 360, seed=51, n=3.0, taper=.06, wobble=.02, flat_bottom=.3)
rp = stone(676, 590, 150, 352, seed=52, n=3.0, taper=.06, wobble=.02, flat_bottom=.3)
top = stone(512, 384, 560, 118, seed=53, n=3.4, tilt=.02, wobble=.02)
for nm, dd in (("lp", lp), ("rp", rp)):
    b9 += f'<path d="{dd}" fill="url(#ivo9)"/><clipPath id="{nm}"><path d="{dd}"/></clipPath><g clip-path="url(#{nm})"><ellipse cx="512" cy="448" rx="300" ry="34" fill="#5A1E05" opacity=".45" filter="url(#blur8)"/></g>'
b9 += f'<path d="{top}" fill="url(#ivo9)"/><clipPath id="t9"><path d="{top}"/></clipPath><g clip-path="url(#t9)"><ellipse cx="470" cy="346" rx="260" ry="40" fill="#fff" opacity=".6" filter="url(#blur16)"/></g>'
add("09-stone-gate", "Stone Gate",
    "Two standing stones and a lintel framing a warm opening: a way in to your codebase, your worktrees, the next task. Strong silhouette, calm at every size.",
    EMBER + d9, b9 + finish())

# 10 Contour Stone (night)
d10 = lin("ivo10", [(0, "#FFFCF6"), (.6, "#F1E4D2"), (1, "#D6BFA2")])
rock_pts = stone_pts(512, 536, 620, 470, seed=61, n=2.1, taper=.1, wobble=.035, flat_bottom=.35, k=48)
rock = smooth(rock_pts)
b10 = night_tile()
b10 += f'<ellipse cx="520" cy="782" rx="280" ry="40" fill="#000" opacity=".65" filter="url(#blur16)"/>'
b10 += f'<path d="{rock}" fill="url(#ivo10)"/><clipPath id="r10"><path d="{rock}"/></clipPath><g clip-path="url(#r10)">'
b10 += f'<path d="{rock}" fill="#fff" filter="url(#grain)" opacity=".22"/>'
b10 += f'<ellipse cx="450" cy="380" rx="280" ry="150" fill="#fff" opacity=".6" filter="url(#blur28)"/>'
b10 += f'<ellipse cx="540" cy="780" rx="340" ry="150" fill="#6B4320" opacity=".4" filter="url(#blur28)"/>'
peak = (586, 446)
for k in range(1, 6):
    sc = 1 - k * 0.16
    rnd = random.Random(100 + k)
    ring = stone_pts(peak[0] - 90 * (1 - sc), peak[1] + 120 * (1 - sc), 600 * sc, 440 * sc * 0.92, seed=61, n=2.1, wobble=.03, k=40, taper=.1, flat_bottom=.35)
    b10 += f'<path d="{smooth(ring)}" fill="none" stroke="#E8622A" stroke-width="{12 - k:.0f}" stroke-opacity="{.5 + k*.08:.2f}"/>'
b10 += f'<circle cx="{peak[0]}" cy="{peak[1]}" r="14" fill="#E8622A"/>'
b10 += '</g>'
add("10-contour-stone", "Contour Stone",
    "One smooth river stone with a topographic map cut into it in Trek orange, closing in on the summit. A tactile object, like Apple's best icons, and it says \"trek\" without drawing a mountain.",
    NIGHT + d10, b10 + finish())

import json
json.dump(concepts, open(os.path.join(OUT, "concepts.json"), "w"))
print("\n".join(c[0] for c in concepts))
