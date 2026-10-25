import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const locked of [false, true]) {
  test(`Runtime console burst ${locked ? 'refused' : 'ordinary'}`, async () => {
    let session;
    let burst = false;
    let fake;
    fake = await fakeCdp({
      onCommand(message, event) {
        if (message.method !== 'Page.getLayoutMetrics') return;
        if (locked)
          fake.targets[0].childFrames.push({
            frame: {
              id: 'CHILD',
              parentId: 'P',
              loaderId: 'LC',
              url: 'https://protected.test/',
              securityOrigin: 'https://protected.test',
            },
          });
        burst = true;
        event('Runtime.consoleAPICalled', {
          type: 'log',
          executionContextId: 1,
          args: [{ type: 'string', value: 'message-0' }],
        });
      },
      intercept(message, send) {
        if (message.method !== 'Page.getFrameTree' || !burst) return false;
        burst = false;
        for (let index = 1; index <= 20; index++)
          send({
            method: 'Runtime.consoleAPICalled',
            sessionId: session,
            params: {
              type: 'log',
              executionContextId: 1,
              args: [{ type: 'string', value: `message-${index}` }],
            },
          });
        return false;
      },
    });
    fake.targets.push({
      targetId: 'P',
      type: 'page',
      url: 'https://ordinary.test/',
      childFrames: [],
    });
    const client = await guarded(fake.url, 'https://protected.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      session = (await client.send('Target.attachToTarget', { targetId: 'P', flatten: true }))
        .sessionId;
      await client.send('Runtime.enable', {}, session);
      const from = client.events.length;
      await client.send('Page.getLayoutMetrics', {}, session);
      const console = client.events
        .slice(from)
        .filter((event) => event.method === 'Runtime.consoleAPICalled');
      assert.deepEqual(
        console.map((event) => event.params.args[0].value),
        locked ? [] : Array.from({ length: 21 }, (_, index) => `message-${index}`)
      );
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
