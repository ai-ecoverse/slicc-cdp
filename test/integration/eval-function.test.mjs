import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as it } from './chrome-policy.mjs';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);

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
    const child = spawn(exe('playwright-cli'), args, {
      cwd: home,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
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

function remote(evaluated, called) {
  return (message, send) => {
    const answer = (body) => send({ id: message.id, sessionId: message.sessionId, ...body });
    if (message.method === 'Runtime.evaluate' && message.params.objectGroup === 'slicc-eval') {
      answer({ result: { result: evaluated(message.params) } });
      return true;
    }
    if (
      message.method === 'Runtime.callFunctionOn' &&
      message.params.objectGroup === 'slicc-eval'
    ) {
      answer({ result: { result: called(message.params) } });
      return true;
    }
    return false;
  };
}

async function withFake(intercept, body) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-eval-'));
  const fake = await fakeCdp({ intercept });
  try {
    fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
    await body(fake, (args) => run(['--cdp', fake.url, ...args], home));
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

const sent = (fake, method) => fake.calls.filter((call) => call.method === method);

test('eval calls a function expression on its own object and prints what it returns', async () => {
  const intercept = remote(
    () => ({ type: 'function', className: 'Function', description: '() => 1', objectId: 'FN' }),
    () => ({ type: 'number', value: 1 })
  );
  await withFake(intercept, async (fake, pw) => {
    const result = await pw(['eval', '() => 1']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout, '1\n');
    const [evaluated] = sent(fake, 'Runtime.evaluate').filter(
      (call) => call.params.objectGroup === 'slicc-eval'
    );
    assert.equal(evaluated.params.expression, '() => 1');
    assert.equal(evaluated.params.returnByValue, false);
    assert.equal(evaluated.params.awaitPromise, true);
    const [called] = sent(fake, 'Runtime.callFunctionOn');
    assert.equal(called.params.objectId, 'FN');
    assert.equal(called.params.functionDeclaration, 'function() { return this(); }');
    assert.equal(called.params.returnByValue, true);
    assert.equal(called.params.awaitPromise, true);
    assert.equal(sent(fake, 'Runtime.releaseObjectGroup').length, 1);
  });
});

test('eval normalizes function expressions without turning scripts into expressions', async () => {
  for (const [source, normalized, framed] of [
    ['function () {}', '(function () {}\n)', false],
    ['function named() { return 4; }', '(function named() { return 4; }\n)', false],
    ['async function () {}', '(async function () {}\n)', true],
    [
      'function helper() { return 7; }; helper()',
      'function helper() { return 7; }; helper()',
      false,
    ],
  ]) {
    const answers = remote(
      () => ({ type: 'function', objectId: 'FN' }),
      () => ({ type: 'undefined' })
    );
    const intercept = (message, send) => {
      if (message.method === 'Runtime.compileScript') {
        send({
          id: message.id,
          sessionId: message.sessionId,
          result: source.includes('; helper()')
            ? { exceptionDetails: { text: 'SyntaxError' } }
            : {},
        });
        return true;
      }
      return answers(message, send);
    };
    await withFake(intercept, async (fake, pw) => {
      const result = await pw(['eval', ...(framed ? ['--frame', 'TAB'] : []), source]);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, 'undefined\n');
      const [compiled] = sent(fake, 'Runtime.compileScript');
      assert.equal(compiled.params.persistScript, false);
      assert.equal(compiled.params.executionContextId, framed ? 7 : undefined);
      const [evaluated] = sent(fake, 'Runtime.evaluate').filter(
        (call) => call.params.objectGroup === 'slicc-eval'
      );
      assert.equal(evaluated.params.expression, normalized);
    });
  }
});

test('eval of a plain expression prints its value as before', async () => {
  const values = [
    [{ type: 'number', value: 2, description: '2' }, '2\n', 0],
    [{ type: 'string', value: 'hi' }, 'hi\n', 0],
    [{ type: 'undefined' }, 'undefined\n', 0],
    [{ type: 'object', subtype: 'null', value: null }, 'null\n', 0],
    [{ type: 'object', className: 'Object', objectId: 'OBJ' }, '{\n  "a": 1\n}\n', 1],
  ];
  for (const [evaluated, printed, calls] of values) {
    const intercept = remote(
      () => evaluated,
      () => ({ type: 'object', value: { a: 1 } })
    );
    await withFake(intercept, async (fake, pw) => {
      const result = await pw(['eval', 'expression']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, printed);
      const called = sent(fake, 'Runtime.callFunctionOn');
      assert.equal(called.length, calls);
      for (const call of called) {
        assert.equal(call.params.functionDeclaration, 'function() { return this; }');
      }
    });
  }
});

test('a function that throws is reported like any evaluation error', async () => {
  const intercept = remote(
    () => ({ type: 'function', objectId: 'FN' }),
    () => ({ type: 'object', subtype: 'error', objectId: 'ERR' })
  );
  const throwing = (message, send) => {
    if (
      message.method === 'Runtime.callFunctionOn' &&
      message.params.objectGroup === 'slicc-eval'
    ) {
      send({
        id: message.id,
        sessionId: message.sessionId,
        result: {
          result: { type: 'object', subtype: 'error', objectId: 'ERR' },
          exceptionDetails: { text: 'Uncaught', exception: { description: 'Error: nope' } },
        },
      });
      return true;
    }
    return intercept(message, send);
  };
  await withFake(throwing, async (_fake, pw) => {
    const result = await pw(['eval', '() => { throw new Error("nope"); }']);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, 'Error: nope\n');
  });
});

test('eval --frame calls a function expression in the frame world', async () => {
  const intercept = remote(
    () => ({ type: 'function', objectId: 'FN' }),
    () => ({ type: 'string', value: 'framed' })
  );
  await withFake(intercept, async (fake, pw) => {
    const result = await pw(['eval', '--frame', 'TAB', '() => "framed"']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout, 'framed\n');
    const [evaluated] = sent(fake, 'Runtime.evaluate').filter(
      (call) => call.params.objectGroup === 'slicc-eval'
    );
    assert.equal(evaluated.params.contextId, 7);
    assert.equal(sent(fake, 'Runtime.callFunctionOn').length, 1);
  });
});

for (const kind of ['native', 'kernel']) {
  it(`${kind} eval calls arrow, async arrow and function expressions in Chrome`, async (t) => {
    const site = await startServer();
    let browser;
    let runner;
    try {
      browser = await launchBrowser();
      runner =
        kind === 'native'
          ? nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl)
          : await kernelRunner(
              path.join(wasmDir, 'playwright-cli.wasm'),
              path.join(wasmDir, 'curlwright.wasm'),
              browser.wsUrl
            );
      const cwd = await runner.fresh();
      const opened = await runner.playwright(['open', `${site.origin}/cb`], cwd);
      assert.equal(opened.code, 0, opened.stderr);
      const cases = [
        ['() => 1', '1'],
        ['async () => { await new Promise((r) => setTimeout(r, 10)); return 2; }', '2'],
        ['function () { return document.title; }', 'Callback'],
        ['function () {}', 'undefined'],
        ['function named() { return 4; }', '4'],
        ['async function () {}', 'undefined'],
        ['async function () { return [1, 2]; }', '[\n  1,\n  2\n]'],
        ['() => ({ a: 1 })', '{\n  "a": 1\n}'],
        ['() => undefined', 'undefined'],
        ['1 + 1', '2'],
        ['({ a: 1 })', '{\n  "a": 1\n}'],
        ['document.title = "page"; document.title', 'page'],
        ['function helper() { return 7; }; helper()', '7'],
        ['window.runs = (window.runs || 0) + 1; window.runs', '1'],
        ['await Promise.resolve(3)', '3'],
        ['typeof (() => 1)', 'function'],
      ];
      for (const [expression, printed] of cases) {
        await t.test(expression, async () => {
          const result = await runner.playwright(['eval', expression], cwd);
          assert.equal(result.code, 0, result.stderr);
          assert.equal(result.stdout, `${printed}\n`);
        });
      }
      await t.test('a throwing function exits 1', async () => {
        const result = await runner.playwright(['eval', '() => { throw new Error("nope"); }'], cwd);
        assert.equal(result.code, 1);
        assert.match(result.stderr, /nope/);
      });
    } finally {
      await Promise.resolve(runner?.close()).catch(() => {});
      await browser?.close().catch(() => {});
      await site.close().catch(() => {});
    }
  });
}
