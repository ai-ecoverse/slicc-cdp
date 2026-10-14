# slicc-cdp

`@ai-ecoverse/slicc-cdp` publishes `playwright-cli` and `curlwright`, Rust clients for an already-running Chrome. They speak Chrome DevTools Protocol JSON. The package root exports `CDPClient` and `connect`, one browser DevTools socket. Host `connect()` does not dial port 9222 unless `options.url` or `SLICC_CDP_URL` names it.

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

## playwright-cli

The binary is `crates/playwright-cli`. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab. It does not invent a debugger path.

Connect, in order: `--cdp`, else `SLICC_CDP_URL`, else a live `DevToolsActivePort` under `<config>/slicc/seven-profiles`, else a loopback endpoint this tool saved at `<config>/slicc/playwright-cli/sessions/<hash>.json`. A `.playwright-cli/session.json` in the working directory is ignored. A child of that directory is a profile only when its name is a browser id: it starts with a letter or digit, then letters, digits, `.`, `_`, or `-`, at most 64 characters, and it does not contain `..`. Dotfiles, including `.migrated-from-profiles`, are not profiles. `playwright-cli` does not migrate `slicc/profiles`. Host `list`, and `open` when that port is absent or unreachable, GET `/json/version` and `/json/list` on `127.0.0.1:9222-9229`. An unreachable seven-profiles port falls through to that probe, so `open` may attach to a personal browser that already has a seven tab. A successful command writes the loopback endpoint it used, its provenance, and the ids of tabs it created. A saved endpoint is reused only when it is loopback. A probe-derived session is reused only while that browser still has a seven tab, and later commands show only tabs this tool created. An explicit `--cdp` or `SLICC_CDP_URL` may name any host, and that URL is not saved unless it is loopback. Host `open` and `tab-new` treat an unreachable saved endpoint as absent, then probe and may launch. Other commands report that connection error. Launch never binds those ports. The wasm build still uses port 9222. A `ws://` URL is dialed as given. An `http://` URL is discovery. On the native build a `wss://` URL is dialed as given. The WASI build has no TLS, so `wss://` and `https://` fail there. `--runtime <name>` is appended to the query of the URL that is opened and is not interpreted.

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `console`, `requests`, `request`, `request-headers`, `request-body`, `response-headers`, `response-body`, `route`, `route-list`, `unroute`, `list`, `discover`, `webmcp-list`, `webmcp-call`. `snapshot` prints ARIA refs such as `e1` and iframe refs such as `f1e5`. Other names from the command manifest print `playwright-cli <command>: not implemented yet` and exit non-zero. `--help` lists those names under "Not implemented yet". It does not yet match upstream help for the whole shared set.

### discover

`playwright-cli discover` attaches to the running CDP session, watches every page target, and emits one event per finding of an agent-facing web standard. It runs until interrupted. `--once <url>` navigates to one URL, checks it, and exits.

Event JSON (NDJSON, one object per line):

```json
{"v":1,"standard":"ard","origin":"https://example.com","url":"https://example.com/.well-known/ard.json","via":"well-known","page":"https://example.com/","targetId":"...","status":"valid","summary":{...}}
```

Sinks:

- **stdout** (default): NDJSON. Pipe into `jq` or an agent reader.
- **`--exec <cmd>`**: run `<cmd>` once per event. The event JSON is on stdin. Three env vars are set: `SLICC_DISCOVER_STANDARD`, `SLICC_DISCOVER_ORIGIN`, `SLICC_DISCOVER_URL`. The command is never run through a shell with event data; event fields are never interpolated into the command line. One command runs at a time per origin. `--exec-timeout <seconds>` (default 10) kills the whole process group on timeout. A non-zero exit is logged to stderr and does not stop the watcher. Not available in wasm.
- **`--lick`**: write each event to `~/.slicc/discoveries/<fingerprint-hash>.json` and ship a `~/.slicc/watches/discover.json` so slicc-agent picks up changes. Default when `SLICC_CDP_URL` is set and no other sink is given.

Opt-out: `--off` exits immediately. A file at `<config>/slicc/discover.disabled` (the same user config dir as the saved session) disables the watcher; checked at start and on SIGHUP.

Example `--exec` hook for Claude Code:

```sh
playwright-cli discover --exec my-handler.sh
```

Where `my-handler.sh` reads JSON from stdin:

```sh
#!/bin/sh
echo "Found $SLICC_DISCOVER_STANDARD at $SLICC_DISCOVER_ORIGIN"
cat  # the full event JSON is on stdin
```

`route <pattern>` takes upstream's flags: `--status`, `--body`, `--content-type`, repeatable `--header "Name: Value"`, and `--remove-header a,b`. `--fulfill-file <path>` is a SLICC addition: that file's bytes (at most 16 MiB) are the body, and its extension picks the content type unless `--content-type` is given. Patterns are Playwright globs. With `--status`, `--body` or `--fulfill-file` a match is fulfilled, with `--header` added to the response. Otherwise the request goes on with `--header` added and `--remove-header` dropped; cookies come from the cookie jar after that, so they can't be removed. The newest matching route wins. `route-list` and `unroute [pattern]` print what upstream prints. Routes are saved with the session for the working directory and apply while a later command is attached to a tab: `open`, `goto`, `snapshot`, `click`, `eval` and the rest enable `Fetch` on that tab's session for the command and disable it before they detach. There is no daemon, so a request the page makes between commands is not routed. A pattern whose host names a `sliccy.ai` host or the vault is refused with `slicc: refused: target is on seven's origin`, and a request to a protected origin (`protectedOrigins` from `slicc-browsers`) is never routed, even under `**`.

Seven's tabs, the vault's and every other protected target are off limits on every path: an explicit `--cdp`, the saved endpoint, and the kernel facade. `tab-list` leaves them out, and every command that names one with `--tab` (or whose current tab became one) exits 1 with `slicc: refused: target is on seven's origin`. `open` and `goto` refuse protected URLs. A tab this tool touches can't load a protected document at all: a script, a link, a meta refresh or a redirect into the vault gets Chrome's error page, and the tab is refused from then on. The rules are `slicc-cdp-client`'s, ported from slicc-cli's `cdp` relay; see crates/cdp-client/README.md.

`webmcp-list` lists WebMCP tools from `document.modelContext.getTools()`. `webmcp-call <name> <json|->` calls `document.modelContext.executeTool`. Pass `-` as the JSON argument to read input from stdin. `--allow-consequential` permits calling tools whose `annotations.consequentialHint` is true. Note: `--allow-consequential` is advisory, not a security boundary. The `consequentialHint` annotation is declared by the page, so a hostile page can simply omit it. Treat the flag as a UX guard that reduces accidental side-effects when working with cooperative pages, not as a trust gate. When Chrome's native WebMCP API (`document.modelContext` with `Symbol.toStringTag === 'ModelContext'`) is detected, `executeTool` input is passed as an object; otherwise it is passed as a JSON string for polyfill compatibility.

A non-2xx discovery response prints the status and the body and exits non-zero. A server close frame prints `websocket closed <code> <reason>` and exits non-zero. After that frame is visible, the client does not write to the socket again. `--version` and `-V` print `playwright-cli` plus the `package.json` version.

## curlwright

`curlwright` is the Rust binary in `crates/curlwright`. It runs curl-style requests as a page-context `fetch()` in an already-open tab, so that tab's cookies, origin, and service worker apply. It uses `--cdp`, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort. On the host, if none of those exist, it exits without dialing port 9222. Ports 9222-9229 inside those files are not dialed. The wasm build still uses port 9222. The shared CDP client is `crates/cdp-client`. Without `--tab`, it uses the `current` target this tool saved for the working directory under the user config dir when that tab is still open, otherwise the single open tab on the request URL's origin. It does not read a `.playwright-cli/session.json` in the working directory and it does not read the saved endpoint. Any other case exits 2 and lists `--tab` choices. A tab on a different origin is not used unless `--tab` or that session names it. A request URL on a protected origin (any `sliccy.ai` host, the vault, or `//` to one) is refused before any tab is chosen, with `curlwright: slicc: refused: target is on seven's origin`, and so is `--tab` on a protected tab.

## Kernel commands

`slicc.abi` is `wasi`. The commands are `playwright-cli` (`bin/playwright-cli.wasm`) and `curlwright` (`bin/curlwright.wasm`). `npm run build:wasm` builds both for `wasm32-wasip1` and copies them there. The release workflow runs that before publish. Do not commit the wasm. On that target, TCP comes from the `wasix-net` 0.1 crate on crates.io.

Inside the kernel the browser socket is `SLICC_CDP_URL`: `ws://127.0.0.1:9222/devtools/browser/<id>` from slicc-kernel. Port 9222 accepts connections only from inside the kernel.

Routes in the kernel use these CDP methods through the facade, always with a tab's `sessionId`: `Fetch.enable`, `Fetch.disable`, `Fetch.continueRequest`, `Fetch.fulfillRequest`, the `Fetch.requestPaused` event, and `Target.getTargetInfo`. The `slicc-cdp-client` API can also send `Fetch.failRequest`, `Fetch.getResponseBody` and `Fetch.continueResponse`; `playwright-cli` doesn't.

## Backend contract

This is what `playwright-cli`, `curlwright`, `slicc-cdp-client` and the JS `CDPClient` need from any CDP backend: Chrome itself, the slicc-extension `sliccExtension.cdp` backend, bios's link, or slicc-node. It was taken from the code at `f6d43e8`. `crates/cdp-client/src/contract.rs` keeps the same lists, and its unit test checks them against every call in `crates/*/src` that sends no `sessionId` and against this section.

One browser WebSocket carries everything. Requests are `{id, method, params}` with an optional top-level `sessionId`. Replies are `{id, result}` or `{id, error: {message}}`. Events are `{method, params}`, with `sessionId` when they come from a session. Replies to ids the client didn't send are dropped.

### Browser-level methods

These are sent with no `sessionId`.

- `Target.getTargets` with `{}`. The client reads `targetInfos[]` and, in each one, `targetId`, `type`, `url`, `title` and `openerId`. Only `type: "page"` is listed as a tab.
- `Target.getTargetInfo` with `{targetId}`. The client reads `targetInfo.type`, `targetInfo.url` and `targetInfo.openerId`. It asks about openers, and about a `frameId` the caller named; an error for a frame id is fine.
- `Target.setDiscoverTargets` with `{discover: true}`, once per connection, before the first attach.
- `Target.createTarget` with `{url: "about:blank"}`, and `background` from `playwright-cli`. It must return `targetId`. The client always creates a blank tab, attaches its guard, and only then navigates it over the guard session. Any other `url` is never sent here.
- `Target.attachToTarget` with `{targetId, flatten: true}`. It must return `sessionId`. Every later command for that tab carries that `sessionId` at the top level. Non-flat attach is refused before it is sent.
- `Target.detachFromTarget` with `{sessionId}`.
- `Target.closeTarget` with `{targetId}`.
- `Target.activateTarget` with `{targetId}`. `open` ignores an error; `tab-select` reports it.
- `Browser.getVersion` with `{}`. It is optional and only `webmcp-call` uses it. `product` is read as `Chrome/NNN` or `HeadlessChrome/NNN`. On an error or any other product, `executeTool` input is passed as a string.
- `Slicc.action` with `{phase, kind, ...}`. It is an optional notification for the slicc panel. The client never waits for the reply, so a backend may ignore it or answer with an error.

### Browser-level events

- `Target.targetCreated` with `targetInfo`: `targetId`, `type`, `url`, `openerId`.
- `Target.targetInfoChanged` with the same `targetInfo`. It must fire on every committed navigation, because the protected-origin guard depends on it.
- `Target.targetDestroyed` with `targetId`.
- `Target.detachedFromTarget` with `sessionId`.

### Protected tabs

A backend may leave out of `Target.getTargets` the tabs the client has never seen, such as seven's and the vault's. A tab the client could see that later becomes protected must produce `Target.targetInfoChanged` with its real `url`, or `Target.targetDestroyed`. It must never go silent.

The guard on a tab is its own flat session. On it the client sends `Fetch.enable` for `Document` requests at the `Request` stage, `Network.enable` and `Network.setBypassServiceWorker`, which must succeed, and `Page.enable` and `Runtime.enable`, which may fail with an error message.

### Session-level domains

Everything else goes over a flattened session, with a top-level `sessionId`:

- `Page`
- `Runtime`
- `DOM`
- `Input`
- `Network`
- `Fetch`
- `Accessibility`

The CLIs send no `Emulation` or `IO` command. `slicc-cdp-client` lets a caller `IO.read` a stream that one of the session replies above handed out.

## Package

Node ≥ 24. `npm run lint` runs `slicc-lint`. Releases use semantic-release on `main`. Git `package.json` stays `0.0.0`. The published version is the release tag.

## Install and run

```
curl -fsSL https://get.sliccy.ai/playwright-cli | sh
curl -fsSL https://get.sliccy.ai/curlwright | sh
```

Both at once, straight from the release:

```
curl -fsSL https://github.com/ai-ecoverse/slicc-cdp/releases/latest/download/install.sh | sh
```

Flags: `sh -s -- --version 1.2.3 --dir /usr/local/bin --only playwright-cli`.

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
