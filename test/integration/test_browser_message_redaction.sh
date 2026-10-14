#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMPDIR_TEST=$(mktemp -d /tmp/slicc-browser-msg-XXXXXX)
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  sleep 1
  rm -rf "$TMPDIR_TEST" 2>/dev/null || true
}
trap cleanup EXIT

cargo build --manifest-path "$ROOT/Cargo.toml" -p curlwright 2>/dev/null
CW="$ROOT/target/debug/curlwright"

PORT_FILE="$TMPDIR_TEST/http-port"
python3 -c "
import http.server, socketserver, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(b'<html><body>ok</body></html>')
    def log_message(self, *a): pass
srv = socketserver.TCPServer(('127.0.0.1', 0), H)
port = srv.server_address[1]
with open('$PORT_FILE', 'w') as f:
    f.write(str(port))
srv.serve_forever()
" &
HTTP_PID=$!
PIDS+=("$HTTP_PID")

for i in $(seq 1 30); do [ -f "$PORT_FILE" ] && break; sleep 0.1; done
HTTP_PORT=$(cat "$PORT_FILE")
echo "HTTP server on port $HTTP_PORT"

PROFILE="$TMPDIR_TEST/chrome-profile"
CHROMIUM="${CHROMIUM_EXECUTABLE:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

"$CHROMIUM" \
  --headless --disable-gpu --no-sandbox \
  --remote-debugging-port=0 \
  --user-data-dir="$PROFILE" \
  --remote-allow-origins='*' \
  about:blank >/dev/null 2>&1 &
CHROME_PID=$!
PIDS+=("$CHROME_PID")

CDP_PORT=""
for i in $(seq 1 60); do
  if [ -f "$PROFILE/DevToolsActivePort" ]; then
    CDP_PORT=$(head -1 "$PROFILE/DevToolsActivePort")
    if [ -n "$CDP_PORT" ] && [ "$CDP_PORT" != "0" ]; then
      break
    fi
  fi
  sleep 0.5
done

if [ -z "$CDP_PORT" ]; then
  echo "FAIL: Chrome did not start"
  exit 1
fi

echo "CDP on port $CDP_PORT"
CDP_URL="http://127.0.0.1:${CDP_PORT}"
sleep 1

FAILS=0
fail() { echo "FAIL: $1"; FAILS=$((FAILS + 1)); }

USERINFO_URL="http://u:PW_SECRET_VALUE@127.0.0.1:${HTTP_PORT}/x"
TAB_ID=$(curl -s "${CDP_URL}/json" | python3 -c "import sys,json; tabs=json.load(sys.stdin); print(tabs[0]['id'])")

echo "=== curlwright fetch with userinfo URL (should redact) ==="
OUTPUT=$("$CW" --cdp "$CDP_URL" --tab="$TAB_ID" "$USERINFO_URL" 2>&1 || true)
echo "$OUTPUT"
if echo "$OUTPUT" | grep -q "PW_SECRET_VALUE"; then
  fail "curlwright leaks password from browser error"
fi
if ! echo "$OUTPUT" | grep -q "127.0.0.1"; then
  fail "output should still mention the host"
fi

echo "=== curlwright fetch with --full-urls (should show password) ==="
OUTPUT_FULL=$("$CW" --cdp "$CDP_URL" --tab="$TAB_ID" --full-urls "$USERINFO_URL" 2>&1 || true)
echo "$OUTPUT_FULL"
if ! echo "$OUTPUT_FULL" | grep -q "PW_SECRET_VALUE"; then
  fail "--full-urls should show the password"
fi

echo ""
if [ "$FAILS" -gt 0 ]; then
  echo "FAILED: $FAILS test(s)"
  exit 1
else
  echo "ALL PASSED"
  exit 0
fi
