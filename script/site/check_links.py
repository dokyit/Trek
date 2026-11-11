"""Check every link in the site: fragments and files inside it, and that each outside address answers.

check_links.py SITE_DIR [--online]
"""
import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path

site = Path(sys.argv[1])
online = "--online" in sys.argv


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids, self.links, self.assets, self.h1, self.imgs = set(), [], [], 0, []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if "id" in a:
            self.ids.add(a["id"])
        if tag == "a" and "href" in a:
            self.links.append(a["href"])
        if tag == "h1":
            self.h1 += 1
        if tag == "img":
            self.imgs.append(a)
        for key in ("src", "href"):
            if tag in ("img", "script", "link", "use") and key in a:
                self.assets.append(a[key])
        for key in ("srcset",):
            if key in a:
                self.assets += [part.strip().split(" ")[0] for part in a[key].split(",")]
        if tag == "meta" and a.get("property") in ("og:image", "og:url") or tag == "meta" and a.get("name") == "twitter:image":
            self.links.append(a.get("content", ""))


pages = {}
for f in [site / "index.html", site / "privacy" / "index.html", site / "404.html"]:
    p = Page()
    p.feed(f.read_text())
    pages[f] = p

bad = 0
outside = set()


def local(path):
    path = path.split("#")[0].split("?")[0]
    target = site / path.lstrip("/")
    if path.endswith("/") or path == "":
        target = target / "index.html"
    return target


for f, p in pages.items():
    here = "/" + str(f.relative_to(site)).replace("index.html", "")
    if p.h1 != 1:
        print(f"BAD {f.name}: {p.h1} h1"); bad += 1
    for img in p.imgs:
        if "alt" not in img or "width" not in img or "height" not in img:
            print(f"BAD {f.name}: img without alt/width/height: {img.get('src')}"); bad += 1
    for href in p.links + p.assets:
        if href.startswith("mailto:"):
            if href != "mailto:support@trekdesktop.com":
                print(f"BAD mail {href}"); bad += 1
            continue
        if href.startswith("https://trekdesktop.com"):
            href = href[len("https://trekdesktop.com"):] or "/"
        if href.startswith("http"):
            outside.add(href)
            continue
        if href.startswith("#"):
            frag, target_page = href[1:], p
        else:
            t = local(href)
            if not t.exists():
                print(f"BAD {f.name}: missing {href}"); bad += 1
                continue
            frag = href.split("#")[1] if "#" in href else ""
            target_page = pages.get(t)
        if frag and target_page is not None and frag not in target_page.ids and not frag.startswith("i-") and frag not in ("mark", "way", "stack1", "stack2", "stack3", "stack4"):
            print(f"BAD {f.name}: no #{frag}"); bad += 1
        if frag and target_page is not None and (frag.startswith("i-") or frag in ("mark", "way", "stack1", "stack2", "stack3", "stack4")) and frag not in target_page.ids:
            print(f"BAD {f.name}: no symbol #{frag}"); bad += 1

# css urls
css = (site / "assets" / "site.css").read_text()
for u in re.findall(r'url\("([^"]+)"\)', css):
    if not local(u).exists():
        print("BAD css url", u); bad += 1

print("inside: ok" if not bad else f"inside: {bad} bad")
for u in sorted(outside):
    if not online:
        print("  outside", u)
        continue
    code = subprocess.run(["curl", "-s", "-o", "/dev/null", "-L", "-I", "-w", "%{http_code}", "--max-time", "25", "-A", "Mozilla/5.0 (link check)", u], capture_output=True, text=True).stdout
    print(f"  {code} {u}")
