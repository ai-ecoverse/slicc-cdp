#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/sign-macos.sh identifier:binary [identifier:binary ...]" >&2
  exit 2
fi

ids=()
bins=()
for arg in "$@"; do
  ids+=("${arg%%:*}")
  bins+=("${arg#*:}")
done

if [ -z "${APPLE_CERTIFICATE_BASE64:-}" ] || [ -z "${APPLE_CERTIFICATE_PASSWORD:-}" ]; then
  echo "::error::APPLE_CERTIFICATE_BASE64 or APPLE_CERTIFICATE_PASSWORD is not set"
  exit 1
fi
if [ "$(uname -s)" != "Darwin" ]; then
  echo "::error::signing the darwin binaries needs a macOS runner"
  exit 1
fi

work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/slicc-sign.XXXXXX")"
keychain="$work/signing.keychain-db"
cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

raw="$work/raw.p12"
pem="$work/cert.pem"
norm="$work/normalized.p12"
printf '%s' "$APPLE_CERTIFICATE_BASE64" | base64 --decode > "$raw"
if ! openssl pkcs12 -in "$raw" -nodes -out "$pem" -passin "pass:$APPLE_CERTIFICATE_PASSWORD" 2>/dev/null; then
  openssl pkcs12 -in "$raw" -nodes -out "$pem" -passin "pass:$APPLE_CERTIFICATE_PASSWORD" -legacy
fi
if ! openssl pkcs12 -export -in "$pem" -out "$norm" -password "pass:$APPLE_CERTIFICATE_PASSWORD" \
  -keypbe AES-256-CBC -certpbe AES-256-CBC -macalg sha256 2>/dev/null; then
  openssl pkcs12 -export -in "$pem" -out "$norm" -password "pass:$APPLE_CERTIFICATE_PASSWORD" \
    -keypbe AES-256-CBC -certpbe AES-256-CBC -macalg sha256 -legacy
fi
rm -f "$raw" "$pem"
password="$(openssl rand -base64 32)"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$norm" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$keychain"
security set-key-partition-list -S apple-tool:,apple:,codesign: -k "$password" "$keychain" >/dev/null
existing=()
while IFS= read -r line; do
  line="${line#"${line%%[![:space:]]*}"}"
  existing+=("${line//\"/}")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}
rm -f "$norm"

identity="$(security find-identity -v -p codesigning "$keychain" | grep 'Developer ID Application' | head -1 | awk '{print $2}' || true)"
if [ -z "$identity" ]; then
  echo "::error::no Developer ID Application identity in the certificate"
  exit 1
fi
for i in "${!bins[@]}"; do
  echo "signing $(basename "${bins[$i]}") as ${ids[$i]}"
  codesign --force --timestamp --options runtime --identifier "${ids[$i]}" \
    --keychain "$keychain" --sign "$identity" "${bins[$i]}"
  codesign --verify --strict --verbose=2 "${bins[$i]}"
done

if [ -z "${APPLE_API_KEY_P8_BASE64:-}" ] || [ -z "${APPLE_API_KEY_ID:-}" ] || [ -z "${APPLE_API_KEY_ISSUER_ID:-}" ]; then
  echo "::error::App Store Connect API key secrets are not set"
  exit 1
fi
key="$work/AuthKey.p8"
zip="$work/notarize.zip"
result="$work/notarize.json"
printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$key"
zip -q -j "$zip" "${bins[@]}"
echo "notarizing the darwin binaries"
xcrun notarytool submit "$zip" --key "$key" --key-id "$APPLE_API_KEY_ID" \
  --issuer "$APPLE_API_KEY_ISSUER_ID" --wait --timeout 30m --output-format json > "$result" || true
status="$(node -p "JSON.parse(require('fs').readFileSync('$result','utf8')).status" 2>/dev/null || echo unknown)"
id="$(node -p "JSON.parse(require('fs').readFileSync('$result','utf8')).id" 2>/dev/null || echo)"
if [ "$status" != "Accepted" ]; then
  echo "::error::notarization ended with status $status"
  cat "$result" >&2 || true
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" --key "$key" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_KEY_ISSUER_ID" >&2 || true
  fi
  exit 1
fi
echo "notarization accepted ($id)"
