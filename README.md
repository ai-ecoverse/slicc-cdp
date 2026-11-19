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

`playwright-cli upgrade`, or `playwright-cli update`, replaces a prebuilt `playwright-cli` with the latest release, in place. It fetches over https only, and the download must match the release's `SHA256SUMS` and report the release version. It finds the latest release without the GitHub API, whose 60 requests an hour per IP address without a token a shared network or CI runner uses up: it reads the tag from the redirect of `https://github.com/ai-ecoverse/slicc-cdp/releases/latest` (only the tag; the download URLs are built from it) and downloads `SHA256SUMS` and the binary from `releases/download/<tag>/`. When github.com names no release, it asks `GET /repos/ai-ecoverse/slicc-cdp/releases/latest` on the API, with `GH_TOKEN` (or else `GITHUB_TOKEN`) as a bearer token when set; the token goes to the API only, and a used-up rate limit is named in the error. `SLICC_UPDATE_WEB_BASE` and `SLICC_UPDATE_API_BASE` replace `https://github.com` and `https://api.github.com`, for mirrors and tests; with only `SLICC_UPDATE_API_BASE` set, it asks that API only. `--check` only reports whether a newer release exists. It only replaces a binary it can write; a development build needs `--force`, and a copy installed with Cargo is left to `cargo install slicc-cdp`. The wasm build can't upgrade itself.

## Use

```sh
playwright-cli open https://example.com
playwright-cli snapshot
playwright-cli click e3
curlwright -i https://example.com/
```

`open` attaches to a running Chrome or launches one. To pick a browser, pass `--cdp <port>` (for example `--cdp 9333`, which means `localhost:9333`) or `--cdp http://127.0.0.1:<port>`, or set `SLICC_CDP_URL`. Without those, host connect tries `slicc browsers --json` when `slicc` is on `PATH` (prefer `sevenOpen`, then a slicc profile, then best tier; loopback `ws://` only), then a live `DevToolsActivePort` under slicc `seven-profiles` / legacy `profiles` / SLICC six profile dirs (loopback `/json/version` check; the live websocket pathname must match the file; newest live wins), then a loopback endpoint saved under the user config dir, then probes `127.0.0.1:9222-9229`. `curlwright` uses the same order without the saved session and exits when nothing answers. The wasm builds still use port 9222 and skip the CLI and file steps. Run `playwright-cli --help` for all commands.

`--cdp` and `SLICC_CDP_URL` take a port, `host`, `host:port` (port 9222 when it is left out), an `http://` or `https://` URL, or a `ws://` or `wss://` URL. A `ws://` or `wss://` URL with a path is dialled as it is; every other form reads `/json/version` first. The client tries every address the host resolves to, and `localhost`, `127.0.0.1`, `::1` (`[::1]`) and `0.0.0.0` all mean this machine: both `::1` and `127.0.0.1` are tried. Every address but the last gets two seconds to connect.

A `wss://` or `https://` endpoint on this machine with a self-signed certificate takes a pin: set `SLICC_CDP_CERT_SHA256` to the certificate's SHA-256 fingerprint, as 64 hex digits, with or without colons (`openssl x509 -noout -fingerprint -sha256`). The certificate must then match exactly, and it is checked before anything is sent, so a wrong certificate never sees the token in a URL. The pin replaces certificate verification only for names of this machine (`localhost`, `127.0.0.1`, `::1`, `0.0.0.0` and `::`); any other host is verified as usual, and a value that isn't a fingerprint is an error. The wasm build has no TLS: in the kernel, a program reaches the browser through the `ws://` URL in `SLICC_CDP_URL`, and the page that hosts the kernel holds any `wss://` socket.

When the other end closes the connection, for example a relay with `1011` and a reason, the command stops and prints `websocket closed <code> <reason>`, wherever the close arrived, a guard check included. Nothing more is sent on that connection, and a check the close cut short still fails the command.

Seven's origins, the vault, browser pages, extensions and local files are always off limits. Closing is the exception: `tab-close --tab <id>`, or `tab-close` and `close` on the current tab, close a protected tab, which `tab-list` still hides, unless it shows a live page of seven's UI (a seven origin under `/os/`, except `/os/callback.html`) or the vault (`vault.sliccy.ai` and `vault-*` previews). `goto` and `navigate` can leave a blocked error tab for an ordinary HTTP(S) destination without exposing its old document. A page containing a protected or unverified frame refuses whole-page screenshots, PDF output, Runtime, DOM, CSS, accessibility reads, input and eval attachments, including operations in its ordinary frames. A browser extension's frame, such as a password manager's autofill list, is the exception: the page stays readable and the frame stays out of reach. Screenshots and input work only while every such frame is hidden and none holds the deep active element, including inside closed shadow roots; visibility and focus are rechecked before every command. Otherwise the refusal names `chrome-extension://`. The check-to-dispatch race remains a limit: page code can change visibility or focus between the check and delivery. PDF output and screencasts are refused while an extension frame is present. Snapshots skip it. The check includes frames already loaded in other renderer processes and is repeated before returning content. Unplaced frames fail closed. Screencasts, layer snapshots, headless frame capture and tab mirroring remain unsupported; unadmitted screencast events are stopped and discarded. This client guard does not hold sign-in popups or callbacks: the combined sign-in hold sits in front of it and is proved on the bios link route, so the published guard set's `C7.sign-in-hold.*` vectors skip here as `not-supported:signInHold`.

`snapshot` prints the same tree as `@playwright/cli`: the same role names and refs, with `[active]`, `[cursor=pointer]` and `/url:`. It runs Playwright's own aria snapshot code, taken from `playwright-core` 1.64 (Apache-2.0, Copyright Microsoft Corporation), in an isolated world, in `crates/playwright-cli/src/aria-snapshot.js`. A `/url:` is redacted like every other URL: a protected origin shows only its origin, and query values, fragments and credentials show as `…` unless `--full-urls` is given. A cross-origin iframe that runs in its own process is captured through its own flat session, with `f<N>e<M>` refs that `click`, `fill` and the other ref commands drive in that session, and `frames` lists it. A protected, unverified or unknown child frame is never read. `snapshot` refuses its whole page, and `frames` shows a protected frame by its origin alone. The refs' nodes are described in pipelined batches of up to 512 `DOM.describeNode` calls (`call_batch` splits anything larger), with one page check before each batch and one after it, and every batch's check must find the same document as the first batch's. Replies are checked by the page check after their batch, not as they arrive. One 30 s deadline covers a batch's sends and replies. If the page changes before, between or during batches, or a reply fails or is late, the whole call is refused and the snapshot prints nothing.

`eval` calls a function expression (arrow, `function` or async function) and prints its awaited result, like Playwright's evaluate. Other expressions and multi-statement scripts still print their values without being called. `--frame` uses the selected frame's isolated world.

The cookie commands, `cookie-list`, `cookie-get`, `cookie-set`, `cookie-delete` and `cookie-clear`, work like Microsoft's and print ordinary values as they are. Seven's and the vault's cookies, including cookies scoped to a parent domain of a protected host, are never listed, read, set or deleted. Domain arguments are canonicalized before the guard checks them; protected scopes and malformed domains are refused. `cookie-clear` leaves protected cookies, and requests to protected origins are not recorded. Headers containing protected-scope cookies and console lines containing their values are hidden, even on ordinary pages. Response-side `Set-Cookie` for protected scopes and for the catalog's credential origins through `Fetch.fulfillRequest`, `Fetch.continueResponse` and `Network.continueInterceptedRequest` is refused.

The catalog's `credentialOrigins` (from `@ai-ecoverse/slicc-browsers`, today the two IMS hosts `https://ims-na1.adobelogin.com` and `https://ims-na1-stg1.adobelogin.com`) are protected for cookie and storage export on every route, always, and fail closed. The guard withholds their cookies from export, including `Domain=.adobelogin.com` cookies and cookies partitioned under `https://adobelogin.com`, and refuses writes to them with the catalog refusal `slicc: refused: this origin holds sign-in credentials`, including response-side `Set-Cookie` / `Set-Cookie2` through `Fetch.fulfillRequest`, `Fetch.continueResponse` and `Network.continueInterceptedRequest` (`responseHeaders`, `binaryResponseHeaders` and `rawResponse`). Every other cookie under `adobelogin.com` is hidden too, because a parent-domain cookie can't be told apart from the sign-in origin's own. Cookie and Set-Cookie headers are stripped from events, interception and recordings for requests that can carry those cookies. On every request, associated, blocked and exempted cookies whose cookie or `cookieLine` Domain covers a protected or credential host are dropped. Without a Domain the request host decides, and an entry the guard can't parse is dropped. Their localStorage, sessionStorage, IndexedDB, Cache Storage and file system are refused to `localstorage-*`, `sessionstorage-*`, `state-save`, `state-load` and the Storage, DOMStorage, IndexedDB, CacheStorage and FileSystem domains, and storage events that name them are dropped. Cookie issues from `Audits.issueAdded` that name a protected or credential origin, or whose origin can't be told, are dropped. Pages on those origins stay drivable: navigate, attach, `eval`, snapshots, locators, fill, click and screenshots all work, and their URLs aren't redacted. In-page script (`eval`, `Runtime.evaluate`, locators) can still read their non-HttpOnly cookies, localStorage, sessionStorage and IndexedDB. Response bodies, OAuth codes in URLs and Authorization headers on a drivable sign-in page aren't withheld. Stock clients sign in by driving these pages through script, so blocking script would break them. The JS guard's `credentialUrl` (parent-label cover for `cookie.url` on all methods) is stricter than the Rust guard by design. Not covered: cookies scoped only to a sub-host such as `x.ims-na1.adobelogin.com`, Adobe's login UI hosts `auth.services.adobe.com` and `adobeid-na1.services.adobe.com`, and `Domain=.adobe.com` cookies, which all stay exportable, and federated corporate SSO, MFA and consent hosts. Those pages stay drivable and their cookies and storage stay exportable.

Saved sessions never contain cookie values or value hashes. They retain only cookie names and scopes (domain and path), with Cookie and Set-Cookie header values redacted. `request-headers`, `response-headers` and `request` look up those scopes in the live guarded cookie jar and label any values they show as current jar values, not necessarily what was sent at that time. Removed and protected cookies stay `[redacted]`. A header with no retained scopes can show current cookies applicable to its URL only when that URL cannot share a protected parent-domain cookie. Cookie-bearing console messages may be shown live but are not saved.

The web storage commands, `localstorage-list`, `-get`, `-set`, `-delete` and `-clear` and the same five `sessionstorage-` commands, work like Microsoft's and print ordinary values as they are, with terminal control characters escaped. They run in a fresh isolated world of the tab's main frame through the guarded client, so a tab whose current document is protected, unverified, an error page or opaque is refused. Seven's and the vault's localStorage and sessionStorage are never read or written. `--tab` picks the tab; otherwise the current tab is used.

`state-save [filename]` writes Microsoft's storageState JSON: `cookies` in Playwright's cookie shape and `origins` with each origin's `localStorage`. Without a filename it writes `.playwright-cli/storage-state-<timestamp>.json`. **The file is a credential.** It holds live cookie values, including HttpOnly session cookies, and localStorage, where sites often keep login tokens, so anyone who reads it can sign in as you on those sites. On Unix it is created owner-only (0600). Save to a regular file in a writable directory, not a device such as `/dev/stdout`. Atomic publication replaces a destination symlink with a regular file rather than writing through the link. Keep it out of version control, chat transcripts and shared directories, and delete it when you're done. It never holds seven's, the vault's or the credential origins' cookies (parent-domain and partitioned ones included) or their origins, and like Microsoft's it never holds sessionStorage. Deviations: the origins are those of the open ordinary tabs, read in place, not every origin the browser context has visited; a tab the guard can't verify is skipped. An absolute filename prints as given.

`state-load <filename>` checks the whole file before it changes anything. A protected, malformed or non-http(s) cookie scope or origin refuses the file, and so do IndexedDB, OPFS and credential entries, which are not restored. It then clears the ordinary cookies (protected ones stay) and sets the file's cookies. For each origin in the file it opens one temporary background tab, answers the tab's requests to those origins with an empty page through guarded interception, and, as Microsoft does, unregisters that origin's service workers, deletes its IndexedDB databases, clears its localStorage and writes the file's localStorage. The tab is then closed. Deviations: the HTTP cache is not cleared, because that would clear seven's, and origins that are not in the file keep their storage. Errors from these commands go to stderr as `Error: ...` and exit 1.

WebMCP and Browser Annotation API polyfills are owned by [mcpcat](https://github.com/ai-ecoverse/mcpcat). Use `mcpcat serve --webmcp` for page tools. slicc-cdp provides the generic `eval-attach` / `eval-detach` primitive below; consumers supply their scripts and enforce their API semantics. `discover` still reports a page-provided WebMCP API at load time. It does not watch polyfill snapshots.

`dialog-accept [prompt]` and `dialog-dismiss` are bounded live waits: start one before triggering an alert, confirm or prompt. It waits up to five seconds, verifies an observed ordinary main-frame dialog against the browser's current target and navigation history, then answers it. The dialog-only flat session never enables Network or Runtime or reads a renderer frame tree, so it can answer while a click is in flight. Protected, unknown, stale and subframe ownership are refused. A dialog already open before attachment is not answered; the error names the [foreground dialog holder follow-up #529](https://github.com/ai-ecoverse/slicc-cdp/issues/529).

## Eval attachment holder

```sh
playwright-cli --cdp <browser-url> -s shim eval-attach shim.js --tab <targetId> [--world main|isolated] [--early]
playwright-cli --cdp <browser-url> -s shim eval-attach shim.js --host '*.example.com'
playwright-cli --cdp <browser-url> -s shim eval-attach shim.js --match '*://*.example.com/app/*' --match 'https://other.example/*'
playwright-cli -s shim eval-detach [attachmentId]
```

`eval-attach <file>` reads a UTF-8 JavaScript body and runs it with a lexical `send(payload)` function. For example, `send({title: document.title})` sends one message. Functions registered by the body can call `send` later. The source is read once per holder and is installed once per matching top-frame document through ordinary guarded `Runtime.evaluate`, pinned to a verified execution context's unique ID and loader. The normal eval gates apply to installation and queue reads, including page and protected-origin checks. The evaluated wrapper also checks the verified URL before executing the body or draining, so a same-document URL race fails closed. Without `--frames`, only top documents receive the script. Repeat `--frames <http(s)-origin>` to include child documents of exactly that origin, at any depth, or use `--frames same-origin` for children sharing the current top document’s origin. Origins allow an optional trailing slash, but no path, query, fragment, credentials or wildcards. Origins are never inferred from page content. Child navigation installs once in the new verified document. A protected child or ancestor, including seven, the vault, extensions and error pages, never receives the script. Child refusals retain the top attachment; ordinary page guards can suspend its evaluations while a protected child is present. Cross-origin children on WASI or an attested guarded route emit `reason: "cross-origin child tools are unavailable"`.

Choose exactly one scope: `--tab <targetId>`, `--host <exact-host|*.example.com>`, or repeatable `--match <scheme://host/path-glob>`. Host wildcards include the apex and subdomains. Match schemes are `http`, `https`, or `*` (both); hosts are exact, `*`, or `*.example.com`; `*` in the path matches any characters, including `/`. Matching includes the query and ignores the fragment. Ports are ignored; port literals in patterns are rejected. Exact host spelling is lowercase ASCII/punycode. Host/match installation covers HTTP(S) documents. A pattern naming a protected origin is refused before connecting; broad wildcards still pass through the document guard. Matching tabs are discovered through the guarded page list about every 250 ms. Protected targets hidden by that list are never installed; an explicit tab or a watched target that becomes protected reports a refusal without its URL.

By default installation happens after the holder observes the top frame's default execution context and verifies the frame tree; page scripts may already have run, and events report `early: false`. Add `--early` with any scope and either world to finish the synchronous script body before the first parser-inserted inline head script. The holder arms `Debugger.setInstrumentationBreakpoint` (`beforeScriptExecution`), converts the instrumentation pause to an owned debugger pause, verifies the document through the normal guards, evaluates, then resumes. Async callbacks and promises are not awaited. Only this explicit flag admits caller-supplied pre-script code. No `Page.addScriptToEvaluateOnNewDocument` or `Runtime.addBinding` is used.

With `--early`, existing documents, newly discovered tabs whose first script already ran, missed pauses, documents whose first script does not arrive within one second of context creation (including script-less pages), and documents that cannot be verified within the one-second pause deadline are refused (`reason: "late"` or a guard/context refusal). There is no late fallback for top documents. Children selected by `--frames` always install late with `early: false`, including when `--early` was requested. The existing instrumentation owner only admits its session root; polling discovers OOPIFs after they may have executed scripts. Early child installation would require guard-owned auto-attachment holding new iframe targets with `waitForDebuggerOnStart`, arming instrumentation before release, and verification of each child’s first-script pause. `ready` reports the requested mode, not successful installation or a barrier for every tab: wait for the initial refusal before navigating a watched existing tab. A new matching tab may be discovered too late; subsequent matching navigations are watched while the holder is attached. Server redirects are checked against the committed document, including redirects into protected origins.

For `--world isolated --early`, `Page.createIsolatedWorld` creates the attachment's world while the first main-world script remains paused. Its unique context and document loader are verified before evaluation and resume. A successful `document-ready` with `early: true` guarantees the isolated body finished before that page script; DOM changes are visible to it, while JavaScript globals remain isolated. If world creation or verification cannot complete under the pause, the document is refused.

`--world main` is the default. `--world isolated` uses `Page.createIsolatedWorld`; the page cannot read or forge that world's queue. Main-world messages can be forged or changed by the page. All payloads are untrusted page data. `send` serializes immediately with `JSON.stringify`; non-serializable values follow its usual behavior (including throwing on cycles/BigInt). Each queue holds at most 256 messages of at most 65,536 serialized UTF-16 code units each; excess messages are dropped. The holder drains about every 100 ms, with guarded evaluation and backpressure affecting timing. Relayed serialized messages are additionally limited to 65,536 UTF-8 bytes. Control characters are JSON escaped so every event occupies one stdout line.

Call an attachment’s installed realm with `eval` or `eval-file` in the same CLI session and config directory, using `--tab <top-targetId> --attachment <attachmentId> --frame <frameId> --loader <loaderId>`. Take the identifiers from `document-ready`. For example:

```sh
playwright-cli --cdp <browser-url> -s shim eval-file call.js --tab <top-targetId> --attachment <attachmentId> --frame <frameId> --loader <loaderId>
```

This uses the installation’s unique execution context, including its named isolated world or its main world. It rechecks the frame, loader, URL, origin and every ancestor, and refuses a detached attachment or a changed document. A call does not silently follow a navigation. Ordinary `eval --frame` and `eval-file --frame` keep their existing isolated-world behavior when `--attachment` is absent.

Child `document-ready` and `message` events include `origin`, `parentFrameId`, and `ancestors` in top-to-parent order. Each ancestor has `frameId`, `loaderId`, `origin`, `allow`, `srcOrigin`, `permissionsPolicy`, and `toolsAllowed`; the child’s corresponding policy/delegation fields are on the event itself. `allow` and `srcOrigin` come from the embedding iframe in the verified parent’s isolated world, using verified DOM placement. `permissionsPolicy` is the observed Document response header, including an empty string for an observed absent header; `toolsAllowed` is the browser’s native `tools` Permissions Policy decision. Missing evidence is `null`, including headers from documents loaded before observation. No credentialless header re-fetch is performed by this generic holder. Consumers must deny unknown policy evidence, and may need a navigation after attachment to observe headers on browsers without native policy evidence. Evidence is refreshed for messages and bound to verified document identities; it is a snapshot, so consumers must enforce permissions again when invoking tools.

Generic eval does not enforce WebMCP tool semantics. A consumer such as mcpcat can enforce WebMCP checks using `ancestors[0].origin` for the caller and `exposedTo` matching; explicit requested origins and the child `origin` for provider selection; each document’s `toolsAllowed` and `permissionsPolicy` for policy (unknown denies children); and each child/ancestor’s `allow`, `srcOrigin` and its parent’s origin/header for delegation along the entire chain.

This is a foreground held lease, supported on plain Chrome over `--cdp` and in the kernel WASI build. Keep stdin open with a pipe. Closing stdin or running `eval-detach [attachmentId]` in the same working directory and CLI session stops future installs and message relaying. Already-running scripts and their callbacks remain until their document unloads. There is one holder per CLI session; use distinct `-s` names for independent attachments. An optional ID must match the session's lease. Detach waits for acknowledgement and prints the same final event as the holder. A killed holder's stale lease is reclaimable after five seconds using the same native lock/kernel process-identity checks as HAR.

Stdout is NDJSON with these stable fields. Every event has `event: string` and `attachmentId: string` (an opaque `eval-` identifier). IDs refer to CDP targets, frames, and loaders, not CLI tab indexes.

| `event` | Additional fields and types |
| --- | --- |
| `ready` | `world: "main" | "isolated"`, `early: boolean`; requested `--early` mode; lease is held and observation is starting, even if no document matches yet |
| `document-ready` | `tab: string`, `frameId: string`, `loaderId: string`, `url: string`, `world: "main" | "isolated"`, `early: boolean`; true only for a verified pre-script install; script body evaluated successfully |
| `refused` | `tab: string`, `frameId: string`, `reason: "guard" | "unverified-context" | "installation" | "relay" | "late" | "origin-not-requested" | "cross-origin child tools are unavailable"`; children also include `parentFrameId` and `origin` (`null` for guard refusals); non-fatal, no URL or exception text; repeated checks of the same refused document are suppressed, and opaque guard-blocked intervals are coalesced |
| `message` | `tab: string`, `frameId: string`, `loaderId: string`, `payload: JSON value`; attribution is the installation's document, and the payload is untrusted |
| `stopped` | `reason: "stdin" | "detach" | "error"`; final event before exit |

A refused installation or thrown synchronous script body reports `refused` and does not retry that execution context. Navigation supplies a new context and can be installed normally. Browser/transport failures terminate the holder with `stopped`/`error` and a nonzero exit. Each installation records an `eval` action with `--intent`/`SLICC_INTENT`; the long-running holder itself is not a page action.

## Foreground dialog holder

Start `playwright-cli --cdp "$CDP" -s "$SESSION" dialog-hold --tab "$TAB"` before triggering dialogs, keep its stdin open, and wait for its newline JSON `ready` event. This uses the same selected session and user config directory as other commands; there is no daemon or mailbox relay, and no prompt text is saved. Standalone `dialog-accept` and `dialog-dismiss` remain unchanged and do not relay into the holder.

The holder retains its observing CDP session. A `dialog-opened` event supplies `dialogId`; answer that observed dialog by writing one JSON line to the holder's stdin:

```jsonl
{"id":1,"dialogId":7,"command":"dialog-accept","promptText":"Lars"}
{"id":2,"dialogId":8,"command":"dialog-dismiss"}
```

`id` and `dialogId` are unsigned integers. Each request gets a reply event correlated by `id`: `handled`, `no-dialog`, `stale-dialog`, `dialog-refused` or `invalid-request`. A request with no observed open dialog gets `no-dialog`; dialogs opened before this holder attached are still unanswerable. `dialog-closed` revokes the observed ID. Only current verified ordinary main-frame ownership can be answered; protected, unknown, opaque, stale or subframe ownership stays refused. Closing stdin emits `stopped` and detaches.

`ready` contains `target`, `loaderId`, `loaderIdSource` and `status`. The initial `loaderId` can be null (`unavailable`, `observing`): the holder never invents one or queries a frozen renderer. A verified ordinary main-frame `frameNavigated` event supplies the loader ID in `document-ready` (`frame-navigated`, `document-verified`). Dialog ownership is independently reverified before every answer; a loader ID is not permission to handle a dialog. No dialog message, URL or prompt answer is echoed in events.

`resize`, `keydown`, `keyup`, `mousemove`, `mousedown`, `mouseup`, `mousewheel` and `drop` take Microsoft's arguments, print its `### Ran Playwright code` lines and send Playwright's key definitions, buttons, coordinates and modifier masks. The deliberate differences:
- Every command is its own process, so the pointer position, held buttons and held keys are saved per tab in the session, and the next command continues from them as Playwright's `page.mouse` and `page.keyboard` do. Modifiers held with `keydown` apply to later key and mouse events. On a macOS browser `keydown` sends Playwright's editing commands; `ControlOrMeta` follows the browser's platform rather than the CLI's.
- Mouse commands first hit-test the whole pixels around the point with `DOM.getNodeForLocation`, resolve the ordinary frame under that point (including nested and out-of-process frames), and arm the landing check in that frame. `keydown` and `keyup` do the same for the focused frame. Protected, unverified or unresolvable frames, and closed shadow roots, stay refused.
- A native HTML drag that `mousedown` and `mousemove` start is not intercepted. `Input.setInterceptDrags` is a persistent capability the guard refuses, and an intercepted drag could not span separate commands. `drag` and `drop` cover element-to-element and external drops.
- `resize` sets the window with `Browser.setWindowBounds`, adding the measured difference between the window and the viewport, so the viewport becomes `<w>` by `<h>` and stays that size after the command exits. `Emulation.setDeviceMetricsOverride` is not used because Chrome clears it when the command's session detaches. Sizes must be whole numbers from 1 to 10000000. The guard admits the change only for the session's own window, with an ordinary current document and frame tree, and refuses it while any protected or unverifiable tab, frame or other window-capable target is open in the same browser profile, since a window resize reaches every tab in the window. Chrome's own `browser_ui` targets on `chrome://*.top-chrome/`, such as the omnibox popup of a headed window, are not tabs and are not counted.
- `drop` dispatches synthetic `dragenter`, `dragover` and `drop` events in the page, as Microsoft's does, rather than `Input.dispatchDragEvent`, whose file list makes the browser read paths itself. The CLI reads only the regular files named with `--path`, resolved against the working directory, refuses directories and other non-regular files, and caps them at 25 MiB in total. The wasm build reads the kernel's files with the same code. The printed locator is `locator('aria-ref=<ref>')`, not Playwright's generated one, only snapshot refs are accepted as targets, and MIME types come from a short built-in table.
- Chrome never acknowledges `Input.dispatchMouseEvent` `mouseWheel` in a background tab, and Playwright's pages are always visible. In a visible tab `mousewheel` sends the trusted wheel event. In a background tab it scrolls in place, without bringing the tab forward: from an isolated world it dispatches a cancelable, untrusted `WheelEvent` at the point to the element under it, and unless the page cancels that event, it scrolls the nearest ancestor that can scroll in each axis, or the page. Browser-level wheel effects such as Ctrl zoom, smooth scrolling and overscroll do not happen there. Only if that cannot run does `mousewheel` bring the tab to the front, send the trusted wheel and switch back, the same way as the other input commands (see [Input on background tabs](#input-on-background-tabs)). Otherwise it is refused and names `tab-select <index> --foreground`. `Input.synthesizeScrollGesture` is driven by the begin-frames a background tab does not get, and `Emulation.setFocusEmulationEnabled` and `Page.setWebLifecycleState` do not make a tab visible and are persistent capabilities the guard refuses, so none of them is used.
- Errors go to stderr without the `### Error` heading, as for the other commands.
- `--` ends the options, so `mousewheel -- 0 -100` scrolls up.

### Input on background tabs

Headed Chrome doesn't deliver trusted input to a tab it isn't showing, and playwright-cli's tabs usually stay in the background next to seven's. So `click`, `dblclick`, `hover`, `type` and `press` check from an isolated world which events reached the page. If Chrome dropped them, the same events are dispatched in place to the element under the point, or to the focused element, without bringing the tab forward. The page then sees them with `isTrusted: false`: there is no user activation, `:hover` doesn't change, and `click` is dispatched just after the command returns, so a dialog it opens doesn't block it. `type` needs a focused editable element. `press` handles printable keys, `Enter`, `Escape`, `Backspace`, `Delete` and the modifier keys in place.

`drag`, `mousemove`, `mousedown`, `mouseup`, `keydown` and `keyup` take the same path. A `drag` counts as arrived only when both its press and its release did; if only part of it arrived, it is finished in place from the first missing step. In place, the mouse commands dispatch pointer and mouse events with the held buttons and modifiers, `pointerover`/`pointerout` and `mouseenter`/`mouseleave` as the element under the pointer changes from the saved position, focus on `mousedown`, and on `mouseup` a `click` (`auxclick` for other buttons) on the nearest common ancestor of the element under the press point and the one under the release, just after the command returns. A `mouseup` after a `mousedown` that was dispatched in place is dispatched in place too, without Chrome's release, since Chrome never saw the press and would send no click. `drag` does that for its whole move, press, move and release. `keydown` dispatches only `keydown`, with `keypress` and the default action of `press`, and `keyup` only `keyup`. Modifier keys and `Escape` get no `keypress` and no default action. A key `press` can't handle, `keyup` of Space on a button or link, a `drag` from or to a ref inside a frame, and a point with no element take the front-and-back path below, the last without the arrival check, since no element is there to see it. Each refuses rather than reports success when it can't be brought forward.

Only when that can't work (another key, focus inside a frame, or nothing at the point) does it bring the tab to the front, send the input again, check that it arrived and switch back to the tab that was in front. The guard does the switching. It reads which tab is visible from isolated worlds of the ordinary tabs in the window, and switches back to seven's tab only when no ordinary tab there was visible and seven's is the one protected tab in that window. A tab alone in its window is brought forward with nothing to switch back. It refuses, so that no tab is activated, when the vault, another protected or unknown tab could have been in front, when a protected tab is in another window, or when the window is minimized. The error names `tab-select --foreground`. Input that still doesn't arrive fails with an error rather than reporting success.

## Page tools

`generate-locator`, `highlight` and `network-state-set` take Microsoft's arguments and options and print its output and exit codes. `eval-file` and `fetch` are SLICC-only. Where they differ from Microsoft's, or from SLICC's own handlers:
- `generate-locator <ref>` runs Playwright's own selector generator and `asLocator` from the `playwright-core` 1.64 injected script (Apache-2.0, Copyright Microsoft Corporation), vendored in `crates/playwright-cli/vendor/playwright-injected.js.txt` with upstream's license header and its `LICENSE` and `NOTICE` beside it, in an isolated world of the element's frame, so it prints the same `getByRole(…)`, `getByTestId(…)` or `.nth(…)` locator. A ref in an iframe gets the iframe's locator and `.contentFrame()` in front. A frame nested inside an out-of-process iframe is refused instead.
- `highlight <ref>` prints `Highlighted locator('aria-ref=e5')` like upstream. The overlay is SLICC's: an `x-slicc-highlight` element in the top layer, with an open shadow root and one box per element, that follows the element while its isolated world lives. It shows no locator tooltip. `--style` is appended to the box's inline style. `--hide <ref>` removes that element's box, and `--hide` alone removes every overlay in the tab's frames that the guard admits.
- `network-state-set offline|online` is saved with the session, like routes, because each command runs in its own process. While offline, every later command that attaches to a tab first sends Playwright's `Network.emulateNetworkConditions` with `offline: true`, latency 0 and unlimited throughput, and the setting ends when that command detaches. Out-of-process iframes and workers keep their network. The guard admits only that exact shape, only on an ordinary page session, and never on seven's, the vault's or an unknown target. It does not change guard interception. Upstream's `--tab` is not accepted, as upstream has none.
- The vendored script is `injectedScriptSource` from `playwright-core@1.64.0-alpha-1790635538000` (`lib/coreBundle.js`), minified with `esbuild@0.25.10`. `node scripts/vendor-playwright-injected.mjs [version]` downloads that package with `npm pack`, extracts and minifies the source the same way, and rewrites the file, `LICENSE` and `NOTICE`; with the default version it reproduces the committed bytes. The `.txt` extension keeps the license header out of the no-comment lint, and the file is only embedded with `include_str!`.
- These ref commands take snapshot refs only, not selectors, like `click`. Errors go to stderr; upstream prints them on stdout under `### Error`, except for `highlight` with neither a ref nor `--hide`, which prints upstream's error.
- `eval-file <path> [--tab] [--frame] [--output|--filename <file>]` reads only the file you name, up to 16 MiB of UTF-8, and evaluates it through `eval`'s guarded path: a function expression is called, and top-level `await` and `return` work. `--output` prints `Result saved to <file> (<n> KB)`.
- `fetch <url> [--method <method>] [--tab]` runs `fetch()` in the tab, so the tab's cookies, origin and CORS rules apply. It is not SLICC's proxied fetch and has no `--discover`. A relative URL is resolved against the document. Only http and https are fetched. A protected destination is refused before connecting and again after it is resolved. Every request uses `redirect: 'manual'`: ordinary redirect Locations must be unambiguously correlated to that request in admitted CDP Network events and pass the guard before the next hop is sent. Missing or ambiguous redirect metadata fails closed, and at most 20 redirects are followed. POST becomes GET on 301/302, and 303 becomes GET except for HEAD; 307/308 preserve the method. The body is never read. It prints JSON with `url`, `status`, `redirected`, the `links` from the final `Link` header (one entry per `rel`, protected targets dropped) and the SLICC `upskill` link as `handoff`, and exits 1 on non-2xx responses or with an `error` when the request fails. The guard redacts SLICC's `handoff` relation, which names seven's origin, so it is listed only as that origin and never reported as a handoff.

## Intent

Every `playwright-cli` command, and `curlwright`, takes `--intent "<text>"` or `--intent=<text>`: what the command is for, in a few words, such as `--intent "Filling the order form"`. Without the flag it uses `SLICC_INTENT`, which slicc-agent sets for trusted bash calls. The flag always wins, and `--intent=` turns an inherited intent off. Neither is required, and a command without an intent works as before. The text is cleaned, never refused: control and format characters (bidi overrides and isolates, zero-width characters, the byte order mark, soft hyphens, tag characters, variation selectors) are removed, whitespace, including line and paragraph separators, collapses to single spaces, and the result is cut to 120 characters. The intent is never parsed, evaluated or printed, and nothing intent-related is ever sent into a page. It goes only into the `Slicc.action` notification, a JavaScript line comment in a recording, a HAR `comment` string and a ledger file.

The cleaned intent is the `intent` field of the `Slicc.action` start notification, next to `kind`, `tab`, `target` and `agent`. slicc-bios turns these notifications into the Browser panel's recent actions. The end notification doesn't repeat it. Only commands that send `Slicc.action` carry it: `open`, `tab-new`, `goto`, `navigate`, `close`, `tab-close`, `tab-select`, `snapshot`, `screenshot`, `eval`, `click`, `fill`, `type` and `press` in playwright-cli, and every curlwright request.

For recordings and HAR, each page command with an intent (`open`, `tab-new`, `goto`, `navigate`, `go-back`, `go-forward`, `reload`, `click`, `dblclick`, `fill`, `type`, `press`, `keydown`, `keyup`, `hover`, `select`, `check`, `uncheck`, `upload`, `drag`, `drop`, the `mouse*` commands, `eval`, `eval-file`, `snapshot`, `screenshot`, `pdf`, `resize`, `highlight`, `generate-locator`, and `fetch`) writes its tab, intent, start and end to a 0600 file in `<user-config>/slicc/playwright-cli/intents/` once it knows its tab, and again when it ends. A page command without an intent writes an entry with no intent only when a command with one is still running on its tab or ended less than a second before, which closes that command's window. The next write deletes files older than ten minutes. The ledger is keyed by tab, so it is shared by every session and working directory on this machine, but the host CLI and the kernel have separate file systems and don't see each other's.

`recording-stop`, and the holder's `stopped` event, put the intent of the command that caused an action in a `// <intent>` line above it. Consecutive actions with the same intent share one comment. The recorder stamps each action with the time its event fired, from its isolated world, and the holder matches it against the ledger for its tab when it stops.

A `har-start` or `record` holder sets an entry's HAR `comment` to the intent of the command that caused the request, matched by the request's `startedDateTime` (Chrome's `wallTime`) against the ledger for the holder's tab. The request belongs to the latest command on that tab that had started and was still running, with a command that died counted as running for 60 s. Failing that, it belongs to the last command that had started, if the request came less than 1 s after that command ended. Commands that overlap with different intents, and a later command without an intent, leave the comment out. Entries are matched at each checkpoint and on stop. This is attribution by time, not cause: a page timer, a poll or another client's request inside the window gets the intent too, and a request a command set off more than a second after it ended gets none. Both times come from this machine's clock, so a browser on another machine shifts the window by its clock skew.

## Regenerating the Playwright injected script

From the repository root, run:

```sh
node scripts/vendor-playwright-injected.mjs 1.64.0-alpha-1790635538000
git diff -- crates/playwright-cli/vendor
npm run lint
```

The pinned package and `esbuild@0.25.10` produce the committed bytes. Keep the upstream copyright and Apache-2.0 header, `LICENSE` and `NOTICE` together when updating. If changing the source version, update the script's default version and this README, then run the native and kernel `page-tools.test.mjs` locator regressions in CI.

## Recording

`recording-start` and `recording-stop` are Microsoft's user-action recording commands. `record` and `stop-recording` are SLICC HAR aliases, not aliases for these commands. Microsoft keeps the browser in a daemon, so its `recording-start` returns at once. There is no daemon here: `recording-start` is a foreground holder, like `eval-attach`. It records until its stdin closes or `recording-stop` runs in another terminal with the same session.

```sh
playwright-cli recording-start --tab <targetId>
playwright-cli recording-stop
```

The holder prints one JSON event per line: `ready` with the tab and `loaderId`, `paused` and `resumed` around documents it may not record, and `stopped` with a `reason` (`stdin`, `stop`, `closed` or `refused`) and the recorded `actions`. `recording-stop` prints Microsoft's result: `### Result`, `Recording stopped. Recorded actions:` and the actions in a `js` code block, or `Recording stopped. No actions were recorded.` Without a running holder it prints Microsoft's `Error: No recording in progress, use browser_start_recording to start one.` on stderr and exits 1. A second `recording-start` while one runs prints `Error: Recording is already in progress.` and exits 1.

The recorder writes Playwright code for trusted clicks, double clicks, right and middle clicks, fills, key presses, checks, selects and file choices in the main frame. A new document is recorded as `page.goto` unless a click, press or fill came less than five seconds before it. Locators prefer a test id, then role and accessible name (from Playwright's aria code in `aria-snapshot.js`), placeholder, label, text, role alone, a stable id, and finally `nth` or a CSS path.

The recorder runs in an isolated world that the guarded client installs only after the guard has verified the current main-frame document, and its events are read through guarded `Runtime.evaluate` calls pinned to that context. It uses no binding, init script, screencast or tracing. When the holder stops it removes the recorder's listeners, each start replaces any earlier recorder on that document, and a recorder that nobody drains for five seconds drops what it sees, so input between recordings is never captured. Nothing is installed in or read from seven's, the vault's, protected, unverified or unknown documents: capture pauses on an unverified document, and recording stops with `reason: "refused"` and exit 1 when the tab turns protected. Actions recorded before that stay available to `recording-stop`. Frames are not recorded, so a protected iframe is never read. A `goto` URL is redacted like every other URL unless `--full-urls` is given. The holder keeps the actions in memory; when it stops for `recording-stop`, a closed tab or a refusal, the result waits in a 0600 file next to the saved session until `recording-stop` reads and deletes it.

Deliberate differences from Microsoft's recorder:
- `recording-start` holds the terminal and prints JSON events instead of `### Result` and returning.
- `recording-stop` prints no `### Page` or `### Snapshot` section, like the other commands here.
- Only the selected tab's main frame is recorded: no iframes, popups (`page1`), dialog, download or popup signals, and no assertions.
- Events are observed, not intercepted: there is no recorder overlay, and the locator generator is a smaller version of Playwright's.
- The code is always JavaScript; the `codegen` language setting does not exist here.
- Actions caused by a command with an intent carry it as a comment; see [Intent](#intent).
- `--tab <targetId>` picks the tab. SLICC's `record [url] [--filter=<glob>]` and `stop-recording [recordingId]` record HAR traffic instead, as described below. They do not record user actions.

### HAR recording (SLICC extension)

```sh
playwright-cli -s capture har-start --tab <targetId> --url-filter '**/api/**' --content omit --mode full traffic.har
playwright-cli -s capture har-stop
```

`har-start [--tab <id>] [--url-filter <glob>] [--content omit|embed] [--mode full|minimal] [--include-credentials] <file>` is a foreground holder, not a daemon. Keep stdin open; stdin EOF or `har-stop` in another terminal with the same working directory and session ends it. JSON events (`ready`, `paused`, `resumed`, `checkpoint`, `stopped`) include `recordingId`, the tab, output path and entry count. HAR 1.2 is atomically checkpointed on main-frame navigation and written on stop, for Chrome DevTools and Playwright `routeFromHAR`. The snapshots are cumulative; the supplied file is replaced atomically, not appended. The archive is private (0600 on native Unix and WASI), but existing output directories and their parents keep their original permissions. Session control files remain in the private config directory. A killed holder's stale lease is reclaimed automatically after five seconds; an active holder's lease is never removed by a regular stop command. Lease ownership is verified independently of heartbeat age using a held native file lock or the kernel process's PID, start time and boot identity, so a starved background job is not mistaken for a dead holder. Terminal results are published once in a recording-specific completion file; har-stop waits for that acknowledgement instead of reading a longer terminal JSON value through a cached heartbeat-file snapshot.

Entries a command with an intent caused carry it in `comment`; see [Intent](#intent). Bodies, including request post data, are omitted by default. `--content embed` enables body capture only while the current document and response are verified ordinary. `--mode minimal` uses zero replay timings; `full` retains capture duration. Cookie, Set-Cookie, Authorization and Proxy-Authorization headers are stripped and cookies arrays are empty by default. `--include-credentials` permits ordinary credential headers; it never admits protected traffic. Protected or unclassifiable URLs, protected schemes and their redirect hops are excluded. An ordinary redirect response into a protected destination can remain as its own entry with redirect carriers redacted by the guard; no destination entry or body is retained. Capture pauses on protected or unknown documents and reattaches only after a fresh guarded verification of an ordinary page. Each admitted request retains its issuing document URL and loader. Completed ordinary metadata survives a later navigation, but body reads still require the current document to verify. Paused fetches and child-document traffic are never retained. An ordinary main-document request that starts while paused is quarantined with its redirect hops, even when it finishes loading before its commit. It enters the HAR only after its issuing loader verifies as the current ordinary document; a commit of any other document discards it.

For the slicc6 skill:

```sh
playwright-cli -s capture record https://example.com --filter='**/api/**'
playwright-cli -s capture stop-recording <recordingId>
```

`record [url] [--filter=<glob>] [--content omit|embed] [--mode full|minimal] [--include-credentials]` opens a new tab (default `about:blank`), attaches before navigating, and writes `<user-config>/slicc/playwright-cli/recordings/<recordingId>/recording.har` beside saved sessions (for example `~/Library/Application Support/slicc/…` on macOS or `~/.config/slicc/…` on Linux). Its JSON `ready` event prints the recordingId and path. The filter is a URL glob; slicc6's arbitrary JavaScript filter functions are rejected with an explanatory error. `stop-recording [recordingId]` aliases `har-stop`; an optional ID must belong to the same session. The Microsoft action commands remain `recording-start` and `recording-stop`.
- Microsoft's `video-*` and `tracing-*` commands are not offered: `Page.startScreencast` and `Tracing.start` keep capturing across navigations, including into protected documents, and the guard refuses both.

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

`sliccy-browser` has no dependencies, on purpose. The kernel's pnpm hoists a dependency next to the package, and the kernel reads `slicc.commands` from everything in a global `node_modules`, so a dependency on `@ai-ecoverse/slicc-cdp` would give the user that copy's `playwright-cli` and `curlwright` ahead of the base set's. It carries its own copies of `src/connect.js` and `src/cdp-client.js`, and of `@ai-ecoverse/slicc-browsers`'s `protected.mjs` plus the catalog's `protectedOrigins` rules as `src/protected.js` and `src/protected-origins.js`, instead. When a change to those files lands, `test/integration/sliccy-browser-install.test.mjs` fails until the same pull request copies them and raises `sliccy-browser`'s version, which publishes them. It fails on any other change to what `sliccy-browser` ships that doesn't raise the version, too. A slicc-cdp release that leaves those files alone needs nothing here.

`@ai-ecoverse/sliccy-browser` finds a browser without launching one. Order: `SLICC_CDP_URL` if set; else `slicc browsers --json` when `slicc` is on `PATH` (prefer `sevenOpen`, then a slicc profile `userDataDir`, then the best tier); else a live `DevToolsActivePort` under slicc's `seven-profiles` / `profiles` / SLICC six profile dirs on this machine (loopback `/json/version` check, and the live `webSocketDebuggerUrl` path must match the file's second line so a reused port with a different browser id is skipped; newest matching file wins); else `127.0.0.1:9222`. Loopback only; no arbitrary port scan. When nothing answers, the error lists each step. An explicit `SLICC_CDP_URL` stays unguarded. On a discovered endpoint it refuses protected top-level tabs (catalog `protectedOrigins`, plus missing/non-page targets, empty URLs, inheriting `about:` documents and `chrome-error:` pages) for `findTab`, `ensureTab`, `eval`, `evalAsync`, `localStorage`, `cookie`, `fetch` and `websocket` (fail closed, live URL pre/post checks). `cookie` asks CDP with `urls: [top-level]` and drops any cookie whose domain, url or partition key is in a protected cookie scope (including parent-domain scopes such as `.ai`). It also drops cookies of the credential origins `https://ims-na1.adobelogin.com` and `https://ims-na1-stg1.adobelogin.com`, by the same rule as playwright-cli, while their tabs stay usable. `localStorage` and `eval` run script in the page, so on a credential-origin tab they can still read its localStorage and non-HttpOnly cookies, as with playwright-cli `eval`. `websocket` drops frames whose socket URL is protected. Chrome does not replay `Network.webSocketCreated` for sockets that were already open before the first `Network.enable` on that tab, so discovered-mode subscriptions also drop frames from those unknown-URL sockets (fail closed); the subscription warns once and exposes a `dropped` count, and reloading the tab or reconnecting the socket after the first attach makes a new allowed socket visible. Page-level residuals it does not stop: same-origin frame DOM reads through `eval`, `window.open` / `postMessage` / `BroadcastChannel` / service-worker side channels, and anything an allowed page's own script can do without a CDP aggregation. `node:child_process` is imported only when the CLI discovery step runs, so `require('sliccy:browser')` still loads under the kernel's `slicc` export condition.

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
- `Browser.getVersion` (`crates/cdp-client/src/document_verification.rs` `document_route_attested`)
- `Browser.getWindowForTarget` (`crates/cdp-client/src/front.rs` `front_window`)
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
- `Network.getResponseBodyForInterception`
- `Network.takeResponseBodyForInterceptionAsStream`
- `Inspector.enable` (private guard only)
- `Fetch.*`

The guarded Rust client supports document-scoped operations, origin-scoped cookies and guarded interception. Persistent page capabilities that cannot be restricted to verified ordinary documents are refused before sending, including generic init scripts, bindings, page overrides and automatic debugger-waiting attachment. `Network.emulateNetworkConditions` is the one network override admitted, and only as Playwright's offline toggle on an ordinary page session. `eval-attach --early` is the only pre-script execution path. It accepts caller-supplied source and verifies the document, loader and unique context through the ordinary eval gates before evaluating it. Its private instrumentation lease uses `Debugger.enable`, `Debugger.setInstrumentationBreakpoint`, `Debugger.pause`, `Debugger.resume`, `Debugger.removeBreakpoint` and `Debugger.disable`. Generic calls cannot acquire that lease; source reads, call-frame evaluation and every other Debugger, DOMDebugger or EventBreakpoints method remain refused. The production method lifetime [inventory](crates/cdp-client/src/capability_inventory_test.rs) is checked against the CLI sources. The Node `CDPClient` is a raw CDP transport, not the guarded Rust client. Both clients send message IDs from 1 through 2^30 - 1 and refuse further sends when that range is exhausted; IDs never wrap.

### Session-level domains

- `Page`
- `Runtime`
- `Debugger`
- `DOM`
- `Input`
- `Network`
- `Fetch`
- `Inspector` (private guard only)

Guard sessions enable Inspector before renderer-side domains. Chrome then reports an already-crashed target on the new session, rather than letting Network.enable consume the guard deadline. A reported renderer crash refuses the call; it is never retried silently.

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

Without `--tab` it uses the tab `playwright-cli` last selected, else the one open tab on the URL's origin. `--intent` and `SLICC_INTENT` work as for `playwright-cli` (see [Intent](#intent)) and go into the request's `Slicc.action`.

`curlwright upgrade`, or `curlwright update`, replaces a prebuilt `curlwright` with the latest release, under the same rules as `playwright-cli upgrade`. A copy installed with Cargo is left to `cargo install curlwright`. As the first argument the word is always that command, so write `./update` for a relative URL.

#### Web Bot Auth signing

`--sign-key <file>` signs every request curlwright sends in that run, redirect hops included, with [Web Bot Auth](https://datatracker.ietf.org/doc/draft-ietf-webbotauth-httpsig-protocol/) (RFC 9421 HTTP Message Signatures). The file is an Ed25519 private JWK (`kty` `OKP`, `crv` `Ed25519`, `d`, and `x` if you like); any other key exits 2 without printing it. Each hop gets `Signature-Input` and `Signature` under the label `sig1`, covering that hop's `@authority`, with `created`, `expires` 60 seconds later, `keyid` (the key's RFC 7638 JWK thumbprint), `alg="ed25519"`, a random `nonce` and `tag="web-bot-auth"`.

```
curlwright --sign-key bot.jwk --signature-agent https://bot.example https://example.com/
```

`--signature-agent <https URL>` names your key directory: it sends `Signature-Agent: sig1="<url>"`, the dictionary form of draft -00, and covers it as `"signature-agent";key="sig1"`. Cloudflare's verifier still expects the older bare string, so `--signature-agent-legacy` sends `Signature-Agent: "<url>"` and covers `"signature-agent"`. The draft wants Signature-Agent on every signed request; leave it out only for a verifier that already holds your key. Serving the directory is up to you.

Nothing is signed without `--sign-key`, and `playwright-cli` never signs: its navigations, `fetch` and `discover` probes are the user's own browser traffic. The headers ride on the page's `fetch()`, so a cross-origin server has to allow them in a CORS preflight, which the browser sends unsigned. Signing needs an absolute http(s) URL. The signer is tested against Cloudflare's published Ed25519 test vectors, the draft's Appendix E.2.

Prebuilt binaries: `curl -fsSL https://get.sliccy.ai/curlwright | sh`. See [slicc-cdp](https://github.com/ai-ecoverse/slicc-cdp).
