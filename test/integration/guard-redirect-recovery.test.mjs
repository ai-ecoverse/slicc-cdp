import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const delivery of ['settled', 'inflight']) {
  test(`native redirect recovery tolerates matching opaque context metadata when ${delivery}`, async () => {
    const ordinary = 'https://example.test/page';
    const failed = 'https://protected.test/failed';
    let phase = 'initial';
    let guardSession;
    let callerSession;
    const errorFrame = {
      id: 'TAB1',
      url: 'chrome-error://chromewebdata/',
      securityOrigin: '://',
      unreachableUrl: failed,
    };
    const commit = (send) => {
      send({
        method: 'Page.frameNavigated',
        sessionId: guardSession,
        params: { frame: errorFrame },
      });
      send({
        method: 'Runtime.executionContextCreated',
        sessionId: callerSession,
        params: {
          context: { id: 17, origin: '://', auxData: { frameId: 'TAB1', isDefault: true } },
        },
      });
    };
    const backend = await fakeCdp({
      intercept: (message, send) => {
        if (phase === 'redirect' && message.method === 'Network.enable') {
          send({
            method: 'Network.responseReceivedExtraInfo',
            sessionId: callerSession,
            params: { requestId: 'REDIRECT', statusCode: 302, headers: { Location: failed } },
          });
          commit(send);
          phase = delivery === 'inflight' ? 'refresh' : 'settled';
        } else if (phase === 'refresh' && message.method === 'Page.getFrameTree') {
          commit(send);
          phase = 'settled';
        }
      },
    });
    const client = await guarded(backend.url, 'https://protected.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { targetId } = await client.send('Target.createTarget', { url: ordinary });
      const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
      callerSession = sessionId;
      guardSession = backend.calls.find(
        (call) =>
          call.method === 'Fetch.enable' &&
          call.params?.patterns?.some(
            (pattern) => pattern.urlPattern === '*' && !pattern.resourceType
          )
      ).sessionId;
      const target = backend.targets.find((target) => target.targetId === targetId);
      target.url = failed;
      target.frame = errorFrame;
      phase = 'redirect';
      await client.send('Network.enable', {}, sessionId);
      await client.send('Fetch.disable', {}, sessionId);
      await assert.rejects(
        client.send('Runtime.evaluate', { expression: 'document.body.innerHTML' }, sessionId),
        /slicc: refused:/
      );
      await assert.rejects(
        client.send('Accessibility.getFullAXTree', { frameId: targetId }, sessionId),
        /slicc: refused:/
      );
      await client.send('Page.navigate', { url: ordinary }, sessionId);
      await client.send('Page.getFrameTree', {}, sessionId).catch(() => {});
      await client.send('Page.getFrameTree', {}, sessionId);
      await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
    } finally {
      await client.close();
      await backend.close();
    }
  });
}
