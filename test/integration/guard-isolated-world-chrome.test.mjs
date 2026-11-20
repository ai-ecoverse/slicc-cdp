import assert from 'node:assert/strict';
import { appendFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import path from 'node:path';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';
import './watchdog.mjs';

const MARKER = 'PROTECTED-BODY-MARKER-IW-UA';
const OPEN_MARKER = 'OPEN-CROSS-ORIGIN-MARKER-IW-UA';
const HEADER = 'x-protected-marker';
const HEADER_VALUE = 'PROTECTED-HEADER-MARKER-IW-UA';
const OPEN_HEADER = 'x-open-marker';
const OPEN_HEADER_VALUE = 'OPEN-HEADER-MARKER-IW-UA';
const COOKIE_NAME = 'iw_ua_session';
const COOKIE_VALUE = 'IW-UA-COOKIE-SECRET';
const SAME_ORIGIN_EXPR =
  '({ kind: "sameOrigin", titleDoc: document.title === "doc", originPlain: location.origin.includes("plain.guard.test") })';

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await raw.send('Page.navigate', { url }, session);
  await loaded;
}

function hitRecord(req, tag) {
  return {
    tag,
    method: req.method,
    path: new URL(req.url || '/', 'http://127.0.0.1').pathname,
    cookiePresent: String(req.headers.cookie || '').includes(`${COOKIE_NAME}=${COOKIE_VALUE}`),
    originPresent: Boolean(req.headers.origin),
    refererPresent: Boolean(req.headers.referer),
    origin: req.headers.origin ? 'set' : 'absent',
    referer: req.headers.referer ? 'set' : 'absent',
  };
}

function dualSite() {
  const hits = [];
  const server = createServer((req, res) => {
    const host = String(req.headers.host || '').split(':')[0];
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    if (host === 'seven.sliccy.ai' && url.pathname === '/secret') {
      hits.push(hitRecord(req, 'protected'));
      res.writeHead(200, {
        'content-type': 'text/plain; charset=utf-8',
        [HEADER]: HEADER_VALUE,
      });
      res.end(`${MARKER}\n`);
      return;
    }
    if (host === 'open.other.test' && url.pathname === '/open') {
      hits.push(hitRecord(req, 'open'));
      res.writeHead(200, {
        'content-type': 'text/plain; charset=utf-8',
        [OPEN_HEADER]: OPEN_HEADER_VALUE,
      });
      res.end(`${OPEN_MARKER}\n`);
      return;
    }
    hits.push(hitRecord(req, 'other'));
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      resolve({
        port,
        hits,
        protectedUrl: `http://seven.sliccy.ai:${port}/secret`,
        openUrl: `http://open.other.test:${port}/open`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function probeExpression(url, kind, marker, headerName, headerValue) {
  if (kind === 'fetch') {
    return `(() => fetch(${JSON.stringify(url)}, { credentials: 'include', cache: 'no-store' })
      .then(async (response) => {
        const body = await response.text();
        return {
          kind: 'fetch',
          status: response.status,
          ok: response.ok,
          bodyLen: body.length,
          hasMarker: body.includes(${JSON.stringify(marker)}),
          headerHasMarker: response.headers.get(${JSON.stringify(headerName)}) === ${JSON.stringify(headerValue)},
          error: null
        };
      })
      .catch((error) => ({
        kind: 'fetch',
        status: 0,
        ok: false,
        bodyLen: 0,
        hasMarker: false,
        headerHasMarker: false,
        error: String(error && error.name ? error.name : 'Error')
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
        headerHasMarker = xhr.getResponseHeader(${JSON.stringify(headerName)}) === ${JSON.stringify(headerValue)};
      } catch (_) {}
      resolve({
        kind: 'xhr',
        status: xhr.status,
        ok: xhr.status >= 200 && xhr.status < 300,
        bodyLen: body.length,
        hasMarker: body.includes(${JSON.stringify(marker)}),
        headerHasMarker,
        error: null
      });
    };
    xhr.onerror = () => resolve({
      kind: 'xhr',
      status: xhr.status || 0,
      ok: false,
      bodyLen: 0,
      hasMarker: false,
      headerHasMarker: false,
      error: 'NetworkError'
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

async function createWorld(client, sessionId, frameId, options = {}) {
  const params = { frameId };
  if (options.worldName !== undefined) params.worldName = options.worldName;
  if (options.universal) params.grantUniveralAccess = true;
  if (options.csp !== undefined) params.contentSecurityPolicy = options.csp;
  const world = await client.send('Page.createIsolatedWorld', params, sessionId);
  const contextId = world.executionContextId;
  assert.equal(typeof contextId, 'number');
  assert.ok(contextId > 0);
  return { world, contextId, params };
}

function readable(probe) {
  return Boolean(
    probe?.fetch?.hasMarker ||
      probe?.xhr?.hasMarker ||
      probe?.fetch?.headerHasMarker ||
      probe?.xhr?.headerHasMarker
  );
}

async function worldReads(client, sessionId, contextId, protectedUrl, openUrl) {
  const sameOrigin = valueOf(await evaluateRaw(client, sessionId, SAME_ORIGIN_EXPR, contextId));
  const fetch = valueOf(
    await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedUrl, 'fetch', MARKER, HEADER, HEADER_VALUE),
      contextId
    )
  );
  const xhr = valueOf(
    await evaluateRaw(
      client,
      sessionId,
      probeExpression(protectedUrl, 'xhr', MARKER, HEADER, HEADER_VALUE),
      contextId
    )
  );
  const openFetch = valueOf(
    await evaluateRaw(
      client,
      sessionId,
      probeExpression(openUrl, 'fetch', OPEN_MARKER, OPEN_HEADER, OPEN_HEADER_VALUE),
      contextId
    )
  );
  const openXhr = valueOf(
    await evaluateRaw(
      client,
      sessionId,
      probeExpression(openUrl, 'xhr', OPEN_MARKER, OPEN_HEADER, OPEN_HEADER_VALUE),
      contextId
    )
  );
  return { sameOrigin, fetch, xhr, openFetch, openXhr };
}

function publishReport(report) {
  const text = JSON.stringify(report);
  console.log(`IW-UA-PROBE ${text}`);
  const dir = process.env.SLICC_CHROME_REPORT_DIR;
  if (dir) writeFileSync(path.join(dir, `iw-ua-probe-${process.pid}.json`), text);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `\n### isolated-world probe\n\n\`\`\`json\n${text}\n\`\`\`\n`
    );
  }
}

test('Chrome: isolated world without universal access cannot read a protected cross-origin body', {
  timeout: 180000,
}, async () => {
  const fixture = await fixtureServer();
  const sites = origins(fixture);
  const cross = await dualSite();
  let browser;
  let raw;
  let client;
  try {
    const args = browserArgs();
    const resolver = args.findIndex((arg) => arg.startsWith('--host-resolver-rules='));
    if (resolver >= 0) args[resolver] = `${args[resolver]}, MAP open.other.test 127.0.0.1`;
    else args.push('--host-resolver-rules=MAP open.other.test 127.0.0.1');
    browser = await launchBrowser(args);
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
    const pageState = valueOf(
      await evaluateRaw(
        raw,
        rawSession,
        '({ hrefPlain: location.href.includes("plain.guard.test") && location.href.includes("/doc"), titleDoc: document.title === "doc", ready: document.readyState })'
      )
    );
    assert.equal(
      pageState?.hrefPlain,
      true,
      `unguarded page URL; state=${JSON.stringify(pageState)}`
    );
    assert.equal(pageState?.titleDoc, true);

    const rawTree = await raw.send('Page.getFrameTree', {}, rawSession);
    const frameId = rawTree.frameTree.frame.id;

    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    for (const method of ['Page.enable', 'Runtime.enable', 'Network.enable']) {
      await client.send(method, {}, sessionId);
    }

    const attackNamed = await createWorld(client, sessionId, frameId, {
      worldName: 'slicc-ua-attack',
      universal: true,
    });
    const attackAnon = await createWorld(client, sessionId, frameId, {
      universal: true,
    });
    const cspControl = await createWorld(client, sessionId, frameId, {
      worldName: 'slicc-ua-csp',
      universal: true,
      csp: "default-src 'none'",
    });
    const plain = await createWorld(client, sessionId, frameId, {
      worldName: 'slicc-plain',
    });

    const unguardedSameOriginAttack = valueOf(
      await evaluateRaw(raw, rawSession, SAME_ORIGIN_EXPR, attackNamed.contextId)
    );
    assert.equal(
      unguardedSameOriginAttack?.titleDoc,
      true,
      `unguarded attack world; state=${JSON.stringify(unguardedSameOriginAttack)}`
    );
    assert.equal(unguardedSameOriginAttack?.originPlain, true);

    const hitsBefore = cross.hits.length;
    const attackNamedReads = await worldReads(
      client,
      sessionId,
      attackNamed.contextId,
      cross.protectedUrl,
      cross.openUrl
    );
    const attackAnonReads = await worldReads(
      client,
      sessionId,
      attackAnon.contextId,
      cross.protectedUrl,
      cross.openUrl
    );
    const cspReads = await worldReads(
      client,
      sessionId,
      cspControl.contextId,
      cross.protectedUrl,
      cross.openUrl
    );
    const plainReads = await worldReads(
      client,
      sessionId,
      plain.contextId,
      cross.protectedUrl,
      cross.openUrl
    );
    const mainFetch = valueOf(
      await evaluateRaw(
        client,
        sessionId,
        probeExpression(cross.protectedUrl, 'fetch', MARKER, HEADER, HEADER_VALUE)
      )
    );
    const mainXhr = valueOf(
      await evaluateRaw(
        client,
        sessionId,
        probeExpression(cross.protectedUrl, 'xhr', MARKER, HEADER, HEADER_VALUE)
      )
    );
    const mainOpenFetch = valueOf(
      await evaluateRaw(
        client,
        sessionId,
        probeExpression(cross.openUrl, 'fetch', OPEN_MARKER, OPEN_HEADER, OPEN_HEADER_VALUE)
      )
    );

    const report = {
      chromeVersion: version.product || version.userAgent || version,
      ordinaryUrl,
      protectedUrl: cross.protectedUrl,
      openUrl: cross.openUrl,
      cookie: `${COOKIE_NAME}=${COOKIE_VALUE}`,
      worldParams: {
        attackNamed: attackNamed.params,
        attackAnon: attackAnon.params,
        cspControl: cspControl.params,
        plain: plain.params,
      },
      hitsAfter: cross.hits.slice(hitsBefore),
      attackNamed: attackNamedReads,
      attackAnon: attackAnonReads,
      cspControl: cspReads,
      withoutGrantUniveralAccess: plainReads,
      mainWorld: { fetch: mainFetch, xhr: mainXhr, openFetch: mainOpenFetch },
      positiveControlReadable: readable({
        fetch: attackNamedReads.openFetch,
        xhr: attackNamedReads.openXhr,
      }),
      protectedReadableAttackNamed: readable(attackNamedReads),
      protectedReadableAttackAnon: readable(attackAnonReads),
    };
    publishReport(report);

    assert.equal(
      report.positiveControlReadable,
      true,
      `universal-access world must read an unprotected no-CORS cross-origin body; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      report.protectedReadableAttackNamed,
      false,
      `protected content must stay unreadable from the attack isolated world; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      report.protectedReadableAttackAnon,
      false,
      `protected content must stay unreadable from the anonymous attack world; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      attackNamedReads.fetch?.hasMarker,
      false,
      `fetch hasMarker; probe=${JSON.stringify(report)}`
    );
    assert.equal(
      attackNamedReads.xhr?.hasMarker,
      false,
      `xhr hasMarker; probe=${JSON.stringify(report)}`
    );
  } finally {
    await client?.close();
    await raw?.close();
    await browser?.close();
    await cross.close();
    await fixture.close();
  }
});
