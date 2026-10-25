import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const locked of [false, true]) {
  for (const [method, params, limit] of [
    ['Runtime.evaluate', { expression: 'document.title', returnByValue: true }, 1],
    ['Runtime.getHeapUsage', {}, 2],
  ]) {
    test(`Runtime selected document uses fresh page evidence ${method} ${locked ? 'refused' : 'ordinary'}`, async () => {
      let active = false;
      let trees = 0;
      const fake = await fakeCdp({
        childFrames: [
          {
            frame: {
              id: 'CHILD',
              parentId: 'USER',
              loaderId: 'LC',
              url: locked ? 'https://private.test/child' : 'https://ordinary.test/child',
              securityOrigin: locked ? 'https://private.test' : 'https://ordinary.test',
            },
          },
        ],
        intercept(message, send) {
          if (!active || message.method !== 'Page.getFrameTree' || ++trees <= limit) return false;
          send({
            id: message.id,
            sessionId: message.sessionId,
            error: { code: -32000, message: 'duplicate renderer metadata query is unavailable' },
          });
          return true;
        },
      });
      fake.targets.push({ targetId: 'USER', type: 'page', url: 'https://ordinary.test/' });
      const client = await guarded(fake.url, 'https://private.test', {
        variant: 'prompt',
        detachDelay: 0,
      });
      try {
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId: 'USER',
          flatten: true,
        });
        active = true;
        const from = fake.calls.length;
        if (locked) {
          await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
          assert.equal(
            fake.calls.slice(from).some((call) => call.method === method),
            false
          );
        } else {
          await client.send(method, params, sessionId);
          assert.equal(trees, limit);
        }
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}
