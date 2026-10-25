import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const mode of ['ordinary', 'carrier-only', 'selected-context', 'protected']) {
  test(`Runtime authorize document commit ${mode}`, async () => {
    const url =
      mode === 'carrier-only'
        ? 'http://localhost:8765/authorize?to=https%3A%2F%2Fprivate.test%2Fcb%23access_token%3DT%26state%3DS'
        : 'http://localhost:8765/authorize?to=http%3A%2F%2F127.0.0.1%3A8765%2Fcb%23access_token%3DT%26state%3DS';
    const allowed = mode === 'ordinary' || mode === 'carrier-only';
    const frame = {
      id: 'USER',
      loaderId: 'L1',
      url,
      securityOrigin: 'http://localhost:8765',
    };
    const emitters = new Map();
    let active = false;
    let changed = false;
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.method === 'Runtime.enable') {
          emitters.set(message.sessionId, send);
          send({
            method: 'Runtime.executionContextCreated',
            sessionId: message.sessionId,
            params: {
              context: {
                id: 1,
                uniqueId: `context:${frame.loaderId}`,
                origin: frame.securityOrigin,
                name: '',
                auxData: { frameId: frame.id, isDefault: true },
              },
            },
          });
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          return true;
        }
        if (active && !changed && message.method === 'Runtime.disable') {
          changed = true;
          frame.loaderId = 'L2';
          if (mode === 'protected') {
            frame.url = 'https://private.test/';
            frame.securityOrigin = 'https://private.test';
          }
          for (const [sessionId, emit] of emitters)
            emit({ method: 'Page.frameNavigated', sessionId, params: { frame: { ...frame } } });
        }
        return false;
      },
    });
    fake.targets.push({ targetId: 'USER', type: 'page', url, frame });
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      await client.send('Runtime.enable', {}, sessionId);
      for (const [owner, emit] of emitters)
        if (owner !== sessionId)
          emit({ method: 'Runtime.executionContextsCleared', sessionId: owner, params: {} });
      await client.send('Page.getNavigationHistory', {}, sessionId);
      active = true;
      const from = fake.calls.length;
      const result = client.send(
        'Runtime.evaluate',
        {
          expression: 'document.title',
          returnByValue: true,
          ...(mode === 'selected-context' ? { contextId: 1 } : {}),
        },
        sessionId
      );
      if (allowed) await result;
      else await assert.rejects(result, /slicc: refused:/);
      assert.equal(changed, true);
      assert.equal(
        fake.calls.slice(from).filter((call) => call.method === 'Runtime.evaluate').length,
        allowed ? 1 : 0
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
