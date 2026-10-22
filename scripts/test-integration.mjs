import { spawnSync } from 'node:child_process';
import { prepareChromeReport } from '../test/integration/chrome-report.mjs';
import { testArguments } from '../test/integration/test-arguments.mjs';

const files = ['test/integration/*.test.mjs', ...process.argv.slice(2)];
prepareChromeReport(process.env, [process.execPath, ...files]);
const result = spawnSync(process.execPath, [...testArguments, ...files], {
  stdio: 'inherit',
  env: process.env,
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
