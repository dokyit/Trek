#!/usr/bin/env bash
# The Mac's <channel>.json for a dry run's publish jobs (.github/workflows/release.yml). A real
# run's Windows, Intel and Linux publish jobs merge into the manifest the Mac put on the release;
# a dry run puts nothing there, so the macOS job hands its manifest over as the mac-manifest
# artifact instead, and each dry run merges into that.
#
#   script/lib/dry-run-manifest.sh stage <version> <channel> <release dir> <to dir>
#       Copies <release dir>/<version, without a "v">/<channel>.json to <to dir>/<channel>.json,
#       the file the artifact uploads. One file named outright, so it lands at the artifact's
#       top: upload-artifact keeps the folders after a wildcard, and dist/release/*/beta.json went
#       up as 0.4.64-beta.1/beta.json, where no job looked.
#   script/lib/dry-run-manifest.sh find <artifact dir> <channel>
#       Prints the manifest's path in the downloaded artifact, or fails saying it isn't there: a
#       dry run that merged into nothing would pass without trying the merge it's there to try.
#
# Tests: script/tests/release-manifests.sh.
set -euo pipefail

die() { echo "dry-run-manifest: $*" >&2; exit 1; }

case "${1:-}" in
  stage)
    [[ $# -eq 5 ]] || die "usage: stage <version> <channel> <release dir> <to dir>"
    from="$4/${2#v}/$3.json"
    [[ -f $from ]] || die "no $from: the Mac's release.sh writes it"
    mkdir -p "$5"
    cp "$from" "$5/$3.json"
    echo "$5/$3.json"
    ;;
  find)
    [[ $# -eq 3 ]] || die "usage: find <artifact dir> <channel>"
    manifest="$2/$3.json"
    if [[ ! -f $manifest ]]; then
      found=$(cd "$2" 2>/dev/null && find . -name "$3.json" | head -3 | tr '\n' ' ')
      die "the mac-manifest artifact has no $3.json at its top (it has: ${found:-nothing}); the macOS job's \"Manifest for the dry runs\" stages it there"
    fi
    echo "$manifest"
    ;;
  *) die "stage or find (see the top of this file)" ;;
esac
