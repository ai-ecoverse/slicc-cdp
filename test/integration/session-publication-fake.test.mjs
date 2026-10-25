import { execFileSync } from 'node:child_process';
import test from 'node:test';

test('state publication fake preserves remembered generation metadata while a writer is open', {
  timeout: 120000,
}, () => {
  execFileSync(
    'cargo',
    [
      'test',
      '--locked',
      '-j',
      '4',
      '-p',
      'slicc-cdp',
      'session_atomic_publication',
      '--',
      '--test-threads=1',
    ],
    { stdio: 'inherit', env: { ...process.env, CARGO_BUILD_JOBS: '4' } }
  );
});
