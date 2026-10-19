import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const wasm = path.join(root, 'target/wasm32-wasip1/debug');

test('a protected-target refusal returns while discovery events keep arriving', {
  timeout: 20000,
}, async () => {
  const backend = await fakeCdp({
    refuse: ['Target.getTargetInfo'],
    refusalMessage:
      'slicc: refused: seven’s own pages and the vault are out of reach of browser automation',
    noisyRefusal: true,
  });
  let runner;
  let timer;
  try {
    runner = await kernelRunner(
      path.join(wasm, 'playwright-cli.wasm'),
      path.join(wasm, 'curlwright.wasm'),
      backend.url
    );
    const cwd = await runner.fresh();
    const result = await Promise.race([
      runner.playwright(['eval', '--tab', 'VAULT', 'document.title'], cwd),
      new Promise((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error('the protected-target refusal stalled before the next send')),
          5000
        );
      }),
    ]);
    assert.equal(result.code, 1, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, "slicc: refused: target is on seven's origin\n");
    assert.ok(backend.calls.some((call) => call.method === 'Slicc.action'));
    assert.ok(backend.calls.every((call) => call.method !== 'Target.attachToTarget'));
  } finally {
    clearTimeout(timer);
    await runner?.close();
    await backend.close();
  }
});
