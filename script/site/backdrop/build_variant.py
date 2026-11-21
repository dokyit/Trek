"""A preview copy of site/ with the "one afternoon" backdrop (backdrop.css), for judging it before it
goes on the live page. Nothing here changes site/.

build_variant.py IMG_DIR OUT_DIR
  IMG_DIR  the files make_backdrops.py encoded (hill-* and dusk-*)
  OUT_DIR  a fresh copy of site/ with the backdrop: serve it with
           node script/site/serve.mjs 8084 OUT_DIR
"""
import re, shutil, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
IMG, OUT = Path(sys.argv[1]), Path(sys.argv[2])
FMTS = ("avif", "webp", "jpg")
TYPE = {"avif": "image/avif", "webp": "image/webp", "jpg": "image/jpeg"}
WIDTHS = {"wide": (1280, 1920, 2560), "tall": (720, 1080)}
FRAME = (("tall", "(max-width: 700px)"), ("wide", "(min-width: 701px)"))


def srcset(name, fr, grade, ext):
    return ", ".join(f"/assets/img/backdrop/{name}-{fr}-{grade}-{w}.{ext} {w}w" for w in WIDTHS[fr])


def picture(cls, name, light, dark, lazy):
    """Paper and Night grades, a wide frame for a desktop and a tall one for a phone, AVIF then WebP
    then the JPEG in the <img>."""
    out = [f'<picture class="{cls}" aria-hidden="true">']
    for grade, scheme in ((dark, "(prefers-color-scheme: dark)"), (light, "(prefers-color-scheme: light)")):
        for fr, mq in FRAME:
            for ext in FMTS:
                if grade == light and scheme.endswith("light)") and fr == "wide" and ext == "jpg":
                    continue  # that one is the <img>
                out.append(f'  <source media="{scheme} and {mq}" type="{TYPE[ext]}" srcset="{srcset(name, fr, grade, ext)}" sizes="100vw">')
    load = 'loading="lazy"' if lazy else 'fetchpriority="high"'
    out.append(f'  <img src="/assets/img/backdrop/{name}-wide-{light}-1920.jpg" srcset="{srcset(name, "wide", light, "jpg")}" sizes="100vw" width="2560" height="1600" alt="" {load} decoding="async">')
    out.append("</picture>")
    return "\n".join(out)


def preloads():
    """The first screen's picture, in the format every current browser takes, preloaded."""
    return "\n".join(
        f'<link rel="preload" as="image" type="image/avif" media="{scheme} and {mq}" imagesrcset="{srcset("hill", fr, grade, "avif")}" imagesizes="100vw">'
        for grade, scheme in (("night", "(prefers-color-scheme: dark)"), ("day", "(prefers-color-scheme: light)"))
        for fr, mq in FRAME)


if OUT.exists():
    shutil.rmtree(OUT)
shutil.copytree(REPO / "site", OUT, ignore=shutil.ignore_patterns(".impeccable"))
img = OUT / "assets" / "img" / "backdrop"
img.mkdir(parents=True)
for f in [*IMG.glob("hill-*"), *IMG.glob("dusk-*")]:
    if f.suffix.lstrip(".") in FMTS:
        shutil.copy(f, img / f.name)
shutil.copy(HERE / "grain.svg", img / "grain.svg")
shutil.copy(HERE / "backdrop.css", OUT / "assets" / "backdrop.css")

page = OUT / "index.html"
s = page.read_text()
s = re.sub(r'(<link rel="stylesheet" href="/assets/site.css\?v=\w+">)', lambda m: m.group(1) + '\n<link rel="stylesheet" href="/assets/backdrop.css">\n' + preloads(), s, count=1)
for old, new in (
    ("<body>", '<body class="bd-hill">'),
    ('<section class="hero" data-hero>\n', '<section class="hero" data-hero>\n' + picture("scene", "hill", "day", "night", False) + "\n"),
    ('<footer class="foot">\n', '<footer class="foot">\n' + picture("closing-land", "dusk", "dusk", "night", True) + "\n"),
):
    assert old in s, old
    s = s.replace(old, new, 1)
page.write_text(s)
subprocess.run([sys.executable, str(REPO / "script/site/stamp.py"), str(OUT)], check=True, capture_output=True)
print(OUT, "built")
