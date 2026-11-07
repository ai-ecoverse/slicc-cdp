import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { chmod, mkdir, mkdtemp, readFile, rm, utimes, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import {
  acceptLoopbackWs,
  resolveCdpEndpoint,
  sliccProfileRoots,
} from '../../packages/sliccy-browser/src/discover.js';
import './watchdog.mjs';

const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-discovery-'));
after(() => rm(scratch, { recursive: true, force: true }));

function schema1(entries) {
  return JSON.stringify({ schema: 1, minChromium: 116, entries });
}

function endpoint({ port, id, tier, sevenOpen, userDataDir, profile, host = '127.0.0.1' }) {
  return {
    id: id ?? `cdp:${host}:${port}`,
    kind: 'endpoint',
    tier,
    name: 'Chrome',
    version: '154.0.0.0',
    endpoint: {
      cdp: `http://${host}:${port}`,
      webSocketDebuggerUrl: `ws://${host}:${port}/devtools/browser/${port}`,
      name: 'Chrome',
      version: '154.0.0.0',
      source: 'devtools-active-port',
      sevenOpen,
      sevenTabs: sevenOpen ? [{ id: 't', url: 'https://x.sliccy.ai/', title: 'seven' }] : [],
      ...(userDataDir ? { userDataDir } : {}),
      ...(profile ? { profile } : {}),
    },
  };
}

async function fakeSlicc(script) {
  const bin = path.join(scratch, `bin-${Math.random().toString(36).slice(2)}`);
  await mkdir(bin);
  const file = path.join(bin, 'slicc');
  await writeFile(file, script);
  await chmod(file, 0o755);
  return bin;
}

async function listen(handler) {
  const server = createServer((req, res) => handler(req, res, server));
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return {
    port,
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve()))
      ),
  };
}

function versionOk(port, res, wsHost = '127.0.0.1') {
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(
    JSON.stringify({
      Browser: 'Chrome/154.0.0.0',
      webSocketDebuggerUrl: `ws://${wsHost}:${port}/devtools/browser/${port}`,
    })
  );
}

async function writeActive(home, relative, port, mtimeMs) {
  const dir = path.join(home, ...relative);
  await mkdir(dir, { recursive: true });
  const file = path.join(dir, 'DevToolsActivePort');
  await writeFile(file, `${port}\n/devtools/browser/${port}\n`);
  if (mtimeMs !== undefined) await utimes(file, mtimeMs / 1000, mtimeMs / 1000);
  return file;
}

function contactedTracker() {
  const contacted = [];
  const fetchImpl = async (url, init) => {
    contacted.push(String(url));
    return globalThis.fetch(url, init);
  };
  return { contacted, fetchImpl };
}

test('acceptLoopbackWs rejects non-ws, non-loopback, and port mismatches', () => {
  assert.equal(acceptLoopbackWs('ws://127.0.0.1:9333/devtools/browser/x', 9333), true);
  assert.equal(acceptLoopbackWs('ws://localhost:9333/x', 9333), true);
  assert.equal(acceptLoopbackWs('ws://[::1]:9333/x', 9333), true);
  assert.equal(acceptLoopbackWs('wss://127.0.0.1:9333/x', 9333), false);
  assert.equal(acceptLoopbackWs('ws://10.0.0.2:9333/x', 9333), false);
  assert.equal(acceptLoopbackWs('ws://127.0.0.1:9334/x', 9333), false);
});

test('SLICC_CDP_URL wins and skips slicc / DevToolsActivePort / 9222', async () => {
  const { contacted, fetchImpl } = contactedTracker();
  let spawned = false;
  const result = await resolveCdpEndpoint({
    env: { ...process.env, SLICC_CDP_URL: 'ws://127.0.0.1:9555/devtools/browser/explicit' },
    fetch: fetchImpl,
    spawn: () => {
      spawned = true;
      throw new Error('slicc must not run');
    },
    profileRoots: [path.join(scratch, 'unused-profiles')],
  });
  assert.equal(result.url, 'ws://127.0.0.1:9555/devtools/browser/explicit');
  assert.equal(spawned, false);
  assert.deepEqual(contacted, []);
});

test('slicc browsers --json prefers sevenOpen, then slicc userDataDir, then lowest tier', async () => {
  const seven = endpoint({
    port: 9401,
    tier: 2,
    sevenOpen: true,
    userDataDir: '/tmp/other',
  });
  const profile = endpoint({
    port: 9402,
    tier: 2,
    sevenOpen: false,
    userDataDir: '/Users/me/Library/Application Support/slicc/seven-profiles/chrome',
  });
  const plainHigh = endpoint({ port: 9403, tier: 1, sevenOpen: false, userDataDir: '/tmp/plain' });
  const plainLow = endpoint({ port: 9404, tier: 2, sevenOpen: false, userDataDir: '/tmp/plain2' });
  const remote = endpoint({
    port: 9405,
    tier: 1,
    sevenOpen: true,
    host: '10.0.0.9',
    userDataDir: '/tmp/remote',
  });
  const installed = {
    id: 'chrome',
    kind: 'installed',
    tier: 3,
    name: 'Google Chrome',
    installed: {
      id: 'chrome',
      name: 'Google Chrome',
      engine: 'chromium',
      path: '/Applications/Google Chrome.app',
      executable: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      debugSupport: 'supported',
    },
  };

  const binSeven = await fakeSlicc(
    `#!/bin/sh\nprintf '%s' '${schema1([plainHigh, seven, profile, remote, installed])}'\n`
  );
  const pickedSeven = await resolveCdpEndpoint({
    env: {
      ...process.env,
      PATH: `${binSeven}${path.delimiter}${process.env.PATH}`,
      SLICC_CDP_URL: '',
    },
    profileRoots: [],
    fetch: async () => {
      throw new Error('fetch must not run when CLI picks');
    },
  });
  assert.equal(pickedSeven.url, 'ws://127.0.0.1:9401/devtools/browser/9401');

  const binProfile = await fakeSlicc(
    `#!/bin/sh\nprintf '%s' '${schema1([plainHigh, profile, plainLow, remote, installed])}'\n`
  );
  const pickedProfile = await resolveCdpEndpoint({
    env: {
      ...process.env,
      PATH: `${binProfile}${path.delimiter}${process.env.PATH}`,
      SLICC_CDP_URL: '',
    },
    profileRoots: [],
    fetch: async () => {
      throw new Error('fetch must not run when CLI picks');
    },
  });
  assert.equal(pickedProfile.url, 'ws://127.0.0.1:9402/devtools/browser/9402');

  const binTier = await fakeSlicc(
    `#!/bin/sh\nprintf '%s' '${schema1([plainLow, plainHigh, remote])}'\n`
  );
  const pickedTier = await resolveCdpEndpoint({
    env: {
      ...process.env,
      PATH: `${binTier}${path.delimiter}${process.env.PATH}`,
      SLICC_CDP_URL: '',
    },
    profileRoots: [],
    fetch: async () => {
      throw new Error('fetch must not run when CLI picks');
    },
  });
  assert.equal(pickedTier.url, 'ws://127.0.0.1:9403/devtools/browser/9403');
});

test('slicc missing, non-zero, invalid JSON, and timeout fall through', async () => {
  const live = await listen((req, res, server) => {
    if (req.url !== '/json/version') {
      res.writeHead(404);
      res.end();
      return;
    }
    versionOk(server.address().port, res);
  });
  after(() => live.close());

  const home = path.join(scratch, 'fallthrough-home');
  await writeActive(
    home,
    ['Library', 'Application Support', 'slicc', 'seven-profiles', 'chrome'],
    live.port,
    Date.now()
  );

  async function through(bin) {
    return resolveCdpEndpoint({
      env: {
        ...process.env,
        HOME: home,
        PATH: bin ? `${bin}${path.delimiter}/nonexistent` : '/nonexistent',
        SLICC_CDP_URL: '',
      },
      platform: 'darwin',
      fetch: globalThis.fetch,
    });
  }

  const missing = await through(undefined);
  assert.equal(missing.url, `ws://127.0.0.1:${live.port}/devtools/browser/${live.port}`);
  assert.match(missing.tried.join('\n'), /slicc browsers --json: slicc not on PATH/);

  const fromExit = await resolveCdpEndpoint({
    env: {
      ...process.env,
      HOME: home,
      PATH: '/nonexistent',
      SLICC_CDP_URL: '',
    },
    platform: 'darwin',
    spawn: () => {
      const child = new EventEmitter();
      child.stdout = new EventEmitter();
      child.kill = () => undefined;
      queueMicrotask(() => child.emit('close', 3));
      return child;
    },
    fetch: globalThis.fetch,
  });
  assert.equal(fromExit.url, `ws://127.0.0.1:${live.port}/devtools/browser/${live.port}`);
  assert.match(fromExit.tried.join('\n'), /slicc browsers --json: exited 3/);

  const badJson = await fakeSlicc('#!/bin/sh\nprintf not-json\n');
  const fromJson = await through(badJson);
  assert.equal(fromJson.url, `ws://127.0.0.1:${live.port}/devtools/browser/${live.port}`);
  assert.match(fromJson.tried.join('\n'), /slicc browsers --json: invalid JSON/);

  const slow = await fakeSlicc('#!/bin/sh\n/bin/sleep 5\n');
  const fromTimeout = await resolveCdpEndpoint({
    env: {
      ...process.env,
      HOME: home,
      PATH: `${slow}${path.delimiter}/nonexistent`,
      SLICC_CDP_URL: '',
    },
    platform: 'darwin',
    cliTimeoutMs: 200,
  });
  assert.equal(fromTimeout.url, `ws://127.0.0.1:${live.port}/devtools/browser/${live.port}`);
  assert.match(fromTimeout.tried.join('\n'), /slicc browsers --json: timed out/);
});

test('DevToolsActivePort picks the live newest file and ignores stale or non-loopback answers', async () => {
  const live = await listen((req, res, server) => {
    if (req.url !== '/json/version') {
      res.writeHead(404);
      res.end();
      return;
    }
    versionOk(server.address().port, res);
  });
  const noWs = await listen((req, res) => {
    if (req.url !== '/json/version') {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ Browser: 'Chrome/154.0.0.0' }));
  });
  const remoteWs = await listen((req, res, server) => {
    if (req.url !== '/json/version') {
      res.writeHead(404);
      res.end();
      return;
    }
    versionOk(server.address().port, res, '8.8.8.8');
  });
  after(async () => {
    await live.close();
    await noWs.close();
    await remoteWs.close();
  });

  const home = path.join(scratch, 'active-home');
  const now = Date.now();
  await writeActive(
    home,
    ['Library', 'Application Support', 'slicc', 'seven-profiles', 'stale'],
    1,
    now + 30_000
  );
  await writeActive(
    home,
    ['Library', 'Application Support', 'slicc', 'seven-profiles', 'nows'],
    noWs.port,
    now + 20_000
  );
  await writeActive(
    home,
    ['Library', 'Application Support', 'slicc', 'seven-profiles', 'remote'],
    remoteWs.port,
    now + 10_000
  );
  await writeActive(
    home,
    ['Library', 'Application Support', 'slicc', 'seven-profiles', 'live'],
    live.port,
    now
  );
  await writeActive(
    home,
    ['Library', 'Application Support', 'Slicc', 'profiles', 'browser-coding-agent-chrome'],
    live.port,
    now - 5_000
  );

  const { contacted, fetchImpl } = contactedTracker();
  const result = await resolveCdpEndpoint({
    env: { ...process.env, HOME: home, PATH: '/nonexistent', SLICC_CDP_URL: '' },
    platform: 'darwin',
    fetch: fetchImpl,
  });
  assert.equal(result.url, `ws://127.0.0.1:${live.port}/devtools/browser/${live.port}`);
  assert.match(result.tried.join('\n'), /DevToolsActivePort: chose a live loopback port/);
  for (const url of contacted) {
    assert.match(url, /^http:\/\/127\.0\.0\.1:\d+\/json\/version$/u);
    const port = Number(new URL(url).port);
    assert.ok([1, noWs.port, remoteWs.port, live.port].includes(port));
  }
  assert.equal(contacted.includes(`http://127.0.0.1:${live.port}/json/version`), true);
});

test('127.0.0.1:9222 is last when CLI and DevToolsActivePort fail', async () => {
  const contacted = [];
  const result = await resolveCdpEndpoint({
    env: {
      ...process.env,
      PATH: '/nonexistent',
      SLICC_CDP_URL: '',
      HOME: path.join(scratch, 'empty-home'),
    },
    platform: 'darwin',
    profileRoots: [path.join(scratch, 'empty-profiles')],
    fetch: async (url) => {
      contacted.push(String(url));
      assert.equal(String(url), 'http://127.0.0.1:9222/json/version');
      return {
        ok: true,
        async text() {
          return JSON.stringify({
            webSocketDebuggerUrl: 'ws://127.0.0.1:9222/devtools/browser/fallback',
          });
        },
      };
    },
  });
  assert.equal(result.url, 'ws://127.0.0.1:9222/devtools/browser/fallback');
  assert.deepEqual(contacted, ['http://127.0.0.1:9222/json/version']);
  assert.match(result.tried.join('\n'), /127\.0\.0\.1:9222: answered/);
});

test('error lists every step when nothing is found', async () => {
  await assert.rejects(
    () =>
      resolveCdpEndpoint({
        env: {
          ...process.env,
          PATH: '/nonexistent',
          SLICC_CDP_URL: '',
          HOME: path.join(scratch, 'gone'),
        },
        platform: 'darwin',
        profileRoots: [path.join(scratch, 'gone-profiles')],
        fetch: async (url) => {
          assert.equal(String(url), 'http://127.0.0.1:9222/json/version');
          throw new Error('refused');
        },
      }),
    (error) => {
      assert.match(error.message, /no debugger is connected/);
      assert.match(error.message, /SLICC_CDP_URL: not set/);
      assert.match(error.message, /slicc browsers --json: slicc not on PATH/);
      assert.match(error.message, /DevToolsActivePort:/);
      assert.match(error.message, /127\.0\.0\.1:9222:/);
      return true;
    }
  );
});

test('sliccProfileRoots covers seven-profiles, legacy profiles, and six profiles', () => {
  const darwin = sliccProfileRoots({
    platform: 'darwin',
    env: { HOME: '/Users/me' },
  });
  assert.deepEqual(darwin, [
    '/Users/me/Library/Application Support/slicc/seven-profiles',
    '/Users/me/Library/Application Support/slicc/profiles',
    '/Users/me/Library/Application Support/Slicc/profiles',
  ]);
  const linux = sliccProfileRoots({
    platform: 'linux',
    env: {
      HOME: '/home/me',
      XDG_CONFIG_HOME: '/home/me/.config',
      XDG_STATE_HOME: '/home/me/.local/state',
    },
  });
  assert.deepEqual(linux, [
    '/home/me/.config/slicc/seven-profiles',
    '/home/me/.config/slicc/profiles',
    '/home/me/.local/state/slicc/profiles',
  ]);
  const windows = sliccProfileRoots({
    platform: 'win32',
    env: {
      HOME: 'C:\\Users\\me',
      APPDATA: 'C:\\Users\\me\\AppData\\Roaming',
      LOCALAPPDATA: 'C:\\Users\\me\\AppData\\Local',
    },
  });
  assert.equal(windows[0].endsWith(`${path.sep}slicc${path.sep}seven-profiles`), true);
  assert.equal(windows[1].endsWith(`${path.sep}slicc${path.sep}profiles`), true);
  assert.equal(windows[2].endsWith(`${path.sep}Slicc${path.sep}profiles`), true);
});

test('spawn uses fixed args without a shell', async () => {
  let seen;
  const bin = await fakeSlicc(
    `#!/bin/sh\nprintf '%s' '${schema1([endpoint({ port: 9410, tier: 2, sevenOpen: true })])}'\n`
  );
  await resolveCdpEndpoint({
    env: { ...process.env, PATH: `${bin}${path.delimiter}/nonexistent`, SLICC_CDP_URL: '' },
    profileRoots: [],
    spawn: (command, args, options) => {
      seen = { command, args, shell: options.shell };
      return spawn(command, args, options);
    },
    fetch: async () => {
      throw new Error('unused');
    },
  });
  assert.deepEqual(seen, { command: 'slicc', args: ['browsers', '--json'], shell: false });
});

test('loadSpawn failure falls through like missing slicc (kernel without child_process)', async () => {
  const result = await resolveCdpEndpoint({
    env: {
      ...process.env,
      PATH: '/nonexistent',
      SLICC_CDP_URL: '',
      HOME: path.join(scratch, 'loadspawn-home'),
    },
    platform: 'darwin',
    profileRoots: [path.join(scratch, 'loadspawn-profiles')],
    loadSpawn: async () => {
      throw new Error('Unavailable in this environment');
    },
    fetch: async (url) => {
      assert.equal(String(url), 'http://127.0.0.1:9222/json/version');
      return {
        ok: true,
        async text() {
          return JSON.stringify({
            webSocketDebuggerUrl: 'ws://127.0.0.1:9222/devtools/browser/fallback',
          });
        },
      };
    },
  });
  assert.equal(result.url, 'ws://127.0.0.1:9222/devtools/browser/fallback');
  assert.match(
    result.tried.join('\n'),
    /slicc browsers --json: child_process unavailable \(Unavailable in this environment\)/
  );
});

test('browser.js and discover.js do not statically import node:child_process', async () => {
  const browserSrc = await readFile(
    new URL('../../packages/sliccy-browser/src/browser.js', import.meta.url),
    'utf8'
  );
  const discoverSrc = await readFile(
    new URL('../../packages/sliccy-browser/src/discover.js', import.meta.url),
    'utf8'
  );
  assert.equal(/from ['"]node:child_process['"]/u.test(browserSrc), false);
  assert.equal(/from ['"]node:child_process['"]/u.test(discoverSrc), false);
  assert.equal(/import\(['"]node:child_process['"]\)/u.test(discoverSrc), true);
  const mod = await import('../../packages/sliccy-browser/src/browser.js');
  assert.equal(typeof mod.default.findTab, 'function');
  assert.equal(typeof mod.version, 'string');
});
