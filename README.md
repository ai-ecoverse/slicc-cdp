# slicc-cdp

Node package for SLICC's CDP layer. The JavaScript entry is still empty:

```js
import {} from '@ai-ecoverse/slicc-cdp';
```

`playwright-cli` is the Rust binary in `crates/playwright-cli`. It speaks Chrome CDP JSON. Connect with `--cdp`, else `SLICC_CDP_URL`, else `GET http://127.0.0.1:9222/json/version`. It does not launch Chrome and does not invent a debugger path.

The kernel command is the `wasm32-wasip1` build at `bin/playwright-cli.wasm`, declared as `slicc.commands.playwright-cli` with `slicc.abi` `wasi`. `npm run build:wasm` produces it, and the release workflow runs that before publish. On that target, TCP comes from the vendored homescoop `wasix-net` crate (`vendor/wasix-net`, pinned in `UPSTREAM`) instead of Rust std, which does not open sockets for `wasm32-wasip1`. `--version` prints the `package.json` version.

Node ≥ 24. `npm run lint` runs `slicc-lint`. Releases use semantic-release on `main`.
