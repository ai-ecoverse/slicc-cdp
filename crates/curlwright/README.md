# curlwright

curl-style requests inside an already-open browser tab. The request is a page-context `fetch()`, so that tab's cookies, origin, and service worker apply.

```
cargo install curlwright
```

Host builds run on macOS, Linux, and Windows.

```
google-chrome --headless --remote-debugging-port=9333
curlwright --cdp http://127.0.0.1:9333 https://example.com/
curlwright --cdp http://127.0.0.1:9333 -I https://example.com/
```

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

The connect order matches `playwright-cli`: `--cdp`, else `SLICC_CDP_URL`, else discovery on port 9222. Without `--tab`, curlwright uses the `current` tab in `.playwright-cli/session.json` when that tab is still open, otherwise the single open tab on the URL's origin. Any other case exits 2 and lists `--tab` choices.

The npm package `@ai-ecoverse/slicc-cdp` ships the `wasm32-wasip1` build for SLICC. That build has no TLS.

`curlwright --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.
