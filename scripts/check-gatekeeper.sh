#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/check-gatekeeper.sh binary [binary ...]" >&2
  exit 2
fi

delays=(20 60)
for bin in "$@"; do
  attempt=1
  while :; do
    echo "gatekeeper attempt $attempt/3 for $bin"
    signed_status=0
    signed="$(codesign -dv --verbose=2 "$bin" 2>&1)" || signed_status=$?
    spctl_status=0
    gatekeeper="$(spctl -a -vv -t install "$bin" 2>&1)" || spctl_status=$?
    printf '%s\n' "$signed"
    printf '%s\n' "$gatekeeper"
    echo "codesign exit $signed_status, spctl exit $spctl_status"
    if [ "$signed_status" -ne 0 ]; then
      echo "::error::codesign could not read the signature of $bin"
      exit 1
    fi
    if ! grep 'Authority=Developer ID Application' <<<"$signed" >/dev/null; then
      echo "::error::$bin is not signed by a Developer ID Application certificate"
      exit 1
    fi
    if [ "$spctl_status" -eq 0 ] && grep -x 'source=Notarized Developer ID' <<<"$gatekeeper" >/dev/null; then
      echo "gatekeeper accepted $bin as notarised on attempt $attempt/3"
      break
    fi
    if [ "$attempt" -ge 3 ]; then
      echo "::error::Gatekeeper did not accept $bin as notarised after 3 attempts"
      exit 1
    fi
    echo "retrying Gatekeeper for $bin in ${delays[$((attempt - 1))]}s"
    sleep "${delays[$((attempt - 1))]}"
    attempt=$((attempt + 1))
  done
done
