import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
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

function evaluation(value, exception, contexts) {
  return (message, send) => {
    const { method, params = {}, sessionId } = message;
    if (method === 'Runtime.enable' || method === 'Page.createIsolatedWorld') {
      const isolated = method === 'Page.createIsolatedWorld';
      if (isolated) contexts.isolated = true;
      const kinds = isolated ? ['isolated'] : contexts.isolated ? ['main', 'isolated'] : ['main'];
      for (const kind of kinds)
        send({
          method: 'Runtime.executionContextCreated',
          sessionId,
          params: {
            context: {
              id: kind === 'isolated' ? 2 : 1,
              uniqueId: `${kind}-TAB-L`,
              origin: 'https://example.test',
              name: '',
              auxData: { frameId: params.frameId || 'TAB', isDefault: kind === 'main' },
            },
          },
        });
      send({ id: message.id, result: isolated ? { executionContextId: 2 } : {} });
      return true;
    }
    if (method === 'Page.getPermissionsPolicyState') {
      send({ id: message.id, result: { states: [{ feature: 'tools', allowed: true }] } });
      return true;
    }
    if (method !== 'Runtime.evaluate') return false;
    const expression = params.expression;
    if (
      expression === "'modelContext' in Document.prototype" ||
      expression === 'document.modelContext != null' ||
      expression.includes('api.__slicc_webmcp_shim')
    ) {
      const present =
        expression !== "'modelContext' in Document.prototype" && value?.__webmcp_error !== 'absent';
      send({ id: message.id, result: { result: { type: 'boolean', value: present } } });
      return true;
    }
    if (!expression.includes('const tools = []') && !expression.includes('__webmcp_error')) {
      return false;
    }
    send({
      id: message.id,
      sessionId: message.sessionId,
      result: exception
        ? { exceptionDetails: { text: exception } }
        : {
            result: {
              type: 'object',
              value: Array.isArray(value)
                ? { kind: 'page', toolsJson: JSON.stringify(value) }
                : value,
            },
          },
    });
    return true;
  };
}

for (const kind of ['native', 'kernel']) {
  test(`fake WebMCP ${kind}: absent differs from an empty tool list`, async (t) => {
    let answer = { __webmcp_error: 'absent' };
    let exception;
    const contexts = { isolated: false };
    const fake = await fakeCdp({
      intercept: (message, send) => evaluation(answer, exception, contexts)(message, send),
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
}
