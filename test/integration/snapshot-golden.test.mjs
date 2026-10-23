import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
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
const fixtures = path.join(root, 'test/integration/fixtures/snapshot');
const read = (name) => readFileSync(path.join(fixtures, name), 'utf8');
const titles = { example: 'Example Domain', forms: 'Sign up', links: 'Links', frames: 'Frames' };

async function golden(runner, site) {
  for (const [name, title] of Object.entries(titles)) {
    const cwd = await runner.fresh();
    const url = `${site.origin}/snapshot/${name}.html`;
    const opened = await runner.playwright(['open', url], cwd);
    assert.equal(opened.code, 0, opened.stderr);
    const shot = await runner.playwright(['snapshot'], cwd);
    assert.equal(shot.code, 0, `${shot.stderr}\n${shot.stdout}`);
    assert.equal(
      shot.stdout,
      `### Page\n- Page URL: ${url}\n- Page Title: ${title}\n### Snapshot\n\`\`\`yaml\n${read(`${name}.yml`)}\`\`\`\n`,
      name
    );
    if (name === 'example') {
      const upstream = `### Page\n- Page URL: ${url}\n- Page Title: ${title}\n### Snapshot\n\`\`\`yaml\n${read('example.upstream.yml')}\`\`\`\n`;
      assert.ok(Buffer.byteLength(shot.stdout) <= Buffer.byteLength(upstream) * 1.02);
    }
    if (name === 'links') {
      const ref = shot.stdout.match(/button "Press" \[ref=(e\d+)\]/)?.[1];
      const clicked = await runner.playwright(['click', ref], cwd);
      assert.equal(clicked.code, 0, clicked.stderr);
      const text = await runner.playwright(
        ['eval', "document.querySelector('button').textContent"],
        cwd
      );
      assert.equal(text.stdout, 'Done\n');
    }
  }
}

test('snapshot matches upstream @playwright/cli on the fixture pages in Chrome', async (t) => {
  const site = await startServer();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser([
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const native = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    await t.test('native', () => golden(native, site));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('kernel', () => golden(kernel, site));
  } finally {
    await kernel?.close();
    await browser?.close();
    await site.close();
  }
});
