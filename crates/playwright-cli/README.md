# slicc-cdp

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

```
cargo install slicc-cdp
```

That installs the `playwright-cli` command. It talks to a Chrome that is already running and does not launch the browser.

Host builds run on macOS, Linux, and Windows. Rust 1.88 or newer. Start Chrome with `--remote-debugging-port`, then pass that endpoint:

```
google-chrome --headless --remote-debugging-port=9333
playwright-cli --cdp http://127.0.0.1:9333 open https://example.com
playwright-cli --cdp http://127.0.0.1:9333 snapshot
playwright-cli --cdp http://127.0.0.1:9333 tab-list
```

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

`--cdp` wins, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version`. A host build dials `wss://`. The `wasm32-wasip1` binaries in the npm package `@ai-ecoverse/slicc-cdp` have no TLS.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`.

`playwright-cli --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.
