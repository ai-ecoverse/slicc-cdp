import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { findBrowser } from '../../src/browser.js';
import { launchBrowser, rawSocket } from './browser.mjs';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const playwright = exe('playwright-cli');
const curlwright = exe('curlwright');
const windows = process.platform === 'win32';
const refusal = "slicc: refused: target is on seven's origin\n";

function bindable(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once('error', () => resolve(false));
    server.listen(port, '127.0.0.1', () => server.close(() => resolve(true)));
  });
}

async function portRange(count) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const base = 20000 + Math.floor(Math.random() * 40000);
    const ports = Array.from({ length: count }, (_, index) => base + index);
    let free = true;
    for (const port of ports) {
      if (!(await bindable(port))) {
        free = false;
        break;
      }
    }
    if (free) return ports;
  }
  throw new Error('no free port range');
}

function listen(handler, port = 0) {
  const server = createServer(handler);
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () =>
      resolve({
        port: server.address().port,
        origin: `http://127.0.0.1:${server.address().port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      })
    );
  });
}

function page(title) {
  return `<!doctype html><title>${title}</title><p>${title}</p>`;
}

function site() {
  return listen((req, res) => {
    const url = new URL(req.url || '/', 'http://fixture');
    if (url.pathname === '/api') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"from":"api"}');
      return;
    }
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(page(url.pathname.slice(1) || 'root'));
  });
}

async function home() {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-ports-'));
  return realpath(dir);
}

function hostEnv(dir, ports, extra = {}) {
  const env = { ...process.env };
  delete env.SLICC_CDP_URL;
  delete env.PLAYWRIGHT_CLI_SESSION;
  delete env.SNAP_NAME;
  return {
    ...env,
    HOME: dir,
    USERPROFILE: dir,
    APPDATA: path.join(dir, 'AppData', 'Roaming'),
    LOCALAPPDATA: path.join(dir, 'AppData', 'Local'),
    XDG_CONFIG_HOME: path.join(dir, '.config'),
    XDG_STATE_HOME: path.join(dir, '.state'),
    SLICC_CDP_PORTS: `${ports[0]}-${ports[ports.length - 1]}`,
    CHROME_PATH: process.execPath,
    ...extra,
  };
}

function configDir(dir) {
  if (process.platform === 'darwin') return path.join(dir, 'Library', 'Application Support');
  if (windows) return path.join(dir, 'AppData', 'Roaming');
  return path.join(dir, '.config');
}

async function session(dir, cwd) {
  const key = createHash('sha256')
    .update(await realpath(cwd))
    .digest('hex');
  const file = path.join(configDir(dir), 'slicc', 'playwright-cli', 'sessions', `${key}.json`);
  return JSON.parse(await readFile(file, 'utf8'));
}

function run(file, args, cwd, env, timeoutMs = 60000) {
  return new Promise((resolve, reject) => {
    const child = spawn(file, args, { cwd, env, detached: !windows });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error(`timed out: ${path.basename(file)} ${args.join(' ')}`));
    }, timeoutMs);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

function tabLines(stdout) {
  return stdout
    .trim()
    .split('\n')
    .filter((line) => /^\d+: \[/.test(line));
}

test('a browser on a catalog debug port is attached and driven without a seven tab', {
  timeout: 240_000,
}, async (t) => {
  const ports = await portRange(3);
  const fixture = await site();
  const other = await site();
  const dir = await home();
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-ports-cwd-'));
  const browser = await launchBrowser(
    ['--host-resolver-rules=MAP vault.sliccy.ai 127.0.0.1', '--no-proxy-server'],
    { port: ports[1] }
  );
  const raw = await rawSocket(browser.wsUrl);
  const env = hostEnv(dir, ports);
  const pw = (args, where = cwd) => run(playwright, args, where, env);
  try {
    const personal = await raw.open(`${fixture.origin}/personal`);
    const vault = await raw.open(`http://vault.sliccy.ai:${fixture.port}/vault`);
    await raw.open(`${other.origin}/only`);
    raw.close();

    await t.test('open attaches to the probed browser and saves a probe session', async () => {
      const opened = await pw(['open', `${fixture.origin}/mine`]);
      assert.equal(opened.code, 0, opened.stderr);
      const saved = await session(dir, cwd);
      assert.equal(saved.provenance, 'probe');
      assert.equal(saved.endpoint, `http://127.0.0.1:${ports[1]}/json/version`);
      const targets = await (await fetch(`http://127.0.0.1:${ports[1]}/json/list`)).json();
      assert.ok(targets.some((target) => target.url === `${fixture.origin}/mine`));
    });

    await t.test('tab-list shows every tab but the protected one', async () => {
      const listed = await pw(['tab-list']);
      assert.equal(listed.code, 0, listed.stderr);
      const lines = tabLines(listed.stdout);
      assert.ok(
        lines.some((line) => line.includes(`[${personal}] ${fixture.origin}/personal`)),
        listed.stdout
      );
      assert.ok(
        lines.some((line) => line.includes(`${fixture.origin}/mine`)),
        listed.stdout
      );
      assert.ok(!listed.stdout.includes(vault), listed.stdout);
      assert.ok(!listed.stdout.includes('vault.sliccy.ai'), listed.stdout);
    });

    await t.test('a tab this tool did not open is selected and driven', async () => {
      const listed = await pw(['tab-list']);
      const index = tabLines(listed.stdout).findIndex((line) => line.includes(`[${personal}]`));
      assert.ok(index >= 0, listed.stdout);
      const selected = await pw(['tab-select', String(index + 1)]);
      assert.equal(selected.code, 0, selected.stderr);
      assert.match(selected.stdout, new RegExp(`targetId: ${personal}`));
      const title = await pw(['eval', 'document.title']);
      assert.equal(title.code, 0, title.stderr);
      assert.match(title.stdout, /personal/);
      const tabbed = await pw(['eval', 'location.pathname', '--tab', personal]);
      assert.equal(tabbed.code, 0, tabbed.stderr);
      assert.match(tabbed.stdout, /\/personal/);
    });

    await t.test('the protected tab stays refused', async () => {
      const refused = await pw(['eval', 'document.title', '--tab', vault]);
      assert.equal(refused.code, 1);
      assert.equal(refused.stderr, refusal);
      assert.ok(!refused.stdout.includes('vault'), refused.stdout);
    });

    await t.test('list reports the probed browser with all its unprotected tabs', async () => {
      const listed = await pw(['list', '--json']);
      assert.equal(listed.code, 0, listed.stderr);
      const report = JSON.parse(listed.stdout);
      const entry = report.entries.find(
        (item) => item.endpoint?.cdp === `http://127.0.0.1:${ports[1]}`
      );
      assert.ok(entry, listed.stdout);
      assert.equal(entry.endpoint.source, 'probe');
      assert.deepEqual(entry.endpoint.sevenTabs, []);
      const urls = entry.endpoint.tabs.map((tab) => tab.url);
      assert.ok(urls.includes(`${fixture.origin}/personal`), listed.stdout);
      assert.ok(urls.includes(`${fixture.origin}/mine`), listed.stdout);
      assert.ok(!listed.stdout.includes('vault.sliccy.ai'), listed.stdout);
    });

    await t.test('curlwright finds the probed browser without --cdp', async () => {
      const fresh = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-ports-curl-'));
      try {
        const fetched = await run(curlwright, ['-s', `${other.origin}/api`], fresh, env);
        assert.equal(fetched.code, 0, fetched.stderr);
        assert.equal(fetched.stdout, '{"from":"api"}');
      } finally {
        await rm(fresh, { recursive: true, force: true });
      }
    });
  } finally {
    raw.close();
    await browser.close();
    await fixture.close();
    await other.close();
    await rm(dir, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
});

test('discover probes a page on a catalog debug port', { timeout: 120_000 }, async () => {
  const seen = [];
  const proxy = await listen((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1:9222');
    seen.push(url.href);
    if (url.host !== '127.0.0.1:9222') {
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('unexpected host');
      return;
    }
    if (url.pathname === '/') {
      res.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        link: '</catalog>; rel="api-catalog"',
      });
      res.end(page('Debug port page'));
      return;
    }
    if (url.pathname === '/.well-known/ard.json') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ name: 'Port ARD', description: 'served on 9222' }));
      return;
    }
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not found');
  });
  const browser = await launchBrowser([
    `--proxy-server=http://127.0.0.1:${proxy.port}`,
    '--proxy-bypass-list=<-loopback>',
  ]);
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-ports-discover-'));
  const dir = await home();
  try {
    const result = await run(
      playwright,
      ['--cdp', browser.wsUrl, 'discover', '--json', '--once', 'http://127.0.0.1:9222/'],
      cwd,
      hostEnv(dir, await portRange(1))
    );
    assert.equal(result.code, 0, result.stderr);
    assert.ok(seen.includes('http://127.0.0.1:9222/'), seen.join('\n'));
    const events = result.stdout
      .trim()
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    const link = events.find((e) => e.via === 'link-header' && e.standard === 'api-catalog');
    assert.ok(link, result.stdout);
    assert.equal(link.origin, 'http://127.0.0.1:9222');
    const ard = events.find((e) => e.via === 'well-known' && e.standard === 'ard');
    assert.ok(ard, result.stdout);
    assert.equal(ard.status, 'valid');
    assert.equal(ard.summary.name, 'Port ARD');
    assert.ok(seen.includes('http://127.0.0.1:9222/.well-known/ard.json'), seen.join('\n'));
  } finally {
    await browser.close();
    await proxy.close();
    await rm(cwd, { recursive: true, force: true });
    await rm(dir, { recursive: true, force: true });
  }
});

async function chromium(dir) {
  const { executable } = await findBrowser();
  if (windows) return executable;
  const file = path.join(dir, 'chrome');
  await writeFile(
    file,
    `#!/bin/sh\nexec ${JSON.stringify(executable)} --no-sandbox --disable-gpu "$@"\n`
  );
  await chmod(file, 0o755);
  return file;
}

async function launchedPort(dir, cwd) {
  try {
    return new URL((await session(dir, cwd)).endpoint).port;
  } catch {
    const file = path.join(
      configDir(dir),
      'slicc',
      'seven-profiles',
      'chrome',
      'DevToolsActivePort'
    );
    return (await readFile(file, 'utf8')).split(/\r?\n/)[0];
  }
}

async function closeLaunched(dir, cwd) {
  const port = Number(await launchedPort(dir, cwd));
  if (port >= 9222 && port <= 9229) return;
  const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  const raw = await rawSocket(version.webSocketDebuggerUrl);
  await raw.send('Browser.close').catch(() => {});
  raw.close();
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      await fetch(`http://127.0.0.1:${port}/json/version`);
    } catch {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

test('open launches on the first free catalog debug port', { timeout: 120_000 }, async () => {
  const ports = await portRange(3);
  const busy = await listen((req, res) => {
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not a browser');
  }, ports[0]);
  const fixture = await site();
  const dir = await home();
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-ports-launch-'));
  try {
    const env = hostEnv(dir, ports, { CHROME_PATH: await chromium(dir) });
    const opened = await run(playwright, ['open', `${fixture.origin}/launched`], cwd, env, 90000);
    assert.equal(opened.code, 0, opened.stderr);
    const version = await (await fetch(`http://127.0.0.1:${ports[1]}/json/version`)).json();
    assert.match(version.webSocketDebuggerUrl, new RegExp(`^ws://127\\.0\\.0\\.1:${ports[1]}/`));
    const targets = await (await fetch(`http://127.0.0.1:${ports[1]}/json/list`)).json();
    assert.ok(targets.some((target) => target.url === `${fixture.origin}/launched`));
    const saved = await session(dir, cwd);
    assert.equal(saved.endpoint, `http://127.0.0.1:${ports[1]}/json/version`);
    const listed = await run(playwright, ['tab-list'], cwd, env);
    assert.equal(listed.code, 0, listed.stderr);
    assert.match(listed.stdout, /\/launched/);
  } finally {
    await closeLaunched(dir, cwd).catch(() => {});
    await busy.close();
    await fixture.close();
    await rm(cwd, { recursive: true, force: true });
    await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  }
});
