import assert from 'node:assert/strict';
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser } from '../../src/index.js';
import { launchBrowser as launchSuiteBrowser, rawSocket } from './browser.mjs';
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
  const saved = process.env.CHROMIUM_EXECUTABLE;
  process.env.CHROMIUM_EXECUTABLE = file;
  try {
    for (let round = 0; round < 20; round += 1) {
      const started = Date.now();
      await assert.rejects(launchSuiteBrowser(), (error) => {
        assert.match(error.message, /chromium exited code 1/);
        assert.ok(error.message.includes(fatal), error.message);
        return true;
      });
      const took = Date.now() - started;
      assert.ok(took < 5000, `took ${took} ms`);
    }
  } finally {
    if (saved === undefined) delete process.env.CHROMIUM_EXECUTABLE;
    else process.env.CHROMIUM_EXECUTABLE = saved;
    await rm(dir, { recursive: true, force: true });
  }
});

test('the suite browser reports how Chrome died after it started, and nothing while it runs', async () => {
  const browser = await launchSuiteBrowser();
  try {
    assert.equal(browser.died(), null);
    const socket = await rawSocket(browser.wsUrl);
    socket.send('Browser.crash').catch(() => {});
    const deadline = Date.now() + 15000;
    while (!browser.died() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 100));
    socket.close();
    assert.match(browser.died() ?? 'still running', /^chromium exited (signal [A-Z]+|code -?\d+)/);
    const died = browser.died();
    await browser.close();
    assert.equal(browser.died(), died, 'closing keeps how it died');
  } finally {
    await browser.close();
  }
  const closed = await launchSuiteBrowser();
  await closed.close();
  assert.equal(closed.died(), null, 'a browser the suite closed did not die');
});
