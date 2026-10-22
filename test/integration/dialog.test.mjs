import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const playwright = path.join(
  nativeDir,
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const curlwright = path.join(
  nativeDir,
  process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'
);

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
    const child = spawn(playwright, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 45000);
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

function dialogs(open, { url = 'https://example.test/', delay = 200 } = {}) {
  const state = { open };
  const intercept = (message, send) => {
    if (message.method !== 'Page.handleJavaScriptDialog') return false;
    const reply = (body) => send({ id: message.id, sessionId: message.sessionId, ...body });
    if (state.open) {
      state.open = false;
      reply({ result: {} });
      return true;
    }
    reply({ error: { code: -32000, message: 'No dialog is showing' } });
    if (state.later) return true;
    state.later = setTimeout(() => {
      state.open = true;
      send({
        method: 'Page.javascriptDialogOpening',
        sessionId: message.sessionId,
        params: {
          url,
          frameId: 'F',
          message: 'name?',
          type: 'prompt',
          hasBrowserHandler: true,
          defaultPrompt: '',
        },
      });
    }, delay);
    return true;
  };
  return { state, intercept };
}

async function withFake(options, body) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-dialog-'));
  const fake = await fakeCdp(options);
  try {
    fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
    await body(fake, (args) => run(['--cdp', fake.url, ...args], home));
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

const handled = (fake) =>
  fake.calls.filter((call) => call.method === 'Page.handleJavaScriptDialog');

test('dialog-accept answers a dialog that is already open, with the prompt text', async () => {
  const { intercept } = dialogs(true, { delay: 60000 });
  await withFake({ intercept }, async (fake, pw) => {
    const accepted = await pw(['dialog-accept', 'Lars', 'Trieloff']);
    assert.equal(accepted.code, 0, accepted.stderr);
    assert.equal(accepted.stdout, 'Accepted the dialog\n');
    assert.deepEqual(
      handled(fake).map((call) => [call.target, call.params]),
      [['TAB', { accept: true, promptText: 'Lars Trieloff' }]]
    );
  });
});

test('dialog-dismiss waits for a dialog that opens later', async () => {
  const { state, intercept } = dialogs(false);
  await withFake({ intercept }, async (fake, pw) => {
    const dismissed = await pw(['dialog-dismiss']);
    clearTimeout(state.later);
    assert.equal(dismissed.code, 0, dismissed.stderr);
    assert.equal(dismissed.stdout, 'Dismissed the dialog\n');
    assert.deepEqual(
      handled(fake).map((call) => call.params),
      [{ accept: false }, { accept: false }]
    );
    assert.ok(fake.calls.some((call) => call.method === 'Page.enable' && call.target === 'TAB'));
  });
});

test('dialog-accept without a dialog says so and exits 1', async () => {
  const { state, intercept } = dialogs(false, { delay: 60000 });
  await withFake({ intercept }, async (fake, pw) => {
    const started = Date.now();
    const accepted = await pw(['dialog-accept']);
    clearTimeout(state.later);
    assert.equal(accepted.code, 1);
    assert.equal(accepted.stdout, '');
    assert.equal(accepted.stderr, 'Error: No dialog visible\n');
    assert.ok(Date.now() - started >= 4500, 'gave up before the dialog wait');
    assert.equal(handled(fake).length, 1);
  });
});

test('a dialog that a protected origin opens later is never answered', async () => {
  const { state, intercept } = dialogs(false, { url: 'https://seven.sliccy.ai/' });
  await withFake({ intercept }, async (fake, pw) => {
    const accepted = await pw(['dialog-accept']);
    clearTimeout(state.later);
    assert.equal(accepted.code, 1);
    assert.equal(accepted.stderr, 'Error: No dialog visible\n');
    assert.equal(handled(fake).length, 1);
  });
});

test('dialog commands refuse a protected tab before they touch it', async () => {
  const { intercept } = dialogs(true);
  await withFake({ intercept }, async (fake, pw) => {
    fake.targets.push({ targetId: 'VAULT', url: 'http://vault.sliccy.ai/vault' });
    for (const args of [
      ['dialog-accept', '--tab', 'VAULT'],
      ['dialog-dismiss', '--tab', 'VAULT'],
    ]) {
      const refused = await pw(args);
      assert.equal(refused.code, 1, refused.stdout);
      assert.match(refused.stderr, /refused/);
    }
    assert.equal(handled(fake).length, 0);
  });
});

test('dialog-accept and dialog-dismiss answer real Chrome dialogs', async (t) => {
  const site = await startServer();
  let browser;
  try {
    browser = await launchBrowser();
    const runner = nativeRunner(playwright, curlwright, browser.wsUrl);
    const cwd = await runner.fresh();
    const opened = await runner.playwright(['open', `${site.origin}/cb`], cwd);
    assert.equal(opened.code, 0, opened.stderr);
    const value = async (expression) => {
      const result = await runner.playwright(['eval', expression], cwd);
      assert.equal(result.code, 0, result.stderr);
      return result.stdout.trim();
    };

    await t.test('a prompt that opens after dialog-accept started gets the text', async () => {
      await value("setTimeout(() => { window.answer = prompt('name?'); }, 1500); 'armed'");
      const accepted = await runner.playwright(['dialog-accept', 'Lars'], cwd);
      assert.equal(accepted.code, 0, accepted.stderr);
      assert.equal(accepted.stdout, 'Accepted the dialog\n');
      assert.equal(await value('window.answer'), 'Lars');
    });

    await t.test('a confirm that is already open is dismissed', async () => {
      await value("setTimeout(() => { window.sure = confirm('sure?'); }, 0); 'armed'");
      await new Promise((resolve) => setTimeout(resolve, 500));
      const dismissed = await runner.playwright(['dialog-dismiss'], cwd);
      assert.equal(dismissed.code, 0, dismissed.stderr);
      assert.equal(dismissed.stdout, 'Dismissed the dialog\n');
      assert.equal(await value('String(window.sure)'), 'false');
    });

    await t.test('an alert that is already open is accepted', async () => {
      await value("setTimeout(() => { alert('hi'); window.alerted = true; }, 0); 'armed'");
      await new Promise((resolve) => setTimeout(resolve, 500));
      const accepted = await runner.playwright(['dialog-accept'], cwd);
      assert.equal(accepted.code, 0, accepted.stderr);
      assert.equal(await value('String(window.alerted)'), 'true');
    });

    await t.test('no dialog exits 1', async () => {
      const accepted = await runner.playwright(['dialog-accept'], cwd);
      assert.equal(accepted.code, 1);
      assert.equal(accepted.stderr, 'Error: No dialog visible\n');
    });
  } finally {
    await browser?.close().catch(() => {});
    await site.close().catch(() => {});
  }
});
