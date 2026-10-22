import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const ordinary = 'https://example.test/page';
const failed = 'https://protected.test/failed';

for (const securityOrigin of ['https://protected.test', undefined]) {
  test(`frame reads remain refused after an allowed error-page recovery with ${securityOrigin ? 'protected' : 'unknown'} child metadata`, async () => {
    const childFrames = [
      {
        frame: {
          id: 'CHILD',
          parentId: 'RECOVERY',
          url: 'https://example.test/child',
          securityOrigin,
        },
      },
    ];
    const backend = await fakeCdp({ childFrames });
    backend.targets.push({
      targetId: 'RECOVERY',
      url: failed,
      frame: {
        id: 'RECOVERY',
        url: 'chrome-error://chromewebdata/',
        securityOrigin: '://',
        unreachableUrl: failed,
      },
      history: {
        currentIndex: 1,
        entries: [ordinary, failed].map((url, index) => ({
          id: index + 1,
          url,
          userTypedURL: url,
          title: '',
          transitionType: 'typed',
        })),
      },
    });
    const client = await guarded(backend.url, 'https://protected.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'RECOVERY',
        flatten: true,
      });
      await client.send('Page.reload', {}, sessionId);
      await client.send('Page.navigate', { url: ordinary }, sessionId);
      await client.send('Page.getFrameTree', {}, sessionId).catch(() => {});
      await client.send('Page.getFrameTree', {}, sessionId);
      const before = backend.calls.length;
      await assert.rejects(
        client.send('Accessibility.getFullAXTree', { frameId: 'CHILD' }, sessionId),
        /slicc: refused:/
      );
      await assert.rejects(
        client.send('DOM.getDocument', { pierce: true }, sessionId),
        /slicc: refused:/
      );
      assert.equal(
        backend.calls
          .slice(before)
          .some((call) => ['Accessibility.getFullAXTree', 'DOM.getDocument'].includes(call.method)),
        false
      );
      await client.send('Page.reload', {}, sessionId);
    } finally {
      await client.close();
      await backend.close();
    }
  });
}
