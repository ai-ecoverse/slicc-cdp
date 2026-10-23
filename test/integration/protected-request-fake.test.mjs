import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { before, test } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

before(() => {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    {
      stdio: 'inherit',
    }
  );
});

test('the request guard stops every protected resource before dispatch and fails closed', async () => {
  let guardSession;
  let patterns = [];
  let triggered = false;
  const hits = [];
  const pending = new Map();
  const rows = [
    'Document',
    'Fetch',
    'XHR',
    'Image',
    'Ping',
    'Script',
    'Stylesheet',
    'Font',
    'Other',
    'Preflight',
  ].flatMap((resourceType) =>
    [
      'https://seven.sliccy.ai/hop',
      'https://vault.sliccy.ai/direct',
      'https://protected.test/direct',
      'unclassifiable',
      '',
      'null',
      '://',
    ].map((url) => ({ resourceType, url }))
  );
  rows.push({ resourceType: 'Fetch' });
  const backend = await fakeCdp({
    intercept(message, send) {
      if (message.method === 'Fetch.enable' && !guardSession) {
        guardSession = message.sessionId;
        patterns = message.params.patterns;
      }
      if (message.method === 'Network.enable' && message.sessionId !== guardSession && !triggered) {
        triggered = true;
        for (const [index, row] of rows.entries()) {
          const requestId = `P${index}`;
          if (
            !patterns.some(
              (pattern) =>
                pattern.urlPattern === '*' &&
                (!pattern.resourceType || pattern.resourceType === row.resourceType)
            )
          ) {
            hits.push(requestId);
            continue;
          }
          pending.set(requestId, row);
          send({
            method: 'Fetch.requestPaused',
            sessionId: guardSession,
            params: {
              requestId,
              networkId: `N${index}`,
              resourceType: row.resourceType,
              request: {
                method: 'GET',
                headers: {},
                ...(row.url === undefined ? {} : { url: row.url }),
              },
            },
          });
        }
        send({
          method: 'Fetch.requestPaused',
          sessionId: guardSession,
          params: {
            requestId: 'ORDINARY',
            resourceType: 'Image',
            request: { url: 'https://ordinary.test/image', method: 'GET', headers: {} },
          },
        });
      }
      if (message.method === 'Fetch.continueRequest' && pending.has(message.params.requestId))
        hits.push(message.params.requestId);
    },
  });
  const client = await guarded(backend.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  try {
    const { targetId } = await client.send('Target.createTarget', {
      url: 'https://ordinary.test/page',
    });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    await client.send('Network.enable', {}, sessionId);
    await client.send('Network.enable', {}, sessionId);
    assert.deepEqual(hits, [], 'protected requests must never be dispatched');
    assert.equal(
      backend.calls.filter((call) => call.method === 'Fetch.failRequest').length,
      rows.length
    );
    assert.ok(
      backend.calls.some(
        (call) => call.method === 'Fetch.continueRequest' && call.params.requestId === 'ORDINARY'
      )
    );
  } finally {
    await client.close();
    await backend.close();
  }
});
