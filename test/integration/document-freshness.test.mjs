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
  for (const [method, params] of [
    ['Runtime.evaluate', { expression: 'document.documentElement.innerHTML' }],
    ['DOM.getDocument', { depth: -1 }],
    ['Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter' }],
  ]) {
    test(`current document check before ${method} (${transport})`, { timeout: 60000 }, async () => {
      const backend = await fakeCdp({ delayHistoryEvents: true });
      backend.targets.push({
        targetId: 'T',
        url: 'https://ordinary.example/current',
        history: {
          currentIndex: 1,
          entries: [
            { id: 1, url: 'https://seven.sliccy.ai/document' },
            { id: 2, url: 'https://ordinary.example/current' },
          ],
        },
      });
      let client;
      try {
        client = await guarded(backend.url, 'https://private.example.invalid', {
          variant: 'lagging',
          detachDelay: 0,
          transport,
        });
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId: 'T',
          flatten: true,
        });
        await client.send('Runtime.evaluate', { expression: 'history.back()' }, sessionId);
        assert.equal(backend.targets[0].url, 'https://seven.sliccy.ai/document');
        assert.ok(backend.heldEvents.length > 0);
        const from = backend.calls.length;
        const result = await client.send(method, params, sessionId).then(
          (value) => ({ value }),
          (error) => ({ error })
        );
        backend.releaseEvents();
        assert.match(result.error?.message ?? '', /slicc: refused:/);
        assert.ok(
          !backend.calls
            .slice(from)
            .some((call) => call.method === method && call.sessionId === sessionId)
        );
        assert.ok(
          backend.calls
            .slice(from)
            .some((call) => call.method === 'Page.getFrameTree' && call.sessionId !== sessionId)
        );
      } finally {
        await client?.close();
        await backend.close();
      }
    });
  }
}
