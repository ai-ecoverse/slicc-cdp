import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

const foregrounding = [
  'Target.activateTarget',
  'Page.bringToFront',
  'Emulation.setFocusEmulationEnabled',
];

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
    delete env.PLAYWRIGHT_CLI_SESSION;
    const child = spawn(cli, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 30000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

async function withFake(body, options) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-background-'));
  const fake = await fakeCdp(options);
  try {
    const pw = async (args) => {
      const result = await run(['--cdp', fake.url, ...args], home);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result;
    };
    await body(fake, pw, (args) => run(['--cdp', fake.url, ...args], home));
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

function sent(calls, from, method) {
  return calls.slice(from).filter((call) => call.method === method);
}

function createsInBackground(calls, from) {
  for (const call of sent(calls, from, 'Target.createTarget')) {
    assert.equal(call.params?.background, true, JSON.stringify(call));
  }
}

test('tab-select keeps the browser foreground tab unless --foreground is given', {
  timeout: 60_000,
}, async () => {
  await withFake(async (fake, pw) => {
    await pw(['open', 'about:blank']);
    await pw(['tab-new', 'about:blank']);
    let from = fake.calls.length;
    const selected = await pw(['tab-select', '1']);
    assert.match(selected.stdout, /Selected tab 1 \[targetId: TAB1\]/);
    assert.deepEqual(sent(fake.calls, from, 'Target.activateTarget'), []);
    const current = await pw(['eval', 'document.title']);
    assert.match(current.stdout, /fake-eval/);
    const evaluated = fake.calls.filter(
      (call) => call.method === 'Runtime.evaluate' && call.params?.expression === 'document.title'
    );
    assert.equal(evaluated.at(-1)?.target, 'TAB1');
    for (const flag of ['--foreground', '--fg']) {
      from = fake.calls.length;
      await pw(['tab-select', '2', flag]);
      const activated = sent(fake.calls, from, 'Target.activateTarget');
      assert.equal(activated.length, 1, flag);
      assert.equal(activated[0].params.targetId, 'TAB2');
    }
  });
});

test('tab-select --foreground reports a failed activation and keeps the current tab', {
  timeout: 60_000,
}, async () => {
  await withFake(
    async (fake, pw, raw) => {
      await pw(['open', 'about:blank']);
      await pw(['tab-new', 'about:blank']);
      const failed = await raw(['tab-select', '1', '--fg']);
      assert.notEqual(failed.code, 0, failed.stdout);
      assert.match(failed.stderr, /Target\.activateTarget refused/);
      await pw(['eval', 'document.title']);
      const evaluated = fake.calls.filter(
        (call) => call.method === 'Runtime.evaluate' && call.params?.expression === 'document.title'
      );
      assert.equal(evaluated.at(-1)?.target, 'TAB2');
    },
    { refuse: ['Target.activateTarget'] }
  );
});

test('open, tab-new, goto, snapshot, click, eval and tab-select never foreground a tab', {
  timeout: 60_000,
}, async () => {
  await withFake(async (fake, pw) => {
    await pw(['open', 'about:blank']);
    await pw(['tab-new', 'http://example.test/second']);
    await pw(['goto', 'http://example.test/page']);
    const snapshot = await pw(['snapshot']);
    const ref = snapshot.stdout.match(/\[ref=(e\d+)\]/)?.[1];
    assert.ok(ref, snapshot.stdout);
    await pw(['click', ref]);
    await pw(['eval', 'document.title']);
    await pw(['tab-select', '1']);
    const methods = new Set(fake.calls.map((call) => call.method));
    for (const method of [
      'Target.createTarget',
      'Page.navigate',
      'DOM.describeNode',
      'Input.dispatchMouseEvent',
      'Runtime.evaluate',
    ]) {
      assert.ok(methods.has(method), `never sent ${method}`);
    }
    for (const method of foregrounding) {
      assert.deepEqual(sent(fake.calls, 0, method), [], method);
    }
    createsInBackground(fake.calls, 0);
  });
});
