import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test, { before } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

before(() => {
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync(
      'cargo',
      ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
});

for (const transport of ['native', 'wasi']) {
  test(`guard bridge ${transport} emits a Fetch pause while navigation is in flight`, {
    timeout: 20000,
  }, async () => {
    const peer = await fakeCdp({ pauseNavigation: true });
    const previous = process.env.SLICC_GUARD_TRANSPORT;
    let client;
    try {
      process.env.SLICC_GUARD_TRANSPORT = transport;
      client = await guarded(peer.url, 'https://own.invalid', {
        variant: 'prompt',
        detachDelay: 0,
      });
      const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
      await client.send(
        'Fetch.enable',
        { patterns: [{ urlPattern: '*', requestStage: 'Request' }] },
        sessionId
      );
      const paused = client.waitEvent(
        (event) => event.method === 'Fetch.requestPaused' && event.sessionId === sessionId,
        2000
      );
      let complete = false;
      const navigation = client
        .send('Page.navigate', { url: 'https://ordinary.invalid/queue' }, sessionId)
        .then((result) => {
          complete = true;
          return result;
        });
      const event = await paused;
      assert.equal(complete, false, 'navigation responded before its required Fetch continuation');
      assert.equal(event.params.networkId, `NET-${sessionId}`);
      await client.send('Fetch.continueRequest', { requestId: event.params.requestId }, sessionId);
      const result = await navigation;
      assert.equal(result.frameId, targetId);
      assert.match(result.loaderId, /^L\d+$/);
      assert.ok(
        peer.calls.some(
          (call) =>
            call.method === 'Fetch.continueRequest' &&
            call.params.requestId === event.params.requestId
        )
      );
      await client.send('Target.closeTarget', { targetId });
    } finally {
      await client?.close();
      await peer.close();
      if (previous === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
      else process.env.SLICC_GUARD_TRANSPORT = previous;
    }
  });
}
