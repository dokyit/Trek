"""The home page's drawings: one small object beside a feature, all in one language: line drawings of
blocks in isometric projection (30 degrees), one ink line on the page's own light, and one ember line
for the thing that needs you. Hidden edges are hidden (each face is filled with the page's light
at that place, so a block in front covers what is behind it); no shading, no shadows, no fills of
colour, no icons, no people.

make_illustrations.py SITE_DIR
    writes each drawing into SITE_DIR/index.html between <!-- ill:NAME --> and <!-- /ill:NAME -->.

The drawings are inline SVG so the page's stylesheet gives them their colours (each theme, and the
light at their place in the page's afternoon) and draws their lines in once as they arrive. Faces are
paths with the class e (an edge that draws in; ae for an ember one, dash for a dashed one), with
pathLength 1 for the draw-in. Everything is decorative: aria-hidden, no text.
"""
import math, re, sys
from pathlib import Path

C, S = math.cos(math.radians(30)), 0.5
EYE = (1, 1, 1)  # the faces whose outward normal points this way are seen


def proj(p, k):
    x, y, z = p
    return ((x - y) * C * k, ((x + y) * S - z) * k)


def rot(p, about, axis, deg):
    """Rotate point p about a pivot, around the x, y or z axis."""
    a = math.radians(deg)
    x, y, z = (p[i] - about[i] for i in range(3))
    c, s = math.cos(a), math.sin(a)
    if axis == "z":
        x, y = x * c - y * s, x * s + y * c
    elif axis == "y":
        x, z = x * c + z * s, -x * s + z * c
    else:
        y, z = y * c - z * s, y * s + z * c
    return (x + about[0], y + about[1], z + about[2])


class Drawing:
    def __init__(self, name, scale=2.0):
        self.name, self.k, self.items, self.pts = name, scale, [], []

    def face(self, pts3, cls):
        pts = [proj(p, self.k) for p in pts3]
        self.pts += pts
        d = "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in pts) + " Z"
        # pathLength 1 lets a line draw itself in; a dashed line keeps real lengths for its dashes
        self.items.append(f'<path class="{cls}" d="{d}"' + ("" if cls == "dash" else ' pathLength="1"') + "/>")

    def box(self, x, y, z, w, d, h, cls="e", turn=None):
        """A block; turn = (pivot, axis, degrees) to rotate it. Only the faces seen are drawn,
        far ones first."""
        P = [(x, y, z), (x + w, y, z), (x + w, y + d, z), (x, y + d, z),
             (x, y, z + h), (x + w, y, z + h), (x + w, y + d, z + h), (x, y + d, z + h)]
        if turn:
            P = [rot(p, *turn) for p in P]
        faces = [(0, 1, 2, 3), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
        cx = [sum(p[i] for p in P) / 8 for i in range(3)]
        seen = []
        for f in faces:
            a, b, c = P[f[0]], P[f[1]], P[f[2]]
            u = [b[i] - a[i] for i in range(3)]
            v = [c[i] - a[i] for i in range(3)]
            n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
            m = [sum(P[j][i] for j in f) / 4 for i in range(3)]
            if sum(n[i] * (m[i] - cx[i]) for i in range(3)) < 0:
                n = [-t for t in n]  # outward
            if sum(n[i] * EYE[i] for i in range(3)) > 1e-6:
                seen.append(([P[j] for j in f], sum(m)))
        for pts, _ in sorted(seen, key=lambda s: s[1]):
            self.face(pts, cls)

    def prism(self, plan, z, h, cls="e"):
        """A block of any plan (points on the ground, in order) standing h high: its seen sides,
        far ones first, then its top."""
        n = len(plan)
        sides = []
        cxp = sum(p[0] for p in plan) / n, sum(p[1] for p in plan) / n
        for i in range(n):
            (x0, y0), (x1, y1) = plan[i], plan[(i + 1) % n]
            nx, ny = y1 - y0, -(x1 - x0)  # a normal of the edge in plan
            mx, my = (x0 + x1) / 2, (y0 + y1) / 2
            if nx * (mx - cxp[0]) + ny * (my - cxp[1]) < 0:
                nx, ny = -nx, -ny
            if nx + ny > 1e-6:  # faces the eye
                sides.append(([(x0, y0, z), (x1, y1, z), (x1, y1, z + h), (x0, y0, z + h)], mx + my))
        for pts, _ in sorted(sides, key=lambda t: t[1]):
            self.face(pts, cls)
        self.face([(x, y, z + h) for x, y in plan], cls)

    def svg(self, pad=6):
        xs, ys = [p[0] for p in self.pts], [p[1] for p in self.pts]
        x0, y0, x1, y1 = min(xs) - pad, min(ys) - pad, max(xs) + pad, max(ys) + pad
        w, h = x1 - x0, y1 - y0
        return (f'<svg class="ill ill-{self.name}" viewBox="{x0:.0f} {y0:.0f} {w:.0f} {h:.0f}" width="{w:.0f}" height="{h:.0f}" aria-hidden="true" focusable="false">'
                + "".join(self.items) + "</svg>")


def inbox():
    """One inbox: a quiet stack of threads, and the one that needs you lifted clear of it, tipped
    toward you and drawn in ember."""
    D = Drawing("inbox", scale=2.6)
    for i in range(5):
        D.box(0, 0, i * 5, 64, 44, 5)
    # lifted clear of the stack (air all round under it), pulled forward toward you, and tipped
    # 10 degrees, its near end down
    D.box(14, 8, 62, 64, 44, 5, cls="ae", turn=((14, 8, 62), "y", 10))
    return D


def worktrees():
    """Worktrees: the thread's work as one long beam; a second beam leaves its side, turns and runs
    beside it on its own line, in ember: the same work on a branch of its own."""
    D = Drawing("worktrees")
    D.box(0, 0, 0, 150, 14, 10)                                     # the main line
    # the branch leaves its side at the same height and turns to run beside it: one piece
    D.prism([(34, 14), (48, 14), (48, 30), (150, 30), (150, 44), (34, 44)], 0, 10, cls="ae")
    return D


def study():
    """Study: learning built on what you already know: two solid blocks, and the next one only
    drawn in, in ember, waiting to be made."""
    D = Drawing("study")
    D.box(0, 0, 0, 50, 50, 26)
    D.box(10, 10, 26, 30, 30, 20)
    D.box(17, 17, 46, 16, 16, 18, cls="dash")
    return D


ALL = [inbox, worktrees, study]

if __name__ == "__main__":
    page = Path(sys.argv[1]) / "index.html"
    s = page.read_text()
    for f in ALL:
        d = f()
        svg = d.svg()
        pat = re.compile(rf"<!-- ill:{d.name} -->.*?<!-- /ill:{d.name} -->", re.S)
        if not pat.search(s):
            sys.exit(f"no <!-- ill:{d.name} --> marker in {page}")
        s = pat.sub(lambda m: f"<!-- ill:{d.name} -->{svg}<!-- /ill:{d.name} -->", s)
        print(f"{d.name:10s} {len(svg):5d} bytes")
    page.write_text(s)
