import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';
import { diagnosticValue, guardDiagnostics } from './guard-diagnostics.mjs';

const frame = (loader, origin = 'https://ordinary.example') => ({
  id: 'TAB',
  loaderId: loader,
  url: `${origin}/same`,
  securityOrigin: origin,
});
const reply = (message, send, value) =>
  send({
    id: message.id,
    sessionId: message.sessionId,
    result: { result: { type: 'string', value } },
  });

for (const transport of ['native', 'wasi']) {
  for (const mode of [
    'resolve',
    'navigation',
    'poll-navigation',
    'poll-refusal',
    'original-refusal',
  ]) {
    test(`guard promise bridge ${transport}: ${mode}`, { timeout: 60000 }, async () => {
      let backend;
      let session;
      let started = false;
      let polls = 0;
      let changed = false;
      const messages = [];
      backend = await fakeCdp({
        intercept: (message, send) => {
          if (
            mode === 'original-refusal' &&
            session &&
            message.method === 'Page.getFrameTree' &&
            message.sessionId === session &&
            !changed
          ) {
            changed = true;
            send({
              id: message.id,
              sessionId: session,
              result: { frameTree: { frame: frame('L1') } },
            });
            backend.targets[0].frame = frame('LPROTECTED', 'https://seven.sliccy.ai');
            backend.targets[0].url = backend.targets[0].frame.url;
            return true;
          }
          if (
            started &&
            !changed &&
            message.method === 'Page.getFrameTree' &&
            ['poll-navigation', 'poll-refusal'].includes(mode)
          ) {
            changed = true;
            backend.targets[0].frame =
              mode === 'poll-navigation' ? frame('L2') : { ...frame('L1'), securityOrigin: '' };
            if (mode === 'poll-navigation')
              send({
                method: 'Page.frameNavigated',
                sessionId: session,
                params: { frame: backend.targets[0].frame },
              });
          }
          if (message.method !== 'Runtime.evaluate') return false;
          if (message.params.expression.includes('Promise.resolve((0, eval)')) {
            started = true;
            reply(message, send, true);
            if (mode === 'navigation') {
              changed = true;
              backend.targets[0].frame = frame('L2');
              send({
                method: 'Page.frameNavigated',
                sessionId: session,
                params: { frame: backend.targets[0].frame },
              });
            }
            return true;
          }
          if (message.params.expression.startsWith('JSON.stringify(globalThis[')) {
            polls += 1;
            reply(message, send, JSON.stringify({ done: polls >= 2, value: 'actual-outcome' }));
            return true;
          }
          return false;
        },
      });
      backend.targets.push({
        targetId: 'TAB',
        type: 'page',
        url: frame('L1').url,
        frame: frame('L1'),
      });
      let client;
      try {
        client = await guarded(backend.url, 'https://private.example.invalid', {
          variant: 'prompt',
          detachDelay: 0,
          transport,
          onMessage: (message) => messages.push(message),
        });
        session = (await client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true }))
          .sessionId;
        await client.send('Page.enable', {}, session);
        const outcome = client.send(
          'Runtime.evaluate',
          { expression: 'document.title', awaitPromise: true, returnByValue: true },
          session
        );
        if (mode === 'resolve') {
          assert.equal((await outcome).result.value, 'actual-outcome');
          assert.equal(polls, 2);
          const evaluations = backend.calls.filter((call) => call.method === 'Runtime.evaluate');
          assert(evaluations.every((call) => call.params.uniqueContextId === 'default:TAB:L1'));
        } else if (['navigation', 'poll-navigation'].includes(mode)) {
          await assert.rejects(outcome, (error) => {
            assert.equal(error.message, 'bridge evaluation navigated or superseded');
            assert(!/refused|session.*closed|target.*closed|no session/i.test(error.message));
            return true;
          });
          assert.equal(polls, 0);
          assert(messages.some((message) => message.diagnostic?.phase === 'superseded'));
        } else {
          await assert.rejects(outcome, /slicc: refused:/);
          assert.equal(polls, 0);
          if (mode === 'original-refusal') {
            assert.equal(started, false);
            assert(
              messages.some(
                (message) => message.diagnostic?.phase === 'original' && message.diagnostic.error
              )
            );
          } else assert(messages.some((message) => message.diagnostic?.phase === 'poll'));
        }
      } finally {
        await client?.close();
        await backend.close();
      }
    });
  }
}

test('C5 diagnostics hash values but preserve commands, error classes and physical detach evidence', async () => {
  const secret = 'private-token-do-not-emit';
  const sanitized = diagnosticValue({
    method: 'Runtime.evaluate',
    params: { expression: secret, headers: { authorization: secret } },
    error: { message: `failed ${secret}` },
    result: { value: secret },
  });
  assert(!JSON.stringify(sanitized).includes(secret));
  assert.equal(sanitized.method, 'Runtime.evaluate');
  assert.equal(sanitized.params.expression.sha256, sanitized.result.value.sha256);
  const backend = await fakeCdp({
    attachEvents: true,
    intercept: (message, send) => {
      if (message.method === 'Target.detachFromTarget')
        send({
          method: 'Target.detachedFromTarget',
          params: { sessionId: message.params.sessionId },
        });
      return false;
    },
  });
  const trace = await guardDiagnostics(backend.url, {
    id: 'C5.document',
    variant: 'prompt',
    target: 'ordinary',
  });
  let client;
  try {
    client = await guarded(trace.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
      transport: 'native',
      onMessage: trace.onMessage,
    });
    trace.wrap(client);
    const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    await client.send(
      'Fetch.enable',
      { patterns: [{ urlPattern: '*', requestStage: 'Request' }] },
      sessionId
    );
    await client.send('Target.detachFromTarget', { sessionId });
    assert(trace.record.wire.some((entry) => entry.message.method === 'Target.attachToTarget'));
    assert(trace.record.wire.some((entry) => entry.message.method === 'Fetch.enable'));
    assert(trace.record.wire.some((entry) => entry.message.method === 'Target.detachFromTarget'));
    assert(trace.record.wire.some((entry) => entry.message.method === 'Target.detachedFromTarget'));
    assert(trace.record.events.some((entry) => entry.event.method === 'Target.detachedFromTarget'));
    assert.equal(trace.record.calls[0].id, 1);
    const failed = {
      send: async () => {
        throw Object.assign(new Error(`failed ${secret}`), {
          cdp: { message: `failed ${secret}` },
        });
      },
    };
    trace.wrap(failed);
    await assert.rejects(failed.send('Runtime.evaluate', { expression: secret }, sessionId));
    assert.equal(trace.record.calls.at(-1).method, 'Runtime.evaluate');
    assert.equal(trace.record.calls.at(-1).error.message.kind, 'error');
    assert(!JSON.stringify(trace.record).includes(secret));
  } finally {
    await client?.close();
    await trace.close();
    await backend.close();
  }
});
