import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../..');

test('the pipeline test-glob owns every guard partition, union and redirect file without workflow shadow lists', async () => {
  const workflow = await readFile(path.join(root, '.github/workflows/ci.yml'), 'utf8');
  const packageJson = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8'));
  const inventory = (await readdir(import.meta.dirname)).filter((file) =>
    file.endsWith('.test.mjs')
  );
  assert.match(
    workflow,
    /uses: ai-ecoverse\/slicc-shared-web\/\.github\/workflows\/pipeline\.yml@/
  );
  assert.match(workflow, /test-glob: test\/integration\/\*\.test\.mjs/);
  assert.ok(packageJson.scripts.test.includes('test/integration/*.test.mjs'));
  for (const file of [
    'guard-a.test.mjs',
    'guard-b.test.mjs',
    'guard-c.test.mjs',
    'guard-d.test.mjs',
    'guard-union.test.mjs',
    'guard-c2-redirect.test.mjs',
    'redirect-navigation.test.mjs',
  ])
    assert.ok(inventory.includes(file), file);
  assert.equal(
    inventory.includes('guard-runner.test.mjs'),
    false,
    'no duplicate unpartitioned guard sweep'
  );
  for (const job of [
    'integration-suite:',
    'guard-partitions:',
    'guard-union:',
    'redirect-regressions:',
  ])
    assert.equal(workflow.includes(`  ${job}`), false);
});

test('a newly added ordinary file is covered exactly once by the pipeline shared-glob shard loader', async () => {
  const directory = await mkdtemp(path.join(tmpdir(), 'pipeline-glob-regression-'));
  const require = createRequire(import.meta.url);
  const sharedPackage = path.dirname(require.resolve('@ai-ecoverse/slicc-shared-web/package.json'));
  const loader = path.join(sharedPackage, 'harness/shard.cjs');
  const files = ['guard-owned', 'redirect-owned', 'ordinary', 'new-manifest-only'].map((name) =>
    path.join(directory, `${name}.test.mjs`)
  );
  try {
    for (const file of files)
      await writeFile(file, "import test from 'node:test';test('owned behaviour',()=>{});\n");
    const actual = [];
    for (const shard of [0, 1]) {
      const junit = path.join(directory, `${shard}.xml`);
      const env = {
        ...process.env,
        SLICC_TEST_ALL: files.map((file) => path.basename(file)).join('\n'),
        SLICC_TEST_FILES: files
          .filter((_, index) => index % 2 === shard)
          .map((file) => path.basename(file))
          .join('\n'),
        SLICC_TEST_SHARD: String(shard),
        SLICC_TEST_JUNIT: junit,
        SLICC_TEST_NODE_OPTIONS: '',
      };
      delete env.NODE_TEST_CONTEXT;
      const ran = spawnSync(process.execPath, ['--require', loader, '--test', '*.test.mjs'], {
        cwd: directory,
        encoding: 'utf8',
        env,
      });
      assert.equal(ran.status, 0, ran.stderr);
      const records = (await readdir(directory)).filter(
        (name) => name.startsWith(`${shard}-`) && name.endsWith('.files')
      );
      assert.equal(records.length, 1);
      actual.push(...(await readFile(path.join(directory, records[0]), 'utf8')).trim().split('\n'));
    }
    assert.deepEqual(actual.sort(), files.map((file) => path.basename(file)).sort());
    assert.equal(new Set(actual).size, files.length);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
