import assert from 'node:assert/strict';
import path from 'node:path';
import { launchBrowser } from './browser.mjs';
import { chromeTest as it } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const native = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.join(root, 'target/wasm32-wasip1/debug', `${name}.wasm`);
const unavailable = (command) =>
  `${command}: WebMCP is unavailable on this page (document.modelContext is absent)\n`;
const runnerFor = (kind, url) =>
  kind === 'native'
    ? nativeRunner(native('playwright-cli'), native('curlwright'), url)
    : kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url);

for (const kind of ['native', 'kernel']) {
  it(`Chrome WebMCP ${kind}: disabled and null modelContext are unavailable`, async (t) => {
    const site = await startServer();
    let browser;
    let runner;
    try {
      browser = await launchBrowser(['--disable-features=WebMCP']);
      runner = await runnerFor(kind, browser.wsUrl);
      const cwd = await runner.fresh();
      const opened = await runner.playwright(['open', `${site.origin}/cb`], cwd);
      assert.equal(opened.code, 0, opened.stderr);
      const checked = await runner.playwright(['eval', 'typeof document.modelContext'], cwd);
      assert.equal(checked.code, 0, checked.stderr);
      assert.equal(checked.stdout, 'undefined\n');
      for (const state of ['absent', 'null', 'available but empty']) {
        await t.test(state, async () => {
          if (state !== 'absent') {
            const set = await runner.playwright(
              [
                'eval',
                state === 'null'
                  ? "Object.defineProperty(document, 'modelContext', { configurable: true, value: null }); 'set'"
                  : "Object.defineProperty(document, 'modelContext', { configurable: true, value: { getTools: async () => [] } }); 'set'",
              ],
              cwd
            );
            assert.equal(set.code, 0, set.stderr);
          }
          for (const args of [
            ['webmcp-list'],
            ['webmcp-list', '--json'],
            ['webmcp-call', 'anything', '{}'],
          ]) {
            const result = await runner.playwright(args, cwd);
            if (state === 'available but empty') {
              assert.equal(result.code, args[0] === 'webmcp-call' ? 2 : 0, result.stderr);
              if (args[0] === 'webmcp-list') {
                assert.equal(
                  result.stdout,
                  args.includes('--json') ? '[]\n' : 'No WebMCP tools on this page\n'
                );
              } else {
                assert.match(result.stderr, /unknown tool/);
              }
            } else {
              assert.equal(result.code, 2, result.stderr);
              assert.equal(result.stdout, '');
              assert.equal(result.stderr, unavailable(args[0]));
            }
          }
        });
      }
    } finally {
      await Promise.resolve(runner?.close()).catch(() => {});
      await browser?.close().catch(() => {});
      await site.close().catch(() => {});
    }
  });
}
