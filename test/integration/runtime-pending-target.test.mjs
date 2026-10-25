import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const protectedFrame of [false, true]) {
  test(`Runtime pending page inventory uses guarded renderer evidence ${protectedFrame ? 'refused' : 'ordinary'}`, async () => {
    const fake = await fakeCdp();
    const frame = {
      id: 'USER',
      loaderId: 'L',
      url: 'https://ordinary.test/',
      securityOrigin: 'https://ordinary.test',
    };
    const page = { targetId: 'USER', type: 'page', url: frame.url, frame };
    fake.targets.push(page);
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      page.url = '';
      if (protectedFrame) {
        frame.url = 'https://private.test/';
        frame.securityOrigin = 'https://private.test';
      }
      const from = fake.calls.length;
      const methods = [
        ['Runtime.evaluate', { expression: 'document.title', returnByValue: true }],
        ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
        ['Input.dispatchMouseEvent', { type: 'mousePressed', x: 20, y: 20, button: 'left' }],
      ];
      for (const [method, params] of methods) {
        if (protectedFrame)
          await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
        else await client.send(method, params, sessionId);
      }
      assert.equal(
        fake.calls.slice(from).some((call) => call.method === 'Runtime.evaluate'),
        !protectedFrame
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
