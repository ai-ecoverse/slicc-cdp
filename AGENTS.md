# slicc-cdp

`@ai-ecoverse/slicc-cdp`. Node ≥ 24, ESM. `src/index.js` exports `CDPClient` and `connect`. `slicc-lint`. No comments in files we write. No CLAUDE.md. Extra tests: `test/unit/`, `test/integration/`, or `#[cfg(test)]`. semantic-release on `main`. `wasix-net` 0.1 comes from crates.io.

CLIs: `crates/playwright-cli`, `crates/curlwright`. Shared CDP: `crates/cdp-client`. Attach with `flatten: true` and a top-level `sessionId`. No `/devtools/page/<id>` socket. `--version` prints `package.json`.

Connect: `--cdp`, else `SLICC_CDP_URL`, else a `seven-profiles` DevToolsActivePort, else a session.json. Host open and tab-new skip the stale session endpoint. List and open reuse probe 9222-9229 read-only. Launch never binds them. Wasm uses 9222. `ws`/`wss` dial; `http` discovers. Non-2xx exits non-zero. WASI has no TLS, so `wss` and `https` error.

Kernel wasm: `bin/playwright-cli.wasm`, `bin/curlwright.wasm` (`slicc.abi` `wasi`). `npm run build:wasm` writes both. Do not commit them.
