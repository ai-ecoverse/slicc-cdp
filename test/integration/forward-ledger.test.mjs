import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { ForwardLedger } from './forward-ledger.mjs';
import { guarded } from './guard-client.mjs';
import { inboundClients } from './inbound-clients.mjs';

function ledger() {
  const value = new ForwardLedger();
  value.message({ audit: { stage: 'ready', version: 1 } });
  value.wait = async () => {};
  return value;
}
const command = (
  id = 1,
  method = 'Runtime.evaluate',
  params = { expression: 'document.title' }
) => ({ id, method, params, sessionId: 'S' });
const received = (value, message) => {
  value.request(message);
  value.message({ audit: { id: message.id, stage: 'received', method: message.method } });
};
const descriptor = (message, sentAt = 0) => ({
  id: message.id,
  method: message.method,
  params: message.params,
  sentAt,
});

test('non-forwarding requires received and terminal facts; metadata sends do not assert a content send', async () => {
  const value = ledger();
  const request = command();
  const audit = value.auditor('S');
  value.request(request);
  assert.equal((await audit({ probes: [descriptor(request)] })).complete, false);
  value.message({ audit: { id: 1, stage: 'received', method: request.method } });
  assert.equal((await audit({ probes: [descriptor(request)] })).complete, false);
  for (const stage of ['attempted', 'forwarded'])
    value.message({ audit: { id: 1, stage, method: 'Page.getFrameTree', upstreamId: 50 } });
  value.message({ audit: { id: 1, stage: 'complete' } });
  const result = await audit({ probes: [descriptor(request)], final: true });
  assert.equal(result.complete, true);
  assert.equal(result.probes[0].forwarded, false);
  assert.equal(result.probes[0].clientRequestId, 1);
});

test('actual forwarding, failed partial writes and malformed lifecycle cannot qualify as non-forwarding', async () => {
  for (const mode of ['forwarded', 'uncertain', 'duplicate', 'late', 'wrong-method']) {
    const value = ledger();
    const request = command();
    received(value, request);
    value.message({ audit: { id: 1, stage: 'attempted', method: request.method, upstreamId: 10 } });
    if (mode !== 'uncertain')
      value.message({
        audit: {
          id: 1,
          stage: 'forwarded',
          method: mode === 'wrong-method' ? 'DOM.getDocument' : request.method,
          upstreamId: 10,
        },
      });
    value.message({ audit: { id: 1, stage: 'complete' } });
    if (mode === 'duplicate') value.message({ audit: { id: 1, stage: 'complete' } });
    if (mode === 'late')
      value.message({
        audit: { id: 1, stage: 'attempted', method: request.method, upstreamId: 11 },
      });
    const result = await value.auditor('S')({ probes: [descriptor(request)], final: true });
    assert.notEqual(result.probes[0].forwarded, false);
    assert.equal(result.complete, mode === 'forwarded');
  }
});

test('audit binds exact params, method, session, timing, and distinct logical probe identities', async () => {
  const value = ledger();
  const request = command();
  received(value, request);
  value.message({ audit: { id: 1, stage: 'complete' } });
  for (const probe of [
    { ...descriptor(request), method: 'DOM.getDocument' },
    { ...descriptor(request), params: { expression: 'private-other' } },
    { ...descriptor(request), sentAt: Date.now() + 10000 },
  ])
    assert.equal((await value.auditor('S')({ probes: [probe] })).complete, false);
  assert.equal((await value.auditor('OTHER')({ probes: [descriptor(request)] })).complete, false);
  const result = await value.auditor('S')({
    probes: [descriptor(request), { ...descriptor(request), id: 2 }],
  });
  assert.equal(result.complete, false);
  assert.throws(() => value.request(request));
  assert.throws(() => value.message({ audit: { id: 999, stage: 'complete' } }));
});

for (const transport of ['native', 'wasi']) {
  test(`actual ${transport} upstream transport distinguishes refused history from forwarded ordinary content`, {
    timeout: 60000,
  }, async () => {
    const backend = await fakeCdp();
    backend.targets.push({
      targetId: 'TAB',
      url: 'https://ordinary.example/',
      frame: {
        id: 'TAB',
        loaderId: 'L',
        url: 'https://ordinary.example/',
        securityOrigin: 'https://ordinary.example',
      },
      history: {
        currentIndex: 1,
        entries: [
          { id: 1, url: 'https://seven.sliccy.ai/' },
          { id: 2, url: 'https://ordinary.example/' },
        ],
      },
    });
    const value = new ForwardLedger();
    let client;
    try {
      client = await guarded(backend.url, 'https://private.example.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        transport,
        onCommand: (message) => value.request(message),
        onMessage: (message) => value.message(message),
      });
      await value.initialized(5000);
      const session = (
        await client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true })
      ).sessionId;
      const params = { entryId: 1 };
      const sentAt = Date.now();
      await assert.rejects(
        client.send('Page.navigateToHistoryEntry', params, session),
        /slicc: refused:/
      );
      const result = await value.auditor(session)({
        historyTrigger: {
          id: 'history-trigger',
          method: 'Page.navigateToHistoryEntry',
          params,
          sentAt,
        },
        probes: [],
        final: false,
      });
      assert.equal(result.historyTrigger.complete, true);
      assert.equal(result.historyTrigger.forwarded, false);
      assert(!backend.calls.some((message) => message.method === 'Page.navigateToHistoryEntry'));
      const before = Date.now();
      const expression = { expression: 'document.title', returnByValue: true };
      await client.send('Runtime.evaluate', expression, session);
      const content = await value.auditor(session)({
        probes: [{ id: 1, method: 'Runtime.evaluate', params: expression, sentAt: before }],
        final: true,
      });
      assert.equal(content.complete, true);
      assert.equal(content.probes[0].forwarded, true);
      assert(backend.calls.some((message) => message.method === 'Runtime.evaluate'));
    } finally {
      await client?.close();
      await backend.close();
    }
  });
}

test('held inbound history replies produce terminal real non-forwarding evidence without a reply bypass', {
  timeout: 60000,
}, async () => {
  const protectedOrigin = 'https://seven.sliccy.ai';
  const ordinaryOrigin = 'https://ordinary.example';
  const backend = await fakeCdp();
  let made;
  try {
    made = await inboundClients(backend.url, 'https://private.example.invalid', {
      variant: 'prompt',
      detachDelay: 0,
      protectedOrigin,
      ordinaryOrigin,
    });
    const { targetId } = await made.unguarded.send('Target.createTarget', { url: 'about:blank' });
    const raw = (await made.unguarded.send('Target.attachToTarget', { targetId, flatten: true }))
      .sessionId;
    await made.unguarded.send('Page.enable', {}, raw);
    const protectedURL = `${protectedOrigin}/bfcache?n=audit-seed`;
    const ordinaryURL = `${ordinaryOrigin}/bfcache?n=audit-seed-b`;
    await made.unguarded.send('Page.navigate', { url: protectedURL }, raw);
    await made.unguarded.send('Page.navigate', { url: ordinaryURL }, raw);
    backend.targets[0].history = {
      currentIndex: 1,
      entries: [
        { id: 1, url: protectedURL },
        { id: 2, url: ordinaryURL },
      ],
    };
    const session = (await made.client.send('Target.attachToTarget', { targetId, flatten: true }))
      .sessionId;
    const gate = made.client.inboundDelay({
      client: made.client,
      targetId,
      sessionId: session,
      ms: 1500,
      side: 'guarded',
      gate: true,
    });
    const params = { entryId: 1 };
    const sentAt = Date.now();
    const outcome = made.client.send('Page.navigateToHistoryEntry', params, session);
    const audit = await gate.audit({
      historyTrigger: {
        id: 'history-trigger',
        method: 'Page.navigateToHistoryEntry',
        params,
        sentAt,
      },
      probes: [],
      final: false,
    });
    assert.equal(audit.historyTrigger.complete, true);
    assert.equal(audit.historyTrigger.forwarded, false);
    await assert.rejects(outcome, /slicc: refused:/);
    assert(!backend.calls.some((message) => message.method === 'Page.navigateToHistoryEntry'));
    assert(backend.calls.some((message) => message.method === 'Page.getNavigationHistory'));
    await gate.release();
  } finally {
    await made?.close();
    await backend.close();
  }
});
