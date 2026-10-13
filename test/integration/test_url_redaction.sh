#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMPDIR_TEST=$(mktemp -d /tmp/slicc-url-redact-XXXXXX)
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  sleep 1
  rm -rf "$TMPDIR_TEST" 2>/dev/null || true
}
trap cleanup EXIT

cargo build --manifest-path "$ROOT/Cargo.toml" -p slicc-cdp 2>/dev/null
PW="$ROOT/target/debug/playwright-cli"

PORT_FILE="$TMPDIR_TEST/http-port"
python3 -c "
import http.server, socketserver, sys, os
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(b'<html><head><title>Test Page</title></head><body>ok</body></html>')
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

SECRET_URL="http://127.0.0.1:${HTTP_PORT}/success?id_token=SECRET_TOKEN_VALUE_XYZ#access_token=SECRET_FRAG_VALUE_ABC"
PROFILE="$TMPDIR_TEST/chrome-profile"

/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --headless --disable-gpu --no-sandbox \
  --remote-debugging-port=0 \
  --user-data-dir="$PROFILE" \
  "$SECRET_URL" >/dev/null 2>&1 &
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

echo "=== tab-list (default, should redact) ==="
TAB_LIST=$("$PW" --cdp "$CDP_URL" tab-list 2>&1 || true)
echo "$TAB_LIST"
if echo "$TAB_LIST" | grep -q "SECRET_TOKEN_VALUE_XYZ"; then
  fail "tab-list leaks id_token value"
fi
if echo "$TAB_LIST" | grep -q "SECRET_FRAG_VALUE_ABC"; then
  fail "tab-list leaks fragment value"
fi
if ! echo "$TAB_LIST" | grep -q "id_token="; then
  fail "tab-list should keep parameter name id_token"
fi

echo "=== tab-list --full-urls (should show full) ==="
TAB_LIST_FULL=$("$PW" --cdp "$CDP_URL" --full-urls tab-list 2>&1 || true)
echo "$TAB_LIST_FULL"
if ! echo "$TAB_LIST_FULL" | grep -q "SECRET_TOKEN_VALUE_XYZ"; then
  fail "--full-urls tab-list should show id_token value"
fi

echo "=== snapshot (should redact Page URL) ==="
SNAP=$("$PW" --cdp "$CDP_URL" snapshot 2>&1 || true)
echo "$SNAP" | head -5
if echo "$SNAP" | grep -q "SECRET_TOKEN_VALUE_XYZ"; then
  fail "snapshot leaks id_token value in Page URL"
fi
if ! echo "$SNAP" | grep -q "Page URL:"; then
  fail "snapshot missing Page URL header"
fi

echo "=== goto then requests ==="
"$PW" --cdp "$CDP_URL" goto "$SECRET_URL" >/dev/null 2>&1 || true
sleep 0.5
REQS=$("$PW" --cdp "$CDP_URL" requests --static 2>&1 || true)
echo "$REQS"
if echo "$REQS" | grep -q "SECRET_TOKEN_VALUE_XYZ"; then
  fail "requests leaks id_token value"
fi

REQS_FULL=$("$PW" --cdp "$CDP_URL" --full-urls requests --static 2>&1 || true)
echo "$REQS_FULL"
if echo "$REQS_FULL" | grep -q "SECRET_TOKEN_VALUE_XYZ"; then
  echo "OK: --full-urls requests shows full URL"
elif echo "$REQS_FULL" | grep -q "No network"; then
  echo "SKIP: no network requests captured (tracing race)"
else
  fail "--full-urls requests should show id_token value or have no requests"
fi

echo ""
if [ "$FAILS" -gt 0 ]; then
  echo "FAILED: $FAILS test(s)"
  exit 1
else
  echo "ALL PASSED"
  exit 0
fi
