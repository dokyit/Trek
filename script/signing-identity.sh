#!/bin/zsh
# Create (once) the self-signed code-signing identity Trek bundles are signed with, so macOS keeps
# Accessibility / Screen Recording grants across rebuilds and updates: those grants are tied to the
# signing certificate, and an ad-hoc signature changes with every build.
#
#   script/signing-identity.sh          create "Trek Local Signing" if it isn't in the login keychain
#   script/signing-identity.sh --check  only report which identity script/bundle.sh would use
#
# The key and certificate are generated here (openssl) and kept in ~/.trek-signing/ (mode 600) so the
# same identity can be imported on another Mac; then they're imported into the login keychain with
# only /usr/bin/codesign allowed to use the key. Nothing here changes trust settings or needs a
# password prompt: codesign can sign with an untrusted self-signed certificate.
set -euo pipefail
NAME="Trek Local Signing"
DIR="$HOME/.trek-signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

has_identity() { security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "\"$1\""; }

if [[ "${1:-}" == "--check" ]]; then
  source "${0:A:h}/lib/sign.sh"
  echo "$(trek_sign_identity) ($(trek_sign_identity_kind))"
  exit 0
fi

if has_identity "$NAME"; then
  echo "• \"$NAME\" is already in the login keychain"
  exit 0
fi

umask 077
mkdir -p "$DIR"
if [[ ! -f "$DIR/codesign.p12" ]]; then
  echo "• generating \"$NAME\" (RSA 2048, 10 years) in $DIR"
  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=$NAME" \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null
  # The p12 password only protects the file in transit; the file itself stays mode 600.
  openssl rand -hex 24 > "$DIR/codesign.p12.password"
  # Legacy PBE + SHA-1 MAC: the PKCS#12 flavour `security import` reads on every macOS (LibreSSL
  # writes it by default; OpenSSL 3 needs -legacy).
  LEGACY=(); openssl version | grep -q '^OpenSSL 3' && LEGACY=(-legacy)
  openssl pkcs12 -export "${LEGACY[@]}" -name "$NAME" -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -out "$DIR/codesign.p12" -passout "file:$DIR/codesign.p12.password"
fi

echo "• importing into the login keychain (key usable by /usr/bin/codesign only)"
security import "$DIR/codesign.p12" -k "$KEYCHAIN" -f pkcs12 \
  -P "$(cat "$DIR/codesign.p12.password")" -T /usr/bin/codesign >/dev/null
has_identity "$NAME" && echo "• \"$NAME\" ready" || { echo "import finished but no identity found" >&2; exit 1; }
