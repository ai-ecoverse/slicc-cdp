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
    if grep -q 'already uploaded' "$log"; then
      rm -f "$log"
      return 0
    fi
    rm -f "$log"
    n=$((n + 1))
    sleep 15
  done
  return 1
}
publish_one slicc-cdp-client
publish_one slicc-discovery
publish_one slicc-cdp
publish_one curlwright
echo "publish-crates: PASS 4 crates $version"
