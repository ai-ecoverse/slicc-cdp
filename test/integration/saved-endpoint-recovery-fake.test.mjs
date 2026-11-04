import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, realpath, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { killTree } from './watchdog.mjs';
import { webmcpBackend } from './webmcp-backend.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const playwright = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function configDir(home) {
  if (process.platform === 'darwin') return path.join(home, 'Library', 'Application Support');
  if (process.platform === 'win32') return path.join(home, 'AppData', 'Roaming');
  return path.join(home, '.config');
}

async function sessionFile(home, cwd) {
  const key = createHash('sha256')
    .update(await realpath(cwd))
    .digest('hex');
  return path.join(configDir(home), 'slicc', 'playwright-cli', 'sessions', `${key}.json`);
}

function hostEnv(home) {
  const env = {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    APPDATA: path.join(home, 'AppData', 'Roaming'),
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_STATE_HOME: path.join(home, '.state'),
    SLICC_CDP_PORTS: HARNESS_PORTS,
  };
  delete env.SLICC_CDP_URL;
  delete env.PLAYWRIGHT_CLI_SESSION;
  delete env.CHROME_PATH;
  return env;
}

function run(args, cwd, env) {
  return new Promise((resolve, reject) => {
    const child = spawn(playwright, args, {
      cwd,
      env,
      detached: process.platform !== 'win32',
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 60000);
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

function bindable(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => resolve(false));
    server.listen(port, '127.0.0.1', () => server.close(() => resolve(true)));
  });
}

async function freeHarnessPorts(count) {
  const ports = [];
  for (let port = 9300; port <= 9309 && ports.length < count; port += 1) {
    if (await bindable(port)) ports.push(port);
  }
  if (ports.length < count) throw new Error(`need ${count} free ports in ${HARNESS_PORTS}`);
  return ports;
}

function holder(args, cwd, endpoint, env) {
  const child = spawn(playwright, ['--cdp', endpoint, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (bytes) => {
    stdout += bytes;
  });
  child.stderr.on('data', (bytes) => {
    stderr += bytes;
  });
  const ended = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code) => resolve({ code, stdout, stderr }));
  });
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(stderr || 'holder did not become ready'));
    }, 15000);
    child.stdout.on('data', () => {
      if (!stdout.includes('\n')) return;
      clearTimeout(timer);
      try {
        resolve(JSON.parse(stdout.split('\n')[0]));
      } catch (error) {
        reject(error);
      }
    });
    ended.then((result) => {
      clearTimeout(timer);
      if (!stdout.includes('\n')) reject(new Error(result.stderr));
    }, reject);
  });
  ready.catch(() => {});
  return { child, ended, ready };
}

test('open recovers when a remembered http discovery endpoint is dead', async () => {
  const [deadPort, livePort] = await freeHarnessPorts(2);
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-dead-endpoint-'));
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-dead-cwd-'));
  const env = hostEnv(home);
  const dead = await fakeCdp({ version: true, port: deadPort });
  try {
    const listed = await run(['--cdp', `http://127.0.0.1:${deadPort}`, 'tab-list'], cwd, env);
    assert.equal(listed.code, 0, listed.stderr);
    const saved = JSON.parse(await readFile(await sessionFile(home, cwd), 'utf8'));
    assert.equal(saved.endpoint, `http://127.0.0.1:${deadPort}/json/version`);
    assert.equal(saved.provenance, 'explicit');
    await dead.close();
    const live = await fakeCdp({ version: true, port: livePort });
    try {
      const opened = await run(['open', 'about:blank'], cwd, env);
      assert.equal(opened.code, 0, opened.stderr);
      assert.match(opened.stdout, /Opened about:blank/);
      assert.doesNotMatch(opened.stderr, /This CLI does not launch Chrome/);
    } finally {
      await live.close();
    }
  } finally {
    await dead.close().catch(() => {});
    await rm(home, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
});

test('second-process webmcp-list finds holder tools across http and ws spellings', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-webmcp-spellings-'));
  const home = cwd;
  const env = hostEnv(home);
  const backend = await webmcpBackend({
    definitions: [{ name: 'add', description: 'Add', execute: () => 3 }],
  });
  const http = `http://127.0.0.1:${backend.port}`;
  const ws = backend.url;
  const held = holder(['webmcp-polyfill', '--tab', 'TAB', '--reload'], cwd, http, env);
  try {
    assert.equal((await held.ready).mode, 'polyfill');
    const viaWs = await run(['--cdp', ws, 'webmcp-list', '--tab', 'TAB', '--json'], cwd, env);
    assert.equal(viaWs.code, 0, viaWs.stderr);
    assert.deepEqual(
      JSON.parse(viaWs.stdout).map((tool) => tool.name),
      ['add']
    );
    const viaHttp = await run(['--cdp', http, 'webmcp-list', '--tab', 'TAB', '--json'], cwd, env);
    assert.equal(viaHttp.code, 0, viaHttp.stderr);
    assert.deepEqual(
      JSON.parse(viaHttp.stdout).map((tool) => tool.name),
      ['add']
    );
  } finally {
    held.child.kill('SIGKILL');
    await held.ended.catch(() => {});
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
});
