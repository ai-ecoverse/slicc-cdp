import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import browser from '../../packages/sliccy-browser/src/browser.js';
import { connect } from '../../src/index.js';
import { launchBrowser } from './browser.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

test('sliccy:browser over CDP', async () => {
  const named = await import('../../packages/sliccy-browser/src/browser.js');
  assert.equal(typeof named.findTab, 'function');
  assert.equal(typeof named.eval, 'function');
  assert.equal(typeof named.evalAsync, 'function');
  assert.equal(named.default.findTab, named.findTab);
  assert.equal(named.default.eval, named.eval);

  await assert.rejects(() => browser.findTab(), {
    message: 'browser.findTab: query requires `domain` or `urlMatch`',
  });
  await assert.rejects(() => browser.ensureTab(''), TypeError);
  assert.throws(
    () => browser.websocket.on('unused').filter(() => true),
    /filter must be a declarative JSON object/
  );
  assert.throws(
    () => browser.websocket.on('unused').filter('kind'),
    /filter must be a declarative JSON object/
  );
  await assert.rejects(
    () => browser.websocket.on('unused').forward({ sink: 'webhook' }),
    (error) => error instanceof Error && error.code === 'ENOTSUP'
  );
  await assert.rejects(
    () => browser.websocket.on('unused').forward({ sink: 'scoop' }),
    (error) => error instanceof Error && error.code === 'ENOTSUP'
  );
  await assert.rejects(
    () => browser.websocket.on('unused').forward({ sink: 'vfs', path: 'frames.log' }),
    /absolute path/
  );
  await assert.rejects(
    () => browser.websocket.on('unused').forward({ sink: 'nope' }),
    /unknown sink/
  );
  await assert.rejects(
    () => browser.websocket.on('unused').forward({}),
    /forward\.sink is required/
  );

  const site = await startServer();
  const chrome = await launchBrowser();
  const socket = await startSocket();
  const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-browser-'));
  const logged = [];
  const originalLog = console.log;
  console.log = (...args) => {
    logged.push(args.map((part) => String(part)).join(' '));
    originalLog.apply(console, args);
  };
  process.env.SLICC_CDP_URL = chrome.wsUrl;
  let logSink;
  let vfsSink;
  try {
    assert.equal(chrome.port === 9222, false);
    const page = `${site.origin}/page`;
    const tab = await browser.ensureTab(page);
    assert.equal(tab.title, 'Fixture');
    assert.match(tab.url, /\/page$/);

    const byDomain = await browser.findTab({
      domain: '127.0.0.1',
      urlMatch: 'this-does-not-match',
    });
    assert.equal(byDomain?.targetId, tab.targetId);
    const byUrl = await browser.findTab({ urlMatch: '/page$' });
    assert.equal(byUrl?.targetId, tab.targetId);
    assert.equal(await browser.findTab({ domain: 'no-such-host.invalid' }), null);
    await assert.rejects(
      () => browser.findTab({ urlMatch: '(' }),
      /browser\.findTab: invalid url match regex/
    );
    await assert.rejects(
      () => browser.ensureTab(page, { matchUrl: '(' }),
      /browser\.ensureTab: invalid url match regex/
    );

    const sameOrigin = await browser.ensureTab(`${site.origin}/other`);
    assert.equal(sameOrigin.targetId, tab.targetId);
    const other = await browser.ensureTab(`${site.origin}/other`, { matchUrl: '/other$' });
    assert.notEqual(other.targetId, tab.targetId);
    assert.equal(other.title, 'Other');

    assert.equal(await browser.eval(tab, () => document.title), 'Fixture');
    assert.equal(await browser.evalAsync(tab, async () => 2 + 2), 4);
    await assert.rejects(() => browser.eval(tab, 1), /source must be a function or string/);

    assert.equal(await browser.cookie(tab, 'session'), 'from-tab');
    assert.equal(await browser.cookie(tab, 'missing'), null);
    await browser.eval(tab, () => localStorage.setItem('n', '123'));
    assert.equal(await browser.localStorage(tab, 'n'), '123');

    const me = await browser.fetch(tab, `${site.origin}/api/me`);
    assert.equal(me.status, 200);
    assert.equal(me.body, 'session=from-tab');

    const vfsPath = path.join(scratch, 'frames.log');
    logSink = await browser.websocket
      .on(tab)
      .filter({ parseAs: 'json', where: { kind: 'tick' }, project: ['n'] })
      .forward({ sink: 'log' });
    vfsSink = await browser.websocket
      .on(tab, { urlMatch: escapeRegExp(socket.url) })
      .filter({ parseAs: 'json', where: { kind: 'tick' }, project: ['n'] })
      .forward({ sink: 'vfs', path: vfsPath });
    const listed = await browser.websocket.list();
    assert.equal(listed.length, 2);
    const frame = await browser.evalAsync(
      tab,
      `new Promise((resolve, reject) => {
        const ws = new WebSocket(${JSON.stringify(socket.url)});
        const timer = setTimeout(() => reject(new Error('ws timeout')), 8000);
        ws.onerror = () => {
          clearTimeout(timer);
          reject(new Error('ws error'));
        };
        ws.onmessage = (event) => {
          clearTimeout(timer);
          resolve({ url: ws.url, data: String(event.data) });
        };
        ws.onopen = () => ws.send('ping');
      })`
    );
    assert.equal(frame.data, '{"kind":"tick","n":1}');
    assert.equal(
      await browser.eval(tab, 'typeof __sliccWsRouterReport + typeof __sliccWsRouter'),
      'undefinedundefined'
    );
    await waitFor(() => logged.includes('{"n":1}'), `log sink missed ${JSON.stringify(logged)}`);
    const vfsText = await readWhen(vfsPath);
    assert.match(vfsText, /\{"n":1\}\n/);
    assert.equal(await logSink.close(), true);
    assert.equal((await browser.websocket.list()).length, 1);
    assert.equal(await vfsSink.close(), true);

    const raw = await connect();
    let earlySink;
    try {
      const { targetId: early } = await raw.send('Target.createTarget', {
        url: `${site.origin}/other`,
      });
      const rawSession = await raw.attach(early);
      await waitFor(
        async () =>
          (
            await raw.send(
              'Runtime.evaluate',
              { expression: 'document.readyState', returnByValue: true },
              rawSession
            )
          ).result.value === 'complete',
        'the early tab did not load'
      );
      await raw.send(
        'Runtime.evaluate',
        {
          expression: `new Promise((resolve) => { window.early = new WebSocket(${JSON.stringify(socket.url)}); window.early.onopen = () => resolve(true); })`,
          awaitPromise: true,
        },
        rawSession
      );
      const earlyPath = path.join(scratch, 'early.log');
      earlySink = await browser.websocket
        .on(early, { urlMatch: escapeRegExp(socket.url) })
        .filter({ where: { kind: 'tick' }, project: ['n'] })
        .forward({ sink: 'vfs', path: earlyPath });
      await browser.eval(early, () => window.early.send('ping'));
      assert.match(await readWhen(earlyPath), /^\{"n":1\}\n$/);
      assert.equal(await earlySink.close(), true);
    } finally {
      await settle(earlySink?.close());
      await settle(raw.close());
    }

    console.log = originalLog;
    const child = await runJsh(chrome.wsUrl, page);
    assert.equal(child.code, 0, `${child.stderr}\n${child.stdout}`);
    assert.equal(child.stdout, 'done\n');
  } finally {
    console.log = originalLog;
    await settle(logSink?.close());
    await settle(vfsSink?.close());
    delete process.env.SLICC_CDP_URL;
    socket.close();
    await settle(chrome.close());
    await settle(site.close());
    await rm(scratch, { recursive: true, force: true });
  }
});

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(predicate, label) {
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await delay(50);
  }
  throw new Error(label);
}

async function readWhen(file) {
  const deadline = Date.now() + 8000;
  let last = '';
  while (Date.now() < deadline) {
    try {
      last = await readFile(file, 'utf8');
      if (last.includes('{"n":1}')) return last;
    } catch {
      last = '';
    }
    await delay(50);
  }
  throw new Error(`vfs sink did not record a frame (${last})`);
}

function startSocket() {
  const sockets = new Set();
  const server = createServer((_req, res) => {
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.end('ok');
  });
  server.on('upgrade', (req, socket) => {
    const key = req.headers['sec-websocket-key'];
    if (typeof key !== 'string') {
      socket.destroy();
      return;
    }
    const accept = createHash('sha1')
      .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64');
    socket.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
    );
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => socket.destroy());
    let replied = false;
    socket.on('data', () => {
      if (replied) return;
      replied = true;
      const payload = Buffer.from('{"kind":"tick","n":1}');
      socket.write(Buffer.concat([Buffer.from([0x81, payload.length]), payload]));
    });
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({
        url: `ws://127.0.0.1:${port}/`,
        close() {
          for (const socket of sockets) socket.destroy();
          server.close();
        },
      });
    });
  });
}

const jshRunner = `import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import vm from 'node:vm';

const root = process.argv[2];
const scriptPath = process.argv[3];
const baseRequire = createRequire(path.join(root, 'package.json'));

function require(name) {
  if (typeof name === 'string' && name.startsWith('sliccy:')) {
    return baseRequire(\`@ai-ecoverse/sliccy-\${name.slice('sliccy:'.length)}\`);
  }
  return baseRequire(name);
}

const code = readFileSync(scriptPath, 'utf8');
const sandbox = {
  require,
  console,
  process,
  setTimeout,
  clearTimeout,
  queueMicrotask,
  URL,
  Buffer,
};
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
try {
  await vm.runInContext('(async () => {\\n' + code + '\\n})()', sandbox, { filename: scriptPath });
} catch (error) {
  console.error(error);
  process.exit(1);
}
`;

const jshScript = `const browser = require('sliccy:browser');
if (typeof browser.findTab !== 'function') {
  throw new Error('require(sliccy:browser) did not expose findTab');
}
const tab = await browser.ensureTab(process.env.PAGE_URL, { matchUrl: '/page$' });
const title = await browser.eval(tab, 'document.title');
if (title !== 'Fixture') throw new Error('title ' + String(title));
await new Promise((resolve) => setTimeout(resolve, 200));
const again = await browser.eval(tab, 'document.title');
if (again !== 'Fixture') throw new Error('after idle ' + String(again));
console.log('done');
`;

function runJsh(wsUrl, pageUrl) {
  return new Promise((resolve, reject) => {
    mkdtemp(path.join(tmpdir(), 'sliccy-jsh-'))
      .then(async (dir) => {
        const script = path.join(dir, 'script.jsh');
        const runner = path.join(dir, 'run.mjs');
        await writeFile(script, jshScript);
        await writeFile(runner, jshRunner);
        const child = spawn(process.execPath, [runner, root, script], {
          cwd: root,
          env: { ...process.env, SLICC_CDP_URL: wsUrl, PAGE_URL: pageUrl },
        });
        let stdout = '';
        let stderr = '';
        const timer = setTimeout(() => child.kill('SIGKILL'), 20000);
        child.stdout.on('data', (chunk) => {
          stdout += chunk.toString('utf8');
        });
        child.stderr.on('data', (chunk) => {
          stderr += chunk.toString('utf8');
        });
        child.on('error', (error) => {
          clearTimeout(timer);
          reject(error);
        });
        child.on('close', (code) => {
          clearTimeout(timer);
          rm(dir, { recursive: true, force: true }).finally(() => {
            resolve({ code, stdout, stderr });
          });
        });
      })
      .catch(reject);
  });
}

async function settle(value) {
  try {
    await value;
  } catch {
    return undefined;
  }
}
