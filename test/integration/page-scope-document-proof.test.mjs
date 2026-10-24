import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const own of [false, true]) {
  test(`page scope keeps a fresh document proof with ${own ? 'own protected' : 'unrelated filtered'} ancestry`, async () => {
    let active = false;
    let trees = 0;
    const fake = await fakeCdp({
      intercept(message, send) {
        if (!active) return false;
        if (message.method === 'Target.getTargets') trees = 0;
        if (message.method !== 'Page.getFrameTree' || ++trees === 1) return false;
        send({
          id: message.id,
          sessionId: message.sessionId,
          error: { code: -32000, message: 'renderer metadata is already busy' },
        });
        return true;
      },
    });
    fake.targets.push({ targetId: 'USER', type: 'page', url: 'https://ordinary.test/' });
    fake.targets.push({
      targetId: 'CHILD',
      type: 'iframe',
      parentFrameId: own ? 'USER' : 'FILTERED',
      url: 'https://private.test/',
    });
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      active = true;
      const from = fake.calls.length;
      for (const [method, params] of [
        ['DOM.describeNode', { backendNodeId: 1 }],
        ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: 10, y: 20 }],
      ]) {
        if (own) await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
        else await client.send(method, params, sessionId);
      }
      assert.equal(
        fake.calls.slice(from).some((call) => call.method === 'Input.dispatchMouseEvent'),
        !own
      );
      assert.equal(
        fake.calls
          .slice(from)
          .some(
            (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'CHILD'
          ),
        false
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
