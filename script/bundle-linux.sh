#!/usr/bin/env bash
# Package a Linux build of Trek: an AppImage, a .deb and a tarball, from the release binaries.
# The Linux twin of script/bundle.sh (the Mac's .app) and release.ps1's zip; release-linux.sh
# builds the binaries and signs and publishes what this makes.
#
#   script/bundle-linux.sh [--version V] [--arch x86_64|aarch64] [--bin-dir DIR] [--out DIR]
#                          [--formats appimage,deb,tar] [--release]
#
# --release (script/release-linux.sh) marks the AppImage as a published release, which is what
# lets it replace itself (`update::linux`); one made without it is a local build, which doesn't.
#
# --bin-dir holds `trek` and `trek-mcp` (default $CARGO_TARGET_DIR/release, else target/release).
# Writes to --out (default dist/linux/<version>):
#   Trek-<v>-linux-<arch>.AppImage   one file to download and run; updates itself (the update
#                                    manifest points at it). Mounted by the static type2 runtime:
#                                    no libfuse2 needed.
#   trek_<debv>_<amd64|arm64>.deb    apt installs it with what it needs; apt updates it
#   Trek-<v>-linux-<arch>.tar.gz     the binaries, desktop file and icon, to put anywhere
#
# What Trek needs from the system, in every format: glibc 2.35 or later (Ubuntu 22.04, Debian 12,
# Fedora 36), GTK 3 and WebKitGTK 4.1 (with libsoup 3), xkbcommon-x11, and git. The .deb asks apt
# for them; the AppImage's start script names any that's missing and how to install it. WebKitGTK
# isn't bundled into the AppImage: its helper processes live at a path compiled into the library.
#
# Tools: dpkg-deb, and appimagetool with the type2 runtime, both pinned and downloaded on first use
# (checked by SHA-256) into $TREK_TOOLS_DIR (default ~/.cache/trek-tools); rsvg-convert or resvg
# draws the icons from assets/brand. Runs on Linux (CI's ubuntu runners, script/linux/Dockerfile).
set -euo pipefail

cd "$(dirname "$0")/.."
root=$PWD

version=""
arch=$(uname -m)
bin_dir=""
out=""
formats="appimage,deb,tar"
release=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --arch) arch=$2; shift 2 ;;
    --bin-dir) bin_dir=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --formats) formats=$2; shift 2 ;;
    --release) release=1; shift ;;
    -h|--help) sed -n "2,26p" "$0"; exit 0 ;;
    *) echo "bundle-linux: unknown option $1" >&2; exit 2 ;;
  esac
done

die() { echo "bundle-linux: $*" >&2; exit 1; }
say() { echo "• $*"; }

[[ $(uname -s) == Linux ]] || die "packages are made on Linux (script/linux/Dockerfile on a Mac)"
case "$arch" in
  x86_64) deb_arch=amd64 ;;
  aarch64|arm64) arch=aarch64; deb_arch=arm64 ;;
  *) die "no Linux package for $arch" ;;
esac
if [[ -z $version ]]; then
  version=$(awk '/^\[workspace.package\]/{p=1;next} /^\[/{p=0} p&&/^version *=/{gsub(/"/,"",$3);print $3;exit}' Cargo.toml)
fi
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "$version is not a semver version"
bin_dir=${bin_dir:-${CARGO_TARGET_DIR:-target}/release}
out=${out:-dist/linux/$version}
for b in trek trek-mcp; do
  [[ -x $bin_dir/$b ]] || die "$bin_dir/$b missing: cargo build --release -p trek-app -p trek-mcp"
done
file "$bin_dir/trek" | grep -q "$( [[ $arch == x86_64 ]] && echo x86-64 || echo aarch64 )" \
  || die "$bin_dir/trek isn't an $arch program"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"
out=$(cd "$out" && pwd)

# ---------- pinned tools ----------
tools=${TREK_TOOLS_DIR:-$HOME/.cache/trek-tools}
mkdir -p "$tools"
# fetch <url> <file> <sha256>: download once, refuse anything but the pinned bytes.
fetch() {
  local url=$1 file=$tools/$2 sha=$3
  if [[ ! -f $file ]] || ! echo "$sha  $file" | sha256sum -c --status; then
    curl -fsSL --retry 3 -o "$file.part" "$url"
    echo "$sha  $file.part" | sha256sum -c --status || { rm -f "$file.part"; die "$2: checksum mismatch"; }
    mv "$file.part" "$file"
    chmod +x "$file"
  fi
  echo "$file"
}
# appimagetool 1.9.1 and type2-runtime 20251108: the static runtime, which mounts the AppImage
# without libfuse2 (Ubuntu 22.04 and later don't install it).
host=$(uname -m)
case "$host" in
  x86_64) tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
  aarch64) tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
  *) tool_sha="" ;;
esac
case "$arch" in
  x86_64) runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64) runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
esac

# ---------- the shared tree: binaries, desktop file, icons ----------
say "Trek $version for linux-$arch from $bin_dir"
icons=$work/icons
mkdir -p "$icons"
render() { # render <svg> <px> <png>
  if command -v resvg >/dev/null; then resvg -w "$2" -h "$2" "$1" "$3"
  elif command -v rsvg-convert >/dev/null; then rsvg-convert -w "$2" -h "$2" -o "$3" "$1"
  else die "needs resvg or rsvg-convert (apt install librsvg2-bin) to draw the icons"
  fi
}
# Night, the app icon on every platform; 16 and 32 px from the art drawn for those sizes.
for px in 16 32 48 64 128 256 512; do
  art=assets/brand/trek-icon-night.svg
  [[ -f assets/brand/trek-icon-night-$px.svg ]] && art=assets/brand/trek-icon-night-$px.svg
  render "$art" "$px" "$icons/$px.png"
done

app_id=dev.trek.Trek
desktop() { # desktop <Exec command>
  cat <<EOF
[Desktop Entry]
Type=Application
Name=Trek
GenericName=Coding agents
Comment=Every agent, one trail.
Exec=$1 %U
Icon=$app_id
Terminal=false
Categories=Development;IDE;
Keywords=AI;agent;Claude;Codex;coding;
MimeType=x-scheme-handler/trek;
StartupWMClass=$app_id
StartupNotify=true
EOF
}

# install_share <prefix>: the desktop file and icons under <prefix>/share.
install_share() {
  local share=$1/share exec=$2
  mkdir -p "$share/applications"
  desktop "$exec" > "$share/applications/$app_id.desktop"
  for px in 16 32 48 64 128 256 512; do
    install -Dm644 "$icons/$px.png" "$share/icons/hicolor/${px}x${px}/apps/$app_id.png"
  done
  install -Dm644 assets/brand/trek-icon-night.svg "$share/icons/hicolor/scalable/apps/$app_id.svg"
}

if command -v desktop-file-validate >/dev/null; then
  desktop trek > "$work/check.desktop"
  desktop-file-validate "$work/check.desktop" || die "the desktop file doesn't validate"
fi

made=()

# ---------- AppImage ----------
if [[ ,$formats, == *,appimage,* ]]; then
  [[ -n $tool_sha ]] || die "appimagetool runs on x86_64 and aarch64 hosts"
  appimagetool=$(fetch "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$host.AppImage" "appimagetool-1.9.1-$host.AppImage" "$tool_sha")
  runtime=$(fetch "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch" "type2-runtime-20251108-$arch" "$runtime_sha")
  appdir=$work/Trek.AppDir
  install -Dm755 "$bin_dir/trek" "$appdir/usr/bin/trek"
  install -Dm755 "$bin_dir/trek-mcp" "$appdir/usr/bin/trek-mcp"
  install_share "$appdir/usr" trek
  install -Dm644 LICENSE "$appdir/usr/share/doc/trek/LICENSE"
  install -Dm644 NOTICE "$appdir/usr/share/doc/trek/NOTICE"
  cp "$appdir/usr/share/applications/$app_id.desktop" "$appdir/$app_id.desktop"
  cp "$icons/256.png" "$appdir/$app_id.png"
  cp "$icons/256.png" "$appdir/.DirIcon"
  install -Dm755 script/linux/AppRun "$appdir/AppRun"
  # `update::linux`'s RELEASE_MARKER, beside usr/bin: a release AppImage replaces itself.
  if [[ $release == 1 ]]; then
    mkdir -p "$appdir/usr/share/trek"
    echo true > "$appdir/usr/share/trek/release"
  fi
  name="Trek-$version-linux-$arch.AppImage"
  # appimagetool is itself an AppImage: extract-and-run works without FUSE (containers, CI).
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch "$appimagetool" --no-appstream \
    --runtime-file "$runtime" "$appdir" "$out/$name" >"$work/appimagetool.log" 2>&1 \
    || { cat "$work/appimagetool.log" >&2; die "appimagetool failed"; }
  chmod +x "$out/$name"
  made+=("$name")
fi

# ---------- .deb ----------
if [[ ,$formats, == *,deb,* ]]; then
  command -v dpkg-deb >/dev/null || die "needs dpkg-deb (apt install dpkg-dev)"
  # Debian orders 0.4.1~beta.2 before 0.4.1, as semver orders 0.4.1-beta.2.
  deb_version=${version/-/\~}
  pkg=$work/deb
  # The binaries in a folder of their own, so Trek finds trek-mcp beside itself; `trek` on PATH.
  install -Dm755 "$bin_dir/trek" "$pkg/usr/lib/trek/trek"
  install -Dm755 "$bin_dir/trek-mcp" "$pkg/usr/lib/trek/trek-mcp"
  # Says to Trek that apt installed it, so it leaves updates to apt (`update::Blocker::Packaged`).
  echo deb > "$pkg/usr/lib/trek/packaged-by"
  mkdir -p "$pkg/usr/bin"
  ln -s ../lib/trek/trek "$pkg/usr/bin/trek"
  install_share "$pkg/usr" /usr/bin/trek
  # Debian's place for a package's licence (copyright), and the notices beside it.
  install -Dm644 LICENSE "$pkg/usr/share/doc/trek/copyright"
  install -Dm644 NOTICE "$pkg/usr/share/doc/trek/NOTICE"
  size=$(du -sk "$pkg/usr" | cut -f1)
  mkdir -p "$pkg/DEBIAN"
  cat > "$pkg/DEBIAN/control" <<EOF
Package: trek
Version: $deb_version
Architecture: $deb_arch
Maintainer: Trek <support@trekdesktop.com>
Installed-Size: $size
Depends: libc6 (>= 2.35), libgtk-3-0, libwebkit2gtk-4.1-0, libsoup-3.0-0, libxkbcommon-x11-0, libxcb1, libfontconfig1, libdbus-1-3, git
Recommends: libayatana-appindicator3-1, xdg-utils, ca-certificates
Suggests: gnome-keyring
Section: devel
Priority: optional
Homepage: https://trekdesktop.com
Description: Every coding agent, one trail
 Trek runs Claude Code, Codex, OpenCode and other coding agents side by
 side in one window, with their threads, changes, terminals and a browser.
EOF
  name="trek_${deb_version}_${deb_arch}.deb"
  # Owned by root whoever builds it.
  dpkg-deb --root-owner-group --build "$pkg" "$out/$name" >/dev/null
  made+=("$name")
fi

# ---------- tarball ----------
if [[ ,$formats, == *,tar,* ]]; then
  dir=$work/tar/trek-$version
  install -Dm755 "$bin_dir/trek" "$dir/trek"
  install -Dm755 "$bin_dir/trek-mcp" "$dir/trek-mcp"
  install -Dm644 LICENSE "$dir/LICENSE"
  install -Dm644 NOTICE "$dir/NOTICE"
  desktop trek > "$dir/$app_id.desktop"
  cp "$icons/256.png" "$dir/$app_id.png"
  cat > "$dir/README.txt" <<EOF
Trek $version for Linux ($arch)

Run ./trek from this folder. Keep trek-mcp beside it.

Trek needs glibc 2.35 or later, GTK 3, WebKitGTK 4.1 and git:
  Ubuntu, Debian:  sudo apt install libgtk-3-0 libwebkit2gtk-4.1-0 git
  Fedora:          sudo dnf install gtk3 webkit2gtk4.1 git
  Arch:            sudo pacman -S gtk3 webkit2gtk-4.1 git

To have it in your app menu, and open trek:// links: copy $app_id.desktop to
~/.local/share/applications/ with Exec= set to this folder's trek, and $app_id.png to
~/.local/share/icons/hicolor/256x256/apps/. This copy doesn't update itself: the AppImage does,
and apt updates the .deb. https://trekdesktop.com
EOF
  name="Trek-$version-linux-$arch.tar.gz"
  tar -C "$work/tar" --owner=0 --group=0 --numeric-owner -czf "$out/$name" "trek-$version"
  made+=("$name")
fi

for m in "${made[@]}"; do
  say "$out/$m  $(du -h "$out/$m" | cut -f1)  sha256 $(sha256sum "$out/$m" | cut -d' ' -f1)"
done
