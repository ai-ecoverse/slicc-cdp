import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const CONTEXT_GONE = /Cannot find context|context.*(destroyed|not found)/i;
const OBJECT_GONE = /Cannot find context with specified id|Could not find object with given id/i;
const NODE_GONE = /Could not find node with given id|Document needs to be requested first/i;

function fixture(doc) {
  const server = createServer((req, res) => {
    res.setHeader('Content-Type', 'text/html');
    res.setHeader('Cache-Control', 'no-store');
    res.end(
      `<!doctype html><html><body data-doc="${doc}"><p id="mark">${doc} ${req.url}</p></body></html>`
    );
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

function within(what, ms, watched) {
  let timer;
  return Promise.race([
    watched.seen,
    new Promise((_, reject) => {
      timer = setTimeout(() => {
        watched.cancel();
        reject(new Error(`${what} not seen within ${ms}ms`));
      }, ms);
    }),
  ]).finally(() => clearTimeout(timer));
}

function collect(raw, method, sessionId, into) {
  return raw.event(method, sessionId, (params) => {
    into.push({ method, params });
    return false;
  });
}

async function failure(what, pending, chrome) {
  let value;
  try {
    value = await pending;
  } catch (error) {
    let parsed;
    try {
      parsed = JSON.parse(error.message);
    } catch {
      parsed = { message: error.message };
    }
    return parsed;
  }
  assert.fail(`${what} still succeeded on ${chrome}: ${JSON.stringify(value)}`);
}

test('Chrome: a cross-document commit kills the old execution context, its objects and nodes, while a same-document navigation keeps them', {
  timeout: 180000,
}, async (t) => {
  const serverA = await fixture('a');
  const serverB = await fixture('b');
  const originA = `http://a.test:${serverA.address().port}`;
  const originB = `http://b.test:${serverB.address().port}`;
  let browser;
  let raw;
  const watchers = [];
  try {
    browser = await launchBrowser([
      '--site-per-process',
      '--host-resolver-rules=MAP *.test 127.0.0.1',
      '--no-proxy-server',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const version = await raw.send('Browser.getVersion');
    const chrome = `${version.product} (revision ${version.revision}, protocol ${version.protocolVersion})`;
    t.diagnostic(`chrome: ${chrome}`);
    const { targetId: tab } = await raw.send('Target.createTarget', {
      url: 'about:blank',
      background: true,
    });
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: tab,
      flatten: true,
    });
    const send = (method, params = {}) => raw.send(method, params, sessionId);
    await send('Page.enable');
    const loaded = raw.event('Page.loadEventFired', sessionId);
    const first = await send('Page.navigate', { url: `${originA}/start` });
    assert.equal(first.errorText, undefined, `${chrome}: ${first.errorText}`);
    await within('first load', 30000, loaded);
    const defaultFor = (origin) => (params) =>
      params.context?.auxData?.isDefault === true &&
      params.context.auxData.frameId === tab &&
      params.context.origin === origin;
    const initial = raw.event('Runtime.executionContextCreated', sessionId, defaultFor(originA));
    await send('Runtime.enable');
    const old = (await within('default context on A', 30000, initial)).context;
    t.diagnostic(`old context: id=${old.id} uniqueId=${old.uniqueId} origin=${old.origin}`);
    assert.equal(typeof old.uniqueId, 'string', `${chrome}: ${JSON.stringify(old)}`);
    const lifecycle = [];
    for (const method of [
      'Runtime.executionContextDestroyed',
      'Runtime.executionContextsCleared',
      'Runtime.executionContextCreated',
    ])
      watchers.push(collect(raw, method, sessionId, lifecycle));
    const probe =
      '({origin: location.origin, doc: document.body.dataset.doc, marker: typeof globalThis.contextMarker})';
    const evaluate = async (params) =>
      (await send('Runtime.evaluate', { expression: probe, returnByValue: true, ...params })).result
        .value;
    const made = await send('Runtime.evaluate', {
      expression: 'globalThis.contextMarker = { generation: "old" }',
      contextId: old.id,
    });
    const objectId = made.result.objectId;
    assert.equal(typeof objectId, 'string', `${chrome}: ${JSON.stringify(made)}`);
    const { root } = await send('DOM.getDocument');
    const { nodeId } = await send('DOM.querySelector', { nodeId: root.nodeId, selector: '#mark' });
    assert.ok(nodeId > 0, `${chrome}: no node for #mark`);
    const generation = async () =>
      (
        await send('Runtime.callFunctionOn', {
          objectId,
          functionDeclaration: 'function () { return this.generation; }',
          returnByValue: true,
        })
      ).result.value;
    const alive = { origin: originA, doc: 'a', marker: 'object' };
    const assertAlive = async (when) => {
      assert.deepEqual(
        await evaluate({ contextId: old.id }),
        alive,
        `${when} contextId on ${chrome}`
      );
      assert.deepEqual(
        await evaluate({ uniqueContextId: old.uniqueId }),
        alive,
        `${when} uniqueContextId on ${chrome}`
      );
      assert.equal(await generation(), 'old', `${when} objectId on ${chrome}`);
      const { outerHTML } = await send('DOM.getOuterHTML', { nodeId });
      assert.match(outerHTML, /id="mark"/, `${when} nodeId on ${chrome}`);
      const { node } = await send('DOM.describeNode', { nodeId });
      assert.equal(node.nodeName, 'P', `${when} describeNode on ${chrome}`);
    };
    await assertAlive('before any navigation');
    for (const [label, expression] of [
      ['pushState', 'history.pushState({}, "", "/pushed")'],
      ['fragment', 'location.hash = "fragment"'],
    ]) {
      const moved = raw.event(
        'Page.navigatedWithinDocument',
        sessionId,
        (params) => params.frameId === tab
      );
      await send('Runtime.evaluate', { expression, contextId: old.id });
      const seen = await within(`${label} same-document navigation`, 30000, moved);
      t.diagnostic(`${label}: ${seen.url}`);
      await assertAlive(`after ${label}`);
    }
    assert.deepEqual(
      lifecycle.filter(({ method }) => method !== 'Runtime.executionContextCreated'),
      [],
      `same-document navigation destroyed a context on ${chrome}`
    );
    const navigated = raw.event(
      'Page.frameNavigated',
      sessionId,
      (params) =>
        params.frame?.id === tab && !params.frame.parentId && params.frame.url.startsWith(originB)
    );
    const created = raw.event('Runtime.executionContextCreated', sessionId, defaultFor(originB));
    const commit = await send('Page.navigate', { url: `${originB}/next` });
    assert.equal(commit.errorText, undefined, `${chrome}: ${commit.errorText}`);
    const frame = (await within('frameNavigated to B', 30000, navigated)).frame;
    const fresh = (await within('default context on B', 30000, created)).context;
    t.diagnostic(`frameNavigated: ${frame.url} loaderId=${frame.loaderId}`);
    t.diagnostic(`new context: id=${fresh.id} uniqueId=${fresh.uniqueId} origin=${fresh.origin}`);
    t.diagnostic(
      `lifecycle: ${JSON.stringify(lifecycle.map(({ method, params }) => [method, params.executionContextId ?? params.executionContextUniqueId ?? params.context?.id]))}`
    );
    assert.notEqual(fresh.uniqueId, old.uniqueId, `${chrome}: uniqueId reused`);
    const byId = await failure(
      'evaluate on the old contextId',
      evaluate({ contextId: old.id }),
      chrome
    );
    t.diagnostic(`old contextId: ${JSON.stringify(byId)}`);
    assert.match(byId.message, CONTEXT_GONE, `old contextId on ${chrome}: ${JSON.stringify(byId)}`);
    const byUnique = await failure(
      'evaluate on the old uniqueContextId',
      evaluate({ uniqueContextId: old.uniqueId }),
      chrome
    );
    t.diagnostic(`old uniqueContextId: ${JSON.stringify(byUnique)}`);
    assert.match(
      byUnique.message,
      CONTEXT_GONE,
      `old uniqueContextId on ${chrome}: ${JSON.stringify(byUnique)}`
    );
    const byObject = await failure('callFunctionOn the old objectId', generation(), chrome);
    t.diagnostic(`old objectId: ${JSON.stringify(byObject)}`);
    assert.match(
      byObject.message,
      OBJECT_GONE,
      `old objectId on ${chrome}: ${JSON.stringify(byObject)}`
    );
    const byOuter = await failure(
      'DOM.getOuterHTML on the old nodeId',
      send('DOM.getOuterHTML', { nodeId }),
      chrome
    );
    t.diagnostic(`old nodeId getOuterHTML: ${JSON.stringify(byOuter)}`);
    assert.match(byOuter.message, NODE_GONE, `old nodeId on ${chrome}: ${JSON.stringify(byOuter)}`);
    const byDescribe = await failure(
      'DOM.describeNode on the old nodeId',
      send('DOM.describeNode', { nodeId }),
      chrome
    );
    t.diagnostic(`old nodeId describeNode: ${JSON.stringify(byDescribe)}`);
    assert.match(
      byDescribe.message,
      NODE_GONE,
      `old nodeId on ${chrome}: ${JSON.stringify(byDescribe)}`
    );
    const current = { origin: originB, doc: 'b', marker: 'undefined' };
    assert.deepEqual(await evaluate({}), current, `default context on ${chrome}`);
    assert.deepEqual(
      await evaluate({ uniqueContextId: fresh.uniqueId }),
      current,
      `new uniqueContextId on ${chrome}`
    );
  } finally {
    for (const watcher of watchers) watcher.cancel();
    raw?.close();
    await browser?.close();
    await new Promise((resolve) => serverA.close(resolve));
    await new Promise((resolve) => serverB.close(resolve));
  }
});
