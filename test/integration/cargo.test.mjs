import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('the Rust workspace tests pass on the locked dependencies', () => {
  const result = spawnSync('cargo', ['test', '--locked', '--workspace'], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
  });
  const output = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  assert.equal(result.status, 0, output.slice(-20000));
  const passed = [...output.matchAll(/^test result: ok\. (\d+) passed; 0 failed/gm)].reduce(
    (sum, match) => sum + Number(match[1]),
    0
  );
  assert.ok(passed > 0, 'no Rust test ran');
  assert.doesNotMatch(output, /^test result: FAILED/m);
});

test('the Windows binaries type-check on the locked dependencies', () => {
  const result = spawnSync(
    'cargo',
    [
      'check',
      '--locked',
      '--target',
      'x86_64-pc-windows-msvc',
      '-p',
      'slicc-cdp',
      '-p',
      'curlwright',
    ],
    { cwd: root, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 }
  );
  const output = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  assert.equal(result.status, 0, output.slice(-20000));
  assert.doesNotMatch(output, /^error/m);
});
