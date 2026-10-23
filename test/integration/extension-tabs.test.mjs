import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const REFUSED = "slicc: refused: target is on seven's origin";
const driven = 'http://localhost:43117/driven.html';
const child = 'http://localhost:43118/child.html';

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
    const spawned = spawn(cli, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
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

async function nativeRunner() {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-extension-tabs-'));
  return {
    run: (url, args) => native(['--cdp', url, ...args], home),
    close: () => rm(home, { recursive: true, force: true }),
  };
}

async function wasiRunner(url) {
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

const originOf = (url) => new URL(url).origin;
const mainFrame = (tab) => (Number(tab) * 7919).toString(16).toUpperCase().padStart(32, '0');

function extensionTabs(state, children) {
  let tabs = 1736;
  return (message, send) => {
    const { id, method, params = {}, sessionId } = message;
    const fake = state.fake;
    const answer = (result) => send({ id, result, ...(sessionId ? { sessionId } : {}) });
    const event = (name, eventParams) => send({ method: name, params: eventParams, sessionId });
    const tab = fake.targets.find((item) => item.targetId === fake.calls.at(-1)?.target);
    const main = tab && mainFrame(tab.targetId);
    if (method === 'Target.createTarget') {
      tabs += 1;
      fake.targets.push({ targetId: String(tabs), url: params.url || 'about:blank' });
      answer({ targetId: String(tabs) });
      return true;
    }
    if (
      method === 'Target.getTargetInfo' &&
      typeof params.targetId === 'string' &&
      !fake.targets.some((item) => item.targetId === params.targetId)
    ) {
      send({
        id,
        error: {
          code: -32000,
          message: `slicc: refused: Invalid targetId: ${params.targetId} is not a tab`,
        },
      });
      return true;
    }
    if (!tab) return false;
    if (method === 'Page.getFrameTree') {
      answer({
        frameTree: {
          frame: {
            id: main,
            loaderId: tab.loaderId ?? `L${tab.targetId}`,
            url: tab.url,
            securityOrigin: tab.origin ?? originOf(tab.url),
          },
          childFrames: tab.url === 'about:blank' ? [] : children(main),
        },
      });
      return true;
    }
    if (method === 'Page.navigate') {
      tab.url = params.url;
      tab.loaderId = `L${tab.targetId}-2`;
      answer({ frameId: main, loaderId: `L${tab.targetId}-2` });
      event('Page.frameNavigated', {
        frame: {
          id: main,
          loaderId: `L${tab.targetId}-2`,
          url: params.url,
          securityOrigin: originOf(params.url),
        },
      });
      event('Page.domContentEventFired', { timestamp: 1 });
      event('Page.loadEventFired', { timestamp: 1 });
      return true;
    }
    if (method === 'Runtime.enable') {
      event('Runtime.executionContextCreated', {
        context: {
          id: 1,
          uniqueId: `main:${tab.targetId}:${tab.loaderId ?? `L${tab.targetId}`}`,
          origin: tab.origin ?? originOf(tab.url),
          name: '',
          auxData: { frameId: main, isDefault: true },
        },
      });
      for (const world of tab.worlds ?? [])
        event('Runtime.executionContextCreated', { context: world });
      answer({});
      return true;
    }
    if (
      method === 'Runtime.evaluate' &&
      params.expression?.startsWith('JSON.stringify({href:location.href')
    ) {
      answer({
        result: { type: 'string', value: JSON.stringify({ href: tab.url, title: 'Driven' }) },
      });
      return true;
    }
    return false;
  };
}

function frames(main) {
  return [
    {
      frame: {
        id: `${main}-PLAIN`,
        parentId: main,
        loaderId: 'LP',
        url: child,
        securityOrigin: originOf(child),
      },
    },
    {
      frame: {
        id: `${main}-SEVEN`,
        parentId: main,
        loaderId: 'LS',
        url: 'https://seven.sliccy.ai/',
        securityOrigin: 'https://seven.sliccy.ai',
      },
    },
    {
      frame: {
        id: `${main}-VAULT`,
        parentId: main,
        loaderId: 'LV',
        url: 'about:blank',
        securityOrigin: 'https://vault.sliccy.ai',
      },
    },
  ];
}

async function overExtension(start, body) {
  const state = {};
  const fake = await fakeCdp({ intercept: extensionTabs(state, frames) });
  state.fake = fake;
  const runner = await start(fake.url);
  try {
    await body((args) => runner.run(fake.url, args), fake);
  } finally {
    await runner.close();
    await fake.close();
  }
}

const isolated = (fake, frame) =>
  fake.calls.filter(
    (call) => call.method === 'Page.createIsolatedWorld' && call.params.frameId === frame
  );

for (const [label, start] of [
  ['native', nativeRunner],
  ['wasi', wasiRunner],
]) {
  test(`${label}: over slicc-extension 7.6's tab ids, an ordinary localhost page on another port opens, goes and snapshots`, async () => {
    assert.ok(label === 'native' || existsSync(path.join(wasmDir, 'playwright-cli.wasm')));
    await overExtension(start, async (run, fake) => {
      const opened = await run(['open', 'about:blank']);
      assert.equal(opened.code, 0, opened.stderr);
      const went = await run(['goto', driven]);
      assert.equal(went.code, 0, went.stderr);
      const shot = await run(['snapshot']);
      assert.equal(shot.code, 0, shot.stderr);
      assert.match(shot.stdout, /button "Go"/);
      const tab = fake.targets.at(-1);
      assert.equal(tab.url, driven);
      assert.equal(isolated(fake, mainFrame(tab.targetId)).length, 1);
      const framed = await run(['snapshot', `--frame=${mainFrame(tab.targetId)}-PLAIN`]);
      assert.equal(framed.code, 0, framed.stderr);
      assert.equal(isolated(fake, `${mainFrame(tab.targetId)}-PLAIN`).length, 1);
    });
  });

  test(`${label}: over slicc-extension 7.6's tab ids, frames on seven's origin or the vault stay refused`, async () => {
    await overExtension(start, async (run, fake) => {
      assert.equal((await run(['open', driven])).code, 0);
      const tab = fake.targets.at(-1);
      for (const name of ['SEVEN', 'VAULT']) {
        const frame = `${mainFrame(tab.targetId)}-${name}`;
        const refused = await run(['snapshot', `--frame=${frame}`]);
        assert.notEqual(refused.code, 0, name);
        assert.equal(refused.stderr, `${REFUSED}\n`, name);
        assert.deepEqual(isolated(fake, frame), [], name);
      }
    });
  });

  test(`${label}: over slicc-extension 7.6's tab ids, a tab whose document is on seven's origin or the vault stays refused`, async () => {
    for (const [url, origin] of [
      ['https://seven.sliccy.ai/', undefined],
      [driven, 'https://seven.sliccy.ai'],
      ['about:blank', 'https://vault.sliccy.ai'],
    ]) {
      await overExtension(start, async (run, fake) => {
        assert.equal((await run(['open', driven])).code, 0);
        const tab = fake.targets.at(-1);
        tab.url = url;
        tab.origin = origin;
        const refused = await run(['snapshot']);
        assert.notEqual(refused.code, 0, `${url} ${origin}`);
        assert.match(refused.stderr, /^slicc: refused: /, `${url} ${origin}`);
        assert.deepEqual(isolated(fake, mainFrame(tab.targetId)), [], `${url} ${origin}`);
      });
    }
  });
}
