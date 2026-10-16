import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { digest, json, save } from './common.mjs';

const { values } = parseArgs({
  options: { directory: { type: 'string' }, out: { type: 'string' } },
});
const expected = json(path.join(import.meta.dirname, 'inventory.json'));
const sums = path.join(values.directory, 'SHA256SUMS');
const checksums = new Map();
if (existsSync(sums)) {
  for (const line of readFileSync(sums, 'utf8').trim().split('\n')) {
    const entry = line.match(/^([a-f0-9]{64}) {2}(?:\.\/)?([a-zA-Z0-9.-]+)$/);
    if (!entry || checksums.has(entry[2])) throw new Error('invalid or duplicate checksum entry');
    checksums.set(entry[2], entry[1]);
  }
}
const files = expected.map((file) => {
  const local = path.join(values.directory, file);
  const actual = existsSync(local) ? digest(local) : null;
  const expectedHash = checksums.get(file);
  return {
    file,
    sha256: actual,
    expected: expectedHash,
    status: !actual || !expectedHash ? 'INCOMPLETE' : actual !== expectedHash ? 'FAIL' : 'PASS',
  };
});
const status = files.some((file) => file.status === 'FAIL')
  ? 'FAIL'
  : files.some((file) => file.status === 'INCOMPLETE')
    ? 'INCOMPLETE'
    : 'PASS';
const result = { status, files };
if (values.out) save(values.out, result);
console.log(JSON.stringify(result));
process.exitCode = status === 'PASS' ? 0 : 1;
