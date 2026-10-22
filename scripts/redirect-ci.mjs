import { spawnSync } from 'node:child_process';
import { integrationArguments } from './integration-selection.mjs';

try {
  const files = await integrationArguments('redirects');
  const outcome = spawnSync(
    process.execPath,
    ['scripts/test-integration.mjs', '--test-concurrency=1', ...files],
    { stdio: 'inherit', env: process.env }
  );
  if (outcome.error) throw outcome.error;
  process.exitCode = outcome.status === 0 ? 0 : 1;
  console.log(
    `redirect-regressions: ${outcome.status === 0 ? 'PASS' : 'FAIL'} ${files.length} files`
  );
} catch (error) {
  console.error(`redirect-regressions: FAIL ${error.message}`);
  process.exitCode = 1;
}
