import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const playwright = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function run(file, args, home) {
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
    const child = spawn(file, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
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

function forms(port) {
  const hosts = ['localhost', '127.0.0.1', '[::1]', '0.0.0.0'];
  return [
    String(port),
    ...hosts.flatMap((host) => [
      `${host}:${port}`,
      `http://${host}:${port}`,
      `http://${host}:${port}/`,
      `ws://${host}:${port}`,
      `ws://${host}:${port}/devtools/browser/fake`,
    ]),
  ];
}

async function listening(listen) {
  try {
    return await fakeCdp({ listen, version: true });
  } catch (error) {
    if (listen === '::1' && ['EADDRNOTAVAIL', 'EAFNOSUPPORT'].includes(error.code)) return null;
    throw error;
  }
}

for (const listen of ['127.0.0.1', '::1']) {
  test(`every spelling of localhost reaches a debugger listening only on ${listen}`, async (t) => {
    const fake = await listening(listen);
    if (!fake) {
      t.skip(`${listen} is not available on this host`);
      return;
    }
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-localhost-'));
    try {
      for (const cdp of forms(fake.port)) {
        const before = fake.calls.length;
        const listed = await run(playwright, ['--cdp', cdp, 'tab-list'], home);
        assert.equal(listed.code, 0, `--cdp ${cdp}: ${listed.stderr}`);
        assert.ok(fake.calls.length > before, `--cdp ${cdp} sent no CDP call`);
      }
    } finally {
      await fake.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}

test('a bare port that nothing listens on fails on every loopback address', async () => {
  const fake = await fakeCdp({ version: true });
  const port = fake.port;
  await fake.close();
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-localhost-'));
  try {
    const listed = await run(playwright, ['--cdp', String(port), 'tab-list'], home);
    assert.notEqual(listed.code, 0);
    assert.match(listed.stderr, new RegExp(`CDP connection to localhost:${port} failed`));
    const junk = await run(playwright, ['--cdp', 'localhost:nope', 'tab-list'], home);
    assert.notEqual(junk.code, 0);
    assert.match(junk.stderr, /unsupported CDP URL/);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
