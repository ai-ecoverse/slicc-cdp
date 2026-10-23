import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { findBrowser } from '../../src/index.js';
import { freePort, launchManagedBrowser as launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const cli = path.join(root, 'src/cli.js');
const run = promisify(execFile);
const pkg = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8'));
const pinned = pkg.config.chrome;
const { CHROME_PATH: _, ...clean } = process.env;
const sandbox = false;

async function version(port) {
  return (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
}

test('the pinned Chrome for Testing is installed, found and reported by the bin', async () => {
  const installed = await run(process.execPath, [cli, 'install-browser', '--version', pinned], {
    env: clean,
  });
  assert.match(installed.stderr, new RegExp(`Chrome for Testing ${pinned} is already installed`));
  const found = await findBrowser({ env: clean, version: pinned });
  assert.equal(found.version, pinned);
  assert.equal(installed.stdout, `${found.executable}\n`);
  const env = { ...clean, CHROME_PATH: '/elsewhere/chrome' };
  const listed = await run(process.execPath, [cli, 'find-browser', '--version', pinned], { env });
  assert.equal(listed.stdout, `${found.executable}\n`);
  const own = await run(process.execPath, [cli, '--version']);
  assert.equal(own.stdout, `slicc-cdp ${pkg.version}\n`);
});

test('launchBrowser runs it on a throwaway profile on port 0 and removes the profile', async () => {
  const { executable } = await findBrowser({ env: clean, version: pinned });
  const browser = await launchBrowser({ executable, sandbox });
  try {
    assert.match(path.basename(browser.userDataDir), /^slicc-cdp-profile-/);
    assert.ok(browser.userDataDir.startsWith(tmpdir()));
    const info = await version(browser.port);
    assert.equal(info.webSocketDebuggerUrl, browser.wsUrl);
    assert.equal(info.Browser, `Chrome/${pinned}`);
  } finally {
    await browser.close();
  }
  assert.equal(existsSync(browser.userDataDir), false);
});

test('a fixed port is waited on over /json/version, and a busy one is refused', async () => {
  const { executable } = await findBrowser({ env: clean, version: pinned });
  const port = await freePort();
  const browser = await launchBrowser({ executable, port, sandbox });
  try {
    assert.equal(browser.port, port);
    assert.equal((await version(port)).webSocketDebuggerUrl, browser.wsUrl);
    await assert.rejects(
      launchBrowser({ executable, port, sandbox }),
      new RegExp(`a browser already listens on 127.0.0.1:${port}`)
    );
  } finally {
    await browser.close();
  }
});

test('a stale DevToolsActivePort in a given profile is not trusted, and the profile is kept', async () => {
  const { executable } = await findBrowser({ env: clean, version: pinned });
  const userDataDir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-stale-'));
  try {
    await writeFile(path.join(userDataDir, 'DevToolsActivePort'), '1\n/devtools/browser/stale\n');
    const browser = await launchBrowser({ executable, userDataDir, sandbox });
    try {
      assert.notEqual(browser.port, 1);
      assert.equal((await version(browser.port)).webSocketDebuggerUrl, browser.wsUrl);
    } finally {
      await browser.close();
    }
    assert.equal(existsSync(userDataDir), true);
  } finally {
    await rm(userDataDir, { recursive: true, force: true });
  }
});

test('args cannot move the debugging port, address, pipe or profile', async () => {
  const { executable } = await findBrowser({ env: clean, version: pinned });
  for (const arg of [
    '--remote-debugging-port=9222',
    '--remote-debugging-address=0.0.0.0',
    '--remote-debugging-pipe',
    '--user-data-dir=/tmp/elsewhere',
  ]) {
    await assert.rejects(launchBrowser({ executable, args: [arg] }), /args may not set/);
  }
  await assert.rejects(launchBrowser({ executable, port: 9222 }), /refusing to bind port 9222/);
});

test('args are refused in every spelling Chrome parses, and a benign switch still launches', async () => {
  const found = await run(process.execPath, [cli, 'find-browser', '--version', pinned], {
    env: clean,
  });
  const executable = found.stdout.trim();
  const alt = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-alt-'));
  const port = String(await freePort());
  const names = [
    'user-data-dir',
    'remote-debugging-port',
    'remote-debugging-address',
    'remote-debugging-pipe',
    'profile-directory',
  ];
  const value = (name) =>
    name === 'remote-debugging-port' ? port : name === 'remote-debugging-address' ? '0.0.0.0' : alt;
  const mixed = (name) => name.replace(/(^|-)([a-z])/g, (_, dash, c) => dash + c.toUpperCase());
  const cases = [];
  for (const name of names) {
    const v = value(name);
    cases.push(
      { args: [`--${name}=${v}`] },
      { args: [`--${name}`, v] },
      { args: [`-${name}=${v}`] },
      { args: [`-${name}`, v] },
      { args: [`--${mixed(name)}=${v}`] },
      { args: [`-${name.toUpperCase()}=${v}`] },
      { args: [`/${name}=${v}`], platform: 'win32' },
      { args: [`/${mixed(name)}`, v], platform: 'win32' }
    );
  }
  try {
    for (const { args, platform } of cases) {
      await assert.rejects(
        launchBrowser({ executable, args, platform, sandbox, timeout: 5000 }),
        (error) => {
          assert.equal(
            error.message,
            `args may not set ${args[0]}: use the port and userDataDir options`,
            `${platform ?? process.platform} ${args.join(' ')}`
          );
          return true;
        }
      );
    }
    assert.equal(existsSync(path.join(alt, 'DevToolsActivePort')), false);
    await assert.rejects(
      launchBrowser({ executable, args: ['--', `-user-data-dir=${alt}`], sandbox, timeout: 5000 }),
      /did not start: .*Multiple targets are not supported in headless mode/
    );
    assert.equal(existsSync(path.join(alt, 'DevToolsActivePort')), false);
    const browser = await launchBrowser({
      executable,
      args: ['-lang=en-US', '--window-size=800,600', '--user-data-dirs=1'],
      sandbox,
    });
    try {
      assert.notEqual(browser.userDataDir, alt);
      assert.equal((await version(browser.port)).webSocketDebuggerUrl, browser.wsUrl);
      assert.equal(existsSync(path.join(alt, 'DevToolsActivePort')), false);
    } finally {
      await browser.close();
    }
  } finally {
    await rm(alt, { recursive: true, force: true });
  }
});
