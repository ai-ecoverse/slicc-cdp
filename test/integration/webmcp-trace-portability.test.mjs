import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';

const read = (name) => readFileSync(new URL(`../../crates/${name}`, import.meta.url), 'utf8');
const body = (source, name) => {
  const found = source.match(new RegExp(`\\bfn ${name}(?:<[^>]*>)?\\(`));
  assert.ok(found, name);
  const start = source.indexOf('{', found.index);
  let depth = 1;
  let end = start + 1;
  for (; end < source.length && depth; end++) {
    if (source[end] === '{') depth++;
    if (source[end] === '}') depth--;
  }
  assert.equal(depth, 0, name);
  return source.slice(start + 1, end - 1);
};

test('WebMCP namespace diagnostics and every Rust call site are removed', () => {
  assert.equal(
    existsSync(new URL('../../crates/playwright-cli/src/webmcp_namespace.rs', import.meta.url)),
    false
  );
  assert.doesNotMatch(read('cdp-client/src/webmcp_guard.rs'), /root-pause-held/);
  for (const name of ['lib.rs', 'commands.rs', 'webmcp_polyfill.rs', 'webmcp_frames.rs']) {
    assert.doesNotMatch(
      read(`playwright-cli/src/${name}`),
      /webmcp_namespace|cdp-675-namespace\.ndjson/
    );
  }
});

test('namespace removal retains endpoint and document policy selection', () => {
  const frames = read('playwright-cli/src/webmcp_frames.rs');
  assert.ok(
    frames.includes(
      'entry.get("connection").and_then(Value::as_str) == Some(&state.webmcp_connection)'
    )
  );
  assert.ok(frames.includes('entry.get("origin").and_then(Value::as_str) == Some(&origin)'));
  assert.ok(frames.includes('unwrap_or("tools=()")'));
  assert.ok(frames.includes('if first == next { first } else { "tools=()".to_string() }'));
});

test('remaining optional Rust diagnostics use no WASI filesystem or process path APIs', () => {
  const guard = read('cdp-client/src/webmcp_guard.rs');
  const cdp = read('cdp-client/src/cdp.rs');
  const names = [
    'webmcp_trace_result',
    'webmcp_trace_stage',
    'webmcp_trace_scheme',
    'webmcp_trace_frame',
    'webmcp_trace_order',
    'webmcp_trace_fetch_command',
    'webmcp_trace_page',
    'webmcp_trace_navigation',
    'webmcp_trace_event',
    'webmcp_frame_refusal',
  ];
  const inspected = [
    ...names.map((name) => body(guard, name)),
    ...['content_blocked', 'session_target', 'raw_exchange', 'raw_reply'].map((name) =>
      body(cdp, name)
    ),
  ].join('\n');
  assert.doesNotMatch(
    inspected,
    /std::(?:fs|path|process)::|std::env::(?:temp_dir|current_dir|home_dir|current_exe|set_current_dir)|\.canonicalize\(/
  );
  assert.match(body(guard, 'webmcp_trace_stage'), /std::env::var\("SLICC_WEBMCP_TRACE"\)/);
  assert.ok(body(guard, 'webmcp_trace_stage').includes("file.rsplit('/')"));
  assert.ok(
    body(guard, 'webmcp_trace_navigation').includes('self.raw_exchange("Page.getFrameTree"')
  );
});
