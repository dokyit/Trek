#!/usr/bin/env bash
# Build, sign and attach the Linux downloads of a release the Mac publishes (release.ps1's Linux
# twin; .github/workflows/release.yml runs it, docs/RELEASING.md). Two halves, as Windows has:
#
#   script/release-linux.sh <version> [--channel C] --build-only [--skip-tests]
#   script/release-linux.sh <version> [--channel C] --publish-only [--publish] [--existing-manifest FILE]
#
# --build-only sets the version for the build (Cargo.toml and Cargo.lock are put back after),
# builds trek and trek-mcp for this machine's architecture, packages them (script/bundle-linux.sh:
# AppImage, .deb, tarball), signs the AppImage with minisign (trusted comment "Trek <version>
# linux-<arch>"), checks the signature against assets/update/minisign.pub, and writes the
# platform's manifest entry (linux-<arch>.json) into dist/release/<version>/linux-<arch>/. The
# AppImage is what the update manifest points at: it's the one that replaces itself. The build
# is unstripped, and each executable's DWARF (line tables) goes into trek.debug and trek-mcp.debug
# before it's stripped for the packages, kept as Trek-<version>-linux-<arch>.debug.tar.gz
# (docs/RELEASING.md, "Symbols"); the GNU build ID in both matches them.
#
# --publish-only takes every dist/release/<version>/linux-*/ there is (the workflow downloads each
# architecture's), merges their entries into the release's <channel>.json (script/lib/merge-manifest.py,
# which wants the Mac's entry there first) and, with --publish, uploads the packages and their
# signatures first and the manifest last. Without --publish nothing leaves the machine.
#
# Environment:
#   TREK_MINISIGN_KEY        secret key file (default ~/.trek-signing/minisign.key)
#   TREK_MINISIGN_PASSWORD   its password, if it has one
#   TREK_MINISIGN            the minisign binary (default: minisign on PATH)
#   TREK_REPO                owner/name (default $GITHUB_REPOSITORY, else dokyit/Trek)
#   BUILD=0                  reuse the binaries already in the target folder
set -euo pipefail

cd "$(dirname "$0")/.."

die() { echo "release-linux: $*" >&2; exit 1; }
say() { echo " $*"; }

version=""
channel=stable
mode=""
publish=0
skip_tests=0
existing=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --channel) channel=$2; shift 2 ;;
    --build-only) mode=build; shift ;;
    --publish-only) mode=publish; shift ;;
    --publish) publish=1; shift ;;
    --skip-tests) skip_tests=1; shift ;;
    --existing-manifest) existing=$2; shift 2 ;;
    -h|--help) sed -n '2,31p' "$0"; exit 0 ;;
    -*) die "unknown option $1" ;;
    *) [[ -z $version ]] || die "one version, please"; version=${1#v}; shift ;;
  esac
done
[[ -n $version ]] || die "usage: script/release-linux.sh <version> --build-only | --publish-only [--publish]"
[[ -n $mode ]] || die "choose --build-only or --publish-only"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "$version is not a semver version"
pre=""; [[ $version == *-* ]] && pre=${version#*-}
case "$channel" in
  stable) [[ -z $pre ]] || die "stable versions have no pre-release suffix ($version)" ;;
  beta|nightly) [[ $pre == "$channel"* ]] || die "$channel versions look like 1.2.3-$channel.N ($version)" ;;
  *) die "the channel is stable, beta or nightly" ;;
esac
[[ $mode == build && $publish == 1 ]] && die "--build-only never uploads; the upload half is --publish-only --publish"
repo=${TREK_REPO:-${GITHUB_REPOSITORY:-dokyit/Trek}}
tag=$channel; [[ $channel == stable ]] && tag=v$version
out=dist/release/$version
pubkey=assets/update/minisign.pub
minisign=${TREK_MINISIGN:-minisign}

# ---------- build half ----------
if [[ $mode == build ]]; then
  arch=$(uname -m)
  case "$arch" in x86_64|aarch64) ;; *) die "no Linux release for $arch" ;; esac
  platform=linux-$arch
  dir=$out/$platform
  key=${TREK_MINISIGN_KEY:-$HOME/.trek-signing/minisign.key}
  [[ -f $key ]] || say "! no minisign key at $key: the AppImage is left unsigned, and every Trek refuses an unsigned update"

  # The version, for the build only: put back on the way out (the Mac's release.sh commits it).
  backup=$(mktemp -d)
  cp Cargo.toml Cargo.lock "$backup/"
  trap 'cp "$backup/Cargo.toml" "$backup/Cargo.lock" . ; rm -rf "$backup"' EXIT
  current=$(awk '/^\[workspace.package\]/{p=1;next} /^\[/{p=0} p&&/^version *=/{gsub(/"/,"",$3);print $3;exit}' Cargo.toml)
  if [[ $current != "$version" ]]; then
    say "version $current -> $version"
    awk -v v="$version" '/^\[/{p=($0=="[workspace.package]")} p&&/^version *=/{$0="version = \""v"\""} {print}' Cargo.toml > Cargo.toml.new
    mv Cargo.toml.new Cargo.toml
    cargo update --workspace --offline 2>/dev/null || cargo update --workspace || die "couldn't update Cargo.lock for the version bump"
  fi

  target=${CARGO_TARGET_DIR:-target}
  if [[ ${BUILD:-1} != 0 ]]; then
    # Unstripped, with the DWARF in the executables (not a .dwp beside them), for the .debug files.
    say "build: cargo build --release --locked -p trek-app -p trek-mcp (unstripped)"
    CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=off \
      cargo build --release --locked -p trek-app -p trek-mcp
  fi
  if [[ $skip_tests == 0 ]]; then
    say "smoke: cargo test -p trek-core --locked"
    cargo test -p trek-core --locked
  fi

  rm -rf "$dir"
  mkdir -p "$dir"
  # The symbols out, then the executables stripped as a release ships them, each pointing at its
  # .debug by name (a debugger looks for it there) and both keeping the build ID.
  command -v objcopy >/dev/null || die "needs objcopy (apt install binutils)"
  bins=$(mktemp -d)
  syms=$bins/symbols
  mkdir -p "$syms"
  for b in trek trek-mcp; do
    cp "$target/release/$b" "$bins/$b"
    objcopy --only-keep-debug "$bins/$b" "$syms/$b.debug"
    objcopy --strip-all "$bins/$b"
    (cd "$syms" && objcopy --add-gnu-debuglink="$b.debug" "$bins/$b")
    id=$(readelf -n "$bins/$b" 2>/dev/null | awk '/Build ID/{print $3}')
    [[ -n $id ]] || say "! $b has no GNU build ID: a symbolicator can't match its .debug"
    [[ $id == "$(readelf -n "$syms/$b.debug" 2>/dev/null | awk '/Build ID/{print $3}')" ]] \
      || die "$b.debug's build ID isn't the executable's"
    say "$b  build ID $id  $(du -h "$syms/$b.debug" | cut -f1) of symbols"
  done
  symbols="Trek-$version-$platform.debug.tar.gz"
  tar -C "$syms" -czf "$dir/$symbols" trek.debug trek-mcp.debug
  script/bundle-linux.sh --version "$version" --arch "$arch" --bin-dir "$bins" --out "$dir" --release
  rm -rf "$bins"
  image="Trek-$version-$platform.AppImage"
  [[ -f $dir/$image ]] || die "$dir/$image missing after packaging"

  signature=""
  if [[ -f $key ]]; then
    # Whether the key has a password: its KDF is named in the key's second line ("Sc": scrypt).
    locked=0
    [[ $(sed -n 2p "$key" | base64 -d 2>/dev/null | head -c 4 | tail -c 2) == Sc ]] && locked=1
    [[ $locked == 1 ]] || say "! $key has no password: anything running as you can read it and sign updates"
    args=(-S -s "$key" -m "$dir/$image" -x "$dir/$image.minisig" -t "Trek $version $platform")
    if [[ $locked == 1 && -n ${TREK_MINISIGN_PASSWORD:-} ]]; then
      printf '%s\n' "$TREK_MINISIGN_PASSWORD" | "$minisign" "${args[@]}" >/dev/null
    else
      "$minisign" "${args[@]}" </dev/null >/dev/null
    fi
    "$minisign" -V -q -p "$pubkey" -m "$dir/$image" -x "$dir/$image.minisig" \
      || die "$key doesn't match $pubkey; Trek would refuse this release"
    signature=$(cat "$dir/$image.minisig")
  fi
  sha=$(sha256sum "$dir/$image" | cut -d' ' -f1)
  base=${TREK_RELEASE_DOWNLOAD_URL:-https://github.com/$repo/releases/download/$tag}
  python3 - "$version" "$platform" "${base%/}/$image" "$sha" "$signature" > "$dir/$platform.json" <<'EOF'
import json, sys
version, platform, url, sha, signature = sys.argv[1:]
print(json.dumps({"version": version, "platforms": {platform: {"url": url, "sha256": sha, "signature": signature}}}, indent=2))
EOF
  say "$image  sha256 $sha$([[ -n $signature ]] && echo '  (signature verified)')"
  say "build half done: $dir"
  ls -1 "$dir" | sed 's/^/   /'
  exit 0
fi

# ---------- publish half ----------
shopt -s nullglob
dirs=("$out"/linux-*/)
(( ${#dirs[@]} )) || die "no $out/linux-*/: the --build-only half runs first"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
manifest=$work/$channel.json
if [[ $publish == 1 ]]; then
  gh release download "$tag" --repo "$repo" --pattern "$channel.json" --output "$manifest" --clobber \
    || die "no $channel.json on release $tag: the Mac's release.sh publishes it first"
elif [[ -n $existing ]]; then
  cp "$existing" "$manifest"
else
  # Nothing to merge into: the Linux entries alone, so a dry run still shows them.
  echo '{"version": "'"$version"'", "platforms": {"darwin-aarch64": {}}}' > "$manifest"
fi
assets=()
for d in "${dirs[@]}"; do
  platform=$(basename "$d")
  [[ -f $d/$platform.json ]] || die "$d has no $platform.json"
  if [[ $publish == 1 ]] && ! grep -q '"signature": "untrusted comment' "$d/$platform.json"; then
    die "$platform's AppImage isn't signed: every Trek refuses an unsigned update"
  fi
  python3 script/lib/merge-manifest.py "$manifest" "$d/$platform.json" > "$work/merged.json"
  mv "$work/merged.json" "$manifest"
  # The .tar.gz glob takes the symbols (.debug.tar.gz) too; a build without them says so here
  # and goes out anyway ("Every platform released" names the missing file).
  for f in "$d"/*.AppImage "$d"/*.AppImage.minisig "$d"/*.deb "$d"/*.tar.gz; do assets+=("$f"); done
  compgen -G "$d/*.debug.tar.gz" >/dev/null || say "! no symbols in $d"
done
mkdir -p "$out"
cp "$manifest" "$out/$channel.json"
say "$out/$channel.json has $(python3 -c 'import json,sys; print(", ".join(sorted(json.load(open(sys.argv[1]))["platforms"])))' "$manifest")"
if [[ $publish != 1 ]]; then
  say "dry run: not publishing. Would upload to $repo's release $tag ($channel):"
  for a in "${assets[@]}" "$out/$channel.json"; do say "  $(basename "$a")"; done
  exit 0
fi
# The packages first, the manifest that points at them last: a Trek that reads the manifest
# mid-upload finds its AppImage already there.
gh release upload "$tag" --repo "$repo" --clobber "${assets[@]}"
gh release upload "$tag" --repo "$repo" --clobber "$out/$channel.json"
# Under the moving beta and nightly tags, last time's Linux downloads go.
keep=$(printf '%s\n' "${assets[@]##*/}")
gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name' | while read -r a; do
  if [[ $a == Trek-*-linux-* || $a == trek_*.deb ]] && ! grep -qxF "$a" <<< "$keep"; then
    gh release delete-asset "$tag" "$a" --repo "$repo" -y
  fi
done
say "published Trek $version for Linux on $channel: https://github.com/$repo/releases/tag/$tag"
