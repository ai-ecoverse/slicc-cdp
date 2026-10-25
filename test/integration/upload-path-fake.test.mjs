import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);

test('native upload gives Chrome absolute CLI-cwd paths instead of ungrantable relative paths', async () => {
  const backend = await fakeCdp({
    aria: () => ({
      json: [{ role: 'button', name: 'Attachment', ref: 'e1' }],
      refs: [{ ref: 'e1', selector: 'input[type=file]' }],
      base: 'https://example.test/',
      nodes: [{ backendNodeId: 12 }],
    }),
  });
  const runner = nativeRunner(exe('playwright-cli'), exe('curlwright'), backend.url);
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-upload-path-'));
  try {
    await runner.write(cwd, 'attachment.txt', 'fixture bytes');
    await runner.write(cwd, 'nested/other.txt', 'other bytes');
    const run = async (...args) => {
      const result = await runner.playwright(args, cwd);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result.stdout;
    };
    await run('open', 'https://example.test/');
    const snapshot = await run('snapshot');
    const ref = snapshot.match(/"Attachment"[^\n]*\[ref=(e\d+)\]/)?.[1];
    assert.ok(ref, snapshot);
    await run('upload', ref, 'attachment.txt', 'nested/other.txt');
    const upload = backend.calls.find((call) => call.method === 'DOM.setFileInputFiles');
    assert.ok(upload);
    assert.equal(upload.params.backendNodeId, 12);
    assert.ok(upload.params.files.every((file) => path.isAbsolute(file)));
    assert.deepEqual(upload.params.files, [
      path.resolve(cwd, 'attachment.txt'),
      path.resolve(cwd, 'nested/other.txt'),
    ]);
    assert.equal(backend.calls.filter((call) => call.method === 'DOM.setFileInputFiles').length, 1);
    assert.ok(!backend.calls.some((call) => call.method === 'Input.dispatchMouseEvent'));
  } finally {
    await runner.close();
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
});
