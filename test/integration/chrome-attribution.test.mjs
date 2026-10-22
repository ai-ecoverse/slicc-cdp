import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import * as chromePolicy from './chrome-policy.mjs';

const { chromeSkipReason, chromeTestArguments } = chromePolicy;

const root = path.resolve(import.meta.dirname, '../..');

test('guard wrappers retain Chrome policy and are the measured JUnit file', async () => {
  for (const partition of ['a', 'b', 'c', 'd']) {
    const source = await readFile(
      path.join(root, `test/integration/guard-${partition}.test.mjs`),
      'utf8'
    );
    assert.match(source, /import test from 'node:test'/);
    assert.match(source, /\.\.\.chromeTestArguments\(/);
    assert.match(source, new RegExp(`runPartition\\(t, '${partition}'\\)`));
  }
  const dir = await mkdtemp(path.join(tmpdir(), 'chrome-attribution-pure-'));
  const file = path.join(dir, 'wrapper.test.mjs');
  const report = path.join(dir, 'junit.xml');
  const policy = new URL('./chrome-policy.mjs', import.meta.url).href;
  await writeFile(
    file,
    `import test from 'node:test';import {chromeTestArguments} from ${JSON.stringify(policy)};test('fake wrapper',...chromeTestArguments(()=>{}));`
  );
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  try {
    const result = spawnSync(
      process.execPath,
      ['--test', '--test-reporter=junit', `--test-reporter-destination=${report}`, file],
      { env, encoding: 'utf8' }
    );
    assert.equal(result.status, 0, result.stderr);
    const junit = await readFile(report, 'utf8');
    const attrs = /<testcase\b([^>]*\bname="fake wrapper"[^>]*)>/.exec(junit)?.[1];
    assert.ok(attrs, junit);
    assert.equal(/\bfile="([^"]*)"/.exec(attrs)?.[1], await realpath(file));
    assert.ok(Number(/\btime="([^"]*)"/.exec(attrs)?.[1]) >= 0);
    if (chromeSkipReason()) assert.match(junit, /<skipped\b/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('caller-side Chrome arguments preserve skip, timeout and tracked callback behavior', async () => {
  let ran = false;
  const [options, callback] = chromeTestArguments({ timeout: 1234 }, () => {
    ran = true;
  });
  assert.equal(options.timeout, 1234);
  assert.equal(options.skip, process.env.CI ? false : chromeSkipReason());
  await callback({ test() {}, skip() {} });
  assert.equal(ran, true);
});
