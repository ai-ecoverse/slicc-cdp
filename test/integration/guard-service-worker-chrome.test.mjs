import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createServer as https } from 'node:https';
import path from 'node:path';
import { before } from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';

const directory = fileURLToPath(new URL('.', import.meta.url));
const tls = {
  cert: await readFile(path.join(directory, 'fixtures/tls/cert.pem')),
  key: await readFile(path.join(directory, 'fixtures/tls/key.pem')),
};

before(() => {
  if (chromeSkipReason()) return;
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    { stdio: 'inherit' }
  );
});

function swServer() {
  const pages = {
    '/ordinary/': () =>
      `<!doctype html><title>ordinary sw</title><script>
        navigator.serviceWorker.register('/ordinary/sw.js', { scope: '/ordinary/' })
          .then(() => { document.title = 'sw-registered'; })
          .catch((error) => { document.title = 'sw-failed:' + error.message; });
      </script>`,
    '/ordinary/sw.js': () => `self.addEventListener('install', (event) => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(clients.claim()));`,
    '/seven/': () =>
      `<!doctype html><title>seven sw</title><script>
        navigator.serviceWorker.register('/seven/sw.js', { scope: '/seven/' })
          .then(() => { document.title = 'sw-registered'; })
          .catch((error) => { document.title = 'sw-failed:' + error.message; });
      </script>`,
    '/seven/sw.js': () => `self.addEventListener('install', (event) => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(clients.claim()));`,
  };
  const server = https(tls, (req, res) => {
    const page = pages[new URL(req.url ?? '/', 'https://127.0.0.1').pathname];
    if (!page) {
      res.writeHead(404, { 'content-type': 'text/plain' });
      return res.end('missing');
    }
    const body = page();
    res.writeHead(200, {
      'content-type': body.includes('addEventListener') ? 'text/javascript' : 'text/html',
      'Service-Worker-Allowed': '/',
    });
    res.end(body);
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      resolve({
        port: server.address().port,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

async function until(read, accept, label) {
  const deadline = Date.now() + 30000;
  do {
    const value = await read();
    if (accept(value)) return value;
    await new Promise((resolve) => setTimeout(resolve, 100));
  } while (Date.now() < deadline);
  throw new Error(label);
}

async function openPage(raw, url) {
  const created = await raw.send('Target.createTarget', { url: 'about:blank' });
  const attached = await raw.send('Target.attachToTarget', {
    targetId: created.targetId,
    flatten: true,
  });
  await raw.send('Page.enable', {}, attached.sessionId);
  await raw.send('Runtime.enable', {}, attached.sessionId);
  await raw.send('Page.navigate', { url }, attached.sessionId);
  await until(
    async () => {
      const evaluated = await raw.send(
        'Runtime.evaluate',
        { expression: 'document.title', returnByValue: true },
        attached.sessionId
      );
      return evaluated.result?.value;
    },
    (title) => title === 'sw-registered' || String(title || '').startsWith('sw-failed:'),
    `service worker registration on ${url}`
  );
  return { targetId: created.targetId, sessionId: attached.sessionId };
}

test('Chrome: guarded client never sees a protected-origin service worker target', {
  timeout: 180000,
}, async (t) => {
  const server = await swServer();
  let browser;
  let raw;
  let client;
  t.after(async () => {
    await client?.close();
    await browser?.close();
    await server.close();
  });
  browser = await launchBrowser([
    '--ignore-certificate-errors',
    '--no-proxy-server',
    `--host-resolver-rules=MAP ordinary.sw.test 127.0.0.1:${server.port},MAP seven.sliccy.ai 127.0.0.1:${server.port}`,
    '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
  ]);
  raw = await observer(browser.wsUrl);
  client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
    variant: 'prompt',
    detachDelay: 0,
  });
  const ordinaryUrl = `https://ordinary.sw.test:${server.port}/ordinary/`;
  const protectedUrl = `https://seven.sliccy.ai:${server.port}/seven/`;
  const ordinary = await openPage(raw, ordinaryUrl);
  const title = await raw.send(
    'Runtime.evaluate',
    { expression: 'document.title', returnByValue: true },
    ordinary.sessionId
  );
  assert.equal(title.result.value, 'sw-registered', 'ordinary page must register a service worker');
  const ordinarySw = await until(
    async () =>
      (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.find(
        (target) =>
          target.type === 'service_worker' &&
          String(target.url || '').includes(`ordinary.sw.test:${server.port}/ordinary/sw.js`)
      ),
    Boolean,
    'ordinary service worker target'
  );
  assert.ok(ordinarySw.targetId);
  const protectedPage = await openPage(raw, protectedUrl);
  const protectedTitle = await raw.send(
    'Runtime.evaluate',
    { expression: 'document.title', returnByValue: true },
    protectedPage.sessionId
  );
  assert.equal(
    protectedTitle.result.value,
    'sw-registered',
    'protected origin must register a service worker through the fixture'
  );
  const protectedSw = await until(
    async () =>
      (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.find(
        (target) =>
          target.type === 'service_worker' &&
          String(target.url || '').includes(`seven.sliccy.ai:${server.port}/seven/sw.js`)
      ),
    Boolean,
    'raw protected service worker target'
  );
  const guardedListed = await client.send('Target.getTargets', { filter: [{}] });
  const text = JSON.stringify(guardedListed);
  assert.doesNotMatch(text, /\/seven\/sw\.js/);
  assert.doesNotMatch(text, new RegExp(`seven\\.sliccy\\.ai:${server.port}/`));
  const guardedProtected = (guardedListed.targetInfos || []).filter(
    (target) => target.targetId === protectedSw.targetId
  );
  const protectedOrigin = new URL(protectedUrl).origin;
  for (const target of guardedProtected) {
    assert.equal(target.url, protectedOrigin);
    assert.equal(target.title, protectedOrigin);
  }
  await assert.rejects(
    () =>
      client.send('Target.attachToTarget', {
        targetId: protectedSw.targetId,
        flatten: true,
      }),
    /slicc: refused/
  );
});
