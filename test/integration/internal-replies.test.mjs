import { execFileSync } from 'node:child_process';
import test from 'node:test';

const options = { stdio: 'inherit', env: { ...process.env, CARGO_BUILD_JOBS: '4' } };

test('late internal replies never reach callers', { timeout: 120000 }, () => {
  execFileSync(
    'cargo',
    [
      'test',
      '--locked',
      '-j',
      '4',
      '-p',
      'slicc-cdp-client',
      '--lib',
      'replies_',
      '--',
      '--test-threads=1',
    ],
    options
  );
});

test('internal request ownership remains stable', { timeout: 120000 }, () => {
  execFileSync(
    'cargo',
    [
      'test',
      '--locked',
      '-j',
      '4',
      '-p',
      'slicc-cdp-client',
      '--example',
      'guard_bridge',
      'duplicate_caller_requests',
      '--',
      '--test-threads=1',
    ],
    options
  );
});
