import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const command = fileURLToPath(new URL('../../scripts/release-checksums.mjs', import.meta.url));

test('release checksums cover the installer and reject a tampered installer', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-checksums-'));
  try {
    for (const file of ['install.sh', 'LICENSE', 'playwright-cli-linux-amd64'])
      writeFileSync(path.join(dir, file), `original ${file}\n`);
    const generate = () => spawnSync(process.execPath, [command, dir], { encoding: 'utf8' });
    assert.equal(generate().status, 0);
    const before = readFileSync(path.join(dir, 'SHA256SUMS'), 'utf8');
    assert.match(before, /^[a-f0-9]{64} {2}install\.sh$/m);
    assert.equal(generate().status, 0);
    assert.equal(readFileSync(path.join(dir, 'SHA256SUMS'), 'utf8'), before);
    const verify = () =>
      spawnSync('shasum', ['-a', '256', '-c', 'SHA256SUMS'], { cwd: dir, encoding: 'utf8' });
    assert.equal(verify().status, 0);
    writeFileSync(path.join(dir, 'install.sh'), 'tampered installer\n');
    const rejected = verify();
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stdout, /install\.sh: FAILED/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('checksum generation refuses empty directories and symlinked assets', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-checksums-'));
  try {
    assert.notEqual(spawnSync(process.execPath, [command, dir]).status, 0);
    writeFileSync(path.join(dir, 'LICENSE'), 'licence\n');
    symlinkSync('LICENSE', path.join(dir, 'install.sh'));
    assert.notEqual(spawnSync(process.execPath, [command, dir]).status, 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
