#!/usr/bin/env python3
"""Import Catppuccin's file icons (https://github.com/catppuccin/zed-icons, MIT) for Trek.

    git clone --depth 1 https://github.com/catppuccin/zed-icons /tmp/catppuccin-zed-icons
    script/file-icons.py /tmp/catppuccin-zed-icons

Writes, for the Mac, crates/trek-app/assets/file-icons/{mocha,latte}/<icon>.svg and map.json; for
the iPhone, ios/Trek/FileIcons.xcassets (one image set per icon, Latte for light and Mocha for
dark) and ios/Trek/Resources/file-icons.json. Mocha goes with Night and Latte with Paper.

The SVGs are drawn at 16 points; their width and height are raised to 64 (the view box stays) so
the Mac, which rasterises an SVG once at its own size, has enough pixels for Retina.
"""

import json
import os
import re
import shutil
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAC = os.path.join(ROOT, "crates/trek-app/assets/file-icons")
IOS_ASSETS = os.path.join(ROOT, "ios/Trek/FileIcons.xcassets")
IOS_MAP = os.path.join(ROOT, "ios/Trek/Resources/file-icons.json")
FLAVOURS = {"mocha": "dark", "latte": "light"}
SIZE = 64


def icon_name(path):
    """`./icons/latte/folder_src.svg` -> `folder_src`."""
    return os.path.splitext(os.path.basename(path))[0]


def main(src):
    themes = json.load(open(os.path.join(src, "icon_themes/catppuccin-icons.json")))["themes"]
    latte = next(t for t in themes if t["name"] == "Catppuccin Latte")
    icons = {k: icon_name(v["path"]) for k, v in latte["file_icons"].items()}
    folders = {name: icon_name(v["collapsed"]) for name, v in latte["named_directory_icons"].items()}
    mapping = {
        # Matched against a file's whole name first, then its suffixes, longest first
        # (`a.component.ts`: `component.ts`, then `ts`).
        "names": {k: icons[v] for k, v in latte["file_stems"].items() if v in icons},
        "suffixes": {k: icons[v] for k, v in latte["file_suffixes"].items() if v in icons},
        # A folder's name -> its icon; `<icon>_open` is the open one.
        "folders": folders,
    }
    used = set(mapping["names"].values()) | set(mapping["suffixes"].values())
    used |= set(folders.values()) | {f + "_open" for f in folders.values()}
    used |= {"_file", "_folder", "_folder_open"}

    shutil.rmtree(MAC, ignore_errors=True)
    shutil.rmtree(IOS_ASSETS, ignore_errors=True)
    os.makedirs(IOS_ASSETS)
    json.dump({"info": {"author": "xcode", "version": 1}}, open(os.path.join(IOS_ASSETS, "Contents.json"), "w"), indent=2)
    missing = []
    for name in sorted(used):
        sources = {f: os.path.join(src, "icons", f, name + ".svg") for f in FLAVOURS}
        if not all(os.path.exists(p) for p in sources.values()):
            missing.append(name)
            continue
        imageset = os.path.join(IOS_ASSETS, f"fi-{name}.imageset")
        os.makedirs(imageset)
        images = []
        for flavour, appearance in FLAVOURS.items():
            svg = open(sources[flavour]).read()
            big = re.sub(r'(<svg[^>]*?)\swidth="16"\sheight="16"', rf'\1 width="{SIZE}" height="{SIZE}"', svg, count=1)
            os.makedirs(os.path.join(MAC, flavour), exist_ok=True)
            open(os.path.join(MAC, flavour, name + ".svg"), "w").write(big)
            # iOS keeps the 16-point original: the asset catalog scales vectors itself.
            open(os.path.join(imageset, f"{flavour}.svg"), "w").write(svg)
            image = {"filename": f"{flavour}.svg", "idiom": "universal"}
            if appearance == "dark":
                image["appearances"] = [{"appearance": "luminosity", "value": "dark"}]
            images.append(image)
        json.dump(
            {"images": images, "info": {"author": "xcode", "version": 1}, "properties": {"preserves-vector-representation": True}},
            open(os.path.join(imageset, "Contents.json"), "w"),
            indent=2,
        )
    known = used - set(missing)
    for key in ("names", "suffixes"):
        mapping[key] = {k: v for k, v in mapping[key].items() if v in known}
    mapping["folders"] = {k: v for k, v in mapping["folders"].items() if v in known and v + "_open" in known}
    text = json.dumps(mapping, separators=(",", ":"), sort_keys=True)
    open(os.path.join(MAC, "map.json"), "w").write(text)
    os.makedirs(os.path.dirname(IOS_MAP), exist_ok=True)
    open(IOS_MAP, "w").write(text)
    shutil.copy(os.path.join(src, "LICENSE"), os.path.join(MAC, "LICENSE"))
    print(f"{len(known)} icons, {len(mapping['names'])} names, {len(mapping['suffixes'])} suffixes, {len(mapping['folders'])} folders")
    if missing:
        print("missing:", ", ".join(missing))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
