#!/bin/sh
# Install the LaunchAgent that keeps Trek on your iPhone up to date (and signed) over Wi-Fi:
# every 30 minutes it runs phone-update.sh, which rebuilds and reinstalls the app when Trek on
# this Mac is newer or the free signing is about to lapse, and updates this Mac's Trek while it's
# closed. See phone-update.sh for the rules. A published Trek keeps the agent's copy of the script
# at its own version (`remote::phone_update::refresh_agent_copy`).
#   ios/scripts/install-phone-update.sh              install (or update) it, and run it once
#   ios/scripts/install-phone-update.sh --uninstall  remove it
# Pause it without uninstalling: touch "~/Library/Application Support/dev.trek.phone-update/off".
# Log: ~/Library/Logs/trek-phone-update.log
set -eu
LABEL=dev.trek.phone-update
STATE="$HOME/Library/Application Support/$LABEL"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG="$HOME/Library/Logs/trek-phone-update.log"
DOMAIN="gui/$(id -u)"

if [ "${1:-}" = --uninstall ]; then
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST" "$STATE/phone-update.sh"
  echo "Removed $LABEL. (Its build folder and worktree stay in $STATE: delete them with"
  echo "  git -C <repo> worktree remove --force \"$STATE/src\"; rm -rf \"$STATE\")"
  exit 0
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
# The repo the updater builds from: this checkout's main repo (not a worktree of it).
REPO="$(cd "$(git -C "$HERE" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
mkdir -p "$STATE" "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
# A copy of its own: the checkout can change branch under it.
cp "$HERE/phone-update.sh" "$STATE/phone-update.sh"
chmod +x "$STATE/phone-update.sh"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>$STATE/phone-update.sh</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>TREK_REPO</key>
    <string>$REPO</string>
  </dict>
  <key>StartInterval</key>
  <integer>1800</integer>
  <key>RunAtLoad</key>
  <true/>
  <key>LowPriorityIO</key>
  <true/>
  <key>Nice</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>$LOG</string>
  <key>StandardErrorPath</key>
  <string>$LOG</string>
</dict>
</plist>
EOF
plutil -lint "$PLIST" >/dev/null
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "Installed $LABEL: every 30 minutes, builds from $REPO when the iPhone needs it."
echo "Log: $LOG    Off: touch \"$STATE/off\"    Remove: $0 --uninstall"
