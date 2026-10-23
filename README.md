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

`playwright-cli upgrade`, or `playwright-cli update`, replaces a prebuilt `playwright-cli` with the latest release, in place. It fetches over https only, and the download must match the release's `SHA256SUMS` and report the release version. `--check` only reports whether a newer release exists. It only replaces a binary it can write; a development build needs `--force`, and a copy installed with Cargo is left to `cargo install slicc-cdp`. The wasm build can't upgrade itself.

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

`eval` calls a function expression (arrow, `function` or async function) and prints its awaited result, like Playwright's evaluate. Other expressions and multi-statement scripts still print their values without being called. `--frame` uses the selected frame's isolated world.

The cookie commands, `cookie-list`, `cookie-get`, `cookie-set`, `cookie-delete` and `cookie-clear`, work like Microsoft's and print ordinary values as they are. Seven's and the vault's cookies, including cookies scoped to a parent domain of a protected host, are never listed, read, set or deleted. Domain arguments are canonicalized before the guard checks them; protected scopes and malformed domains are refused. `cookie-clear` leaves protected cookies, and requests to protected origins are not recorded. Headers containing protected-scope cookies and console lines containing their values are hidden, even on ordinary pages.

Saved sessions never contain cookie values or value hashes. They retain only cookie names and scopes (domain and path), with Cookie and Set-Cookie header values redacted. `request-headers`, `response-headers` and `request` look up those scopes in the live guarded cookie jar and label any values they show as current jar values, not necessarily what was sent at that time. Removed and protected cookies stay `[redacted]`. A header with no retained scopes can show current cookies applicable to its URL only when that URL cannot share a protected parent-domain cookie. Cookie-bearing console messages may be shown live but are not saved.

The web storage commands, `localstorage-list`, `-get`, `-set`, `-delete` and `-clear` and the same five `sessionstorage-` commands, work like Microsoft's and print ordinary values as they are, with terminal control characters escaped. They run in a fresh isolated world of the tab's main frame through the guarded client, so a tab whose current document is protected, unverified, an error page or opaque is refused. Seven's and the vault's localStorage and sessionStorage are never read or written. `--tab` picks the tab; otherwise the current tab is used.

`state-save [filename]` writes Microsoft's storageState JSON: `cookies` in Playwright's cookie shape and `origins` with each origin's `localStorage`. Without a filename it writes `.playwright-cli/storage-state-<timestamp>.json`. **The file is a credential.** It holds live cookie values, including HttpOnly session cookies, and localStorage, where sites often keep login tokens, so anyone who reads it can sign in as you on those sites. On Unix it is created owner-only (0600). Keep it out of version control, chat transcripts and shared directories, and delete it when you're done. It never holds seven's or the vault's cookies (parent-domain and partitioned ones included) or their origins, and like Microsoft's it never holds sessionStorage. Deviations: the origins are those of the open ordinary tabs, read in place, not every origin the browser context has visited; a tab the guard can't verify is skipped. An absolute filename prints as given.

`state-load <filename>` checks the whole file before it changes anything. A protected, malformed or non-http(s) cookie scope or origin refuses the file, and so do IndexedDB, OPFS and credential entries, which are not restored. It then clears the ordinary cookies (protected ones stay) and sets the file's cookies. For each origin in the file it opens one temporary background tab, answers the tab's requests to those origins with an empty page through guarded interception, and, as Microsoft does, unregisters that origin's service workers, deletes its IndexedDB databases, clears its localStorage and writes the file's localStorage. The tab is then closed. Deviations: the HTTP cache is not cleared, because that would clear seven's, and origins that are not in the file keep their storage. Errors from these commands go to stderr as `Error: ...` and exit 1.

## WebMCP

`webmcp-list` and `webmcp-call` prefer native WebMCP, including declarative tools when the browser supports them. For an app that registers JavaScript tools without native WebMCP, keep a dedicated holder running:

```sh
playwright-cli --cdp 9333 webmcp-polyfill --tab <targetId> --reload
```

The holder is a foreground process: keep stdin open, and close stdin to stop it. Its first stdout line is JSON with `event: "ready"`, `loaderId`, `mode`, `tab`, `late`, `reloaded` and measured `pauseMicros`. After later navigations, a `document-ready` JSON event identifies the current `loaderId` once the top document has loaded, every frame document the holder is waiting on, including cross-origin iframes in their own processes, has passed or failed the gate, and the holder has persisted its verified metadata; consumers should await this event before listing load-time tools. Ordinary CLI saves preserve holder-owned metadata, and listing refreshes it after verifying the current document. `--reload` is opt-in: it reloads only the selected tab so registration during page load sees the API. Without it, the shim installs into the already-loaded document and `late: true` warns that load-time registrations may have been missed. An existing native or page-provided API is not replaced. The shim's document and navigator aliases remain replaceable; the holder retains its registered tools through a private, random lexical bridge, not a page-callable binding.

From another terminal, using the same CDP endpoint and saved session:

```sh
playwright-cli --cdp 9333 webmcp-list --tab <targetId> --json
playwright-cli --cdp 9333 webmcp-call get_inbox_state '{}' --tab <targetId>
playwright-cli --cdp 9333 webmcp-call send_reply '{}' --tab <targetId> --allow-consequential
```

Same-origin frames are included. A cross-origin provider additionally requires repeatable `--from-origin <http(s)-origin>`, explicit `exposedTo` permission for the caller's origin, the `tools` Permissions Policy and iframe delegation along the entire ancestor chain. `--json` includes each provider's `frameId`; use `webmcp-call ... --frame <frameId>` to resolve duplicate names. Unknown policy is denied for all iframes and for our injected shim: on browsers without native `tools` policy introspection, the holder must observe the document response, normally through `--reload`. For compatibility, an ordinary top-level page-defined API may expose its own tools when no policy was observed and the browser does not recognize `tools`; these tools carry `"policy": "unknown"` in `--json` and a “policy: unknown, top-level page-defined” label in human output. Verified tools carry `"policy": "allowed"`. Any observed `tools=()` denies tools, including in compatibility mode. Run `webmcp-polyfill --tab <targetId> --reload` to record verified response headers rather than rely on compatibility mode. Tool lists and call results are capped at 65,536 characters per page response. The shim supports JavaScript tool registration, not declarative HTML tools.

Only the hash-pinned shim can use the dedicated installer. Its inert pre-execution gate pauses each new document, verifies fresh guard-owned frame, loader and context evidence, then permits the shim body. Protected or unverified documents, including child frames, never pass the gate; errors, missing pauses or changed document evidence remove the installer and resume without the shim. No holder means no debugger gate or extra navigation pause. Stopping the holder removes future injection; an already-installed ordinary document API lasts until that document is discarded. Browser-backed tests and navigation/pause measurements run on CI only.

`webmcp-list` succeeds with an empty list (`--json`: `[]`) or “No WebMCP tools on this page” when WebMCP is available but has no registered tools. If `document.modelContext` is absent or null, both `webmcp-list` and `webmcp-call` print “WebMCP is unavailable on this page” to stderr, leave stdout empty and exit 2. Evaluation and connection failures exit 1.

`dialog-accept [prompt]` and `dialog-dismiss` are bounded live waits: start one before triggering an alert, confirm or prompt. It waits up to five seconds, verifies an observed ordinary main-frame dialog against the browser's current target and navigation history, then answers it. The dialog-only flat session never enables Network or Runtime or reads a renderer frame tree, so it can answer while a click is in flight. Protected, unknown, stale and subframe ownership are refused. A dialog already open before attachment is not answered; the error names the [foreground dialog holder follow-up #529](https://github.com/ai-ecoverse/slicc-cdp/issues/529).

`resize`, `keydown`, `keyup`, `mousemove`, `mousedown`, `mouseup`, `mousewheel` and `drop` take Microsoft's arguments, print its `### Ran Playwright code` lines and send Playwright's key definitions, buttons, coordinates and modifier masks. The deliberate differences:

- Every command is its own process, so the pointer position, held buttons and held keys are saved per tab in the session, and the next command continues from them as Playwright's `page.mouse` and `page.keyboard` do. Modifiers held with `keydown` apply to later key and mouse events. On a macOS browser `keydown` sends Playwright's editing commands; `ControlOrMeta` follows the browser's platform rather than the CLI's.
- Mouse commands first hit-test the whole pixels around the point with `DOM.getNodeForLocation` and refuse a point over an iframe, frame, object, embed or fenced frame, or inside a child frame. `keydown` and `keyup` refuse while focus is inside one of those or inside a closed shadow root. This keeps raw input out of protected and unverified frames.
- A native HTML drag that `mousedown` and `mousemove` start is not intercepted. `Input.setInterceptDrags` is a persistent capability the guard refuses, and an intercepted drag could not span separate commands. `drag` and `drop` cover element-to-element and external drops.
- `resize` sets the window with `Browser.setWindowBounds`, adding the measured difference between the window and the viewport, so the viewport becomes `<w>` by `<h>` and stays that size after the command exits. `Emulation.setDeviceMetricsOverride` is not used because Chrome clears it when the command's session detaches. Sizes must be whole numbers from 1 to 10000000. The guard admits the change only for the session's own window, with an ordinary current document and frame tree, and refuses it while any protected or unverifiable tab, frame or other window-capable target is open in the same browser profile, since a window resize reaches every tab in the window.
- `drop` dispatches synthetic `dragenter`, `dragover` and `drop` events in the page, as Microsoft's does, rather than `Input.dispatchDragEvent`, whose file list makes the browser read paths itself. The CLI reads only the regular files named with `--path`, resolved against the working directory, refuses directories and other non-regular files, and caps them at 25 MiB in total. The wasm build reads the kernel's files with the same code. The printed locator is `locator('aria-ref=<ref>')`, not Playwright's generated one, only snapshot refs are accepted as targets, and MIME types come from a short built-in table.
- `mousewheel` is refused in a background tab, because Chrome never acknowledges a wheel event there; bring the tab to the front with `tab-select <index> --foreground` or `open --foreground`. Playwright's pages are always visible, so Microsoft's command never meets this.
- Errors go to stderr without the `### Error` heading, as for the other commands.
- `--` ends the options, so `mousewheel -- 0 -100` scrolls up.

## Develop

```sh
npm run lint
npm run test:unit
npm test
```

`npm test` builds the native and wasm CLIs and the guard bridge, installs the pinned Chrome for Testing (`config.chrome`) and runs `test/integration/*.test.mjs`, the Rust workspace tests included. The published guard set from `@ai-ecoverse/slicc-browsers` runs in four files, `guard-a.test.mjs` to `guard-d.test.mjs`, one per partition of its targets; `guard-union.test.mjs` checks the partitions add up to the whole set. Every row must pass, or skip with the reason its vector declares, and the `C2.redirect-*` and `C4.iframe-preattach` `.shared` rows must pass. Each partition writes its report to `artifacts/guard-<partition>.md`.

Real-Chrome integration cases run only on CI by default; set `SLICC_LOCAL_CHROME=1` to opt in locally. Fake-backend cases still run without it.

## CI and releases

`.github/workflows/ci.yml` calls [slicc-shared-web's pipeline](https://github.com/ai-ecoverse/slicc-shared-web#pipeline): lint, the integration tests split over four shards on the pool, and the Claude and Codex reviews on pull requests. The ruleset requires `ci / integration` and `ci / review`.

On `main`, semantic-release publishes `@ai-ecoverse/slicc-cdp` to npm with provenance from `ci.yml`, tags `v<version>` and opens a draft GitHub release. When it released, `ci.yml` then publishes the four crates to crates.io, builds the binaries for macOS, Linux and Windows (the macOS ones signed and notarised), and uploads them with `install.sh`, `LICENSE`, `THIRD-PARTY-NOTICES` and `SHA256SUMS` before it publishes the release as latest. `@ai-ecoverse/sliccy-browser` is published from the same workflow whenever its `package.json` names a version npm doesn't have yet.

## Backend contract

What the clients need from a CDP backend, checked against the code by `crates/cdp-client/src/contract.rs`.

### Browser-level methods

- `Target.getTargets` (`crates/playwright-cli/src/browser.rs` `list_pages`)
- `Target.getTargetInfo` (`crates/cdp-client/src/cdp.rs` `judge_target`)
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
