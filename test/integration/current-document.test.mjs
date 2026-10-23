import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';

async function documentServer() {
  const server = createServer((request, response) => {
    response.writeHead(200, {
      'content-type': 'text/html',
      'cache-control': 'public, max-age=3600',
    });
    response.end('<!doctype html><title>Document</title><body>document-value</body>');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    url: `http://127.0.0.1:${server.address().port}/document`,
    close: async () => {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

for (const transport of ['native', 'wasi']) {
  chromeTest(
    `current document admission with delayed events (${transport})`,
    {
      timeout: 180000,
    },
    async () => {
      const args = ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'];
      if (transport === 'wasi') args.push('--target', 'wasm32-wasip1');
      execFileSync('cargo', args, { stdio: 'inherit' });
      const ordinary = await documentServer();
      const protectedPage = await documentServer();
      let browser;
      let raw;
      let proxy;
      let client;
      let timer;
      let armed = false;
      let historyId;
      let restored;
      let releaseHistory;
      const historyComplete = new Promise((resolve) => {
        releaseHistory = resolve;
      });
      const calls = [];
      try {
        browser = await launchBrowser();
        raw = await observer(browser.wsUrl);
        const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
        const { sessionId: rawSession } = await raw.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
        await raw.send('Page.enable', {}, rawSession);
        for (const url of [protectedPage.url, ordinary.url]) {
          const loaded = raw.waitEvent(
            (event) => event.sessionId === rawSession && event.method === 'Page.loadEventFired',
            10000
          );
          await raw.send('Page.navigate', { url }, rawSession);
          await loaded;
        }
        proxy = await cdpProxy(browser.wsUrl, {
          holdEvents: () => armed,
          responseBarrier: (message) => (message.id === historyId ? historyComplete : null),
          observe: (direction, message) => {
            if (direction === 'client') {
              calls.push(message);
              if (
                message.method === 'Runtime.evaluate' &&
                message.params?.expression?.includes('history.back()')
              )
                historyId = message.id;
            } else if (
              armed &&
              message.method === 'Page.frameNavigated' &&
              message.params?.frame?.url === protectedPage.url
            ) {
              restored = message;
              releaseHistory();
            }
          },
        });
        client = await guarded(proxy.url, new URL(protectedPage.url).origin, {
          variant: 'lagging',
          detachDelay: 0,
          transport,
        });
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
        armed = true;
        timer = setTimeout(releaseHistory, 10000);
        const historyFrom = calls.length;
        await client
          .send('Runtime.evaluate', { expression: 'history.back()' }, sessionId)
          .catch((error) => {
            assert.match(error.message, /slicc: refused:/);
          });
        await historyComplete;
        clearTimeout(timer);
        assert.equal(
          restored?.params?.type,
          'BackForwardCacheRestore',
          JSON.stringify({
            commands: calls.slice(historyFrom),
            events: proxy.heldEvents.filter(
              (event) => event.method.startsWith('Page.') || event.method.startsWith('Fetch.')
            ),
          })
        );
        assert.ok(proxy.heldEvents.includes(restored));
        const from = calls.length;
        const result = await client
          .send(
            'Runtime.evaluate',
            {
              expression: 'document.documentElement.innerHTML',
              returnByValue: true,
            },
            sessionId
          )
          .then(
            (value) => ({ value }),
            (error) => ({ error })
          );
        assert.match(result.error?.message ?? '', /slicc: refused:/);
        assert.ok(
          calls
            .slice(historyFrom)
            .some((call) => call.method === 'Page.getFrameTree' && call.sessionId !== sessionId)
        );
        assert.ok(
          !calls
            .slice(from)
            .some((call) => call.method === 'Runtime.evaluate' && call.sessionId === sessionId)
        );
        for (const [method, params] of [
          ['DOM.getDocument', { depth: -1 }],
          ['Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter' }],
        ]) {
          await assert.rejects(client.send(method, params, sessionId), /slicc: refused:/);
          assert.ok(
            !calls
              .slice(from)
              .some((call) => call.method === method && call.sessionId === sessionId)
          );
        }
        armed = false;
        proxy.releaseEvents();
      } finally {
        clearTimeout(timer);
        releaseHistory();
        await client?.close();
        await proxy?.close();
        await raw?.close();
        await browser?.close();
        await protectedPage.close();
        await ordinary.close();
      }
    }
  );
}
