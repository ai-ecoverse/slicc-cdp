import assert from 'node:assert/strict';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import browser from '../../packages/sliccy-browser/src/browser.js';

test('sliccy:browser localStorage preserves stored strings', async () => {
  const values = new Map();
  const context = {
    window: {
      localStorage: {
        setItem: (key, value) => values.set(key, String(value)),
        getItem: (key) => values.get(key) ?? null,
      },
    },
  };
  const calls = [];
  const closed = Promise.withResolvers();
  const previousWebSocket = globalThis.WebSocket;
  globalThis.WebSocket = class extends EventTarget {
    constructor(url) {
      super();
      assert.equal(url, 'ws://memory.invalid');
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }

    send(raw) {
      const message = JSON.parse(raw);
      calls.push(message);
      const { id, method, params, sessionId } = message;
      let result = {};
      if (method === 'Target.attachToTarget') {
        assert.deepEqual(params, { targetId: 'tab', flatten: true });
        result = { sessionId: 'session' };
      } else {
        assert.equal(sessionId, 'session');
        if (method === 'Runtime.evaluate') {
          assert.equal(params.returnByValue, true);
          const value = runInNewContext(params.expression, context);
          result = { result: { type: typeof value, value } };
        } else {
          assert.ok(['Page.enable', 'Runtime.enable', 'Network.enable'].includes(method));
        }
      }
      queueMicrotask(() =>
        this.dispatchEvent(
          new MessageEvent('message', {
            data: JSON.stringify({ id, sessionId, result }),
          })
        )
      );
    }

    close() {
      closed.resolve();
    }
  };
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = 'ws://memory.invalid';
  try {
    const json = ' { "workspace": "E02", "items": [1, 2] }\n';
    for (const value of [
      json,
      '[1,2]',
      '"quoted"',
      '"{\\"nested\\":true}"',
      '123',
      'true',
      'null',
      '',
      'plain',
    ]) {
      await browser.eval('tab', `window.localStorage.setItem('config', ${JSON.stringify(value)})`);
      const raw = await browser.localStorage({ targetId: 'tab' }, 'config');
      assert.equal(raw, value);
      if (value === json) assert.deepEqual(JSON.parse(raw), { workspace: 'E02', items: [1, 2] });
    }
    assert.equal(await browser.localStorage('tab', 'missing'), null);
    assert.deepEqual(await browser.eval('tab', `(${JSON.stringify(json)})`), {
      workspace: 'E02',
      items: [1, 2],
    });
    assert.deepEqual(await browser.evalAsync('tab', `(${JSON.stringify(json)})`), {
      workspace: 'E02',
      items: [1, 2],
    });
    assert.ok(
      calls.some((call) => call.method === 'Runtime.evaluate' && call.params.awaitPromise === false)
    );
  } finally {
    await closed.promise;
    globalThis.WebSocket = previousWebSocket;
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
  }
});
