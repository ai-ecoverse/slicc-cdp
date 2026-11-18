import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test, { before } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const PLANTS = ['seven.sliccy.ai', 'vault.sliccy.ai', '.sliccy.ai'];
const CREDENTIAL = ['.adobelogin.com', 'ims-na1.adobelogin.com'];

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

function forwarded(backend, method, idKey, id) {
  return backend.calls.some((call) => call.method === method && call.params?.[idKey] === id);
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
    client = await guarded(backend.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
    });
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
      const fromFulfill = backend.calls.length;
      await assert.rejects(
        send('Fetch.fulfillRequest', {
          requestId: fulfillId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /cookies on seven's origin/
      );
      assert.equal(
        backend.calls.slice(fromFulfill).some((call) => call.method === 'Fetch.fulfillRequest'),
        false,
        `fulfill ${domain} must not reach the backend`
      );

      const continueId = await enable('Response');
      const fromContinue = backend.calls.length;
      await assert.rejects(
        send('Fetch.continueResponse', {
          requestId: continueId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /cookies on seven's origin/
      );
      assert.equal(
        backend.calls.slice(fromContinue).some((call) => call.method === 'Fetch.continueResponse'),
        false,
        `continueResponse ${domain} must not reach the backend`
      );

      const rawId = await enable('Request');
      const fromRaw = backend.calls.length;
      await assert.rejects(
        send('Network.continueInterceptedRequest', {
          interceptionId: rawId,
          rawResponse: Buffer.from(
            `HTTP/1.1 200 OK\r\nSet-Cookie: ${plant(domain)}\r\n\r\n`
          ).toString('base64'),
        }),
        /cookies on seven's origin/
      );
      assert.equal(
        backend.calls
          .slice(fromRaw)
          .some((call) => call.method === 'Network.continueInterceptedRequest'),
        false,
        `continueInterceptedRequest ${domain} must not reach the backend`
      );
    }

    for (const domain of CREDENTIAL) {
      const fulfillId = await enable('Request');
      const fromFulfill = backend.calls.length;
      await assert.rejects(
        send('Fetch.fulfillRequest', {
          requestId: fulfillId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /this origin holds sign-in credentials/
      );
      assert.equal(
        backend.calls.slice(fromFulfill).some((call) => call.method === 'Fetch.fulfillRequest'),
        false,
        `credential fulfill ${domain} must not reach the backend`
      );

      const continueId = await enable('Response');
      const fromContinue = backend.calls.length;
      await assert.rejects(
        send('Fetch.continueResponse', {
          requestId: continueId,
          responseCode: 200,
          responseHeaders: [{ name: 'Set-Cookie', value: plant(domain) }],
        }),
        /this origin holds sign-in credentials/
      );
      assert.equal(
        backend.calls.slice(fromContinue).some((call) => call.method === 'Fetch.continueResponse'),
        false,
        `credential continueResponse ${domain} must not reach the backend`
      );

      const rawId = await enable('Request');
      const fromRaw = backend.calls.length;
      await assert.rejects(
        send('Network.continueInterceptedRequest', {
          interceptionId: rawId,
          rawResponse: Buffer.from(
            `HTTP/1.1 200 OK\r\nSet-Cookie: ${plant(domain)}\r\n\r\n`
          ).toString('base64'),
        }),
        /this origin holds sign-in credentials/
      );
      assert.equal(
        backend.calls
          .slice(fromRaw)
          .some((call) => call.method === 'Network.continueInterceptedRequest'),
        false,
        `credential continueInterceptedRequest ${domain} must not reach the backend`
      );
    }

    const ordinaryId = await enable('Request');
    await send('Fetch.fulfillRequest', {
      requestId: ordinaryId,
      responseCode: 200,
      responseHeaders: [{ name: 'Set-Cookie', value: 'sid=ok; Domain=ordinary.example; Path=/' }],
    });
    assert.ok(forwarded(backend, 'Fetch.fulfillRequest', 'requestId', ordinaryId));

    const deleteId = await enable('Request');
    await send('Fetch.fulfillRequest', {
      requestId: deleteId,
      responseCode: 200,
      responseHeaders: [{ name: 'Set-Cookie', value: 'sid=; Max-Age=0; Path=/' }],
    });
    assert.ok(forwarded(backend, 'Fetch.fulfillRequest', 'requestId', deleteId));

    const cookie2PlantId = await enable('Request');
    const fromCookie2Plant = backend.calls.length;
    await assert.rejects(
      send('Fetch.fulfillRequest', {
        requestId: cookie2PlantId,
        responseCode: 200,
        responseHeaders: [{ name: 'Set-Cookie2', value: plant('.sliccy.ai') }],
      }),
      /cookies on seven's origin/
    );
    assert.equal(
      backend.calls.slice(fromCookie2Plant).some((call) => call.method === 'Fetch.fulfillRequest'),
      false,
      'Set-Cookie2 plant must not reach the backend'
    );

    const cookie2OrdinaryId = await enable('Request');
    await send('Fetch.fulfillRequest', {
      requestId: cookie2OrdinaryId,
      responseCode: 200,
      responseHeaders: [{ name: 'Set-Cookie2', value: 'sid=ok; Domain=ordinary.example; Path=/' }],
    });
    assert.ok(forwarded(backend, 'Fetch.fulfillRequest', 'requestId', cookie2OrdinaryId));

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
