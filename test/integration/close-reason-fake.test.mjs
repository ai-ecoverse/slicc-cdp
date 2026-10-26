import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';
import { killTree } from './watchdog.mjs';

const reason = 'the CDP host is gone';
const closed = `websocket closed 1011 ${reason}\n`;
const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);

function spawned(file, args, home) {
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
    const child = spawn(file, args, {
      cwd: home,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
      detached: process.platform !== 'win32',
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
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

const runners = {
  native: async (url) => {
    const homes = [];
    return {
      async fresh() {
        const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-close-'));
        homes.push(home);
        return home;
      },
      playwright: (args, home) => spawned(exe('playwright-cli'), ['--cdp', url, ...args], home),
      curl: (args, home) => spawned(exe('curlwright'), ['--cdp', url, ...args], home),
      async close() {
        for (const home of homes) await rm(home, { recursive: true, force: true });
      },
    };
  },
  wasi: (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};

const commands = {
  'playwright-cli eval': {
    trees: [1, 2, 3, 4, 5],
    async prepare(pw, cwd) {
      const opened = await pw.playwright(['open', 'about:blank'], cwd);
      assert.equal(opened.code, 0, opened.stderr);
    },
    run: (pw, cwd) => pw.playwright(['eval', 'new Promise(() => {})'], cwd),
    stderr: [closed],
  },
  curlwright: {
    trees: [1, 2, 3],
    tabs: [{ targetId: 'TAB', url: 'https://example.test/' }],
    async prepare() {},
    run: (pw, cwd) => pw.curl(['https://example.test/api'], cwd),
    stderr: [closed, `curlwright: ${closed}`],
  },
};

for (const [transport, runner] of Object.entries(runners)) {
  for (const [name, command] of Object.entries(commands)) {
    test(`${transport}: ${name} reports the server's close code and reason when the close lands on any Page.getFrameTree`, async (t) => {
      const armed = { at: 0, seen: 0, closedAt: -1 };
      const fake = await fakeCdp({
        intercept(message, _send, close) {
          if (!armed.at || message.method !== 'Page.getFrameTree') return false;
          armed.seen += 1;
          if (armed.seen !== armed.at) return false;
          armed.closedAt = fake.calls.length;
          close(1011, reason);
          return true;
        },
      });
      fake.targets.push(...(command.tabs ?? []));
      const pw = await runner(fake.url);
      try {
        for (const at of command.trees) {
          await t.test(`close on Page.getFrameTree #${at}`, async () => {
            const cwd = await pw.fresh();
            Object.assign(armed, { at: 0, seen: 0, closedAt: -1 });
            await command.prepare(pw, cwd);
            armed.at = at;
            const result = await command.run(pw, cwd);
            armed.at = 0;
            assert.equal(armed.seen, at, 'the command reached the armed Page.getFrameTree');
            assert.equal(result.code, 1, result.stdout);
            assert.equal(result.stdout, '');
            assert.ok(command.stderr.includes(result.stderr), result.stderr);
            assert.deepEqual(
              fake.calls.slice(armed.closedAt).map((call) => call.method),
              [],
              'nothing is sent after the close'
            );
          });
        }
      } finally {
        await pw.close();
        await fake.close();
      }
    });
  }
}
