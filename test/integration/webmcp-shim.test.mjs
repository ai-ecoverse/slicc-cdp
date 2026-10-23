import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const source = await readFile(
  new URL('../../crates/cdp-client/src/webmcp-shim.js', import.meta.url),
  'utf8'
);

function realm({ secure = true, native = false, defined = false } = {}) {
  class Document {}
  class Navigator {}
  const document = new Document();
  const navigator = new Navigator();
  const existing = { registerTool() {} };
  if (native) Object.defineProperty(Document.prototype, 'modelContext', { value: existing });
  if (defined) document.modelContext = existing;
  const location = { origin: 'https://ordinary.test' };
  const window = { frames: [], document, location };
  const context = vm.createContext({
    Document,
    Navigator,
    document,
    navigator,
    window,
    location,
    isSecureContext: secure,
    EventTarget,
    Event,
    AbortController,
    DOMException,
  });
  const bridge = vm.runInContext(source, context, { filename: 'webmcp-shim.js' });
  return { document, navigator, bridge, context, window, existing, Document };
}

function definition(name = 'greet', execute = (input) => ({ greeting: input.name })) {
  return {
    name,
    title: 'Greet',
    description: 'Greets a visitor',
    inputSchema: { type: 'object' },
    annotations: { readOnly: true, untrustedContent: true },
    execute,
  };
}

function tools(bridge) {
  return JSON.parse(bridge.list().toolsJson);
}

test('WebMCP shim is inert for native, page-defined and insecure documents', () => {
  for (const options of [{ secure: false }, { native: true }, { defined: true }]) {
    const page = realm(options);
    assert.equal(page.bridge, undefined);
    assert.equal(page.navigator.modelContext, undefined);
    assert.equal(page.document.modelContext, options.secure === false ? undefined : page.existing);
  }
});

test('WebMCP shim registration, sorting, annotations and abort lifecycle', async () => {
  const page = realm();
  const api = page.document.modelContext;
  assert.equal(api, page.navigator.modelContext);
  assert.equal(Object.prototype.toString.call(api), '[object ModelContext]');
  assert.equal(Object.getOwnPropertyDescriptor(page.document, 'modelContext').configurable, true);
  let events = 0;
  let handlerEvents = 0;
  api.addEventListener('toolchange', () => events++);
  api.ontoolchange = () => handlerEvents++;
  const registration = new AbortController();
  api.registerTool(definition('z'), { signal: registration.signal });
  api.registerTool(definition('a'));
  assert.deepEqual(
    tools(page.bridge).map((tool) => tool.name),
    ['a', 'z']
  );
  assert.deepEqual(tools(page.bridge)[0].annotations, {
    readOnly: true,
    untrustedContent: true,
    readOnlyHint: true,
    untrustedContentHint: true,
  });
  assert.equal((await api.getTools())[0].window, page.window);
  assert.throws(() => api.registerTool(definition('a')), { name: 'InvalidStateError' });
  assert.throws(() => api.registerTool({ name: 'invalid' }), { name: 'TypeError' });
  assert.throws(() => api.registerTool({ ...definition('schema'), inputSchema: [] }), {
    name: 'TypeError',
  });
  registration.abort();
  assert.deepEqual(
    tools(page.bridge).map((tool) => tool.name),
    ['a']
  );
  const stopped = new AbortController();
  stopped.abort();
  api.registerTool(definition('never'), { signal: stopped.signal });
  assert.equal(events, 3);
  assert.equal(handlerEvents, 3);
});

test('WebMCP shim execution has native errors, object input and inner abort signal', async () => {
  const page = realm();
  const api = page.document.modelContext;
  api.registerTool(definition());
  assert.equal(await api.executeTool('greet', { name: 'Alice' }), '{"greeting":"Alice"}');
  assert.equal(
    await api.executeTool((await api.getTools())[0], '{"name":"Bob"}'),
    '{"greeting":"Bob"}'
  );
  await assert.rejects(api.executeTool('missing', {}), { name: 'NotFoundError' });
  await assert.rejects(api.executeTool('greet', 'bad'), { name: 'UnknownError' });
  await assert.rejects(api.executeTool('greet', 42), { name: 'UnknownError' });
  api.registerTool(
    definition('fail', () => {
      throw new Error('failed');
    })
  );
  await assert.rejects(api.executeTool('fail', {}), { name: 'UnknownError' });
  let inner;
  api.registerTool(
    definition('wait', (_input, client) => {
      inner = client.signal;
      return new Promise(() => {});
    })
  );
  const outer = new AbortController();
  const pending = api.executeTool('wait', {}, { signal: outer.signal });
  outer.abort();
  await assert.rejects(pending, { name: 'AbortError' });
  assert.equal(inner.aborted, true);
  await assert.rejects(api.executeTool('wait', {}, { signal: outer.signal }), {
    name: 'AbortError',
  });
});

test('WebMCP bridge is bounded, mirrors replacements and gates consequential tools', async () => {
  const page = realm();
  const api = page.document.modelContext;
  api.registerTool({
    ...definition('risky'),
    annotations: { consequential: true },
    exposedTo: ['https://consumer.test'],
  });
  assert.deepEqual(tools(page.bridge)[0].exposedTo, ['https://consumer.test']);
  assert.equal((await page.bridge.call('risky', {}, false)).__webmcp_error, 'consequential');
  assert.equal(
    (await page.bridge.call('risky', { name: 'Allowed' }, true)).__webmcp_result,
    '{"greeting":"Allowed"}'
  );
  assert.equal((await page.bridge.call('missing', {}, true)).__webmcp_error, 'unknown');
  api.registerTool(definition('large', () => 'x'.repeat(100000)));
  assert.equal((await page.bridge.call('large', {}, true)).__webmcp_result.length, 65536);
  api.registerTool(definition('fails', () => Promise.reject(new Error('failed'))));
  assert.equal((await page.bridge.call('fails', {}, true)).__webmcp_error, 'threw');
  page.document.modelContext = {
    getTools: api.getTools,
    registerTool: api.registerTool,
    executeTool: api.executeTool,
  };
  page.document.modelContext.registerTool(definition('mirrored'));
  assert.equal(page.bridge.list().replaced, true);
  assert.ok(tools(page.bridge).some((tool) => tool.name === 'mirrored'));
  const huge = realm();
  huge.document.modelContext.registerTool({ ...definition(), description: 'x'.repeat(100000) });
  assert.equal(huge.bridge.list().error, 'limit');
});

test('WebMCP page-facing aggregation is same-origin only', async () => {
  const parent = realm();
  const child = realm();
  child.document.modelContext.registerTool(definition('child'));
  parent.window.frames.push(child.window, {
    get location() {
      throw new DOMException('cross origin', 'SecurityError');
    },
  });
  const list = await parent.document.modelContext.getTools();
  assert.equal(list.length, 1);
  assert.equal(list[0].name, 'child');
  assert.equal(
    await parent.document.modelContext.executeTool(list[0], { name: 'Child' }),
    '{"greeting":"Child"}'
  );
  await assert.rejects(
    parent.document.modelContext.executeTool(
      { window: { location: { origin: 'https://other.test' } } },
      {}
    ),
    { name: 'SecurityError' }
  );
  assert.equal(tools(parent.bridge).length, 0);
});

test('WebMCP bridge captures collection intrinsics and never enumerates its lexical identifier', () => {
  const page = realm();
  page.document.modelContext.registerTool(definition());
  vm.runInContext(
    'Map.prototype.get=()=>{throw Error("patched")};Array.prototype.sort=()=>{throw Error("patched")};JSON.stringify=()=>{throw Error("patched")}',
    page.context
  );
  assert.equal(tools(page.bridge)[0].name, 'greet');
  const privateRealm = realm({ secure: false });
  vm.runInContext(
    `const __wmcp_0123456789abcdef0123456789abcdef = ${source}`,
    privateRealm.context
  );
  assert.equal(
    vm.runInContext(
      'Object.getOwnPropertyNames(globalThis).includes("__wmcp_0123456789abcdef0123456789abcdef")',
      privateRealm.context
    ),
    false
  );
});
