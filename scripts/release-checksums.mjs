import { createHash } from 'node:crypto';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const directory = process.argv[2];
if (!directory) throw new Error('release-checksums requires an asset directory');
const files = readdirSync(directory, { withFileTypes: true })
  .filter((file) => file.name !== 'SHA256SUMS')
  .sort((a, b) => a.name.localeCompare(b.name));
if (!files.length || files.some((file) => !file.isFile() || !/^[a-zA-Z0-9.-]+$/.test(file.name)))
  throw new Error('release assets must be regular files with safe names');
const lines = files.map((file) => {
  const sha256 = createHash('sha256')
    .update(readFileSync(path.join(directory, file.name)))
    .digest('hex');
  return `${sha256}  ${file.name}\n`;
});
writeFileSync(path.join(directory, 'SHA256SUMS'), lines.join(''));
console.log(`checksummed ${files.length} release assets`);
