import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { before } from 'node:test';
import { freePort, launchBrowser } from './browser.mjs';
import { chromeSkipReason, chromeTest } from './chrome-policy.mjs';
import { guarded } from './guard-client.mjs';

const RUNS = 10;
const POLL = 15;
const WINDOW = 2500;
const SECRET = /QSECRET|path-secret/;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

before(() => {
  if (chromeSkipReason()) return;
  const args = ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'];
  execFileSync('cargo', args, { stdio: 'inherit' });
  execFileSync('cargo', [...args, '--target', 'wasm32-wasip1'], { stdio: 'inherit' });
});

async function redirectServer(protectedPort, ordinaryPort) {
  const server = createServer((request, response) => {
    const path = new URL(request.url, 'http://localhost').pathname;
    if (path === '/r') {
      response.writeHead(302, {
        location: `http://127.0.0.1:${protectedPort}/path-secret?q=QSECRET`,
      });
      response.end();
    } else if (path === '/o') {
      response.writeHead(302, {
        location: `http://localhost:${ordinaryPort}/ordinary-missing?q=VISIBLE`,
      });
      response.end();
    } else {
      response.writeHead(200, { 'content-type': 'text/html' });
      response.end('<!doctype html><title>start</title><body>start</body>');
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    origin: `http://localhost:${server.address().port}`,
    close: async () => {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

async function openStart(client, origin) {
  const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
  await client.send('Page.enable', {}, sessionId);
  await client.send('Runtime.enable', {}, sessionId);
  const loaded = client.waitEvent(
    (event) => event.sessionId === sessionId && event.method === 'Page.loadEventFired',
    15000
  );
  await client.send('Page.navigate', { url: `${origin}/start` }, sessionId);
  await loaded;
  return { targetId, sessionId };
}

async function poll(client, sessionId, path, done) {
  await client.send(
    'Runtime.evaluate',
    { expression: `setTimeout(()=>location.href=${JSON.stringify(path)},0)`, returnByValue: true },
    sessionId
  );
  const replies = [];
  const deadline = Date.now() + WINDOW;
  while (Date.now() < deadline) {
    const reply = await client
      .send(
        'Runtime.evaluate',
        {
          expression:
            "location.href+' '+document.documentElement.outerHTML.match(/QSECRET|path-secret|ordinary-missing|VISIBLE/g)",
          returnByValue: true,
        },
        sessionId
      )
      .then(
        (value) => ({ value: String(value?.result?.value ?? '') }),
        (error) => ({ error: error.message })
      );
    replies.push(reply);
    if (done?.(reply)) break;
    await delay(POLL);
  }
  return replies;
}

for (const transport of ['native', 'wasi']) {
  chromeTest(
    `protected redirect targets stay unreadable while their error document commits (${transport})`,
    { timeout: 240000 },
    async () => {
      const protectedPort = await freePort();
      const ordinaryPort = await freePort();
      const server = await redirectServer(protectedPort, ordinaryPort);
      let browser;
      let client;
      try {
        browser = await launchBrowser();
        client = await guarded(browser.wsUrl, `http://127.0.0.1:${protectedPort}`, {
          variant: 'prompt',
          detachDelay: 0,
          transport,
        });
        const leaks = [];
        let refusals = 0;
        for (let run = 0; run < RUNS; run++) {
          const { targetId, sessionId } = await openStart(client, server.origin);
          const replies = await poll(client, sessionId, '/r');
          leaks.push(...replies.filter((reply) => SECRET.test(reply.value ?? '')));
          refusals += replies.filter((reply) => /slicc: refused:/.test(reply.error ?? '')).length;
          const settled = await client
            .send(
              'Runtime.evaluate',
              { expression: 'document.documentElement.outerHTML', returnByValue: true },
              sessionId
            )
            .then(
              (value) => String(value?.result?.value ?? ''),
              (error) => error.message
            );
          assert.doesNotMatch(settled, SECRET);
          assert.match(settled, /slicc: refused:/);
          await client.send('Target.closeTarget', { targetId });
        }
        assert.deepEqual(leaks, []);
        assert.ok(refusals > 0);
        const { targetId, sessionId } = await openStart(client, server.origin);
        const replies = await poll(client, sessionId, '/o', (reply) =>
          reply.value?.startsWith('chrome-error://chromewebdata/')
        );
        const visible = replies.find((reply) =>
          reply.value?.startsWith('chrome-error://chromewebdata/')
        );
        assert.match(
          visible?.value ?? JSON.stringify(replies.slice(-5)),
          /ordinary-missing|VISIBLE/
        );
        await client.send('Target.closeTarget', { targetId });
      } finally {
        await client?.close();
        await browser?.close();
        await server.close();
      }
    }
  );
}
