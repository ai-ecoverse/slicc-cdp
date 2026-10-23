import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest } from './chrome-policy.mjs';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);
const REFUSED = /^slicc: refused: /;
const linked = 'data:text/html,<title>linked-browser</title>';
const ordinary = 'http://localhost:43117/driven.html';
const SEVEN = 'https://seven.sliccy.ai/';
const VAULT = 'https://vault.sliccy.ai';

function native(args, home) {
  return new Promise((resolve, reject) => {
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      APPDATA: home,
      XDG_CONFIG_HOME: home,
      XDG_STATE_HOME: home,
      SLICC_CDP_PORTS: HARNESS_PORTS,
    };
    delete env.SLICC_CDP_URL;
    delete env.PLAYWRIGHT_CLI_SESSION;
    const spawned = spawn(exe('playwright-cli'), args, {
      cwd: home,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const out = [];
    const err = [];
    spawned.stdout.on('data', (chunk) => out.push(chunk));
    spawned.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      spawned.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 30000);
    spawned.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    spawned.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

async function nativeFake() {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-data-document-'));
  return {
    run: (url, args) => native(['--cdp', url, ...args], home),
    close: () => rm(home, { recursive: true, force: true }),
  };
}

async function wasiFake(url) {
  const kernel = await kernelRunner(
    path.join(wasmDir, 'playwright-cli.wasm'),
    path.join(wasmDir, 'curlwright.wasm'),
    url
  );
  const cwd = await kernel.fresh();
  return {
    run: (_url, args) => kernel.playwright(args, cwd),
    close: () => kernel.close(),
  };
}

const opaque = (url) => {
  const origin = new URL(url).origin;
  return origin === 'null' ? '://' : origin;
};

const described = (target) => ({
  targetId: target.targetId,
  type: target.type ?? 'page',
  ...(target.openerId ? { openerId: target.openerId } : {}),
  url: target.url,
  title: target.url,
  attached: false,
  canAccessOpener: false,
  browserContextId: 'CTX',
});

function linkedRelay(state) {
  return (message, send) => {
    const { id, method, params = {}, sessionId } = message;
    const fake = state.fake;
    const browser = (name, targetInfo) =>
      send({ method: name, params: { targetInfo }, sessionId: 'BROWSER' });
    if (method === 'Target.attachToBrowserTarget') {
      send({ id, result: { sessionId: 'BROWSER' } });
      return true;
    }
    if (method === 'Target.createTarget') {
      queueMicrotask(() => browser('Target.targetCreated', described(fake.targets.at(-1))));
      return false;
    }
    const tab = fake.targets.find((item) => item.targetId === fake.calls.at(-1)?.target);
    if (method !== 'Page.navigate' || !tab || !params.url.startsWith('data:')) return false;
    const loaderId = `LD${fake.calls.length}`;
    tab.url = params.url;
    tab.loaderId = loaderId;
    delete tab.frame;
    send({ id, result: { frameId: tab.targetId, loaderId }, sessionId });
    const frame = { id: tab.targetId, loaderId, url: params.url, securityOrigin: '://' };
    send({ method: 'Page.frameNavigated', params: { frame, type: 'Navigation' }, sessionId });
    send({ method: 'Page.domContentEventFired', params: { timestamp: 1 }, sessionId });
    send({ method: 'Page.loadEventFired', params: { timestamp: 1 }, sessionId });
    browser('Target.targetInfoChanged', described(tab));
    return true;
  };
}

async function overLinkedRelay(start, body) {
  const state = {};
  const fake = await fakeCdp({ intercept: linkedRelay(state), attachEvents: true });
  state.fake = fake;
  const runner = await start(fake.url);
  try {
    await body((args) => runner.run(fake.url, args), fake);
  } finally {
    await runner.close();
    await fake.close();
  }
}

const sentTo = (fake, method, tab) =>
  fake.calls.filter((call) => call.method === method && call.target === tab.targetId);

const document = (tab, url, securityOrigin, childFrames = []) => {
  tab.url = url;
  tab.loaderId = `L${tab.targetId}-${url.length}`;
  tab.frame = { id: tab.targetId, loaderId: tab.loaderId, url, securityOrigin };
  tab.childFrames = childFrames;
};

const child = (tab, name, url, securityOrigin, childFrames = []) => ({
  frame: {
    id: `${tab.targetId}-${name}`,
    parentId: tab.targetId,
    loaderId: `L${name}`,
    url,
    securityOrigin,
  },
  childFrames: childFrames.map((item) => ({
    ...item,
    frame: { ...item.frame, parentId: `${tab.targetId}-${name}` },
  })),
});

for (const [label, start] of [
  ['native', nativeFake],
  ['wasi', wasiFake],
]) {
  test(`${label}: on a linked runtime, a data: page opens, evals, snapshots and goes to another data: page`, async () => {
    assert.ok(label === 'native' || existsSync(path.join(wasmDir, 'playwright-cli.wasm')));
    await overLinkedRelay(start, async (run, fake) => {
      const opened = await run(['open', linked]);
      assert.equal(opened.code, 0, opened.stderr);
      const tab = fake.targets.at(-1);
      assert.equal(tab.url, linked);
      const evaluated = await run(['eval', 'document.title']);
      assert.equal(evaluated.code, 0, evaluated.stderr);
      assert.equal(evaluated.stdout, 'fake-eval\n');
      const shot = await run(['snapshot']);
      assert.equal(shot.code, 0, shot.stderr);
      assert.match(shot.stdout, /button "Go"/);
      assert.equal(sentTo(fake, 'Page.createIsolatedWorld', tab).length, 1);
      const went = await run(['goto', 'data:text/html,<title>start</title>']);
      assert.equal(went.code, 0, went.stderr);
      assert.equal((await run(['eval', 'location.href'])).code, 0);
      const seven = await run(['goto', SEVEN]);
      assert.notEqual(seven.code, 0, seven.stdout);
      assert.match(seven.stderr, REFUSED);
      assert.equal(tab.url, 'data:text/html,<title>start</title>');
      assert.equal((await run(['eval', 'location.href'])).code, 0);
      const closed = await run(['close']);
      assert.equal(closed.code, 0, closed.stderr);
      assert.equal(fake.targets.length, 0);
    });
  });

  test(`${label}: on a linked runtime, a data: frame in an ordinary page snapshots`, async () => {
    await overLinkedRelay(start, async (run, fake) => {
      assert.equal((await run(['open', ordinary])).code, 0);
      const tab = fake.targets.at(-1);
      document(tab, ordinary, opaque(ordinary), [child(tab, 'DATA', linked, '://')]);
      const framed = await run(['snapshot', `--frame=${tab.targetId}-DATA`]);
      assert.equal(framed.code, 0, framed.stderr);
      assert.equal(
        fake.calls.filter(
          (call) =>
            call.method === 'Page.createIsolatedWorld' &&
            call.params.frameId === `${tab.targetId}-DATA`
        ).length,
        1
      );
    });
  });

  test(`${label}: on a linked runtime, other opaque documents, seven, the vault and unknown targets stay refused`, async () => {
    const cases = [
      ['a sandboxed page', (tab) => document(tab, ordinary, '://')],
      ['a blob: page', (tab) => document(tab, 'blob:null/0f1e2d3c', '://')],
      [
        'an about:blank opened by seven',
        (tab, fake) => {
          fake.targets.push({ targetId: 'SEVEN', url: SEVEN, loaderId: 'LS' });
          tab.openerId = 'SEVEN';
          document(tab, 'about:blank', '://');
        },
      ],
      ['a page on seven', (tab) => document(tab, SEVEN, 'https://seven.sliccy.ai')],
      ['a data: page reporting seven as its origin', (tab) => document(tab, linked, opaque(SEVEN))],
      ['a page on the vault', (tab) => document(tab, 'about:blank', VAULT)],
    ];
    for (const [name, mutate] of cases) {
      await overLinkedRelay(start, async (run, fake) => {
        assert.equal((await run(['open', linked])).code, 0, name);
        const tab = fake.targets.at(-1);
        mutate(tab, fake);
        for (const args of [['eval', 'document.title'], ['snapshot']]) {
          const refused = await run(args);
          assert.notEqual(refused.code, 0, `${name}: ${args[0]}`);
          assert.match(refused.stderr, REFUSED, `${name}: ${args[0]}`);
        }
        assert.deepEqual(sentTo(fake, 'Page.createIsolatedWorld', tab), [], name);
      });
    }
  });

  test(`${label}: on a linked runtime, opaque or protected frames and data: frames under them stay refused`, async () => {
    await overLinkedRelay(start, async (run, fake) => {
      assert.equal((await run(['open', ordinary])).code, 0);
      const tab = fake.targets.at(-1);
      const data = child(tab, 'DATA', linked, '://');
      const cases = [
        ['a sandboxed frame', 'SANDBOXED', [child(tab, 'SANDBOXED', ordinary, '://')]],
        ['a blob: frame', 'BLOB', [child(tab, 'BLOB', 'blob:null/0f1e2d3c', '://')]],
        ['a frame on the vault', 'VAULT', [child(tab, 'VAULT', 'about:blank', VAULT)]],
        [
          'a data: frame under the vault',
          'DATA',
          [child(tab, 'VAULT', 'about:blank', VAULT, [data])],
        ],
        ['a data: frame under seven', 'DATA', [child(tab, 'SEVEN', SEVEN, opaque(SEVEN), [data])]],
        [
          'a data: frame under a sandboxed frame',
          'DATA',
          [child(tab, 'SANDBOXED', ordinary, '://', [data])],
        ],
      ];
      for (const [name, id, childFrames] of cases) {
        document(tab, ordinary, opaque(ordinary), childFrames);
        const frame = `${tab.targetId}-${id}`;
        const refused = await run(['snapshot', `--frame=${frame}`]);
        assert.notEqual(refused.code, 0, name);
        assert.match(refused.stderr, REFUSED, name);
        assert.deepEqual(
          fake.calls.filter(
            (call) => call.method === 'Page.createIsolatedWorld' && call.params.frameId === frame
          ),
          [],
          name
        );
      }
      const unknown = await run(['eval', '--tab', 'NO-SUCH-TAB', 'document.title']);
      assert.notEqual(unknown.code, 0);
    });
  });
}

function framedSite() {
  const server = createServer((_req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(
      `<!doctype html><title>framed</title><iframe src="${linked.replace('linked-browser', 'framed-data')}"></iframe>`
    );
  });
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () =>
      resolve({
        origin: `http://127.0.0.1:${server.address().port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      })
    )
  );
}

for (const kind of ['native', 'kernel']) {
  chromeTest(
    `${kind}: in Chrome, a data: page opens, evals, snapshots and goes on, while seven stays refused`,
    async () => {
      const site = await startServer();
      const framedPage = await framedSite();
      let browser;
      let runner;
      try {
        browser = await launchBrowser();
        runner =
          kind === 'native'
            ? nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl)
            : await kernelRunner(
                path.join(wasmDir, 'playwright-cli.wasm'),
                path.join(wasmDir, 'curlwright.wasm'),
                browser.wsUrl
              );
        const cwd = await runner.fresh();
        const opened = await runner.playwright(['open', linked], cwd);
        assert.equal(opened.code, 0, opened.stderr);
        const title = await runner.playwright(['eval', 'document.title'], cwd);
        assert.equal(title.code, 0, title.stderr);
        assert.equal(title.stdout, 'linked-browser\n');
        const shot = await runner.playwright(['snapshot'], cwd);
        assert.equal(shot.code, 0, shot.stderr);
        const went = await runner.playwright(['goto', 'data:text/html,<title>start</title>'], cwd);
        assert.equal(went.code, 0, went.stderr);
        const href = await runner.playwright(['eval', 'location.href'], cwd);
        assert.equal(href.code, 0, href.stderr);
        assert.equal(href.stdout, 'data:text/html,<title>start</title>\n');
        const seven = await runner.playwright(['goto', SEVEN], cwd);
        assert.notEqual(seven.code, 0, seven.stdout);
        assert.match(seven.stderr, REFUSED);
        const stayed = await runner.playwright(['eval', 'location.href'], cwd);
        assert.equal(stayed.code, 0, stayed.stderr);
        assert.equal(stayed.stdout, 'data:text/html,<title>start</title>\n');
        const ordinaryPage = await runner.playwright(['goto', `${site.origin}/cb`], cwd);
        assert.equal(ordinaryPage.code, 0, ordinaryPage.stderr);
        const framed = await runner.playwright(['goto', `${framedPage.origin}/framed`], cwd);
        assert.equal(framed.code, 0, framed.stderr);
        const frames = await runner.playwright(['snapshot'], cwd);
        assert.equal(frames.code, 0, frames.stderr);
        const closed = await runner.playwright(['close'], cwd);
        assert.equal(closed.code, 0, closed.stderr);
      } finally {
        await Promise.resolve(runner?.close()).catch(() => {});
        await browser?.close().catch(() => {});
        await framedPage.close().catch(() => {});
        await site.close().catch(() => {});
      }
    }
  );
}
