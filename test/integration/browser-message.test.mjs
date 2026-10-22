import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');

test('browser message URL redaction', async (t) => {
  const site = await startServer();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser();
    const native = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('native curlwright redacts userinfo', () => curlRedaction(native, site));
    await t.test('native playwright eval redacts userinfo', () => evalRedaction(native, site));
    await t.test('kernel curlwright redacts userinfo', () => curlRedaction(kernel, site));
    await t.test('kernel playwright eval redacts userinfo', () => evalRedaction(kernel, site));
  } finally {
    await settle(kernel?.close());
    await settle(browser?.close());
    await settle(site.close());
  }
});

async function settle(value) {
  try {
    await value;
  } catch {
    return undefined;
  }
}

function targetOf(stdout) {
  const match = stdout.match(/\[targetId: ([^\]]+)\]/);
  if (!match) throw new Error(stdout);
  return match[1];
}

async function curlRedaction(runner, site) {
  const cwd = await runner.fresh();
  const opened = await runner.playwright(['open', `${site.origin}/page`], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const tab = targetOf(opened.stdout);

  const url = `http://u:PW_LEAKED_SECRET@127.0.0.1:${site.port}/x`;

  const result = await runner.curl(['--tab', tab, url], cwd);
  assert.notEqual(result.code, 0);
  const combined = result.stdout + result.stderr;
  assert.ok(
    !combined.includes('PW_LEAKED_SECRET'),
    `password leaked in default mode:\n${combined}`
  );
  assert.ok(combined.includes('127.0.0.1'), `host should still appear:\n${combined}`);

  const full = await runner.curl(['--tab', tab, '--full-urls', url], cwd);
  assert.notEqual(full.code, 0);
  const fullCombined = full.stdout + full.stderr;
  assert.ok(
    fullCombined.includes('PW_LEAKED_SECRET'),
    `--full-urls should show password:\n${fullCombined}`
  );

  await runner.playwright(['tab-close', '--tab', tab], cwd);
}

async function evalRedaction(runner, site) {
  const cwd = await runner.fresh();
  const opened = await runner.playwright(['open', `${site.origin}/page`], cwd);
  assert.equal(opened.code, 0, opened.stderr);
  const tab = targetOf(opened.stdout);

  const url = `http://u:PW_EVAL_SECRET@127.0.0.1:${site.port}/x`;
  const expr = `fetch(${JSON.stringify(url)})`;

  const result = await runner.playwright(['eval', expr], cwd);
  assert.notEqual(result.code, 0);
  const combined = result.stdout + result.stderr;
  assert.ok(!combined.includes('PW_EVAL_SECRET'), `password leaked in eval:\n${combined}`);
  assert.ok(combined.includes('127.0.0.1'), `host should still appear:\n${combined}`);

  const full = await runner.playwright(['--full-urls', 'eval', expr], cwd);
  assert.notEqual(full.code, 0);
  const fullCombined = full.stdout + full.stderr;
  assert.ok(
    fullCombined.includes('PW_EVAL_SECRET'),
    `--full-urls should show password:\n${fullCombined}`
  );

  await runner.playwright(['tab-close', '--tab', tab], cwd);
}
