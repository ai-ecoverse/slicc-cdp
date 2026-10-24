import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const shape of ['blank-url', 'unverified-origin', 'missing-parent', 'unknown-parent']) {
  test(`page scope refuses its own ${shape} child`, async () => {
    const child = {
      id: 'OWN',
      parentId: 'USER',
      loaderId: 'LO',
      url: 'https://ordinary.test/child',
      securityOrigin: 'https://ordinary.test',
    };
    const fake = await fakeCdp();
    fake.targets.push({
      targetId: 'USER',
      type: 'page',
      url: 'https://ordinary.test/',
      childFrames: [{ frame: child }],
    });
    const remote = {
      targetId: 'OWN',
      type: 'iframe',
      parentFrameId: 'USER',
      url: child.url,
      tree: { frame: { ...child } },
    };
    if (shape === 'blank-url') remote.url = '';
    if (shape === 'unverified-origin') remote.tree.frame.securityOrigin = '';
    if (shape === 'missing-parent') delete remote.parentFrameId;
    if (shape === 'unknown-parent') remote.parentFrameId = 'ABSENT';
    fake.targets.push(remote);
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      const from = fake.calls.length;
      await assert.rejects(
        client.send('DOM.describeNode', { backendNodeId: 1 }, sessionId),
        /slicc: refused:/
      );
      assert.equal(
        fake.calls.slice(from).some((call) => call.method === 'DOM.describeNode'),
        false
      );
      if (shape === 'blank-url')
        assert.equal(
          fake.calls
            .slice(from)
            .some(
              (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'OWN'
            ),
          false
        );
      assert.equal(
        fake.calls
          .slice(from)
          .some(
            (call) =>
              call.target === 'OWN' &&
              ['Runtime.evaluate', 'DOM.describeNode'].includes(call.method)
          ),
        false
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
