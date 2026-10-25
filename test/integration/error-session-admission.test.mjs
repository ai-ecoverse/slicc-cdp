import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const reads = [
  ['Runtime.evaluate', { expression: 'document.title' }],
  ['Runtime.evaluate', { expression: 'loadTimeData.data_', returnByValue: true }],
  ['DOM.getDocument', { depth: 1 }],
  ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
];

for (const mode of ['unavailable', 'stale']) {
  for (const kind of ['page', 'iframe']) {
    test(`guard refuses unverified error sessions (${kind}, ${mode})`, async () => {
      const marker = randomUUID();
      const origin = 'https://private.test';
      const failedUrl = `${origin}/controller/${marker}`;
      const output = [];
      const fake = await fakeCdp({
        tabSessions: true,
        intercept(message, send) {
          const target = fake.calls.at(-1)?.target;
          if (target === 'ERROR' && message.method === 'Page.getFrameTree') {
            if (mode === 'unavailable') {
              send({
                id: message.id,
                sessionId: message.sessionId,
                error: { message: 'not ready' },
              });
            } else {
              send({
                id: message.id,
                sessionId: message.sessionId,
                result: {
                  frameTree: {
                    frame: {
                      id: 'ERROR',
                      loaderId: 'LE',
                      url: 'https://ordinary.test/child',
                      securityOrigin: 'https://ordinary.test',
                      ...(kind === 'iframe' ? { parentId: 'TOP' } : {}),
                    },
                  },
                },
              });
            }
            return true;
          }
          if (target === 'ERROR' && mode === 'stale' && message.method === 'Runtime.enable') {
            send({
              method: 'Runtime.executionContextCreated',
              sessionId: message.sessionId,
              params: {
                context: {
                  id: 1,
                  uniqueId: 'error-context',
                  origin: 'https://ordinary.test',
                  name: '',
                  auxData: { frameId: 'ERROR', isDefault: true },
                },
              },
            });
            send({ id: message.id, sessionId: message.sessionId, result: {} });
            return true;
          }
          if (target === 'ERROR' && reads.some(([method]) => method === message.method)) {
            send({ id: message.id, sessionId: message.sessionId, result: {} });
            return true;
          }
          return false;
        },
      });
      const parent = {
        targetId: 'TOP',
        type: 'page',
        url: 'https://ordinary.test/',
        frame: {
          id: 'TOP',
          loaderId: 'LT',
          url: 'https://ordinary.test/',
          securityOrigin: 'https://ordinary.test',
        },
      };
      fake.targets.push(parent);
      const client = await guarded(fake.url, origin, {
        variant: 'prompt',
        detachDelay: 0,
        onMessage: (message) => output.push(message),
      });
      try {
        const { sessionId: top } = await client.send('Target.attachToTarget', {
          targetId: 'TOP',
          flatten: true,
        });
        await client.send('Runtime.evaluate', { expression: 'document.title' }, top);
        const frame = {
          id: 'ERROR',
          loaderId: 'LE',
          url: 'chrome-error://chromewebdata/',
          securityOrigin: '://',
          unreachableUrl: failedUrl,
          ...(kind === 'iframe' ? { parentId: 'TOP' } : {}),
        };
        fake.targets.push({
          targetId: 'ERROR',
          type: kind,
          url: frame.url,
          frame,
          ...(kind === 'iframe' ? { parentFrameId: 'TOP' } : {}),
        });
        if (kind === 'iframe')
          parent.childFrames = [
            {
              frame:
                mode === 'stale'
                  ? {
                      id: 'ERROR',
                      parentId: 'TOP',
                      loaderId: 'LE',
                      url: 'https://ordinary.test/child',
                      securityOrigin: 'https://ordinary.test',
                    }
                  : { ...frame },
            },
          ];
        const attached = await client
          .send('Target.attachToTarget', { targetId: 'ERROR', flatten: true })
          .then(
            (value) => value,
            (error) => ({ refused: /slicc: refused:/.test(error.message) })
          );
        const from = fake.calls.length;
        const results = [];
        for (const [method, params] of reads) {
          const refused = await client.send(method, params, attached.sessionId ?? 'ERROR').then(
            () => false,
            (error) => /slicc: refused:/.test(error.message)
          );
          results.push({ method, refused });
        }
        assert.deepEqual(
          results,
          reads.map(([method]) => ({ method, refused: true }))
        );
        assert.equal(
          fake.calls
            .slice(from)
            .some(
              (call) => call.target === 'ERROR' && reads.some(([method]) => method === call.method)
            ),
          false
        );
        const text = JSON.stringify(output);
        assert.equal(text.includes(failedUrl), false, 'protected URL is absent');
        assert.equal(text.includes(marker), false, 'document marker is absent');
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}

test('guard refuses pending page replies after a late unverified frame', async () => {
  const marker = randomUUID();
  const output = [];
  let release;
  const reached = Promise.withResolvers();
  const parent = { targetId: 'TOP', type: 'page', url: 'https://ordinary.test/' };
  const fake = await fakeCdp({
    intercept(message, send) {
      if (message.method !== 'Page.captureScreenshot') return false;
      const child = {
        id: 'ERROR',
        parentId: 'TOP',
        loaderId: 'LE',
        url: 'chrome-error://chromewebdata/',
        securityOrigin: '://',
        unreachableUrl: `https://private.test/controller/${marker}`,
      };
      parent.childFrames = [{ frame: child }];
      fake.targets.push({
        targetId: 'ERROR',
        type: 'iframe',
        parentFrameId: 'TOP',
        url: child.url,
        frame: child,
      });
      send({
        method: 'Page.frameAttached',
        sessionId: message.sessionId,
        params: { frameId: 'ERROR', parentFrameId: 'TOP' },
      });
      release = () =>
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: { data: Buffer.from(marker).toString('base64') },
        });
      reached.resolve();
      return true;
    },
  });
  fake.targets.push(parent);
  const client = await guarded(fake.url, 'https://private.test', {
    variant: 'prompt',
    detachDelay: 0,
    onMessage: (message) => output.push(message),
  });
  try {
    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId: 'TOP',
      flatten: true,
    });
    const screenshot = assert.rejects(
      client.send('Page.captureScreenshot', {}, sessionId),
      /slicc: refused:/
    );
    await reached.promise;
    release();
    await screenshot;
    const text = JSON.stringify(output);
    assert.equal(text.includes(marker), false, 'document marker is absent');
    assert.equal(text.includes(Buffer.from(marker).toString('base64')), false, 'image is absent');
  } finally {
    release?.();
    await client.close();
    await fake.close();
  }
});
