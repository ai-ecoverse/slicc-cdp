import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test, { before } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

before(() => {
  const args = ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'];
  execFileSync('cargo', args, { stdio: 'inherit' });
  execFileSync('cargo', [...args, '--target', 'wasm32-wasip1'], { stdio: 'inherit' });
});

for (const transport of ['native', 'wasi']) {
  test(`ordinary controls do not probe an unavailable document (${transport})`, {
    timeout: 60000,
  }, async () => {
    let caller;
    let request = 0;
    let blocked = false;
    const backend = await fakeCdp({
      refuseWhen: (message) => blocked && message.method === 'Page.getFrameTree',
      onCommand: (message, event) => {
        if (message.method !== 'Fetch.enable' || message.sessionId !== caller) return;
        blocked = true;
        event('Fetch.requestPaused', {
          requestId: `R${++request}`,
          request: { url: 'https://ordinary.example/request', method: 'GET' },
          resourceType: 'Document',
          responseStatusCode: 200,
        });
      },
    });
    backend.targets.push({ targetId: 'T', url: 'https://ordinary.example/' });
    let client;
    try {
      client = await guarded(backend.url, 'https://private.example.invalid', {
        transport,
        variant: 'lagging',
        detachDelay: 0,
      });
      caller = (await client.send('Target.attachToTarget', { targetId: 'T', flatten: true }))
        .sessionId;
      const from = backend.calls.length;
      for (const [method, params] of [
        ['Fetch.continueRequest', { method: 'POST' }],
        ['Fetch.fulfillRequest', { responseCode: 200, body: 'U2lnbmVkIGlu' }],
        ['Fetch.failRequest', { errorReason: 'Aborted' }],
        ['Fetch.continueResponse', { responseCode: 200 }],
        ['Fetch.continueWithAuth', { authChallengeResponse: { response: 'Default' } }],
      ]) {
        await client.send(
          'Fetch.enable',
          { patterns: [{ urlPattern: 'https://ordinary.example/*' }] },
          caller
        );
        await client.send(method, { requestId: `R${request}`, ...params }, caller);
      }
      for (const [method, params] of [
        ['Page.enable', {}],
        ['Page.setLifecycleEventsEnabled', { enabled: true }],
        ['Runtime.enable', {}],
        ['DOM.enable', {}],
        ['Network.enable', {}],
        ['Page.navigate', { url: 'https://ordinary.example/next' }],
        ['Runtime.evaluate', { expression: 'document.readyState', returnByValue: true }],
      ])
        await client.send(method, params, caller);
      assert.ok(!backend.calls.slice(from).some((call) => call.method === 'Page.getFrameTree'));
      const contentFrom = backend.calls.length;
      await assert.rejects(client.send('DOM.getDocument', {}, caller), /slicc: refused:/);
      assert.ok(
        !backend.calls.slice(contentFrom).some((call) => call.method === 'DOM.getDocument')
      );
      await assert.rejects(client.send('Page.readFutureContent', {}, caller), /slicc: refused:/);
      assert.ok(!backend.calls.some((call) => call.method === 'Page.readFutureContent'));
    } finally {
      await client?.close();
      await backend.close();
    }
  });
}
