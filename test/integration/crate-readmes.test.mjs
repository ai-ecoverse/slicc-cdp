import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { CRATES, crateReadme } from '../../scripts/crate-readmes.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const readme = readFileSync(path.join(root, 'README.md'), 'utf8');

test('each published crate gets its README from its section of the root README', () => {
  for (const [name, directory] of CRATES) {
    const manifest = readFileSync(path.join(root, 'crates', directory, 'Cargo.toml'), 'utf8');
    assert.match(manifest, new RegExp(`^name = "${name}"$`, 'm'));
    assert.match(manifest, /^readme = "README\.md"$/m);
    const text = crateReadme(readme, name);
    assert.ok(text.startsWith(`# ${name}\n\n`), text.slice(0, 80));
    assert.doesNotMatch(text, /^### /m);
    assert.ok(text.length > 400, `${name} README is short`);
  }
  assert.match(crateReadme(readme, 'slicc-cdp-client'), /^## Fetch interception$/m);
});

test('the crate READMEs are generated, not tracked', () => {
  const tracked = spawnSync('git', ['ls-files', 'crates/*/README.md'], {
    cwd: root,
    encoding: 'utf8',
  });
  assert.equal(tracked.status, 0, tracked.stderr);
  assert.equal(tracked.stdout, '');
  assert.match(
    readFileSync(path.join(root, 'scripts/publish-crates.sh'), 'utf8'),
    /node scripts\/crate-readmes\.mjs/
  );
});
