import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const native = (name) =>
  path.join(root, 'target/debug', `${name}${process.platform === 'win32' ? '.exe' : ''}`);

test('the cert runner executes every public harness-port test with the same preload as package tests', {
  skip: !existsSync(native('playwright-cli')) || !existsSync(native('curlwright')),
}, (t) => {
  const parent = path.join(root, '.cert/harness-proof');
  mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(path.join(parent, 'result-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const control = process.env.SLICC_CERT_RUNNER_SOURCE ?? root;
  const out = path.join(directory, 'suite');
  const result = spawnSync(
    process.execPath,
    [
      path.join(control, 'scripts/cert/run-suite.mjs'),
      '--directory',
      root,
      '--files',
      '["test/integration/harness-ports.test.mjs"]',
      '--out',
      out,
      '--no-retry',
    ],
    {
      cwd: root,
      encoding: 'utf8',
      timeout: 120000,
      env: { ...process.env, SLICC_CDP_PORTS: '9300-9309' },
    }
  );
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const document = JSON.parse(readFileSync(`${out}.json`));
  assert.equal(document.status, 'PASS');
  assert.deepEqual(document.missing, []);
  assert.equal(document.rows.filter((row) => row.type === 'test:pass').length, 3);
});
