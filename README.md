# slicc-cdp

`@ai-ecoverse/slicc-cdp` publishes `playwright-cli` and `curlwright`, Rust clients for an already-running Chrome. They speak Chrome DevTools Protocol JSON. The package root exports `CDPClient` and `connect`, one browser DevTools socket. Host `connect()` does not dial port 9222 unless `options.url` or `SLICC_CDP_URL` names it.

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

## playwright-cli

The binary is `crates/playwright-cli`. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab. It does not invent a debugger path.

Connect, in order: `--cdp`, else `SLICC_CDP_URL`, else a live `DevToolsActivePort` under `<config>/slicc/seven-profiles`, else a loopback endpoint this tool saved at `<config>/slicc/playwright-cli/sessions/<hash>.json`. A `.playwright-cli/session.json` in the working directory is ignored. A child of that directory is a profile only when its name is a browser id: it starts with a letter or digit, then letters, digits, `.`, `_`, or `-`, at most 64 characters, and it does not contain `..`. Dotfiles, including `.migrated-from-profiles`, are not profiles. `playwright-cli` does not migrate `slicc/profiles`. Host `list`, and `open` when that port is absent or unreachable, GET `/json/version` and `/json/list` on `127.0.0.1:9222-9229`. An unreachable seven-profiles port falls through to that probe, so `open` may attach to a personal browser that already has a seven tab. A successful command writes the loopback endpoint it used, its provenance, and the ids of tabs it created. A saved endpoint is reused only when it is loopback. A probe-derived session is reused only while that browser still has a seven tab, and later commands show only seven tabs and tabs this tool created. An explicit `--cdp` or `SLICC_CDP_URL` may name any host, and that URL is not saved unless it is loopback. Host `open` and `tab-new` treat an unreachable saved endpoint as absent, then probe and may launch. Other commands report that connection error. Launch never binds those ports. The wasm build still uses port 9222. A `ws://` URL is dialed as given. An `http://` URL is discovery. On the native build a `wss://` URL is dialed as given. The WASI build has no TLS, so `wss://` and `https://` fail there. `--runtime <name>` is appended to the query of the URL that is opened and is not interpreted.

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `list`. `snapshot` prints ARIA refs such as `e1` and iframe refs such as `f1e5`. Other names from the command manifest print `playwright-cli <command>: not implemented yet` and exit non-zero. `--help` lists those names under "Not implemented yet". It does not yet match upstream help for the whole shared set.

A non-2xx discovery response prints the status and the body and exits non-zero. A server close frame prints `websocket closed <code> <reason>` and exits non-zero. After that frame is visible, the client does not write to the socket again. `--version` and `-V` print `playwright-cli` plus the `package.json` version.

## curlwright

`curlwright` is the Rust binary in `crates/curlwright`. It runs curl-style requests as a page-context `fetch()` in an already-open tab, so that tab's cookies, origin, and service worker apply. It uses `--cdp`, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort. On the host, if none of those exist, it exits without dialing port 9222. Ports 9222-9229 inside those files are not dialed. The wasm build still uses port 9222. The shared CDP client is `crates/cdp-client`. Without `--tab`, it uses the `current` target this tool saved for the working directory under the user config dir when that tab is still open, otherwise the single open tab on the request URL's origin. It does not read a `.playwright-cli/session.json` in the working directory and it does not read the saved endpoint. Any other case exits 2 and lists `--tab` choices. A tab on a different origin is not used unless `--tab` or that session names it.

## Kernel commands

`slicc.abi` is `wasi`. The commands are `playwright-cli` (`bin/playwright-cli.wasm`) and `curlwright` (`bin/curlwright.wasm`). `npm run build:wasm` builds both for `wasm32-wasip1` and copies them there. The release workflow runs that before publish. Do not commit the wasm. On that target, TCP comes from the `wasix-net` 0.1 crate on crates.io.

Inside the kernel the browser socket is `SLICC_CDP_URL`: `ws://127.0.0.1:9222/devtools/browser/<id>` from slicc-kernel. Port 9222 accepts connections only from inside the kernel.

## Package

Node ≥ 24. `npm run lint` runs `slicc-lint`. Releases use semantic-release on `main`. Git `package.json` stays `0.0.0`. The published version is the release tag.

## Install and run

`cargo install slicc-cdp` installs the `playwright-cli` command. `cargo install curlwright` installs `curlwright`. Host builds run on macOS, Linux, and Windows. TLS there is `native-tls` (Secure Transport, OpenSSL, or SChannel). The `slicc-cdp` crate needs Rust 1.88. `curlwright` and `slicc-cdp-client` need Rust 1.83.

```
cargo install slicc-cdp
cargo install curlwright
```

`@ai-ecoverse/slicc-cdp` on npm is the SLICC package. It ships `bin/playwright-cli.wasm` and `bin/curlwright.wasm`, built for `wasm32-wasip1`. Those builds have no TLS, so `wss://` and `https://` fail in the kernel. The same commands exist there; the Cargo install is the host build, not those wasm files.

Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab. `curlwright` does not launch a browser. You can still start Chrome with `--remote-debugging-port` and a non-default `--user-data-dir`, then pass that HTTP endpoint as `--cdp`. Chrome 136 and later ignore the debugging port on the default profile, and an already-open Chrome ignores a second launch. `--headless` is optional: leave it off to open a window. The examples use port 9333.

macOS:

```
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
```

Linux:

```
google-chrome --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
```

```
playwright-cli --cdp http://127.0.0.1:9333 open https://example.com
playwright-cli --cdp http://127.0.0.1:9333 snapshot
playwright-cli --cdp http://127.0.0.1:9333 tab-list
playwright-cli list
curlwright --cdp http://127.0.0.1:9333 https://example.com/
curlwright --cdp http://127.0.0.1:9333 -I https://example.com/
```

`playwright-cli list` prints `connected`, `debug ports`, and `launchable`. An empty section is the line `none`. `connected` includes an explicit `--cdp` or `SLICC_CDP_URL` browser and each live `DevToolsActivePort`, as `browser - <websocket url>`. It does not launch a browser and does not open a websocket. On the host it also probes catalog ports `127.0.0.1:9222-9229` with GET `/json/version` and GET `/json/list`, reports `source` `probe`, and drops every tab that is not a seven origin. `playwright-cli list --json` prints one JSON document and no text sections. A child of `seven-profiles` is a `profile`. A default user-data directory is a `browser`. `https://<label>.sliccy.ai:443/` counts as seven. Any other explicit port does not.

`--cdp` still wins over `SLICC_CDP_URL`, which wins over a live `seven-profiles` DevToolsActivePort, which wins over a loopback session this tool saved under the user config dir, for `playwright-cli`. A `.playwright-cli/session.json` in the working directory is ignored. An unreachable saved endpoint does not stop `open` or `tab-new`. `curlwright` does not read that session endpoint. Saved state never dials a non-loopback host. `open` then reuses a catalog probe port only when that browser already has a seven tab, including when the seven-profiles port is unreachable. Launch never binds `127.0.0.1:9222-9229`. `curlwright` still does not dial those ports unless `--cdp` or `SLICC_CDP_URL` names that URL. Node `connect()` requires `options.url` or `SLICC_CDP_URL` and does not dial port 9222 by itself. The wasm build still uses port 9222. A crate installed from crates.io prints its crate version. In this repository, and in the npm wasm builds, `--version` prints the `package.json` version. The release workflow writes that version into the crate manifests before `cargo publish`, and only after semantic-release has published the npm package. Git keeps the crate versions at `0.0.0`.
