import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { testArguments } from '../test/integration/test-arguments.mjs';
import { tapVerdict } from './gate.mjs';

const dir = mkdtempSync(path.join(tmpdir(), 'slicc-cdp-tap-'));
const tap = path.join(dir, 'report.tap');
try {
  const result = spawnSync(
    process.execPath,
    [
      ...testArguments,
      '--test-reporter=spec',
      '--test-reporter-destination=stdout',
      '--test-reporter=tap',
      `--test-reporter-destination=${tap}`,
      ...process.argv.slice(2),
    ],
    { stdio: 'inherit', env: process.env }
  );
  if (result.error) throw result.error;
  const status = result.status ?? 1;
  const report = existsSync(tap) ? readFileSync(tap, 'utf8') : '';
  const { line, reason } = tapVerdict('integration', report);
  if (status === 0 && line) console.log(line);
  else console.error(`integration: FAIL ${status === 0 ? reason : `exit ${status}`}`);
  process.exitCode = status === 0 && line ? 0 : 1;
} finally {
  rmSync(dir, { recursive: true, force: true });
}
