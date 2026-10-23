import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { before } from 'node:test';
import { launchBrowser } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { guarded } from './guard-client.mjs';

const CHAINS = [[301], [302], [303], [307], [308], [301, 302, 303, 307, 308]];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

before(() => {
  if (chromeSkipReason()) return;
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync(
      'cargo',
      ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
});

for (const transport of ['native', 'wasi']) {
  test(`client-issued request hops never reach protected hosts through ${transport}`, {
    timeout: 600000,
  }, async (t) => {
    const hits = [];
    const protectedServer = createServer((req, res) => {
      hits.push({ host: req.headers.host, url: req.url });
      res.end('protected');
    });
    const ordinaryHits = [];
    const flips = new Map();
    let base;
    let protectedPort;
    const ordinary = createServer((req, res) => {
      ordinaryHits.push(req.url);
      const url = new URL(req.url, base);
      const host = url.searchParams.get('host') ?? 'ordinary';
      const path = url.searchParams.get('path') ?? 'protected';
      const end =
        host === 'ordinary' ? `${base}/landing` : `http://${host}:${protectedPort}/${path}`;
      if (url.pathname === '/page') {
        res.setHeader('Content-Type', 'text/html');
        return res.end('<!doctype html><title>page</title><p>page</p>');
      }
      if (url.pathname === '/landing') return res.end('landing');
      if (url.pathname.endsWith('/auth') && !req.headers.authorization) {
        res.writeHead(401, { 'WWW-Authenticate': 'Basic realm="client-issued"' });
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
      res.writeHead(statuses[0], {
        Location:
          statuses.length > 1
            ? `${base}/hop?statuses=${statuses.slice(1).join(',')}&host=${host}&path=${path}`
            : end,
      });
      res.end();
    });
    const previousTransport = process.env.SLICC_GUARD_TRANSPORT;
    let browser;
    let client;
    try {
      await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
      await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
      protectedPort = protectedServer.address().port;
      base = `http://127.0.0.1:${ordinary.address().port}`;
      browser = await launchBrowser([
        '--host-resolver-rules=MAP *.sliccy.ai 127.0.0.1',
        '--no-proxy-server',
      ]);
      process.env.SLICC_GUARD_TRANSPORT = transport;
      client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
        variant: 'prompt',
        detachDelay: 0,
      });
      const tab = async (fetchParams) => {
        const { targetId } = await client.send('Target.createTarget', { url: `${base}/page` });
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
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
              event.params.request.url === `${base}${path}`,
            20000
          );
        return { sessionId, send, evaluate, load, paused };
      };
      const pattern = { patterns: [{ urlPattern: `${base}/start*`, requestStage: 'Request' }] };
      const intercept = async (page, path) => {
        const seen = page.paused(path);
        const done = page.load(path);
        return { done, requestId: (await seen).params.requestId };
      };
      const noHits = (label) => assert.deepEqual(hits, [], `${label} reached a protected host`);
      for (const host of ['seven.sliccy.ai', 'vault.sliccy.ai']) {
        await t.test(`url overrides into ${host}`, async () => {
          const page = await tab(pattern);
          for (const chain of CHAINS) {
            const path = `/start?chain=${chain.join('-')}&host=${host}`;
            const { done, requestId } = await intercept(page, path);
            await assert.rejects(
              page.send('Fetch.continueRequest', {
                requestId,
                url: `http://${host}:${protectedPort}/protected`,
              }),
              /slicc: refused:/
            );
            const before = ordinaryHits.length;
            await page.send('Fetch.continueRequest', {
              requestId,
              url: `${base}/hop?statuses=${chain.join(',')}&host=${host}`,
            });
            assert.match(await done, /^failed:/);
            noHits(`${chain.join(' → ')} into ${host}`);
            assert.equal(
              ordinaryHits.slice(before).filter((url) => url.startsWith('/hop?')).length,
              chain.length
            );
          }
        });
        await t.test(`fulfilled redirects into ${host}`, async () => {
          const page = await tab(pattern);
          const direct = await intercept(page, `/start?fulfil=direct&host=${host}`);
          await assert.rejects(
            page.send('Fetch.fulfillRequest', {
              requestId: direct.requestId,
              responseCode: 302,
              responseHeaders: [
                { name: 'Location', value: `http://${host}:${protectedPort}/protected` },
              ],
            }),
            /slicc: refused:/
          );
          await page.send('Fetch.failRequest', {
            requestId: direct.requestId,
            errorReason: 'Failed',
          });
          assert.match(await direct.done, /^failed:/);
          const hop = await intercept(page, `/start?fulfil=hop&host=${host}`);
          await page.send('Fetch.fulfillRequest', {
            requestId: hop.requestId,
            responseCode: 302,
            responseHeaders: [{ name: 'Location', value: `/hop?statuses=307,308&host=${host}` }],
          });
          assert.match(await hop.done, /^failed:/);
          noHits(`fulfilled 302 into ${host}`);
        });
        await t.test(`continueWithAuth then a redirect into ${host}`, async () => {
          const page = await tab({ ...pattern, handleAuthRequests: true });
          const challenged = client.waitEvent(
            (event) => event.sessionId === page.sessionId && event.method === 'Fetch.authRequired',
            20000
          );
          const { done, requestId } = await intercept(page, `/start/auth?host=${host}`);
          await page.send('Fetch.continueRequest', { requestId });
          const challenge = await challenged;
          await page.send('Fetch.continueWithAuth', {
            requestId: challenge.params.requestId,
            authChallengeResponse: { response: 'ProvideCredentials', username: 'u', password: 'p' },
          });
          assert.match(await done, /^failed:/);
          noHits(`continueWithAuth into ${host}`);
        });
        await t.test(`replayXHR of a request that now redirects into ${host}`, async () => {
          const page = await tab();
          const key = `replay-${host}-${transport}`;
          const url = `${base}/flip?host=${host}&key=${key}`;
          assert.equal(
            await page.evaluate(
              `new Promise((resolve) => { const xhr = new XMLHttpRequest(); xhr.open('GET', ${JSON.stringify(url)}); xhr.onloadend = () => resolve(xhr.responseText); xhr.send(); })`
            ),
            'landing'
          );
          const sent = client.events.find(
            (event) =>
              event.sessionId === page.sessionId &&
              event.method === 'Network.requestWillBeSent' &&
              event.params.request.url === url
          );
          assert.ok(sent, 'the XHR must be observed');
          await page.send('Network.replayXHR', { requestId: sent.params.requestId });
          const deadline = Date.now() + 10000;
          while ((flips.get(key) ?? 0) < 2 && Date.now() < deadline) await delay(50);
          await delay(1500);
          assert.equal(flips.get(key), 2, 'the replay must send its ordinary first hop');
          noHits(`replayXHR into ${host}`);
        });
      }
      await t.test(
        'ordinary client-issued requests and unmodified page traffic stay live',
        async () => {
          const page = await tab({ ...pattern, handleAuthRequests: true });
          const override = await intercept(page, '/start?control=override');
          await page.send('Fetch.continueRequest', {
            requestId: override.requestId,
            url: `${base}/hop?statuses=301,302,303,307,308&host=ordinary`,
          });
          assert.equal(await override.done, 'landing');
          const fulfilled = await intercept(page, '/start?control=fulfil');
          await page.send('Fetch.fulfillRequest', {
            requestId: fulfilled.requestId,
            responseCode: 302,
            responseHeaders: [{ name: 'Location', value: '/hop?statuses=302&host=ordinary' }],
          });
          assert.equal(await fulfilled.done, 'landing');
          const challenged = client.waitEvent(
            (event) => event.sessionId === page.sessionId && event.method === 'Fetch.authRequired',
            20000
          );
          const authed = await intercept(page, '/start/auth?host=ordinary');
          await page.send('Fetch.continueRequest', { requestId: authed.requestId });
          await page.send('Fetch.continueWithAuth', {
            requestId: (await challenged).params.requestId,
            authChallengeResponse: { response: 'ProvideCredentials', username: 'u', password: 'p' },
          });
          assert.equal(await authed.done, 'landing');
          assert.equal(await page.load('/hop?statuses=302&host=ordinary'), 'landing');
          const live = await intercept(
            page,
            '/start/hop?statuses=302&host=seven.sliccy.ai&path=live'
          );
          await page.send('Fetch.continueRequest', { requestId: live.requestId });
          await live.done;
          assert.deepEqual(
            hits.map((hit) => hit.url),
            ['/live'],
            'an unmodified continue stays page traffic'
          );
          hits.length = 0;
          const key = `replay-ordinary-${transport}`;
          const url = `${base}/flip?host=ordinary&key=${key}`;
          await page.evaluate(
            `new Promise((resolve) => { const xhr = new XMLHttpRequest(); xhr.open('GET', ${JSON.stringify(url)}); xhr.onloadend = () => resolve(xhr.responseText); xhr.send(); })`
          );
          const sent = client.events.find(
            (event) =>
              event.sessionId === page.sessionId &&
              event.method === 'Network.requestWillBeSent' &&
              event.params.request.url === url
          );
          const landings = ordinaryHits.filter((hit) => hit === '/landing').length;
          await page.send('Network.replayXHR', { requestId: sent.params.requestId });
          const deadline = Date.now() + 10000;
          while (
            ordinaryHits.filter((hit) => hit === '/landing').length === landings &&
            Date.now() < deadline
          )
            await delay(50);
          assert.equal(flips.get(key), 2);
          assert.equal(ordinaryHits.filter((hit) => hit === '/landing').length, landings + 1);
          await page.send('Fetch.disable', {});
          assert.equal(await page.load('/start/hop?statuses=302&host=ordinary'), 'landing');
          assert.deepEqual(hits, []);
        }
      );
    } finally {
      if (previousTransport === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
      else process.env.SLICC_GUARD_TRANSPORT = previousTransport;
      await client?.close();
      await browser?.close();
      ordinary.closeAllConnections();
      protectedServer.closeAllConnections();
      await new Promise((resolve) => ordinary.close(resolve));
      await new Promise((resolve) => protectedServer.close(resolve));
    }
  });
}
