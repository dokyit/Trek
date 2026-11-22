#!/usr/bin/env bash
# The channel manifest through a release's publish jobs (.github/workflows/release.yml), as a dry
# run hands it over and as a real run merges it, for the Mac (Apple silicon, then Intel), Windows
# and Linux (x86_64 and Arm). Nothing is built or uploaded: the Mac's and the other builds'
# manifests are fixtures, and script/release-linux.sh runs its --publish-only half without
# --publish.
#
#   bash script/tests/release-manifests.sh
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FIX="$ROOT/script/tests/fixtures"
MERGE="$ROOT/script/lib/merge-manifest.py"
DRY="$ROOT/script/lib/dry-run-manifest.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
fails=0

ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fails=$((fails + 1)); }
field() { # file, python expression over the manifest m
  python3 -I -c 'import json, sys; m = json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"
}
expect() { # what, file, expression, wanted
  if [ "$(field "$2" "$3")" = "$4" ]; then ok "$1"; else bad "$1: $3 is $(field "$2" "$3"), not $4"; fi
}
publish_only() { # what, release dir, existing manifest, log: release-linux.sh's dry half
  if TREK_RELEASE_DIR="$2" "$ROOT/script/release-linux.sh" 0.4.0 --channel stable --publish-only --existing-manifest "$3" > "$4" 2>&1; then
    ok "$1: release-linux.sh --publish-only ran"
  else
    bad "$1: release-linux.sh --publish-only failed: $(cat "$4")"
  fi
}
same_entry() { # what, platform, manifest a, manifest b: the platform's entry is the same in both
  if [ "$(field "$3" "m['platforms']['$2']")" = "$(field "$4" "m['platforms']['$2']")" ]; then ok "$1"; else bad "$1"; fi
}

# The Linux builds' halves, as `release-linux.sh --build-only` leaves them in each architecture's
# folder: the packages (empty here) and the manifest entry.
linux_builds() { # release dir
  for arch in x86_64 aarch64; do
    mkdir -p "$1/0.4.0/linux-$arch"
    deb=amd64; [ "$arch" = aarch64 ] && deb=arm64
    for f in "Trek-0.4.0-linux-$arch.AppImage" "Trek-0.4.0-linux-$arch.AppImage.minisig" \
      "Trek-0.4.0-linux-$arch.tar.gz" "trek_0.4.0_$deb.deb" "Trek-0.4.0-linux-$arch.debug.tar.gz"; do
      : > "$1/0.4.0/linux-$arch/$f"
    done
    python3 -I - "$arch" > "$1/0.4.0/linux-$arch/linux-$arch.json" <<'EOF'
import json, sys
arch = sys.argv[1]
url = f"https://github.com/dokyit/Trek/releases/download/v0.4.0/Trek-0.4.0-linux-{arch}.AppImage"
sig = f"untrusted comment: signature from minisign secret key\nRUTestLinux==\ntrusted comment: Trek 0.4.0 linux-{arch}\nTGludXg="
print(json.dumps({"version": "0.4.0", "platforms": {f"linux-{arch}": {"url": url, "sha256": "e" * 64, "signature": sig}}}, indent=2))
EOF
  done
}

# ---------- a dry run ----------
# The macOS job's release.sh leaves its manifest in a version folder (without the input's "v").
REL="$WORK/dry/dist/release"
mkdir -p "$REL/0.4.0"
cp "$FIX/mac-only.json" "$REL/0.4.0/stable.json"

# Staged for the artifact as one file, so it's at the artifact's top where the publish jobs look.
ART="$WORK/dry/mac-manifest"
"$DRY" stage v0.4.0 stable "$REL" "$ART" > /dev/null
if [ "$(cd "$ART" && find . -type f)" = "./stable.json" ]; then ok "dry run: the artifact holds stable.json at its top, alone"; else bad "dry run: the artifact holds $(cd "$ART" && find . -type f | tr '\n' ' ')"; fi
if cmp -s "$ART/stable.json" "$FIX/mac-only.json"; then ok "dry run: it's the Mac's manifest as release.sh wrote it"; else bad "dry run: the staged manifest differs"; fi
if "$DRY" stage 0.4.1 stable "$REL" "$WORK/dry/none" > /dev/null 2>&1; then bad "dry run: staging a version with no manifest fails"; else ok "dry run: staging a version with no manifest fails"; fi

# Found where the publish jobs look; the layout 0.4.64-beta.1's wildcard upload made is refused,
# saying where the file was, rather than merged into nothing.
mac=$("$DRY" find "$ART" stable)
if [ "$mac" = "$ART/stable.json" ]; then ok "dry run: the publish jobs find it"; else bad "dry run: found $mac"; fi
mkdir -p "$WORK/dry/old/0.4.0"
cp "$FIX/mac-only.json" "$WORK/dry/old/0.4.0/stable.json"
if "$DRY" find "$WORK/dry/old" stable > /dev/null 2> "$WORK/err"; then
  bad "dry run: a manifest one folder down isn't taken for none"
elif grep -q "./0.4.0/stable.json" "$WORK/err"; then
  ok "dry run: a manifest one folder down fails, naming where it is"
else
  bad "dry run: one folder down said: $(cat "$WORK/err")"
fi

# The Intel dry run merges its build into the Mac's manifest (the step that failed).
python3 -I "$MERGE" "$mac" "$FIX/intel-only.json" > "$WORK/dry/intel.json"
expect "dry run, Intel: both Macs" "$WORK/dry/intel.json" "','.join(sorted(m['platforms']))" "darwin-aarch64,darwin-x86_64"
same_entry "dry run, Intel: the Apple silicon entry is the Mac's own" darwin-aarch64 "$WORK/dry/intel.json" "$FIX/mac-only.json"

# The Linux dry run merges both architectures into the Mac's manifest, not into a stand-in.
linux_builds "$REL"
publish_only "dry run, Linux" "$REL" "$mac" "$WORK/dry/linux.log"
expect "dry run, Linux: the Mac's and both Linux entries" "$REL/0.4.0/stable.json" "','.join(sorted(m['platforms']))" "darwin-aarch64,linux-aarch64,linux-x86_64"
same_entry "dry run, Linux: the Apple silicon entry is the Mac's own, not a stand-in" darwin-aarch64 "$REL/0.4.0/stable.json" "$FIX/mac-only.json"
expect "dry run, Linux: the release's notes are the Mac's" "$REL/0.4.0/stable.json" "m['notes']" "$(field "$FIX/mac-only.json" "m['notes']")"
if grep -q "dry run: not publishing" "$WORK/dry/linux.log"; then ok "dry run, Linux: nothing uploaded"; else bad "dry run, Linux: $(cat "$WORK/dry/linux.log")"; fi

# ---------- a real run ----------
# Each publish job downloads the release's manifest, merges its own platforms and uploads it back:
# the Mac's (Apple silicon), then Windows', then Intel's, then Linux's. Windows' merge is
# release.ps1's (script/tests/release-manifest.Tests.ps1); mac-and-windows.json is what it leaves.
REAL="$WORK/real/dist/release"
mkdir -p "$REAL"
python3 -I "$MERGE" "$FIX/mac-and-windows.json" "$FIX/intel-only.json" > "$WORK/real/after-intel.json"
linux_builds "$REAL"
publish_only "real run, Linux" "$REAL" "$WORK/real/after-intel.json" "$WORK/real/linux.log"
final="$REAL/0.4.0/stable.json"
expect "real run: every platform" "$final" "','.join(sorted(m['platforms']))" "darwin-aarch64,darwin-x86_64,linux-aarch64,linux-x86_64,windows-x86_64"
for p in darwin-aarch64 windows-x86_64; do
  same_entry "real run: $p is as the Mac and Windows left it" "$p" "$final" "$FIX/mac-and-windows.json"
done
same_entry "real run: darwin-x86_64 is the Intel build's" darwin-x86_64 "$final" "$FIX/intel-only.json"
for arch in x86_64 aarch64; do
  same_entry "real run: linux-$arch is its build's" "linux-$arch" "$final" "$REAL/0.4.0/linux-$arch/linux-$arch.json"
done
expect "real run: the release's date stays the Mac's" "$final" "m['pub_date']" "$(field "$FIX/mac-and-windows.json" "m['pub_date']")"

# Intel's publish failed: Linux, which runs after it whatever became of it, merges into what the
# Mac and Windows left, and Intel's entry can still join later (a re-run of the failed job).
rm -rf "$REAL/0.4.0/stable.json"
publish_only "real run, Intel failed, Linux" "$REAL" "$FIX/mac-and-windows.json" "$WORK/real/linux-2.log"
expect "real run, Intel failed: Linux joins all the same" "$final" "','.join(sorted(m['platforms']))" "darwin-aarch64,linux-aarch64,linux-x86_64,windows-x86_64"
python3 -I "$MERGE" "$final" "$FIX/intel-only.json" > "$WORK/real/intel-later.json"
expect "real run, Intel failed: its re-run adds it after Linux" "$WORK/real/intel-later.json" "','.join(sorted(m['platforms']))" "darwin-aarch64,darwin-x86_64,linux-aarch64,linux-x86_64,windows-x86_64"

if [ "$fails" -ne 0 ]; then
  echo "$fails failed"
  exit 1
fi
echo "all passed"
