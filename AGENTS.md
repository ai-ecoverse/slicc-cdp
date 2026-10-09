# slicc-cdp

`@ai-ecoverse/slicc-cdp`. Node ≥ 24, ESM. `src/index.js` stays `export {}`. Lint with `slicc-lint`. No comments in any file. Notes only in this file. No CLAUDE.md. Rejected unit tests stay in gitignored `test/unit/`. `#[cfg(test)]` in Rust sources is allowed. Publish from `main` with semantic-release (`release.yml`). Do not rewrite CI.

CLI: `crates/playwright-cli`, binary `playwright-cli`. One browser WebSocket. `Target.attachToTarget` uses `flatten: true` and a top-level `sessionId`. No `/devtools/page/<id>` socket.

Connect: `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version` and dial `webSocketDebuggerUrl`. `ws://` and `wss://` dial directly. `http://` is discovery. The path is opaque. Never invent a host or path. `--runtime` is a query on the opened URL. Non-2xx prints status and body and exits non-zero. Does not launch Chrome.

`native-tls` is `cfg(not(target_os = "wasi"))`. WASI rejects `wss` and `https`.
