import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const shape of ['ordinary', 'same-loader', 'opaque', 'error']) {
  test(`guard verifies document replacement before retiring prior frames (${shape})`, async () => {
    const initial = {
      id: 'PAGE',
      loaderId: 'INITIAL',
      url: 'https://ordinary.test/',
      securityOrigin: 'https://ordinary.test',
    };
    const old = { ...initial, loaderId: 'OLD', url: 'https://ordinary.test/previous' };
    const current = {
      ...initial,
      loaderId: shape === 'same-loader' ? 'OLD' : 'CURRENT',
      url: 'https://ordinary.test/current',
    };
    const child = {
      id: 'CHILD',
      parentId: 'PAGE',
      loaderId: 'LC',
      url: shape === 'error' ? 'chrome-error://chromewebdata/' : 'https://ordinary.test/child',
      securityOrigin: ['opaque', 'error'].includes(shape) ? '://' : 'https://ordinary.test',
      ...(shape === 'error' ? { unreachableUrl: 'https://private.test/controller/fixture' } : {}),
    };
    const page = { targetId: 'PAGE', type: 'page', url: initial.url, frame: initial };
    let guardSession;
    let phase = 'idle';
    const event = (sessionId, frame) => ({
      method: 'Page.frameNavigated',
      sessionId,
      params: { frame },
    });
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
        if (message.method === 'Page.getNavigationHistory' && phase === 'old') {
          page.frame = old;
          page.url = old.url;
          page.childFrames = [{ frame: child }];
          send(event(guardSession, old));
          send(event(message.sessionId, old));
          send(event(guardSession, child));
          send(event(message.sessionId, child));
          phase = 'idle';
        } else if (message.method === 'Page.getNavigationHistory' && phase === 'current') {
          send(event(message.sessionId, current));
          phase = 'idle';
        }
        if (message.method === 'Runtime.evaluate') {
          send({
            id: message.id,
            sessionId: message.sessionId,
            result: { result: { type: 'string', value: 'ordinary' } },
          });
          return true;
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
      await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
      phase = 'old';
      await client.send('Page.getNavigationHistory', {}, sessionId);
      page.frame = current;
      page.url = current.url;
      page.childFrames =
        shape === 'ordinary'
          ? [
              {
                frame: {
                  id: 'CURRENT_CHILD',
                  parentId: 'PAGE',
                  loaderId: 'NC',
                  url: 'https://ordinary.test/child',
                  securityOrigin: 'https://ordinary.test',
                },
              },
            ]
          : [];
      phase = 'current';
      await client.send('Page.getNavigationHistory', {}, sessionId);
      const from = fake.calls.length;
      const result = client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
      if (shape === 'ordinary') assert.equal((await result).result.value, 'ordinary');
      else await assert.rejects(result, /slicc: refused:/);
      assert.equal(
        fake.calls.slice(from).filter((call) => call.method === 'Runtime.evaluate').length,
        shape === 'ordinary' ? 1 : 0
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}

for (const kind of ['Navigation', 'BackForwardCacheRestore']) {
  for (const copy of ['later', 'absent']) {
    test(`guard accepts a verified newer session document (${kind}, ${copy})`, async () => {
      const initial = {
        id: 'PAGE',
        loaderId: 'INITIAL',
        url: 'https://ordinary.test/',
        securityOrigin: 'https://ordinary.test',
      };
      const old = { ...initial, loaderId: 'OLD', url: 'https://ordinary.test/previous' };
      const current = { ...initial, loaderId: 'CURRENT', url: 'https://ordinary.test/current' };
      const page = { targetId: 'PAGE', type: 'page', url: initial.url, frame: initial };
      let guardSession;
      let writer;
      let phase = 'idle';
      const event = (sessionId, frame, type = 'Navigation') => ({
        method: 'Page.frameNavigated',
        sessionId,
        params: { type, frame },
      });
      const fake = await fakeCdp({
        intercept(message, send) {
          writer = send;
          if (message.method === 'Page.getFrameTree') guardSession = message.sessionId;
          if (message.method === 'Page.getNavigationHistory' && phase === 'old') {
            send(event(guardSession, old));
            send(event(message.sessionId, old));
            phase = 'idle';
          } else if (message.method === 'Page.getNavigationHistory' && phase === 'current') {
            send(event(message.sessionId, current, kind));
            phase = 'idle';
          }
          if (message.method === 'Runtime.evaluate') {
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { result: { type: 'string', value: 'ordinary' } },
            });
            return true;
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
        await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
        phase = 'old';
        await client.send('Page.getNavigationHistory', {}, sessionId);
        page.frame = current;
        page.url = current.url;
        phase = 'current';
        await client.send('Page.getNavigationHistory', {}, sessionId);
        const from = fake.calls.length;
        const result = await client.send(
          'Runtime.evaluate',
          { expression: 'document.title' },
          sessionId
        );
        assert.equal(result.result.value, 'ordinary');
        assert.equal(
          fake.calls.slice(from).filter((call) => call.method === 'Runtime.evaluate').length,
          1
        );
        if (copy === 'later') {
          writer(event(guardSession, current, kind));
          await client.send('Page.getNavigationHistory', {}, sessionId);
          await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
        }
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}
