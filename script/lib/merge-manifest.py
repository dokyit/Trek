#!/usr/bin/env python3
"""Add a build's platform to a release's channel manifest, for the Intel half of a release.

    merge-manifest.py <the release's <channel>.json> <the build's <channel>.json>

Prints the release's manifest with the build's platforms added or replaced, everything else as it
was: the other platforms, the notes, the date. The two must be for one version, so users are never
pointed at another version's archive. Tests: script/tests/merge-manifest.sh.
"""
import json
import sys


def merge(release, build):
    if release.get("version") != build.get("version"):
        raise SystemExit(
            f"merge-manifest: the release is {release.get('version')}, this build is {build.get('version')}"
        )
    if not build.get("platforms"):
        raise SystemExit("merge-manifest: the build's manifest lists no platform")
    if not any(p.startswith("darwin-") for p in release.get("platforms", {})):
        raise SystemExit("merge-manifest: the release's manifest has no darwin-* entry: did the Mac publish?")
    return {**release, "platforms": {**release["platforms"], **build["platforms"]}}


def main(argv):
    if len(argv) != 3:
        raise SystemExit(__doc__)
    with open(argv[1], encoding="utf-8") as f:
        release = json.load(f)
    with open(argv[2], encoding="utf-8") as f:
        build = json.load(f)
    print(json.dumps(merge(release, build), indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main(sys.argv)
