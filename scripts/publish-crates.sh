#!/bin/sh
set -eu
version="$1"
node scripts/set-crate-versions.mjs "$version"
node scripts/crate-readmes.mjs
cargo update --offline --workspace
publish_one() {
  name="$1"
  n=1
  while [ "$n" -le 8 ]; do
    log=$(mktemp)
    if cargo publish -p "$name" --allow-dirty >"$log" 2>&1; then
      cat "$log"
      rm -f "$log"
      return 0
    fi
    cat "$log"
    if grep 'already uploaded' "$log" >/dev/null; then
      rm -f "$log"
      return 0
    fi
    if grep -i -E 'status 429|too many requests|too many versions|rate.?limit' "$log" >/dev/null; then
      rm -f "$log"
      return 2
    fi
    rm -f "$log"
    n=$((n + 1))
    sleep 15
  done
  return 1
}
for name in slicc-cdp-client slicc-discovery slicc-cdp curlwright; do
  status=0
  publish_one "$name" || status=$?
  if [ "$status" -eq 2 ]; then
    echo "::warning::crates.io rate-limited $name $version; skipped it and the crates after it, the next release publishes the newest version"
    echo "publish-crates: SKIP $name $version rate-limited by crates.io"
    exit 0
  fi
  [ "$status" -eq 0 ] || exit "$status"
done
echo "publish-crates: PASS 4 crates $version"
