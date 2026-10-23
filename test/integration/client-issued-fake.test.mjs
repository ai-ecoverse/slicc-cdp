import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { before, test } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { guarded } from './guard-client.mjs';

const REDIRECTS = [301, 302, 303, 307, 308];
const CHAINS = [[301], [302], [303], [307], [308], [301, 302, 303, 307, 308]];
const PROTECTED = ['seven.sliccy.ai', 'vault.sliccy.ai'];

before(() => {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    {
      stdio: 'inherit',
    }
  );
});

function glob(pattern, text) {
  let source = '';
  for (let index = 0; index < pattern.length; index += 1) {
    const ch = pattern[index];
    if (ch === '*') source += '.*';
    else if (ch === '?') source += '.';
    else {
      const literal = ch === '\\' && index + 1 < pattern.length ? pattern[++index] : ch;
      source += literal.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
    }
  }
  return new RegExp(`^${source}$`, 's').test(text);
}

function header(headers, name) {
  return (headers ?? []).find((field) => field.name.toLowerCase() === name)?.value;
}

async function world(t) {
  const hits = [];
  const protectedServer = createServer((req, res) => {
    hits.push({ host: req.headers['x-host'], url: req.url });
    res.end('protected');
  });
  const ordinaryHits = [];
  let base;
  const ordinary = createServer((req, res) => {
    ordinaryHits.push(req.url);
    const url = new URL(req.url, base);
    const host = url.searchParams.get('host') ?? 'ordinary';
    const end =
      host === 'ordinary'
        ? `${base}/landing`
        : `http://${host}/${url.searchParams.get('path') ?? 'protected'}`;
    if (url.pathname === '/landing') return res.end('landing');
    if (url.pathname.endsWith('/auth') && !req.headers.authorization) {
      res.writeHead(401, { 'WWW-Authenticate': 'Basic realm="r"' });
      return res.end();
    }
    if (url.pathname.endsWith('/flip')) {
      res.writeHead(302, { Location: end });
      return res.end();
    }
    const statuses = (url.searchParams.get('statuses') ?? '302').split(',').map(Number);
    const location =
      statuses.length > 1
        ? `${base}/hop?statuses=${statuses.slice(1).join(',')}&host=${host}&path=${url.searchParams.get('path') ?? 'protected'}`
        : end;
    res.writeHead(statuses[0], { Location: location });
    res.end();
  });
  await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
  await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
  base = `http://127.0.0.1:${ordinary.address().port}`;
  const protectedBase = `http://127.0.0.1:${protectedServer.address().port}`;
  let session;
  let send;
  let patterns = [];
  let handleAuth = false;
  let seq = 0;
  const pending = new Map();
  const answers = new Set([
    'Fetch.continueRequest',
    'Fetch.failRequest',
    'Fetch.fulfillRequest',
    'Fetch.continueWithAuth',
    'Fetch.continueResponse',
  ]);
  const intercepted = (url, type) =>
    patterns.some(
      (pattern) =>
        (pattern.requestStage ?? 'Request') === 'Request' &&
        (!pattern.resourceType || pattern.resourceType === type) &&
        glob(pattern.urlPattern ?? '*', url)
    );
  const wait = (event, params) =>
    new Promise((resolve) => {
      pending.set(params.requestId, resolve);
      send({ method: event, sessionId: session, params });
    });
  const request = async (url, headers = {}) => {
    const parsed = new URL(url);
    const remote = parsed.hostname.endsWith('sliccy.ai');
    const response = await fetch(remote ? `${protectedBase}${parsed.pathname}` : url, {
      redirect: 'manual',
      headers: { ...headers, ...(remote ? { 'x-host': parsed.hostname } : {}) },
    });
    return {
      status: response.status,
      location: response.headers.get('location'),
      body: await response.text(),
    };
  };
  const load = async (url, type = 'Fetch') => {
    const networkId = `N${++seq}`;
    let current = url;
    let redirectedRequestId;
    for (let hop = 0; hop < 20; hop += 1) {
      const requestId = `F${++seq}`;
      let answer;
      const paused = intercepted(current, type);
      if (paused)
        answer = await wait('Fetch.requestPaused', {
          requestId,
          networkId,
          frameId: 'TAB',
          resourceType: type,
          request: { url: current, method: 'GET', headers: {} },
          ...(redirectedRequestId ? { redirectedRequestId } : {}),
        });
      if (answer?.method === 'Fetch.failRequest') return { failed: answer.params.errorReason };
      if (answer?.params?.url) current = answer.params.url;
      let response;
      if (answer?.method === 'Fetch.fulfillRequest')
        response = {
          status: answer.params.responseCode,
          location: header(answer.params.responseHeaders, 'location'),
          body: Buffer.from(answer.params.body ?? '', 'base64').toString(),
        };
      else response = await request(current);
      if (response.status === 401 && paused && handleAuth) {
        const auth = await wait('Fetch.authRequired', {
          requestId,
          frameId: 'TAB',
          resourceType: type,
          request: { url: current, method: 'GET', headers: {} },
          authChallenge: {
            source: 'Server',
            origin: new URL(current).origin,
            scheme: 'basic',
            realm: 'r',
          },
        });
        if (auth.params.authChallengeResponse?.response !== 'ProvideCredentials')
          return { status: 401 };
        response = await request(current, { authorization: 'Basic dTpw' });
      }
      if (REDIRECTS.includes(response.status) && response.location) {
        redirectedRequestId = requestId;
        current = new URL(response.location, current).href;
        continue;
      }
      return response;
    }
    throw new Error('redirect limit');
  };
  let replays = [];
  const backend = await fakeCdp({
    intercept(message, reply) {
      if (!session || message.sessionId !== session) return false;
      const ok = () => reply({ id: message.id, sessionId: session, result: {} });
      if (message.method === 'Fetch.enable') {
        send = reply;
        patterns = message.params.patterns ?? [{ urlPattern: '*' }];
        handleAuth = message.params.handleAuthRequests === true;
        ok();
        return true;
      }
      if (message.method === 'Fetch.disable') {
        patterns = [];
        ok();
        return true;
      }
      if (message.method === 'Network.enable') {
        send = reply;
        ok();
        return true;
      }
      if (message.method === 'Network.replayXHR') {
        ok();
        replays.push(load(replayUrl, 'XHR'));
        return true;
      }
      if (!answers.has(message.method)) return false;
      const resolve = pending.get(message.params.requestId);
      if (!resolve) {
        reply({
          id: message.id,
          sessionId: session,
          error: { code: -32602, message: 'Invalid InterceptionId.' },
        });
        return true;
      }
      pending.delete(message.params.requestId);
      ok();
      resolve(message);
      return true;
    },
  });
  let replayUrl;
  backend.targets.push({ targetId: 'TAB', url: `${base}/page`, loaderId: 'L' });
  const client = await guarded(backend.url, 'https://private.example.invalid', {
    variant: 'prompt',
    detachDelay: 0,
  });
  t.after(async () => {
    await client.close();
    await backend.close();
    ordinary.closeAllConnections();
    protectedServer.closeAllConnections();
    await new Promise((resolve) => ordinary.close(resolve));
    await new Promise((resolve) => protectedServer.close(resolve));
  });
  ({ sessionId: session } = await client.send('Target.attachToTarget', {
    targetId: 'TAB',
    flatten: true,
  }));
  const paused = (url) =>
    client.waitEvent(
      (event) => event.method === 'Fetch.requestPaused' && event.params.request.url === url,
      10000
    );
  return {
    base,
    hits,
    ordinaryHits,
    client,
    session,
    load,
    paused,
    send: (method, params) => client.send(method, params, session),
    replay: async (url, chain) => {
      replayUrl = url;
      replays = [];
      for (const [index, hop] of chain.entries())
        send({
          method: 'Network.requestWillBeSent',
          sessionId: session,
          params: {
            requestId: 'X1',
            loaderId: 'L',
            documentURL: `${base}/page`,
            type: 'XHR',
            frameId: 'TAB',
            timestamp: index + 1,
            wallTime: index + 1,
            initiator: { type: 'script' },
            request: { url: hop, method: 'GET', headers: {} },
            ...(index
              ? {
                  redirectResponse: {
                    url: chain[index - 1],
                    status: 302,
                    headers: { Location: hop },
                  },
                }
              : {}),
          },
        });
      send({
        method: 'Network.loadingFinished',
        sessionId: session,
        params: { requestId: 'X1', timestamp: chain.length + 1, encodedDataLength: 1 },
      });
      await new Promise((resolve) => setTimeout(resolve, 200));
      return client.send('Network.replayXHR', { requestId: 'X1' }, session).then(
        async () => ({ result: await Promise.all(replays) }),
        (error) => ({ error: error.message })
      );
    },
  };
}

async function start(page, path, options = {}) {
  await page.send('Fetch.enable', {
    patterns: [{ urlPattern: `${page.base}/start*`, requestStage: 'Request' }],
    ...options,
  });
  const url = `${page.base}/start${path}`;
  const seen = page.paused(url);
  const done = page.load(url);
  const event = await seen;
  return { done, requestId: event.params.requestId };
}

for (const host of PROTECTED) {
  for (const chain of CHAINS) {
    test(`fake: url overrides into ${host}, direct or by ${chain.join(' → ')}, are refused before they are sent`, async (t) => {
      const page = await world(t);
      const { done, requestId } = await start(page, '');
      await assert.rejects(
        page.send('Fetch.continueRequest', { requestId, url: `http://${host}/protected` }),
        /slicc: refused:/
      );
      await page.send('Fetch.continueRequest', {
        requestId,
        url: `${page.base}/hop?statuses=${chain.join(',')}&host=${host}`,
      });
      assert.deepEqual(await done, { failed: 'BlockedByClient' });
      assert.deepEqual(page.hits, []);
      assert.equal(page.ordinaryHits.filter((url) => url.startsWith('/hop')).length, chain.length);
    });
  }

  test(`fake: a fulfilled 302 into ${host} is refused, directly or through an ordinary hop`, async (t) => {
    const page = await world(t);
    const first = await start(page, '?n=1');
    for (const location of [
      `http://${host}/protected`,
      `//${host}/protected`,
      `http://${host}:80/x`,
    ]) {
      await assert.rejects(
        page.send('Fetch.fulfillRequest', {
          requestId: first.requestId,
          responseCode: 302,
          responseHeaders: [{ name: 'Location', value: location }],
        }),
        /slicc: refused:/
      );
    }
    await assert.rejects(
      page.send('Fetch.fulfillRequest', {
        requestId: first.requestId,
        responseCode: 302,
        binaryResponseHeaders: Buffer.from(`location: http://${host}/protected`).toString('base64'),
      }),
      /slicc: refused:/
    );
    await page.send('Fetch.failRequest', { requestId: first.requestId, errorReason: 'Failed' });
    await first.done;
    const seen = page.paused(`${page.base}/start?n=2`);
    const done = page.load(`${page.base}/start?n=2`);
    const { params } = await seen;
    await page.send('Fetch.fulfillRequest', {
      requestId: params.requestId,
      responseCode: 302,
      responseHeaders: [{ name: 'Location', value: `/hop?statuses=307,308&host=${host}` }],
    });
    assert.deepEqual(await done, { failed: 'BlockedByClient' });
    assert.deepEqual(page.hits, []);
  });

  test(`fake: continueWithAuth on a request that then redirects into ${host} is refused at the hop`, async (t) => {
    const page = await world(t);
    const { done, requestId } = await start(page, `/auth?host=${host}`, {
      handleAuthRequests: true,
    });
    const auth = page.client.waitEvent((event) => event.method === 'Fetch.authRequired', 10000);
    await page.send('Fetch.continueRequest', { requestId });
    const challenge = await auth;
    await page.send('Fetch.continueWithAuth', {
      requestId: challenge.params.requestId,
      authChallengeResponse: { response: 'ProvideCredentials', username: 'u', password: 'p' },
    });
    assert.deepEqual(await done, { failed: 'BlockedByClient' });
    assert.deepEqual(page.hits, []);
  });

  test(`fake: replayXHR of a request that now redirects into ${host} is refused at the hop`, async (t) => {
    const page = await world(t);
    await page.send('Network.enable', {});
    const url = `${page.base}/flip?host=${host}`;
    const replayed = await page.replay(url, [url, `${page.base}/landing`]);
    assert.deepEqual(replayed, { result: [{ failed: 'BlockedByClient' }] });
    assert.deepEqual(page.hits, []);
    assert.deepEqual(page.ordinaryHits, ['/flip?host=' + host]);
    const tainted = await page.replay(`${page.base}/flip?host=ordinary`, [
      `${page.base}/flip?host=ordinary`,
      `http://${host}/protected`,
    ]);
    assert.match(tainted.error, /slicc: refused:/);
  });
}

test('fake: ordinary client-issued requests and unmodified page traffic stay live', async (t) => {
  const page = await world(t);
  const override = await start(page, '?n=1');
  await page.send('Fetch.continueRequest', {
    requestId: override.requestId,
    url: `${page.base}/hop?statuses=301,302,303,307,308&host=ordinary`,
  });
  assert.equal((await override.done).body, 'landing');
  const seen = page.paused(`${page.base}/start?n=2`);
  const fulfilled = page.load(`${page.base}/start?n=2`);
  const { params } = await seen;
  await page.send('Fetch.fulfillRequest', {
    requestId: params.requestId,
    responseCode: 302,
    responseHeaders: [{ name: 'Location', value: '/hop?statuses=302&host=ordinary' }],
  });
  assert.equal((await fulfilled).body, 'landing');
  const live = page.paused(`${page.base}/start/hop?statuses=302&host=seven.sliccy.ai&path=live`);
  const traffic = page.load(`${page.base}/start/hop?statuses=302&host=seven.sliccy.ai&path=live`);
  await page.send('Fetch.continueRequest', { requestId: (await live).params.requestId });
  assert.equal((await traffic).body, 'protected');
  assert.deepEqual(page.hits, [{ host: 'seven.sliccy.ai', url: '/live' }]);
  await page.send('Network.enable', {});
  const replayed = await page.replay(`${page.base}/flip?host=ordinary`, [
    `${page.base}/flip?host=ordinary`,
    `${page.base}/landing`,
  ]);
  assert.equal(replayed.result[0].body, 'landing');
  await page.send('Fetch.disable', {});
  const after = await page.load(`${page.base}/start/hop?statuses=302&host=ordinary`);
  assert.equal(after.body, 'landing');
});
