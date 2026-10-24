import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import './watchdog.mjs';

test('HAR records Chrome traffic but never protected navigations or fetches, credentials or default bodies', {
  timeout: 180000,
}, async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-chrome-'));
  const hits = [];
  const server = createServer((req, res) => {
    hits.push({
      host: req.headers.host,
      url: req.url,
      authorization: req.headers.authorization,
      cookie: req.headers.cookie,
    });
    const url = new URL(req.url, 'http://example.test');
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader('Access-Control-Allow-Headers', '*');
    if (url.pathname === '/redirect') {
      res.writeHead(302, {
        Location: `http://seven.sliccy.ai:${server.address().port}/protected?redirect-secret=1`,
      });
      res.end('redirect-body-secret');
      return;
    }
    res.setHeader('Set-Cookie', 'har-credential=credential-cookie-secret; Path=/');
    res.setHeader('Content-Type', 'text/html');
    res.end('<!doctype html><title>HAR fixture</title><body>response-body-secret</body>');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  const origin = `http://127.0.0.1:${port}`;
  let browser;
  let raw;
  let held;
  try {
    browser = await launchBrowser([
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1, MAP vault.sliccy.ai 127.0.0.1',
      '--no-proxy-server',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const opened = await harCommand(browser.wsUrl, ['tab-new', 'about:blank'], home);
    assert.equal(opened.code, 0, opened.stderr);
    const target = opened.stdout.match(/targetId: ([^\]]+)\]/)?.[1];
    assert.ok(target, opened.stdout);
    const { sessionId } = await raw.send('Target.attachToTarget', {
      targetId: target,
      flatten: true,
    });
    await raw.send('Page.enable', {}, sessionId);
    const file = path.join(home, 'traffic.har');
    held = harHolder(browser.wsUrl, ['har-start', '--tab', target, file], home);
    await held.event('ready');
    await held.event('paused');
    const initialLoad = raw.event('Page.loadEventFired', sessionId);
    await raw.send('Page.navigate', { url: `${origin}/initial` }, sessionId);
    await initialLoad.seen;
    await held.event('resumed');
    const evaluate = async (expression) => {
      const result = await raw.send(
        'Runtime.evaluate',
        { expression, awaitPromise: true, returnByValue: true },
        sessionId
      );
      assert.equal(result.exceptionDetails, undefined, JSON.stringify(result));
    };
    await evaluate(
      `fetch(${JSON.stringify(`${origin}/ordinary-api`)}, {headers:{Authorization:'Bearer credential-authorization-secret'}}).then(r=>r.text())`
    );
    for (const host of ['seven', 'vault']) {
      const protectedUrl = `http://${host}.sliccy.ai:${port}/protected-fetch?${host}-secret=1`;
      await evaluate(`fetch(${JSON.stringify(protectedUrl)}).then(r=>r.text()).catch(()=>null)`);
    }
    await evaluate(
      `fetch(${JSON.stringify(`${origin}/redirect`)}).then(r=>r.text()).catch(()=>null)`
    );
    const barrierLoad = raw.event('Page.loadEventFired', sessionId);
    await raw.send('Page.navigate', { url: `${origin}/before-protected` }, sessionId);
    await barrierLoad.seen;
    await held.event('checkpoint', (event) => event.entries >= 3);
    const checkpoint = JSON.parse(await readFile(file, 'utf8'));
    for (const pathname of ['/initial', '/ordinary-api', '/redirect']) {
      assert.ok(
        checkpoint.log.entries.some((entry) => entry.request.url === `${origin}${pathname}`),
        JSON.stringify(checkpoint)
      );
    }
    for (const host of ['seven', 'vault']) {
      const before = held.events().filter((event) => event.event === 'paused').length;
      const commit = raw.event(
        'Page.frameNavigated',
        sessionId,
        (params) => !params.frame.parentId
      );
      await raw.send(
        'Page.navigate',
        {
          url: `http://${host}.sliccy.ai:${port}/protected-navigation?${host}-navigation-secret=1`,
        },
        sessionId
      );
      await commit.seen;
      await held.event(
        'paused',
        () => held.events().filter((event) => event.event === 'paused').length > before
      );
      const resumedBefore = held.events().filter((event) => event.event === 'resumed').length;
      const load = raw.event('Page.loadEventFired', sessionId);
      await raw.send('Page.navigate', { url: `${origin}/back-${host}` }, sessionId);
      await load.seen;
      await held.event(
        'resumed',
        () => held.events().filter((event) => event.event === 'resumed').length > resumedBefore
      );
      await evaluate(`fetch(${JSON.stringify(`${origin}/after-${host}`)}).then(r=>r.text())`);
    }
    const stopped = await harCommand(browser.wsUrl, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    const ended = await held.ended;
    assert.equal(ended.code, 0, ended.stderr);
    const har = JSON.parse(await readFile(file, 'utf8'));
    assert.equal(har.log.version, '1.2');
    assert.ok(
      har.log.entries.some((entry) => entry.request.url === `${origin}/ordinary-api`),
      JSON.stringify(har)
    );
    for (const host of ['seven', 'vault']) {
      assert.equal(
        har.log.entries.filter(
          (entry) => new URL(entry.request.url).hostname === `${host}.sliccy.ai`
        ).length,
        0
      );
      assert.ok(
        har.log.entries.some((entry) => entry.request.url === `${origin}/after-${host}`),
        JSON.stringify(har)
      );
    }
    const redirect = har.log.entries.find((entry) => entry.request.url === `${origin}/redirect`);
    assert.ok(redirect, JSON.stringify(har));
    assert.equal(redirect.response.status, 302);
    assert.equal(
      redirect.response.headers.find((header) => header.name.toLowerCase() === 'location').value,
      `http://seven.sliccy.ai:${port}`
    );
    for (const entry of har.log.entries) {
      assert.equal(entry.response.content.text, undefined);
      assert.equal(entry.request.postData, undefined);
      assert.deepEqual(entry.request.cookies, []);
      assert.deepEqual(entry.response.cookies, []);
      assert.ok(
        ![...entry.request.headers, ...entry.response.headers].some((header) =>
          /^(cookie|set-cookie|authorization|proxy-authorization)$/i.test(header.name)
        )
      );
    }
    for (const secret of [
      'response-body-secret',
      'credential-cookie-secret',
      'credential-authorization-secret',
      'redirect-secret',
      'seven-secret',
      'vault-secret',
      'seven-navigation-secret',
      'vault-navigation-secret',
    ]) {
      assert.ok(!JSON.stringify(har).includes(secret), secret);
    }
    assert.ok(
      hits.some(
        (hit) =>
          hit.url === '/ordinary-api' &&
          hit.authorization === 'Bearer credential-authorization-secret'
      )
    );
    assert.ok(
      hits.some(
        (hit) => hit.url === '/ordinary-api' && hit.cookie?.includes('credential-cookie-secret')
      )
    );
    for (const host of ['seven', 'vault'])
      assert.ok(
        hits.some(
          (hit) => hit.host?.startsWith(`${host}.sliccy.ai:`) && hit.url.includes('protected-fetch')
        ),
        JSON.stringify(hits)
      );
  } catch (error) {
    const output = held?.output();
    const context = [
      error.message,
      `holder stdout:\n${output?.stdout ?? ''}`,
      `holder stderr:\n${output?.stderr ?? ''}`,
      `fixture hits: ${JSON.stringify(hits.map((hit) => `${hit.host}${hit.url}`))}`,
    ].join('\n');
    error.stack = `${error.stack}\n${context}`;
    error.message = context;
    throw error;
  } finally {
    held?.child.kill('SIGKILL');
    raw?.close();
    await browser?.close();
    await new Promise((resolve) => {
      server.closeAllConnections();
      server.close(resolve);
    });
    await rm(home, { recursive: true, force: true });
  }
});
