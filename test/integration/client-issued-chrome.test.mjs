import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import path from 'node:path';
import { before } from 'node:test';
import { launchBrowser } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { guarded } from './guard-client.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const CHAINS = [[301], [302], [303], [307], [308], [301, 302, 303, 307, 308]];

before(() => {
  if (chromeSkipReason()) return;
  for (const target of [[], ['--target', 'wasm32-wasip1']]) {
    execFileSync(
      'cargo',
      ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
    execFileSync('cargo', ['build', '--locked', '-p', 'slicc-cdp', '-p', 'curlwright', ...target], {
      stdio: 'inherit',
    });
  }
});

async function world(t, transport, guard = true) {
  const hits = [];
  const ordinaryHits = [];
  const flips = new Map();
  let base;
  let protectedPort;
  const protectedServer = createServer((req, res) => {
    hits.push({ host: req.headers.host, url: req.url });
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.end('protected');
  });
  const ordinary = createServer((req, res) => {
    ordinaryHits.push(req.url);
    res.setHeader('Access-Control-Allow-Origin', '*');
    const url = new URL(req.url, base);
    const host = url.searchParams.get('host') ?? 'ordinary';
    const end =
      host === 'ordinary'
        ? `${base}/landing`
        : `http://${host}:${protectedPort}/${url.searchParams.get('path') ?? 'protected'}`;
    if (url.pathname === '/page') {
      res.setHeader('Content-Type', 'text/html');
      return res.end('<!doctype html><title>page</title><p>page</p>');
    }
    if (url.pathname === '/landing') return res.end('landing');
    if (url.pathname.startsWith('/direct/')) return res.end('unrouted');
    if (url.pathname.endsWith('/auth') && !req.headers.authorization) {
      res.writeHead(401, { 'WWW-Authenticate': `Basic realm="${url.pathname}"` });
      return res.end('auth');
    }
    if (url.pathname === '/flip') {
      const key = url.searchParams.get('key');
      const seen = flips.get(key) ?? 0;
      flips.set(key, seen + 1);
      res.writeHead(302, { Location: seen ? end : `${base}/landing` });
      return res.end();
    }
    const statuses = (url.searchParams.get('statuses') ?? '302').split(',').map(Number);
    const peers = url.searchParams.get('peers') === '1';
    const nextBase = peers
      ? `http://hop-${statuses.length % 2}.ordinary.invalid:${ordinary.address().port}`
      : base;
    res.writeHead(statuses[0], {
      Location:
        statuses.length > 1
          ? `${nextBase}${url.pathname}?statuses=${statuses.slice(1).join(',')}&host=${host}&path=${url.searchParams.get('path') ?? 'protected'}&peers=${peers ? 1 : 0}`
          : end,
    });
    res.end();
  });
  const previousTransport = process.env.SLICC_GUARD_TRANSPORT;
  let browser;
  let client;
  t.after(async () => {
    if (previousTransport === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
    else process.env.SLICC_GUARD_TRANSPORT = previousTransport;
    await client?.close();
    await browser?.close();
    ordinary.closeAllConnections();
    protectedServer.closeAllConnections();
    await Promise.all([
      new Promise((resolve) => ordinary.close(resolve)),
      new Promise((resolve) => protectedServer.close(resolve)),
    ]);
  });
  await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
  await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
  protectedPort = protectedServer.address().port;
  base = `http://127.0.0.1:${ordinary.address().port}`;
  browser = await launchBrowser([
    '--host-resolver-rules=MAP *.sliccy.ai 127.0.0.1, MAP *.ordinary.invalid 127.0.0.1',
    '--no-proxy-server',
  ]);
  process.env.SLICC_GUARD_TRANSPORT = transport;
  if (guard)
    client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
      variant: 'prompt',
      detachDelay: 0,
    });
  const tab = async (fetchParams) => {
    const { targetId } = await client.send('Target.createTarget', { url: `${base}/page` });
    const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
    for (const method of ['Network.enable', 'Page.enable', 'Runtime.enable'])
      await client.send(method, {}, sessionId);
    if (fetchParams) await client.send('Fetch.enable', fetchParams, sessionId);
    const send = (method, params) => client.send(method, params, sessionId);
    const evaluate = (expression) =>
      send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }).then(
        (reply) => reply.result.value
      );
    const load = (path) =>
      evaluate(
        `fetch(${JSON.stringify(`${base}${path}`)}, { cache: 'no-store' }).then((response) => response.text(), (error) => 'failed:' + error.message)`
      );
    const paused = (path) =>
      client.waitEvent(
        (event) =>
          event.sessionId === sessionId &&
          event.method === 'Fetch.requestPaused' &&
          !event.params.responseStatusCode &&
          event.params.request.url === `${base}${path}`,
        20000
      );
    const intercept = async (path) => {
      const seen = paused(path);
      const done = load(path);
      return { done, requestId: (await seen).params.requestId };
    };
    const challenge = () =>
      client.waitEvent(
        (event) => event.sessionId === sessionId && event.method === 'Fetch.authRequired',
        20000
      );
    const xhr = (url) =>
      evaluate(
        `new Promise((resolve) => { const xhr = new XMLHttpRequest(); xhr.open('GET', ${JSON.stringify(url)}); xhr.onloadend = () => resolve(xhr.responseText); xhr.send(); })`
      );
    return { sessionId, send, evaluate, load, intercept, challenge, xhr };
  };
  const replay = async (page, host) => {
    const key = `${host}-${transport}`;
    const url = `${base}/flip?host=${host}&key=${key}`;
    assert.equal(await page.xhr(url), 'landing');
    const sent = client.events.find(
      (event) =>
        event.sessionId === page.sessionId &&
        event.method === 'Network.requestWillBeSent' &&
        event.params.request.url === url
    );
    assert.ok(sent, 'the XHR must be observed');
    await assert.rejects(
      page.send('Network.replayXHR', { requestId: sent.params.requestId }),
      /slicc: refused:/
    );
    assert.equal(flips.get(key), 1, 'an uncorrelated replay must not be sent');
  };
  return {
    base,
    protectedPort,
    hits,
    ordinaryHits,
    browser,
    client,
    tab,
    replay,
    pattern: { patterns: [{ urlPattern: `${base}/start*`, requestStage: 'Request' }] },
    noHits: (label) => assert.deepEqual(hits, [], `${label} reached a protected host`),
  };
}

const credentials = (requestId) => ({
  requestId,
  authChallengeResponse: { response: 'ProvideCredentials', username: 'u', password: 'p' },
});

for (const transport of ['native', 'wasi']) {
  for (const host of ['seven.sliccy.ai', 'vault.sliccy.ai']) {
    test(`${transport}: client-issued URL overrides into ${host}`, {
      timeout: 120000,
    }, async (t) => {
      const fixture = await world(t, transport);
      const page = await fixture.tab(fixture.pattern);
      for (const chain of CHAINS) {
        const key = chain.join('-');
        const { done, requestId } = await page.intercept(`/start/${key}`);
        await assert.rejects(
          page.send('Fetch.continueRequest', {
            requestId,
            url: `http://${host}:${fixture.protectedPort}/protected`,
          }),
          /slicc: refused:/
        );
        const before = fixture.ordinaryHits.length;
        await page.send('Fetch.continueRequest', {
          requestId,
          url: `${fixture.base}/hop/${key}?statuses=${chain.join(',')}&host=${host}&peers=1`,
        });
        assert.match(await done, /^failed:/);
        fixture.noHits(`${key} into ${host}`);
        assert.equal(
          fixture.ordinaryHits.slice(before).filter((url) => url.startsWith(`/hop/${key}?`)).length,
          chain.length
        );
      }
    });
    test(`${transport}: client-issued fulfilled redirects into ${host}`, {
      timeout: 120000,
    }, async (t) => {
      const fixture = await world(t, transport);
      const page = await fixture.tab(fixture.pattern);
      const direct = await page.intercept('/start/direct');
      for (const value of [
        `http://${host}:${fixture.protectedPort}/protected`,
        'http://[::1',
        '',
        '/a\u0001b',
      ]) {
        await assert.rejects(
          page.send('Fetch.fulfillRequest', {
            requestId: direct.requestId,
            responseCode: 302,
            responseHeaders: [{ name: 'Location', value }],
          }),
          /slicc: refused:/
        );
      }
      await page.send('Fetch.failRequest', { requestId: direct.requestId, errorReason: 'Failed' });
      assert.match(await direct.done, /^failed:/);
      const hop = await page.intercept('/start/fulfilled');
      await page.send('Fetch.fulfillRequest', {
        requestId: hop.requestId,
        responseCode: 302,
        responseHeaders: [
          { name: 'Location', value: `/hop/fulfilled?statuses=307,308&host=${host}&peers=1` },
        ],
      });
      assert.match(await hop.done, /^failed:/);
      assert.equal(
        fixture.ordinaryHits.filter((url) => url.startsWith('/hop/fulfilled?')).length,
        2
      );
      fixture.noHits(`fulfilled 302 into ${host}`);
    });
    test(`${transport}: client-issued response-stage redirects into ${host}`, {
      timeout: 120000,
    }, async (t) => {
      const fixture = await world(t, transport);
      const page = await fixture.tab({
        patterns: [
          ...fixture.pattern.patterns,
          { urlPattern: `${fixture.base}/start*`, requestStage: 'Response' },
        ],
      });
      const response = fixture.client.waitEvent(
        (event) =>
          event.sessionId === page.sessionId &&
          event.method === 'Fetch.requestPaused' &&
          event.params.responseStatusCode === 302 &&
          event.params.request.url === `${fixture.base}/start/response`,
        20000
      );
      const first = await page.intercept('/start/response');
      await page.send('Fetch.continueRequest', { requestId: first.requestId });
      const { requestId } = (await response).params;
      await assert.rejects(
        page.send('Fetch.continueResponse', {
          requestId,
          responseCode: 302,
          responseHeaders: [
            { name: 'Location', value: `http://${host}:${fixture.protectedPort}/protected` },
          ],
        }),
        /slicc: refused:/
      );
      await assert.rejects(
        page.send('Fetch.fulfillRequest', {
          requestId,
          responseCode: 302,
          body: '',
          responseHeaders: [
            { name: 'Location', value: `http://${host}:${fixture.protectedPort}/protected` },
          ],
        }),
        /slicc: refused:/
      );
      await page.send('Fetch.fulfillRequest', {
        requestId,
        responseCode: 302,
        body: '',
        responseHeaders: [
          { name: 'Location', value: `/hop/response?statuses=307,308&host=${host}&peers=1` },
        ],
      });
      assert.match(await first.done, /^failed:/);
      assert.equal(
        fixture.ordinaryHits.filter((url) => url.startsWith('/hop/response?')).length,
        2
      );
      fixture.noHits(`response-stage redirect into ${host}`);
    });
    test(`${transport}: client-issued auth redirects into ${host}`, {
      timeout: 120000,
    }, async (t) => {
      const fixture = await world(t, transport);
      const page = await fixture.tab({ ...fixture.pattern, handleAuthRequests: true });
      const challenged = page.challenge();
      const { done, requestId } = await page.intercept(`/start/${host}/auth?host=${host}`);
      await page.send('Fetch.continueRequest', { requestId });
      await page.send('Fetch.continueWithAuth', credentials((await challenged).params.requestId));
      assert.match(await done, /^failed:/);
      fixture.noHits(`continueWithAuth into ${host}`);
    });
    test(`${transport}: client-issued replay into ${host} is refused before sending`, {
      timeout: 120000,
    }, async (t) => {
      const fixture = await world(t, transport);
      const page = await fixture.tab();
      await fixture.replay(page, host);
      fixture.noHits(`replayXHR into ${host}`);
    });
  }
  test(`${transport}: ordinary client-issued controls and unmodified page traffic stay live`, {
    timeout: 120000,
  }, async (t) => {
    const fixture = await world(t, transport);
    const page = await fixture.tab({ ...fixture.pattern, handleAuthRequests: true });
    const override = await page.intercept('/start/override');
    await page.send('Fetch.continueRequest', {
      requestId: override.requestId,
      url: `${fixture.base}/hop/override?statuses=301,302,303,307,308&host=ordinary&peers=1`,
    });
    assert.equal(await override.done, 'landing');
    assert.equal(fixture.ordinaryHits.filter((url) => url.startsWith('/hop/override?')).length, 5);
    const fulfilled = await page.intercept('/start/fulfilled');
    await page.send('Fetch.fulfillRequest', {
      requestId: fulfilled.requestId,
      responseCode: 302,
      responseHeaders: [{ name: 'Location', value: '/hop/fulfilled?statuses=302&host=ordinary' }],
    });
    assert.equal(await fulfilled.done, 'landing');
    const challenged = page.challenge();
    const authed = await page.intercept('/start/ordinary/auth?host=ordinary');
    await page.send('Fetch.continueRequest', { requestId: authed.requestId });
    await page.send('Fetch.continueWithAuth', credentials((await challenged).params.requestId));
    assert.equal(await authed.done, 'landing');
    await fixture.replay(page, 'ordinary');
    await page.send('Fetch.disable', {});
    assert.equal(await page.load('/start/disabled?host=ordinary'), 'landing');
    fixture.noHits('ordinary controls');
    await page.send('Fetch.enable', fixture.pattern);
    const live = await page.intercept('/start/live?host=seven.sliccy.ai&path=live');
    await page.send('Fetch.continueRequest', { requestId: live.requestId });
    await live.done;
    assert.deepEqual(
      fixture.hits.map((hit) => hit.url),
      ['/live'],
      'an unmodified continue stays page traffic'
    );
  });
  test(`${transport}: playwright-cli route redirects follow the client-issued rule`, {
    timeout: 120000,
  }, async (t) => {
    const fixture = await world(t, transport, false);
    const runner =
      transport === 'native'
        ? nativeRunner(
            path.resolve('target/debug/playwright-cli'),
            path.resolve('target/debug/curlwright'),
            fixture.browser.wsUrl
          )
        : await kernelRunner(
            path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
            path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
            fixture.browser.wsUrl
          );
    t.after(() => runner.close?.());
    const cwd = await runner.fresh();
    const ok = (result) => {
      assert.equal(result.code, 0, `${result.stderr}\n${result.stdout}`);
      return result.stdout;
    };
    const fetched = async (path) =>
      ok(
        await runner.playwright(
          [
            'eval',
            `fetch(${JSON.stringify(path)}, { cache: 'no-store' }).then((r) => r.text(), (e) => 'failed:' + e.message)`,
          ],
          cwd
        )
      );
    ok(await runner.playwright(['open', `${fixture.base}/page`], cwd));
    for (const host of ['seven', 'vault']) {
      ok(
        await runner.playwright(
          [
            'route',
            `**/routed/${host}*`,
            '--status=302',
            '--header',
            `Location: /hop/${host}?statuses=307,308&host=${host}.sliccy.ai&peers=1`,
          ],
          cwd
        )
      );
      ok(
        await runner.playwright(
          [
            'route',
            `**/direct/${host}*`,
            '--status=302',
            '--header',
            `Location: http://${host}.sliccy.ai:${fixture.protectedPort}/protected`,
          ],
          cwd
        )
      );
      assert.match(await fetched(`/routed/${host}`), /^failed:/);
      assert.equal(fixture.ordinaryHits.filter((url) => url.startsWith(`/hop/${host}?`)).length, 2);
      assert.equal(await fetched(`/direct/${host}`), 'unrouted\n');
      fixture.noHits(`route into ${host}`);
    }
    ok(
      await runner.playwright(
        [
          'route',
          '**/routed/ordinary*',
          '--status=302',
          '--header',
          'Location: /hop/ordinary?statuses=301,302,303,307,308&host=ordinary&peers=1',
        ],
        cwd
      )
    );
    assert.equal(await fetched('/routed/ordinary'), 'landing\n');
    assert.equal(fixture.ordinaryHits.filter((url) => url.startsWith('/hop/ordinary?')).length, 5);
    fixture.noHits('ordinary route control');
    ok(await runner.playwright(['unroute'], cwd));
  });
}
