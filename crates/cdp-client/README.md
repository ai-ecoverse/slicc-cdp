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

Seven and the vault are never intercepted. `protected::Protected` implements `protectedOrigins` from the `slicc-browsers` crate with a WHATWG URL parser and passes every vector in its `protected.json`. A paused request whose URL, or whose `Location`, is protected is continued unchanged at once and never reaches `next` or a decider. `enable` refuses a pattern that names a protected host, and any call without a session (browser-wide `Fetch.enable`). `Interception::attach` refuses a target on a protected origin, or a blank popup whose opener is protected or gone. A decision can't redirect to a protected URL, by `url`, `Location` or `Refresh`. `Cdp::protect_origins(&[...])` adds extra origins.

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
