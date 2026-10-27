# Code-signing helpers shared by bundle.sh and signing-identity.sh (zsh, sourced).
#
# Identity, first match wins:
#   1. $TREK_SIGN_IDENTITY (any identity in your keychain; "-" forces ad-hoc)
#   2. "Trek Local Signing" (script/signing-identity.sh creates it)
#   3. "Shelf Dev", an older self-signed identity on the maintainer's Mac: any stable key keeps
#      permissions across rebuilds, though switching between identities resets them once
#   4. ad-hoc ("-"): runs fine, but macOS forgets Accessibility / Screen Recording on every rebuild
TREK_SIGN_IDENTITIES=("Trek Local Signing" "Shelf Dev")

trek_sign_identity() {
  # print, not echo: zsh's echo prints nothing for "-".
  if [[ -n "${TREK_SIGN_IDENTITY:-}" ]]; then print -r -- "$TREK_SIGN_IDENTITY"; return; fi
  local found name
  found=$(security find-identity -p codesigning 2>/dev/null)
  for name in "${TREK_SIGN_IDENTITIES[@]}"; do
    if grep -qF "\"$name\"" <<< "$found"; then print -r -- "$name"; return; fi
  done
  print -r -- "-"
}

# Whether identity $1 is an Apple-issued "Developer ID Application" certificate: the only kind
# Gatekeeper trusts outside the App Store, and the only kind notarization accepts. Its bundles are
# signed with the hardened runtime and a secure timestamp (notarization requires both); every other
# identity keeps the plain signature Trek has always had, which users' Accessibility and Screen
# Recording grants are tied to.
trek_is_developer_id() {
  [[ $1 == "Developer ID Application:"* ]]
}

# This file's folder, for the entitlements beside it (sourced, so $0 is the caller).
TREK_SIGN_LIB=${${(%):-%x}:A:h}

trek_sign_identity_kind() {
  case "$(trek_sign_identity)" in
    -) echo "ad-hoc: permissions reset on every rebuild; run script/signing-identity.sh" ;;
    *) echo "stable identity: permissions survive rebuilds and updates" ;;
  esac
}

# Sign the helper executables inside the bundle, then the bundle itself, with one identity.
trek_codesign_bundle() {
  local app=$1 id exe main
  local -a extra=()
  id=$(trek_sign_identity)
  # Notarization needs the hardened runtime and a secure timestamp; the entitlements keep what
  # Trek does under that runtime working.
  if trek_is_developer_id "$id"; then
    extra=(--options runtime --timestamp --entitlements "$TREK_SIGN_LIB/hardened-runtime.entitlements")
  fi
  main=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
  for exe in "$app"/Contents/MacOS/*(N); do
    [[ "${exe:t}" == "$main" ]] && continue
    codesign --force "${extra[@]}" --sign "$id" "$exe"
  done
  # In a synced folder (iCloud Documents) the file provider can put Finder metadata back on the
  # bundle between clearing it and signing, and codesign refuses "detritus": clear it, try again.
  local attempt
  for attempt in 1 2 3 4 5; do
    xattr -cr "$app"
    codesign --force "${extra[@]}" --sign "$id" "$app" 2>/dev/null && break
    # The last try keeps codesign's error and, failing, stops the build.
    (( attempt < 5 )) || codesign --force "${extra[@]}" --sign "$id" "$app"
  done
  # Not --strict: in a synced folder (iCloud Documents) Finder metadata can land on the bundle
  # right after signing. Release archives leave extended attributes out, and the updater checks
  # the unpacked copy strictly.
  codesign --verify --deep "$app"
}
