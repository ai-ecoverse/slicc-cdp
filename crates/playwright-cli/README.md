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

`list` prints `connected`, `debug ports`, and `launchable`. An empty section is the line `none`. It does not launch a browser and does not open a websocket. On the host it probes `127.0.0.1:9222-9229`. `list --json` prints one JSON document and no text sections.

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

`--cdp` wins, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort, else a loopback endpoint this tool saved under the user config dir. A `.playwright-cli/session.json` in the working directory is ignored. In the kernel, the wasm builds keep it at `<state>/slicc/playwright-cli/sessions/<hash>.json`, where `<state>` is `$XDG_STATE_HOME`, else `$XDG_CONFIG_HOME`, else `$HOME/.config`, and the hash is of `$PWD`. When that is not writable, `playwright-cli` prints `playwright-cli: running without saved state (<reason>)` on stderr and the command still succeeds, except `route` and `unroute`, which fail because they only save state. Host `list` GETs `/json/version` and `/json/list` on `127.0.0.1:9222-9229`, and host `open` GETs `/json/version` there when that port is absent or unreachable. `open` attaches to the first of those ports that answers, with or without a seven tab, and an unreachable seven-profiles port falls through to it. A successful command records a loopback endpoint and its provenance. A probe-derived session is reused while that browser answers, and every command shows all of its tabs except those on protected origins. Saved state never dials a non-loopback host. Host `open` and `tab-new` treat an unreachable saved endpoint as absent. Other commands report that connection error. When nothing answers, launch binds the first free port from 9222 to 9229, or lets Chrome pick one. `SLICC_CDP_PORTS=<from>-<to>`, for tests, replaces that range and stops every dial to loopback 9222-9229. The wasm build still uses port 9222. A host build dials `wss://`. The `wasm32-wasip1` binaries in the npm package `@ai-ecoverse/slicc-cdp` have no TLS.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `console`, `requests`, `request`, `request-headers`, `request-body`, `response-headers`, `response-body`, `route`, `route-list`, `unroute`, `list`.

```
playwright-cli route "**/api/users" --body='[{"id":1}]' --content-type=application/json
playwright-cli route "**/*.png" --fulfill-file ./pixel.png
playwright-cli route-list
playwright-cli unroute "**/api/users"
```

Routes are saved with the session and apply while a later command is attached to a tab. Requests to seven's origins and the vault are never routed.

`playwright-cli --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.
