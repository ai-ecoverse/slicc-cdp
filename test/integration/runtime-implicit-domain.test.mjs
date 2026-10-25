import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const protectedChild of [false, true]) {
  test(`Runtime default evaluation without client enable ${protectedChild ? 'refused' : 'ordinary fulfillment'}`, async () => {
    const url =
      'http://localhost:8765/authorize?to=http%3A%2F%2F127.0.0.1%3A8765%2Fcb%23access_token%3DT%26state%3DS';
    let publicSession;
    const enabled = new Set();
    const senders = new Map();
    const frame = { id: 'USER', loaderId: 'L', url, securityOrigin: 'http://localhost:8765' };
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.sessionId) senders.set(message.sessionId, send);
        if (message.method === 'Runtime.enable') {
          enabled.add(message.sessionId);
          send({
            method: 'Runtime.executionContextCreated',
            sessionId: message.sessionId,
            params: {
              context: {
                id: 1,
                uniqueId: 'default:USER:L',
                origin: frame.securityOrigin,
                name: '',
                auxData: { frameId: frame.id, isDefault: true },
              },
            },
          });
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          return true;
        }
        if (message.method === 'Runtime.evaluate') {
          if (
            message.sessionId !== publicSession ||
            message.params.uniqueContextId !== 'default:USER:L'
          )
            send({
              id: message.id,
              sessionId: message.sessionId,
              error: {
                code: -32000,
                message: 'default execution context identity does not match the document',
              },
            });
          else
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { result: { type: 'string', value: 'Signed in' } },
            });
          return true;
        }
        return false;
      },
    });
    const page = { targetId: 'USER', type: 'page', url, frame };
    fake.targets.push(page);
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
      publicSession = sessionId;
      await client.send(
        'Fetch.enable',
        { patterns: [{ urlPattern: '*', requestStage: 'Response' }] },
        sessionId
      );
      senders.get(sessionId)({
        method: 'Fetch.requestPaused',
        sessionId,
        params: {
          requestId: 'AUTH',
          frameId: 'USER',
          resourceType: 'Document',
          responseStatusCode: 302,
          request: { url, method: 'GET', headers: {} },
          responseHeaders: [],
        },
      });
      await client.send('Page.getNavigationHistory', {}, sessionId);
      await client.send(
        'Fetch.fulfillRequest',
        {
          requestId: 'AUTH',
          responseCode: 200,
          responseHeaders: [{ name: 'Content-Type', value: 'text/html' }],
          body: Buffer.from('<title>Signed in</title>').toString('base64'),
        },
        sessionId
      );
      assert.equal(enabled.has(sessionId), false);
      if (protectedChild)
        page.childFrames = [
          {
            frame: {
              id: 'CHILD',
              parentId: 'USER',
              loaderId: 'LC',
              url: 'https://private.test/child',
              securityOrigin: 'https://private.test',
            },
          },
        ];
      const from = fake.calls.length;
      const evaluate = client.send(
        'Runtime.evaluate',
        { expression: 'document.title', returnByValue: true },
        sessionId
      );
      if (protectedChild) {
        await assert.rejects(evaluate, /slicc: refused:/);
        assert.equal(enabled.has(sessionId), false);
        assert.equal(
          fake.calls.slice(from).some((call) => call.method === 'Runtime.evaluate'),
          false
        );
      } else {
        assert.equal((await evaluate).result.value, 'Signed in');
        const forwarded = fake.calls.slice(from).find((call) => call.method === 'Runtime.evaluate');
        assert.equal(forwarded.sessionId, sessionId);
        assert.equal(forwarded.params.uniqueContextId, 'default:USER:L');
        assert.equal(enabled.has(sessionId), false);
        assert.ok(enabled.size > 0);
      }
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
