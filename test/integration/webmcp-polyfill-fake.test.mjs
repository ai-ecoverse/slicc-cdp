import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { webmcpBackend } from './webmcp-backend.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const binary = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function launch(args, cwd, endpoint) {
  const env = { ...process.env, HOME: cwd, USERPROFILE: cwd, APPDATA: cwd, XDG_CONFIG_HOME: cwd };
  delete env.SLICC_CDP_URL;
  const child = spawn(binary, ['--cdp', endpoint, ...args], {
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
    const timer = setTimeout(
      () => reject(new Error(`holder status did not arrive: ${stderr}`)),
      15000
    );
    const onData = () => {
      const line = stdout.split('\n')[0];
      if (!stdout.includes('\n')) return;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      try {
        resolve(JSON.parse(line));
      } catch (error) {
        reject(error);
      }
    };
    child.stdout.on('data', onData);
    ended.then((result) => {
      clearTimeout(timer);
      if (!stdout.includes('\n')) reject(new Error(result.stderr));
    }, reject);
  });
  ready.catch(() => {});
  return { child, ended, ready };
}

async function run(args, cwd, endpoint) {
  const process = launch(args, cwd, endpoint);
  process.child.stdin.end();
  return process.ended;
}

async function fixture(policy, check) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-fake-'));
  const backend = await webmcpBackend({
    policy,
    definitions: [
      {
        name: 'greet',
        description: 'Greets',
        inputSchema: { type: 'object' },
        annotations: { readOnly: true },
        execute: (input, client) => {
          assert.ok(client.signal);
          return { greeting: input.name };
        },
      },
      {
        name: 'risky',
        description: 'Consequential',
        annotations: { consequential: true },
        execute: () => 'executed',
      },
      { name: 'large', execute: () => 'x'.repeat(100000) },
    ],
  });
  let holder;
  try {
    holder = launch(['webmcp-polyfill', '--tab', 'TAB', '--reload'], cwd, backend.url);
    const status = await holder.ready;
    assert.equal(status.mode, 'polyfill');
    assert.equal(status.reloaded, true);
    assert.equal(JSON.stringify(status).includes('__wmcp_'), false);
    await check(backend, cwd, status);
    holder.child.stdin.end();
    assert.equal((await holder.ended).code, 0);
    assert.ok(
      backend.calls.some((call) => call.method === 'Page.removeScriptToEvaluateOnNewDocument')
    );
    assert.ok(backend.calls.every((call) => call.method !== 'Runtime.addBinding'));
    assert.equal(backend.scripts.size, 0);
  } finally {
    holder?.child.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
}

await test('fake: holder reload, private bridge listing/calling, consequential gate and output cap', async () => {
  await fixture('', async (backend, cwd, status) => {
    assert.ok(status.pauseMicros.length >= 2);
    const listed = await run(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
    assert.equal(listed.code, 0, listed.stderr);
    const tools = JSON.parse(listed.stdout);
    assert.deepEqual(
      tools.map((tool) => tool.name),
      ['greet', 'large', 'risky']
    );
    assert.ok(tools.every((tool) => tool.polyfilled && tool.frameId === 'TAB'));
    const called = await run(
      ['webmcp-call', 'greet', '{"name":"Alice"}', '--tab', 'TAB'],
      cwd,
      backend.url
    );
    assert.equal(called.code, 0, called.stderr);
    assert.deepEqual(JSON.parse(called.stdout), { greeting: 'Alice' });
    const risky = await run(['webmcp-call', 'risky', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(risky.code, 2);
    const allowed = await run(
      ['webmcp-call', 'risky', '{}', '--tab', 'TAB', '--allow-consequential'],
      cwd,
      backend.url
    );
    assert.equal(allowed.code, 0, allowed.stderr);
    assert.equal(allowed.stdout.trim(), 'executed');
    const large = await run(['webmcp-call', 'large', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(large.code, 0, large.stderr);
    assert.equal(large.stdout.trim().length, 65536);
  });
});

await test('fake: tools Permissions Policy denies bridge listing and invocation', async () => {
  await fixture('tools=()', async (backend, cwd) => {
    const listed = await run(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
    assert.equal(listed.code, 0, listed.stderr);
    assert.deepEqual(JSON.parse(listed.stdout), []);
    const called = await run(['webmcp-call', 'greet', '{}', '--tab', 'TAB'], cwd, backend.url);
    assert.equal(called.code, 2);
  });
});
