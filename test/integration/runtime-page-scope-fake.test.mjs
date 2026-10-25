import assert from 'node:assert/strict';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const ordinary = 'https://ordinary.test';
const protectedOrigin = 'https://protected.test';
const frame = (id, origin, parentId) => ({
  id,
  loaderId: `L${id}`,
  url: `${origin}/`,
  securityOrigin: origin,
  ...(parentId ? { parentId } : {}),
});
const calls = [
  ['Runtime.evaluate', { expression: "'runtime marker'", returnByValue: true }],
  [
    'Runtime.callFunctionOn',
    { functionDeclaration: "() => 'runtime marker'", returnByValue: true },
  ],
  ['Runtime.globalLexicalScopeNames', {}],
  ['Runtime.getHeapUsage', {}],
  ['Runtime.discardConsoleEntries', {}],
];

async function fixture(layout, origin, options = {}) {
  const fake = await fakeCdp(options);
  const page = { targetId: 'P', type: 'page', url: `${ordinary}/`, childFrames: [] };
  fake.targets.push(page, { targetId: 'Q', type: 'page', url: `${ordinary}/control` });
  fake.targets.push({
    targetId: 'SIBLING',
    type: 'iframe',
    parentFrameId: 'P',
    url: `${ordinary}/sibling`,
    tree: { frame: frame('SIBLING', ordinary, 'P') },
  });
  if (layout === 'local') page.childFrames.push({ frame: frame('C', origin, 'P') });
  else {
    fake.targets.push({
      targetId: 'M',
      type: 'iframe',
      parentFrameId: 'P',
      url: `${layout === 'remote' ? origin : ordinary}/`,
      tree: {
        frame: frame('M', layout === 'remote' ? origin : ordinary, 'P'),
        ...(layout === 'nested' ? { childFrames: [{ frame: frame('C', origin, 'M') }] } : {}),
      },
    });
  }
  const client = await guarded(fake.url, protectedOrigin, { variant: 'prompt', detachDelay: 0 });
  const { sessionId } = await client.send('Target.attachToTarget', {
    targetId: 'P',
    flatten: true,
  });
  return { fake, client, page, sessionId };
}

for (const layout of ['local', 'remote', 'nested']) {
  for (const locked of [false, true]) {
    test(`Runtime page scope ${layout} ${locked ? 'refusal' : 'ordinary control'}`, async () => {
      const { fake, client, sessionId } = await fixture(
        layout,
        locked ? protectedOrigin : ordinary
      );
      try {
        for (const [method, params] of calls) {
          const from = fake.calls.length;
          if (locked) {
            await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
            assert.equal(
              fake.calls.slice(from).some((call) => call.method === method),
              false,
              method
            );
          } else await client.send(method, params, sessionId);
        }
        const sibling = (
          await client.send('Target.attachToTarget', { targetId: 'SIBLING', flatten: true })
        ).sessionId;
        if (locked)
          await assert.rejects(client.send('Runtime.getHeapUsage', {}, sibling), /slicc: refused:/);
        else await client.send('Runtime.getHeapUsage', {}, sibling);
        const control = (
          await client.send('Target.attachToTarget', { targetId: 'Q', flatten: true })
        ).sessionId;
        await client.send('Runtime.getHeapUsage', {}, control);
      } finally {
        await client.close();
        await fake.close();
      }
    });
  }
}

for (const reply of ['value', 'exception', 'error']) {
  test(`Runtime page scope verifies an executed ${reply} reply`, async () => {
    let page;
    const {
      fake,
      client,
      page: current,
      sessionId,
    } = await fixture('local', ordinary, {
      intercept(message, send) {
        if (message.method !== 'Runtime.evaluate' || message.params.expression !== 'lateMarker')
          return false;
        page.childFrames.push({ frame: frame('LATE', protectedOrigin, 'P') });
        send({
          method: 'Page.frameAttached',
          sessionId: message.sessionId,
          params: { frameId: 'LATE', parentFrameId: 'P' },
        });
        if (reply === 'error') {
          send({ id: message.id, error: { code: -32000, message: 'runtime marker' } });
          return true;
        }
        send({
          id: message.id,
          result: {
            result: { type: 'string', value: 'runtime marker' },
            ...(reply === 'exception' ? { exceptionDetails: { text: 'runtime marker' } } : {}),
          },
        });
        return true;
      },
    });
    page = current;
    try {
      await assert.rejects(
        client.send(
          'Runtime.evaluate',
          { expression: 'lateMarker', returnByValue: true },
          sessionId
        ),
        /slicc: refused:/
      );
      assert.ok(fake.calls.some((call) => call.params.expression === 'lateMarker'));
    } finally {
      await client.close();
      await fake.close();
    }
  });
}

test('Runtime page scope filters content events after placement', async () => {
  let page;
  const {
    fake,
    client,
    page: current,
    sessionId,
  } = await fixture('local', ordinary, {
    onCommand(message, event) {
      if (message.method !== 'Page.getLayoutMetrics') return;
      page.childFrames.push({ frame: frame('LATE', protectedOrigin, 'P') });
      event('Runtime.consoleAPICalled', {
        type: 'log',
        executionContextId: 1,
        args: [{ type: 'string', value: 'runtime marker' }],
      });
    },
  });
  page = current;
  try {
    await client.send('Runtime.enable', {}, sessionId);
    const from = client.events.length;
    await client.send('Page.getLayoutMetrics', {}, sessionId);
    assert.equal(
      client.events.slice(from).some((event) => event.method === 'Runtime.consoleAPICalled'),
      false
    );
  } finally {
    await client.close();
    await fake.close();
  }
});
