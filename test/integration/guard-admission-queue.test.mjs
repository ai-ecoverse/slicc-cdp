import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const PERSISTENT =
  /^slicc: refused: persistent page capabilities are not supported by slicc-cdp\n$/;

async function heldAdmission(transport) {
  const held = [];
  const state = { session: undefined };
  let hold = false;
  const backend = await fakeCdp({
    intercept: (message, send) => {
      if (!hold || message.method !== 'Page.getFrameTree' || message.sessionId !== state.session)
        return false;
      hold = false;
      held.push(() => {
        const frame = backend.targets[0].frame;
        send({ id: message.id, sessionId: message.sessionId, result: { frameTree: { frame } } });
      });
      return true;
    },
  });
  backend.targets.push({
    targetId: 'TAB',
    type: 'page',
    url: 'https://ordinary.example/page',
    frame: {
      id: 'TAB',
      loaderId: 'L1',
      url: 'https://ordinary.example/page',
      securityOrigin: 'https://ordinary.example',
    },
  });
  const messages = [];
  const client = await guarded(backend.url, 'https://private.example.invalid', {
    variant: 'prompt',
    detachDelay: 0,
    transport,
    onMessage: (message) => messages.push(message),
  });
  state.session = (
    await client.send('Target.attachToTarget', { targetId: 'TAB', flatten: true })
  ).sessionId;
  await client.send('Page.enable', {}, state.session);
  return {
    backend,
    client,
    messages,
    session: state.session,
    arm: () => {
      hold = true;
    },
    waitHeld: async () => {
      const until = Date.now() + 10000;
      while (!held.length) {
        if (Date.now() > until) throw new Error('the admission frame tree was never requested');
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    },
    release: () => {
      for (const send of held.splice(0)) send();
    },
    close: async () => {
      await client.close();
      await backend.close();
    },
  };
}

function settle(promise) {
  return promise.then(
    (value) => ({ value }),
    (error) => ({ error })
  );
}

for (const transport of ['native', 'wasi']) {
  test(`guard bridge ${transport}: a persistent-capability refusal does not wait behind a held admission reply`, {
    timeout: 60000,
  }, async () => {
    const harness = await heldAdmission(transport);
    let released = false;
    let timer;
    try {
      harness.arm();
      const admission = settle(
        harness.client.send('DOM.getDocument', { depth: -1 }, harness.session)
      );
      await harness.waitHeld();
      const refusal = harness.client
        .send('Page.addScriptToEvaluateOnNewDocument', { source: 'window.x = 1' }, harness.session)
        .then(
          () => assert.fail('a persistent capability must stay refused'),
          (error) => ({ error, released })
        );
      timer = setTimeout(() => {
        released = true;
        harness.release();
      }, 4000);
      const refused = await refusal;
      assert.match(refused.error.message, PERSISTENT);
      assert.equal(refused.released, false, 'the refusal waited for the held admission reply');
      clearTimeout(timer);
      released = true;
      harness.release();
      const outcome = await admission;
      assert.equal(outcome.error, undefined, outcome.error?.message);
      assert(
        harness.backend.calls.every(
          (call) => call.method !== 'Page.addScriptToEvaluateOnNewDocument'
        )
      );
    } finally {
      clearTimeout(timer);
      await harness.close();
    }
  });

  test(`guard bridge ${transport}: forwarded commands keep their order behind a held admission`, {
    timeout: 60000,
  }, async () => {
    const harness = await heldAdmission(transport);
    let released = false;
    let timer;
    try {
      const sent = harness.messages.length;
      harness.arm();
      const first = settle(harness.client.send('DOM.getDocument', { depth: -1 }, harness.session));
      await harness.waitHeld();
      const second = settle(harness.client.send('Browser.getVersion'));
      const refusal = harness.client
        .send('Page.addScriptToEvaluateOnNewDocument', { source: 'window.x = 1' }, harness.session)
        .then(
          () => assert.fail('a persistent capability must stay refused'),
          (error) => ({
            error,
            released,
            forwarded: harness.backend.calls.map((call) => call.method),
          })
        );
      const third = settle(harness.client.send('Target.getTargets'));
      timer = setTimeout(() => {
        released = true;
        harness.release();
      }, 4000);
      const refused = await refusal;
      assert.match(refused.error.message, PERSISTENT);
      assert.equal(refused.released, false, 'the refusal waited for the held admission reply');
      assert(!refused.forwarded.includes('DOM.getDocument'));
      assert(!refused.forwarded.includes('Browser.getVersion'));
      assert(!refused.forwarded.includes('Target.getTargets'));
      await new Promise((resolve) => setTimeout(resolve, 300));
      assert(harness.backend.calls.every((call) => call.method !== 'Browser.getVersion'));
      assert(harness.backend.calls.every((call) => call.method !== 'Target.getTargets'));
      clearTimeout(timer);
      released = true;
      harness.release();
      const outcomes = await Promise.all([first, second, third]);
      for (const outcome of outcomes)
        assert.equal(outcome.error, undefined, outcome.error?.message);
      const methods = harness.backend.calls.map((call) => call.method);
      const getDocument = methods.indexOf('DOM.getDocument');
      const getVersion = methods.indexOf('Browser.getVersion');
      const getTargets = methods.lastIndexOf('Target.getTargets');
      assert(
        getDocument >= 0 && getDocument < getVersion && getVersion < getTargets,
        methods.join()
      );
      const later = harness.messages.slice(sent);
      const ids = (stage) =>
        later
          .filter((message) => message.audit?.stage === stage)
          .map((message) => message.audit.id);
      const replies = later
        .filter((message) => message.id !== undefined && !message.audit)
        .map((message) => message.id);
      const [a, b, c, d] = ids('received');
      assert.deepEqual(
        replies.filter((id) => id !== c),
        [a, b, d]
      );
      assert(replies.indexOf(c) < replies.indexOf(a));
      const forwarded = later
        .filter((message) => message.audit?.stage === 'forwarded')
        .map((message) => message.audit.id);
      const owners = forwarded.filter((id, index) => forwarded.indexOf(id) === index);
      assert.deepEqual(owners, [a, b, d]);
      assert(!forwarded.includes(c));
    } finally {
      clearTimeout(timer);
      await harness.close();
    }
  });
}
