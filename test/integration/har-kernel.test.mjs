import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

test('slicc6 HAR aliases open a new tab and persist HAR 1.2 in the kernel recordings directory', async () => {
  const backend = await fakeCdp();
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    await runner.writeFile('/recordings/.keep', '');
    const cwd = await runner.fresh();
    const held = await runner.playwright(
      ['record', 'https://example.test/page', '--filter=**/api/**'],
      cwd,
      { stdin: '' }
    );
    assert.equal(held.code, 0, held.stderr);
    const events = held.stdout.trim().split('\n').map(JSON.parse);
    const ready = events.find((event) => event.event === 'ready');
    assert.match(ready.recordingId, /^rec-/);
    assert.equal(ready.path, `/recordings/${ready.recordingId}/recording.har`);
    assert.equal(events.at(-1).reason, 'stdin');
    assert.equal(
      JSON.parse(new TextDecoder().decode(await runner.readFile(ready.path))).log.version,
      '1.2'
    );
    assert.ok(backend.calls.some((call) => call.method === 'Target.createTarget'));
    assert.ok(
      backend.calls.some(
        (call) => call.method === 'Page.navigate' && call.params.url === 'https://example.test/page'
      )
    );
    assert.ok(!backend.calls.some((call) => call.method === 'Network.getResponseBody'));
    const stopped = await runner.playwright(['stop-recording', ready.recordingId], cwd);
    assert.equal(stopped.code, 1);
    assert.match(stopped.stderr, /no HAR recording/);
    const heldFile = await runner.playwright(
      ['har-start', '--tab', ready.tab, 'capture.har'],
      cwd,
      { stdin: '' }
    );
    assert.equal(heldFile.code, 0, heldFile.stderr);
    assert.equal(
      JSON.parse(new TextDecoder().decode(await runner.read(cwd, 'capture.har'))).log.version,
      '1.2'
    );
  } finally {
    await runner?.close();
    await backend.close();
  }
});
