import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test, { before } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const PLANTS = ['seven.sliccy.ai', 'vault.sliccy.ai', '.sliccy.ai'];

before(() => {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    {
      stdio: 'inherit',
    }
  );
});

function plant(domain) {
  return `sid=PLANTED; Domain=${domain}; HttpOnly`;
}

async function withPage(body) {
  let request = 0;
  let session;
  const backend = await fakeCdp({
    onCommand: (message, event) => {
      if (message.method !== 'Fetch.enable' || message.sessionId !== session) return;
      const stage = message.params?.patterns?.[0]?.requestStage ?? 'Request';
      request += 1;
      const params = {
        requestId: `R${request}`,
        networkId: `N${request}`,
        frameId: 'F1',
        resourceType: 'XHR',
        request: { url: 'https://ordinary.example/api', method: 'GET', headers: {} },
      };
      if (stage === 'Response') {
        params.responseStatusCode = 200;
        params.responseHeaders = [{ name: 'Content-Type', value: 'text/plain' }];
      }
      event('Fetch.requestPaused', params);
    },
  });
  backend.targets.push({ targetId: 'T', url: 'https://ordinary.example/' });
  let client;
  try {
    client = await guarded(backend.url, 'https://private.example.invalid');
    session = (await client.send('Target.attachToTarget', { targetId: 'T', flatten: true }))
      .sessionId;
    await body({
      send: (method, params) => client.send(method, params, session),
      enable: async (stage) => {
        await client.send(
          'Fetch.enable',
          { patterns: [{ urlPattern: 'https://ordinary.example/*', requestStage: stage }] },
          session
        );
        return `R${request}`;
      },
      backend,
    });
  } finally {
    await client?.close();
    await backend.close();
  }
}

test('fake: response-side Set-Cookie for protected scopes through interception is refused', async () => {
  await withPage(async ({ send, enable, backend }) => {
    for (const domain of PLANTS) {
      const fulfillId = await enable('Request');
      await assert.rejects(
        send('Fetch.fulfillRequest', {
          requestId: fulfillId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /cookies on seven's origin/
      );
      const binaryId = await enable('Request');
      await assert.rejects(
        send('Fetch.fulfillRequest', {
          requestId: binaryId,
          responseCode: 200,
          binaryResponseHeaders: Buffer.from(
            `Set-Cookie: ${plant(domain)}\0Content-Type: text/plain`
          ).toString('base64'),
        }),
        /cookies on seven's origin/
      );
      const continueId = await enable('Response');
      await assert.rejects(
        send('Fetch.continueResponse', {
          requestId: continueId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /cookies on seven's origin/
      );
      const rawId = await enable('Request');
      await assert.rejects(
        send('Network.continueInterceptedRequest', {
          interceptionId: rawId,
          rawResponse: Buffer.from(
            `HTTP/1.1 200 OK\r\nSet-Cookie: ${plant(domain)}\r\n\r\n`
          ).toString('base64'),
        }),
        /cookies on seven's origin/
      );
    }
    const okId = await enable('Request');
    await send('Fetch.fulfillRequest', {
      requestId: okId,
      responseCode: 200,
      responseHeaders: [{ name: 'Set-Cookie', value: 'sid=; Max-Age=0; Path=/' }],
    });
    assert.ok(
      backend.calls.some(
        (call) => call.method === 'Fetch.fulfillRequest' && call.params?.requestId === okId
      )
    );
    await assert.rejects(
      send('Network.setRequestInterception', { patterns: [] }),
      /slicc: refused:/
    );
    await assert.rejects(
      send('Network.getResponseBodyForInterception', { interceptionId: 'I1' }),
      /slicc: refused:/
    );
  });
});
