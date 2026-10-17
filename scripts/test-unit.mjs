import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { testArguments } from '../test/integration/test-arguments.mjs';

const root = path.resolve(import.meta.dirname, '..');
const result = spawnSync(
  process.execPath,
  [
    ...testArguments,
    ...(process.argv.length > 2 ? process.argv.slice(2) : ['test/unit/**/*.test.mjs']),
  ],
  { cwd: root, stdio: 'inherit', env: process.env }
);
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
