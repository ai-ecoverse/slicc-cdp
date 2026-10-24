import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const ordinary of [false, true]) {
  test(`page scope error frame ${ordinary ? 'ordinary' : 'protected'}`, async () => {
    const childFrames = [
      {
        frame: {
          id: 'CHILD',
          parentId: 'P',
          loaderId: 'LC',
          url: 'https://ordinary.test/child',
          securityOrigin: 'https://ordinary.test',
        },
      },
    ];
    const fake = await fakeCdp({ childFrames });
    fake.targets.push({ targetId: 'P', type: 'page', url: 'https://ordinary.test/' });
    const client = await guarded(fake.url, 'https://protected.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'P',
        flatten: true,
      });
      await client.send('DOM.getDocument', { depth: 1 }, sessionId);
      childFrames[0].frame = {
        id: 'CHILD',
        parentId: 'P',
        loaderId: 'LE',
        url: 'chrome-error://chromewebdata/',
        securityOrigin: '://',
        unreachableUrl: ordinary
          ? 'https://ordinary.test/missing'
          : 'https://protected.test/missing',
      };
      if (ordinary) await client.send('DOM.getDocument', { depth: 1 }, sessionId);
      else
        await assert.rejects(
          client.send('DOM.getDocument', { depth: 1 }, sessionId),
          /slicc: refused:/
        );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
