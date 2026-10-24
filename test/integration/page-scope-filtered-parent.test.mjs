import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const frame = (id, origin, parentId) => ({
  id,
  loaderId: `L${id}`,
  url: `${origin}/`,
  securityOrigin: origin,
  ...(parentId ? { parentId } : {}),
});

for (const parent of ['FILTERED', 'USER']) {
  test(`page scope filtered-parent inventory ${parent === 'USER' ? 'related' : 'unrelated'}`, async () => {
    const fake = await fakeCdp();
    fake.targets.push({ targetId: 'USER', type: 'page', url: 'https://ordinary.test/' });
    fake.targets.push({
      targetId: 'CHILD',
      type: 'iframe',
      parentFrameId: parent,
      url: 'https://ordinary.test/frame',
      tree: { frame: frame('CHILD', 'https://ordinary.test', parent) },
    });
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
      const from = fake.calls.length;
      await client.send('DOM.describeNode', { backendNodeId: 1 }, sessionId);
      await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'A' }, sessionId);
      if (parent === 'FILTERED')
        assert.equal(
          fake.calls
            .slice(from)
            .some(
              (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'CHILD'
            ),
          false
        );
      const related = fake.targets.find((target) => target.targetId === 'CHILD');
      related.url = 'https://private.test/';
      related.tree.frame.url = related.url;
      related.tree.frame.securityOrigin = 'https://private.test';
      if (parent === 'USER')
        await assert.rejects(
          client.send('DOM.describeNode', { backendNodeId: 1 }, sessionId),
          /slicc: refused:/
        );
      else await client.send('DOM.describeNode', { backendNodeId: 1 }, sessionId);
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
