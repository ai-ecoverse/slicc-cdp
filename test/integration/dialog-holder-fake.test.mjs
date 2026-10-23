import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

const binary = path.resolve(
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const URL = 'https://ordinary.test/';

function holder(url, home) {
  const child = spawn(binary, ['--cdp', url, '-s', 'dialogs', 'dialog-hold', '--tab', 'TAB'], {
    cwd: home,
    env: { ...process.env, HOME: home, USERPROFILE: home, XDG_CONFIG_HOME: home },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  let pending = '';
  const events = [];
  const listeners = new Set();
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  child.stdout.on('data', (chunk) => {
    pending += chunk;
    for (;;) {
      const newline = pending.indexOf('\n');
      if (newline < 0) break;
      const event = JSON.parse(pending.slice(0, newline));
      pending = pending.slice(newline + 1);
      events.push(event);
      for (const listener of listeners) listener(event);
    }
  });
  const ended = new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stderr }));
  });
  const wait = (predicate) => {
    const found = events.find(predicate);
    if (found) return Promise.resolve(found);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        listeners.delete(listener);
        reject(new Error(`holder event timed out: ${stderr}`));
      }, 10000);
      const listener = (event) => {
        if (predicate(event)) {
          clearTimeout(timer);
          listeners.delete(listener);
          resolve(event);
        }
      };
      listeners.add(listener);
      ended.then((result) => {
        clearTimeout(timer);
        listeners.delete(listener);
        reject(new Error(`holder ended: ${result.stderr}`));
      }, reject);
    });
  };
  return { child, events, wait, ended };
}

async function fixture(body) {
  let channel;
  const backend = await fakeCdp({
    intercept: (message, send) => {
      if (message.method === 'Page.enable') {
        channel = { send, sessionId: message.sessionId };
        send({ id: message.id, sessionId: message.sessionId, result: {} });
        return true;
      }
      return false;
    },
  });
  backend.targets.push({ targetId: 'TAB', url: URL });
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-dialog-holder-'));
  const process = holder(backend.url, home);
  const emit = (method, params) => channel.send({ method, params, sessionId: channel.sessionId });
  try {
    await body(process, backend, emit);
  } finally {
    process.child.stdin.end();
    process.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
}

const request = (process, value) => process.child.stdin.write(`${JSON.stringify(value)}\n`);

test('foreground holder correlates retained-dialog requests, has no mailbox and ends at EOF', async () => {
  await fixture(async (process, backend, emit) => {
    const ready = await process.wait((event) => event.event === 'ready');
    assert.equal(ready.target, 'TAB');
    assert.equal(ready.loaderId, null);
    assert.equal(ready.loaderIdSource, 'unavailable');
    assert.equal(ready.status, 'observing');
    request(process, { id: 1, dialogId: 1, command: 'dialog-accept' });
    assert.equal((await process.wait((event) => event.id === 1)).event, 'no-dialog');
    emit('Page.frameNavigated', {
      frame: { id: 'TAB', loaderId: 'L1', url: URL, securityOrigin: 'https://ordinary.test' },
    });
    const document = await process.wait((event) => event.event === 'document-ready');
    assert.equal(document.loaderId, 'L1');
    assert.equal(document.loaderIdSource, 'frame-navigated');
    assert.equal(document.status, 'document-verified');
    emit('Page.javascriptDialogOpening', {
      frameId: 'TAB',
      url: URL,
      type: 'prompt',
      message: 'PRIVATE PROMPT',
    });
    const open = await process.wait((event) => event.event === 'dialog-opened');
    request(process, { id: 2, dialogId: open.dialogId + 1, command: 'dialog-dismiss' });
    assert.equal((await process.wait((event) => event.id === 2)).event, 'stale-dialog');
    request(process, {
      id: 3,
      dialogId: open.dialogId,
      command: 'dialog-accept',
      promptText: 'PRIVATE ANSWER',
    });
    const answered = await process.wait((event) => event.id === 3);
    assert.equal(answered.event, 'handled');
    assert.equal(answered.accept, true);
    assert.deepEqual(
      backend.calls
        .filter((call) => call.method === 'Page.handleJavaScriptDialog')
        .map((call) => call.params),
      [{ accept: true, promptText: 'PRIVATE ANSWER' }]
    );
    request(process, { id: 4, dialogId: open.dialogId, command: 'dialog-dismiss' });
    assert.equal((await process.wait((event) => event.id === 4)).event, 'no-dialog');
    process.child.stdin.end();
    assert.equal((await process.ended).code, 0);
    assert.ok(
      process.events.some((event) => event.event === 'stopped' && event.reason === 'stdin-eof')
    );
    assert.ok(!JSON.stringify(process.events).includes('PRIVATE'));
    assert.ok(!JSON.stringify(process.events).includes(URL));
    assert.equal(
      backend.calls.filter((call) =>
        ['Network.enable', 'Runtime.enable', 'Page.getFrameTree'].includes(call.method)
      ).length,
      0
    );
  });
});

test('holder refuses protected and unknown dialog ownership without reflecting URL values', async () => {
  await fixture(async (process, backend, emit) => {
    await process.wait((event) => event.event === 'ready');
    for (const [index, url, frameId] of [
      [1, 'https://seven.sliccy.ai/?secret=PRIVATE', 'TAB'],
      [2, 'http://vault.sliccy.ai/', 'TAB'],
      [3, '', 'TAB'],
      [4, URL, 'CHILD'],
    ]) {
      emit('Page.javascriptDialogOpening', { frameId, url, type: 'alert' });
      const refused = await process.wait(
        (event) => event.event === 'dialog-refused' && event.dialogId === index
      );
      request(process, { id: index, dialogId: refused.dialogId, command: 'dialog-dismiss' });
      assert.equal((await process.wait((event) => event.id === index)).event, 'dialog-refused');
    }
    assert.equal(
      backend.calls.filter((call) => call.method === 'Page.handleJavaScriptDialog').length,
      0
    );
    assert.ok(!JSON.stringify(process.events).includes('PRIVATE'));
  });
});

test('holder validates line JSON and does not invent document loader ownership', async () => {
  await fixture(async (process, backend, emit) => {
    await process.wait((event) => event.event === 'ready');
    request(process, { id: 5, dialogId: 1, command: 'dialog-dismiss', promptText: 'bad' });
    assert.equal((await process.wait((event) => event.id === 5)).event, 'invalid-request');
    request(process, { id: 6, dialogId: 1, command: 'unknown' });
    assert.equal((await process.wait((event) => event.id === 6)).event, 'invalid-request');
    request(process, { id: 7, dialogId: 1, command: 'dialog-accept', extra: true });
    assert.equal((await process.wait((event) => event.id === 7)).event, 'invalid-request');
    emit('Page.frameNavigated', {
      frame: {
        id: 'CHILD',
        parentId: 'TAB',
        loaderId: 'UNVERIFIED',
        url: URL,
        securityOrigin: 'https://ordinary.test',
      },
    });
    emit('Page.javascriptDialogOpening', { frameId: 'TAB', url: URL, type: 'confirm' });
    const open = await process.wait((event) => event.event === 'dialog-opened');
    assert.equal(process.events.filter((event) => event.event === 'document-ready').length, 0);
    request(process, { id: 8, dialogId: open.dialogId, command: 'dialog-dismiss' });
    assert.equal((await process.wait((event) => event.id === 8)).event, 'handled');
    assert.deepEqual(
      backend.calls
        .filter((call) => call.method === 'Page.handleJavaScriptDialog')
        .map((call) => call.params),
      [{ accept: false }]
    );
  });
});

test('holder stops on its flat-session detach with stdin still open and discovery disabled', async () => {
  for (const observed of [false, true]) {
    await fixture(async (process, backend, emit) => {
      await process.wait((event) => event.event === 'ready');
      if (observed) {
        emit('Page.javascriptDialogOpening', { frameId: 'TAB', url: URL, type: 'confirm' });
        await process.wait((event) => event.event === 'dialog-opened');
      }
      const held = backend.calls.find((call) => call.method === 'Page.enable').sessionId;
      emit('Target.detachedFromTarget', { sessionId: 'UNRELATED' });
      request(process, { id: 91, dialogId: 999, command: 'dialog-dismiss' });
      assert.ok(
        ['no-dialog', 'stale-dialog'].includes(
          (await process.wait((event) => event.id === 91)).event
        )
      );
      assert.equal(
        backend.calls.filter((call) => call.method === 'Target.setDiscoverTargets').length,
        0
      );
      emit('Page.frameNavigated', {
        frame: { id: 'TAB', loaderId: 'LAST', url: URL, securityOrigin: 'https://ordinary.test' },
      });
      emit('Target.detachedFromTarget', { sessionId: held, targetId: 'TAB' });
      const stopped = await process.wait((event) => event.event === 'stopped');
      assert.equal(stopped.reason, 'target closed, changed or refused');
      assert.notEqual((await process.ended).code, 0);
      assert.equal(process.child.stdin.writableEnded, false);
      assert.equal(
        backend.calls.filter((call) => call.method === 'Page.handleJavaScriptDialog').length,
        0
      );
      assert.equal(process.events.filter((event) => event.event === 'stopped').length, 1);
    });
  }
});

test('WASI holder follows stdin EOF and uses the same JSON-ready protocol', async () => {
  const backend = await fakeCdp();
  backend.targets.push({ targetId: 'TAB', url: URL });
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    const result = await runner.playwright(['dialog-hold', '--tab', 'TAB'], await runner.fresh(), {
      stdin: '',
    });
    assert.equal(result.code, 0, result.stderr);
    const events = result.stdout
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
    assert.equal(events[0].event, 'ready');
    assert.equal(events[0].loaderId, null);
    assert.equal(events.at(-1).event, 'stopped');
    assert.equal(events.at(-1).reason, 'stdin-eof');
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await backend.close();
  }
});
