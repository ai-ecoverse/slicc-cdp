import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const origin = 'https://ordinary.test';

test('page scope metadata sessions remain internal during attachment', async () => {
  const fake = await fakeCdp({ attachEvents: true });
  fake.targets.push({ targetId: 'P', type: 'page', url: `${origin}/` });
  fake.targets.push({ targetId: 'Q', type: 'page', url: `${origin}/seed` });
  fake.targets.push({
    targetId: 'V',
    type: 'iframe',
    url: `${origin}/frame`,
    parentFrameId: 'P',
    tree: {
      frame: {
        id: 'V',
        parentId: 'P',
        loaderId: 'LV',
        url: `${origin}/frame`,
        securityOrigin: origin,
      },
    },
  });
  const client = await guarded(fake.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'P',
      flatten: true,
    });
    const from = client.events.length;
    await client.send('DOMSnapshot.captureSnapshot', { computedStyles: [] }, sessionId);
    const attached = client.events
      .slice(from)
      .filter((event) => event.method === 'Target.attachedToTarget');
    assert.deepEqual(attached, [], JSON.stringify(attached));
    assert.ok(
      fake.calls.some(
        (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'V'
      )
    );
    assert.equal(
      fake.calls.some(
        (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'Q'
      ),
      false
    );
    await client.send('Target.closeTarget', { targetId: 'P' });
    assert.ok(fake.targets.some((target) => target.targetId === 'Q'));
  } finally {
    await client.close();
    await fake.close();
  }
});
