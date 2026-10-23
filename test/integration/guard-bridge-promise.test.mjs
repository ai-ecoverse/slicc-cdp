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
    'quiet-navigation',
    'poll-navigation',
    'poll-refusal',
    'original-refusal',
    'protected-poll',
    'protected-poll-error',
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
          if (message.params.expression.includes('globalThis[key] = { done: false }')) {
            started = true;
            reply(message, send, JSON.stringify({ done: false }));
            if (['navigation', 'quiet-navigation'].includes(mode)) {
              changed = true;
              backend.targets[0].frame = frame('L2');
              if (mode === 'navigation')
                send({
                  method: 'Page.frameNavigated',
                  sessionId: session,
                  params: { frame: backend.targets[0].frame },
                });
            }
            if (mode === 'protected-poll') {
              backend.targets[0].frame = frame('LPROTECTED', 'https://seven.sliccy.ai');
              backend.targets[0].url = backend.targets[0].frame.url;
              send({
                method: 'Page.frameNavigated',
                sessionId: session,
                params: { type: 'BackForwardCacheRestore', frame: backend.targets[0].frame },
              });
            }
            return true;
          }
          if (message.params.expression.startsWith('JSON.stringify(globalThis[')) {
            polls += 1;
            if (mode === 'protected-poll-error') {
              backend.targets[0].frame = frame('LPROTECTED', 'https://seven.sliccy.ai');
              backend.targets[0].url = backend.targets[0].frame.url;
              send({
                method: 'Page.frameNavigated',
                sessionId: session,
                params: { type: 'BackForwardCacheRestore', frame: backend.targets[0].frame },
              });
              send({
                id: message.id,
                sessionId: session,
                error: { code: -32000, message: 'Cannot find context with specified id' },
              });
            } else
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
        } else if (['navigation', 'quiet-navigation', 'poll-navigation'].includes(mode)) {
          await assert.rejects(outcome, (error) => {
            assert.equal(error.message, 'bridge evaluation navigated or superseded');
            assert(!/refused|session.*closed|target.*closed|no session/i.test(error.message));
            return true;
          });
          assert.equal(polls, 0);
          assert(messages.some((message) => message.diagnostic?.phase === 'superseded'));
        } else {
          await assert.rejects(outcome, /slicc: refused:/);
          assert.equal(polls, mode === 'protected-poll-error' ? 1 : 0);
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

for (const transport of ['native', 'wasi']) {
  test(`guard promise bridge ${transport}: synchronous navigation releases paused Fetch input`, {
    timeout: 60000,
  }, async () => {
    let backend;
    let session;
    let started = false;
    let paused = false;
    let rendererProbes = 0;
    let polls = 0;
    let fulfilled = false;
    const heldTrees = [];
    const answers = [];
    let documentReady;
    const documentAnswered = new Promise((resolve) => {
      documentReady = resolve;
    });
    const messages = [];
    let client;
    backend = await fakeCdp({
      intercept: (message, send) => {
        if (message.method === 'Page.getFrameTree' && paused) {
          rendererProbes += 1;
          heldTrees.push(() =>
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { frameTree: { frame: backend.targets[0].frame } },
            })
          );
          return true;
        }
        if (
          message.method === 'Runtime.evaluate' &&
          message.params.expression.includes('globalThis[key] = { done: false }')
        ) {
          started = true;
          paused = true;
          reply(message, send, JSON.stringify({ done: true, value: 'actual-synchronous-outcome' }));
          for (const [requestId, resourceType] of [
            ['BEACON', 'Ping'],
            ['DOCUMENT', 'Document'],
          ])
            send({
              method: 'Fetch.requestPaused',
              sessionId: session,
              params: {
                requestId,
                frameId: 'TAB',
                resourceType,
                request: {
                  url: `https://ordinary.example/${requestId}`,
                  method: 'GET',
                  headers: {},
                },
              },
            });
          return true;
        }
        if (
          message.method === 'Runtime.evaluate' &&
          message.params.expression.startsWith('JSON.stringify(globalThis[')
        ) {
          polls += 1;
          return true;
        }
        if (message.method === 'Fetch.fulfillRequest' && message.params.requestId === 'DOCUMENT') {
          fulfilled = true;
          paused = false;
          backend.targets[0].frame = frame('L2');
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          send({
            method: 'Page.frameNavigated',
            sessionId: session,
            params: { frame: backend.targets[0].frame },
          });
          for (const release of heldTrees.splice(0)) release();
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
    try {
      client = await guarded(backend.url, 'https://private.example.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        transport,
        onMessage: (message) => {
          messages.push(message);
          const event = message.event;
          if (event?.method === 'Fetch.requestPaused' && event.sessionId === session) {
            const document = event.params.requestId === 'DOCUMENT';
            const answer = client.send(
              document ? 'Fetch.fulfillRequest' : 'Fetch.continueRequest',
              document
                ? { requestId: 'DOCUMENT', responseCode: 200, body: 'Zm9yZ2Vk' }
                : { requestId: event.params.requestId },
              session
            );
            answers.push(answer);
            if (document) answer.then(documentReady, documentReady);
          }
        },
      });
      session = (await client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true }))
        .sessionId;
      await client.send('Page.enable', {}, session);
      await client.send(
        'Fetch.enable',
        { patterns: [{ urlPattern: '*', requestStage: 'Request' }] },
        session
      );
      const result = await client.send(
        'Runtime.evaluate',
        {
          expression: 'location.href = "https://ordinary.example/next"',
          awaitPromise: true,
          returnByValue: true,
        },
        session
      );
      assert.equal(result.result.value, 'actual-synchronous-outcome');
      await client.send('Browser.getVersion');
      await documentAnswered;
      await Promise.all(answers);
      assert.equal(started, true);
      assert.equal(fulfilled, true);
      assert.equal(rendererProbes, 0);
      assert.equal(polls, 0);
      assert(
        messages.some(
          (message) => message.diagnostic?.phase === 'original' && !message.diagnostic.error
        )
      );
      assert(
        !messages.some((message) => ['poll', 'superseded'].includes(message.diagnostic?.phase))
      );
    } finally {
      await client?.close();
      await Promise.allSettled(answers);
      await backend.close();
    }
  });
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
  assert.equal(sanitized.method.sha256.length, 64);
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
