# slicc-cdp

`@ai-ecoverse/slicc-cdp`. Node ≥ 24, ESM. `src/index.js` stays `export {}`. `slicc-lint`. No comments in files we write. No CLAUDE.md. Rejected tests stay in `test/unit/`. `#[cfg(test)]` is allowed. Publish from `main` with semantic-release. `vendor/wasix-net` is homescoop `86df95d`, copied as-is.

CLI: `crates/playwright-cli`. `Target.attachToTarget` uses `flatten: true` and a top-level `sessionId`. No `/devtools/page/<id>` socket. `--version` prints the `package.json` version.

Connect: `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version` and dial `webSocketDebuggerUrl`. `ws` and `wss` dial directly. `http` is discovery. `--runtime` is a query on the opened URL. Non-2xx prints status and body, then exits non-zero. Does not launch Chrome. No `native-tls` on WASI, so `wss` and `https` error. TCP is `wasix_net::TcpStream`.

Kernel command: `slicc.abi` `wasi`, wasm `bin/playwright-cli.wasm`. `npm run build:wasm` writes it. Do not commit the wasm.
