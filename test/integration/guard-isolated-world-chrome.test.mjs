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

const OPEN_MARKER = 'OPEN-DOM-MARKER-IW-UA';
const PROT_MARKER = 'PROTECTED-DOM-MARKER-IW-UA';

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await raw.send('Page.navigate', { url }, session);
  await loaded;
}

function site() {
  const hits = [];
  const server = createServer((req, res) => {
    const host = String(req.headers.host || '').split(':')[0];
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    hits.push({
      host,
      path: url.pathname,
      method: req.method,
    });
    const html = (title, body) => {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(`<!doctype html><meta charset="utf-8"><title>${title}</title>${body}`);
    };
    if (host === 'plain.guard.test' && url.pathname === '/parent') {
      const openSrc = `http://open.other.test:${server.address().port}/open-frame`;
      const protSrc = `http://seven.sliccy.ai:${server.address().port}/prot-frame`;
      return html(
        'doc',
        `<p>parent</p><iframe id="open" src="${openSrc}"></iframe><iframe id="prot" src="${protSrc}"></iframe>`
      );
    }
    if (host === 'open.other.test' && url.pathname === '/open-frame') {
      return html('open', `<body data-kind="open">${OPEN_MARKER}</body>`);
    }
    if (host === 'seven.sliccy.ai' && url.pathname === '/prot-frame') {
      return html('seven', `<body data-kind="prot">${PROT_MARKER}</body>`);
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
        parentUrl: `http://plain.guard.test:${port}/parent`,
        openFrameUrl: `http://open.other.test:${port}/open-frame`,
        protFrameUrl: `http://seven.sliccy.ai:${port}/prot-frame`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function domProbeExpression(frameId, marker) {
  return `(() => {
    const frame = document.getElementById(${JSON.stringify(frameId)});
    const out = {
      frameFound: Boolean(frame),
      hasContentDocument: false,
      bodyExists: false,
      textLen: 0,
      hasMarker: false,
      hrefKind: 'none',
      originKind: 'none',
      error: null
    };
    if (!frame) return out;
    try {
      const doc = frame.contentDocument;
      out.hasContentDocument = doc != null;
      if (doc && doc.body) {
        out.bodyExists = true;
        const text = String(doc.body.innerText || '');
        out.textLen = text.length;
        out.hasMarker = text.includes(${JSON.stringify(marker)});
      }
    } catch (error) {
      out.error = String(error && error.name ? error.name : 'Error');
    }
    try {
      const href = String(frame.contentWindow.location.href || '');
      const origin = String(frame.contentWindow.location.origin || '');
      if (href === 'about:blank') out.hrefKind = 'blank';
      else if (href.includes('open.other.test')) out.hrefKind = 'open';
      else if (href.includes('seven.sliccy.ai')) out.hrefKind = 'prot';
      else if (href) out.hrefKind = 'other';
      if (origin === 'null') out.originKind = 'null';
      else if (origin.includes('open.other.test')) out.originKind = 'open';
      else if (origin.includes('seven.sliccy.ai')) out.originKind = 'prot';
      else if (origin) out.originKind = 'other';
    } catch (error) {
      out.hrefKind = 'error';
      out.originKind = 'error';
      if (!out.error) out.error = String(error && error.name ? error.name : 'Error');
    }
    return out;
  })()`;
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

async function waitFrames(raw, sessionId) {
  const deadline = Date.now() + 15000;
  do {
    const state = valueOf(
      await evaluateRaw(
        raw,
        sessionId,
        `({
          openReady: (() => { try { const f=document.getElementById('open'); return Boolean(f && f.contentDocument && f.contentDocument.readyState === 'complete' && (f.contentDocument.body||{}).innerText); } catch (_) { return false } })(),
          protHrefKind: (() => { try { const href=String(document.getElementById('prot').contentWindow.location.href||''); if(href==='about:blank') return 'blank'; if(href.includes('seven.sliccy.ai')) return 'prot'; if(href) return 'other'; return 'none'; } catch (e) { return 'error:' + (e && e.name ? e.name : 'Error'); } })(),
          protHasDoc: (() => { try { return document.getElementById('prot').contentDocument != null; } catch (_) { return false } })(),
          protTextLen: (() => { try { const d=document.getElementById('prot').contentDocument; return d && d.body ? String(d.body.innerText||'').length : 0; } catch (_) { return -1 } })(),
          protHasMarker: (() => { try { const d=document.getElementById('prot').contentDocument; return Boolean(d && d.body && String(d.body.innerText||'').includes(${JSON.stringify(PROT_MARKER)})); } catch (_) { return false } })()
        })`
      )
    );
    if (state?.openReady) return state;
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  throw new Error('iframe settle timeout');
}

function publishReport(report) {
  const text = JSON.stringify(report);
  console.log(`IW-UA-DOM-PROBE ${text}`);
  const dir = process.env.SLICC_CHROME_REPORT_DIR;
  if (dir) writeFileSync(path.join(dir, `iw-ua-dom-probe-${process.pid}.json`), text);
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `\n### isolated-world DOM probe\n\n\`\`\`json\n${text}\n\`\`\`\n`
    );
  }
}

function protectedReadable(probe) {
  return Boolean(probe?.hasContentDocument && (probe?.hasMarker || probe?.textLen > 0));
}

test('Chrome: isolated world without universal access cannot read a protected cross-origin iframe DOM', {
  timeout: 180000,
}, async () => {
  const fixture = await fixtureServer();
  const sites = origins(fixture);
  const pages = await site();
  let browser;
  let raw;
  let client;
  try {
    const args = browserArgs({ siteIsolationOff: true });
    const resolver = args.findIndex((arg) => arg.startsWith('--host-resolver-rules='));
    if (resolver >= 0) args[resolver] = `${args[resolver]}, MAP open.other.test 127.0.0.1`;
    else args.push('--host-resolver-rules=MAP open.other.test 127.0.0.1');
    browser = await launchBrowser(args);
    raw = await observer(browser.wsUrl);
    const version = await raw.send('Browser.getVersion', {});

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
    await navigate(raw, rawSession, pages.parentUrl);
    const pageState = valueOf(
      await evaluateRaw(
        raw,
        rawSession,
        '({ titleDoc: document.title === "doc", hasOpen: Boolean(document.getElementById("open")), hasProt: Boolean(document.getElementById("prot")) })'
      )
    );
    assert.equal(pageState?.titleDoc, true, JSON.stringify(pageState));
    assert.equal(pageState?.hasOpen, true);
    assert.equal(pageState?.hasProt, true);

    const unguardedIframes = await waitFrames(raw, rawSession);
    const rawTree = await raw.send('Page.getFrameTree', {}, rawSession);
    const frameId = rawTree.frameTree.frame.id;

    const { sessionId } = await client.send('Target.attachToTarget', {
      targetId,
      flatten: true,
    });
    for (const method of ['Page.enable', 'Runtime.enable']) {
      await client.send(method, {}, sessionId);
    }

    const attackNamed = await createWorld(client, sessionId, frameId, {
      worldName: 'slicc-ua-dom',
      universal: true,
    });
    const attackAnon = await createWorld(client, sessionId, frameId, { universal: true });
    const plain = await createWorld(client, sessionId, frameId, {
      worldName: 'slicc-plain-dom',
    });

    const worlds = [
      ['attackNamed', attackNamed.contextId],
      ['attackAnon', attackAnon.contextId],
      ['withoutGrantUniveralAccess', plain.contextId],
      ['mainWorld', undefined],
    ];
    const perWorld = {};
    for (const [name, contextId] of worlds) {
      perWorld[name] = {
        open: valueOf(
          await evaluateRaw(client, sessionId, domProbeExpression('open', OPEN_MARKER), contextId)
        ),
        protected: valueOf(
          await evaluateRaw(client, sessionId, domProbeExpression('prot', PROT_MARKER), contextId)
        ),
      };
    }

    const report = {
      chromeVersion: version.product || version.userAgent || version,
      siteIsolationOff: true,
      sameSiteParentGap:
        'catalog protectedOrigins.domains includes sliccy.ai, so no ordinary same-site parent with a protected iframe exists in this harness',
      parentUrl: pages.parentUrl,
      openFrameUrl: pages.openFrameUrl,
      protFrameUrl: pages.protFrameUrl,
      unguardedIframes,
      protectedIframeLoadedInUnguardedBrowser: Boolean(
        unguardedIframes?.protHrefKind === 'prot' || unguardedIframes?.protHasMarker
      ),
      worldParams: {
        attackNamed: attackNamed.params,
        attackAnon: attackAnon.params,
        plain: plain.params,
      },
      perWorld,
      positiveControl: {
        attackNamedOpenReadable: protectedReadable(perWorld.attackNamed.open),
        attackAnonOpenReadable: protectedReadable(perWorld.attackAnon.open),
        plainOpenReadable: protectedReadable(perWorld.withoutGrantUniveralAccess.open),
        mainOpenReadable: protectedReadable(perWorld.mainWorld.open),
      },
      protectedReadable: {
        attackNamed: protectedReadable(perWorld.attackNamed.protected),
        attackAnon: protectedReadable(perWorld.attackAnon.protected),
        withoutGrantUniveralAccess: protectedReadable(
          perWorld.withoutGrantUniveralAccess.protected
        ),
        mainWorld: protectedReadable(perWorld.mainWorld.protected),
      },
      hits: pages.hits,
    };
    publishReport(report);

    for (const [name, readable] of Object.entries(report.protectedReadable)) {
      assert.equal(
        readable,
        false,
        `protected iframe DOM must stay unreadable in ${name}; probe=${JSON.stringify(report)}`
      );
    }
  } finally {
    await client?.close();
    await raw?.close();
    await browser?.close();
    await pages.close();
    await fixture.close();
  }
});
