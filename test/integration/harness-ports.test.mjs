import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const playwright = exe('playwright-cli');
const curlwright = exe('curlwright');

function run(file, args, env) {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { cwd: env.HOME, env });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${path.basename(file)} ${args.join(' ')}`));
    }, 60000);
    child.on('error', reject);
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

async function harnessEnv() {
  const home = await realpath(await mkdtemp(path.join(tmpdir(), 'slicc-cdp-harness-')));
  const env = { ...process.env };
  delete env.SLICC_CDP_URL;
  delete env.PLAYWRIGHT_CLI_SESSION;
  return {
    ...env,
    HOME: home,
    USERPROFILE: home,
    APPDATA: path.join(home, 'AppData', 'Roaming'),
    LOCALAPPDATA: path.join(home, 'AppData', 'Local'),
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_STATE_HOME: path.join(home, '.state'),
  };
}

function fakeBrowser() {
  const seen = [];
  const server = createServer((req, res) => {
    seen.push(req.url);
    if (req.url === '/json/version') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          Browser: 'Chrome/156.0.8078.4',
          'User-Agent': 'Mozilla/5.0 Chrome/156.0.8078.4',
          webSocketDebuggerUrl: 'ws://127.0.0.1:9300/devtools/browser/harness',
        })
      );
      return;
    }
    if (req.url === '/json/list') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify([
          { id: 'P1', type: 'page', url: 'http://personal.test/', title: 'Harness page' },
        ])
      );
      return;
    }
    res.writeHead(404);
    res.end();
  });
  server.on('upgrade', (req, socket) => {
    seen.push(`upgrade ${req.url}`);
    socket.destroy();
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(9300, '127.0.0.1', () =>
      resolve({
        seen,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      })
    );
  });
}

test('every test process starts with the harness range and the debug-port guard', async () => {
  assert.equal(process.env.SLICC_CDP_PORTS, '9300-9309');
  assert.throws(
    () => net.connect({ host: '127.0.0.1', port: 9222 }),
    /test connected to 127\.0\.0\.1:9222/
  );
  assert.throws(() => net.createConnection(9229, 'localhost'), /test connected to localhost:9229/);
  await assert.rejects(fetch('http://127.0.0.1:9225/json/version'), (error) => {
    assert.match(String(error.cause?.message), /test connected to 127\.0\.0\.1:9225/);
    return true;
  });
});

test('the CLIs find a browser on the harness range by default', async () => {
  assert.equal(process.env.SLICC_CDP_PORTS, '9300-9309');
  const browser = await fakeBrowser();
  const env = await harnessEnv();
  try {
    const listed = await run(playwright, ['list', '--json'], env);
    assert.equal(listed.code, 0, listed.stderr);
    const report = JSON.parse(listed.stdout);
    const entry = report.entries.find((item) => item.endpoint?.cdp === 'http://127.0.0.1:9300');
    assert.ok(entry, listed.stdout);
    assert.deepEqual(
      entry.endpoint.tabs.map((tab) => tab.id),
      ['P1']
    );
    const fetched = await run(curlwright, ['-s', 'http://personal.test/api'], env);
    assert.notEqual(fetched.code, 0);
    assert.ok(browser.seen.includes('upgrade /devtools/browser/harness'), browser.seen.join('\n'));
  } finally {
    await browser.close();
    await rm(env.HOME, { recursive: true, force: true });
  }
});

test('the CLIs refuse to dial the real debug ports under the harness', async () => {
  assert.equal(process.env.SLICC_CDP_PORTS, '9300-9309');
  const env = await harnessEnv();
  try {
    const listed = await run(playwright, ['--cdp', 'http://127.0.0.1:9222', 'tab-list'], env);
    assert.equal(listed.code, 101, listed.stderr);
    assert.match(listed.stderr, /test dialed 127\.0\.0\.1:9222/);
    const fetched = await run(
      curlwright,
      ['--cdp', 'ws://localhost:9223/devtools/browser/x', 'http://personal.test/'],
      env
    );
    assert.equal(fetched.code, 101, fetched.stderr);
    assert.match(fetched.stderr, /test dialed localhost:9223/);
  } finally {
    await rm(env.HOME, { recursive: true, force: true });
  }
});
