# slicc-cdp

`@ai-ecoverse/slicc-cdp` publishes `playwright-cli` and `curlwright`, Rust clients for an already-running Chrome. They speak Chrome DevTools Protocol JSON. The package root exports `CDPClient` and `connect`, one browser DevTools socket, and `findBrowser`, `installBrowser` and `launchBrowser` (see [Browser](#browser)). Host `connect()` does not dial port 9222 unless `options.url` or `SLICC_CDP_URL` names it.

`playwright-cli` is a playwright-cli-compatible command for SLICC and not affiliated with Microsoft Playwright.

## playwright-cli

The binary is `crates/playwright-cli`. Host `open` launches a browser into `<config>/slicc/seven-profiles/<id>` when no debugger is already connected, and inside slicc it only opens a tab. It does not invent a debugger path.

Connect, in order: `--cdp`, else `SLICC_CDP_URL`, else a live `DevToolsActivePort` under `<config>/slicc/seven-profiles`, else a loopback endpoint this tool saved at `<config>/slicc/playwright-cli/sessions/<hash>.json`. A `.playwright-cli/session.json` in the working directory is ignored. In the kernel, the wasm builds keep it at `<state>/slicc/playwright-cli/sessions/<hash>.json`, where `<state>` is `$XDG_STATE_HOME`, else `$XDG_CONFIG_HOME`, else `$HOME/.config`, and the hash is of `$PWD`. When that is not writable, `playwright-cli` prints `playwright-cli: running without saved state (<reason>)` on stderr and the command still succeeds, except `route` and `unroute`, which fail because they only save state. A child of that directory is a profile only when its name is a browser id: it starts with a letter or digit, then letters, digits, `.`, `_`, or `-`, at most 64 characters, and it does not contain `..`. Dotfiles, including `.migrated-from-profiles`, are not profiles. `playwright-cli` does not migrate `slicc/profiles`. Host `list`, and `open` when that port is absent or unreachable, GET `/json/version` and `/json/list` on `127.0.0.1:9222-9229`. An unreachable seven-profiles port falls through to that probe, so `open` may attach to a personal browser that already has a seven tab. A successful command writes the loopback endpoint it used, its provenance, and the ids of tabs it created. A saved endpoint is reused only when it is loopback. A probe-derived session is reused only while that browser still has a seven tab, and later commands show only tabs this tool created. An explicit `--cdp` or `SLICC_CDP_URL` may name any host, and that URL is not saved unless it is loopback. Host `open` and `tab-new` treat an unreachable saved endpoint as absent, then probe and may launch. Other commands report that connection error. Launch never binds those ports. The wasm build still uses port 9222. A `ws://` URL is dialed as given. An `http://` URL is discovery. On the native build a `wss://` URL is dialed as given. The WASI build has no TLS, so `wss://` and `https://` fail there. `--runtime <name>` is appended to the query of the URL that is opened and is not interpreted.

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

## Browser

On the host, the package can find, install and launch a Chrome for a test suite or a script. This is Node only, not in the wasm commands.

- **`findBrowser({ version })`:** `$CHROME_PATH`, else the newest Chrome for Testing that `install-browser` put under the install root, else a system Chrome or Chromium (`/Applications`, `/usr/bin`, `/snap/bin`, Program Files). With `version` (a full version or a milestone) only that installed Chrome for Testing counts, and `$CHROME_PATH` is not used. Returns `{ executable, source, version? }` or `null`.
- **`installBrowser({ version, channel, withDeps })`:** downloads Chrome for Testing for this platform, unzips it, checks that `--version` prints the expected version (on Windows, which prints nothing, that the build's `<version>.manifest` is there), and marks it with `.slicc-cdp-browser.json`. An exact version that is already installed is used without the network. The install root is `$SLICC_BROWSERS_PATH`, else `~/Library/Caches/slicc-cdp/browsers`, `$XDG_CACHE_HOME/slicc-cdp/browsers` (default `~/.cache`), or `%LOCALAPPDATA%\slicc-cdp\browsers`. `withDeps` is opt-in. On Linux it runs `apt-get satisfy` on the build's `deb.deps`, as root or with `sudo -n`, and otherwise fails saying so.
- **`launchBrowser({ executable, userDataDir, port, headless, sandbox, args })`:** starts the browser with `--remote-debugging-port` (0 by default) on `127.0.0.1`. Without `userDataDir` the profile is a fresh `slicc-cdp-profile-*` directory in the temp dir, never the user's, and `close()` kills the browser and removes the profile. Ports 9222-9229 are refused, and so are `args` that set the debugging port, address, pipe or profile. A fixed `port` that already answers is refused. Returns `{ child, executable, userDataDir, port, wsUrl, close }`. With port 0 the `wsUrl` comes from a fresh `DevToolsActivePort`, which is removed before launch. Otherwise it comes from `/json/version` on that port. The sandbox stays on unless `sandbox: false`. Linux without unprivileged user namespaces (Ubuntu 23.10 and later, GitHub's `ubuntu-latest` included) needs that, or Chrome aborts with "No usable sandbox".

The `slicc-cdp` bin does the same from a shell:

```
npx slicc-cdp install-browser --version 153.0.8010.12   # prints the executable
npx slicc-cdp install-browser --channel stable --with-deps
npx slicc-cdp find-browser
```

Trust: Chrome for Testing publishes no checksums or signatures for its zips. `installBrowser` downloads only over HTTPS, only from `googlechromelabs.github.io` (the version index) and `storage.googleapis.com` (the zips). Redirects are refused. The only checks are the byte count and the version, so you are trusting those Google hosts and TLS. Pin an exact version (`--version x.y.z.w`) in CI, so a run gets the build you tested.

## Kernel commands

`slicc.abi` is `wasi`. The commands are `playwright-cli` (`bin/playwright-cli.wasm`) and `curlwright` (`bin/curlwright.wasm`). `npm run build:wasm` builds both for `wasm32-wasip1` and copies them there. The release workflow runs that before publish. Do not commit the wasm. On that target, TCP comes from the `wasix-net` 0.1 crate on crates.io.

Inside the kernel the browser socket is `SLICC_CDP_URL`: `ws://127.0.0.1:9222/devtools/browser/<id>` from slicc-kernel. Port 9222 accepts connections only from inside the kernel.

Routes in the kernel use these CDP methods through the facade, always with a tab's `sessionId`: `Fetch.enable`, `Fetch.disable`, `Fetch.continueRequest`, `Fetch.fulfillRequest`, the `Fetch.requestPaused` event, and `Target.getTargetInfo`. The `slicc-cdp-client` API can also send `Fetch.failRequest`, `Fetch.getResponseBody` and `Fetch.continueResponse`; `playwright-cli` doesn't.

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

## Release certification

`cert-review.yml` is reusable and runs after each release has published its npm package, crates and draft assets. Replay an exact version from main with `gh workflow run cert-review.yml --ref main -f version=1.26.2`; `-f dry_run=true` retains the evidence without writing a release or commit status. No job changes the npm `certified` tag. Every reusable job has an explicit status-aware condition, so a skipped ancestor in the release caller does not silently skip evidence. `cert-call-proof.yml` makes a main-only dry-run call with the same release needs and a deliberately skipped asset ancestor; its verifier requires all seven evidence jobs and the resulting record. Shared browser fixtures use the current harness API independently of the older implementation under test, and per-worker summaries retain results defined in imported test helpers.

Hosted planning chooses the newest earlier successful cert-review record, skipping failed and unreviewed releases, with `scripts/cert/base.json` as the lane-approved fallback. Rust is pinned by the tag's `rust-toolchain.toml`, a recorded historical release pin, or the compiler version in that exact release's CI log. The same resolved version, commit, baseline, harness, pins and workflow attempt identify every result document.

The bench pool rebuilds both wasm files byte-for-byte against npm, runs the pinned shared guard runner through the Rust client's public bridge with controls and liveness, runs the same guard suite on the previous certified client's source, and runs the tagged integration suites on published JavaScript, native assets and wasm in the tagged kernel pin. Source-built Rust drivers used by existing integration wrappers are named separately from published command bytes. Guard owner additions remain in the suite. Non-executable shared-set descriptions and explicit upstream non-applicable rows remain visible and are not counted as executed passes.

Fixes map to committed integration titles and declared guard row IDs. `scripts/cert/expectations.json` can name an existing test or guard row for an exact fix commit when it is not inferable from the patch. A missing test, an unrun NEGATIVE, a missing row, a skipped required row, a lost process or a compiler failure in the previous test driver cannot become a pass. Incomplete/failing files get one retry with the same bytes and pins; both attempts are retained. Gaps are `INCOMPLETE`, never waived by a model.

Hosted jobs verify the full release checksums and all four macOS signatures, identifiers, Developer ID team and notarisation. A separate fresh hosted record job independently downloads the exact npm version with `npm pack`, hashes it in code, verifies registry integrity and signed SLSA provenance, requires gitHead = provenance commit = tag on main, and compares the four crates.io versions and source manifests to the tag. It validates all result documents and job outcomes before emitting `record.json`, the job summary, a cert-review status and the release's Certification evidence section.

A complete record emits `certified-candidate @ai-ecoverse/slicc-cdp@<v> sha256=<hex> gitHead=<sha> guard=GUARD-PASS(slicc-cdp@<sha>; vectors @ai-ecoverse/slicc-browsers@<v>)`. Missing evidence emits `cert-ci: INCOMPLETE`; a coded integrity or test failure emits `cert-ci: FAIL`. The cert lane reviews exceptions and retains the authoritative dist-tag decision during this phase. Subsequent publish-only-certified work will certify immutable packed bytes before a hosted main-only publisher sends those same bytes directly to npm latest, then verify provenance, with no candidate npm tag or promotion credential.
