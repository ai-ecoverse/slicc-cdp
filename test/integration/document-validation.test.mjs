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

const frame = (loaderId) => ({
  id: 'T',
  loaderId,
  url: 'https://ordinary.example/',
  securityOrigin: 'https://ordinary.example',
});

for (const transport of ['native', 'wasi']) {
  for (const shape of ['worker', 'child', 'response', 'stream']) {
    test(`document validation ${shape} (${transport})`, { timeout: 60000 }, async () => {
      let target;
      const backend = await fakeCdp({
        refuseWhen: (message) => {
          if (
            shape === 'response' &&
            message.method === 'Runtime.evaluate' &&
            message.params?.expression === 'document.title'
          )
            target.frame = frame('L2');
          return false;
        },
      });
      target = {
        targetId: 'T',
        type: shape === 'worker' ? 'service_worker' : 'page',
        url: 'https://ordinary.example/',
        frame: frame('L1'),
      };
      backend.targets.push(target);
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
        if (shape === 'child') {
          target.childFrames = [
            {
              frame: {
                id: 'CHILD',
                loaderId: 'LC',
                url: 'https://seven.sliccy.ai/',
                securityOrigin: 'https://seven.sliccy.ai',
              },
            },
          ];
        }
        if (shape === 'stream') {
          await client.send('Page.printToPDF', { transferMode: 'ReturnAsStream' }, sessionId);
          target.frame = frame('L2');
        }
        const from = backend.calls.length;
        const method = shape === 'stream' ? 'IO.read' : 'Runtime.evaluate';
        const params = shape === 'stream' ? { handle: 'H' } : { expression: 'document.title' };
        await assert.rejects(
          client.send(method, params, shape === 'stream' ? undefined : sessionId),
          /slicc: refused:/
        );
        if (shape !== 'response')
          assert.ok(!backend.calls.slice(from).some((call) => call.method === method));
        else
          assert.ok(
            backend.calls.slice(from).filter((call) => call.method === 'Page.getFrameTree')
              .length >= 2
          );
      } finally {
        await client?.close();
        await backend.close();
      }
    });
  }
}
