import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const childPath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  'sliccy-browser-guard-chrome-frames-child.mjs'
);

test('websocket frames route while the tab has an ordinary child frame in another process', async () => {
  await runChromeCase({
    mode: 'ordinary-child',
    maps: 'MAP child.other.test 127.0.0.1',
    expectLog: /websocket:child-ordinary/,
  });
});

test('frames of a socket opened in another process do not reach the subscriber', async () => {
  await runChromeCase({
    mode: 'premise',
    maps: 'MAP seven.sliccy.ai 127.0.0.1, MAP child.other.test 127.0.0.1',
    expectLog: /websocket:premise-other-process/,
  });
});

async function runChromeCase({ mode, maps, expectLog }) {
  const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-guard-chrome-frames-'));
  const logPath = path.join(scratch, 'child.log');
  await writeFile(logPath, '');
  const site = await startSite({ logPath });
  const allowedWs = `ws://127.0.0.1:${site.port}/ws`;
  let chrome;
  try {
    chrome = await launchBrowser([
      `--host-resolver-rules=${maps}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const bin = await fakeSlicc(scratch, chrome.wsUrl);
    const home = path.join(scratch, 'home');
    await mkdir(home, { recursive: true });
    const env = { ...process.env };
    delete env.SLICC_CDP_URL;
    env.PATH = `${bin}${path.delimiter}/nonexistent`;
    env.HOME = home;
    env.GUARD_SITE = site.origin;
    env.GUARD_ALLOWED_WS = allowedWs;
    env.GUARD_LOG = logPath;
    env.GUARD_MODE = mode;
    const child = await runChild(env);
    assert.equal(child.code, 0, `${child.stderr}\n${child.stdout}\n${await readSafe(logPath)}`);
    assert.match(child.stdout, /^OK\n$/u);
    const log = await readFile(logPath, 'utf8');
    assert.match(log, expectLog);
    assert.match(log, /done/);
  } finally {
    await chrome?.close();
    await site.close();
    await rm(scratch, { recursive: true, force: true });
  }
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
          sevenTabs: [{ id: 't', url: 'http://example.test/', title: 'ok' }],
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

function startSite({ logPath } = {}) {
  const sockets = new Set();
  let listenPort = 0;
  const appendFixture = (line) => {
    if (typeof logPath !== 'string' || logPath.length === 0) return;
    writeFile(logPath, `${line}\n`, { flag: 'a' }).catch(() => undefined);
  };
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    const title = url.pathname.slice(1) || 'page';
    let body = title;
    if (url.pathname === '/child-frame') body = 'child-frame';
    if (url.pathname === '/framed') body = 'framed';
    if (url.pathname === '/child-socket') {
      const childWs = `ws://127.0.0.1:${listenPort}/ws?from=child`;
      body = `<script>(()=>{const ws=new WebSocket(${JSON.stringify(childWs)});ws.onopen=()=>ws.send('ping');})();</script>child-socket`;
    }
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(`<!doctype html><html><head><title>${title}</title></head><body>${body}</body></html>`);
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
    const reqUrl = new URL(req.url || '/ws', 'http://127.0.0.1');
    const from = reqUrl.searchParams.get('from') || 'main';
    const n = from === 'child' ? 2 : 1;
    if (from === 'child') appendFixture('fixture:ws-accepted:from=child');
    let answered = false;
    socket.on('data', () => {
      const payload = Buffer.from(JSON.stringify({ kind: 'tick', n, from }));
      const frame = Buffer.alloc(2 + payload.length);
      frame[0] = 0x81;
      frame[1] = payload.length;
      payload.copy(frame, 2);
      try {
        socket.write(frame);
        if (from === 'child' && !answered) {
          answered = true;
          appendFixture('fixture:ws-answered:from=child');
        }
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
      listenPort = typeof address === 'object' && address ? address.port : 0;
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
