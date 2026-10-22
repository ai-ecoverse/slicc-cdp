import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { CHROME_FILES, chromeShardFiles } from './chrome-report.mjs';

function childEnv(mode) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (
      key === 'CI' ||
      key === 'SLICC_LOCAL_CHROME' ||
      key === 'NODE_TEST_CONTEXT' ||
      key === 'NODE_OPTIONS' ||
      key === 'GITHUB_STEP_SUMMARY' ||
      key === 'SLICC_CHROME_REPORT_DIR' ||
      key.startsWith('SLICC_TEST_')
    )
      delete env[key];
  }
  return { ...env, ...mode };
}

test('every Chrome-owning integration file is in the shard proof inventory', async () => {
  const directory = import.meta.dirname;
  const files = await readdir(directory);
  const expected = [];
  for (const name of files.filter((name) => name.endsWith('.test.mjs'))) {
    const source = await readFile(path.join(directory, name), 'utf8');
    const gated =
      /import\s+\{[^}]*\bchromeTest\b[^}]*\}\s+from\s+['"]\.\/chrome-policy\.mjs['"]/.test(source);
    const launches =
      /import\s+\{[^}]*\b(?:launchBrowser|launchManagedBrowser)\b[^}]*\}\s+from\s+['"]\.\/browser\.mjs['"]/.test(
        source
      );
    if (launches && name !== 'chrome-policy.test.mjs') {
      assert.ok(gated, `${name} must gate its Chrome cases`);
    }
    if (
      gated ||
      /import\s+\{[^}]*\bchromiumSuite\b[^}]*\}\s+from\s+['"]\.\/chromium-flows\.mjs['"]/.test(
        source
      )
    ) {
      expected.push(path.join(directory, name));
    }
  }
  assert.deepEqual([...CHROME_FILES].sort(), expected.sort());
  const [first, second] = expected;
  assert.deepEqual(chromeShardFiles({ SLICC_TEST_FILES: `${first}\n${second}\n${first}` }, []), [
    first,
    second,
  ]);
});

test('CI shard proof writes started and finished counts and fails missing Chrome activity', async () => {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-chrome-proof-'));
  const file = path.join(dir, 'runner-stub.mjs');
  const runner = new URL('../../scripts/test-integration.mjs', import.meta.url).href;
  const [owned, another] = CHROME_FILES;
  await writeFile(
    file,
    `import childProcess from 'node:child_process';
import { writeFileSync } from 'node:fs';
import path from 'node:path';
import { syncBuiltinESMExports } from 'node:module';
childProcess.spawnSync = () => {
  if (process.env.PROOF_COUNTS && process.env.SLICC_CHROME_REPORT_DIR) {
    writeFileSync(path.join(process.env.SLICC_CHROME_REPORT_DIR, 'stub.json'), process.env.PROOF_COUNTS);
  }
  return { status: 0 };
};
syncBuiltinESMExports();
await import(${JSON.stringify(runner)});
`
  );
  const complete = { file: owned, started: 2, finished: 2, failed: 0, launched: 1 };
  try {
    for (const [saved, files, status, reason] of [
      [null, [owned], 1, 'no real-Chrome test cases started'],
      [complete, [owned], 0, 'PASS'],
      [complete, [owned, another], 1, 'a Chrome file ran no real-Chrome cases'],
      [{ ...complete, finished: 1 }, [owned], 1, 'real-Chrome test cases did not all finish'],
      [{ ...complete, launched: 0 }, [owned], 1, 'no real Chrome launched'],
      [{ ...complete, failed: 1 }, [owned], 1, 'real-Chrome test cases failed'],
    ]) {
      const summary = path.join(dir, 'summary');
      await writeFile(summary, '');
      const result = spawnSync(process.execPath, [file], {
        env: childEnv({
          CI: 'true',
          SLICC_TEST_FILES: files.join('\n'),
          SLICC_TEST_SHARD: '7',
          GITHUB_STEP_SUMMARY: summary,
          ...(saved ? { PROOF_COUNTS: JSON.stringify(saved) } : {}),
        }),
        encoding: 'utf8',
      });
      assert.equal(result.status, status, result.stderr);
      const text = await readFile(summary, 'utf8');
      assert.match(text, /Real Chrome, shard 7/);
      assert.match(text, /Started \| Finished \| Failed \| Launched/);
      assert.ok(text.includes(reason), text);
      if (status) assert.match(result.stderr, /CI Chrome proof failed/);
      else assert.match(text, /\| Total \| 2 \| 2 \| 0 \| 1 \|/);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
