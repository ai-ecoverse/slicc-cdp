import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import browser, { resolveCdpEndpoint } from '../../packages/sliccy-browser/src/browser.js';
import { connect } from '../../packages/sliccy-browser/src/connect.js';

const site = process.env.GUARD_SITE;
const allowedWs = process.env.GUARD_ALLOWED_WS;
const logPath = process.env.GUARD_LOG;
const mode = process.env.GUARD_MODE || 'ordinary-child';

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, label, ms = 8000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await delay(50);
  }
  throw new Error(label);
}

async function log(line) {
  const { appendFile } = await import('node:fs/promises');
  await appendFile(logPath, `${line}\n`);
}

async function fixtureLogHas(needle) {
  try {
    const text = await readFile(logPath, 'utf8');
    return text.includes(needle);
  } catch {
    return false;
  }
}

function summarizeTree(tree) {
  const out = [];
  const walk = (node) => {
    if (!node?.frame) return;
    out.push({
      id: node.frame.id,
      parentId: node.frame.parentId,
      url: node.frame.url,
      loaderId: node.frame.loaderId,
    });
    for (const child of node.childFrames ?? []) walk(child);
  };
  walk(tree);
  return out;
}

function recordProbe(events, method, message) {
  const params = message?.params ?? {};
  const entry = { sessionId: message?.sessionId, method };
  if (typeof params.url === 'string') entry.url = params.url;
  if (typeof params.requestId === 'string') entry.requestId = params.requestId;
  if (typeof params.frameId === 'string') entry.frameId = params.frameId;
  if (typeof params.reason === 'string') entry.reason = params.reason;
  if (params.targetInfo && typeof params.targetInfo === 'object') {
    entry.targetId = params.targetInfo.targetId;
    entry.type = params.targetInfo.type;
    entry.targetUrl = params.targetInfo.url;
  }
  if (typeof params.targetId === 'string') entry.targetId = params.targetId;
  events.push(entry);
}

async function main() {
  await mkdir(path.dirname(logPath), { recursive: true });
  await writeFile(logPath, '');

  const logged = [];
  const write = console.log;
  console.log = (...args) => {
    logged.push(args.map(String).join(' '));
  };
  let handle;
  let treeSummary = [];
  const probeEvents = [];
  try {
    if (mode === 'premise') {
      handle = await runPremise({
        logged,
        probeEvents,
        getTree: () => treeSummary,
        setTree: (v) => {
          treeSummary = v;
        },
      });
    } else {
      handle = await runOrdinaryChild({
        logged,
        setTree: (v) => {
          treeSummary = v;
        },
      });
    }
    console.log = write;
    console.log('OK');
  } catch (error) {
    console.log = write;
    throw new Error(
      `${error.message}; Page.getFrameTree=${JSON.stringify(treeSummary)}; probe=${JSON.stringify(probeEvents)}`
    );
  } finally {
    console.log = write;
    await handle?.close();
  }
}

async function runOrdinaryChild({ logged, setTree }) {
  const childOrigin = site.replace('127.0.0.1', 'child.other.test');
  const tab = await browser.ensureTab(`${site}/framed`, { matchUrl: '/framed$' });
  const handle = await browser.websocket
    .on(tab, { urlMatch: new RegExp(allowedWs.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')) })
    .filter({ parseAs: 'json', where: { kind: 'tick' } })
    .forward({ sink: 'log' });
  await browser.evalAsync(
    tab,
    `new Promise((resolve) => {
      const frame = document.createElement('iframe');
      frame.id = 'child';
      frame.src = ${JSON.stringify(`${childOrigin}/child-frame`)};
      frame.onload = () => resolve('loaded');
      setTimeout(() => resolve('timeout'), 5000);
      document.body.appendChild(frame);
    })`
  );
  await delay(500);
  const endpoint = await resolveCdpEndpoint();
  const probe = await connect({ url: endpoint.url });
  try {
    const sessionId = await probe.attach(tab.targetId);
    const tree = await probe.send('Page.getFrameTree', {}, sessionId);
    setTree(summarizeTree(tree?.frameTree));
  } finally {
    probe.close();
  }
  logged.length = 0;
  await browser.evalAsync(
    tab,
    `new Promise((resolve, reject) => {
      const ws = new WebSocket(${JSON.stringify(allowedWs)});
      const timer = setTimeout(() => reject(new Error('main ws timeout')), 8000);
      ws.onerror = () => { clearTimeout(timer); reject(new Error('main ws error')); };
      ws.onopen = () => { ws.send('ping'); clearTimeout(timer); resolve('opened'); };
    })`
  );
  await waitFor(
    () => logged.some((line) => line.includes('"n":1') || line.includes('"n": 1')),
    `routing stopped while a child frame was present; logged=${JSON.stringify(logged)}`
  );
  await log('websocket:child-ordinary');
  await log('done');
  return handle;
}

async function runPremise({ logged, probeEvents, setTree }) {
  const protectedOrigin = site.replace('127.0.0.1', 'seven.sliccy.ai');
  const mainWs = `${allowedWs}?from=main`;
  const tab = await browser.ensureTab(`${site}/framed`, { matchUrl: '/framed$' });
  const handle = await browser.websocket
    .on(tab, { urlMatch: /./u })
    .filter({ parseAs: 'json', where: { kind: 'tick' } })
    .forward({ sink: 'log' });
  const endpoint = await resolveCdpEndpoint();
  const probe = await connect({ url: endpoint.url });
  try {
    const pageSession = await probe.attach(tab.targetId);
    await probe.send('Page.enable', {}, pageSession).catch(() => undefined);
    await probe.send('Network.enable', {}, pageSession).catch(() => undefined);
    await probe.send('Target.setDiscoverTargets', { discover: true }).catch(() => undefined);
    for (const method of [
      'Network.webSocketCreated',
      'Network.webSocketFrameReceived',
      'Page.frameAttached',
      'Page.frameDetached',
      'Target.targetCreated',
      'Target.targetInfoChanged',
    ]) {
      probe.on(method, (message) => recordProbe(probeEvents, method, message));
    }
    await browser.evalAsync(
      tab,
      `new Promise((resolve) => {
        const frame = document.createElement('iframe');
        frame.id = 'child';
        frame.src = ${JSON.stringify(`${protectedOrigin}/child-socket`)};
        frame.onload = () => resolve('loaded');
        setTimeout(() => resolve('timeout'), 8000);
        document.body.appendChild(frame);
      })`
    );
    await waitFor(
      async () =>
        (await fixtureLogHas('fixture:ws-accepted:from=child')) &&
        (await fixtureLogHas('fixture:ws-answered:from=child')),
      'premise: the cross-site child never opened its socket'
    );
    const tree = await probe.send('Page.getFrameTree', {}, pageSession);
    setTree(summarizeTree(tree?.frameTree));
    const targets = await probe.send('Target.getTargets');
    const iframeTargets = (targets?.targetInfos ?? [])
      .filter((info) => info && (info.type === 'iframe' || info.type === 'page'))
      .map((info) => ({ targetId: info.targetId, type: info.type, url: info.url }));
    await browser.evalAsync(
      tab,
      `new Promise((resolve, reject) => {
        const ws = new WebSocket(${JSON.stringify(mainWs)});
        const timer = setTimeout(() => reject(new Error('main ws timeout')), 8000);
        ws.onerror = () => { clearTimeout(timer); reject(new Error('main ws error')); };
        ws.onopen = () => { ws.send('ping'); clearTimeout(timer); resolve('opened'); };
      })`
    );
    await waitFor(
      () =>
        logged.some(
          (line) =>
            (line.includes('"n":1') || line.includes('"n": 1')) &&
            (line.includes('"from":"main"') || line.includes('"from": "main"'))
        ),
      `main frame tick missing; targets=${JSON.stringify(iframeTargets)} logged=${JSON.stringify(logged)}`
    );
    const childTicks = logged.filter(
      (line) => line.includes('"n":2') || line.includes('"from":"child"')
    );
    const childSocketIds = new Set(
      probeEvents
        .filter(
          (event) =>
            event.method === 'Network.webSocketCreated' &&
            typeof event.url === 'string' &&
            event.url.includes('from=child') &&
            typeof event.requestId === 'string'
        )
        .map((event) => event.requestId)
    );
    const childCreatedOnPage = probeEvents.some(
      (event) =>
        event.method === 'Network.webSocketCreated' &&
        typeof event.url === 'string' &&
        event.url.includes('from=child')
    );
    const childFrameOnPage = probeEvents.some(
      (event) =>
        event.method === 'Network.webSocketFrameReceived' &&
        typeof event.requestId === 'string' &&
        childSocketIds.has(event.requestId)
    );
    if (childCreatedOnPage || childFrameOnPage) {
      throw new Error(
        `premise: cross-site child socket events reached the page session; targets=${JSON.stringify(iframeTargets)}`
      );
    }
    if (childTicks.length > 0) {
      throw new Error(
        `frames of a socket opened in another process reached the subscriber; childTicks=${JSON.stringify(childTicks)}`
      );
    }
    await log('websocket:premise-other-process');
    await log('done');
  } finally {
    probe.close();
  }
  return handle;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
