# slicc-cdp

Node package for SLICC's CDP layer. The JavaScript entry is still empty:

```js
import {} from '@ai-ecoverse/slicc-cdp';
```

`playwright-cli` is the Rust binary in `crates/playwright-cli`. It speaks Chrome CDP JSON. Connect with `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version`. It does not launch Chrome and does not invent a debugger path.

`curlwright` is the Rust binary in `crates/curlwright`. It runs curl-style requests as a page-context `fetch()` in an already-open tab, so that tab's cookies, origin, and service worker apply. It uses the same connect order and the shared CDP client in `crates/cdp-client`. The kernel wasm is `bin/curlwright.wasm`.

The kernel commands are the `wasm32-wasip1` builds at `bin/playwright-cli.wasm` and `bin/curlwright.wasm`, declared under `slicc.commands` with `slicc.abi` `wasi`. `npm run build:wasm` produces both, and the release workflow runs that before publish. On that target, TCP comes from the vendored homescoop `wasix-net` crate (`vendor/wasix-net`, pinned in `UPSTREAM`) instead of Rust std, which does not open sockets for `wasm32-wasip1`. `--version` prints the `package.json` version. A server close frame is reported as `websocket closed <code> <reason>`, and this client does not write again after it.

Node ≥ 24. `npm run lint` runs `slicc-lint`. Releases use semantic-release on `main`.
