import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const runners = {
  native: async (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: async (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};
const WEBKIT = 'Crest (WebKit)';

async function withBrowser(runner, options, body) {
  const fake = await fakeCdp(options);
  fake.targets.push({ targetId: 'TAB', url: 'http://example.test/' });
  const pw = await runner(fake.url);
  try {
    const cwd = await pw.fresh();
    await body((args) => pw.playwright(args, cwd), fake);
  } finally {
    await pw.close();
    await fake.close();
  }
}

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: route says at once that this browser can't, and saves nothing`, async () => {
    await withBrowser(runner, { product: WEBKIT }, async (run) => {
      const added = await run(['route', '**/data.json', '--status=500']);
      assert.equal(added.code, 1);
      assert.match(added.stderr, /this browser can't pause requests/);
      assert.match((await run(['route-list'])).stdout, /No active routes/);
      assert.equal((await run(['eval', '1'])).code, 0);
    });
  });

  test(`${transport}: offline says at once that this browser can't, and saves nothing`, async () => {
    await withBrowser(runner, { product: WEBKIT }, async (run) => {
      const offline = await run(['network-state-set', 'offline']);
      assert.equal(offline.code, 1);
      assert.match(offline.stderr, /this browser can't take a tab offline/);
      assert.equal((await run(['eval', '1'])).code, 0);
      assert.equal((await run(['network-state-set', 'online'])).code, 0);
    });
  });
}

test('route and offline still save on a browser that reports Chrome', async () => {
  await withBrowser(runners.native, {}, async (run) => {
    assert.equal((await run(['route', '**/data.json', '--status=500'])).code, 0);
    assert.match((await run(['route-list'])).stdout, /data\.json/);
    assert.equal((await run(['unroute'])).code, 0);
    assert.equal((await run(['network-state-set', 'offline'])).code, 0);
    assert.equal((await run(['network-state-set', 'online'])).code, 0);
  });
});

test('a saved route a tab can not run names the way out', async () => {
  const refused = (message, send) => {
    if (message.method !== 'Fetch.enable' || message.params.patterns?.[0]?.resourceType)
      return false;
    const error = { code: -32000, message: "Fetch.enable isn't available on WebKit: no" };
    send({ id: message.id, sessionId: message.sessionId, error });
    return true;
  };
  await withBrowser(runners.native, { intercept: refused }, async (run) => {
    assert.equal((await run(['route', '**/data.json'])).code, 0);
    const result = await run(['eval', '1']);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /every command on it fails until you run unroute/);
  });
});

test('dialog-accept says the person answers on a browser that never reports dialogs', async () => {
  await withBrowser(runners.native, { product: WEBKIT }, async (run) => {
    assert.equal((await run(['open', 'http://example.test/page'])).code, 0);
    const result = await run(['dialog-accept']);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /shows a page's dialogs to the person/);
  });
});

test('dialog-accept still reports an unseen dialog on Chrome', async () => {
  await withBrowser(runners.native, {}, async (run) => {
    assert.equal((await run(['open', 'http://example.test/page'])).code, 0);
    const result = await run(['dialog-accept']);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /No dialog visible to this session/);
  });
});

test('resize says the browser sets its own size', async () => {
  const unknown = (message, send) => {
    if (message.method !== 'Browser.setWindowBounds') return false;
    const error = { code: -32601, message: "'Browser.setWindowBounds' wasn't found" };
    send({ id: message.id, sessionId: message.sessionId, error });
    return true;
  };
  await withBrowser(runners.native, { intercept: unknown }, async (run) => {
    const result = await run(['resize', '800', '600']);
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      "Error: this browser sets its own window size, so resize can't change it\n"
    );
  });
});
