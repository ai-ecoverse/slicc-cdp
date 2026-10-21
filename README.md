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

`open` attaches to a running Chrome or launches one. To pick a browser, pass `--cdp http://127.0.0.1:<port>` or set `SLICC_CDP_URL`. Run `playwright-cli --help` for all commands.

Seven's origins, the vault, browser pages, extensions and local files are always off limits.

The cookie commands, `cookie-list`, `cookie-get`, `cookie-set`, `cookie-delete` and `cookie-clear`, work like Microsoft's and print ordinary values as they are. Seven's and the vault's cookies, including cookies scoped to a parent domain of a protected host, are never listed, read, set or deleted. Domain arguments are canonicalized before the guard checks them; protected scopes and malformed domains are refused. `cookie-clear` leaves protected cookies, and requests to protected origins are not recorded. Headers containing protected-scope cookies and console lines containing their values are hidden, even on ordinary pages.

Saved sessions never contain cookie values or value hashes. They retain only cookie names and scopes (domain and path), with Cookie and Set-Cookie header values redacted. `request-headers`, `response-headers` and `request` look up those scopes in the live guarded cookie jar and label any values they show as current jar values, not necessarily what was sent at that time. Removed and protected cookies stay `[redacted]`. Cookie-bearing console messages may be shown live but are not saved.

## Develop

```sh
npm run lint
npm run test:unit
npm run test:integration
```

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

### Session-level domains

- `Page`
- `Runtime`
- `Debugger`
- `DOM`
- `Input`
- `Network`
- `Fetch`
- `Accessibility`

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
