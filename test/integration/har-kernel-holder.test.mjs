import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

const holder =
  '(while sleep 5; do echo || break; done) | playwright-cli -s har har-start --tab TAB /tmp/traffic.har > /tmp/har.log 2>&1 &';

async function until(what, check, ms = 60000) {
  const end = Date.now() + ms;
  for (;;) {
    const found = await check();
    if (found) return found;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

test('a backgrounded har-start fed by a sleep loop records across bash calls until har-stop in the kernel', async () => {
  const emitters = new Map();
  const backend = await fakeCdp({
    onCommand(message, emit) {
      if (message.sessionId) emitters.set(message.sessionId, emit);
    },
  });
  backend.targets.push({ targetId: 'TAB', url: 'https://example.test/', loaderId: 'L' });
  const send = (method, params) => {
    for (const emit of emitters.values()) emit(method, params);
  };
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    await runner.install('@ai-ecoverse/wasm-bash');
    await runner.install('@ai-ecoverse/wasm-coreutils');
    const cwd = await runner.fresh();
    const log = async () => {
      const text = new TextDecoder().decode(
        await runner.readFile('/tmp/har.log').catch(() => new Uint8Array())
      );
      return text
        .split('\n')
        .filter((line) => line.startsWith('{'))
        .map(JSON.parse);
    };
    const started = Date.now();
    const first = await runner.run(['bash', '-c', holder], cwd);
    assert.equal(first.code, 0, first.stderr);
    assert.ok(Date.now() - started < 30000, 'the backgrounding call returns');
    const ready = await until('ready', async () =>
      (await log()).find((event) => event.event === 'ready')
    );
    assert.match(ready.recordingId, /^rec-/);
    assert.equal(ready.path, '/tmp/traffic.har');
    const running = (await runner.processes()).map((entry) => entry.argv.join(' '));
    assert.ok(
      running.some((argv) => argv.includes('har-start')),
      running.join('\n')
    );
    assert.ok(
      running.some((argv) => argv.startsWith('bash')),
      running.join('\n')
    );
    send('Network.requestWillBeSent', {
      requestId: 'R',
      loaderId: 'L',
      timestamp: 1,
      wallTime: 1700000000,
      documentURL: 'https://example.test/',
      type: 'Fetch',
      request: { url: 'https://example.test/api/items', method: 'GET', headers: {} },
    });
    send('Network.responseReceived', {
      requestId: 'R',
      timestamp: 1.1,
      response: {
        url: 'https://example.test/api/items',
        headers: {},
        status: 200,
        statusText: 'OK',
        mimeType: 'application/json',
        encodedDataLength: 2,
      },
    });
    send('Network.loadingFinished', { requestId: 'R', timestamp: 1.2 });
    send('Page.frameNavigated', {
      frame: {
        id: 'TAB',
        loaderId: 'L',
        url: 'https://example.test/',
        securityOrigin: 'https://example.test',
      },
    });
    await until('checkpoint', async () =>
      (await log()).find((event) => event.event === 'checkpoint' && event.entries === 1)
    );
    await new Promise((resolve) => setTimeout(resolve, 12000));
    assert.ok(!(await log()).some((event) => event.event === 'stopped'));
    const second = await runner.run(['bash', '-c', 'playwright-cli -s har har-stop'], cwd);
    let diagnostics = '';
    if (second.code !== 0) {
      const processes = await runner.processes();
      const pid = processes.find((entry) => entry.argv.includes('har-start'))?.pid;
      const process = Number.isInteger(pid)
        ? await runner.run(['bash', '-c', `cat /proc/${pid}/stat /proc/stat`], cwd)
        : undefined;
      diagnostics = JSON.stringify({ afterLog: await log(), processes, process });
    }
    assert.equal(second.code, 0, `${second.stderr} ${diagnostics}`);
    const stopped = JSON.parse(second.stdout.trim());
    assert.equal(stopped.event, 'stopped');
    assert.equal(stopped.reason, 'stop');
    assert.equal(stopped.recordingId, ready.recordingId);
    assert.equal(stopped.entries, 1);
    const har = JSON.parse(new TextDecoder().decode(await runner.readFile('/tmp/traffic.har')));
    assert.equal(har.log.version, '1.2');
    assert.deepEqual(
      har.log.entries.map((entry) => entry.request.url),
      ['https://example.test/api/items']
    );
    assert.equal(har.log.entries[0].response.status, 200);
    await until('the holder to log stopped', async () =>
      (await log()).find((event) => event.event === 'stopped' && event.reason === 'stop')
    );
    let left = [];
    await until(
      `no process left running: ${JSON.stringify(left)}`,
      async () => {
        left = (await runner.processes()).map((entry) => entry.argv.join(' '));
        return left.length === 0;
      },
      30000
    ).catch(() => assert.deepEqual(left, []));
    const again = await runner.run(['bash', '-c', 'playwright-cli -s har har-stop'], cwd);
    assert.equal(again.code, 1);
    assert.match(again.stderr, /no HAR recording/);
  } finally {
    runner?.close();
    await backend.close();
  }
});
