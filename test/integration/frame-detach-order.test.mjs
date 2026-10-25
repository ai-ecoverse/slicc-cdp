import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const shape of ['ordinary', 'opaque', 'error', 'reattached', 'present', 'root-change']) {
  for (const source of ['client', 'guard']) {
    test(`guard verifies frame removal before a late session copy (${source}, ${shape})`, async () => {
      const child = {
        id: 'CHILD',
        parentId: 'PAGE',
        loaderId: 'LC',
        url: 'https://ordinary.test/child',
        securityOrigin: 'https://ordinary.test',
      };
      const frame = {
        id: 'PAGE',
        loaderId: 'ROOT',
        url: 'https://ordinary.test/',
        securityOrigin: 'https://ordinary.test',
      };
      const page = {
        targetId: 'PAGE',
        type: 'page',
        url: frame.url,
        frame,
        childFrames: [{ frame: child }],
      };
      let guardSession;
      let armed = false;
      const fake = await fakeCdp({
        intercept(message, send) {
          if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
          if (armed && message.method === 'Page.getNavigationHistory') {
            armed = false;
            page.childFrames = [];
            const detached = source === 'client' ? message.sessionId : guardSession;
            const delayed = source === 'client' ? guardSession : message.sessionId;
            send({
              method: 'Page.frameDetached',
              sessionId: detached,
              params: { frameId: 'CHILD', reason: 'remove' },
            });
            if (shape === 'reattached')
              send({
                method: 'Page.frameAttached',
                sessionId: message.sessionId,
                params: { frameId: 'CHILD', parentFrameId: 'PAGE' },
              });
            if (shape === 'root-change') {
              page.frame = { ...frame, loaderId: 'NEW_ROOT' };
              send({
                method: 'Page.frameNavigated',
                sessionId: message.sessionId,
                params: { frame: page.frame },
              });
            }
            if (shape === 'present') page.childFrames = [{ frame: child }];
            const late =
              shape === 'error'
                ? {
                    ...child,
                    url: 'chrome-error://chromewebdata/',
                    securityOrigin: '://',
                    unreachableUrl: 'https://private.test/controller/fixture',
                  }
                : shape === 'opaque'
                  ? { ...child, securityOrigin: '://' }
                  : shape === 'present'
                    ? { ...child, loaderId: 'NEW_CHILD' }
                    : child;
            send({ method: 'Page.frameNavigated', sessionId: delayed, params: { frame: late } });
          }
          return false;
        },
      });
      fake.targets.push(page);
      const client = await guarded(fake.url, 'https://private.test', {
        variant: 'prompt',
        detachDelay: 0,
      });
      try {
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId: 'PAGE',
          flatten: true,
        });
        await client.send('DOM.getDocument', {}, sessionId);
        armed = true;
        await client.send('Page.getNavigationHistory', {}, sessionId);
        const from = fake.calls.length;
        if (shape === 'ordinary') {
          await client.send('DOM.getDocument', {}, sessionId);
          await client.send('DOM.getDocument', {}, sessionId);
        } else {
          await assert.rejects(client.send('DOM.getDocument', {}, sessionId), /slicc: refused:/);
          assert.equal(
            fake.calls.slice(from).some((call) => call.method === 'DOM.getDocument'),
            false
          );
        }
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}
