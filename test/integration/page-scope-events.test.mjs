import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const protectedFrame of [false, true]) {
  test(`page scope content events ${protectedFrame ? 'protected' : 'ordinary'}`, async () => {
    const childFrames = [];
    const fake = await fakeCdp({
      childFrames,
      onCommand(message, event) {
        if (message.method !== 'Runtime.evaluate' || message.params.expression !== '0') return;
        const origin = protectedFrame ? 'https://vault.sliccy.ai' : 'https://example.test';
        childFrames.push({
          frame: {
            id: 'CHILD',
            parentId: 'TAB1',
            loaderId: 'LC',
            url: `${origin}/`,
            securityOrigin: origin,
          },
        });
        event('DOM.documentUpdated', {});
        event('DOM.setChildNodes', {
          parentId: 1,
          nodes: [{ nodeId: 2, nodeName: '#text', nodeValue: 'frame marker' }],
        });
      },
    });
    const client = await guarded(fake.url, 'https://protected.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { targetId } = await client.send('Target.createTarget', {
        url: 'https://example.test/',
      });
      const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
      const from = client.events.length;
      await client.send('Runtime.evaluate', { expression: '0' }, sessionId);
      assert.equal(
        client.events.slice(from).filter((event) => event.method === 'DOM.documentUpdated').length,
        1
      );
      assert.equal(
        client.events.slice(from).filter((event) => event.method === 'DOM.setChildNodes').length,
        protectedFrame ? 0 : 1
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
