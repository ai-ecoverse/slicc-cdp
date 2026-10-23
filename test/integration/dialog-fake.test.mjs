import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

const binary = process.env.SLICC_DIALOG_BIN || path.resolve('target/debug/playwright-cli');
const unobserved =
  'Error: No dialog visible to this session. Dialogs already open before attachment require the foreground dialog holder (follow-up #529); arm dialog-accept or dialog-dismiss before triggering a dialog.\n';

function run(args, home) {
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
    const child = spawn(binary, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error('dialog command timed out'));
    }, 20000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

async function withFake(options, body) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-dialog-'));
  const fake = await fakeCdp(options);
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  try {
    await body(fake, (args) => run(['--cdp', fake.url, ...args], home));
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

function opening(message, send, overrides = {}) {
  send({
    method: 'Page.javascriptDialogOpening',
    sessionId: message.sessionId,
    params: {
      url: 'https://example.test/',
      frameId: 'TAB',
      type: 'prompt',
      message: 'name?',
      hasBrowserHandler: true,
      ...overrides,
    },
  });
}

const handled = (fake) =>
  fake.calls.filter((call) => call.method === 'Page.handleJavaScriptDialog');
const renderer = new Set(['Network.enable', 'Runtime.enable', 'Page.getFrameTree']);

test('a dialog-only attach answers a future prompt without any renderer admission', async () => {
  let timer;
  const intercept = (message, send) => {
    if (message.method === 'Page.enable') {
      timer = setTimeout(() => opening(message, send), 30);
      return true;
    }
    if (renderer.has(message.method)) return true;
    return false;
  };
  try {
    await withFake({ intercept }, async (fake, pw) => {
      const result = await pw(['dialog-accept', 'Lars', 'Trieloff']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, 'Accepted the dialog\n');
      assert.deepEqual(
        handled(fake).map((call) => call.params),
        [{ accept: true, promptText: 'Lars Trieloff' }]
      );
      assert.equal(fake.calls.filter((call) => renderer.has(call.method)).length, 0);
      assert.equal(fake.calls.filter((call) => call.method === 'Target.attachToTarget').length, 1);
      assert.ok(
        fake.calls.filter((call) => call.method === 'Page.getNavigationHistory').length >= 2
      );
    });
  } finally {
    clearTimeout(timer);
  }
});

test('dialog-dismiss answers only an observed ordinary main-frame dialog', async () => {
  await withFake(
    {
      intercept: (message, send) => {
        if (message.method === 'Page.enable') {
          opening(message, send);
          return true;
        }
        return false;
      },
    },
    async (fake, pw) => {
      const result = await pw(['dialog-dismiss']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, 'Dismissed the dialog\n');
      assert.deepEqual(
        handled(fake).map((call) => call.params),
        [{ accept: false }]
      );
    }
  );
});

test('unobserved and already-open dialogs never cause an unconditional answer', async () => {
  await withFake(
    { intercept: (message) => message.method === 'Page.enable' || renderer.has(message.method) },
    async (fake, pw) => {
      const result = await pw(['dialog-accept']);
      assert.equal(result.code, 1);
      assert.equal(result.stdout, '');
      assert.equal(result.stderr, unobserved);
      assert.equal(handled(fake).length, 0);
      assert.equal(fake.calls.filter((call) => renderer.has(call.method)).length, 0);
    }
  );
});

test('protected, unknown and subframe dialog ownership is refused', async () => {
  for (const params of [
    { url: 'https://seven.sliccy.ai/?secret=hidden' },
    { url: 'http://vault.sliccy.ai/vault?secret=hidden' },
    { url: '' },
    { url: 'about:blank' },
    { frameId: '' },
    { frameId: 'CHILD' },
    { url: 'https://other.test/' },
  ]) {
    await withFake(
      {
        intercept: (message, send) => {
          if (message.method === 'Page.enable') {
            opening(message, send, params);
            return true;
          }
          return false;
        },
      },
      async (fake, pw) => {
        const result = await pw(['dialog-accept', 'injected']);
        assert.equal(result.code, 1, JSON.stringify(params));
        assert.match(result.stderr, /refused/);
        assert.ok(!result.stderr.includes('hidden'));
        assert.equal(handled(fake).length, 0);
      }
    );
  }
});

test('browser-side ownership is rechecked and an intervening close revokes it', async () => {
  for (const change of ['history', 'close', 'replacement']) {
    let histories = 0;
    await withFake(
      {
        intercept: (message, send) => {
          if (message.method === 'Page.enable') {
            opening(message, send);
            return true;
          }
          if (message.method === 'Page.getNavigationHistory' && ++histories > 1) {
            if (change !== 'history')
              send({
                method: 'Page.javascriptDialogClosed',
                sessionId: message.sessionId,
                params: { frameId: 'TAB' },
              });
            if (change === 'replacement') opening(message, send);
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: {
                currentIndex: 0,
                entries: [
                  {
                    id: 1,
                    url:
                      change === 'history' ? 'https://seven.sliccy.ai/' : 'https://example.test/',
                  },
                ],
              },
            });
            return true;
          }
          return false;
        },
      },
      async (fake, pw) => {
        const result = await pw(['dialog-dismiss']);
        assert.equal(result.code, 1, change);
        assert.match(result.stderr, /refused/);
        assert.equal(handled(fake).length, 0);
      }
    );
  }
});

test('protected and unverifiable targets are refused before a dialog attach', async () => {
  await withFake({}, async (fake, pw) => {
    for (const url of [
      'https://seven.sliccy.ai/?secret=hidden',
      'http://vault.sliccy.ai/',
      '',
      'about:blank',
    ]) {
      fake.targets.push({ targetId: 'REFUSED', url });
      const before = fake.calls.length;
      const result = await pw(['dialog-dismiss', '--tab', 'REFUSED']);
      assert.equal(result.code, 1);
      assert.ok(!result.stderr.includes('hidden'));
      assert.equal(
        fake.calls
          .slice(before)
          .filter(
            (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'REFUSED'
          ).length,
        0
      );
      assert.equal(handled(fake).length, 0);
      fake.targets.pop();
    }
  });
});

test('an armed dialog command releases a click blocked on its input acknowledgement', async () => {
  let enable;
  const armed = new Promise((resolve) => {
    enable = resolve;
  });
  const listeners = new Map();
  let arming = false;
  let pending;
  await withFake(
    {
      intercept: (message, send) => {
        if (message.method === 'Page.enable' && arming) {
          listeners.set(message.sessionId, { message, send });
          enable();
        }
        if (
          message.method === 'Input.dispatchMouseEvent' &&
          message.params.type === 'mouseReleased'
        ) {
          pending = () =>
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { result: { type: 'undefined' } },
            });
          for (const listener of listeners.values()) opening(listener.message, listener.send);
          return true;
        }
        if (message.method === 'Page.handleJavaScriptDialog') {
          assert.ok(pending, 'click was not in flight');
          send({ id: message.id, sessionId: message.sessionId, result: {} });
          pending();
          return true;
        }
        return false;
      },
    },
    async (fake, pw) => {
      const snapshot = await pw(['snapshot']);
      assert.equal(snapshot.code, 0, snapshot.stderr);
      arming = true;
      const accept = pw(['dialog-accept']);
      await armed;
      const click = pw(['click', 'e2']);
      const [accepted, clicked] = await Promise.all([accept, click]);
      assert.equal(clicked.code, 0, clicked.stderr);
      assert.equal(accepted.code, 0, accepted.stderr);
      assert.equal(accepted.stdout, 'Accepted the dialog\n');
      assert.equal(clicked.stdout, 'Clicked e2\n');
      assert.equal(handled(fake).length, 1);
    }
  );
});

test('kernel dialog-only attach handles an observed dialog without renderer admission', async () => {
  const fake = await fakeCdp({
    intercept: (message, send) => {
      if (message.method === 'Page.enable') {
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        opening(message, send);
        return true;
      }
      return false;
    },
  });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      fake.url
    );
    const result = await runner.playwright(['dialog-accept', 'WASI'], await runner.fresh());
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout, 'Accepted the dialog\n');
    assert.deepEqual(
      handled(fake).map((call) => call.params),
      [{ accept: true, promptText: 'WASI' }]
    );
    assert.equal(fake.calls.filter((call) => renderer.has(call.method)).length, 0);
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await fake.close();
  }
});
