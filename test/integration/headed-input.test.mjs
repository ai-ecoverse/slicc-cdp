import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);

function sevenServer() {
  const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-headed-'));
  const key = path.join(scratch, 'key.pem');
  const cert = path.join(scratch, 'cert.pem');
  execFileSync(
    'openssl',
    ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', key, '-out', cert].concat([
      '-days',
      '1',
      '-subj',
      '/CN=seven.sliccy.ai',
    ]),
    { stdio: 'ignore' }
  );
  const server = createServer({ key: readFileSync(key), cert: readFileSync(cert) }, (_, res) => {
    res.setHeader('content-type', 'text/html');
    res.end('<!doctype html><title>Seven</title><p>seven</p>');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () =>
      resolve({
        port: server.address().port,
        close: async () => {
          server.closeAllConnections();
          await new Promise((done) => server.close(done));
          rmSync(scratch, { recursive: true, force: true });
        },
      })
    );
  });
}

function refOf(stdout, role, name) {
  const ref = stdout.match(new RegExp(`${role} "${name}"(?: \\[[^\\]]+\\])*? \\[ref=(e\\d+)\\]`));
  assert.ok(ref, `${role} ${name}\n${stdout}`);
  return ref[1];
}

async function inTab(raw, targetId, expression) {
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  try {
    const { result } = await raw.send(
      'Runtime.evaluate',
      { expression, returnByValue: true },
      sessionId
    );
    return result.value;
  } finally {
    await raw.send('Target.detachFromTarget', { sessionId }).catch(() => {});
  }
}

async function until(read, expected, label) {
  const deadline = Date.now() + 10000;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (last === expected) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail(`${label}: ${JSON.stringify(last)}`);
}

async function flow(runner, raw, site, seven) {
  const cwd = await runner.fresh();
  const pw = async (args) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
    return result.stdout;
  };
  const out = async () => (await pw(['eval', "document.getElementById('out').textContent"])).trim();
  const tab = (await pw(['open', `${site.origin}/page`])).match(/\[targetId: ([^\]]+)\]/)?.[1];
  assert.ok(tab);
  const visibility = (target) => inTab(raw, target, 'document.visibilityState');
  assert.equal(await visibility(tab), 'hidden');
  assert.equal(await visibility(seven), 'visible');

  await pw(['click', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'clicked', 'click');
  await pw(['hover', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'hovered', 'hover');
  await pw(['dblclick', refOf(await pw(['snapshot']), 'button', 'Go')]);
  await until(out, 'dblclicked', 'dblclick');
  await pw(['fill', refOf(await pw(['snapshot']), 'textbox', 'Name'), 'ada']);
  await pw(['type', 'x']);
  await pw(['press', 'Enter']);
  await until(out, 'enter:adax', 'type and press');
  assert.equal(await visibility(seven), 'visible');

  await pw(['press', 'Tab']);
  await until(() => inTab(raw, tab, 'document.activeElement.id'), 'color', 'Tab moved focus');
  await until(() => visibility(seven), 'visible', 'seven is back in front');
  assert.equal(await visibility(tab), 'hidden');

  await inTab(
    raw,
    tab,
    "document.getElementById('go').onclick = () => { window.answer = confirm('sure?'); }"
  );
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId: tab, flatten: true });
  try {
    await raw.send('Page.enable', {}, sessionId);
    const opening = raw.event('Page.javascriptDialogOpening', sessionId);
    await pw(['click', refOf(await pw(['snapshot']), 'button', 'Go')]);
    const dialog = await Promise.race([
      opening.seen,
      new Promise((resolve) => setTimeout(() => resolve(null), 10000)),
    ]);
    opening.cancel();
    assert.equal(dialog?.message, 'sure?');
    await raw.send('Page.handleJavaScriptDialog', { accept: true }, sessionId);
  } finally {
    await raw.send('Target.detachFromTarget', { sessionId }).catch(() => {});
  }
  await until(() => inTab(raw, tab, 'window.answer'), true, 'the dialog answered the click');
  assert.equal(await visibility(seven), 'visible');
  await pw(['close']);
}

test('headed Chrome takes playwright-cli input on a background tab and keeps seven in front', {
  timeout: 300000,
}, async (t) => {
  const site = await startServer();
  const tls = await sevenServer();
  let browser;
  let raw;
  let kernel;
  try {
    browser = await launchBrowser(
      [
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tls.port}`,
        '--ignore-certificate-errors',
        '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
      ],
      { headless: false }
    );
    assert.equal(browser.headless, false);
    raw = await rawSocket(browser.wsUrl);
    const { targetId: seven } = await raw.send('Target.createTarget', {
      url: 'https://seven.sliccy.ai/',
      background: false,
    });
    await until(() => inTab(raw, seven, 'document.title'), 'Seven', 'seven loaded');
    await until(() => inTab(raw, seven, 'document.visibilityState'), 'visible', 'seven in front');
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => flow(native, raw, site, seven));
    kernel = await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
    await t.test('kernel', () => flow(kernel, raw, site, seven));
  } finally {
    kernel?.close();
    raw?.close();
    await browser?.close();
    await tls.close();
    await site.close();
  }
});
