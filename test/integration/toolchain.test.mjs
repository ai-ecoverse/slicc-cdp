import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const root = new URL('../../', import.meta.url);

test('Cargo uses the pinned Rust toolchain and resolves the committed lockfile', async () => {
  const toolchain = await readFile(new URL('rust-toolchain.toml', root), 'utf8');
  const version = toolchain.match(/^channel = "(\d+\.\d+\.\d+)"$/m)?.[1];
  assert.ok(version, 'an exact toolchain is required');
  const before = await readFile(new URL('Cargo.lock', root), 'utf8');
  const rust = execFileSync('rustc', ['--version'], { cwd: root, encoding: 'utf8' });
  assert.ok(rust.startsWith(`rustc ${version} `), rust);
  const metadata = JSON.parse(
    execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1'], {
      cwd: root,
      encoding: 'utf8',
    })
  );
  assert.deepEqual(metadata.packages.map((pkg) => pkg.name).sort(), [
    'curlwright',
    'slicc-cdp',
    'slicc-cdp-client',
    'slicc-discovery',
  ]);
  assert.equal(await readFile(new URL('Cargo.lock', root), 'utf8'), before);
});
