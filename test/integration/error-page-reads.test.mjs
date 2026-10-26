import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const origin = 'https://private.test';
const ordinary = 'https://ordinary.test/page';
const marker = 'error-page-read-marker';
const failed = `${origin}/controller/${marker}?capability=${marker}#${marker}`;
const errorPage = 'chrome-error://chromewebdata/';
const reads = [
  ['Runtime.evaluate', { expression: 'loadTimeData.data_', returnByValue: true }],
  [
    'Runtime.evaluate',
    { expression: 'document.querySelector("#reload-button").dataset.url', returnByValue: true },
  ],
  ['DOM.getDocument', { depth: 1 }],
  ['DOM.getOuterHTML', { backendNodeId: 1 }],
  ['Accessibility.getFullAXTree', {}],
  ['Accessibility.getPartialAXTree', { backendNodeId: 1 }],
  ['Accessibility.getRootAXNode', {}],
  ['Accessibility.getAXNodeAndAncestors', { backendNodeId: 1 }],
  ['Accessibility.getChildAXNodes', { id: 'AX1' }],
  ['Accessibility.queryAXTree', { backendNodeId: 1 }],
  ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
  ['DOMSnapshot.getSnapshot', { computedStyleWhitelist: [] }],
  ['Page.captureScreenshot', {}],
];
const readMethods = new Set(reads.map(([method]) => method));
const entry = (id, url) => ({ id, url, userTypedURL: '', title: '', transitionType: 'link' });

function errorFrame(evidence) {
  return {
    id: 'PAGE',
    loaderId: 'ERROR',
    url: errorPage,
    securityOrigin: '://',
    ...(evidence === 'protected' ? { unreachableUrl: failed } : {}),
    ...(evidence === 'malformed' ? { unreachableUrl: 'not a url' } : {}),
  };
}

function commit(tab, evidence, sessions, send, route, delivery) {
  tab.url = evidence === 'protected' && delivery === 'observed' ? failed : errorPage;
  tab.frame = errorFrame(evidence);
  const current = entry(1, evidence === 'unknown' ? '' : failed);
  tab.history = {
    currentIndex: route === 'blocked-hop' ? 1 : 0,
    entries:
      route === 'blocked-hop' ? [entry(2, ordinary), current] : [current, entry(2, ordinary)],
  };
  if (delivery === 'silent') return;
  send({
    method: 'Target.targetInfoChanged',
    params: { targetInfo: { targetId: 'PAGE', type: 'page', url: tab.url, title: tab.url } },
  });
  for (const sessionId of sessions) {
    send({ method: 'Runtime.executionContextsCleared', sessionId, params: {} });
    send({
      method: 'Page.frameNavigated',
      sessionId,
      params: {
        type: route === 'blocked-hop' ? 'Navigation' : 'BackForwardCacheRestore',
        frame: tab.frame,
      },
    });
  }
}

async function refusedReads(client, fake, sessionId, output) {
  const from = fake.calls.length;
  for (const [method, params] of reads)
    await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/, method);
  assert.deepEqual(
    fake.calls.slice(from).filter((call) => readMethods.has(call.method)),
    [],
    'no content read reaches the error document'
  );
  const { targetInfos } = await client.send('Target.getTargets');
  const info = targetInfos.find((info) => info.targetId === 'PAGE');
  assert.ok(info, 'the error target remains listed');
  if (fake.targets[0].url === failed) assert.equal(info.url, origin);
  const text = JSON.stringify(output);
  assert.equal(text.includes(marker), false, 'the failed URL marker is absent');
  assert.equal(text.includes(Buffer.from(marker).toString('base64')), false);
}

async function scenario(transport, route, evidence, delivery) {
  const output = [];
  const tab = { targetId: 'PAGE', type: 'page', url: ordinary };
  let sessionId;
  let guardSession;
  let navigation;
  let armed = false;
  const fake = await fakeCdp({
    intercept(message, send) {
      if (armed && route === 'blocked-hop' && message.method === 'Page.navigate') {
        navigation = message;
        send({
          method: 'Fetch.requestPaused',
          sessionId: guardSession,
          params: {
            requestId: 'BLOCKED-HOP',
            frameId: 'PAGE',
            resourceType: 'Document',
            request: { url: failed, method: 'GET', headers: {} },
          },
        });
        return true;
      }
      if (message.method === 'Fetch.failRequest' && message.params.requestId === 'BLOCKED-HOP') {
        armed = false;
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        commit(tab, evidence, [guardSession, sessionId], send, route, delivery);
        send({
          id: navigation.id,
          sessionId: navigation.sessionId,
          result: { frameId: 'PAGE', loaderId: 'ERROR', errorText: 'net::ERR_BLOCKED_BY_CLIENT' },
        });
        return true;
      }
      if (
        armed &&
        route === 'history-back' &&
        message.method === 'Runtime.evaluate' &&
        message.params.expression === 'history.back()'
      ) {
        armed = false;
        commit(tab, evidence, [guardSession, sessionId], send, route, delivery);
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: { result: { type: 'undefined' } },
        });
        return true;
      }
      if (tab.frame?.loaderId === 'ERROR' && readMethods.has(message.method)) {
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: {
            result: { type: 'string', value: failed },
            outerHTML: `<button data-url="${failed}">${marker}</button>`,
            data: Buffer.from(marker).toString('base64'),
            nodes: [{ nodeId: 'AX1', name: { type: 'string', value: failed } }],
            strings: [failed],
          },
        });
        return true;
      }
      return false;
    },
  });
  fake.targets.push(tab);
  let client;
  try {
    client = await guarded(fake.url, origin, {
      variant: 'prompt',
      detachDelay: 0,
      transport,
      onMessage: (message) => output.push(message),
    });
    ({ sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'PAGE',
      flatten: true,
    }));
    await client.send('Page.enable', {}, sessionId);
    for (const [method, params] of reads) await client.send(method, params, sessionId);
    guardSession = fake.calls.find(
      (call) =>
        call.method === 'Fetch.enable' &&
        call.params.patterns?.some((pattern) => pattern.resourceType === 'Document')
    ).sessionId;
    if (route === 'history-back')
      tab.history = { currentIndex: 1, entries: [entry(1, failed), entry(2, ordinary)] };
    armed = true;
    const transition =
      route === 'blocked-hop'
        ? client.send('Page.navigate', { url: 'https://ordinary.test/redirect' }, sessionId)
        : client.send('Runtime.evaluate', { expression: 'history.back()' }, sessionId);
    await transition.catch((error) => assert.match(error.message, /slicc: refused:/));
    assert.equal(armed, false, 'the transition reached the fake backend');
    assert.equal(tab.frame.url, errorPage);
    if (route === 'blocked-hop')
      assert.ok(
        fake.calls.some(
          (call) => call.method === 'Fetch.failRequest' && call.params.requestId === 'BLOCKED-HOP'
        ),
        'the guard blocked the protected hop'
      );
    await refusedReads(client, fake, sessionId, output);
  } finally {
    await client?.close();
    await fake.close();
  }
}

for (const transport of ['native', 'wasi']) {
  for (const route of ['blocked-hop', 'history-back']) {
    for (const evidence of ['protected', 'history-only', 'unknown', 'malformed']) {
      for (const delivery of ['observed', 'silent']) {
        test(
          `error page reads stay refused (${transport}, ${route}, ${evidence}, ${delivery})`,
          { timeout: 60000 },
          async () => scenario(transport, route, evidence, delivery)
        );
      }
    }
  }
}
