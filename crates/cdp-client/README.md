# slicc-cdp-client

Shared Chrome DevTools Protocol connection for the `playwright-cli` and `curlwright` binaries.

The crates.io name is `slicc-cdp-client`. The Rust library name stays `cdp_client`:

```
cdp-client = { package = "slicc-cdp-client", version = "1" }
```

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

Host builds use `native-tls` for `wss://` and for `https://` discovery, on macOS, Linux, and Windows. The `wasm32-wasip1` build has no TLS. This crate does not launch Chrome. Callers pass a DevTools WebSocket URL or an `http://` discovery URL.

`playwright-cli` and `curlwright` are the command-line tools. `@ai-ecoverse/slicc-cdp` on npm ships their WASI builds for SLICC.
