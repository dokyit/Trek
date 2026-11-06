#!/usr/bin/env bash
# Tests for script/lib/merge-manifest.py, which adds the Intel build to a release's manifest:
#
#   bash script/tests/merge-manifest.sh
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FIX="$ROOT/script/tests/fixtures"
MERGE="$ROOT/script/lib/merge-manifest.py"
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

# Added after the Mac's and Windows' entries, which stay exactly as they were.
python3 -I "$MERGE" "$FIX/mac-and-windows.json" "$FIX/intel-only.json" > "$WORK/all.json"
expect "all three platforms" "$WORK/all.json" "','.join(sorted(m['platforms']))" "darwin-aarch64,darwin-x86_64,windows-x86_64"
expect "the Intel entry names its archive" "$WORK/all.json" "m['platforms']['darwin-x86_64']['url'].rsplit('/', 1)[1]" "Trek-0.4.0-darwin-x86_64.app.tar.gz"
expect "its signature is its own" "$WORK/all.json" "'darwin-x86_64' in m['platforms']['darwin-x86_64']['signature']" "True"
for key in darwin-aarch64 windows-x86_64; do
  a=$(field "$FIX/mac-and-windows.json" "m['platforms']['$key']")
  b=$(field "$WORK/all.json" "m['platforms']['$key']")
  if [ "$a" = "$b" ]; then ok "$key is untouched"; else bad "$key changed"; fi
done
# The release's notes and date stay; the build's own copy goes.
expect "the release's notes stay" "$WORK/all.json" "m['notes']" "$(field "$FIX/mac-and-windows.json" "m['notes']")"
expect "the release's date stays" "$WORK/all.json" "m['pub_date']" "$(field "$FIX/mac-and-windows.json" "m['pub_date']")"

# Run again (a re-run job): replaced, not doubled.
python3 -I "$MERGE" "$WORK/all.json" "$FIX/intel-only.json" > "$WORK/again.json"
if cmp -s "$WORK/all.json" "$WORK/again.json"; then ok "a second run changes nothing"; else bad "a second run changed it"; fi

# Refused: another version's archive would be offered, or the Mac's release isn't out.
if python3 -I "$MERGE" "$FIX/wrong-version.json" "$FIX/intel-only.json" > /dev/null 2> "$WORK/err"; then
  bad "another version is refused"
elif grep -q "the release is 0.3.9, this build is 0.4.0" "$WORK/err"; then
  ok "another version is refused, and it says which"
else
  bad "another version: $(cat "$WORK/err")"
fi
printf '{"version":"0.4.0","platforms":{"windows-x86_64":{}}}\n' > "$WORK/no-mac.json"
if python3 -I "$MERGE" "$WORK/no-mac.json" "$FIX/intel-only.json" > /dev/null 2>&1; then
  bad "a manifest with no Mac entry is refused"
else
  ok "a manifest with no Mac entry is refused"
fi

if [ "$fails" -ne 0 ]; then
  echo "$fails failed"
  exit 1
fi
echo "all passed"
