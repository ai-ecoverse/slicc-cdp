import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const ordinary = {
  id: 'CHILD',
  parentId: 'TAB1',
  loaderId: 'OLD',
  url: 'https://example.test/child',
  securityOrigin: 'https://example.test',
};
const protectedFrame = {
  ...ordinary,
  loaderId: 'NEW',
  url: 'https://protected.test/child',
  securityOrigin: 'https://protected.test',
};

async function fixture(options, run) {
  const fake = await fakeCdp(options);
  const client = await guarded(fake.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { targetId } = await client.send('Target.createTarget', { url: 'https://example.test/' });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    await run(client, sessionId, fake.calls);
  } finally {
    await client.close();
    await fake.close();
  }
}

function navigation(message, send, frame = protectedFrame) {
  send({ method: 'Page.frameNavigated', sessionId: message.sessionId, params: { frame } });
}

const read = (client, sessionId) =>
  client.send('Accessibility.getFullAXTree', { frameId: 'CHILD' }, sessionId);

test('cached frame records cannot authorize changed or removed frames', async () => {
  for (const replacement of [protectedFrame, undefined]) {
    const childFrames = [{ frame: { ...ordinary } }];
    await fixture({ childFrames }, async (client, sessionId, calls) => {
      await client.send('Page.getFrameTree', {}, sessionId);
      childFrames.splice(0, 1, ...(replacement ? [{ frame: replacement }] : []));
      await assert.rejects(read(client, sessionId), /slicc: refused:/);
      assert.equal(calls.filter((call) => call.method === 'Accessibility.getFullAXTree').length, 0);
    });
  }
});

test('an older frame tree reply cannot replace a newer navigation', async () => {
  for (const seeded of [false, true]) {
    let inject = !seeded;
    const childFrames = [{ frame: { ...ordinary } }];
    await fixture(
      {
        childFrames,
        intercept: (message, send) => {
          if (inject && message.method === 'Page.getFrameTree') navigation(message, send);
        },
      },
      async (client, sessionId, calls) => {
        if (seeded) {
          await client.send('Page.getFrameTree', {}, sessionId);
          inject = true;
          await client.send('Page.getFrameTree', {}, sessionId).catch(() => {});
        }
        await assert.rejects(read(client, sessionId), /slicc: refused:/);
        assert.equal(
          calls.filter((call) => call.method === 'Accessibility.getFullAXTree').length,
          0
        );
      }
    );
  }
});

test('a navigation during a frame-scoped read prevents returning its reply', async () => {
  await fixture(
    {
      childFrames: [{ frame: { ...ordinary } }],
      intercept: (message, send) => {
        if (message.method === 'Accessibility.getFullAXTree') navigation(message, send);
      },
    },
    async (client, sessionId, calls) => {
      await assert.rejects(read(client, sessionId), /slicc: refused:/);
      assert.equal(calls.filter((call) => call.method === 'Accessibility.getFullAXTree').length, 1);
    }
  );
});

test('frame-scoped reads require verified URL and origin records', async () => {
  for (const key of ['url', 'securityOrigin']) {
    for (const value of [undefined, null, '', 'null', '://']) {
      for (const seeded of [false, true]) {
        const frame = { ...ordinary, [key]: value };
        await fixture({ childFrames: [{ frame }] }, async (client, sessionId, calls) => {
          if (seeded) await client.send('Page.getFrameTree', {}, sessionId);
          await assert.rejects(read(client, sessionId), /slicc: refused:/);
          assert.equal(
            calls.filter((call) => call.method === 'Accessibility.getFullAXTree').length,
            0
          );
        });
      }
    }
  }
});

test('ordinary frame-scoped replies are checked against the current tree', async () => {
  await fixture({ childFrames: [{ frame: { ...ordinary } }] }, async (client, sessionId, calls) => {
    await read(client, sessionId);
    assert.equal(calls.filter((call) => call.method === 'Page.getFrameTree').length, 2);
    assert.equal(calls.filter((call) => call.method === 'Accessibility.getFullAXTree').length, 1);
  });
});
