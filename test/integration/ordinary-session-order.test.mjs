import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const delivery of ['later', 'absent']) {
  test(`guard verifies page reads after a frame is removed (${delivery})`, async () => {
    const child = {
      id: 'CHILD',
      parentId: 'PAGE',
      loaderId: 'LC',
      url: 'https://ordinary.test/child',
      securityOrigin: 'https://ordinary.test',
    };
    const page = {
      targetId: 'PAGE',
      type: 'page',
      url: 'https://ordinary.test/',
      childFrames: [{ frame: child }],
    };
    let guardSession;
    let writer;
    let armed = false;
    const fake = await fakeCdp({
      intercept(message, send) {
        writer = send;
        if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
        if (armed && message.method === 'Page.getNavigationHistory') {
          armed = false;
          send({
            method: 'Page.frameNavigated',
            sessionId: guardSession,
            params: { frame: child },
          });
          page.childFrames = [];
          send({
            method: 'Page.frameDetached',
            sessionId: message.sessionId,
            params: { frameId: 'CHILD', reason: 'remove' },
          });
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
      await client.send('DOM.getDocument', {}, sessionId);
      if (delivery === 'later')
        writer({
          method: 'Page.frameDetached',
          sessionId: guardSession,
          params: { frameId: 'CHILD', reason: 'remove' },
        });
      await client.send('DOM.getDocument', {}, sessionId);
    } finally {
      await client.close();
      await fake.close();
    }
  });
}

for (const source of ['client', 'guard']) {
  test(`guard requires current session document evidence (${source})`, async () => {
    const frame = {
      id: 'PAGE',
      loaderId: 'OLD',
      url: 'https://ordinary.test/',
      securityOrigin: 'https://ordinary.test',
    };
    const page = { targetId: 'PAGE', type: 'page', url: frame.url, frame };
    const current = { ...frame, loaderId: 'NEW' };
    let guardSession;
    let armed = false;
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
        if (armed && message.method === 'Page.getNavigationHistory') {
          armed = false;
          send({
            method: 'Page.frameNavigated',
            sessionId: source === 'client' ? message.sessionId : guardSession,
            params: { type: 'BackForwardCacheRestore', frame: current },
          });
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
      const from = fake.calls.length;
      armed = true;
      await client.send('Page.getNavigationHistory', {}, sessionId);
      await assert.rejects(client.send('DOM.getDocument', {}, sessionId), /slicc: refused:/);
      assert.equal(
        fake.calls.slice(from).some((call) => call.method === 'DOM.getDocument'),
        false
      );
      page.frame = current;
      await client.send('DOM.getDocument', {}, sessionId);
      assert.equal(
        fake.calls.slice(from).filter((call) => call.method === 'DOM.getDocument').length,
        1
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
