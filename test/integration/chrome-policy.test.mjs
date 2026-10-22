import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser, launchManagedBrowser } from './browser.mjs';
import { CHROME_REFUSAL, chromeSkipReason } from './chrome-policy.mjs';
import { runPartition } from './guard-set.mjs';
import { kernelRunner, nativeRunner, runProcess } from './runners.mjs';

function setEnv(env) {
  for (const key of ['CI', 'SLICC_LOCAL_CHROME']) {
    if (env[key] === undefined) delete process.env[key];
    else process.env[key] = env[key];
  }
}

function childEnv(mode = {}) {
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

const modes = [{}, { CI: 'true' }, { SLICC_LOCAL_CHROME: '1' }, { SLICC_LOCAL_CHROME: 'true' }];

test('real Chrome launch boundaries refuse local runs and allow CI or explicit opt-in', async (t) => {
  const saved = { CI: process.env.CI, SLICC_LOCAL_CHROME: process.env.SLICC_LOCAL_CHROME };
  const profiles = new Set();
  let spawned = 0;
  t.mock.method(childProcess, 'spawn', (_file, args) => {
    spawned += 1;
    const profile = args.find((arg) => arg.startsWith('--user-data-dir='));
    if (profile) profiles.add(profile.slice('--user-data-dir='.length));
    throw new Error('stub launch reached');
  });
  t.mock.method(childProcess, 'execFileSync', () => {
    throw new Error('stub pool setup reached');
  });
  syncBuiltinESMExports();
  try {
    for (const env of modes) {
      setEnv(env);
      const allowed = !chromeSkipReason();
      const expected = allowed ? /stub launch reached/ : { message: CHROME_REFUSAL };
      const before = spawned;
      await assert.rejects(launchBrowser(), expected);
      await assert.rejects(
        launchManagedBrowser({ executable: process.execPath, sandbox: false }),
        expected
      );
      if (allowed) {
        await assert.rejects(runProcess('/stub/chromium', [], tmpdir()), expected);
        await assert.rejects(runProcess('/stub/playwright-cli', ['open'], tmpdir()), expected);
        assert.equal(typeof nativeRunner('pw', 'curl').playwright, 'function');
      } else {
        assert.throws(() => runProcess('/stub/chromium', [], tmpdir()), expected);
        assert.throws(() => runProcess('/stub/playwright-cli', ['open'], tmpdir()), expected);
        assert.throws(() => nativeRunner('pw', 'curl'), expected);
        await assert.rejects(kernelRunner('pw.wasm', 'curl.wasm'), expected);
      }
      await assert.rejects(
        runPartition({}, 'a'),
        allowed ? /stub pool setup reached/ : { message: CHROME_REFUSAL }
      );
      assert.equal(spawned - before, allowed ? 4 : 0);
    }
  } finally {
    setEnv(saved);
    t.mock.restoreAll();
    syncBuiltinESMExports();
    await Promise.all(
      [...profiles].map((profile) => rm(profile, { recursive: true, force: true }))
    );
  }
});

test('Chrome case registration skips locally without hiding fake cases or skipping on CI', async () => {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-chrome-policy-'));
  const file = path.join(dir, 'mixed.test.mjs');
  const policy = new URL('./chrome-policy.mjs', import.meta.url).href;
  await writeFile(
    file,
    `import test from 'node:test';
import { chromeTest } from ${JSON.stringify(policy)};
test('fake case', () => {});
chromeTest('Chrome case', () => { console.log('CHROME_CASE_RAN'); if (process.env.THROW_CHROME) throw new Error('stub Chrome launch failed'); });
chromeTest('Chrome case with options', { timeout: 1000 }, () => { console.log('CHROME_OPTIONS_RAN'); if (process.env.THROW_CHROME) throw new Error('stub Chrome launch failed'); });
`
  );
  try {
    for (const mode of modes) {
      const env = childEnv(mode);
      const result = childProcess.spawnSync(
        process.execPath,
        ['--test', '--test-reporter=tap', file],
        { env, encoding: 'utf8' }
      );
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /ok 1 - fake case/);
      if (chromeSkipReason(env)) {
        assert.ok(result.stdout.includes(`# SKIP ${CHROME_REFUSAL}`), result.stdout);
        assert.match(result.stdout, /# pass 1/);
        assert.match(result.stdout, /# skipped 2/);
        assert.ok(!result.stdout.includes('CHROME_CASE_RAN'));
        assert.ok(!result.stdout.includes('CHROME_OPTIONS_RAN'));
      } else {
        assert.match(result.stdout, /CHROME_CASE_RAN/);
        assert.match(result.stdout, /CHROME_OPTIONS_RAN/);
        assert.match(result.stdout, /# pass 3/);
        assert.match(result.stdout, /# skipped 0/);
      }
    }
    const failed = childProcess.spawnSync(
      process.execPath,
      ['--test', '--test-reporter=tap', file],
      { env: childEnv({ CI: 'true', THROW_CHROME: '1' }), encoding: 'utf8' }
    );
    assert.equal(failed.status, 1, failed.stderr);
    assert.match(failed.stdout, /stub Chrome launch failed/);
    assert.match(failed.stdout, /# fail 2/);
    assert.match(failed.stdout, /# skipped 0/);
    await writeFile(
      file,
      `import { chromeTest } from ${JSON.stringify(policy)};
chromeTest('CI overrides skip options', { skip: true }, async t => {
  await t.test('nested two arguments', () => {});
  await t.test('nested skip option', { skip: true }, () => {});
});
`
    );
    const strict = childProcess.spawnSync(
      process.execPath,
      ['--test', '--test-reporter=tap', file],
      { env: childEnv({ CI: 'true' }), encoding: 'utf8' }
    );
    assert.equal(strict.status, 0, strict.stderr);
    assert.match(strict.stdout, /# pass 3/);
    assert.match(strict.stdout, /# skipped 0/);
    await writeFile(
      file,
      `import { chromeTest } from ${JSON.stringify(policy)};
chromeTest('CI cannot skip a launch failure', t => t.skip('launch failed'));
`
    );
    const attempted = childProcess.spawnSync(
      process.execPath,
      ['--test', '--test-reporter=tap', file],
      { env: childEnv({ CI: 'true' }), encoding: 'utf8' }
    );
    assert.equal(attempted.status, 1, attempted.stderr);
    assert.match(attempted.stdout, /real Chrome cases must not skip on CI/);
    assert.match(attempted.stdout, /# skipped 0/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
