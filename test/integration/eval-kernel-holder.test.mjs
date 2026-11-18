import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { evalBackend } from './eval-backend.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

async function until(check, ms = 30000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = await check();
    if (value) return value;
    if (Date.now() >= end) throw new Error('eval holder timed out');
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

for (const early of [false, true]) {
  test(`kernel eval-attach ${early ? 'early' : 'late'} holder survives bash calls and eval-detach acknowledges shutdown`, async () => {
    const backend = await evalBackend({ evalCalls: true });
    backend.targets.push({
      targetId: 'TAB',
      url: 'https://example.test/',
      loaderId: 'L',
      childFrames: ['SAME', 'CROSS'].map((id) => ({
        frame: {
          id,
          parentId: 'TAB',
          loaderId: `${id}-L`,
          url: `https://${id === 'SAME' ? 'example' : 'foreign'}.test/child`,
          securityOrigin: `https://${id === 'SAME' ? 'example' : 'foreign'}.test`,
        },
      })),
    });
    let runner;
    try {
      runner = await kernelRunner(
        path.resolve(
          process.env.SLICC_EVAL_WASM || 'target/wasm32-wasip1/debug/playwright-cli.wasm'
        ),
        path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
        backend.url
      );
      await runner.install('@ai-ecoverse/wasm-bash');
      await runner.install('@ai-ecoverse/wasm-coreutils');
      const cwd = await runner.fresh();
      await runner.write(cwd, 'shim.js', 'globalThis.kernelValue = true; send({kernel: true});');
      const log = async () => {
        const data = await runner.read(`${cwd}`, 'eval.log').catch(() => new Uint8Array());
        return new TextDecoder()
          .decode(data)
          .split('\n')
          .filter((line) => line.startsWith('{'))
          .map(JSON.parse);
      };
      const started = await runner.run(
        [
          'bash',
          '-c',
          `(while sleep 1; do echo || break; done) | playwright-cli -s eval eval-attach shim.js --tab TAB --world isolated --frames same-origin --frames https://foreign.test ${early ? '--early' : ''} > eval.log 2> eval.err &`,
        ],
        cwd
      );
      assert.equal(started.code, 0, started.stderr);
      const ready = await until(async () =>
        (await log()).find((event) => event.event === 'ready')
      ).catch(async (error) => {
        throw new Error(
          `${error.message}: ${new TextDecoder().decode(await runner.read(cwd, 'eval.err'))} ${JSON.stringify(await runner.processes())}`
        );
      });
      assert.equal(ready.early, early);
      if (early) {
        await until(async () =>
          (await log()).find((event) => event.event === 'refused' && event.reason === 'late')
        );
        backend.navigate(backend.targets[0], 'https://example.test/next', 'NEXT');
      }
      const message = await until(async () =>
        (await log()).find((event) => event.event === 'message' && event.frameId === 'TAB')
      ).catch(async (error) => {
        throw new Error(
          `${error.message}: ${JSON.stringify(await log())} ${new TextDecoder().decode(await runner.read(cwd, 'eval.err'))}`
        );
      });
      assert.deepEqual(message.payload, { kernel: true });
      assert.equal(
        (await log()).find((event) => event.event === 'document-ready' && event.frameId === 'TAB')
          .early,
        early
      );
      const child = await until(async () =>
        (await log()).find(
          (event) =>
            event.event === 'document-ready' &&
            event.frameId === 'SAME' &&
            (!early || event.ancestors[0].loaderId === 'NEXT')
        )
      );
      assert.equal(child.early, false);
      assert.equal(child.origin, 'https://example.test');
      assert.equal(child.parentFrameId, 'TAB');
      await until(async () =>
        (await log()).find(
          (event) =>
            event.frameId === 'CROSS' && event.reason === 'cross-origin child tools are unavailable'
        )
      );
      const called = await runner.playwright(
        [
          '-s',
          'eval',
          'eval',
          'globalThis.kernelValue',
          '--tab',
          'TAB',
          '--frame',
          'SAME',
          '--attachment',
          ready.attachmentId,
          '--loader',
          child.loaderId,
        ],
        cwd
      );
      assert.equal(called.code, 0, called.stderr);
      assert.equal(called.stdout.trim(), 'true');
      assert.ok((await runner.processes()).some((entry) => entry.argv.includes('eval-attach')));
      const detached = await runner.playwright(
        ['-s', 'eval', 'eval-detach', ready.attachmentId],
        cwd
      );
      assert.equal(detached.code, 0, detached.stderr);
      assert.equal(JSON.parse(detached.stdout).reason, 'detach');
      await until(async () => (await log()).find((event) => event.event === 'stopped'));
      await until(async () => (await runner.processes()).length === 0);
      const eof = await runner.run(
        ['bash', '-c', 'echo | playwright-cli -s eof eval-attach shim.js --tab TAB'],
        cwd
      );
      assert.equal(eof.code, 0, eof.stderr);
      assert.equal(JSON.parse(eof.stdout.trim().split('\n').at(-1)).reason, 'stdin');
    } finally {
      runner?.close();
      await backend.close();
    }
  });
}
