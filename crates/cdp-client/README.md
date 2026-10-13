# slicc-cdp-client

Shared Chrome DevTools Protocol connection for the `playwright-cli` and `curlwright` binaries.

The crates.io name is `slicc-cdp-client`. The Rust library name stays `cdp_client`:

```
cdp-client = { package = "slicc-cdp-client", version = "1" }
```

One browser WebSocket. `Target.attachToTarget` uses `{targetId, flatten: true}`. Later page commands send that `sessionId` at the top level. There is no `/devtools/page/<id>` socket.

## Fetch interception

`cdp_client::fetch` pauses requests on one target session. `Interception::attach(&mut cdp, target_id, &patterns)` checks the target, attaches, and sends `Fetch.enable`; dropping it, or `finish()`, sends `Fetch.disable` and detaches. `Interception::on_session` does the same on a session you already hold and leaves it attached. `cdp::detach` also forgets that session's paused requests.

- `Pattern::request(glob)` and `Pattern::response(glob)` are CDP `urlPattern`s (`*`, `?`, `\` to escape) at the Request or Response stage, with an optional `resource_type`.
- `next(timeout)` returns a `Paused`: `method`, `url`, `url_fragment`, `full_url()`, `headers`, `post_data`, and at the response stage `status`, `response_headers` and `location()`, which resolves `Location` against the request URL and keeps its fragment.
- `decide(&paused, &Decision)` sends `Continue(Overrides)` (`continueRequest` with URL, method, body or headers), `ContinueResponse(ResponseOverrides)`, `Fulfill(Fulfill)` (`fulfillRequest`: status, headers, body; `Fulfill::signed_in()` is a short "Signed in, you can close this tab" page) or `Fail(ErrorReason)` (`failRequest`). `response_body(&paused)` is `Fetch.getResponseBody`, decoded.
- `navigation(guard, event)` reads a `Page.frameNavigated` event as `url` plus `urlFragment`, for a redirect made by script.
- `Cdp::set_decider(session, Some(Box<dyn FnMut(&Paused) -> Decision>))` answers pauses inline, even while another call waits for its reply, so a synchronous `Page.navigate` doesn't deadlock. A decision CDP rejects is retried as a plain continue and kept in `take_fetch_errors()`.

## Seven's origin is off limits

`Cdp` filters every command and event the way slicc-cli's `cdp` relay does (slicc-cli README, "Seven's origin is off limits"). What is protected is `protectedOrigins` from the `slicc-browsers` crate: every `sliccy.ai` host, `vault.sliccy.ai` by name, `blob:`, `filesystem:` and `view-source:` URLs by their inner URL, `devtools:`, and the extra origins from `Cdp::protect_origins(&[...])`. `protected::Protected` parses URLs with a WHATWG parser and passes every vector in `protected.json`. Refusals are `slicc: refused: target is on seven's origin`, or a reason of their own after `slicc: refused: `.

- **Targets.** A target is protected when its URL is, or when it is on `about:blank` (or another inheriting URL) and was protected before or has a protected or unknown opener. A page session also becomes protected when its main frame commits a protected URL or `securityOrigin` (`Page.frameNavigated`), when its default context's origin is protected (`Runtime.executionContextCreated`), or when the target moves there (`Target.targetInfoChanged`). Such a session is detached, its events and results are dropped, and every later command on it is refused.
- **Refused:** `Target.createTarget`, `Page.navigate` and anything else whose `url` is protected; `Page.navigateToHistoryEntry` to a protected entry; `attachToTarget`, `activateTarget`, `closeTarget`, `Browser.getWindowForTarget` and any command whose `targetId` or `frameId` is a protected target; every command on a protected session. `Target.getTargetInfo` and `Target.detachFromTarget` stay allowed.
- **Origins, storage and cookies.** A command whose `origin`, `securityOrigin`, `topLevelSite`, `scopeURL` (and the rest of slicc-cli's list), `storageKey` or `cacheId` names a protected origin is refused. Free-form values (`expression`, `arguments`, `value`, `headers`, `postData`) aren't searched. Cookie readers answer without protected cookies, and cookie writers for a protected domain, URL or partition site are refused with `slicc: refused: cookies on seven's origin`. So are `Network.clearBrowserCookies` and `Storage.clearCookies`, except in a browser context this `Cdp` created.
- **Browser-wide:** `Target.exposeDevToolsProtocol`, `Target.sendMessageToTarget`, `Target.attachToBrowserTarget`, `Tracing.start`, `Extensions.loadUnpacked`, `Fetch.*` without a session, permissions without an origin, and `Browser.resetPermissions` outside an own context. `Target.setAutoAttach` with `autoAttach: true` is refused: slicc-cdp does not support auto-attach.
- **Requests.** A protected request's `Network.*` and `Fetch.*` events are dropped, `ExtraInfo` events are held until their request is known, and seven's cookies leave `associatedCookies`. Its body getters (`Network.getResponseBody`, `getRequestPostData`, `Fetch.getResponseBody`, `replayXHR` and the rest) are refused, and so is one for a request this `Cdp` never saw (`slicc: refused: slicc hasn't seen this request, so it could be seven's`). `IO.read` works only on a stream this `Cdp` handed out.
- **Navigation by other means.** A target this `Cdp` attaches to or creates is touched. On it, `Cdp` attaches a session of its own, enables `Fetch` for document requests at the request stage plus `Network.setBypassServiceWorker`, and fails each document request into a protected origin with `BlockedByClient`. A script's `location`, a link, a meta refresh or a server redirect into the vault then shows Chrome's error page, and the target is protected from then on. `Target.createTarget` opens `about:blank`, touches it, and then navigates. A paused document request to a protected origin on your own `Fetch` is failed the same way, and other protected requests are continued unchanged; neither reaches `next` or a decider.
- **Listing.** `Target.getTargets`, `getTargetInfo`, `targetCreated` and `targetInfoChanged` still show protected targets, but their `url` and `title` are only the origin. `Cdp::is_protected_target(id)` says which ones. `playwright-cli` and `curlwright` leave them out entirely.
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
