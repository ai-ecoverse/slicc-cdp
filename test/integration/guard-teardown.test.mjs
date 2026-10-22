import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../..');

for (const mode of [
  'reject',
  'hang',
  'falsy',
  'registration',
  'fixture-hang',
  'client-hang',
  'observer-hang',
])
  test(`guard ${mode} availability cannot strand a worker or discard measured evidence`, async () => {
    const folder = await mkdtemp(path.join(tmpdir(), 'guard-disposal-regression-'));
    const vectors = path.join(folder, 'vectors.json');
    const report = path.join(folder, 'guard-report');
    await writeFile(
      vectors,
      JSON.stringify({
        variants: { prompt: 0 },
        vectors: [{ id: 'E4.teardown', targets: ['ordinary'] }],
      })
    );
    const environment = {
      ...process.env,
      TEARDOWN_MODE: mode,
      TEARDOWN_VECTORS: vectors,
      SLICC_GUARD_REPORT: report,
      SLICC_GUARD_TARGETS: '["ordinary"]',
      SLICC_GUARD_SHA: 'fake-owned-worker',
      SLICC_GUARD_TIMEOUT: '12000',
    };
    delete environment.NODE_TEST_CONTEXT;
    const worker = spawn(
      process.execPath,
      [
        '--import',
        './test/integration/guard-teardown-fixture.mjs',
        '--test',
        '--test-reporter=tap',
        './test/integration/guard-runner.test.mjs',
      ],
      { cwd: root, env: environment }
    );
    let output = '';
    worker.stdout.on('data', (chunk) => {
      output += chunk;
    });
    worker.stderr.on('data', (chunk) => {
      output += chunk;
    });
    let expired = false;
    const deadline = setTimeout(() => {
      expired = true;
      worker.kill('SIGKILL');
    }, 13000);
    try {
      const [status] = await new Promise((resolve) =>
        worker.once('close', (...details) => resolve(details))
      );
      assert.equal(expired, false, output);
      assert.equal(status, 1, output);
      const evidence = JSON.parse(await readFile(`${report}.json`, 'utf8'));
      const measured = ['reject', 'hang'].includes(mode);
      assert.equal(evidence.results.length, measured ? 1 : 0);
      assert.equal(
        evidence.pass,
        measured,
        'disposal availability must not rewrite candidate observations'
      );
      if (!measured) assert.equal(evidence.missing.length, 1);
      else assert.equal(evidence.results[0].guarded, 'pass');
      assert.equal(Object.hasOwn(evidence, 'harnessErrors'), false);
      const availability = JSON.parse(await readFile(`${report}.availability.json`, 'utf8'));
      assert.equal(availability.type, 'HARNESS_AVAILABILITY');
      assert.equal(availability.failures[0].target, 'ordinary');
      assert.match(
        JSON.stringify(availability),
        mode.includes('hang')
          ? /deadline exceeded/
          : mode === 'registration'
            ? /redirectSecret fixture registration required/
            : mode === 'falsy'
              ? /"message":"0"/
              : /forced browser disposal rejection/
      );
    } finally {
      clearTimeout(deadline);
      worker.kill('SIGKILL');
      await rm(folder, { force: true, recursive: true });
    }
  });
