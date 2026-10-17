import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../..', import.meta.url));
const pins = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8'));
const version = pins.devDependencies['@ai-ecoverse/slicc-browsers'];

test('the shipped Rust catalog and npm guard vectors use exactly the same release', async () => {
  const lock = await readFile(path.join(root, 'Cargo.lock'), 'utf8');
  const versions = lock
    .split('[[package]]')
    .filter((entry) => /^name = "slicc-browsers"$/m.test(entry))
    .map((entry) => entry.match(/^version = "([^"]+)"$/m)?.[1]);
  assert.deepEqual(versions, [version], 'Cargo.lock must use the npm guard release');
  for (const crate of ['cdp-client', 'playwright-cli']) {
    const manifest = await readFile(path.join(root, 'crates', crate, 'Cargo.toml'), 'utf8');
    const requirements = [...manifest.matchAll(/^slicc-browsers = .*version = "([^"]+)"/gm)];
    assert.equal(requirements.length, 2, crate);
    assert(
      requirements.every((match) => match[1] === `=${version}`),
      `${crate} must pin the exact npm release`
    );
  }
});

test('the Rust catalog protectedOrigins equals the installed npm catalog', async () => {
  const catalog = JSON.parse(
    await readFile(
      fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/catalog.json')),
      'utf8'
    )
  );
  const installed = JSON.parse(
    await readFile(
      fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/package.json')),
      'utf8'
    )
  );
  assert.equal(installed.version, version, 'installed vectors must match the pinned release');
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'catalog_rules'],
    {
      cwd: root,
      stdio: 'inherit',
      timeout: 120000,
    }
  );
  const binary = path.join(
    root,
    'target/debug/examples',
    process.platform === 'win32' ? 'catalog_rules.exe' : 'catalog_rules'
  );
  const rules = JSON.parse(execFileSync(binary, [], { encoding: 'utf8', timeout: 10000 }));
  assert.deepEqual(
    rules,
    catalog.protectedOrigins,
    'the guard and its vectors must use identical protection rules'
  );
});
