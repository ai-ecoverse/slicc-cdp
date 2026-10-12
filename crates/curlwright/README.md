# curlwright

curl-style requests inside an already-open browser tab. The request is a page-context `fetch()`, so that tab's cookies, origin, and service worker apply.

```
cargo install curlwright
```

Host builds run on macOS, Linux, and Windows. Rust 1.83 or newer. Start Chrome with `--remote-debugging-port` and a non-default `--user-data-dir`. Chrome 136 and later ignore the debugging port on the default profile. `--headless` is optional: leave it off to open a window.

```
google-chrome --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
curlwright --cdp http://127.0.0.1:9333 https://example.com/
curlwright --cdp http://127.0.0.1:9333 -I https://example.com/
```

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

Connect order: `--cdp`, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort whose directory name is a browser id. Dotfiles, including `.migrated-from-profiles`, are not profiles. On the host, if none of those exist, curlwright exits without dialing port 9222. Ports 9222-9229 inside those files are not dialed. The wasm build still uses port 9222. Without `--tab`, curlwright uses the `current` tab this tool saved for the working directory under the user config dir when that tab is still open, otherwise the single open tab on the URL's origin. It ignores a `.playwright-cli/session.json` in the working directory and does not read the saved endpoint. Any other case exits 2 and lists `--tab` choices.

The npm package `@ai-ecoverse/slicc-cdp` ships the `wasm32-wasip1` build for SLICC. That build has no TLS.

`curlwright --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.
