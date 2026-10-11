import { readFileSync, writeFileSync } from 'node:fs';

const version = process.argv[2] ?? '';
if (!/^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  process.stderr.write('refusing to stamp a non-semver crate version\n');
  process.exit(1);
}
if (version === '0.0.0') {
  process.stderr.write('refusing to stamp crate version 0.0.0\n');
  process.exit(1);
}
const manifests = [
  'crates/cdp-client/Cargo.toml',
  'crates/playwright-cli/Cargo.toml',
  'crates/curlwright/Cargo.toml',
];
for (const path of manifests) {
  const before = readFileSync(path, 'utf8');
  const after = before.replaceAll('version = "0.0.0"', `version = "${version}"`);
  if (after === before || after.includes('version = "0.0.0"')) {
    process.stderr.write(`${path} still has version 0.0.0\n`);
    process.exit(1);
  }
  if (!after.includes(`version = "${version}"`)) {
    process.stderr.write(`${path} did not receive ${version}\n`);
    process.exit(1);
  }
  writeFileSync(path, after);
}
