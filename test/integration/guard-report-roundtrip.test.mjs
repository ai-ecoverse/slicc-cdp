import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { summary } from '@ai-ecoverse/slicc-browsers/guard-runner';
import { partitions, published } from './guard-set.mjs';

const root = path.resolve(import.meta.dirname, '../..');

test('lock-verified ordinary guard partition reports preserve canonical reasons across serialization', async () => {
  const vectors = fileURLToPath(
    import.meta.resolve('@ai-ecoverse/slicc-browsers/guard-vectors.json')
  );
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
  const directory = await mkdtemp(path.join(tmpdir(), 'guard-partition-roundtrip-'));
  const entry = path.join(directory, 'entry.test.mjs');
  await writeFile(
    entry,
    `import test from 'node:test';import {runPartition} from ${JSON.stringify(pathToFileURL(path.join(root, 'test/integration/guard-set.mjs')).href)};for(const id of ['a','b','c','d'])test(id,async t=>{process.env.SLICC_GUARD_REPORT=${JSON.stringify(directory)}+'/'+id;await runPartition(t,id);});`
  );
  const env = {
    ...process.env,
    TEARDOWN_MODE: 'roundtrip',
    TEARDOWN_VECTORS: vectors,
    SLICC_GUARD_SHA: 'pure-report-roundtrip',
  };
  delete env.NODE_TEST_CONTEXT;
  const child = spawn(
    process.execPath,
    ['--import', './test/integration/guard-teardown-fixture.mjs', '--test', entry],
    { cwd: root, env }
  );
  let log = '';
  child.stdout.on('data', (chunk) => {
    log += chunk;
  });
  child.stderr.on('data', (chunk) => {
    log += chunk;
  });
  const timer = setTimeout(
    () => child.kill('SIGKILL'),
    Number(process.env.SLICC_ROW_TIMEOUT_MS ?? 605000)
  );
  try {
    const [code] = await new Promise((resolve) =>
      child.once('close', (...details) => resolve(details))
    );
    assert.equal(code, 0, log);
    const union = [];
    for (const id of Object.keys(partitions)) {
      const saved = JSON.parse(await readFile(path.join(directory, `${id}.json`), 'utf8'));
      assert.equal(saved.pass, true);
      union.push(...saved.results);
    }
    const expected = published.vectors.reduce(
      (count, vector) => count + vector.targets.length * Object.keys(published.variants).length,
      0
    );
    assert.equal(union.length, expected);
    const reread = summary({
      set: published,
      results: union,
      guard: 'slicc-cdp',
      sha: 'pure-report-roundtrip',
    });
    assert.equal(reread.pass, true);
    assert.deepEqual(reread.forged, []);
    assert.deepEqual(reread.missing, []);
    assert.ok(union.some((row) => row.control === 'skip' && row.reason === 'not-applicable'));
  } finally {
    clearTimeout(timer);
    child.kill('SIGKILL');
    await rm(directory, { recursive: true, force: true });
  }
});
