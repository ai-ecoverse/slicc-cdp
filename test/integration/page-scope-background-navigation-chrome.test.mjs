import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';

test('Chrome: a filtered background iframe can navigate during an ordinary screenshot reply', {
  timeout: 120000,
}, async (t) => {
  const server = createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(
      req.url === '/seven'
        ? `<iframe src="http://localhost:${server.address().port}/child"></iframe>`
        : '<title>Ordinary</title><h1>Ordinary</h1>'
    );
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  const port = server.address().port;
  const browser = await launchBrowser([
    '--site-per-process',
    '--no-proxy-server',
    '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1',
    '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
  ]);
  t.after(() => browser.close());
  const raw = await observer(browser.wsUrl);
  t.after(() => raw.close());
  await raw.send('Target.setDiscoverTargets', { discover: true, filter: [{}] });
  const childEvent = raw.waitEvent(
    (event) =>
      event.params?.targetInfo?.type === 'iframe' &&
      event.params.targetInfo.url === `http://localhost:${port}/child`,
    15000
  );
  const { targetId: background } = await raw.send('Target.createTarget', {
    url: `http://seven.sliccy.ai:${port}/seven`,
  });
  const child = (await childEvent).params.targetInfo;
  assert.ok(child.parentFrameId);
  const { sessionId: childSession } = await raw.send('Target.attachToTarget', {
    targetId: child.targetId,
    flatten: true,
  });
  let active = false;
  let capture;
  let changed;
  let observed = false;
  let turn = 0;
  const proxy = await cdpProxy(browser.wsUrl, {
    transform(message) {
      if (message.result?.targetInfos)
        message.result.targetInfos = message.result.targetInfos.filter(
          (target) => target.targetId !== background
        );
      return message;
    },
    observe(direction, message) {
      if (direction === 'client' && active && message.method === 'Page.captureScreenshot')
        capture = message.id;
      if (
        direction === 'browser' &&
        capture &&
        message.method === 'Target.targetInfoChanged' &&
        message.params?.targetInfo?.targetId === child.targetId
      ) {
        assert.equal(message.params.targetInfo.parentFrameId, child.parentFrameId);
        observed = true;
        changed?.();
      }
    },
    responseBarrier(message) {
      if (!capture || message.id !== capture) return null;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error('background navigation event missing')),
          10000
        );
        changed = () => {
          clearTimeout(timer);
          resolve();
        };
        raw
          .send(
            'Page.navigate',
            {
              url: `http://localhost:${port}/child?turn=${++turn}`,
            },
            childSession
          )
          .catch((error) => {
            clearTimeout(timer);
            reject(error);
          });
      });
    },
  });
  t.after(() => proxy.close());
  const client = await guarded(proxy.url, 'https://protected.test', {
    variant: 'prompt',
    detachDelay: 0,
  });
  t.after(() => client.close());
  const { targetId } = await client.send('Target.createTarget', {
    url: `http://localhost:${port}/ordinary`,
  });
  const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
  await client.send('Page.captureScreenshot', {}, sessionId);
  active = true;
  const result = await client.send('Page.captureScreenshot', {}, sessionId);
  assert.ok(result.data.length > 100);
  assert.equal(observed, true);
});
