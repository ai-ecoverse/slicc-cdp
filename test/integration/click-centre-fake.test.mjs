import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import './watchdog.mjs';

const binary =
  process.env.SLICC_INPUT_BIN ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );
const selector = ':root > :nth-child(2) > :nth-child(1)';

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
    const child = spawn(binary, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`click timed out: ${args.join(' ')}`));
    }, 20000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (status) => {
      clearTimeout(timer);
      resolve({ code: status, stdout, stderr });
    });
  });
}

function button() {
  return {
    scrollIntoView() {},
    click() {},
    getBoundingClientRect: () => ({ x: 8, y: 8, width: 39, height: 21 }),
  };
}

function page() {
  const el = button();
  const document = { querySelector: (query) => (query === selector ? el : null) };
  return (message, send) => {
    const { id, method, params = {}, sessionId } = message;
    const reply = (value) =>
      send({
        id,
        ...(sessionId ? { sessionId } : {}),
        result: { result: { type: 'object', value } },
      });
    if (
      method === 'Runtime.evaluate' &&
      params.expression?.startsWith('(function() { var el = document.querySelector(')
    ) {
      reply(JSON.parse(JSON.stringify(vm.runInNewContext(params.expression, { document }))));
      return true;
    }
    if (
      method === 'Runtime.callFunctionOn' &&
      params.functionDeclaration?.startsWith('function() { this.scrollIntoView(')
    ) {
      const fn = vm.runInNewContext(`(${params.functionDeclaration})`);
      reply(JSON.parse(JSON.stringify(fn.call(el))));
      return true;
    }
    return false;
  };
}

async function clicked(nodes) {
  const home = await realpath(await mkdtemp(path.join(tmpdir(), 'slicc-click-')));
  const fake = await fakeCdp({
    intercept: page(),
    aria: () => ({
      json: [
        {
          role: 'generic',
          ref: 'e1',
          children: [{ role: 'button', name: 'Go', ref: 'e2', cursor: 'pointer' }],
        },
      ],
      refs: [
        { ref: 'e1', selector: ':root > :nth-child(2)' },
        { ref: 'e2', selector },
      ],
      base: 'http://example.test/',
      nodes,
    }),
  });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  try {
    const pw = (args) => run(['--cdp', fake.url, ...args], home);
    const snapshot = await pw(['snapshot']);
    assert.equal(snapshot.code, 0, snapshot.stderr);
    const result = await pw(['click', 'e2']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout, 'Clicked e2\n');
    return fake.calls
      .filter((call) => call.method === 'Input.dispatchMouseEvent')
      .map(({ params }) => [params.type, params.x, params.y]);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

test('the selector fallback clicks the centre of the element', async () => {
  assert.deepEqual(await clicked([{}, {}]), [
    ['mousePressed', 27.5, 18.5],
    ['mouseReleased', 27.5, 18.5],
  ]);
});

test('the backend node path clicks the centre of the element', async () => {
  assert.deepEqual(await clicked([{ backendNodeId: 1 }, { backendNodeId: 2 }]), [
    ['mousePressed', 27.5, 18.5],
    ['mouseReleased', 27.5, 18.5],
  ]);
});
