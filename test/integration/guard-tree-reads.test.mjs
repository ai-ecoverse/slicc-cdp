import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const reads = [
  ['DOM.getDocument', { pierce: true }],
  ['DOM.getFlattenedDocument', {}],
  ['DOM.describeNode', { nodeId: 1, pierce: true }],
  ['DOM.requestChildNodes', { nodeId: 1, pierce: true }],
  ['DOM.getDocument', { depth: -1 }],
  ['DOM.describeNode', { nodeId: 1, depth: 2 }],
  ['DOM.requestChildNodes', { nodeId: 1, depth: -1 }],
  ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
  ['DOMSnapshot.getSnapshot', { computedStyleWhitelist: [] }],
];

function child(url = 'https://example.test/child', securityOrigin = 'https://example.test') {
  return { frame: { id: 'CHILD', parentId: 'TAB1', url, securityOrigin } };
}

async function fixture(options, run) {
  const fake = await fakeCdp(options);
  const client = await guarded(fake.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { targetId } = await client.send('Target.createTarget', { url: 'https://example.test/' });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    await run(client, sessionId, fake.calls);
  } finally {
    await client.close();
    await fake.close();
  }
}

test('cross-frame reads fail closed for protected or unverified frame trees', async () => {
  for (const childFrames of [
    [child('https://protected.test/child', 'https://protected.test')],
    [child('about:blank', 'https://protected.test')],
    [{ ...child(), childFrames: [child('about:srcdoc', 'https://protected.test')] }],
    [child('', 'https://example.test')],
    [child('https://example.test/child', '')],
    [child('https://example.test/child', 'null')],
    [{ frame: { id: 'UNKNOWN' } }],
    [{ frame: { url: 'https://example.test/child', securityOrigin: 'https://example.test' } }],
    [{ ...child(), childFrames: {} }],
  ]) {
    await fixture({ childFrames }, async (client, sessionId, calls) => {
      for (const [method, params] of reads) {
        const before = calls.length;
        await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
        assert.equal(
          calls.slice(before).some((call) => call.method === method),
          false
        );
      }
      await client.send('DOM.describeNode', { nodeId: 1, depth: 0 }, sessionId);
      await client.send('DOM.getDocument', { depth: 1, pierce: false }, sessionId);
    });
  }
});

test('cross-frame reads fail closed when frame discovery fails or no session is given', async () => {
  await fixture({ refuse: ['Page.getFrameTree'] }, async (client, sessionId, calls) => {
    for (const [method, params] of reads) {
      await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
      await assert.rejects(client.send(method, params), /slicc: refused:/);
      assert.equal(
        calls.some((call) => call.method === method),
        false
      );
    }
  });
});

test('a frame tree change during a read prevents returning the reply', async () => {
  const childFrames = [];
  await fixture({ childFrames }, async (client, sessionId, calls) => {
    const push = calls.push.bind(calls);
    calls.push = (call) => {
      if (call.method === 'DOM.getDocument') {
        childFrames.push(child('https://protected.test/child', 'https://protected.test'));
      }
      return push(call);
    };
    await assert.rejects(
      client.send('DOM.getDocument', { pierce: true }, sessionId),
      /slicc: refused:/
    );
    assert.equal(calls.filter((call) => call.method === 'DOM.getDocument').length, 1);
  });
});

test('verified ordinary frame trees remain readable', async () => {
  for (const childFrames of [[], [child()], [{ ...child(), childFrames: [child()] }]]) {
    await fixture({ childFrames }, async (client, sessionId, calls) => {
      for (const [method, params] of reads) {
        const before = calls.length;
        await client.send(method, params, sessionId);
        assert.equal(calls.slice(before).filter((call) => call.method === method).length, 1);
      }
    });
  }
});
