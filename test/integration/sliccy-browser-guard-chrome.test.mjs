import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const childPath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  'sliccy-browser-guard-chrome-child.mjs'
);

test('discovered-mode Chrome regressions for every prior guard bypass', async () => {
  const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-guard-chrome-'));
  const secretFile = path.join(scratch, 'secret-review.txt');
  await writeFile(secretFile, 'FILE-SECRET\n');
  const site = await startSite();
  const port = site.port;
  const sevenOrigin = `http://seven.sliccy.ai:${port}`;
  const slackOrigin = `http://app.slack.com:${port}`;
  const protectedWs = `ws://seven.sliccy.ai:${port}/ws`;
  const allowedWs = `ws://127.0.0.1:${port}/ws`;
  let chrome;
  let raw;
  try {
    chrome = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${port}, MAP app.slack.com 127.0.0.1:${port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await rawSocket(chrome.wsUrl);
    await raw.send('Storage.setCookies', {
      cookies: [
        {
          name: 'seven_session',
          value: 'SEVEN-SECRET',
          domain: '.sliccy.ai',
          path: '/',
          httpOnly: true,
        },
        { name: 'd', value: 'slack-d', domain: 'app.slack.com', path: '/' },
      ],
    });

    const sevenTarget = await openAndWait(raw, `${sevenOrigin}/landing`);
    await pageEval(
      raw,
      sevenTarget,
      `localStorage.setItem('canary','SEVEN-LS'); document.title='seven'`
    );

    await openAndWait(raw, pathToFileUrl(secretFile));
    await openAndWait(raw, 'chrome://settings/');

    const earlyTarget = await openAndWait(raw, `${site.origin}/early`);
    await pageEval(
      raw,
      earlyTarget,
      `new Promise((resolve, reject) => {
        const ws = new WebSocket(${JSON.stringify(protectedWs)});
        const timer = setTimeout(() => reject(new Error('early ws timeout')), 8000);
        ws.onerror = () => { clearTimeout(timer); reject(new Error('early ws error')); };
        ws.onopen = () => {
          window.__guardEarly = ws;
          clearTimeout(timer);
          resolve(true);
        };
      })`,
      true
    );

    const slackTarget = await openAndWait(raw, `${slackOrigin}/client/T1/C1`);
    await pageEval(raw, slackTarget, `localStorage.setItem('localConfig_v2', '{"ok":true}')`);

    const bin = await fakeSlicc(scratch, chrome.wsUrl);
    const home = path.join(scratch, 'home');
    await mkdir(home, { recursive: true });
    const logPath = path.join(scratch, 'child.log');
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    env.PATH = `${bin}${path.delimiter}/nonexistent`;
    env.HOME = home;
    env.GUARD_SITE = site.origin;
    env.GUARD_SEVEN = sevenOrigin;
    env.GUARD_SLACK = `${slackOrigin}/client/T1/C1`;
    env.GUARD_EDGE = `${slackOrigin}/edgeapi/cache/T1/users/info`;
    env.GUARD_PROTECTED_WS = protectedWs;
    env.GUARD_ALLOWED_WS = allowedWs;
    env.GUARD_SECRET_FILE = secretFile;
    env.GUARD_EARLY_TARGET = earlyTarget;
    env.GUARD_SEVEN_TARGET = sevenTarget;
    env.GUARD_LOG = logPath;

    const child = await runChild(env);
    assert.equal(child.code, 0, `${child.stderr}\n${child.stdout}\n${await readSafe(logPath)}`);
    assert.match(child.stdout, /^OK\n$/u);
    const log = await readFile(logPath, 'utf8');
    assert.match(log, /refuse:first-eval-seven/);
    assert.match(log, /fetch:base-no-request/);
    assert.match(log, /websocket:early-dropped/);
    assert.match(log, /slack:positives/);
    assert.match(log, /websocket:allowed/);
    assert.match(log, /done/);
  } finally {
    raw?.close();
    await chrome?.close();
    await site.close();
    await rm(scratch, { recursive: true, force: true });
  }
});

function pathToFileUrl(file) {
  const resolved = path.resolve(file);
  return `file://${resolved.startsWith('/') ? '' : '/'}${resolved.split(path.sep).join('/')}`;
}

async function readSafe(file) {
  try {
    return await readFile(file, 'utf8');
  } catch {
    return '';
  }
}

async function fakeSlicc(scratch, wsUrl) {
  const parsed = new URL(wsUrl);
  const port = Number(parsed.port);
  const bin = path.join(scratch, 'bin');
  await mkdir(bin, { recursive: true });
  const body = JSON.stringify({
    schema: 1,
    minChromium: 116,
    entries: [
      {
        id: `cdp:127.0.0.1:${port}`,
        kind: 'endpoint',
        tier: 1,
        name: 'Chrome',
        endpoint: {
          cdp: `http://127.0.0.1:${port}`,
          webSocketDebuggerUrl: wsUrl,
          name: 'Chrome',
          source: 'flag',
          sevenOpen: true,
          sevenTabs: [{ id: 't', url: 'http://seven.sliccy.ai/', title: 'seven' }],
        },
      },
    ],
  });
  const file = path.join(bin, 'slicc');
  await writeFile(file, `#!/bin/sh\nprintf '%s' '${body}'\n`);
  await chmod(file, 0o755);
  return bin;
}

function runChild(env) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [childPath], { cwd: root, env });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => child.kill('SIGKILL'), 90000);
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
      resolve({ code, stdout, stderr });
    });
  });
}

async function openAndWait(raw, url) {
  const { targetId } = await raw.send('Target.createTarget', { url, background: true });
  const sessionId = await attach(raw, targetId);
  await waitReady(raw, sessionId);
  return targetId;
}

async function attach(raw, targetId) {
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  await raw.send('Page.enable', {}, sessionId).catch(() => undefined);
  await raw.send('Runtime.enable', {}, sessionId).catch(() => undefined);
  return sessionId;
}

async function waitReady(raw, sessionId) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const result = await raw.send(
      'Runtime.evaluate',
      { expression: 'document.readyState', returnByValue: true },
      sessionId
    );
    if (result?.result?.value === 'complete' || result?.result?.value === 'interactive') return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('page never ready');
}

async function pageEval(raw, targetId, expression, awaitPromise = false) {
  const sessionId = await attach(raw, targetId);
  const result = await raw.send(
    'Runtime.evaluate',
    { expression, returnByValue: true, awaitPromise },
    sessionId
  );
  if (result?.exceptionDetails) {
    throw new Error(JSON.stringify(result.exceptionDetails));
  }
  return result?.result?.value;
}

function startSite() {
  const hits = new Map();
  const sockets = new Set();
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    hits.set(url.pathname, (hits.get(url.pathname) ?? 0) + 1);
    if (url.pathname === '/hits') {
      res.writeHead(200, { 'content-type': 'text/plain' });
      res.end(String(hits.get(url.searchParams.get('path')) ?? 0));
      return;
    }
    if (url.pathname === '/probe-canary') {
      res.writeHead(200, { 'content-type': 'text/plain', 'access-control-allow-origin': '*' });
      res.end('probed');
      return;
    }
    if (url.pathname === '/redirect-seven') {
      res.writeHead(302, { location: `http://seven.sliccy.ai:${server.address().port}/landing` });
      res.end('moved');
      return;
    }
    if (url.pathname === '/api/auth.test') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true}');
      return;
    }
    if (url.pathname === '/edgeapi/cache/T1/users/info') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"ok":true,"user":"u"}');
      return;
    }
    const title =
      url.pathname === '/client/T1/C1'
        ? 'Slack'
        : url.pathname === '/landing'
          ? 'seven'
          : url.pathname.slice(1) || 'page';
    let head = '';
    let body = title;
    if (url.pathname === '/base-seven') {
      head = '<base href="https://seven.sliccy.ai/">';
    }
    if (url.pathname === '/framed-seven') {
      body = `<iframe src="http://seven.sliccy.ai:${server.address().port}/landing"></iframe>`;
    }
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(
      `<!doctype html><html><head>${head}<title>${title}</title></head><body>${body}</body></html>`
    );
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
    socket.on('data', () => {
      const payload = Buffer.from('{"kind":"tick","n":1}');
      const canary = Buffer.from('{"kind":"canary","n":42}');
      const host = req.headers.host ?? '';
      const body = host.includes('sliccy.ai') ? canary : payload;
      const frame = Buffer.alloc(2 + body.length);
      frame[0] = 0x81;
      frame[1] = body.length;
      body.copy(frame, 2);
      try {
        socket.write(frame);
      } catch {
        undefined;
      }
    });
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => sockets.delete(socket));
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const listenPort = typeof address === 'object' && address ? address.port : 0;
      resolve({
        port: listenPort,
        origin: `http://127.0.0.1:${listenPort}`,
        close: () =>
          new Promise((done) => {
            for (const socket of sockets) socket.destroy();
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}
