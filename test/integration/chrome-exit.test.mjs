import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser } from '../../src/index.js';
import { launchBrowser as launchSuiteBrowser, rawSocket } from './browser.mjs';
import { chromeTest as it } from './chrome-policy.mjs';
import './watchdog.mjs';

const fatal = 'FATAL:zygote_host_impl_linux.cc(130)] No usable sandbox!';
const windows = process.platform === 'win32';

async function dying() {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-dying-'));
  const file = path.join(dir, 'chrome');
  await writeFile(file, `#!/bin/sh\necho '${fatal}' >&2\nexit 1\n`);
  await chmod(file, 0o755);
  return { dir, file };
}

test('launchBrowser rejects within seconds with the stderr of a Chrome that exits at startup', {
  skip: windows,
}, async () => {
  const { dir, file } = await dying();
  try {
    for (let round = 0; round < 20; round += 1) {
      const started = Date.now();
      await assert.rejects(launchBrowser({ executable: file }), (error) => {
        assert.match(error.message, /did not start/);
        assert.ok(error.message.includes(fatal), error.message);
        return true;
      });
      const took = Date.now() - started;
      assert.ok(took < 5000, `took ${took} ms`);
    }
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('the suite browser fails within seconds with the stderr of a Chrome that exits at startup', {
  skip: windows,
}, async () => {
  const { dir, file } = await dying();
  const browserModule = new URL('./browser.mjs', import.meta.url).href;
  const source = `import assert from 'node:assert/strict';
import { launchBrowser } from ${JSON.stringify(browserModule)};
for (let round = 0; round < 20; round += 1) {
  const started = Date.now();
  await assert.rejects(launchBrowser(), error => {
    assert.match(error.message, /chromium exited code 1/);
    assert.ok(error.message.includes(${JSON.stringify(fatal)}), error.message);
    return true;
  });
  assert.ok(Date.now() - started < 5000);
}
`;
  const env = { ...process.env, CHROMIUM_EXECUTABLE: file, SLICC_LOCAL_CHROME: '1' };
  delete env.NODE_TEST_CONTEXT;
  delete env.NODE_OPTIONS;
  try {
    const result = await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, ['--input-type=module', '-e', source], {
        env,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      let stderr = '';
      child.stderr.on('data', (chunk) => {
        stderr += chunk;
      });
      child.on('error', reject);
      child.on('close', (code) => resolve({ code, stderr }));
    });
    assert.equal(result.code, 0, result.stderr);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

it('the suite browser reports how Chrome died after it started, and nothing while it runs', async () => {
  const browser = await launchSuiteBrowser();
  try {
    assert.equal(browser.died(), null);
    const socket = await rawSocket(browser.wsUrl);
    socket.send('Browser.crash').catch(() => {});
    const deadline = Date.now() + 15000;
    while (!browser.died() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 100));
    socket.close();
    assert.match(browser.died() ?? 'still running', /^chromium exited (signal [A-Z]+|code -?\d+)/);
  } finally {
    await browser.close();
  }
});
