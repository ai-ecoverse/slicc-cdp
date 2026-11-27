import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readdir, rm } from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { exercise, fixture, origin, refused, request } from './signed-lease-fixture.mjs';

const cli = path.resolve(
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

async function sandbox(t) {
  const home = await mkdtemp(path.join(tmpdir(), 'lease-job-'));
  let hits = 0;
  const trap = http.createServer((_req, res) => {
    hits++;
    res.writeHead(403);
    res.end();
  });
  await new Promise((resolve) => trap.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    await new Promise((resolve) => trap.close(resolve));
    await rm(home, { recursive: true, force: true });
    assert.equal(hits, 0, 'ordinary CDP endpoint must never be contacted');
  });
  const endpoint = `http://127.0.0.1:${trap.address().port}`;
  const run = async (argv, respond, env = {}, fd = respond ? 'pipe' : 'ignore') => {
    const child = spawn(cli, argv, {
      cwd: home,
      timeout: 10000,
      stdio: ['ignore', 'pipe', 'pipe', fd],
      env: {
        PATH: home,
        HOME: home,
        USERPROFILE: home,
        APPDATA: home,
        SystemRoot: process.env.SystemRoot,
        SLICC_CDP_PORTS: '9300-9309',
        SLICC_CDP_URL: endpoint,
        ...(respond ? { SLICC_LEASE_FD: '3' } : {}),
        ...env,
      },
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (data) => {
      stdout += data;
    });
    child.stderr.on('data', (data) => {
      stderr += data;
    });
    let payload = '';
    child.stdio[3]?.setEncoding('utf8').on('data', async (data) => {
      payload += data;
      if (payload.endsWith('\n')) {
        const reply = await respond(JSON.parse(payload));
        child.stdio[3].end(reply === null ? '' : `${JSON.stringify(reply)}\n`);
        payload = '';
      }
    });
    const status = await new Promise((resolve, reject) => {
      child.on('close', resolve);
      child.on('error', reject);
    });
    return { status, stdout, stderr };
  };
  return { home, endpoint, run };
}

test('native lease mode cannot discover a browser or treat a URL, port, or handle as a job adapter', async (t) => {
  const { run, endpoint } = await sandbox(t);
  for (const env of [
    {},
    { SLICC_LEASE_FD: '3' },
    { SLICC_LEASE_FD: '0' },
    { SLICC_CDP_LEASE_URL: endpoint },
    {
      SLICC_CDP_LEASE_URL:
        'ws://127.0.0.1:9222/slicc/lease/v1/018a7d5f-ab01-4abb-bacf-a57dd66c12e4',
    },
  ]) {
    refused(await run(['--lease', 'L1', 'snapshot'], null, env), 'E_UNAVAILABLE', 'L1');
    refused(await run(request, null, env), 'E_UNAVAILABLE');
  }
  refused(await run(['--lease', 'a.b.c', 'snapshot']), 'E_ARGUMENT');
  refused(await run(['--lease', 'L1', 'discover']), 'E_UNSUPPORTED');
  refused(await run(['--cdp', endpoint, ...request]), 'E_UNSUPPORTED');
});

test('a connected TCP or named Unix socket on fd 3 is not a job channel', {
  skip: process.platform === 'win32',
}, async (t) => {
  const { home, run } = await sandbox(t);
  for (const listen of [{ host: '127.0.0.1', port: 0 }, { path: path.join(home, 'sock') }]) {
    let received = '';
    const server = net.createServer((s) =>
      s.on('data', (data) => {
        received += data;
        s.end();
      })
    );
    server.listen(listen);
    await once(server, 'listening');
    const socket = net.connect(
      'port' in listen ? { ...listen, port: server.address().port } : listen
    );
    try {
      await once(socket, 'connect');
      refused(await run(request, null, { SLICC_LEASE_FD: '3' }, socket), 'E_UNAVAILABLE');
      assert.equal(received, '');
    } finally {
      socket.destroy();
      await new Promise((done) => server.close(done));
    }
  }
});

test('native Unix command path speaks only two methods on the inherited job socket', {
  skip: process.platform === 'win32',
}, async (t) => {
  const { home, run } = await sandbox(t);
  const job = fixture();
  const respond = async (msg) => {
    assert.deepEqual(Object.keys(msg).sort(), ['id', 'method', 'params']);
    assert.equal(msg.id, 1);
    const method = { 'Slicc.leaseAction': 'action', 'Slicc.leaseRequest': 'request' }[msg.method];
    assert.ok(method);
    try {
      const result = await job.adapter[method](msg.params);
      return { id: msg.id, result: { ...result, reviewCommand: 'PRIVATE arbitrary command' } };
    } catch (err) {
      return {
        id: msg.id,
        error: { code: -32000, message: err.message, data: { reason: err.reason } },
      };
    }
  };
  await exercise((argv) => run(argv, respond), job);
  assert.deepEqual(await readdir(home), []);
});

test('invalid inputs, unsafe responses and ambiguous outcomes do not emit a command, disclose adapter fields, or retry', {
  skip: process.platform === 'win32',
}, async (t) => {
  const { run } = await sandbox(t);
  for (const args of [
    ['--allow', origin, '--for', '0s'],
    ['--allow', origin, '--allow', origin, '--for', '30s'],
    ['--allow', 'https://a;echo/PRIVATE', '--for', '1h'],
    ['--allow', 'https://user:PRIVATE@example.com', '--for', '1h'],
    ['--allow', `${origin}\nPRIVATE`, '--for', '2h'],
    ['--from', 'L1', '--add', `${origin}/r/1`, '--for', '1h'],
    ['--from', 'a.b.c', '--add', origin],
    ['--from', 'L1', '--add', `${origin}/r/%2fPRIVATE`],
    ['--from', 'L1', '--add', `${origin}/%ZZ`],
  ])
    refused(await run(['lease', 'request', ...args], assert.fail), 'E_ARGUMENT');
  for (const path of ['/', '/r/PRIVATE', '/invoices/**', '/**', '/?q=PRIVATE']) {
    refused(
      await run(['lease', 'request', '--allow', `${origin}${path}`, '--for', '1h'], assert.fail),
      'E_UNSUPPORTED'
    );
  }
  for (const url of ['https://vault.sliccy.ai', 'https://ims-na1.adobelogin.com']) {
    refused(await run(['lease', 'request', '--allow', url, '--for', '1h'], assert.fail), 'E_SCOPE');
  }
  const { jkt } = fixture();
  const good = { request: 'R1', lease: 'L1', jkt };
  for (const patch of [
    { jkt: 'PRIVATE;' },
    { jkt: `${'A'.repeat(42)}B` },
    { jkt: null, reviewCommand: 'slicc lease grant PRIVATE' },
    { request: 'R1\nPRIVATE' },
    { lease: 'PRIVATE.jwt.payload' },
  ]) {
    refused(await run(request, () => ({ id: 1, result: { ...good, ...patch } })), 'E_PROTOCOL');
  }
  for (const reason of ['E_PENDING', 'E_REVOKED', 'E_UNSUPPORTED', 'E_DENIED']) {
    refused(
      await run(['--lease', 'L1', 'click', 'e1'], () => ({
        id: 1,
        error: { message: 'PRIVATE', data: { reason } },
      })),
      reason,
      'L1'
    );
  }
  for (const reply of [
    null,
    { id: 2, result: { result: {} } },
    { id: 1, result: {} },
    { id: 1, result: { result: { error: 'PRIVATE' } } },
    { id: 1, error: { data: { reason: 'PRIVATE' } } },
  ]) {
    let sent = 0;
    refused(
      await run(['--lease', 'L1', 'click', 'e12'], () => {
        sent++;
        return reply;
      }),
      'E_OUTCOME_UNKNOWN',
      'L1'
    );
    assert.equal(sent, 1);
  }
  const snap = await run(['--lease', 'L1', 'snapshot'], () => ({
    id: 1,
    result: { result: { text: '\x1b[2Jinvoice\r', unrelated: 'PRIVATE' } },
  }));
  assert.equal(snap.stdout, '\\x1b[2Jinvoice\\r\n');
});
