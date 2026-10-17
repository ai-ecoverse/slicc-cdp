import { spawnSync } from 'node:child_process';
import { testArguments } from '../test/integration/test-arguments.mjs';

const result = spawnSync(process.execPath, [...testArguments, ...process.argv.slice(2)], {
  stdio: 'inherit',
  env: process.env,
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
