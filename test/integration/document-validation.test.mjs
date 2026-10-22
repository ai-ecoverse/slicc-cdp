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
  test(`guard-created blank content remains readable (${transport})`, {
    timeout: 60000,
  }, async () => {
    const backend = await fakeCdp();
    let client;
    try {
      client = await guarded(backend.url, 'https://private.example.invalid', {
        transport,
        variant: 'lagging',
        detachDelay: 0,
      });
      const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
      const from = backend.calls.length;
      await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
      await client.send('Runtime.evaluate', { expression: 'document.title' }, sessionId);
      assert.ok(
        !backend.calls.slice(from).some((call) => call.method === 'Target.detachFromTarget')
      );
    } finally {
      await client?.close();
      await backend.close();
    }
  });
  for (const shape of ['worker', 'child', 'response', 'stream', 'ordinary-response', 'new-frame']) {
    test(`document validation ${shape} (${transport})`, { timeout: 60000 }, async () => {
      let target;
      const backend = await fakeCdp({
        refuseWhen: (message) => {
          if (
            ['response', 'ordinary-response', 'new-frame'].includes(shape) &&
            message.method === 'Runtime.evaluate' &&
            message.params?.expression === 'document.title'
          ) {
            if (shape === 'new-frame') {
              target.childFrames = [
                {
                  frame: {
                    id: 'NEW',
                    parentId: 'T',
                    loaderId: 'LC',
                    url: 'https://ordinary.example/child',
                    securityOrigin: 'https://ordinary.example',
                  },
                },
              ];
            } else {
              target.frame =
                shape === 'ordinary-response'
                  ? frame('L2')
                  : {
                      ...frame('L2'),
                      url: 'https://seven.sliccy.ai/',
                      securityOrigin: 'https://seven.sliccy.ai',
                    };
            }
          }
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
        if (['ordinary-response', 'new-frame'].includes(shape)) {
          await client.send(method, params, sessionId);
          await client.send(method, params, sessionId);
          const probes = backend.calls
            .slice(from)
            .filter((call) => call.method === 'Page.getFrameTree');
          assert.equal(new Set(probes.map((call) => call.sessionId)).size, 1);
          assert.ok(
            !backend.calls.slice(from).some((call) => call.method === 'Target.detachFromTarget')
          );
        } else
          await assert.rejects(
            client.send(method, params, shape === 'stream' ? undefined : sessionId),
            /slicc: refused:/
          );
        if (!['response', 'ordinary-response', 'new-frame'].includes(shape))
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
