import assert from 'node:assert/strict';
import test from 'node:test';
import { InboundGate } from './inbound-gate.mjs';

for (const side of ['guarded', 'control']) {
  test(`deterministic ${side} BFCache gate holds restore traffic until explicit release`, async () => {
    let clock = 1000;
    const options = {
      protectedOrigin: 'https://private.test',
      ordinaryOrigin: 'https://public.test',
      now: () => clock,
      wait: async (ms) => {
        clock += ms;
      },
    };
    const observerGate = new InboundGate(options);
    const gate =
      side === 'control'
        ? observerGate
        : new InboundGate({
            ...options,
            seedSource: (targetId) => observerGate.observedSeed(targetId),
          });
    observerGate.inbound(
      {
        method: 'Target.attachedToTarget',
        params: { sessionId: 'READER', targetInfo: { targetId: 'PAGE' } },
      },
      () => {}
    );
    for (const url of [
      'https://private.test/bfcache?n=gate-proof',
      'https://public.test/bfcache?n=gate-proof-b',
    ])
      observerGate.inbound(
        { method: 'Page.frameNavigated', sessionId: 'READER', params: { frame: { url } } },
        () => {}
      );
    gate.outbound({ id: 7, method: 'Target.attachToTarget', params: { targetId: 'PAGE' } });
    gate.inbound({ id: 7, result: { sessionId: 'SUBJECT' } }, () => {});
    const hook = gate.arm({ targetId: 'PAGE', sessionId: 'SUBJECT', ms: 1500, side, gate: true });
    assert.equal(hook.protectedObserved, true);
    assert.equal(hook.ordinaryObserved, true);
    const received = [];
    gate.outbound({ id: 10, method: 'Page.getFrameTree', sessionId: 'SUBJECT' });
    gate.inbound({ id: 10, result: {} }, () => received.push('subject reply'));
    gate.inbound(
      {
        method: 'Page.frameNavigated',
        sessionId: 'SUBJECT',
        params: {
          type: 'BackForwardCacheRestore',
          frame: { url: 'https://private.test/bfcache?n=gate-proof' },
        },
      },
      () => received.push('restore')
    );
    gate.inbound({ method: 'Page.loadEventFired', sessionId: 'READER', params: {} }, () =>
      received.push('reader')
    );
    assert.deepEqual(received, side === 'control' ? ['reader'] : []);
    const released = await hook.release();
    assert.equal(released.delayedEvents, 1);
    assert.ok(released.releasedAt >= released.firstDelayedAt + 1500);
    assert.deepEqual(
      received,
      side === 'control'
        ? ['reader', 'subject reply', 'restore']
        : ['subject reply', 'restore', 'reader']
    );
    assert.equal(await hook.release(), released);
    gate.close();
  });
}
