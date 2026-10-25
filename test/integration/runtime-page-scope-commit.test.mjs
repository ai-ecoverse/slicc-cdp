import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

for (const mode of [
  'ordinary',
  'carrier-only',
  'selected-context',
  'protected',
  'protected-child',
  'placement-change',
  'unknown-change',
  'guard-change',
  'repeated',
]) {
  test(`Runtime reproof of an in-flight fulfilled document ${mode}`, async () => {
    const url =
      mode === 'carrier-only'
        ? 'http://localhost:8765/authorize?to=https%3A%2F%2Fprivate.test%2Fcb%23access_token%3DT%26state%3DS'
        : 'http://localhost:8765/authorize?to=http%3A%2F%2F127.0.0.1%3A8765%2Fcb%23access_token%3DT%26state%3DS';
    const allowed = mode === 'ordinary' || mode === 'carrier-only';
    const frame = { id: 'USER', loaderId: 'L0', url: 'about:blank', securityOrigin: '://' };
    const senders = new Map();
    let active = false;
    let fulfilled = false;
    let commits = 0;
    const page = { targetId: 'USER', type: 'page', url: 'about:blank', frame };
    const fake = await fakeCdp({
      intercept(message, send) {
        if (message.sessionId) senders.set(message.sessionId, send);
        if (message.method === 'Target.createTarget') {
          send({ id: message.id, result: { targetId: 'USER' } });
          return true;
        }
        if (message.method === 'Fetch.fulfillRequest') fulfilled = true;
        if (message.method === 'Runtime.enable') {
          send({
            method: 'Runtime.executionContextCreated',
            sessionId: message.sessionId,
            params: {
              context: {
                id: 1,
                uniqueId: `context:${frame.loaderId}`,
                origin: frame.securityOrigin === '://' ? '' : frame.securityOrigin,
                name: '',
                auxData: { frameId: frame.id, isDefault: true },
              },
            },
          });
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          return true;
        }
        if (
          active &&
          fulfilled &&
          message.method === 'Page.getFrameTree' &&
          (commits === 0 || mode === 'repeated')
        ) {
          commits += 1;
          frame.loaderId = `L${commits}`;
          frame.url = mode === 'protected' ? 'https://private.test/' : url;
          frame.securityOrigin =
            mode === 'protected' ? 'https://private.test' : 'http://localhost:8765';
          page.url = frame.url;
          send({
            method: 'Page.frameNavigated',
            sessionId: message.sessionId,
            params: { frame: { ...frame } },
          });
          if (mode === 'protected-child' || mode === 'placement-change') {
            page.childFrames = [
              {
                frame: {
                  id: 'CHILD',
                  parentId: 'USER',
                  loaderId: 'LC',
                  url:
                    mode === 'protected-child'
                      ? 'https://private.test/child'
                      : 'http://localhost:8765/child',
                  securityOrigin:
                    mode === 'protected-child' ? 'https://private.test' : 'http://localhost:8765',
                },
              },
            ];
            if (mode === 'placement-change')
              send({
                method: 'Page.frameAttached',
                sessionId: message.sessionId,
                params: { frameId: 'CHILD', parentFrameId: 'USER' },
              });
          }
          if (mode === 'guard-change')
            send({ method: 'Target.detachedFromTarget', params: { sessionId: message.sessionId } });
          if (mode === 'unknown-change')
            send({
              method: 'Page.frameAttached',
              sessionId: 'UNKNOWN',
              params: { frameId: 'UNKNOWN', parentFrameId: 'UNKNOWN_PARENT' },
            });
          send({
            id: message.id,
            sessionId: message.sessionId,
            result: {
              frameTree: {
                frame: { ...frame },
                ...(page.childFrames ? { childFrames: page.childFrames } : {}),
              },
            },
          });
          return true;
        }
        if (message.method === 'Runtime.evaluate') {
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
    fake.targets.push(page);
    const client = await guarded(fake.url, 'https://private.test', {
      variant: 'prompt',
      detachDelay: 0,
    });
    try {
      await client.send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId: 'USER',
        flatten: true,
      });
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
      page.url = '';
      active = true;
      const from = fake.calls.length;
      const result = client.send(
        'Runtime.evaluate',
        {
          expression: 'document.title',
          returnByValue: true,
          ...(mode === 'selected-context' ? { contextId: 1 } : {}),
        },
        sessionId
      );
      if (allowed) assert.equal((await result).result.value, 'Signed in');
      else await assert.rejects(result, /slicc: refused:/);
      const calls = fake.calls.slice(from);
      assert.ok(commits >= 1 && commits <= (mode === 'repeated' ? 2 : 1));
      assert.equal(
        calls.filter((call) => call.method === 'Runtime.evaluate').length,
        allowed ? 1 : 0
      );
      if (allowed) {
        assert.equal(calls.filter((call) => call.method === 'Target.getTargets').length, 2);
        assert.ok(
          calls
            .filter((call) => call.method === 'Page.getFrameTree')
            .every((call) => call.sessionId !== sessionId)
        );
        assert.equal(
          calls.filter((call) => call.method === 'Runtime.enable' && call.sessionId === sessionId)
            .length,
          0
        );
      }
    } finally {
      await client.close();
      await fake.close();
    }
  });
}
