import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser } from './browser.mjs';
import { chromeTest as it } from './chrome-policy.mjs';
import { fakeCdp } from './fake-cdp.mjs';
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

function evaluation(value, exception) {
  return (message, send) => {
    if (message.method !== 'Runtime.evaluate') return false;
    const expression = message.params.expression;
    if (!expression.includes('const tools = []') && !expression.includes('__webmcp_error')) {
      return false;
    }
    send({
      id: message.id,
      sessionId: message.sessionId,
      result: exception
        ? { exceptionDetails: { text: exception } }
        : { result: { type: 'object', value } },
    });
    return true;
  };
}

for (const kind of ['native', 'kernel']) {
  test(`fake WebMCP ${kind}: absent differs from an empty tool list`, async (t) => {
    let answer = { __webmcp_error: 'absent' };
    let exception;
    const fake = await fakeCdp({
      intercept: (message, send) => evaluation(answer, exception)(message, send),
    });
    let runner;
    try {
      fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
      runner = await runnerFor(kind, fake.url);
      const cwd = await runner.fresh();
      await t.test('absent list and call have the same status and explanation', async () => {
        for (const args of [
          ['webmcp-list'],
          ['webmcp-list', '--json'],
          ['webmcp-call', 'anything', '{}'],
        ]) {
          const result = await runner.playwright(args, cwd);
          assert.equal(result.code, 2, result.stderr);
          assert.equal(result.stdout, '');
          assert.equal(result.stderr, unavailable(args[0]));
        }
      });
      await t.test('empty list remains success in human and JSON output', async () => {
        answer = [];
        const human = await runner.playwright(['webmcp-list'], cwd);
        assert.equal(human.code, 0, human.stderr);
        assert.equal(human.stdout, 'No WebMCP tools on this page\n');
        const json = await runner.playwright(['webmcp-list', '--json'], cwd);
        assert.equal(json.code, 0, json.stderr);
        assert.equal(json.stdout, '[]\n');
      });
      await t.test('invalid or failed evaluation is not an empty tool list', async () => {
        for (const value of [null, {}, 'odd']) {
          answer = value;
          const result = await runner.playwright(['webmcp-list'], cwd);
          assert.equal(result.code, 1);
          assert.equal(result.stdout, '');
          assert.match(result.stderr, /unexpected evaluation result/);
        }
        exception = 'TypeError: getTools failed';
        const result = await runner.playwright(['webmcp-list'], cwd);
        assert.equal(result.code, 1);
        assert.match(result.stderr, /getTools failed/);
      });
      await t.test(
        'protected and unverified targets are refused before WebMCP evaluation',
        async () => {
          for (const url of ['https://seven.sliccy.ai/?credential=hidden', 'not-a-url']) {
            fake.targets.push({
              targetId: 'REFUSED',
              url,
              frame: {
                id: 'REFUSED',
                loaderId: 'L',
                url,
                securityOrigin: url.startsWith('https:') ? new URL(url).origin : '',
              },
            });
            const start = fake.calls.length;
            for (const args of [
              ['webmcp-list', '--tab', 'REFUSED'],
              ['webmcp-call', 'anything', '{}', '--tab', 'REFUSED'],
            ]) {
              const result = await runner.playwright(args, cwd);
              assert.equal(result.code, 1);
              assert.equal(result.stdout, '');
              assert.ok(!result.stderr.includes('hidden'));
            }
            assert.equal(
              fake.calls
                .slice(start)
                .filter(
                  (call) =>
                    call.method === 'Runtime.evaluate' &&
                    call.params.expression.includes('modelContext')
                ).length,
              0
            );
            fake.targets.pop();
          }
        }
      );
    } finally {
      await Promise.resolve(runner?.close()).catch(() => {});
      await fake.close();
    }
  });

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
