# slicc-cdp

The CLIs your agent already knows, where you need them and to the point.

- `playwright-cli`: a playwright-cli-compatible command line. Not affiliated with Microsoft Playwright.
- `curlwright`: curl, run as `fetch()` inside an open tab, with its cookies.
- `@ai-ecoverse/slicc-cdp`: the Node `CDPClient` and the kernel builds of both commands.

Compared with Microsoft's `@playwright/cli`, `playwright-cli` is one 4.6 MB binary instead of 18 MB of npm packages plus Node. It starts in 9 ms instead of 236 ms, and runs each browser command 3 to 20 times faster.

## Install

```sh
curl -fsSL https://get.sliccy.ai/playwright-cli | sh
curl -fsSL https://get.sliccy.ai/curlwright | sh
```

Or with Cargo:

```sh
cargo install slicc-cdp     # playwright-cli
cargo install curlwright
```

Or with npm, for the Node library:

```sh
npm install @ai-ecoverse/slicc-cdp
```

Windows binaries are on the [releases page](https://github.com/ai-ecoverse/slicc-cdp/releases/latest).

## Use

```sh
playwright-cli open https://example.com
playwright-cli snapshot
playwright-cli click e3
curlwright -i https://example.com/
```

`open` attaches to a running Chrome or launches one. To pick a browser, pass `--cdp <port>` (for example `--cdp 9333`, which means `localhost:9333`) or `--cdp http://127.0.0.1:<port>`, or set `SLICC_CDP_URL`. Run `playwright-cli --help` for all commands.

`--cdp` and `SLICC_CDP_URL` take a port, `host`, `host:port` (port 9222 when it is left out), an `http://` or `https://` URL, or a `ws://` or `wss://` URL. A `ws://` or `wss://` URL with a path is dialled as it is; every other form reads `/json/version` first. The client tries every address the host resolves to, and `localhost`, `127.0.0.1`, `::1` (`[::1]`) and `0.0.0.0` all mean this machine: both `::1` and `127.0.0.1` are tried. Every address but the last gets two seconds to connect.

Seven's origins, the vault, browser pages, extensions and local files are always off limits.

`snapshot` prints the same tree as `@playwright/cli`: the same role names and refs, with `[active]`, `[cursor=pointer]` and `/url:`. It runs Playwright's own aria snapshot code, taken from `playwright-core` 1.64 (Apache-2.0, Copyright Microsoft Corporation), in an isolated world, in `crates/playwright-cli/src/aria-snapshot.js`. A `/url:` is redacted like every other URL: a protected origin shows only its origin, and query values, fragments and credentials show as `…` unless `--full-urls` is given. A cross-origin iframe that runs in its own process is captured through its own flat session, with `f<N>e<M>` refs that `click`, `fill` and the other ref commands drive in that session, and `frames` lists it. A protected, unverified or unknown child frame is never attached or read: it shows as a bare `iframe`, and `frames` shows a protected one by its origin alone.

The cookie commands, `cookie-list`, `cookie-get`, `cookie-set`, `cookie-delete` and `cookie-clear`, work like Microsoft's and print ordinary values as they are. Seven's and the vault's cookies, including cookies scoped to a parent domain of a protected host, are never listed, read, set or deleted. Domain arguments are canonicalized before the guard checks them; protected scopes and malformed domains are refused. `cookie-clear` leaves protected cookies, and requests to protected origins are not recorded. Headers containing protected-scope cookies and console lines containing their values are hidden, even on ordinary pages.

Saved sessions never contain cookie values or value hashes. They retain only cookie names and scopes (domain and path), with Cookie and Set-Cookie header values redacted. `request-headers`, `response-headers` and `request` look up those scopes in the live guarded cookie jar and label any values they show as current jar values, not necessarily what was sent at that time. Removed and protected cookies stay `[redacted]`. A header with no retained scopes can show current cookies applicable to its URL only when that URL cannot share a protected parent-domain cookie. Cookie-bearing console messages may be shown live but are not saved.

## WebMCP

`webmcp-list` and `webmcp-call` prefer native WebMCP, including declarative tools when the browser supports them. For an app that registers JavaScript tools without native WebMCP, keep a dedicated holder running:

```sh
playwright-cli --cdp 9333 webmcp-polyfill --tab <targetId> --reload
```

The holder is a foreground process: keep stdin open, and close stdin to stop it. Its first stdout line is JSON with `mode`, `tab`, `late`, `reloaded` and measured `pauseMicros`. `--reload` is opt-in: it reloads only the selected tab so registration during page load sees the API. Without it, the shim installs into the already-loaded document and `late: true` warns that load-time registrations may have been missed. An existing native or page-provided API is not replaced. The shim's document and navigator aliases remain replaceable; the holder retains its registered tools through a private, random lexical bridge, not a page-callable binding.

From another terminal, using the same CDP endpoint and saved session:

```sh
playwright-cli --cdp 9333 webmcp-list --tab <targetId> --json
playwright-cli --cdp 9333 webmcp-call get_inbox_state '{}' --tab <targetId>
playwright-cli --cdp 9333 webmcp-call send_reply '{}' --tab <targetId> --allow-consequential
```

Same-origin frames are included. A cross-origin provider additionally requires repeatable `--from-origin <http(s)-origin>`, explicit `exposedTo` permission for the caller's origin, the `tools` Permissions Policy and iframe delegation along the entire ancestor chain. `--json` includes each provider's `frameId`; use `webmcp-call ... --frame <frameId>` to resolve duplicate names. Unknown policy is denied: on browsers without native `tools` policy introspection, the holder must observe the document response, normally through `--reload`. Tool lists and call results are capped at 65,536 characters per page response. The shim supports JavaScript tool registration, not declarative HTML tools.

Only the hash-pinned shim can use the dedicated installer. Its inert pre-execution gate pauses each new document, verifies fresh guard-owned frame, loader and context evidence, then permits the shim body. Protected or unverified documents, including child frames, never pass the gate; errors, missing pauses or changed document evidence remove the installer and resume without the shim. No holder means no debugger gate or extra navigation pause. Stopping the holder removes future injection; an already-installed ordinary document API lasts until that document is discarded. Browser-backed tests and navigation/pause measurements run on CI only.

## Develop

```sh
npm run lint
npm run test:unit
npm test
```

`npm test` builds the native and wasm CLIs and the guard bridge, installs the pinned Chrome for Testing (`config.chrome`) and runs `test/integration/*.test.mjs`, the Rust workspace tests included. The published guard set from `@ai-ecoverse/slicc-browsers` runs in four files, `guard-a.test.mjs` to `guard-d.test.mjs`, one per partition of its targets; `guard-union.test.mjs` checks the partitions add up to the whole set. Every row must pass, or skip with the reason its vector declares, and the `C2.redirect-*` and `C4.iframe-preattach` `.shared` rows must pass. Each partition writes its report to `artifacts/guard-<partition>.md`.

## CI and releases

`.github/workflows/ci.yml` calls [slicc-shared-web's pipeline](https://github.com/ai-ecoverse/slicc-shared-web#pipeline): lint, the integration tests split over four shards on the pool, and the Claude and Codex reviews on pull requests. The ruleset requires `ci / integration` and `ci / review`.

On `main`, semantic-release publishes `@ai-ecoverse/slicc-cdp` to npm with provenance from `ci.yml`, tags `v<version>` and opens a draft GitHub release. When it released, `ci.yml` then publishes the four crates to crates.io, builds the binaries for macOS, Linux and Windows (the macOS ones signed and notarised), and uploads them with `install.sh`, `LICENSE`, `THIRD-PARTY-NOTICES` and `SHA256SUMS` before it publishes the release as latest. `@ai-ecoverse/sliccy-browser` is published from the same workflow whenever its `package.json` names a version npm doesn't have yet.

## Backend contract

What the clients need from a CDP backend, checked against the code by `crates/cdp-client/src/contract.rs`.

### Browser-level methods

- `Target.getTargets` (`crates/playwright-cli/src/browser.rs` `list_pages`)
- `Target.getTargetInfo` (`crates/cdp-client/src/cdp.rs` `target_protected`)
- `Target.setDiscoverTargets` (`crates/playwright-cli/src/agent_discover.rs` `run`)
- `Target.createTarget` (`crates/cdp-client/src/cdp.rs` `create_target`)
- `Target.attachToTarget` (`crates/cdp-client/src/cdp.rs` `attach`)
- `Target.detachFromTarget` (`crates/cdp-client/src/cdp.rs` `detach`)
- `Target.closeTarget` (`crates/playwright-cli/src/browser.rs` `close_target`)
- `Target.activateTarget` (`crates/playwright-cli/src/browser.rs` `activate`)
- `Browser.getVersion` (`crates/playwright-cli/src/webmcp.rs` `chrome_major_version`)
- `Slicc.action` (`crates/playwright-cli/src/action.rs` `begin`)

### Browser-level events

- `Target.attachedToTarget` (`crates/cdp-client/src/cdp.rs` `adopt_guard_attach`)
- `Target.targetCreated` (`crates/cdp-client/src/cdp.rs` `watch`)
- `Target.targetInfoChanged` (`crates/cdp-client/src/cdp.rs` `watch`)
- `Target.targetDestroyed` (`crates/cdp-client/src/cdp.rs` `watch`)
- `Target.detachedFromTarget` (`crates/cdp-client/src/cdp.rs` `watch`)

### Refused before sending

- `Target.exposeDevToolsProtocol`
- `Target.sendMessageToTarget`
- `Target.attachToBrowserTarget`
- `Tracing.start`
- `Extensions.loadUnpacked`
- `Network.clearBrowserCookies`
- `Storage.clearCookies`
- `Browser.resetPermissions`
- `Browser.setPermission`
- `Browser.grantPermissions`
- `Network.setRequestInterception`
- `Fetch.*`

The guarded Rust client supports document-scoped operations, origin-scoped cookies and guarded interception. Persistent page capabilities that cannot be restricted to verified ordinary documents are refused before sending, including generic init scripts, bindings, page overrides and automatic debugger-waiting attachment. The dedicated WebMCP installer is the sole vetted-shim exception: it accepts a session, never client-provided source, and gates the pinned bytes on guard-owned document verification. The production method lifetime [inventory](crates/cdp-client/src/capability_inventory_test.rs) is checked against the CLI sources. The Node `CDPClient` is a raw CDP transport, not the guarded Rust client.

### Session-level domains

- `Page`
- `Runtime`
- `Debugger`
- `DOM`
- `Input`
- `Network`
- `Fetch`

## Crates

### slicc-cdp-client

The Chrome DevTools Protocol connection behind `playwright-cli` and `curlwright`. One browser WebSocket, flat sessions, and a guard that keeps seven's origins out of reach.

```
cdp-client = { package = "slicc-cdp-client", version = "1" }
```

#### Fetch interception

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

### slicc-cdp

Installs `playwright-cli`, a playwright-cli-compatible command line for SLICC. Not affiliated with Microsoft Playwright.

```
cargo install slicc-cdp
playwright-cli open https://example.com
playwright-cli snapshot
playwright-cli click e3
```

`open` attaches to a running Chrome or launches one. Pass `--cdp http://127.0.0.1:<port>` or set `SLICC_CDP_URL` to pick a browser.

Prebuilt binaries: `curl -fsSL https://get.sliccy.ai/playwright-cli | sh`. See [slicc-cdp](https://github.com/ai-ecoverse/slicc-cdp).

### curlwright

curl-style requests run as `fetch()` inside an already-open browser tab, so that tab's cookies and origin apply.

```
cargo install curlwright
curlwright -i https://example.com/
curlwright --cdp http://127.0.0.1:9333 -I https://example.com/
```

Without `--tab` it uses the tab `playwright-cli` last selected, else the one open tab on the URL's origin.

Prebuilt binaries: `curl -fsSL https://get.sliccy.ai/curlwright | sh`. See [slicc-cdp](https://github.com/ai-ecoverse/slicc-cdp).
