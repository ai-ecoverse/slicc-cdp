import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';
import './watchdog.mjs';

const MARKER = 'PROTECTED-BODY-MARKER-IW-UA';
const HEADER = 'x-protected-marker';
const HEADER_VALUE = 'PROTECTED-HEADER-MARKER-IW-UA';
const COOKIE_NAME = 'iw_ua_session';
const COOKIE_VALUE = 'IW-UA-COOKIE-SECRET';
const SAME_ORIGIN_EXPR = 'location.origin + "|" + document.title';

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await raw.send('Page.navigate', { url }, session);
  await loaded;
}

function protectedSite() {
  const hits = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    hits.push({
      path: url.pathname,
      cookie: String(req.headers.cookie || ''),
      origin: String(req.headers.origin || ''),
    });
    if (url.pathname === '/secret') {
      res.writeHead(200, {
        'content-type': 'text/plain; charset=utf-8',
        [HEADER]: HEADER_VALUE,
      });
      res.end(`${MARKER}\n`);
      return;
    }
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      resolve({
        port,
        hits,
        url: `http://seven.sliccy.ai:${port}/secret`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function probeExpression(url, kind) {
  if (kind === 'fetch') {
    return `(() => fetch(${JSON.stringify(url)}, { credentials: 'include', cache: 'no-store' })
      .then(async (response) => {
        const body = await response.text();
        return {
          kind: 'fetch',
          status: response.status,
          ok: response.ok,
          bodyLen: body.length,
          bodyHasMarker: body.includes(${JSON.stringify(MARKER)}),
          headerHasMarker: response.headers.get(${JSON.stringify(HEADER)}) === ${JSON.stringify(HEADER_VALUE)},
          bodyPrefix: body.slice(0, 64),
          error: null
        };
      })
      .catch((error) => ({
        kind: 'fetch',
        status: 0,
        ok: false,
        bodyLen: 0,
        bodyHasMarker: false,
        headerHasMarker: false,
        bodyPrefix: '',
        error: String(error && error.message ? error.message : error)
      })))()`;
  }
  return `(() => new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    xhr.open('GET', ${JSON.stringify(url)}, true);
    xhr.withCredentials = true;
    xhr.onloadend = () => {
      const body = String(xhr.responseText || '');
      let headerHasMarker = false;
      try {
        headerHasMarker = xhr.getResponseHeader(${JSON.stringify(HEADER)}) === ${JSON.stringify(HEADER_VALUE)};
      } catch (_) {}
      resolve({
        kind: 'xhr',
        status: xhr.status,
        ok: xhr.status >= 200 && xhr.status < 300,
        bodyLen: body.length,
        bodyHasMarker: body.includes(${JSON.stringify(MARKER)}),
        headerHasMarker,
        bodyPrefix: body.slice(0, 64),
        error: null
      });
    };
    xhr.onerror = () => resolve({
      kind: 'xhr',
      status: xhr.status || 0,
      ok: false,
      bodyLen: 0,
      bodyHasMarker: false,
      headerHasMarker: false,
      bodyPrefix: '',
      error: 'xhr-error'
    });
    xhr.send();
  }))()`;
}

async function evaluateRaw(client, sessionId, expression, contextId) {
  const params = {
    expression,
    awaitPromise: true,
    returnByValue: true,
  };
  if (contextId !== undefined) params.contextId = contextId;
  return client.send('Runtime.evaluate', params, sessionId);
}

function valueOf(reply) {
  return reply?.result?.value;
}

async function createWorld(client, sessionId, frameId, worldName, universal) {
  const params = { frameId, worldName };
  if (universal) {
    params.grantUniveralAccess = true;
    params.contentSecurityPolicy = "default-src 'none'";
  }
  const world = await client.send('Page.createIsolatedWorld', params, sessionId);
  const contextId = world.executionContextId;
  assert.equal(typeof contextId, 'number');
  assert.ok(contextId > 0);
  return { world, contextId };
}

function readable(probe) {
  return Boolean(
    probe?.fetch?.bodyHasMarker ||
      probe?.xhr?.bodyHasMarker ||
      probe?.fetch?.headerHasMarker ||
      probe?.xhr?.headerHasMarker
  );
}

test('Chrome: isolated world without universal access cannot read a protected cross-origin body', {
  timeout: 180000,
}, async () => {
  const fixture = await fixtureServer();
  const sites = origins(fixture);
  const protectedFixture = await protectedSite();
  let browser;
  let raw;
  let client;
  try {
    browser = await launchBrowser(browserArgs());
    raw = await observer(browser.wsUrl);
    const version = await raw.send('Browser.getVersion', {});
    await raw.send('Storage.setCookies', {
      cookies: [
        {
          name: COOKIE_NAME,
          value: COOKIE_VALUE,
          domain: 'seven.sliccy.ai',
          path: '/',
          httpOnly: false,
          secure: false,
        },
      ],
    });

    client = await guarded(browser.wsUrl, sites.client, {
      variant: 'prompt',
      detachDelay: 2000,
    });

    const { targetId } = await raw.send('Target.createTarget', {
      url: 'about:blank',
      background: true,
    });
    const { sessionId: rawSession } = await raw.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    await raw.send('Page.enable', {}, rawSession);
    await raw.send('Runtime.enable', {}, rawSession);
    const ordinaryUrl = `${sites.ordinary}/doc?n=iw-ua`;
    await navigate(raw, rawSession, ordinaryUrl);
    const pageState = await raw.send(
      'Runtime.evaluate',
      {
        expression: '({ href: location.href, title: document.title, ready: document.readyState })',
        returnByValue: true,
      },
      rawSession
    );
    assert.equal(
      pageState.result?.value?.href,
      ordinaryUrl,
      `unguarded page URL before world create; state=${JSON.stringify(pageState.result?.value)}`
    );
    assert.equal(pageState.result?.value?.title, 'doc');

    const rawTree = await raw.send('Page.getFrameTree', {}, rawSession);
    const frameId = rawTree.frameTree.frame.id;

    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    for (const method of ['Page.enable', 'Runtime.enable', 'Network.enable']) {
      await client.send(method, {}, sessionId);
    }

    const withUa = await createWorld(client, sessionId, frameId, 'slicc-ua-probe', true);
    const withoutUa = await createWorld(client, sessionId, frameId, 'slicc-plain-probe', false);

    const expectedOrigin = `${sites.ordinary}|doc`;
    const sameOriginGuardedUa = await evaluateRaw(
      client,
      sessionId,
      SAME_ORIGIN_EXPR,
      withUa.contextId
    );
    const sameOriginGuardedPlain = await evaluateRaw(
      client,
      sessionId,
      SAME_ORIGIN_EXPR,
      withoutUa.contextId
    );
    const sameOriginGuardedMain = await evaluateRaw(client, sessionId, SAME_ORIGIN_EXPR);
    const sameOriginUnguardedUa = await evaluateRaw(
      raw,
      rawSession,
      SAME_ORIGIN_EXPR,
      withUa.contextId
    );
    const sameOriginUnguardedPlain = await evaluateRaw(
      raw,
      rawSession,
      SAME_ORIGIN_EXPR,
      withoutUa.contextId
    );
    const sameOriginUnguardedMain = await evaluateRaw(raw, rawSession, SAME_ORIGIN_EXPR);

    assert.equal(
      valueOf(sameOriginUnguardedUa),
      expectedOrigin,
      `unguarded UA world sameOrigin; guardedUa=${JSON.stringify(sameOriginGuardedUa)} unguardedUa=${JSON.stringify(sameOriginUnguardedUa)} guardedMain=${JSON.stringify(sameOriginGuardedMain)} unguardedMain=${JSON.stringify(sameOriginUnguardedMain)}`
    );
    assert.equal(
      valueOf(sameOriginUnguardedPlain),
      expectedOrigin,
      `unguarded plain world sameOrigin; guardedPlain=${JSON.stringify(sameOriginGuardedPlain)} unguardedPlain=${JSON.stringify(sameOriginUnguardedPlain)}`
    );

    const fetchGuardedUa = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'fetch'),
      withUa.contextId
    );
    const xhrGuardedUa = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'xhr'),
      withUa.contextId
    );
    const fetchGuardedPlain = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'fetch'),
      withoutUa.contextId
    );
    const xhrGuardedPlain = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'xhr'),
      withoutUa.contextId
    );
    const fetchGuardedMain = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'fetch')
    );
    const xhrGuardedMain = await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'xhr')
    );
    const fetchUnguardedUa = await evaluateRaw(
      raw,
      rawSession,
      probeExpression(protectedFixture.url, 'fetch'),
      withUa.contextId
    );
    const xhrUnguardedUa = await evaluateRaw(
      raw,
      rawSession,
      probeExpression(protectedFixture.url, 'xhr'),
      withUa.contextId
    );

    const withUaProbe = {
      sameOriginGuarded: valueOf(sameOriginGuardedUa),
      sameOriginUnguarded: valueOf(sameOriginUnguardedUa),
      fetch: valueOf(fetchGuardedUa),
      xhr: valueOf(xhrGuardedUa),
      fetchUnguarded: valueOf(fetchUnguardedUa),
      xhrUnguarded: valueOf(xhrUnguardedUa),
    };
    const withoutUaProbe = {
      sameOriginGuarded: valueOf(sameOriginGuardedPlain),
      sameOriginUnguarded: valueOf(sameOriginUnguardedPlain),
      fetch: valueOf(fetchGuardedPlain),
      xhr: valueOf(xhrGuardedPlain),
    };
    const mainWorld = {
      sameOriginGuarded: valueOf(sameOriginGuardedMain),
      sameOriginUnguarded: valueOf(sameOriginUnguardedMain),
      fetch: valueOf(fetchGuardedMain),
      xhr: valueOf(xhrGuardedMain),
    };

    const report = {
      chromeVersion: version.product || version.userAgent || version,
      ordinaryUrl,
      protectedUrl: protectedFixture.url,
      cookie: `${COOKIE_NAME}=${COOKIE_VALUE}`,
      hits: protectedFixture.hits,
      worldCreate: { withUa: withUa.world, withoutUa: withoutUa.world },
      rawReplies: {
        sameOriginGuardedUa,
        sameOriginGuardedPlain,
        sameOriginGuardedMain,
        sameOriginUnguardedUa,
        sameOriginUnguardedPlain,
        sameOriginUnguardedMain,
        fetchGuardedUa,
        xhrGuardedUa,
        fetchGuardedPlain,
        xhrGuardedPlain,
        fetchGuardedMain,
        xhrGuardedMain,
        fetchUnguardedUa,
        xhrUnguardedUa,
      },
      withGrantUniveralAccess: withUaProbe,
      withoutGrantUniveralAccess: withoutUaProbe,
      mainWorld,
    };

    assert.equal(
      readable(withUaProbe),
      false,
      `protected content must stay unreadable from the isolated world; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUaProbe.fetch?.bodyHasMarker,
      false,
      `fetch body marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUaProbe.xhr?.bodyHasMarker,
      false,
      `xhr body marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUaProbe.fetch?.headerHasMarker,
      false,
      `fetch header marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUaProbe.xhr?.headerHasMarker,
      false,
      `xhr header marker; probe=${JSON.stringify(report)}`
    );
  } finally {
    await client?.close();
    await raw?.close();
    await browser?.close();
    await protectedFixture.close();
    await fixture.close();
  }
});
