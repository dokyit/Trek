#!/bin/sh
# The Debian/Ubuntu packages Trek's Linux build needs (Ubuntu 22.04 and later, Debian 12 and
# later), for CI's ubuntu runners and script/linux/Dockerfile. Run as root (or under sudo).
#
#   --with-run-tools  also what runs Trek with no display of its own and checks it there: Xvfb,
#                     Weston, Mesa's software Vulkan (lavapipe), a session bus, a keyring, and
#                     the tools that look at the screen.
#   --with-packaging  what script/bundle-linux.sh needs besides its pinned downloads (the .deb's
#                     tools, the desktop file's check, the icons' renderer).
#
# Fedora 40+ (dnf) names: gcc gcc-c++ clang cmake pkgconf-pkg-config libxkbcommon-x11-devel
# wayland-devel libxcb-devel fontconfig-devel freetype-devel vulkan-loader-devel gtk3-devel
# webkit2gtk4.1-devel libsoup3-devel libayatana-appindicator-gtk3-devel libxdo-devel
# dbus-devel openssl-devel alsa-lib-devel.
set -eu

build="build-essential pkg-config cmake clang libclang-dev git curl ca-certificates file
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb1-dev
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcursor-dev libxrandr-dev libxi-dev
  libfontconfig1-dev libfreetype6-dev libvulkan-dev
  libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev
  libayatana-appindicator3-dev libxdo-dev libdbus-1-dev libssl-dev libasound2-dev
  sqlite3"
# (sqlite3: the program, which the tests of OpenCode's history run as macOS has it.)

run="xvfb xauth x11-utils x11-xserver-utils xdotool imagemagick weston mesa-vulkan-drivers
  vulkan-tools libgl1-mesa-dri dbus dbus-x11 gnome-keyring libsecret-tools xdg-utils
  fonts-dejavu-core fonts-noto-color-emoji python3 procps psmisc zsh"

packaging="desktop-file-utils dpkg-dev librsvg2-bin"

extra=""
for arg in "$@"; do
  case "$arg" in
    --with-run-tools) extra="$extra $run" ;;
    --with-packaging) extra="$extra $packaging" ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

apt-get update
# shellcheck disable=SC2086
apt-get install -y --no-install-recommends $build $extra
