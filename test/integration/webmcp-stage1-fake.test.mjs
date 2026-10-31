import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { webmcpBackend } from './webmcp-backend.mjs';

const binary = path.resolve('target/debug/playwright-cli');

function processFor(args, cwd, url) {
  const child = spawn(binary, ['--cdp', url, ...args], {
    cwd,
    env: { ...process.env, HOME: cwd, XDG_CONFIG_HOME: cwd, SLICC_CDP_URL: '' },
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
    }, 10000);
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

await test('fake: shipped holder installs late without an unrequested reload', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'thr_uzh6iuph2q-late-'));
  const backend = await webmcpBackend();
  const holder = processFor(['webmcp-polyfill', '--tab', 'TAB'], cwd, backend.url);
  try {
    const status = await holder.ready;
    assert.equal(status.late, true);
    assert.equal(status.reloaded, false);
    assert.equal(status.mode, 'polyfill');
    assert.equal(backend.calls.filter((call) => call.method === 'Page.reload').length, 0);
    assert.ok(
      backend.calls.some((call) => call.method === 'Debugger.setInstrumentationBreakpoint')
    );
    holder.child.stdin.end();
    assert.equal((await holder.ended).code, 0);
    for (const method of [
      'Debugger.pause',
      'Debugger.getScriptSource',
      'Debugger.evaluateOnCallFrame',
      'Page.addScriptToEvaluateOnNewDocument',
      'Runtime.addBinding',
    ])
      assert.ok(
        backend.calls.every((call) => call.method !== method),
        method
      );
  } finally {
    holder.child.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
});

await test('fake: shipped other-extension fallback has a clear zero-tools message and no debugger after refusal', async () => {
  for (const reload of [false, true]) {
    const cwd = await mkdtemp(path.join(tmpdir(), 'thr_uzh6iuph2q-plain-'));
    const backend = await webmcpBackend({
      onCommand(command, _, send) {
        if (command.method !== 'Debugger.enable') return;
        send({
          id: command.id,
          error: {
            code: -32000,
            message:
              'slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them',
          },
        });
        return true;
      },
    });
    const holder = processFor(
      ['webmcp-polyfill', '--tab', 'TAB', ...(reload ? ['--reload'] : [])],
      cwd,
      backend.url
    );
    try {
      const status = await holder.ready;
      assert.equal(status.late, !reload);
      assert.equal(status.reloaded, reload);
      const listed = processFor(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
      listed.child.stdin.end();
      const result = await listed.ended;
      assert.notEqual(result.code, 0);
      assert.equal(result.stdout, '');
      assert.match(result.stderr, /no tools are available through the other-extension fallback/);
      assert.match(result.stderr, /page startup or parse-time checks/);
      holder.child.stdin.end();
      const closed = await holder.ended;
      assert.equal(closed.code, 0, closed.stderr);
      assert.match(closed.stderr, /other-extension fallback/);
      assert.equal(
        backend.calls.filter((call) => call.method === 'Page.reload').length,
        Number(reload)
      );
      const refusal = backend.calls.findIndex((call) => call.method === 'Debugger.enable');
      assert.ok(refusal >= 0);
      assert.ok(
        backend.calls.slice(refusal + 1).every((call) => !call.method.startsWith('Debugger.'))
      );
      assert.ok(
        backend.calls.some(
          (call) =>
            call.method === 'DOM.getDocument' && call.params.depth === -1 && call.params.pierce
        )
      );
      assert.ok(
        backend.calls.some(
          (call) =>
            call.method === 'Runtime.evaluate' && call.params.expression.startsWith('const __wmcp_')
        )
      );
    } finally {
      holder.child.kill('SIGKILL');
      await backend.close();
      await rm(cwd, { recursive: true, force: true });
    }
  }
});

await test('fake: shipped plain fallback retains ordinary iframes and lists and calls a late registration', async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'thr_uzh6iuph2q-plain-tools-'));
  let registrations = 0;
  const backend = await webmcpBackend({
    children: [{ id: 'CHILD', origin: 'https://ordinary.test' }],
    onCommand(command, pages, send) {
      if (command.method === 'Debugger.enable') {
        send({
          id: command.id,
          error: {
            code: -32000,
            message: 'slicc: refused: that execution context belongs to a browser extension',
          },
        });
        return true;
      }
      if (
        command.method === 'Runtime.evaluate' &&
        command.params.expression.startsWith('const __wmcp_')
      ) {
        setImmediate(() => {
          pages
            .get('TAB')
            .document.modelContext.registerTool({ name: 'late', execute: () => 'called' });
          registrations++;
        });
      }
    },
  });
  const holder = processFor(['webmcp-polyfill', '--tab', 'TAB', '--reload'], cwd, backend.url);
  try {
    await holder.ready;
    const listed = processFor(['webmcp-list', '--tab', 'TAB', '--json'], cwd, backend.url);
    listed.child.stdin.end();
    const tools = await listed.ended;
    assert.equal(tools.code, 0, tools.stderr);
    assert.deepEqual(
      JSON.parse(tools.stdout).map((tool) => tool.name),
      ['late']
    );
    const called = processFor(['webmcp-call', 'late', '{}', '--tab', 'TAB'], cwd, backend.url);
    called.child.stdin.end();
    const result = await called.ended;
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout.trim(), 'called');
    assert.equal(registrations, 1);
    assert.equal(backend.calls.filter((call) => call.method === 'Page.reload').length, 1);
    const refusal = backend.calls.findIndex((call) => call.method === 'Debugger.enable');
    assert.ok(
      backend.calls.slice(refusal + 1).every((call) => !call.method.startsWith('Debugger.'))
    );
    holder.child.stdin.end();
    assert.equal((await holder.ended).code, 0);
  } finally {
    holder.child.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
});
