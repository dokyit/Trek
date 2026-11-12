"""Stamp every link to one of the site's own files with a short hash of that file.

stamp.py SITE_DIR [--check]

The site has no build step, so a stylesheet, script or picture keeps its name when it changes, and
Cloudflare and browsers keep the copy they have under that name. A page then arrives with the last
version's stylesheet. With `?v=<hash>` on the link, a changed file is a new address and is fetched
at once; an unchanged one keeps its address and its cached copy.

Run it after changing anything under `site/`. `--check` changes nothing and fails when a stamp is
missing or out of date (the pre-push hook runs it).
"""
import hashlib
import re
import sys
from pathlib import Path

site = Path(sys.argv[1])
check = "--check" in sys.argv

# A link to a file of the site's own: after a quote, a bracket, a space, a comma or the site's
# address, with the stamp it may already carry.
REF = re.compile(
    r"""(?:(?<=["'(\s,=])|(?<=https://trekdesktop\.com))"""
    r"""(/(?:assets/[\w./-]+\.\w+|favicon\.ico|favicon\.svg|apple-touch-icon\.png|site\.webmanifest))"""
    r"""(?:\?v=[0-9a-f]{8})?"""
)


def stamp(text):
    def one(m):
        f = site / m.group(1).lstrip("/")
        if not f.is_file():
            return m.group(0)  # check_links.py reports a link to nothing
        return f"{m.group(1)}?v={hashlib.sha256(f.read_bytes()).hexdigest()[:8]}"

    return REF.sub(one, text)


# In this order: the stylesheet and the manifest link to pictures, and the pages link to them.
pages = [f for f in sorted(site.rglob("*.html")) if not any(part.startswith(".") for part in f.relative_to(site).parts)]
files = [site / "assets" / "site.css", site / "site.webmanifest"] + pages
stale = []
for f in files:
    before = f.read_text()
    after = stamp(before)
    if after != before:
        stale.append(str(f.relative_to(site)))
        if not check:
            f.write_text(after)

if check and stale:
    print("stale stamps in: " + ", ".join(stale) + f"  (run: python3 script/site/stamp.py {site})")
    sys.exit(1)
print(("stamps are current" if not stale else "stamped: " + ", ".join(stale)))
