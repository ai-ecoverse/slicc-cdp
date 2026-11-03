import assert from 'node:assert/strict';
import test from 'node:test';
import browser from '../../packages/sliccy-browser/src/browser.js';

test('sliccy:browser combines domain and URL filters', async () => {
  const targets = [
    { targetId: 'first', type: 'page', url: 'https://app.slack.com/client/E01/one' },
    { targetId: 'other', type: 'page', url: 'https://other.example/client/E02/two' },
    { targetId: 'second', type: 'page', url: 'https://app.slack.com/client/E02/two' },
  ];
  const closed = Promise.withResolvers();
  const previousWebSocket = globalThis.WebSocket;
  globalThis.WebSocket = class extends EventTarget {
    constructor(url) {
      super();
      assert.equal(url, 'ws://memory.invalid');
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }

    send(raw) {
      const { id, method } = JSON.parse(raw);
      assert.equal(method, 'Target.getTargets');
      queueMicrotask(() =>
        this.dispatchEvent(
          new MessageEvent('message', {
            data: JSON.stringify({ id, result: { targetInfos: targets } }),
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
    for (const urlMatch of [/\/client\/E02\//, '/client/E02/']) {
      const hit = await browser.findTab({ domain: 'APP.SLACK.COM', urlMatch });
      assert.equal(hit?.targetId, 'second');
    }
    assert.equal(await browser.findTab({ domain: 'app.slack.com', urlMatch: '/missing' }), null);
    assert.equal(await browser.findTab({ domain: 'missing.example', urlMatch: '/client' }), null);
    assert.equal((await browser.findTab({ domain: 'app.slack.com' }))?.targetId, 'first');
    assert.equal((await browser.findTab({ urlMatch: '/client/E02/' }))?.targetId, 'other');
    assert.equal(await browser.findTab({ urlMatch: '/missing' }), null);
    await assert.rejects(
      browser.findTab({ domain: 'app.slack.com', urlMatch: '(' }),
      /invalid url match regex/
    );
    assert.equal(
      (await browser.ensureTab('https://app.slack.com/new', { matchUrl: '/client/E02/' }))
        ?.targetId,
      'other'
    );
    assert.equal((await browser.ensureTab('https://app.slack.com/new'))?.targetId, 'first');
  } finally {
    await closed.promise;
    globalThis.WebSocket = previousWebSocket;
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
  }
});
