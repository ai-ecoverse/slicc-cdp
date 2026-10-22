import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

test('guard bridge releases observed attached metadata before its response', {
  timeout: 60000,
}, async () => {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    { stdio: 'inherit' }
  );
  const backend = await fakeCdp({ attachEvents: true });
  let client;
  try {
    client = await guarded(backend.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
    });
    const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    const attached = client.events.find(
      (event) => event.method === 'Target.attachedToTarget' && event.params.sessionId === sessionId
    );
    assert(attached, 'the call must not resolve before its already-observed attached event');
    assert.equal(attached.params.targetInfo.targetId, targetId);
    assert.equal(attached.params.targetInfo.url, 'about:blank');
    assert.equal(attached.params.targetInfo.title, 'about:blank');
  } finally {
    await client?.close();
    await backend.close();
  }
});
