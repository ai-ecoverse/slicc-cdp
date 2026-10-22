import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';

const root = path.resolve(import.meta.dirname, '../..');

test('lock-verified published guard report keeps canonical skip reasons across serialization', async () => {
  const vectors = fileURLToPath(
    import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')
  );
  const set = JSON.parse(await readFile(vectors, 'utf8'));
  const installed = JSON.parse(
    await readFile(
      new URL(
        'package.json',
        import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')
      ),
      'utf8'
    )
  );
  const lock = JSON.parse(await readFile(path.join(root, 'package-lock.json'), 'utf8'));
  assert.equal(
    installed.version,
    lock.packages['node_modules/@ai-ecoverse/slicc-browsers'].version
  );
  const directory = await mkdtemp(path.join(tmpdir(), 'guard-report-roundtrip-'));
  const report = path.join(directory, 'report');
  const env = {
    ...process.env,
    TEARDOWN_MODE: 'roundtrip',
    TEARDOWN_VECTORS: vectors,
    SLICC_GUARD_REPORT: report,
    SLICC_GUARD_SHA: 'pure-report-roundtrip',
  };
  delete env.NODE_TEST_CONTEXT;
  delete env.SLICC_GUARD_TARGETS;
  const child = spawn(
    process.execPath,
    [
      '--import',
      './test/integration/guard-teardown-fixture.mjs',
      '--test',
      'test/integration/guard-runner.test.mjs',
    ],
    { cwd: root, env }
  );
  let log = '';
  child.stdout.on('data', (chunk) => {
    log += chunk;
  });
  child.stderr.on('data', (chunk) => {
    log += chunk;
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  try {
    const [code] = await new Promise((resolve) =>
      child.once('close', (...details) => resolve(details))
    );
    assert.equal(code, 0, log);
    const saved = JSON.parse(await readFile(`${report}.json`, 'utf8'));
    const expected = set.vectors.reduce(
      (count, vector) => count + vector.targets.length * Object.keys(set.variants).length,
      0
    );
    assert.equal(saved.results.length, expected);
    assert.equal(saved.pass, true);
    const reread = summary({ set, results: saved.results, guard: 'slicc-cdp', sha: saved.sha });
    assert.equal(reread.pass, true);
    assert.deepEqual(reread.forged, []);
    assert.deepEqual(reread.missing, []);
    assert.ok(
      saved.results.some((row) => row.control === 'skip' && row.reason === 'not-applicable')
    );
  } finally {
    clearTimeout(timer);
    child.kill('SIGKILL');
    await rm(directory, { recursive: true, force: true });
  }
});
