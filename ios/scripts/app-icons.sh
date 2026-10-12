#!/bin/sh
# Render the iPhone app icons from assets/brand/trek-icon-*-ios.svg (run trek_icon.py first if the
# art changed): the Ember icon with its dark and tinted variants, the Night and Glass alternatives
# (Settings › Appearance › App icon), and the small previews that picker shows. Needs resvg.
set -eu
cd "$(dirname "$0")/.."
BRAND=../assets/brand
A=Trek/Assets.xcassets
render() { resvg -w "$3" -h "$3" "$BRAND/trek-icon-$1-ios.svg" "$2"; }
render ember $A/AppIcon.appiconset/icon-1024.png 1024
render dark $A/AppIcon.appiconset/icon-1024-dark.png 1024
render tinted $A/AppIcon.appiconset/icon-1024-tinted.png 1024
for icon in night glass; do
  name=$(printf %s "$icon" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
  render $icon "$A/AppIcon-$name.appiconset/icon-1024.png" 1024
done
for icon in ember night glass; do
  name=$(printf %s "$icon" | awk '{ print toupper(substr($0, 1, 1)) substr($0, 2) }')
  render $icon "$A/AppIconPreview/Icon$name.imageset/icon.png" 192
done
