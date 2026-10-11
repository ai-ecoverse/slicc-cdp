# slicc-cdp

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

```
cargo install slicc-cdp
```

That installs the `playwright-cli` command. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab.

Host builds run on macOS, Linux, and Windows. Rust 1.88 or newer. Start Chrome with `--remote-debugging-port` and a non-default `--user-data-dir`, then pass that endpoint. Chrome 136 and later ignore the debugging port on the default profile. `--headless` is optional: leave it off to open a window.

```
google-chrome --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
playwright-cli --cdp http://127.0.0.1:9333 open https://example.com
playwright-cli --cdp http://127.0.0.1:9333 snapshot
playwright-cli --cdp http://127.0.0.1:9333 tab-list
playwright-cli list
```

`list` prints `connected`, `debug ports`, and `launchable`. An empty section is the line `none`. It does not launch a browser. `list --json` prints one JSON document and no text sections.

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

`--cdp` wins, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort. The host never dials `127.0.0.1:9222-9229` unless `--cdp` or `SLICC_CDP_URL` names that URL. The wasm build still uses port 9222. A host build dials `wss://`. The `wasm32-wasip1` binaries in the npm package `@ai-ecoverse/slicc-cdp` have no TLS.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `list`.

`playwright-cli --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.
