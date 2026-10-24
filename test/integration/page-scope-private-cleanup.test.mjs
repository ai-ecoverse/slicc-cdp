import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

test('page scope retires private guard and renderer sessions before closing', async () => {
  const guards = new Set();
  const renderers = new Set();
  const fake = await fakeCdp({
    onCommand(message) {
      if (message.method === 'Fetch.enable') guards.add(message.sessionId);
      if (message.method === 'Page.enable') renderers.add(message.sessionId);
    },
  });
  fake.targets.push({ targetId: 'USER', type: 'page', url: 'https://ordinary.test/' });
  fake.targets.push({
    targetId: 'CHILD',
    type: 'iframe',
    parentFrameId: 'USER',
    url: 'https://ordinary.test/child',
    tree: {
      frame: {
        id: 'CHILD',
        parentId: 'USER',
        loaderId: 'LC',
        url: 'https://ordinary.test/child',
        securityOrigin: 'https://ordinary.test',
      },
    },
  });
  const client = await guarded(fake.url, 'https://private.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  let closed = false;
  try {
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'USER',
      flatten: true,
    });
    await client.send('DOM.describeNode', { backendNodeId: 1 }, sessionId);
    await client.send('Target.detachFromTarget', { sessionId });
    await client.close();
    closed = true;
    assert.equal(guards.size, 1);
    assert.equal(renderers.size, 2);
    for (const guard of guards)
      assert.ok(
        fake.calls.some((call) => call.method === 'Fetch.disable' && call.sessionId === guard),
        'the private guard must stop interception before closing'
      );
    for (const renderer of renderers)
      assert.ok(
        fake.calls.some(
          (call) => call.method === 'Target.detachFromTarget' && call.params.sessionId === renderer
        ),
        'the private renderer attachment must be explicitly retired'
      );
  } finally {
    if (!closed) await client.close();
    await fake.close();
  }
});
