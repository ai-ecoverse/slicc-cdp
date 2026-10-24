import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';
import './watchdog.mjs';

async function site(page) {
  const server = createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(`<!doctype html>${page}`);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    port: server.address().port,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(resolve);
      }),
  };
}

async function until(run) {
  const end = Date.now() + 15000;
  do {
    const result = await run();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < end);
  throw new Error('Runtime fixture deadline');
}

test('Chrome: Runtime page scope with a preloaded remote frame', { timeout: 120000 }, async () => {
  const child = await site('<title>frame fixture</title><p>Frame</p>');
  const parent = await site(
    `<title>runtime fixture</title><iframe src="http://localhost:${child.port}/"></iframe>`
  );
  let browser;
  let raw;
  let client;
  try {
    browser = await launchBrowser(['--site-per-process'], { headless: true });
    raw = await observer(browser.wsUrl);
    const { targetId } = await raw.send('Target.createTarget', {
      url: `http://127.0.0.1:${parent.port}/`,
    });
    const rawSession = (await raw.send('Target.attachToTarget', { targetId, flatten: true }))
      .sessionId;
    const remote = await until(async () =>
      (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.find(
        (target) => target.type === 'iframe' && target.url === `http://localhost:${child.port}/`
      )
    );
    assert.ok(remote.targetId);
    const control = await raw.send(
      'Runtime.evaluate',
      { expression: 'document.title', returnByValue: true },
      rawSession
    );
    assert.equal(control.result.value, 'runtime fixture');
    for (const locked of [false, true]) {
      client = await guarded(
        browser.wsUrl,
        locked ? `http://localhost:${child.port}` : 'http://localhost:1',
        { variant: 'prompt', detachDelay: 0 }
      );
      const { sessionId } = await client.send('Target.attachToTarget', {
        targetId,
        flatten: true,
      });
      const methods = [
        ['Runtime.evaluate', { expression: 'document.title', returnByValue: true }],
        [
          'Runtime.callFunctionOn',
          { functionDeclaration: '() => document.title', returnByValue: true },
        ],
        ['Runtime.getHeapUsage', {}],
        ['Runtime.globalLexicalScopeNames', {}],
      ];
      for (const [method, params] of methods) {
        if (locked) await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
        else {
          const result = await client.send(method, params, sessionId);
          if (method === 'Runtime.evaluate' || method === 'Runtime.callFunctionOn')
            assert.equal(result.result.value, 'runtime fixture');
        }
      }
      await client.close();
      client = undefined;
    }
  } finally {
    await client?.close();
    await raw?.close();
    await browser?.close();
    await parent.close();
    await child.close();
  }
});
