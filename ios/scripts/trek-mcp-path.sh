#!/bin/sh
# Print the path of a trek-mcp that can install on the iPhone without Xcode (`phone-device`), or
# nothing (exit 1). Tried in order: $TREK_MCP, the installed Trek ($TREK_MAC_APP, default
# /Applications/Trek.app), then this repo's builds ($TREK_REPO, default the repo this script is in).
#   TREK_MCP="$(sh ios/scripts/trek-mcp-path.sh)"
here="$(cd "$(dirname "$0")/../.." && pwd)"
repo="${TREK_REPO:-$here}"
app="${TREK_MAC_APP:-/Applications/Trek.app}"
for c in "${TREK_MCP:-}" "$app/Contents/MacOS/trek-mcp" "$repo/target/release/trek-mcp" \
         "$repo/target/bundle/trek-mcp" "$repo/target/debug/trek-mcp"; do
  [ -n "$c" ] && [ -x "$c" ] || continue
  # One without `phone-device` (Trek before 0.4.22) exits 2 with its usage.
  if "$c" phone-device list >/dev/null 2>&1; then
    echo "$c"
    exit 0
  fi
done
exit 1
