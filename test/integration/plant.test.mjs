import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { plantOrigins } from './plant.mjs';
import './watchdog.mjs';

const lagMs = 400;

test('planting waits for a Set-Cookie response that arrives 400 ms late', async () => {
  const served = [];
  const server = createServer((req, res) => {
    if (req.url !== '/plant') {
      res.writeHead(404);
      res.end();
      return;
    }
    served.push(Date.now());
    setTimeout(() => {
      res.writeHead(200, {
        'content-type': 'text/html',
        'set-cookie': ['canary=planted; Path=/', 'del=1; Path=/'],
      });
      res.end('<!doctype html><title>plant</title><p>planted</p>');
    }, lagMs);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  try {
    browser = await launchBrowser();
    const started = Date.now();
    await plantOrigins(browser.wsUrl, [origin]);
    assert.equal(served.length, 1);
    assert.ok(Date.now() - started >= lagMs, 'planting returned before the response arrived');
    const raw = await rawSocket(browser.wsUrl);
    try {
      const { cookies } = await raw.send('Storage.getCookies', {});
      const names = cookies.filter((cookie) => cookie.domain === '127.0.0.1').map((c) => c.name);
      assert.ok(names.includes('canary'), `canary cookie missing: ${names}`);
      assert.ok(names.includes('del'), `del cookie missing: ${names}`);
    } finally {
      raw.close();
    }
  } finally {
    await browser?.close();
    server.closeAllConnections();
    server.close();
  }
});
