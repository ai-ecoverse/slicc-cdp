import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const layout of ['unowned', 'owned', 'protected-child']) {
  const owned = layout !== 'unowned';
  const allowed = layout === 'owned';
  test(`Runtime ordinary fulfilled response preserves ${layout} opaque context admission`, async () => {
    const emitters = new Map();
    let fulfilled = false;
    let fake;
    const context = (sessionId, targetId) => ({
      method: 'Runtime.executionContextCreated',
      sessionId,
      params: {
        context: {
          id: 1,
          uniqueId: `${fulfilled ? 'fulfilled' : 'initial'}:${targetId}:L`,
          origin: fulfilled ? 'null' : '',
          name: '',
          auxData: { frameId: targetId, isDefault: true },
        },
      },
    });
    fake = await fakeCdp({
      intercept(message, send) {
        const target = fake.targets[0];
        if (!target) return false;
        if (message.method === 'Fetch.fulfillRequest') {
          fulfilled = true;
          if (layout === 'protected-child')
            target.childFrames = [
              {
                frame: {
                  id: 'CHILD',
                  parentId: target.targetId,
                  loaderId: 'LC',
                  url: 'https://private.test/child',
                  securityOrigin: 'https://private.test',
                },
              },
            ];
          for (const [sessionId, emit] of emitters) {
            emit({ method: 'Runtime.executionContextsCleared', sessionId, params: {} });
            emit(context(sessionId, target.targetId));
          }
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          return true;
        }
        if (message.method !== 'Runtime.enable') return false;
        emitters.set(message.sessionId, send);
        send(context(message.sessionId, target.targetId));
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        return true;
      },
    });
    if (!owned) fake.targets.push({ targetId: 'USER', type: 'page', url: 'about:blank' });
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const targetId = owned
        ? (await client.send('Target.createTarget', { url: 'about:blank' })).targetId
        : 'USER';
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId,
        flatten: true,
      });
      await client.send('Runtime.enable', {}, sessionId);
      if (owned)
        await client.send(
          'Runtime.evaluate',
          { expression: 'document.title', returnByValue: true },
          sessionId
        );
      await client.send(
        'Fetch.enable',
        { patterns: [{ urlPattern: '*', requestStage: 'Response' }] },
        sessionId
      );
      emitters.get(sessionId)({
        method: 'Fetch.requestPaused',
        sessionId,
        params: {
          requestId: 'AUTH',
          frameId: targetId,
          resourceType: 'Document',
          request: { url: 'https://ordinary.test/authorize', method: 'GET', headers: {} },
          responseStatusCode: 302,
          responseHeaders: [],
        },
      });
      await client.send('Page.getNavigationHistory', {}, sessionId);
      await client.send(
        'Fetch.fulfillRequest',
        {
          requestId: 'AUTH',
          responseCode: 200,
          responseHeaders: [{ name: 'Content-Type', value: 'text/html' }],
          body: Buffer.from('<title>Signed in</title>').toString('base64'),
        },
        sessionId
      );
      const from = fake.calls.length;
      const evaluate = client.send(
        'Runtime.evaluate',
        { expression: 'document.title', returnByValue: true },
        sessionId
      );
      if (allowed) await evaluate;
      else await assert.rejects(evaluate, /slicc: refused:/);
      assert.equal(
        fake.calls.slice(from).some((call) => call.method === 'Runtime.evaluate'),
        allowed
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
