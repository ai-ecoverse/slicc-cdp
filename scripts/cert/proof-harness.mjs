import { copyFileSync, mkdirSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { json, root } from './common.mjs';

const { values } = parseArgs({
  options: {
    directory: { type: 'string' },
    tests: { type: 'string' },
    harness: { type: 'string', default: root },
  },
});
const tests = json(values.tests);
const source = values.directory;
const harness = values.harness;
const helpers = path.join(source, 'test/integration');
mkdirSync(helpers, { recursive: true });
for (const file of readdirSync(path.join(harness, 'test/integration')))
  if (file.endsWith('.mjs') && !file.endsWith('.test.mjs'))
    copyFileSync(path.join(harness, 'test/integration', file), path.join(helpers, file));
for (const test of tests) {
  if (test.missing || !['integration', 'guard'].includes(test.source)) continue;
  if (!/^test\/integration\/[a-zA-Z0-9-]+\.test\.mjs$/.test(test.file))
    throw new Error('invalid proof test path');
  copyFileSync(path.join(harness, test.file), path.join(source, test.file));
}
const drivers = path.join(harness, 'test/integration');
for (const file of readdirSync(drivers))
  if (file.endsWith('.rs')) copyFileSync(path.join(drivers, file), path.join(helpers, file));
console.log(
  'installed committed public proof harness without replacing the subject implementation'
);
