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
      error: 'xhr-error'
    });
    xhr.send();
  }))()`;
}

async function evaluate(client, sessionId, expression, contextId) {
  const params = {
    expression,
    awaitPromise: true,
    returnByValue: true,
  };
  if (contextId !== undefined) params.contextId = contextId;
  const reply = await client.send('Runtime.evaluate', params, sessionId);
  if (reply.exceptionDetails) {
    throw new Error(JSON.stringify(reply.exceptionDetails));
  }
  return reply.result?.value;
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
  return contextId;
}

async function worldProbe(client, sessionId, contextId, protectedUrl) {
  const sameOrigin = await evaluate(
    client,
    sessionId,
    'location.origin + "|" + document.title',
    contextId
  );
  const fetchProbe = await evaluate(
    client,
    sessionId,
    probeExpression(protectedUrl, 'fetch'),
    contextId
  );
  const xhrProbe = await evaluate(
    client,
    sessionId,
    probeExpression(protectedUrl, 'xhr'),
    contextId
  );
  return { sameOrigin, fetch: fetchProbe, xhr: xhrProbe };
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

    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    for (const method of ['Page.enable', 'Runtime.enable', 'Network.enable']) {
      await client.send(method, {}, sessionId);
    }
    const tree = await client.send('Page.getFrameTree', {}, sessionId);
    const frameId = tree.frameTree.frame.id;

    const withUaId = await createWorld(client, sessionId, frameId, 'slicc-ua-probe', true);
    const withoutUaId = await createWorld(client, sessionId, frameId, 'slicc-plain-probe', false);

    const withUa = await worldProbe(client, sessionId, withUaId, protectedFixture.url);
    const withoutUa = await worldProbe(client, sessionId, withoutUaId, protectedFixture.url);
    const mainFetch = await evaluate(
      client,
      sessionId,
      probeExpression(protectedFixture.url, 'fetch')
    );
    const mainXhr = await evaluate(client, sessionId, probeExpression(protectedFixture.url, 'xhr'));

    const expectedOrigin = `${sites.ordinary}|doc`;
    assert.equal(withUa.sameOrigin, expectedOrigin);
    assert.equal(withoutUa.sameOrigin, expectedOrigin);

    const report = {
      chromeVersion: version.product || version.userAgent || version,
      ordinaryUrl,
      protectedUrl: protectedFixture.url,
      cookie: `${COOKIE_NAME}=${COOKIE_VALUE}`,
      hits: protectedFixture.hits,
      withGrantUniveralAccess: withUa,
      withoutGrantUniveralAccess: withoutUa,
      mainWorld: { fetch: mainFetch, xhr: mainXhr },
    };

    assert.equal(
      readable(withUa),
      false,
      `protected content must stay unreadable from the isolated world; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUa.fetch?.bodyHasMarker,
      false,
      `fetch body marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUa.xhr?.bodyHasMarker,
      false,
      `xhr body marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUa.fetch?.headerHasMarker,
      false,
      `fetch header marker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      withUa.xhr?.headerHasMarker,
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
