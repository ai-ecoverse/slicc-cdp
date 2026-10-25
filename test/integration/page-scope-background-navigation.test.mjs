import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const surfaces = [
  ['Page.captureScreenshot', {}],
  ['DOM.getDocument', {}],
  ['Runtime.evaluate', { expression: 'document.title' }],
  ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: 1, y: 1 }],
];

for (const [method, params] of surfaces) {
  for (const destination of [
    'https://seven.sliccy.ai/',
    'https://vault.sliccy.ai/',
    'https://protected.test/',
  ]) {
    for (const stage of ['proof', 'reply']) {
      for (const parent of ['FILTERED_FRAME', 'USER']) {
        test(`background frame URL change during ${method} ${stage}, ${parent}, ${destination}`, async () => {
          const child = {
            targetId: 'CHILD',
            type: 'iframe',
            parentFrameId: parent,
            url: 'https://ordinary.test/child',
            tree: {
              frame: {
                id: 'CHILD',
                parentId: parent,
                loaderId: 'LC',
                url: 'https://ordinary.test/child',
                securityOrigin: 'https://ordinary.test',
              },
            },
          };
          let active = false;
          let changed = false;
          const fake = await fakeCdp({
            discoverEvents: true,
            intercept(message, send) {
              if (
                !active ||
                changed ||
                message.method !== (stage === 'proof' ? 'Page.getFrameTree' : method)
              )
                return false;
              changed = true;
              child.url = destination;
              child.tree.frame.url = child.url;
              child.tree.frame.securityOrigin = new URL(destination).origin;
              send({
                method: 'Target.targetInfoChanged',
                params: { targetInfo: { ...child, tree: undefined, browserContextId: 'CTX' } },
              });
              return false;
            },
          });
          fake.targets.push(
            { targetId: 'USER', type: 'page', url: 'https://ordinary.test/' },
            child
          );
          const client = await guarded(fake.url, 'https://protected.test', {
            variant: 'prompt',
            detachDelay: 0,
          });
          try {
            const { sessionId } = await client.send('Target.attachToTarget', {
              targetId: 'USER',
              flatten: true,
            });
            await client.send('Page.captureScreenshot', {}, sessionId);
            active = true;
            const result = client.send(method, params, sessionId);
            if (parent === 'USER') await assert.rejects(result, /slicc: refused:/);
            else await result;
            assert.equal(changed, true);
          } finally {
            await client.close();
            await fake.close();
          }
        });
      }
    }
  }
}
