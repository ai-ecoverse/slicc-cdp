import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { rm } from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import test from 'node:test';
import { launchBrowser, NO_KEYCHAIN } from './browser.mjs';

test('the suite Chrome is launched with a mock keychain', async (t) => {
  const saved = {
    SLICC_LOCAL_CHROME: process.env.SLICC_LOCAL_CHROME,
    SLICC_HEADLESS: process.env.SLICC_HEADLESS,
    CHROMIUM_EXECUTABLE: process.env.CHROMIUM_EXECUTABLE,
  };
  process.env.SLICC_LOCAL_CHROME = '1';
  process.env.SLICC_HEADLESS = '1';
  process.env.CHROMIUM_EXECUTABLE = '/stub/chrome';
  const launches = [];
  t.mock.method(childProcess, 'spawn', (_file, args) => {
    launches.push(args);
    throw new Error('stub launch reached');
  });
  syncBuiltinESMExports();
  try {
    await assert.rejects(launchBrowser(), /stub launch reached/);
    await assert.rejects(launchBrowser(['--disable-features=Foo']), /stub launch reached/);
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    t.mock.restoreAll();
    syncBuiltinESMExports();
    for (const args of launches) {
      const profile = args.find((arg) => arg.startsWith('--user-data-dir='));
      if (profile)
        await rm(profile.slice('--user-data-dir='.length), { recursive: true, force: true });
    }
  }
  assert.equal(launches.length, 2);
  assert.deepEqual(NO_KEYCHAIN, ['--password-store=basic', '--use-mock-keychain']);
  for (const args of launches) {
    for (const flag of NO_KEYCHAIN)
      assert.ok(args.includes(flag), `${flag} missing: ${args.join(' ')}`);
  }
});
