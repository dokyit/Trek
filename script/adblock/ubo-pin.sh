#!/usr/bin/env bash
# The pin for uBlock Origin release <version>, for Trek's ad blocker
# (crates/trek-app/src/panels/browser_adblock/scriptlets.rs: UBO_VERSION, UBO_URL, UBO_SHA256).
# Downloads that release's Chromium package, checks it against the SHA-256 GitHub publishes for
# it, and prints the three lines to paste. Read what changed in its js/resources and
# web_accessible_resources before moving Trek to it (they run in every page).
#
#   script/adblock/ubo-pin.sh 1.77.2
set -euo pipefail
version=${1:?usage: script/adblock/ubo-pin.sh <version>}
name="uBlock0_${version}.chromium.zip"
url="https://github.com/gorhill/uBlock/releases/download/${version}/${name}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$name" "$url"
sum=$(shasum -a 256 "$tmp/$name" | cut -d' ' -f1)
published=$(gh api "repos/gorhill/uBlock/releases/tags/${version}" \
    --jq ".assets[] | select(.name==\"${name}\") | .digest" 2>/dev/null || true)
if [ -z "$published" ]; then
    echo "GitHub publishes no digest for ${name}: check ${sum} another way before using it." >&2
elif [ "$published" != "sha256:${sum}" ]; then
    echo "The download (${sum}) isn't what GitHub publishes (${published}): not pinned." >&2
    exit 1
fi
printf 'pub const UBO_VERSION: &str = "%s";\n' "$version"
printf 'pub const UBO_URL: &str =\n    "%s";\n' "$url"
printf 'pub const UBO_SHA256: &str = "%s";\n' "$sum"
