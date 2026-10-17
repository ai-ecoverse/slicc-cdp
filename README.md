# slicc-cdp

`@ai-ecoverse/slicc-cdp` publishes `playwright-cli` and `curlwright`, Rust clients for an already-running Chrome. They speak Chrome DevTools Protocol JSON. The package root exports `CDPClient` and `connect`, one browser DevTools socket, and `findBrowser`, `installBrowser` and `launchBrowser` (see [Browser](#browser)). Host `connect()` does not dial port 9222 unless `options.url` or `SLICC_CDP_URL` names it.

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

## playwright-cli

The binary is `crates/playwright-cli`. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab. It does not invent a debugger path.

Connect, in order: `--cdp`, else `SLICC_CDP_URL`, else a live `DevToolsActivePort` under `<config>/slicc/seven-profiles`, else a loopback endpoint this tool saved at `<config>/slicc/playwright-cli/sessions/<hash>.json`. A `.playwright-cli/session.json` in the working directory is ignored. In the kernel, the wasm builds keep it at `<state>/slicc/playwright-cli/sessions/<hash>.json`, where `<state>` is `$XDG_STATE_HOME`, else `$XDG_CONFIG_HOME`, else `$HOME/.config`, and the hash is of `$PWD`. When that is not writable, `playwright-cli` prints `playwright-cli: running without saved state (<reason>)` on stderr and the command still succeeds, except `route` and `unroute`, which fail because they only save state. A child of that directory is a profile only when its name is a browser id: it starts with a letter or digit, then letters, digits, `.`, `_`, or `-`, at most 64 characters, and it does not contain `..`. Dotfiles, including `.migrated-from-profiles`, are not profiles. `playwright-cli` does not migrate `slicc/profiles`. Host `list` GETs `/json/version` and `/json/list` on `127.0.0.1:9222-9229`, and host `open` GETs `/json/version` there when that port is absent or unreachable. `open` attaches to the first of those ports whose `/json/version` answers, with or without a seven tab, and an unreachable seven-profiles port falls through to that probe. A successful command writes the loopback endpoint it used, its provenance, and the ids of tabs it created. It also keeps each tab's console and request log until the tab closes., redacted as listed below. A saved endpoint is reused only when it is loopback. A probe-derived session is reused while that browser still answers `/json/version`. Every command shows and drives all of a browser's tabs, not only the ones this tool created, except tabs on protected origins, which stay refused. An explicit `--cdp` or `SLICC_CDP_URL` may name any host, and that URL is not saved unless it is loopback. Host `open` and `tab-new` treat an unreachable saved endpoint as absent, then probe and may launch. Other commands report that connection error. When nothing answers, launch passes `--remote-debugging-port` with the first free port from 9222 to 9229, or 0 when all are busy. `SLICC_CDP_PORTS=<from>-<to>` replaces 9222-9229 for the probes and the launch, and while it is set no command dials loopback 9222-9229. It is for tests. The wasm build still uses port 9222. A `ws://` URL is dialed as given. An `http://` URL is discovery. On the native build a `wss://` URL is dialed as given. The WASI build has no TLS, so `wss://` and `https://` fail there. `--runtime <name>` is appended to the query of the URL that is opened and is not interpreted.

Only the saved session file is redacted:

- **Headers:** the values of `Cookie`, `Set-Cookie`, `Authorization`, `Proxy-Authorization`, and any request or response header whose name contains `token`, `secret`, `key`, `auth`, `session`, `csrf` or `cookie`, become `[redacted]`.
- **URLs:** request URLs, URL-valued headers such as `Referer` and `Location`, and console source URLs lose their user info and fragment. Query values whose key contains `token`, `secret`, `key`, `auth`, `code`, `sig`, `session` or `password` become `[redacted]`.
- **URL-valued headers:** `Location`, `Content-Location`, `Referer`, `Refresh` and `Link` values that are relative, such as `Location: /next?token=abc`, are resolved against the request URL and get the same query redaction. A same-origin value is saved as its path and query.
- **Request and response bodies:** dropped. The command that loaded the page keeps them in memory, but a later `response-body <index>` prints `The response body is not kept in the saved session.` and exits non-zero, and `request` says the same instead of pointing to `response-body`.
- **Console entries:** dropped when the text holds a `Bearer` or `Basic` credential, a JWT, `cookie` with an `=`, or a pair such as `token=…` or `"password": "…"` whose value is at least 8 characters. They are also dropped when the text holds a bare token with a well-known prefix (`sk-`, `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`, `glpat-`, `xoxa-`, `xoxb-`, `xoxp-`, `xoxr-`, or a 20-character `AKIA…`/`ASIA…` key), or a run of at least 32 characters that looks random: hex with digits and letters, or base64 that mixes upper case, lower case and digits. Other entries are cut at 2048 bytes.

The output of the command that captured an entry still shows it. Route headers and the saved endpoint are kept as they are.

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `console`, `requests`, `request`, `request-headers`, `request-body`, `response-headers`, `response-body`, `route`, `route-list`, `unroute`, `list`, `discover`, `webmcp-list`, `webmcp-call`. `snapshot` prints ARIA refs such as `e1` and iframe refs such as `f1e5`. Other names from the command manifest print `playwright-cli <command>: not implemented yet` and exit non-zero. `--help` lists those names under "Not implemented yet". It does not yet match upstream help for the whole shared set.

### discover

`playwright-cli discover` attaches to the running CDP session, watches every page target, and emits one event per finding of an agent-facing web standard. It runs until interrupted. `--once <url>` navigates to one URL, checks it, and exits.

Event JSON (NDJSON, one object per line):

```json
{"v":1,"standard":"ard","origin":"https://example.com","url":"https://example.com/.well-known/ard.json","via":"well-known","page":"https://example.com/","targetId":"...","status":"valid","summary":{...}}
```

Payments over HTTP 402: every response in a watched tab, subresources included, with status 402 is decoded as a payment challenge. The watcher never pays, retries, or sends `PAYMENT-SIGNATURE` or `Authorization: Payment`; it only reads the response the page already got. A protected origin's response is never read.

- **`x402`** ([x402](https://github.com/coinbase/x402/tree/main/specs)). V2 is read from the base64 JSON `PAYMENT-REQUIRED` header. V1 is read from the JSON body of a 402 with a JSON content type. The body is fetched once with `Network.getResponseBody` after it has loaded, up to 1 MiB. `summary` is `{status: 402, version, resource, offers: [{scheme, network, amount, asset, payTo, expires, maxTimeoutSeconds}]}`. `amount` is V2's `amount` or V1's `maxAmountRequired`, kept as the string the server sent.
- **`mpp`** ([Machine Payments Protocol](https://datatracker.ietf.org/doc/draft-httpauth-payment/)). It is read from each `WWW-Authenticate: Payment` challenge, whose `request` parameter is decoded as base64url JSON. `summary` is `{status: 402, resource, offers: [{method, intent, realm, amount, asset, expires}]}`, with `amount` and `asset` (the request's `currency`) as sent. One response can carry both standards.
- **Invalid challenges:** a challenge that doesn't decode is reported with `status: "invalid"` and a `reason`, and the watcher goes on. Its `summary` is only `{status: 402, challenge}`, the raw header capped at 1 KiB. Possible reasons:
  - `PAYMENT-REQUIRED is not base64 JSON`;
  - `request is not base64url JSON`;
  - `no x402 payload`: a legacy `X-Payment` header, or a body without `x402Version`/`accepts`;
  - `body unavailable`: the body was evicted or the load failed.
- **Limits:** events are deduplicated per origin, standard and resource, since each paid resource has its own price. A tab reports at most 50 events per payment standard, so a page that loops 402s can't flood the output. `--once` waits up to 2 s after the load event for the tab's requests to finish (and 402 bodies to be read), so the 402s of subresources are reported. A `--once` run can therefore take up to 2 s longer than the load.

Sinks:

- **stdout** (default): NDJSON. Pipe into `jq` or an agent reader.
- **`--exec <cmd>`**: run `<cmd>` once per event. The event JSON is on stdin. Three env vars are set: `SLICC_DISCOVER_STANDARD`, `SLICC_DISCOVER_ORIGIN`, `SLICC_DISCOVER_URL`. The command is never run through a shell with event data; event fields are never interpolated into the command line. One command runs at a time per origin. `--exec-timeout <seconds>` (default 10) kills the whole process group on timeout. A non-zero exit is logged to stderr and does not stop the watcher. Not available in wasm.
- **`--lick`**: write each event to `~/.slicc/discoveries/<fingerprint-hash>.json` and ship a `~/.slicc/watches/discover.json` so slicc-agent picks up changes. Default when `SLICC_CDP_URL` is set and no other sink is given.

Opt-out: `--off` exits immediately. A file at `<config>/slicc/discover.disabled` (the same user config dir as the saved session) disables the watcher; checked at start and on SIGHUP.

UCP ([Universal Commerce Protocol](https://ucp.dev/specification/overview/)) is probed by default: one same-origin `GET /.well-known/ucp` per origin, through the guard and capped like the other well-known probes. The listed endpoints are never called.
- **Validation:** the profile must have `ucp.version` (`YYYY-MM-DD`), `ucp.services` and `ucp.payment_handlers` (both required, empty allowed). Each service entry needs `version`, `spec`, a `transport` of `rest`, `mcp`, `a2a` or `embedded`, and an `endpoint` (`embedded` may give only a `schema`).
- **Summary:** `{version, services: {name: [{transport, endpoint}]}, capabilities: ["name@version"], paymentHandlers, hasKeys, ap2, mcp, a2a}`.
  - `mcp` lists the MCP server candidates and `a2a` the agent-card URLs. Protected endpoints are left out.
  - `hasKeys` only says whether `keys[]` is non-empty; no key material is reported.
  - `ap2` is true when a capability or payment handler names the AP2 extension (a name segment starting with `ap2`, such as `dev.ucp.common.payment.ap2_mandate`). AP2 has no discovery of its own.
- **Versions:** a version other than 2026-01-23, 2026-04-08 or 2026-08-25 is still `valid`, with `summary.note: "unknown version"`.

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

`curlwright` is the Rust binary in `crates/curlwright`. It runs curl-style requests as a page-context `fetch()` in an already-open tab, so that tab's cookies, origin, and service worker apply. It uses `--cdp`, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort. On the host, if none of those exist, it uses the first of `127.0.0.1:9222-9229` whose `/json/version` answers, in the order `playwright-cli` probes them, and exits only when none does. Ports 9222-9229 inside those files are dialed like any other. The wasm build still uses port 9222. The shared CDP client is `crates/cdp-client`. Without `--tab`, it uses the `current` target this tool saved for the working directory under the user config dir when that tab is still open, otherwise the single open tab on the request URL's origin. It does not read a `.playwright-cli/session.json` in the working directory and it does not read the saved endpoint. Any other case exits 2 and lists `--tab` choices. A tab on a different origin is not used unless `--tab` or that session names it. A request URL on a protected origin (any `sliccy.ai` host, the vault, or `//` to one) is refused before any tab is chosen, with `curlwright: slicc: refused: target is on seven's origin`, and so is `--tab` on a protected tab.

## Browser

On the host, the package can find, install and launch a Chrome for a test suite or a script. This is Node only, not in the wasm commands.

- **`findBrowser({ version })`:** `$CHROME_PATH`, else the newest Chrome for Testing that `install-browser` put under the install root, else a system Chrome or Chromium (`/Applications`, `/usr/bin`, `/snap/bin`, Program Files). With `version` (a full version or a milestone) only that installed Chrome for Testing counts, and `$CHROME_PATH` is not used. Returns `{ executable, source, version? }` or `null`.
- **`installBrowser({ version, channel, withDeps })`:** downloads Chrome for Testing for this platform, unzips it, checks that `--version` prints the expected version (on Windows, which prints nothing, that the build's `<version>.manifest` is there), and marks it with `.slicc-cdp-browser.json`. An exact version that is already installed is used without the network. The install root is `$SLICC_BROWSERS_PATH`, else `~/Library/Caches/slicc-cdp/browsers`, `$XDG_CACHE_HOME/slicc-cdp/browsers` (default `~/.cache`), or `%LOCALAPPDATA%\slicc-cdp\browsers`. `withDeps` is opt-in. On Linux it runs `apt-get satisfy` on the build's `deb.deps`, as root or with `sudo -n`, and otherwise fails saying so.
- **`launchBrowser({ executable, userDataDir, port, headless, sandbox, args, platform })`:** starts the browser with `--remote-debugging-port` (0 by default) on `127.0.0.1`. Without `userDataDir` the profile is a fresh `slicc-cdp-profile-*` directory in the temp dir, never the user's, and `close()` kills the browser and removes the profile. Ports 9222-9229 are refused, and so are `args` that set the debugging port, address, pipe, profile or profile directory. They are matched the way Chrome parses its command line: one or two leading dashes, or a leading `/` when `platform` (default `process.platform`) is `win32`, the name up to the first `=`, in any case. Anything after a `--` is not a switch to Chrome and is left alone. A fixed `port` that already answers is refused. Returns `{ child, executable, userDataDir, port, wsUrl, close }`. With port 0 the `wsUrl` comes from a fresh `DevToolsActivePort`, which is removed before launch. Otherwise it comes from `/json/version` on that port. The sandbox stays on unless `sandbox: false`. Linux without unprivileged user namespaces (Ubuntu 23.10 and later, GitHub's `ubuntu-latest` included) needs that, or Chrome aborts with "No usable sandbox".

The `slicc-cdp` bin does the same from a shell:

```
npx slicc-cdp install-browser --version 153.0.8010.12   # prints the executable
npx slicc-cdp install-browser --channel stable --with-deps
npx slicc-cdp find-browser
```

The bin runs through an npm bin link, a symlinked package dir, and `--preserve-symlinks-main`, because it compares the real paths of `process.argv[1]` and its own module. Node resolves a module's relative imports from the path it was loaded by, so `node --preserve-symlinks-main` on a symlink to `cli.js` alone cannot load `./browser.js`; link the package dir instead.

Trust: Chrome for Testing publishes no checksums or signatures for its zips. `installBrowser` downloads only over HTTPS, only from `googlechromelabs.github.io` (the version index) and `storage.googleapis.com` (the zips). Redirects are refused. The only checks are the byte count and the version, so you are trusting those Google hosts and TLS. Pin an exact version (`--version x.y.z.w`) in CI, so a run gets the build you tested.

## Kernel commands

`slicc.abi` is `wasi`. The commands are `playwright-cli` (`bin/playwright-cli.wasm`) and `curlwright` (`bin/curlwright.wasm`). `npm run build:wasm` builds both for `wasm32-wasip1` and copies them there. The release workflow runs that before publish. Do not commit the wasm. On that target, TCP comes from the `wasix-net` 0.1 crate on crates.io.

Inside the kernel the browser socket is `SLICC_CDP_URL`: `ws://127.0.0.1:9222/devtools/browser/<id>` from slicc-kernel. Port 9222 accepts connections only from inside the kernel.

Routes in the kernel use these CDP methods through the facade, always with a tab's `sessionId`: `Fetch.enable`, `Fetch.disable`, `Fetch.continueRequest`, `Fetch.fulfillRequest`, the `Fetch.requestPaused` event, and `Target.getTargetInfo`. The `slicc-cdp-client` API can also send `Fetch.failRequest`, `Fetch.getResponseBody` and `Fetch.continueResponse`; `playwright-cli` doesn't.

## Backend contract

This is what `playwright-cli`, `curlwright`, `slicc-cdp-client` and the JS `CDPClient` need from a CDP backend: Chrome, slicc-extension's `sliccExtension.cdp`, bios's link and guard, or slicc-node. `crates/cdp-client/src/contract.rs` holds the same lists. Its unit test scans the production code in `crates/*/src` and fails when a list misses a sessionless call, a handled browser-level event or a session domain, or names one the code no longer uses. It also fails when this section disagrees with the lists or with `shield.rs`, or when a cited function or const no longer holds what it is cited for.

One browser WebSocket carries everything. An `http` endpoint is read as `GET /json/version`, which must return `webSocketDebuggerUrl` (`crates/cdp-client/src/connect.rs` `websocket_url_from_version`). When nothing names an endpoint, the host probes the `probe_ports` of the slicc-browsers catalog, `127.0.0.1:9222-9229` (`crates/cdp-client/src/ports.rs` `catalog_from`). Requests are `{id, method, params}` with an optional top-level `sessionId`. Replies are `{id, result}` or `{id, error: {message}}`. The client reports an error as `<method>: <message>` (`crates/cdp-client/src/cdp.rs` `raw_call`). Events are `{method, params}`, with `sessionId` when they come from a session.

### Browser-level methods

These are sent with no `sessionId`.

- `Target.getTargets` with `{}`. Reads `targetId`, `type`, `url` and `title` from each of `targetInfos`. Only `type: "page"` is a tab. The client never lists a protected tab, and a backend may leave them out (`crates/playwright-cli/src/browser.rs` `list_pages`).
- `Target.getTargetInfo` with `{targetId}`. Reads `targetInfo.type`, `url`, `openerId`. An error for a `frameId` is fine (`crates/cdp-client/src/cdp.rs` `target_protected`).
- `Target.setDiscoverTargets` with `{discover: true}`, once per connection before the first guard attach (`crates/cdp-client/src/cdp.rs` `touch`). `discover` sends it with the same params (`crates/playwright-cli/src/agent_discover.rs` `run`).
- `Target.createTarget` with `{url: "about:blank", background}`. Must return `targetId`. Any other `url` is sent as `Page.navigate` on the guard session once the guard is up (`crates/cdp-client/src/cdp.rs` `create_target`).
- `Target.attachToTarget` with `{targetId, flatten: true}`. Must return `sessionId` (`crates/cdp-client/src/cdp.rs` `attach`).
- `Target.detachFromTarget` with `{sessionId}` (`crates/cdp-client/src/cdp.rs` `detach`).
- `Target.closeTarget` with `{targetId}` (`crates/playwright-cli/src/browser.rs` `close_target`).
- `Target.activateTarget` with `{targetId}`. `open` ignores an error, `tab-select` reports it (`crates/playwright-cli/src/browser.rs` `activate`).
- `Browser.getVersion` with `{}`. Optional, `webmcp-call` only. Reads `product` as `Chrome/NNN` or `HeadlessChrome/NNN` (`crates/playwright-cli/src/webmcp.rs` `chrome_major_version`).
- `Slicc.action` with `{phase, kind, ...}`. An optional notification for the slicc panel that is never awaited, so a backend may ignore it or answer with an error (`crates/playwright-cli/src/action.rs` `begin`).

The JS `CDPClient` sends only `Target.getTargets` (`src/page.js` `listTargets`) and `Target.attachToTarget` with `flatten: true` (`src/cdp-client.js` `attach`) without a session.

### Browser-level events

A backend may omit any of these.

- `Target.attachedToTarget` with `sessionId` and `targetInfo.targetId`. Registers the guard session if it comes before the attach reply (`crates/cdp-client/src/cdp.rs` `adopt_guard_attach`).
- `Target.targetCreated` with `targetInfo` (`crates/cdp-client/src/cdp.rs` `watch`).
- `Target.targetInfoChanged` with `targetInfo`. A protected `url` marks the tab protected (`crates/cdp-client/src/cdp.rs` `watch`).
- `Target.targetDestroyed` with `targetId` (`crates/cdp-client/src/cdp.rs` `watch`).
- `Target.detachedFromTarget` with `sessionId` (`crates/cdp-client/src/cdp.rs` `watch`).

### Guard

- Every attach uses `flatten: true`. A non-flat `Target.attachToTarget` or `Target.setAutoAttach` is refused before it is sent (`crates/cdp-client/src/cdp.rs` `admit`), and so is `autoAttach: true` (`crates/cdp-client/src/cdp.rs` `admit`).
- Before a tab is used, the client attaches a guard session of its own. It is registered from the `Target.attachToTarget` reply (`crates/cdp-client/src/cdp.rs` `touch`) or from the `Target.attachedToTarget` event (`crates/cdp-client/src/cdp.rs` `adopt_guard_attach`), whichever comes first.
- `Target.setDiscoverTargets`, the attach and the setup steps share one `GUARD_SETUP` budget of 30 s (`crates/cdp-client/src/cdp.rs` `GUARD_SETUP`).
- The guard's `Fetch.enable` is `{patterns: [{urlPattern: "*", resourceType: "Document", requestStage: "Request"}]}` and decides per paused request (`crates/cdp-client/src/cdp.rs` `touch`). A backend must accept it. A backend's shield may refuse any interception pattern that names a protected origin, and clients never send one: `route` checks each `pattern` and refuses such a route before `Fetch.enable` is sent (`crates/cdp-client/src/fetch.rs` `enable`). Sending one broke bios and the extension on 2026-10-17.
- Then `Network.enable` and `Network.setBypassServiceWorker` with `{bypass: true}`, which must succeed (`crates/cdp-client/src/cdp.rs` `touch`).
- `Page.enable` and `Runtime.enable` are optional. Each gets at most `OPTIONAL_STEP`, 10 s of what is left (`crates/cdp-client/src/cdp.rs` `OPTIONAL_STEP`). An error reply, `<method>: <message>`, is ignored (`crates/cdp-client/src/cdp.rs` `touch`). A timeout prints `slicc: guard: <method> timed out; continuing` (`crates/cdp-client/src/cdp.rs` `touch`).
- Any other failure detaches the guard and marks the tab unguarded, so every later command on it is refused (`abandon`, `crates/cdp-client/src/cdp.rs` `touch`).

### Refused before sending

The client refuses these before sending, so a backend needn't implement them (`crates/cdp-client/src/shield.rs` `browser_wide`).

- `Target.exposeDevToolsProtocol`.
- `Target.sendMessageToTarget`. Attach with `flatten: true` instead.
- `Target.attachToBrowserTarget`.
- `Tracing.start`.
- `Extensions.loadUnpacked`.
- `Network.clearBrowserCookies`.
- `Storage.clearCookies` unless its `browserContextId` came from this client's `Target.createBrowserContext`.
- `Browser.resetPermissions` on the same terms.
- `Browser.setPermission` with no `origin`, on the same terms.
- `Browser.grantPermissions` with no `origin`, on the same terms.
- `Network.setRequestInterception`.
- `Fetch.*` with no `sessionId`.

### Session-level domains

Everything else goes over a flat session, with a top-level `sessionId`.

- `Page`
- `Runtime`
- `DOM`
- `Input`
- `Network`
- `Fetch`
- `Accessibility`

`slicc-cdp-client` also lets a caller `IO.read` a stream that a session reply handed out (`crates/cdp-client/src/cdp.rs` `admit`).

## Crates

Each crate's crates.io README is its section below. `scripts/crate-readmes.mjs` writes it to `crates/<dir>/README.md` before `cargo publish`; the generated files are not in git.

### slicc-cdp-client

Shared Chrome DevTools Protocol connection for the `playwright-cli` and `curlwright` binaries.

The crates.io name is `slicc-cdp-client`. The Rust library name stays `cdp_client`:

```
cdp-client = { package = "slicc-cdp-client", version = "1" }
```

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

#### Fetch interception

`cdp_client::fetch` pauses requests on one target session. `Interception::attach(&mut cdp, target_id, &patterns)` checks the target, attaches, and sends `Fetch.enable`; dropping it, or `finish()`, sends `Fetch.disable` and detaches. `Interception::on_session` does the same on a session you already hold and leaves it attached. `cdp::detach` also forgets that session's paused requests.

- `Pattern::request(glob)` and `Pattern::response(glob)` are CDP `urlPattern`s (`*`, `?`, `\` to escape) at the Request or Response stage, with an optional `resource_type`.
- `next(timeout)` returns a `Paused`: `method`, `url`, `url_fragment`, `full_url()`, `headers`, `post_data`, and at the response stage `status`, `response_headers` and `location()`, which resolves `Location` against the request URL and keeps its fragment.
- `decide(&paused, &Decision)` sends `Continue(Overrides)` (`continueRequest` with URL, method, body or headers), `ContinueResponse(ResponseOverrides)`, `Fulfill(Fulfill)` (`fulfillRequest`: status, headers, body; `Fulfill::signed_in()` is a short "Signed in, you can close this tab" page) or `Fail(ErrorReason)` (`failRequest`). `response_body(&paused)` is `Fetch.getResponseBody`, decoded.
- `navigation(guard, event)` reads a `Page.frameNavigated` event as `url` plus `urlFragment`, for a redirect made by script.
- `Cdp::set_decider(session, Some(Box<dyn FnMut(&Paused) -> Decision>))` answers pauses inline, even while another call waits for its reply, so a synchronous `Page.navigate` doesn't deadlock. A decision CDP rejects is retried as a plain continue and kept in `take_fetch_errors()`.

#### Seven's origin is off limits

`Cdp` filters every command and event the way slicc-cli's `cdp` relay does (slicc-cli README, "Seven's origin is off limits"). What is protected is `protectedOrigins` from the `slicc-browsers` crate: every `sliccy.ai` host, `vault.sliccy.ai` by name, `blob:`, `filesystem:` and `view-source:` URLs by their inner URL, `devtools:`, and the extra origins from `Cdp::protect_origins(&[...])`. `protected::Protected` parses URLs with a WHATWG parser and passes every vector in `protected.json`. Refusals are `slicc: refused: target is on seven's origin`, or a reason of their own after `slicc: refused: `. When the connection itself closes, the close frame (`websocket closed <code> <reason>`) is reported, never a refusal.

- **Targets.** A target is protected when its URL is, or when it is on `about:blank` (or another inheriting URL) and was protected before or has a protected or unknown opener. A page session also becomes protected when its main frame commits a protected URL or `securityOrigin` (`Page.frameNavigated`), when its default context's origin is protected (`Runtime.executionContextCreated`), or when the target moves there (`Target.targetInfoChanged`). Such a session is detached, its events and results are dropped, and every later command on it is refused.
- **Refused:** `Target.createTarget`, `Page.navigate` and anything else whose `url` is protected; `Page.navigateToHistoryEntry` to a protected entry; `attachToTarget`, `activateTarget`, `closeTarget`, `Browser.getWindowForTarget` and any command whose `targetId` or `frameId` is a protected target; every command on a protected session. `Target.getTargetInfo` and `Target.detachFromTarget` stay allowed.
- **Origins, storage and cookies.** A command whose `origin`, `securityOrigin`, `topLevelSite`, `scopeURL` (and the rest of slicc-cli's list), `storageKey` or `cacheId` names a protected origin is refused. Free-form values (`expression`, `arguments`, `value`, `headers`, `postData`) aren't searched. Cookie readers answer without protected cookies, and cookie writers for a protected domain, URL or partition site are refused with `slicc: refused: cookies on seven's origin`. So are `Network.clearBrowserCookies` and `Storage.clearCookies`, except in a browser context this `Cdp` created.
- **Browser-wide:** `Target.exposeDevToolsProtocol`, `Target.sendMessageToTarget`, `Target.attachToBrowserTarget`, `Tracing.start`, `Extensions.loadUnpacked`, `Fetch.*` without a session, permissions without an origin, and `Browser.resetPermissions` outside an own context. `Target.setAutoAttach` with `autoAttach: true` is refused: slicc-cdp does not support auto-attach.
- **Requests a protected document makes.** When a touched target commits a protected origin all the same (back/forward cache, a protected `blob:`), seen by its guard session's `Page.frameNavigated` or `Runtime.executionContextCreated`, every page session on it gets `Fetch.disable` and is detached at once, and its pending pauses are continued unchanged. A paused request whose frame is a protected document, whose target is protected, or whose `Referer` or `Origin` is protected is continued unchanged and never reaches `next` or a decider, even when its URL isn't protected. Its `Network` events (by `documentURL`, `initiator` or `frameId`) are dropped.
- **Requests.** A protected request's `Network.*` and `Fetch.*` events are dropped, `ExtraInfo` events are held until their request is known, and seven's cookies leave `associatedCookies`. Its body getters (`Network.getResponseBody`, `getRequestPostData`, `Fetch.getResponseBody`, `replayXHR` and the rest) are refused, and so is one for a request this `Cdp` never saw (`slicc: refused: slicc hasn't seen this request, so it could be seven's`). `IO.read` works only on a stream this `Cdp` handed out.
- **Navigation by other means.** A target this `Cdp` attaches to or creates is touched. On it, `Cdp` attaches a session of its own, enables `Fetch` for document requests at the request stage plus `Network.setBypassServiceWorker`, and fails each document request into a protected origin with `BlockedByClient`. A script's `location`, a link, a meta refresh or a server redirect into the vault then shows Chrome's error page, and the target is protected from then on. `Target.createTarget` opens `about:blank`, touches it, and then navigates. The guard setup (`Target.setDiscoverTargets` the first time, the attach, `Fetch.enable`, `Network.enable` and `Network.setBypassServiceWorker`) shares one 30 s budget and fails closed when it runs out, refused as `…out of this target: <step> timed out`. `Page.enable` and `Runtime.enable` on that session are optional. When one of them times out, the setup goes on, `Cdp::unguarded_optional(target)` lists it, and `Cdp::take_notices()` returns `slicc: guard: <step> timed out; continuing`, which `playwright-cli` and `curlwright` print on stderr. A paused document request to a protected origin on your own `Fetch` is failed the same way, and other protected requests are continued unchanged; neither reaches `next` or a decider.
- **Listing.** `Target.getTargets`, `getTargetInfo`, `targetCreated` and `targetInfoChanged` still show protected targets, but their `url` and `title` are only the origin. `Cdp::is_protected_target(id)` answers from what this `Cdp` has seen (listings, commits, refused sessions). `Cdp::target_protected(id, false)` asks the browser now, a tab that landed on the vault included, and records the answer, so both agree afterwards. `playwright-cli` and `curlwright` leave them out entirely.
- **Limits.** As in slicc-cli: a document that arrives without a request (back/forward cache, a protected `blob:`) is caught only after it commits.

`fetch::enable` refuses a pattern whose host part names a protected host (parsed, so `notsliccy.ai` and `?next=https://seven.sliccy.ai/` pass). A decision can't redirect to a protected URL by `url`, `Location` or `Refresh`.

```rust
use cdp_client::fetch::{Decision, Fulfill, Interception, Pattern};
let mut seen = Interception::attach(&mut cdp, &target_id, &[Pattern::response("https://idp.example/authorize*")])?;
let session = seen.session_id().to_string();
seen.notify("Page.navigate", serde_json::json!({ "url": authorize_url }), Some(&session));
if let Some(paused) = seen.next(std::time::Duration::from_secs(120))? {
    let redirect = paused.location();
    seen.decide(&paused, &Decision::Fulfill(Fulfill::signed_in()))?;
}
```

Host builds use `native-tls` for `wss://` and for `https://` discovery, on macOS, Linux, and Windows. Rust 1.83 or newer. The `wasm32-wasip1` build has no TLS. This crate does not launch Chrome. Callers pass a DevTools WebSocket URL or an `http://` discovery URL.

`cargo install slicc-cdp` installs the `playwright-cli` command. `cargo install curlwright` installs `curlwright`. `@ai-ecoverse/slicc-cdp` on npm ships their WASI builds for SLICC.

### slicc-cdp

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

```
cargo install slicc-cdp
```

That installs the `playwright-cli` command. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab.

Host builds run on macOS, Linux, and Windows. Rust 1.88 or newer. Start Chrome with `--remote-debugging-port` and a non-default `--user-data-dir`, then pass that endpoint. Chrome 136 and later ignore the debugging port on the default profile. `--headless` is optional: leave it off to open a window.

```
google-chrome --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
playwright-cli --cdp http://127.0.0.1:9333 open https://example.com
playwright-cli --cdp http://127.0.0.1:9333 snapshot
playwright-cli --cdp http://127.0.0.1:9333 tab-list
playwright-cli list
```

`list` prints `connected`, `debug ports`, and `launchable`. An empty section is the line `none`. It does not launch a browser and does not open a websocket. On the host it probes `127.0.0.1:9222-9229`. `list --json` prints one JSON document and no text sections.

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

`--cdp` wins, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort, else a loopback endpoint this tool saved under the user config dir. A `.playwright-cli/session.json` in the working directory is ignored. In the kernel, the wasm builds keep it at `<state>/slicc/playwright-cli/sessions/<hash>.json`, where `<state>` is `$XDG_STATE_HOME`, else `$XDG_CONFIG_HOME`, else `$HOME/.config`, and the hash is of `$PWD`. When that is not writable, `playwright-cli` prints `playwright-cli: running without saved state (<reason>)` on stderr and the command still succeeds, except `route` and `unroute`, which fail because they only save state. Host `list` GETs `/json/version` and `/json/list` on `127.0.0.1:9222-9229`, and host `open` GETs `/json/version` there when that port is absent or unreachable. `open` attaches to the first of those ports that answers, with or without a seven tab, and an unreachable seven-profiles port falls through to it. A successful command records a loopback endpoint and its provenance. A probe-derived session is reused while that browser answers, and every command shows all of its tabs except those on protected origins. Saved state never dials a non-loopback host. Host `open` and `tab-new` treat an unreachable saved endpoint as absent. Other commands report that connection error. When nothing answers, launch binds the first free port from 9222 to 9229, or lets Chrome pick one. `SLICC_CDP_PORTS=<from>-<to>`, for tests, replaces that range and stops every dial to loopback 9222-9229. The wasm build still uses port 9222. A host build dials `wss://`. The `wasm32-wasip1` binaries in the npm package `@ai-ecoverse/slicc-cdp` have no TLS.

Implemented commands: `open`, `close`, `goto`, `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `eval`, `tab-list`, `tab-new`, `tab-select`, `tab-close`, `console`, `requests`, `request`, `request-headers`, `request-body`, `response-headers`, `response-body`, `route`, `route-list`, `unroute`, `list`.

```
playwright-cli route "**/api/users" --body='[{"id":1}]' --content-type=application/json
playwright-cli route "**/*.png" --fulfill-file ./pixel.png
playwright-cli route-list
playwright-cli unroute "**/api/users"
```

Routes are saved with the session and apply while a later command is attached to a tab. Requests to seven's origins and the vault are never routed.

`playwright-cli --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.

### curlwright

curl-style requests inside an already-open browser tab. The request is a page-context `fetch()`, so that tab's cookies, origin, and service worker apply.

```
cargo install curlwright
```

Host builds run on macOS, Linux, and Windows. Rust 1.83 or newer. Start Chrome with `--remote-debugging-port` and a non-default `--user-data-dir`. Chrome 136 and later ignore the debugging port on the default profile. `--headless` is optional: leave it off to open a window.

```
google-chrome --headless --remote-debugging-port=9333 --user-data-dir=/tmp/chrome-cdp
curlwright --cdp http://127.0.0.1:9333 https://example.com/
curlwright --cdp http://127.0.0.1:9333 -I https://example.com/
```

On macOS the binary is `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`.

Connect order: `--cdp`, else `SLICC_CDP_URL`, else a live `seven-profiles` DevToolsActivePort whose directory name is a browser id. Dotfiles, including `.migrated-from-profiles`, are not profiles. On the host, if none of those exist, curlwright uses the first of `127.0.0.1:9222-9229` whose `/json/version` answers, as `playwright-cli` does, and exits when none does. Ports 9222-9229 inside those files are dialed like any other. The wasm build still uses port 9222. Without `--tab`, curlwright uses the `current` tab this tool saved for the working directory under the user config dir when that tab is still open, otherwise the single open tab on the URL's origin. It ignores a `.playwright-cli/session.json` in the working directory and does not read the saved endpoint. Any other case exits 2 and lists `--tab` choices.

The npm package `@ai-ecoverse/slicc-cdp` ships the `wasm32-wasip1` build for SLICC. That build has no TLS.

`curlwright --version` prints the crates.io version. A build from this repository prints the `package.json` version instead.

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

`playwright-cli list` prints `connected`, `debug ports`, and `launchable`. An empty section is the line `none`. `connected` includes an explicit `--cdp` or `SLICC_CDP_URL` browser and each live `DevToolsActivePort`, as `browser - <websocket url>`. It does not launch a browser and does not open a websocket. On the host it also probes catalog ports `127.0.0.1:9222-9229` with GET `/json/version` and GET `/json/list`, reports `source` `probe`. In `--json`, an endpoint's `tabs` lists every page except those on protected origins, and `sevenTabs` keeps only the seven ones. `playwright-cli list --json` prints one JSON document and no text sections. A child of `seven-profiles` is a `profile`. A default user-data directory is a `browser`. `https://<label>.sliccy.ai:443/` counts as seven. Any other explicit port does not.

`--cdp` still wins over `SLICC_CDP_URL`, which wins over a live `seven-profiles` DevToolsActivePort, which wins over a loopback session this tool saved under the user config dir, for `playwright-cli`. A `.playwright-cli/session.json` in the working directory is ignored. An unreachable saved endpoint does not stop `open` or `tab-new`. `curlwright` does not read that session endpoint. Saved state never dials a non-loopback host. `open` then reuses a catalog probe port whatever its tabs, including when the seven-profiles port is unreachable. Launch binds the first free port of `127.0.0.1:9222-9229`. `curlwright` probes those ports too when nothing else names a browser. Node `connect()` requires `options.url` or `SLICC_CDP_URL` and does not dial port 9222 by itself. The wasm build still uses port 9222. A crate installed from crates.io prints its crate version. In this repository, and in the npm wasm builds, `--version` prints the `package.json` version. The release workflow writes that version into the crate manifests before `cargo publish`, and only after semantic-release has published the npm package. Git keeps the crate versions at `0.0.0`.

## Testing

A developer's own Chrome listens on `127.0.0.1:9222-9229`, so no test touches those ports. `npm run test:integration` and `npm run test:unit` set `SLICC_CDP_PORTS=9300-9309` unless it is already set, and so do `.cargo/config.toml` for `cargo test` and the CI workflows. Node test runs preload `test/integration/ports.mjs`, which sets the same default and makes any connect to loopback 9222-9229 from the test process throw. It matches the host as written (`localhost`, `*.localhost`, `127.x`, `0.0.0.0`, `::1`, `::`), not the address a name resolves to, so a hosts-file name or a service such as `lvh.me` that resolves to loopback gets past it; the CLIs check the resolved address too. `SLICC_CDP_PORTS` is for tests. While it is set, `playwright-cli` and `curlwright` probe and launch on that range, and loopback 9222-9229 are unreachable: they panic and exit 101 before dialing, even for an explicit `--cdp` or `SLICC_CDP_URL`. A test that needs other ports sets its own range. Run local builds the same way, for example `SLICC_CDP_PORTS=9300-9309 target/debug/playwright-cli list`. `cargo run` in this repository already gets the range from `.cargo/config.toml`, so to reach a browser on 9222-9229 run the built binary with `SLICC_CDP_PORTS` unset.

A workflow step that gates a merge, a release or a cert runs its script through `node scripts/gate.mjs <name> -- <command>`. The script prints a verdict line `<name>: PASS <detail>`, and the gate fails on a non-zero exit or when that line is missing, so an early `exit 0`, a skipped script or empty output fails the step. Evidence steps pass `--accept PASS,FAIL,INCOMPLETE`, because the record step judges their status. For `node --test` the gate takes `--tap`: it needs `# fail 0` and at least one test that ran, not counting a file with no tests or a skipped test. `scripts/test-integration.mjs` prints `integration: PASS <n> tests` the same way. `test/integration/gate.test.mjs` checks that every such workflow step is gated.

## Release supply-chain verification

Release publishes its complete signed assets without an in-release behaviour-certification gate. The code-only `verify-release.yml` runs **after** `publish-release`, or independently from main with `gh workflow run verify-release.yml --ref main -f version=1.26.2`. It hashes the exact npm tarball, verifies registry integrity and signed SLSA v1 (GitHub-hosted `release.yml` on main), requires `gitCommit = gitHead = tag` on main, verifies all four macOS signatures and notarisation, and requires the full checksum inventory including the installer and notices. Missing evidence is red `INCOMPLETE`; a mismatch is a red supply-chain `FAIL`, with integrity failure taking precedence.

A complete byte verification appends `verified @ai-ecoverse/slicc-cdp@<v> sha256=<hex> (...)` between `cert-verify` markers in the published release notes. The `certified` prefix is reserved for the later required PR certification lane and its bound PR records. It does not assert PR behaviour certification, change an npm dist-tag, move a tag or execute a model. **Interim:** until the required PR `cert-review` check is live, releases receive supply-chain verification only, not behavioural certification in CI. Identical notes are unchanged; an existing certificate for different bytes raises an alert rather than being overwritten. Historical releases with incomplete checksum inventories are not silently waived. All potentially developer-hosted cert jobs use `SLICC_CDP_PORTS=9300-9309`.

## Historical behaviour-certification replay

`cert-review.yml` remains available for historical behaviour-certification replay, but is no longer invoked by the release path. Replay an exact version from main with `gh workflow run cert-review.yml --ref main -f version=1.26.2`; `-f dry_run=true` retains the evidence without writing a release or commit status. No job changes the npm `certified` tag. Every reusable job has an explicit status-aware condition, so a skipped ancestor in the release caller does not silently skip evidence. `cert-call-proof.yml` makes a main-only dry-run call with the same release needs and a deliberately skipped asset ancestor; its verifier requires all seven evidence jobs and the resulting record. Shared browser fixtures use the current harness API independently of the older implementation under test, and per-worker summaries retain results defined in imported test helpers.

Hosted planning chooses the newest earlier successful cert-review record, skipping failed and unreviewed releases, with `scripts/cert/base.json` as the lane-approved fallback. Rust is pinned by the tag's `rust-toolchain.toml`, a recorded historical release pin, or the compiler version in that exact release's CI log. The same resolved version, commit, baseline, harness, pins and workflow attempt identify every result document.

The bench pool rebuilds both wasm files byte-for-byte against npm, runs the pinned shared guard runner through the Rust client's public bridge with controls and liveness, runs the same guard suite on the previous certified client's source, and runs the tagged integration suites on published JavaScript, native assets and wasm in the tagged kernel pin. Source-built Rust drivers used by existing integration wrappers are named separately from published command bytes. Guard owner additions remain in the suite. Non-executable shared-set descriptions and explicit upstream non-applicable rows remain visible and are not counted as executed passes.

Fix commit bodies declare `Proves: <test-id>, ...`; `.cert/proofs.json` provides committed PR-to-id declarations for historical fixes. `scripts/cert/proof-tests.json` binds integration ids to exact public test files/titles; `vector:<id>/<target>/<variant>` selects a shared guard row. Planning derives and deduplicates the named set from the git delta and the PR that first introduced each fix. A fix without ids is a known test gap listed on the line, not a fabricated proof and not missing evidence. Named tests must actually pass on the candidate and fail on the previous certified release. Missing named results or an unrun NEGATIVE are `INCOMPLETE`; a named test that passes on the baseline reads `proof does not discriminate`. Any additional NEGATIVE failure is flagged on the line, never silently credited to a fix. Committed external Rust integration drivers run against both public library versions without copying a private test module or replacing the subject implementation.

Hosted jobs verify the full release checksums and all four macOS signatures, identifiers, Developer ID team and notarisation. Only replay mode for commits before #224 may report `legacy: checksum set unavailable`; the asset bytes and GitHub digests still verify, and any present checksum mismatch still fails. Release mode and every commit from #224 onward require a complete checksum set without exception. A separate fresh hosted record job independently downloads the exact npm version with `npm pack`, hashes it in code, verifies registry integrity and signed SLSA provenance, requires gitHead = provenance commit = tag on main, and compares the four crates.io versions and source manifests to the tag. It validates all result documents and job outcomes before emitting `record.json`, the job summary, a cert-review status and the release's Certification evidence section.

A complete record emits `certified-candidate @ai-ecoverse/slicc-cdp@<v> sha256=<hex> gitHead=<sha> guard=GUARD-PASS(slicc-cdp@<sha>; vectors @ai-ecoverse/slicc-browsers@<v>)`. Missing evidence emits `cert-ci: INCOMPLETE`; a coded integrity or test failure emits `cert-ci: FAIL`. The cert lane reviews exceptions and retains the authoritative dist-tag decision during this phase. PR behaviour certification is being built separately; release remains simple with code-only post-publish byte verification. The earlier publish-after-precert proposal was cancelled and never implemented.

## PR certification tooling

The `scripts/cert/pr-*` tools run in `.github/workflows/cert-review-pr.yml`, initially non-required. Every control checkout uses the main base SHA; candidate code runs only in hosted, read-only, secrets-free jobs with credentials disabled. Signing never executes input binaries. The Bedrock credential is confined to the data-only model step, which has no tools; code validates its head-bound structured verdict and execution receipt. Test-only changes and all package manifest changes, including scripts-only `postinstall` changes, take the full review. The merge-group pass-through is exact-head, ancestry-bound and code-only, consuming immutable trusted PR records and a fresh shared self-guard receipt. The always-running aggregate records missing or skipped ancestors as `INCOMPLETE`, never PASS. Release verification remains `verified` until the separately approved required-check rollout. Signed PR binaries and the collected signed assets expire after one day; digest and verification records remain for 90 days. The selected Bedrock model id is recorded in `review.json`, with the pinned default `global.anthropic.claude-opus-5-5`. Source-object fetches are credential-free for this public repository; a future trusted private-fork route would need an explicit read token, not candidate access to signing or review credentials.

The model reviewer uses the kernel lane's pinned `claude-code-action/base-action`, but keeps `--tools ""` and does not request `--json-schema`: that combination returned no `structured_output` in the live bootstrap proof. Unlike the kernel's filesystem-writing reviewer, this model cannot read or write files or execute commands. Main-owned `pr-review.mjs` parses only the execution receipt's successful final result: one complete JSON object or one fenced `json` block with nothing else. It validates the exact schema and head/commit binding, hashes the execution receipt, and requires exactly one `system/init` receipt with empty `tools` and `mcp_servers` arrays. Any advertised tool or MCP server is `INCOMPLETE` with `review model had tools`; absent or malformed capability receipts are also `INCOMPLETE`. The invocation flag alone is not proof of tool isolation; malformed output, free-text keywords and missing execution are never a substitute verdict.

Mandatory bios evidence uses `ai-ecoverse/slicc-bios/.github/workflows/canary.yml` pinned to merged commit `4025d6aabc2c06add216183064d81914a171cd64`, with `packages-update-cdp` and `guard` shards, repeat three, against both unpublished base and candidate packs. Main prepares distinct `base` and `candidate` labels and verifies both pack hashes without executing them. The consumer checks the producer's real `result` output: pinned bios ref, caller run, repeat, labelled artifact URL, exact override version/hash and artifact-source status binding, counts and completion arrays. The producer summary has no structured candidate `source` field; its exact artifact/run/hash status and override hash are checked, not an invented field or `ok` alone. Candidate regressions/failures are `FAIL`; missing calls, unfinished shards, skips, flaky results or malformed output are `INCOMPLETE`. Both-failing rows remain visible as flags rather than being mistaken for new regressions. The final record independently revalidates both result documents against its own byte-checked packs. The pin alone never makes the slot pass: the rollout workflow still needs two literal pinned canary calls and complete results. Required checks and rulesets need separate rollout approval. The shared guard's accepted residual is: self-guard catches accidental cert-path edits; not enforced against a deliberate rewrite of the workflow.
