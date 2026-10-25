import assert from 'node:assert/strict';
import { globSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import { kernelRunner, kernelSessionFile } from './runners.mjs';
import './watchdog.mjs';

const frame = {
  id: 'TAB',
  loaderId: 'L',
  url: 'https://example.test/',
  securityOrigin: 'https://example.test',
};
const holder =
  '(while sleep 5; do echo || break; done) | playwright-cli har-start --tab TAB /tmp/lease.har > /tmp/lease.log 2>&1 &';

async function until(check, canceled = () => false) {
  const deadline = Date.now() + 10000;
  for (;;) {
    if (canceled()) return undefined;
    const value = await check();
    if (value) return value;
    if (Date.now() >= deadline) throw new Error('HAR lease fixture deadline');
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

for (const transport of ['native', 'kernel']) {
  test(`${transport}: har-stop preserves a live holder whose blocked heartbeat is stale`, async () => {
    let block = false;
    let release;
    let captured;
    const blocked = new Promise((resolve) => {
      captured = resolve;
    });
    const backend = await fakeCdp({
      intercept(message, send) {
        if (block && message.method === 'Page.getFrameTree') {
          block = false;
          release = () =>
            send({
              id: message.id,
              sessionId: message.sessionId,
              result: { frameTree: { frame } },
            });
          captured();
          return true;
        }
        return false;
      },
    });
    backend.targets.push({ targetId: 'TAB', url: frame.url, loaderId: 'L' });
    let runner;
    let held;
    let home;
    let canceled = false;
    try {
      let ready;
      let record;
      let load;
      let store;
      let stop;
      let archive;
      if (transport === 'native') {
        home = await mkdtemp(path.join(tmpdir(), 'slicc-live-har-'));
        const file = path.join(home, 'capture.har');
        held = harHolder(backend.url, ['har-start', '--tab', 'TAB', file], home);
        ready = await held.event('ready');
        [record] = globSync(`${home}/**/*.har.json`);
        load = (file) => readFile(file, 'utf8');
        store = (file, text) => writeFile(file, text);
        stop = () => harCommand(backend.url, ['har-stop'], home);
        archive = () => readFile(file, 'utf8');
      } else {
        runner = await kernelRunner(
          path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
          path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
          backend.url
        );
        await runner.install('@ai-ecoverse/wasm-bash');
        await runner.install('@ai-ecoverse/wasm-coreutils');
        const cwd = await runner.fresh();
        const started = await runner.run(['bash', '-c', holder], cwd);
        assert.equal(started.code, 0, started.stderr);
        load = async (file) => new TextDecoder().decode(await runner.readFile(file));
        store = (file, text) => runner.writeFile(file, text);
        ready = await until(async () =>
          (await load('/tmp/lease.log').catch(() => ''))
            .split('\n')
            .filter((line) => line.startsWith('{'))
            .map(JSON.parse)
            .find((event) => event.event === 'ready')
        );
        record = kernelSessionFile('/root', cwd).replace(/\.json$/, '.har.json');
        stop = () => runner.playwright(['har-stop'], cwd);
        archive = () => load('/tmp/lease.har');
      }
      assert.ok(record);
      block = true;
      await blocked;
      const value = JSON.parse(await load(record));
      assert.equal(value.recordingId, ready.recordingId);
      value.beat = Date.now() - 6000;
      await store(record, JSON.stringify(value));
      const stopping = stop();
      await Promise.race([
        until(
          async () =>
            (await load(record.replace(/\.json$/, '.stop')).catch(() => '')) === ready.recordingId,
          () => canceled
        ),
        stopping.then((result) => assert.equal(result.code, 0, result.stderr)),
      ]);
      release();
      release = undefined;
      const result = await stopping;
      assert.equal(result.code, 0, result.stderr);
      const stopped = JSON.parse(result.stdout.trim());
      assert.equal(stopped.recordingId, ready.recordingId);
      assert.equal(stopped.reason, 'stop');
      assert.equal(stopped.state, 'stopped');
      assert.equal(JSON.parse(await archive()).log.version, '1.2');
      if (held) assert.equal((await held.ended).code, 0);
    } finally {
      canceled = true;
      release?.();
      held?.child.kill('SIGKILL');
      runner?.close();
      await backend.close();
      if (home) await rm(home, { recursive: true, force: true });
    }
  });
}
