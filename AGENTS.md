# slicc-cdp

`@ai-ecoverse/slicc-cdp`. Node ≥ 24, ESM. `src/index.js` exports `CDPClient` and `connect`. `slicc-lint`. No comments in files we write. No CLAUDE.md. Extra tests: `test/unit/` or `#[cfg(test)]`. semantic-release on `main`. `vendor/wasix-net` is homescoop `86df95d`, copied as-is.

CLIs: `crates/playwright-cli`, `crates/curlwright`. Shared CDP: `crates/cdp-client`. Attach with `flatten: true` and a top-level `sessionId`. No `/devtools/page/<id>` socket. `--version` prints `package.json`.

Connect: `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version` and dial `webSocketDebuggerUrl`. `ws`/`wss` dial; `http` discovers. `--runtime` is a query. Non-2xx prints status and body, exits non-zero. Does not launch Chrome. WASI has no TLS, so `wss` and `https` error. TCP is `wasix_net::TcpStream`.

Kernel wasm: `bin/playwright-cli.wasm`, `bin/curlwright.wasm` (`slicc.abi` `wasi`). `npm run build:wasm` writes both. Do not commit them.
