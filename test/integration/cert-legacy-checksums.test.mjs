import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

test('legacy unavailable checksums are accepted only for an explicitly eligible replay, never for release or missing bytes', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'slicc-legacy-sums-'));
  try {
    const inventory = JSON.parse(
      readFileSync(path.join(root, 'scripts/cert/inventory.json'), 'utf8')
    );
    const lines = inventory.map((file) => {
      const data = Buffer.from(`asset ${file}`);
      writeFileSync(path.join(dir, file), data);
      return `${createHash('sha256').update(data).digest('hex')}  ${file}\n`;
    });
    writeFileSync(
      path.join(dir, 'SHA256SUMS'),
      lines.filter((line) => !line.endsWith('install.sh\n')).join('')
    );
    const check = (...flags) => {
      const child = spawnSync(
        process.execPath,
        [path.join(root, 'scripts/cert/asset-check.mjs'), '--directory', dir, ...flags],
        { encoding: 'utf8' }
      );
      return JSON.parse(child.stdout);
    };
    assert.equal(check().status, 'INCOMPLETE');
    assert.equal(check('--legacy').status, 'INCOMPLETE');
    assert.equal(check('--mode', 'replay').status, 'INCOMPLETE');
    const legacy = check('--mode', 'replay', '--legacy');
    assert.equal(legacy.status, 'PASS');
    assert.equal(legacy.legacy, 'legacy: checksum set unavailable');
    assert.equal(legacy.files.find((file) => file.file === 'install.sh').status, 'LEGACY');
    unlinkSync(path.join(dir, 'install.sh'));
    assert.equal(check('--mode', 'replay', '--legacy').status, 'INCOMPLETE');
    writeFileSync(path.join(dir, 'install.sh'), 'restored legacy installer');
    writeFileSync(path.join(dir, 'LICENSE'), 'tampered');
    assert.equal(check('--mode', 'replay', '--legacy').status, 'FAIL');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
