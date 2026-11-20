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
    hits.push({ host, path: url.pathname, method: req.method });
    const html = (title, body) => {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(`<!doctype html><meta charset="utf-8"><title>${title}</title>${body}`);
    };
    const port = server.address().port;
    if (host === 'plain.guard.test' && url.pathname === '/open-parent') {
      return html(
        'doc',
        `<p>open-parent</p><iframe id="open" src="http://open.other.test:${port}/open-frame"></iframe>`
      );
    }
    if (host === 'plain.guard.test' && url.pathname === '/prot-parent') {
      return html(
        'doc',
        `<p>prot-parent</p><iframe id="prot" src="http://seven.sliccy.ai:${port}/prot-frame"></iframe>`
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
        openParentUrl: `http://plain.guard.test:${port}/open-parent`,
        protParentUrl: `http://plain.guard.test:${port}/prot-parent`,
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

function childUrls(tree, urls = []) {
  const frame = tree?.frame;
  if (frame?.url) urls.push(String(frame.url));
  for (const child of tree?.childFrames || []) childUrls(child, urls);
  return urls;
}

async function waitChild(raw, sessionId, needle) {
  const deadline = Date.now() + 20000;
  let last = null;
  do {
    const tree = await raw.send('Page.getFrameTree', {}, sessionId);
    const urls = childUrls(tree.frameTree);
    last = { urls };
    if (urls.some((url) => url.includes(needle))) return { tree, urls };
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  throw new Error(`iframe settle timeout for ${needle}; last=${JSON.stringify(last)}`);
}

async function openTab(raw, url, needle) {
  const { targetId } = await raw.send('Target.createTarget', {
    url: 'about:blank',
    background: true,
  });
  const { sessionId } = await raw.send('Target.attachToTarget', {
    targetId,
    flatten: true,
  });
  await raw.send('Page.enable', {}, sessionId);
  await raw.send('Runtime.enable', {}, sessionId);
  await navigate(raw, sessionId, url);
  const settled = await waitChild(raw, sessionId, needle);
  return { targetId, sessionId, frameId: settled.tree.frameTree.frame.id, urls: settled.urls };
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

async function tryStep(label, run) {
  try {
    const result = await run();
    return { label, ok: true, result };
  } catch (error) {
    return {
      label,
      ok: false,
      refusal: String(error?.message || error),
    };
  }
}

function readable(probe) {
  return Boolean(probe?.hasContentDocument && (probe?.hasMarker || probe?.textLen > 0));
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

    const tabA = await openTab(raw, pages.openParentUrl, '/open-frame');
    const tabB = await openTab(raw, pages.protParentUrl, '/prot-frame');

    const { sessionId: guardedA } = await client.send('Target.attachToTarget', {
      targetId: tabA.targetId,
      flatten: true,
    });
    for (const method of ['Page.enable', 'Runtime.enable']) {
      await client.send(method, {}, guardedA);
    }
    const attackNamed = await createWorld(client, guardedA, tabA.frameId, {
      worldName: 'slicc-ua-dom',
      universal: true,
    });
    const attackAnon = await createWorld(client, guardedA, tabA.frameId, { universal: true });
    const plain = await createWorld(client, guardedA, tabA.frameId, {
      worldName: 'slicc-plain-dom',
    });
    const tabAWorlds = {};
    for (const [name, contextId] of [
      ['attackNamed', attackNamed.contextId],
      ['attackAnon', attackAnon.contextId],
      ['withoutGrantUniveralAccess', plain.contextId],
      ['mainWorld', undefined],
    ]) {
      tabAWorlds[name] = valueOf(
        await evaluateRaw(client, guardedA, domProbeExpression('open', OPEN_MARKER), contextId)
      );
    }

    const tabBGuarded = {};
    const attachB = await tryStep('attach', () =>
      client.send('Target.attachToTarget', { targetId: tabB.targetId, flatten: true })
    );
    tabBGuarded.attach = {
      ok: attachB.ok,
      refusal: attachB.refusal || null,
      sessionId: attachB.result?.sessionId || null,
    };
    if (attachB.ok) {
      const guardedB = attachB.result.sessionId;
      tabBGuarded.pageEnable = await tryStep('Page.enable', () =>
        client.send('Page.enable', {}, guardedB)
      );
      tabBGuarded.runtimeEnable = await tryStep('Runtime.enable', () =>
        client.send('Runtime.enable', {}, guardedB)
      );
      tabBGuarded.getFrameTree = await tryStep('Page.getFrameTree', () =>
        client.send('Page.getFrameTree', {}, guardedB)
      );
      const frameId = tabBGuarded.getFrameTree.result?.frameTree?.frame?.id || tabB.frameId;
      tabBGuarded.createIsolatedWorldUa = await tryStep('Page.createIsolatedWorld.ua', () =>
        client.send(
          'Page.createIsolatedWorld',
          {
            frameId,
            worldName: 'slicc-ua-prot',
            grantUniveralAccess: true,
          },
          guardedB
        )
      );
      const uaContext = tabBGuarded.createIsolatedWorldUa.result?.executionContextId;
      tabBGuarded.evaluateUa = await tryStep('Runtime.evaluate.ua', () =>
        evaluateRaw(client, guardedB, domProbeExpression('prot', PROT_MARKER), uaContext)
      );
      tabBGuarded.createIsolatedWorldPlain = await tryStep('Page.createIsolatedWorld.plain', () =>
        client.send(
          'Page.createIsolatedWorld',
          { frameId, worldName: 'slicc-plain-prot' },
          guardedB
        )
      );
      const plainContext = tabBGuarded.createIsolatedWorldPlain.result?.executionContextId;
      tabBGuarded.evaluatePlain = await tryStep('Runtime.evaluate.plain', () =>
        evaluateRaw(client, guardedB, domProbeExpression('prot', PROT_MARKER), plainContext)
      );
      tabBGuarded.evaluateMain = await tryStep('Runtime.evaluate.main', () =>
        evaluateRaw(client, guardedB, domProbeExpression('prot', PROT_MARKER))
      );
    }

    const uaRaw = await createWorld(raw, tabB.sessionId, tabB.frameId, {
      worldName: 'slicc-ua-raw',
      universal: true,
    });
    const plainRaw = await createWorld(raw, tabB.sessionId, tabB.frameId, {
      worldName: 'slicc-plain-raw',
    });
    const tabBUnguarded = {
      attackNamed: valueOf(
        await evaluateRaw(
          raw,
          tabB.sessionId,
          domProbeExpression('prot', PROT_MARKER),
          uaRaw.contextId
        )
      ),
      withoutGrantUniveralAccess: valueOf(
        await evaluateRaw(
          raw,
          tabB.sessionId,
          domProbeExpression('prot', PROT_MARKER),
          plainRaw.contextId
        )
      ),
      mainWorld: valueOf(
        await evaluateRaw(raw, tabB.sessionId, domProbeExpression('prot', PROT_MARKER))
      ),
    };

    const report = {
      chromeVersion: version.product || version.userAgent || version,
      siteIsolationOff: true,
      sameSiteParentGap:
        'catalog protectedOrigins.domains includes sliccy.ai, so no ordinary same-site parent with a protected iframe exists in this harness',
      tabA: {
        url: pages.openParentUrl,
        childUrls: tabA.urls,
        worldParams: {
          attackNamed: attackNamed.params,
          attackAnon: attackAnon.params,
          plain: plain.params,
        },
        perWorld: tabAWorlds,
        positiveControlReadable: {
          attackNamed: readable(tabAWorlds.attackNamed),
          attackAnon: readable(tabAWorlds.attackAnon),
          withoutGrantUniveralAccess: readable(tabAWorlds.withoutGrantUniveralAccess),
          mainWorld: readable(tabAWorlds.mainWorld),
        },
      },
      tabB: {
        url: pages.protParentUrl,
        childUrls: tabB.urls,
        guardedRefusals: tabBGuarded,
      },
      tabBUnguarded: {
        perWorld: tabBUnguarded,
        protectedReadable: {
          attackNamed: readable(tabBUnguarded.attackNamed),
          withoutGrantUniveralAccess: readable(tabBUnguarded.withoutGrantUniveralAccess),
          mainWorld: readable(tabBUnguarded.mainWorld),
        },
      },
      summary: {
        positiveControlReadableInUa:
          readable(tabAWorlds.attackNamed) || readable(tabAWorlds.attackAnon),
        protectedReadableUnguardedUa: readable(tabBUnguarded.attackNamed),
        protectedReadableUnguardedPlain: readable(tabBUnguarded.withoutGrantUniveralAccess),
        protectedReadableUnguardedMain: readable(tabBUnguarded.mainWorld),
        guardedTabBRefused: Object.values(tabBGuarded).some(
          (step) => step && step.ok === false && /seven|refused/i.test(String(step.refusal || ''))
        ),
      },
    };
    publishReport(report);

    const guardedProtectedContent = [
      tabBGuarded.evaluateUa,
      tabBGuarded.evaluatePlain,
      tabBGuarded.evaluateMain,
    ]
      .filter(Boolean)
      .map((step) => (step.ok ? valueOf(step.result) : null));
    for (const probe of guardedProtectedContent) {
      assert.equal(
        readable(probe),
        false,
        `guarded tab B must not return protected iframe content; probe=${JSON.stringify(report)}`
      );
    }
    assert.equal(
      report.summary.guardedTabBRefused ||
        guardedProtectedContent.every((probe) => !readable(probe)),
      true,
      `guarded tab B must refuse or keep protected iframe unreadable; probe=${JSON.stringify(report)}`
    );
  } finally {
    await client?.close();
    await raw?.close();
    await browser?.close();
    await pages.close();
    await fixture.close();
  }
});
