import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import './watchdog.mjs';

async function withWorkerServer(body) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-worker-chrome-'));
  const server = createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    res.setHeader('Access-Control-Allow-Origin', '*');
    if (url.pathname === '/worker.js') {
      res.setHeader('Content-Type', 'text/javascript');
      res.end(`fetch(${JSON.stringify(`http://127.0.0.1:${server.address().port}/from-worker`)});`);
      return;
    }
    if (url.pathname === '/interval-worker.js') {
      res.setHeader('Content-Type', 'text/javascript');
      res.end(
        `setInterval(() => fetch(${JSON.stringify(`http://127.0.0.1:${server.address().port}/from-worker`)} + '?' + Date.now()), 300);`
      );
      return;
    }
    if (url.pathname === '/from-worker') {
      res.setHeader('Content-Type', 'text/plain');
      res.end('from-worker');
      return;
    }
    if (url.pathname === '/page') {
      res.setHeader('Content-Type', 'text/html');
      res.end(`<!doctype html><title>worker har</title><script>
        new Worker(${JSON.stringify(`http://127.0.0.1:${server.address().port}/worker.js`)});
      </script>`);
      return;
    }
    if (url.pathname === '/interval-page') {
      res.setHeader('Content-Type', 'text/html');
      res.end(`<!doctype html><title>interval worker har</title><script>
        new Worker(${JSON.stringify(`http://127.0.0.1:${server.address().port}/interval-worker.js`)});
      </script>`);
      return;
    }
    res.writeHead(404);
    res.end('missing');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  const origin = `http://127.0.0.1:${port}`;
  let browser;
  let raw;
  let held;
  try {
    browser = await launchBrowser([
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1',
      '--no-proxy-server',
    ]);
    raw = await rawSocket(browser.wsUrl);
    await body({
      home,
      origin,
      port,
      browser,
      raw,
      getHeld: () => held,
      setHeld: (value) => {
        held = value;
      },
    });
  } catch (error) {
    const output = held?.output();
    throw new Error(
      [
        error.message,
        `holder stdout:\n${output?.stdout ?? ''}`,
        `holder stderr:\n${output?.stderr ?? ''}`,
      ].join('\n')
    );
  } finally {
    held?.child.kill('SIGKILL');
    raw?.close?.();
    await browser?.close?.();
    await new Promise((resolve) => server.close(resolve));
    await rm(home, { recursive: true, force: true });
  }
}

test('HAR records dedicated worker fetches and excludes workers of protected pages', {
  timeout: 180000,
}, async () => {
  await withWorkerServer(async ({ home, origin, port, browser, raw, setHeld }) => {
    const opened = await harCommand(browser.wsUrl, ['tab-new', 'about:blank'], home);
    assert.equal(opened.code, 0, opened.stderr);
    const target = opened.stdout.match(/targetId: ([^\]]+)\]/)?.[1];
    assert.ok(target, opened.stdout);
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: target,
      flatten: true,
    });
    await raw.send('Page.enable', {}, sessionId);
    const file = path.join(home, 'worker.har');
    const held = harHolder(browser.wsUrl, ['har-start', '--tab', target, file], home);
    setHeld(held);
    await held.event('ready');
    await held.event('paused');
    const load = raw.event('Page.loadEventFired', sessionId);
    await raw.send('Page.navigate', { url: `${origin}/page` }, sessionId);
    await load.seen;
    await held.event('resumed');
    await new Promise((resolve) => setTimeout(resolve, 1000));
    const beforeProtected = held.events().filter((event) => event.event === 'paused').length;
    const commit = raw.event('Page.frameNavigated', sessionId, (params) => !params.frame.parentId);
    await raw.send('Page.navigate', { url: `http://seven.sliccy.ai:${port}/protected` }, sessionId);
    await commit.seen;
    await held.event(
      'paused',
      () => held.events().filter((event) => event.event === 'paused').length > beforeProtected
    );
    await raw
      .send(
        'Runtime.evaluate',
        {
          expression: `try { new Worker(${JSON.stringify(`${origin}/worker.js`)}) } catch (error) { null }`,
          awaitPromise: true,
        },
        sessionId
      )
      .catch(() => null);
    await new Promise((resolve) => setTimeout(resolve, 500));
    const stopped = await harCommand(browser.wsUrl, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    const ended = await held.ended;
    assert.equal(ended.code, 0, ended.stderr);
    const har = JSON.parse(await readFile(file, 'utf8'));
    const urls = har.log.entries.map((entry) => entry.request.url);
    assert.ok(
      urls.some((url) => url === `${origin}/worker.js`),
      `missing worker script: ${JSON.stringify(har)}`
    );
    assert.ok(
      urls.some((url) => url === `${origin}/from-worker`),
      `missing worker fetch: ${JSON.stringify(har)}`
    );
    assert.equal(
      har.log.entries.filter((entry) => {
        try {
          return new URL(entry.request.url).hostname === 'seven.sliccy.ai';
        } catch {
          return false;
        }
      }).length,
      0,
      JSON.stringify(har)
    );
  });
});

test('HAR records fetches from a worker that already existed before har-start', {
  timeout: 180000,
}, async () => {
  await withWorkerServer(async ({ home, origin, browser, raw, setHeld }) => {
    const opened = await harCommand(browser.wsUrl, ['tab-new', 'about:blank'], home);
    assert.equal(opened.code, 0, opened.stderr);
    const target = opened.stdout.match(/targetId: ([^\]]+)\]/)?.[1];
    assert.ok(target, opened.stdout);
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: target,
      flatten: true,
    });
    await raw.send('Page.enable', {}, sessionId);
    const load = raw.event('Page.loadEventFired', sessionId);
    await raw.send('Page.navigate', { url: `${origin}/interval-page` }, sessionId);
    await load.seen;
    await new Promise((resolve) => setTimeout(resolve, 500));
    const file = path.join(home, 'existing-worker.har');
    const held = harHolder(browser.wsUrl, ['har-start', '--tab', target, file], home);
    setHeld(held);
    await held.event('ready');
    await new Promise((resolve) => setTimeout(resolve, 2000));
    const stopped = await harCommand(browser.wsUrl, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    const ended = await held.ended;
    assert.equal(ended.code, 0, ended.stderr);
    const har = JSON.parse(await readFile(file, 'utf8'));
    assert.ok(
      har.log.entries.some((entry) => {
        try {
          return new URL(entry.request.url).pathname === '/from-worker';
        } catch {
          return false;
        }
      }),
      `missing existing-worker fetch: ${JSON.stringify(har)}`
    );
  });
});
