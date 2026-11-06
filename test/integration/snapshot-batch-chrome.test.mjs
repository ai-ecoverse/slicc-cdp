import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { runProcess } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const playwright = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const BUTTONS = 150;

async function site() {
  const server = createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(
      req.url === '/other'
        ? '<title>Other</title><p>other document</p>'
        : `<title>Buttons</title>${Array.from({ length: BUTTONS }, (_, i) => `<button>b${i}</button>`).join('')}`
    );
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    close: () => {
      server.closeAllConnections();
      server.close();
    },
  };
}

async function snapshotThrough(proxy, origin, arm) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-batch-'));
  const cli = (...args) => runProcess(playwright, ['--cdp', proxy.url, ...args], cwd, 120000);
  try {
    const opened = await cli('open', `${origin}/`);
    assert.equal(opened.code, 0, opened.stderr);
    arm();
    return await cli('snapshot');
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
}

test('Chrome: snapshot describes its elements in one checked, pipelined batch', {
  timeout: 180000,
}, async (t) => {
  const pages = await site();
  t.after(() => pages.close());
  const browser = await launchBrowser();
  t.after(() => browser.close());
  const seen = { describe: 0, inventories: 0, inFlight: 0, most: 0 };
  const describing = new Set();
  const state = { armed: false };
  const proxy = await cdpProxy(browser.wsUrl, {
    observe(direction, message) {
      if (!state.armed) return;
      if (direction === 'client' && message.method === 'DOM.describeNode') {
        seen.describe += 1;
        describing.add(message.id);
        seen.most = Math.max(seen.most, describing.size);
      }
      if (direction === 'client' && message.method === 'Target.getTargets') seen.inventories += 1;
      if (direction === 'browser' && message.id !== undefined) describing.delete(message.id);
    },
  });
  t.after(() => proxy.close());
  const snapshot = await snapshotThrough(proxy, pages.origin, () => {
    state.armed = true;
  });
  assert.equal(snapshot.code, 0, snapshot.stderr);
  for (const i of [0, BUTTONS - 1])
    assert.match(snapshot.stdout, new RegExp(`button "b${i}" \\[ref=e\\d+\\]`));
  assert.ok(seen.describe >= BUTTONS, `described ${seen.describe} elements`);
  assert.ok(seen.most > 1, 'the describe calls were pipelined');
  assert.ok(
    seen.inventories <= 20,
    `the page scope was checked ${seen.inventories} times for ${seen.describe} elements`
  );
});

test('Chrome: a navigation while the batch is described refuses the whole snapshot', {
  timeout: 180000,
}, async (t) => {
  const pages = await site();
  t.after(() => pages.close());
  const browser = await launchBrowser();
  t.after(() => browser.close());
  const raw = await rawSocket(browser.wsUrl);
  t.after(() => raw.close());
  const state = { armed: false, ids: new Set(), replies: 0, navigated: null };
  const proxy = await cdpProxy(browser.wsUrl, {
    observe(direction, message) {
      if (state.armed && direction === 'client' && message.method === 'DOM.describeNode')
        state.ids.add(message.id);
    },
    responseBarrier(message) {
      if (!state.ids.has(message.id)) return null;
      state.replies += 1;
      if (state.replies !== 10) return null;
      state.navigated = (async () => {
        const { targetInfos } = await raw.send('Target.getTargets');
        const page = targetInfos.find(
          (info) => info.type === 'page' && info.url.startsWith(pages.origin)
        );
        const { sessionId } = await raw.send('Target.attachToTarget', {
          targetId: page.targetId,
          flatten: true,
        });
        await raw.send('Page.enable', {}, sessionId);
        const loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url: `${pages.origin}/other` }, sessionId);
        await loaded.seen;
        await raw.send('Target.detachFromTarget', { sessionId });
      })();
      return state.navigated;
    },
  });
  t.after(() => proxy.close());
  const snapshot = await snapshotThrough(proxy, pages.origin, () => {
    state.armed = true;
  });
  await state.navigated;
  assert.ok(state.replies >= 10, `the batch got ${state.replies} replies`);
  assert.notEqual(snapshot.code, 0, snapshot.stdout);
  assert.match(snapshot.stderr, /^slicc: refused: /);
  assert.doesNotMatch(snapshot.stdout, /button "b\d+"/);
});
