#!/bin/zsh
# Notarize a Developer ID-signed Trek.app or disk image with Apple and staple the ticket to it,
# so Gatekeeper opens it without the "Open Anyway" detour, offline too (docs/RELEASING.md,
# "Gatekeeper and notarization"). script/release.sh calls it for the app (before it's archived)
# and for the disk image, when the three TREK_NOTARY_* variables are set.
#
#   script/notarize.sh <Trek.app | Trek.dmg>
#
# Environment (an App Store Connect API key, Users and Access › Integrations › Team Keys, with
# the Developer role):
#   TREK_NOTARY_KEY      path to the key's AuthKey_<id>.p8 file
#   TREK_NOTARY_KEY_ID   its key ID
#   TREK_NOTARY_ISSUER   the team's issuer ID
set -euo pipefail
die() { echo "notarize: $*" >&2; exit 1; }
(( $# == 1 )) || { sed -n '7p' "${0:A}" | sed 's/^# \{0,1\}//'; exit 2; }
TARGET=${1:A}
[[ -e $TARGET ]] || die "nothing at $1"
[[ -f ${TREK_NOTARY_KEY:-} && -n ${TREK_NOTARY_KEY_ID:-} && -n ${TREK_NOTARY_ISSUER:-} ]] \
  || die "set TREK_NOTARY_KEY (a .p8 file), TREK_NOTARY_KEY_ID and TREK_NOTARY_ISSUER"
AUTH=(--key "$TREK_NOTARY_KEY" --key-id "$TREK_NOTARY_KEY_ID" --issuer "$TREK_NOTARY_ISSUER")

# The notary service takes a zip, a disk image or a package: an app goes up zipped (ditto keeps
# the bundle's symlinks and signature), and the ticket is stapled to the app itself.
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
case $TARGET in
  *.app)
    UPLOAD="$WORK/${TARGET:t:r}.zip"
    ditto -c -k --keepParent "$TARGET" "$UPLOAD" ;;
  *.dmg) UPLOAD=$TARGET ;;
  *) die "$1 is neither an .app nor a .dmg" ;;
esac

echo "• notarizing ${TARGET:t} (this takes a few minutes)"
# --wait polls until Apple has an answer; the JSON names the submission and its verdict.
xcrun notarytool submit "$UPLOAD" "${AUTH[@]}" --wait --timeout 30m --output-format json \
  > "$WORK/submit.json" || true
ID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id", ""))' "$WORK/submit.json" 2>/dev/null || true)
STATUS=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status", ""))' "$WORK/submit.json" 2>/dev/null || true)
if [[ $STATUS != Accepted ]]; then
  cat "$WORK/submit.json" >&2 || true
  # The log says which file failed which check (an unsigned helper, no hardened runtime, …).
  [[ -z $ID ]] || xcrun notarytool log "$ID" "${AUTH[@]}" >&2 || true
  die "Apple didn't accept ${TARGET:t} (${STATUS:-no answer})"
fi
xcrun stapler staple -q "$TARGET"
xcrun stapler validate -q "$TARGET"
echo "• ${TARGET:t} notarized ($ID) and stapled"
