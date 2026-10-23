import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, kernelSessionFile } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');

test('kernel playwright-cli keeps a non-root session out of a read-only cwd', async (t) => {
  const site = await startServer();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser();
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    const user = await kernel.addUser('cone');
    const cwd = '/opt/readonly';
    await kernel.writeFile(`${cwd}/.keep`, '');
    const as = { user: 'cone' };
    let otherId;

    await t.test('tab-list works in a cwd the user cannot write', async () => {
      const listed = await kernel.playwright(['tab-list'], cwd, as);
      assert.equal(listed.code, 0, listed.stderr);
      assert.equal(listed.stderr, '');
    });

    await t.test('open then tab-select carry the current tab across invocations', async () => {
      const first = await kernel.playwright(['open', `${site.origin}/page`], cwd, as);
      assert.equal(first.code, 0, first.stderr);
      const firstId = targetOf(first.stdout);
      const second = await kernel.playwright(['tab-new', `${site.origin}/other`], cwd, as);
      assert.equal(second.code, 0, second.stderr);
      otherId = targetOf(second.stdout);
      const listed = await kernel.playwright(['tab-list'], cwd, as);
      assert.equal(listed.code, 0, listed.stderr);
      const line = listed.stdout.split('\n').find((item) => item.includes(`[${firstId}]`));
      const index = line?.match(/^(\d+):/)?.[1];
      assert.ok(index, listed.stdout);
      const selected = await kernel.playwright(['tab-select', index], cwd, as);
      assert.equal(selected.code, 0, selected.stderr);
      const title = await kernel.playwright(['eval', 'document.title'], cwd, as);
      assert.equal(title.code, 0, title.stderr);
      assert.match(title.stdout, /Fixture/);
      const stored = JSON.parse(
        Buffer.from(await kernel.readFile(kernelSessionFile(user.home, cwd))).toString('utf8')
      );
      assert.equal(stored.current, firstId);
      const file = await kernel.stat(kernelSessionFile(user.home, cwd));
      assert.equal(file.mode & 0o777, 0o600);
      const dir = await kernel.stat(path.posix.dirname(kernelSessionFile(user.home, cwd)));
      assert.equal(dir.mode & 0o777, 0o700);
      await assert.rejects(kernel.readFile(`${cwd}/.playwright-cli/session.json`));
    });

    await t.test('a cwd session file is never read', async () => {
      await kernel.writeFile(
        `${cwd}/.playwright-cli/session.json`,
        JSON.stringify({ current: otherId })
      );
      const title = await kernel.playwright(['eval', 'document.title'], cwd, as);
      assert.equal(title.code, 0, title.stderr);
      assert.match(title.stdout, /Fixture/);
    });

    await t.test('an unset or unwritable HOME runs without saved state', async () => {
      for (const home of ['', cwd]) {
        const env = { HOME: home, XDG_STATE_HOME: '', XDG_CONFIG_HOME: '' };
        const listed = await kernel.playwright(['tab-list'], cwd, { ...as, env });
        assert.equal(listed.code, 0, listed.stderr);
        assert.match(listed.stdout, /\/page/);
        assert.match(listed.stderr, /^playwright-cli: running without saved state \(.+\)\n$/);
      }
    });
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
