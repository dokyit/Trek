#!/bin/sh
# Run a command on a display of its own, as a Linux desktop session would give Trek one: an X
# server (Xvfb) or a Wayland compositor (Weston's headless backend), a session bus, and Mesa's
# software renderers for GPUI. For CI and containers, where there's no screen.
#
#   script/linux/headless.sh [--wayland] [--size 1600x1000] -- <command> [args...]
#   script/linux/headless.sh [--wayland] [--size 1600x1000] --keep >> "$GITHUB_ENV"
#
# X11 is the default: Trek's web views (WebKitGTK) need it. With --wayland, Trek talks Wayland
# to Weston (and Weston's Xwayland is left off). --keep leaves the display and the bus running and
# prints their variables (NAME=value lines), for a CI job whose later steps use them.
set -eu

backend=x11
size=1600x1000
keep=0
while [ $# -gt 0 ]; do
  case "$1" in
    --wayland) backend=wayland; shift ;;
    --size) size="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --) shift; break ;;
    *) break ;;
  esac
done
[ $# -gt 0 ] || [ $keep = 1 ] || { echo "usage: $0 [--wayland] [--size WxH] -- command... | --keep" >&2; exit 2; }

runtime="${XDG_RUNTIME_DIR:-/tmp/trek-headless-$(id -u)}"
mkdir -p "$runtime" && chmod 700 "$runtime"
export XDG_RUNTIME_DIR="$runtime"
# No GPU here: Mesa's software drivers. On Xvfb, GPUI draws through OpenGL (llvmpipe): Mesa's
# software Vulkan (lavapipe) renders there but its frames never reach Xvfb's screen (Ubuntu 22.04's
# Mesa 23.2: even vkcube stays black), so the Vulkan drivers are hidden and wgpu takes GL. Weston
# gets lavapipe.
if [ "$backend" = x11 ]; then
  export VK_ICD_FILENAMES="${VK_ICD_FILENAMES:-/nonexistent/no-vulkan.json}"
else
  for icd in /usr/share/vulkan/icd.d/lvp_icd.*.json; do
    [ -e "$icd" ] && export VK_ICD_FILENAMES="${VK_ICD_FILENAMES:-$icd}"
  done
fi
export LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 NO_AT_BRIDGE=1

pids=""
cleanup() { for p in $pids; do kill "$p" 2>/dev/null || true; done; }
[ $keep = 1 ] || trap cleanup EXIT INT TERM
# --keep: the servers outlive this script, with nothing of its output held open.
spawn() { if [ $keep = 1 ]; then nohup "$@" </dev/null & else "$@" & fi; }

if [ "$backend" = x11 ]; then
  display=":${TREK_HEADLESS_DISPLAY:-99}"
  spawn Xvfb "$display" -screen 0 "${size}x24" -nolisten tcp >"$runtime/xvfb.log" 2>&1
  pids="$pids $!"
  export DISPLAY="$display"
  unset WAYLAND_DISPLAY
  i=0; until xdpyinfo >/dev/null 2>&1; do i=$((i+1)); [ $i -lt 100 ] || { cat "$runtime/xvfb.log" >&2; exit 1; }; sleep 0.1; done
else
  w=${size%x*}; h=${size#*x}
  spawn weston --backend=headless-backend.so --width="$w" --height="$h" --socket=trek-wayland \
    --idle-time=0 >"$runtime/weston.log" 2>&1
  pids="$pids $!"
  export WAYLAND_DISPLAY=trek-wayland
  unset DISPLAY
  i=0; until [ -S "$runtime/trek-wayland" ]; do i=$((i+1)); [ $i -lt 100 ] || { cat "$runtime/weston.log" >&2; exit 1; }; sleep 0.1; done
fi

# A session bus, as a desktop has, for notifications, the tray and the secret service.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] && command -v dbus-launch >/dev/null; then
  eval "$(dbus-launch --sh-syntax)"
  pids="$pids $DBUS_SESSION_BUS_PID"
fi

if [ $keep = 1 ]; then
  for v in XDG_RUNTIME_DIR VK_ICD_FILENAMES LIBGL_ALWAYS_SOFTWARE WEBKIT_DISABLE_COMPOSITING_MODE \
    NO_AT_BRIDGE DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS; do
    eval "val=\${$v:-}"
    [ -n "$val" ] && echo "$v=$val"
  done
  exit 0
fi

"$@"
