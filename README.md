# slicc-cdp

`@ai-ecoverse/slicc-cdp` publishes `playwright-cli`, a Rust client for an already-running Chrome. It speaks Chrome DevTools Protocol JSON. The JavaScript CDP layer from SLICC is not in this package yet (`src/index.js` is `export {}`).

## playwright-cli

The binary is `crates/playwright-cli`. It does not launch Chrome and it does not invent a debugger path.

Connect, in order: `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version` and dial the returned `webSocketDebuggerUrl`. A `ws://` URL is dialed as given. An `http://` URL is discovery. On the native build a `wss://` URL is dialed as given. The WASI build has no TLS, so `wss://` and `https://` fail there. `--runtime <name>` is appended to the query of the URL that is opened and is not interpreted.

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`. `snapshot` prints ARIA refs such as `e1` and iframe refs such as `f1e5`. Other names from the command manifest print `playwright-cli <command>: not implemented yet` and exit non-zero. `--help` lists those names under "Not implemented yet". It does not yet match upstream help for the whole shared set.

A non-2xx discovery response prints the status and the body and exits non-zero. A server close frame prints `websocket closed <code> <reason>` and exits non-zero. After that frame is visible, the client does not write to the socket again. `--version` and `-V` print `playwright-cli` plus the `package.json` version.

## Kernel command

`slicc.abi` is `wasi`. The command is `playwright-cli` and the module is `bin/playwright-cli.wasm`. `npm run build:wasm` builds `wasm32-wasip1` and copies it there. The release workflow runs that before publish. Do not commit the wasm. On that target, TCP comes from the vendored homescoop `wasix-net` crate (`vendor/wasix-net`, pin recorded in `UPSTREAM`).

Inside the kernel the browser socket is `SLICC_CDP_URL`: `ws://127.0.0.1:9222/devtools/browser/<id>` from slicc-kernel. Port 9222 accepts connections only from inside the kernel.

## Package

Node ≥ 24. `npm run lint` runs `slicc-lint`. Releases use semantic-release on `main`. Git `package.json` stays `0.0.0`. The published version is the release tag.

`curlwright` is not in this package yet.
