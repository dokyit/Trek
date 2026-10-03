#!/bin/zsh
# Publish a Trek release on GitHub (see docs/RELEASING.md).
#
#   script/release.sh <version> [--channel stable|beta|nightly] [--notes FILE] [--dry-run]
#
# Sets the version, builds and signs Trek.app (script/bundle.sh), packs it as
# Trek-<version>-darwin-<arch>.app.tar.gz, signs that with minisign, writes the channel manifest
# (<channel>.json) and release notes, then tags, pushes and creates the GitHub release. --dry-run
# stops before git and GitHub and puts the version back afterwards; everything it built stays in
# dist/release/<version>/.
#
# Channels: stable → release v<version> (marked latest); beta / nightly → the prerelease under the
# fixed tag "beta" / "nightly", whose assets are replaced each time. Beta and nightly versions
# carry a matching pre-release suffix (0.3.0-beta.1, 0.3.0-nightly.20261002).
#
# Only a stable release moves the branch: it commits "Release v<version>" there. A beta or nightly
# is built from HEAD with the version set for the build only; the sources it was built from are
# recorded as a commit off the branch (HEAD plus the version bump) that its tags point at, and the
# working tree goes back to the workspace version. Betas are tagged v<version>; nightlies only move
# the "nightly" tag, whose message names the version. A version must be newer than the last
# release of its own channel and of the channels its users also get (beta: stable; nightly: stable
# and beta), and not older than the workspace version (the last stable).
#
# Environment:
#   TREK_MINISIGN_KEY          secret key (default ~/.trek-signing/minisign.key, no password)
#   TREK_RELEASE_REPO          GitHub repository (default dokyit/Trek)
#   TREK_RELEASE_DOWNLOAD_URL  where the manifest says the archive lives (default: the GitHub
#                              release); for testing against a local server
#   TREK_SIGN_IDENTITY         code-signing identity (see script/lib/sign.sh)
set -euo pipefail
SELF=${0:A}
cd "${SELF:h}/.."
source script/lib/sign.sh
source "$HOME/.cargo/env" 2>/dev/null || true

die() { echo "release: $*" >&2; exit 1; }
usage() { sed -n '2,4p' "$SELF" | sed 's/^# \{0,1\}//'; }

VERSION="" CHANNEL=stable DRY=0 NOTES_FILE=""
while (( $# )); do
  case $1 in
    --channel) CHANNEL=${2:-}; shift 2 ;;
    --channel=*) CHANNEL=${1#*=}; shift ;;
    --notes) NOTES_FILE=${2:-}; shift 2 ;;
    --notes=*) NOTES_FILE=${1#*=}; shift ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) [[ -z $VERSION ]] || die "one version, please"; VERSION=${1#v}; shift ;;
  esac
done
[[ -n $VERSION ]] || { usage; exit 2; }
[[ $VERSION =~ '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' ]] || die "$VERSION is not a semver version"
PRE=""; [[ $VERSION == *-* ]] && PRE=${VERSION#*-}
case $CHANNEL in
  stable) [[ -z $PRE ]] || die "stable versions have no pre-release suffix ($VERSION)" ;;
  beta|nightly) [[ $PRE == $CHANNEL* ]] || die "$CHANNEL versions look like 1.2.3-$CHANNEL.N ($VERSION)" ;;
  *) die "channel is stable, beta or nightly" ;;
esac
[[ -z $NOTES_FILE || -f $NOTES_FILE ]] || die "no notes file at $NOTES_FILE"

REPO=${TREK_RELEASE_REPO:-dokyit/Trek}
KEY=${TREK_MINISIGN_KEY:-$HOME/.trek-signing/minisign.key}
ARCH=$(uname -m); [[ $ARCH == arm64 ]] && ARCH=aarch64
PLATFORM="darwin-$ARCH"
NAME="Trek-$VERSION-$PLATFORM.app.tar.gz"
if [[ $CHANNEL == stable ]]; then RELEASE_TAG="v$VERSION"; else RELEASE_TAG=$CHANNEL; fi
# Nightlies don't get a version tag of their own; they'd bury the real ones.
VERSION_TAG=""; [[ $CHANNEL != nightly ]] && VERSION_TAG="v$VERSION"
OUT="dist/release/$VERSION"

command -v minisign >/dev/null || die "minisign isn't installed (brew install minisign)"
[[ -f $KEY ]] || die "no minisign secret key at $KEY (docs/RELEASING.md)"
if (( ! DRY )); then
  # Tags others pushed, and where the channel tags moved: the version checks below read them.
  git fetch -q --force origin 'refs/tags/*:refs/tags/*' || die "couldn't fetch tags from origin"
fi
[[ -z $VERSION_TAG ]] || ! git rev-parse -q --verify "refs/tags/$VERSION_TAG" >/dev/null || die "tag $VERSION_TAG already exists"
CURRENT=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
LAST_NIGHTLY=$(git for-each-ref --format='%(contents:subject)' refs/tags/nightly | sed -n 's/^Trek //p')
python3 - "$CHANNEL" "$VERSION" "$CURRENT" "$LAST_NIGHTLY" $(git tag --list 'v[0-9]*') <<'PY' || exit 1
import re, sys
channel, new, current, last_nightly, *tags = sys.argv[1:]
def key(v):
    core, _, pre = v.partition("-")
    nums = tuple(int(x) for x in core.split("."))
    # semver: a pre-release sorts before its release; identifiers compare numerically or lexically
    ids = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split(".")) if pre else ()
    return (nums, 0 if pre else 1, ids)
released = [t[1:] for t in tags if re.fullmatch(r"v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", t)]
latest = lambda vs: max(vs, key=key, default="")
floors = [("the latest stable release", latest([v for v in released if "-" not in v]))]
if channel in ("beta", "nightly"):
    floors.append(("the latest beta", latest([v for v in released if "-beta" in v])))
if channel == "nightly":
    floors.append(("the latest nightly", last_nightly))
for what, v in floors:
    if v and key(new) <= key(v):
        sys.exit(f"release: {new} isn't newer than {what} ({v}), so its users wouldn't get it")
if key(new) < key(current):
    sys.exit(f"release: {new} is older than the workspace version {current}")
PY
if (( ! DRY )); then
  [[ -z $(git status --porcelain) ]] || die "the working tree has changes; commit or stash them first"
  [[ $CHANNEL != stable ]] || git symbolic-ref -q HEAD >/dev/null || die "a stable release is committed to a branch; check one out"
  gh auth status >/dev/null 2>&1 || die "gh isn't logged in"
  # macOS ties users' Accessibility / Screen Recording grants to the certificate: every release
  # must use the same one. TREK_SIGN_IDENTITY names another on purpose (a Developer ID, say).
  ID=$(trek_sign_identity)
  [[ $ID == "Trek Local Signing" || ( -n ${TREK_SIGN_IDENTITY:-} && $ID != "-" ) ]] \
    || die "releases are signed with \"Trek Local Signing\", not \"$ID\"; run script/signing-identity.sh (or import ~/.trek-signing/codesign.p12 from the Mac that has it)"
fi

# ---------- version ----------
# The bump is undone on exit unless a stable release committed it to the branch.
BACKUP=$(mktemp -d) COMMITTED=0
cp Cargo.toml Cargo.lock "$BACKUP/"
finish() {
  (( COMMITTED )) || cp "$BACKUP/Cargo.toml" "$BACKUP/Cargo.lock" .
  rm -rf "$BACKUP"
}
trap finish EXIT
# Interrupted (Ctrl-C, a killed build): exit, so the EXIT trap still puts the version back.
trap 'exit 130' INT TERM HUP
if [[ $VERSION != "$CURRENT" ]]; then
  echo "• version $CURRENT → $VERSION"
  sed -i '' '/^\[workspace\.package\]/,/^\[/s/^version = ".*"/version = "'"$VERSION"'"/' Cargo.toml
fi

# ---------- build ----------
script/bundle.sh
[[ $(trek_sign_identity) == "Trek Local Signing" || -n ${TREK_SIGN_IDENTITY:-} ]] \
  || echo "! signed with \"$(trek_sign_identity)\": fine for a dry run, refused for a real release"

# ---------- archive, checksum, signature ----------
rm -rf "$OUT"; mkdir -p "$OUT"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$OUT/$NAME" -C dist Trek.app
SHA=$(shasum -a 256 "$OUT/$NAME" | cut -d' ' -f1)
# The trusted comment is signed too; the updater requires it to name this version.
minisign -S -s "$KEY" -m "$OUT/$NAME" -x "$OUT/$NAME.minisig" -t "Trek $VERSION $PLATFORM" </dev/null >/dev/null
minisign -V -q -p assets/update/minisign.pub -m "$OUT/$NAME" -x "$OUT/$NAME.minisig" \
  || die "$KEY doesn't match assets/update/minisign.pub; Trek would refuse this release"
echo "• $NAME  sha256 $SHA  (signature verified with assets/update/minisign.pub)"

# ---------- notes ----------
if [[ -n $NOTES_FILE ]]; then
  cp "$NOTES_FILE" "$OUT/notes.md"
else
  # The most recent tag, by when it was made: git's version sort puts 0.3.0-beta.1 after 0.3.0.
  case $CHANNEL in
    stable) SINCE=$(git tag --list 'v[0-9]*' --sort=-creatordate | grep -v -- - | head -1 || true) ;;
    nightly) SINCE=$(git rev-parse -q --verify refs/tags/nightly >/dev/null && echo nightly || git tag --list 'v[0-9]*' --sort=-creatordate | head -1 || true) ;;
    *) SINCE=$(git tag --list 'v[0-9]*' --sort=-creatordate | head -1 || true) ;;
  esac
  RANGE=HEAD; [[ -n $SINCE ]] && RANGE="$SINCE..HEAD"
  git log --no-merges --invert-grep --grep='^Release v' --format='- %s' "$RANGE" > "$OUT/notes.md"
  [[ -s $OUT/notes.md ]] || echo "- Maintenance release." > "$OUT/notes.md"
fi

# ---------- manifest ----------
BASE=${TREK_RELEASE_DOWNLOAD_URL:-https://github.com/$REPO/releases/download/$RELEASE_TAG}
python3 - "$VERSION" "$PLATFORM" "${BASE%/}/$NAME" "$SHA" "$OUT/$NAME.minisig" "$OUT/notes.md" > "$OUT/$CHANNEL.json" <<'PY'
import datetime, json, sys
version, platform, url, sha, sig_path, notes_path = sys.argv[1:7]
print(json.dumps({
    "version": version,
    "notes": open(notes_path).read().strip(),
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {platform: {"url": url, "sha256": sha, "signature": open(sig_path).read()}},
}, indent=2))
PY
echo "• $OUT/$CHANNEL.json → ${BASE%/}/$NAME"

ASSETS=("$OUT/$NAME" "$OUT/$NAME.minisig" "$OUT/$CHANNEL.json")
if (( DRY )); then
  echo "• dry run: built $OUT; not committing, tagging, pushing or publishing. Would publish:"
  echo "    $REPO release $RELEASE_TAG ($CHANNEL): ${(j:, :)${ASSETS[@]:t}}"
  exit 0
fi

# ---------- publish ----------
if [[ $CHANNEL == stable ]]; then
  # The workspace version moves with stable releases only.
  if [[ $VERSION != "$CURRENT" ]]; then
    git add Cargo.toml Cargo.lock
    git commit -q -m "Release v$VERSION"
  fi
  COMMITTED=1
  git tag -a "$VERSION_TAG" -m "Trek $VERSION"
  git push origin HEAD
  git push origin "refs/tags/$VERSION_TAG"
  gh release create "$RELEASE_TAG" --repo "$REPO" --verify-tag --latest \
    --title "Trek $VERSION" --notes-file "$OUT/notes.md" "${ASSETS[@]}"
else
  # What was built, as a commit off the branch: HEAD's tree with the bumped Cargo.toml and
  # Cargo.lock. Its tags keep it; the branch and the working tree stay at the workspace version.
  INDEX=$(mktemp)
  GIT_INDEX_FILE=$INDEX git read-tree HEAD
  GIT_INDEX_FILE=$INDEX git update-index --add Cargo.toml Cargo.lock
  TREE=$(GIT_INDEX_FILE=$INDEX git write-tree)
  rm -f "$INDEX"
  BUILT=HEAD
  [[ $TREE == $(git rev-parse 'HEAD^{tree}') ]] || BUILT=$(git commit-tree "$TREE" -p HEAD -m "Release v$VERSION")
  if [[ -n $VERSION_TAG ]]; then
    git tag -a "$VERSION_TAG" -m "Trek $VERSION" "$BUILT"
    git push origin "refs/tags/$VERSION_TAG"
  fi
  # The channel tag moves to the build, its message naming the version (the next release's
  # checks read it); the release keeps its URL and swaps its assets: the archive first, the
  # manifest that points at it last, then older archives go.
  git tag -f -a "$CHANNEL" -m "Trek $VERSION" "$BUILT"
  git push -f origin "refs/tags/$CHANNEL"
  if gh release view "$CHANNEL" --repo "$REPO" >/dev/null 2>&1; then
    gh release upload "$CHANNEL" --repo "$REPO" --clobber "$OUT/$NAME" "$OUT/$NAME.minisig"
    gh release upload "$CHANNEL" --repo "$REPO" --clobber "$OUT/$CHANNEL.json"
    gh release edit "$CHANNEL" --repo "$REPO" --prerelease --title "Trek $VERSION ($CHANNEL)" --notes-file "$OUT/notes.md"
    for asset in ${(f)"$(gh release view "$CHANNEL" --repo "$REPO" --json assets --jq '.assets[].name')"}; do
      if [[ $asset == Trek-*.app.tar.gz* && $asset != "$NAME" && $asset != "$NAME.minisig" ]]; then
        gh release delete-asset "$CHANNEL" "$asset" --repo "$REPO" -y
      fi
    done
  else
    gh release create "$CHANNEL" --repo "$REPO" --verify-tag --prerelease \
      --title "Trek $VERSION ($CHANNEL)" --notes-file "$OUT/notes.md" "${ASSETS[@]}"
  fi
fi
echo "• published Trek $VERSION on $CHANNEL: https://github.com/$REPO/releases/tag/$RELEASE_TAG"
