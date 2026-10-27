#!/bin/zsh
# Pack a built Trek.app into the disk image Mac users download: a window with Trek.app on the
# left, an Applications link on the right and a background that says to drag one onto the other
# (and what to do when macOS won't open Trek the first time). Called by script/release.sh after
# script/bundle.sh; runs on its own too:
#
#   script/dmg.sh <Trek.app> <out.dmg>
#
# Built with dmgbuild (no Finder scripting, so it works on a headless CI runner), installed into
# a virtualenv of its own from script/lib/dmgbuild-requirements.txt, pinned by hash. Layout:
# script/lib/dmg-settings.py; background: assets/dmg/ (script/dmg-background.sh renders it).
#
# Signing: only a Developer ID identity signs the image (script/lib/sign.sh's
# trek_is_developer_id), with a secure timestamp, ready for notarization (script/notarize.sh).
# A disk image signed by a certificate macOS doesn't trust is assessed by Gatekeeper and turned
# away when it's opened, a second "Open Anyway" before the app's own; unsigned, it just mounts,
# and the app inside carries the signature that matters (docs/RELEASING.md, "Gatekeeper").
#
# Environment:
#   TREK_DMGBUILD        a dmgbuild to use instead of the pinned one
#   TREK_DMGBUILD_VENV   where the pinned one is installed (default: ~/Library/Caches/trek/dmgbuild-<hash>)
#   TREK_SIGN_IDENTITY   code-signing identity (see script/lib/sign.sh)
set -euo pipefail
SELF=${0:A}
ROOT=${SELF:h:h}
source "$ROOT/script/lib/sign.sh"

die() { echo "dmg: $*" >&2; exit 1; }
(( $# == 2 )) || { sed -n '6,8p' "$SELF" | sed 's/^# \{0,1\}//'; exit 2; }
APP=${1:A} DMG=${2:A}
[[ -d $APP/Contents ]] || die "no app bundle at $1"
ICON="$APP/Contents/Resources/AppIcon.icns"
[[ -f $ICON ]] || die "$APP has no AppIcon.icns"
BACKGROUND="$ROOT/assets/dmg/background.png"
[[ -f $BACKGROUND && -f ${BACKGROUND:r}@2x.png ]] || die "no background in assets/dmg (script/dmg-background.sh)"

# ---------- dmgbuild ----------
REQS="$ROOT/script/lib/dmgbuild-requirements.txt"
if [[ -n ${TREK_DMGBUILD:-} ]]; then
  DMGBUILD=$TREK_DMGBUILD
else
  VENV=${TREK_DMGBUILD_VENV:-$HOME/Library/Caches/trek/dmgbuild-$(shasum -a 256 "$REQS" | cut -c1-12)}
  DMGBUILD="$VENV/bin/dmgbuild"
  if [[ ! -x $DMGBUILD ]]; then
    python3 -c 'import sys; sys.exit(sys.version_info < (3, 10))' 2>/dev/null \
      || die "dmgbuild needs Python 3.10 or later; python3 is $(python3 --version 2>&1) (brew install python)"
    echo "• installing dmgbuild into $VENV"
    rm -rf "$VENV"
    python3 -m venv "$VENV"
    "$VENV/bin/python3" -m pip install -q --disable-pip-version-check \
      --require-hashes --only-binary :all: -r "$REQS"
  fi
fi

# ---------- image ----------
# dmgbuild makes the image in the output's folder; a half-written one from an earlier run goes.
mkdir -p "${DMG:h}"
rm -f "$DMG"
# A CI runner's Spotlight or Finder can hold the scratch volume for a moment: detach patiently.
"$DMGBUILD" --detach-retries 10 -s "$ROOT/script/lib/dmg-settings.py" \
  -D app="$APP" -D background="$BACKGROUND" -D icon="$ICON" \
  Trek "$DMG" >/dev/null
hdiutil verify -quiet "$DMG" || die "hdiutil can't verify $DMG"

# ---------- signature ----------
ID=$(trek_sign_identity)
if trek_is_developer_id "$ID"; then
  codesign --force --timestamp --sign "$ID" "$DMG"
  codesign --verify --strict "$DMG"
  echo "• ${DMG:t}  signed with \"$ID\""
else
  echo "• ${DMG:t}  left unsigned (\"$ID\" isn't a Developer ID: macOS would refuse a disk image it signed)"
fi
