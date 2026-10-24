import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createServer as createTlsServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const run = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeCli = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const nativeCurl = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'
);
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const release = (version, sha256) => ({
  version,
  sha256,
  url: `https://github.com/ai-ecoverse/slicc-extension/releases/download/v${version}/slicc-extension.zip`,
});
const EXTENSIONS = [
  release('7.9.5', '526763b3d575f8d2301c253ffb9ec8eb62c073b2440cf63cae5a11200f338dff'),
  release('7.7.5', '040827f96f005d4035bd59440d056768dc07e5b6bd323afc3c11c0251ea3669d'),
];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const lapse = (ms) => new Promise((resolve) => setTimeout(resolve, ms).unref());

async function unpacked(extension) {
  const dir = path.join(root, 'node_modules', '.cache', `slicc-extension-${extension.version}`);
  const out = path.join(dir, 'unpacked');
  if (existsSync(path.join(out, 'manifest.json'))) return out;
  const response = await fetch(extension.url);
  if (!response.ok) throw new Error(`${extension.url}: HTTP ${response.status}`);
  const zip = Buffer.from(await response.arrayBuffer());
  const sum = createHash('sha256').update(zip).digest('hex');
  if (sum !== extension.sha256)
    throw new Error(`${extension.url}: sha256 ${sum}, want ${extension.sha256}`);
  await rm(dir, { recursive: true, force: true });
  await mkdir(out, { recursive: true });
  await writeFile(path.join(dir, 'slicc-extension.zip'), zip);
  await run('unzip', ['-q', path.join(dir, 'slicc-extension.zip'), '-d', out]);
  return out;
}

async function certificate() {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-seven-tls-'));
  const key = path.join(dir, 'key.pem');
  const cert = path.join(dir, 'cert.pem');
  try {
    await run('openssl', [
      'req',
      '-x509',
      '-newkey',
      'rsa:2048',
      '-nodes',
      '-days',
      '2',
      '-subj',
      '/CN=seven.sliccy.ai',
      '-keyout',
      key,
      '-out',
      cert,
    ]);
    return { key: await readFile(key), cert: await readFile(cert) };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function decode(buffer) {
  const out = [];
  let offset = 0;
  while (buffer.length - offset >= 2) {
    const second = buffer[offset + 1];
    let length = second & 0x7f;
    let head = 2;
    if (length === 126) {
      if (buffer.length - offset < 4) break;
      length = buffer.readUInt16BE(offset + 2);
      head = 4;
    } else if (length === 127) {
      if (buffer.length - offset < 10) break;
      length = Number(buffer.readBigUInt64BE(offset + 2));
      head = 10;
    }
    const mask = second & 0x80 ? 4 : 0;
    const total = head + mask + length;
    if (buffer.length - offset < total) break;
    const payload = Buffer.from(buffer.subarray(offset + head + mask, offset + total));
    if (mask) {
      const key = buffer.subarray(offset + head, offset + head + 4);
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= key[i % 4];
    }
    out.push({ opcode: buffer[offset] & 0x0f, payload });
    offset += total;
  }
  return { out, rest: buffer.subarray(offset) };
}

function encode(payload, opcode = 1) {
  let head;
  if (payload.length < 126) head = Buffer.from([0x80 | opcode, payload.length]);
  else if (payload.length < 65536) {
    head = Buffer.from([0x80 | opcode, 126, 0, 0]);
    head.writeUInt16BE(payload.length, 2);
  } else {
    head = Buffer.alloc(10);
    head[0] = 0x80 | opcode;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  return Buffer.concat([head, payload]);
}

function peer(req, socket, onMessage, onClose) {
  const accept = createHash('sha1')
    .update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest('base64');
  socket.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
  );
  let buffer = Buffer.alloc(0);
  let fragments = [];
  const send = (message) => {
    if (!socket.destroyed) socket.write(encode(Buffer.from(JSON.stringify(message))));
  };
  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    const { out, rest } = decode(buffer);
    buffer = Buffer.from(rest);
    for (const frame of out) {
      if (frame.opcode === 8) {
        socket.end(encode(Buffer.alloc(0), 8));
        continue;
      }
      if (frame.opcode === 9) {
        socket.write(encode(frame.payload, 10));
        continue;
      }
      if (frame.opcode === 10) continue;
      fragments.push(frame.payload);
      const text = Buffer.concat(fragments).toString('utf8');
      fragments = [];
      try {
        onMessage(JSON.parse(text));
      } catch {}
    }
  });
  socket.on('error', () => socket.destroy());
  socket.on('close', onClose);
  return { send, close: () => socket.destroy() };
}

async function extensionBridge() {
  const traffic = [];
  const sockets = new Set();
  const connections = new Set();
  const pending = new Map();
  let next = 0;
  let seven;
  let sevenReady;
  const connected = new Promise((resolve) => {
    sevenReady = resolve;
  });
  const toSeven = (message, waiting) => {
    next += 1;
    if (waiting) pending.set(next, waiting);
    traffic.push({ at: Date.now(), dir: 'command', ...message });
    seven?.send({ ...message, id: next });
  };
  const server = createServer((_req, res) => {
    res.writeHead(404);
    res.end();
  });
  server.on('upgrade', (req, socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    const url = new URL(req.url, 'http://bridge');
    if (url.pathname === '/seven') {
      seven = peer(
        req,
        socket,
        (message) => {
          if (typeof message.id === 'number') {
            const waiting = pending.get(message.id);
            pending.delete(message.id);
            traffic.push({ at: Date.now(), dir: 'reply', method: waiting?.method, ...message });
            waiting?.done(message);
            return;
          }
          traffic.push({ at: Date.now(), dir: 'event', ...message });
          for (const connection of connections) connection.event(message);
        },
        () => {
          seven = undefined;
        }
      );
      sevenReady(url.searchParams.get('extension') === '1');
      return;
    }
    const sessions = new Map();
    let closed = false;
    const attached = (sessionId, by) => {
      const count = (sessions.get(sessionId) ?? 0) + by;
      if (count > 0) sessions.set(sessionId, count);
      else sessions.delete(sessionId);
    };
    const connection = {
      event(message) {
        if (closed || !sessions.has(message.sessionId)) return;
        if (message.method === 'Target.detachedFromTarget') sessions.delete(message.sessionId);
        if (message.method === 'Target.attachedToTarget' && message.params?.sessionId)
          attached(message.params.sessionId, 1);
        client.send(message);
      },
    };
    const client = peer(
      req,
      socket,
      async (message) => {
        await connected;
        const { id, method, params } = message;
        if (!seven) {
          client.send({ id, error: { code: -32000, message: 'seven is gone' } });
          return;
        }
        toSeven(message, {
          method,
          done(reply) {
            if (reply.error) {
              if (!closed) client.send({ id, error: reply.error });
              return;
            }
            const result = reply.result ?? {};
            if (method === 'Target.attachToTarget' && result.sessionId) {
              if (closed) {
                toSeven({
                  method: 'Target.detachFromTarget',
                  params: { sessionId: result.sessionId },
                });
                return;
              }
              attached(result.sessionId, 1);
            }
            if (method === 'Target.detachFromTarget' && sessions.has(params?.sessionId))
              attached(params.sessionId, -1);
            if (!closed) client.send({ id, result });
          },
        });
      },
      () => {
        closed = true;
        connections.delete(connection);
        for (const [sessionId, count] of sessions)
          for (let n = 0; n < count; n += 1)
            toSeven({ method: 'Target.detachFromTarget', params: { sessionId } });
        sessions.clear();
      }
    );
    connections.add(connection);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return {
    port,
    traffic,
    connected,
    wsUrl: `ws://127.0.0.1:${port}/devtools/browser/seven`,
    close: () => {
      for (const socket of sockets) socket.destroy();
      server.closeAllConnections?.();
      return new Promise((resolve) => server.close(resolve));
    },
  };
}

async function browserCall(wsUrl, method, params = {}) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true });
    ws.addEventListener('error', reject, { once: true });
  });
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method} timed out`)), 15000);
      ws.addEventListener('message', ({ data }) => {
        const message = JSON.parse(data);
        if (message.id !== 1) return;
        clearTimeout(timer);
        if (message.error) reject(new Error(`${method}: ${message.error.message}`));
        else resolve(message.result);
      });
      ws.send(JSON.stringify({ id: 1, method, params }));
    });
  } finally {
    ws.close();
  }
}

function sevenPage(bridgePort) {
  return `<!doctype html><title>seven</title><script>
(async () => {
  for (let i = 0; i < 200 && !globalThis.sliccExtension; i += 1) await new Promise((r) => setTimeout(r, 100));
  const cdp = globalThis.sliccExtension?.cdp;
  const socket = new WebSocket('ws://127.0.0.1:${bridgePort}/seven?extension=' + (cdp ? '1' : '0'));
  if (!cdp) return;
  socket.addEventListener('open', () => cdp.on((message) => socket.send(JSON.stringify(message))));
  socket.addEventListener('message', async ({ data }) => {
    const { id, method, params, sessionId } = JSON.parse(data);
    try {
      const result = await cdp.send(method, params, sessionId);
      socket.send(JSON.stringify({ id, result }));
    } catch (error) {
      socket.send(JSON.stringify({ id, error: { code: -32000, message: String(error?.message ?? error) } }));
    }
  });
})();
</script>`;
}

function summary(traffic) {
  const keep = (entry) =>
    /^(Target\.|Page\.(getFrameTree|getNavigationHistory|navigate|frameNavigated|frameStartedNavigating)|Fetch\.(requestPaused|failRequest))/.test(
      entry.method ?? ''
    );
  return traffic
    .filter(keep)
    .slice(-120)
    .map((entry) => {
      const text = JSON.stringify({ ...entry, at: undefined });
      return text.length > 700 ? `${text.slice(0, 700)}…` : text;
    })
    .join('\n');
}

async function scenario(extension, transport) {
  const extensionRoot = await unpacked(extension);
  const manifest = JSON.parse(await readFile(path.join(extensionRoot, 'manifest.json'), 'utf8'));
  assert.equal(manifest.version, extension.version);
  const hits = [];
  const ordinary = createServer((req, res) => {
    const url = new URL(req.url, `http://${req.headers.host}`);
    hits.push(`${req.headers.host}${url.pathname}`);
    const pages = {
      '/hop.html': `<!doctype html><title>hop</title><meta http-equiv="refresh" content="0;url=${vault}">`,
      '/after.html':
        '<!doctype html><title>after</title><h1>Drivable again</h1><a href="#again">again</a>',
      '/vault': '<!doctype html><title>VAULT-MARK</title><h1>VAULT-BODY-MARK</h1>',
    };
    res.writeHead(pages[url.pathname] ? 200 : 404, { 'content-type': 'text/html' });
    res.end(pages[url.pathname] ?? 'missing');
  });
  await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
  const ordinaryPort = ordinary.address().port;
  const vault = `http://vault.sliccy.ai:${ordinaryPort}/vault`;
  const local = (file) => `http://localhost:${ordinaryPort}${file}`;
  const bridge = await extensionBridge();
  const tls = createTlsServer(await certificate(), (_req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(sevenPage(bridge.port));
  });
  await new Promise((resolve) => tls.listen(0, '127.0.0.1', resolve));
  const browser = await launchBrowser([
    `--load-extension=${extensionRoot}`,
    `--disable-extensions-except=${extensionRoot}`,
    '--ignore-certificate-errors',
    '--disable-features=LocalNetworkAccessChecks,LocalNetworkAccessChecksWebSockets',
    `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tls.address().port}, MAP vault.sliccy.ai 127.0.0.1`,
  ]);
  let runner;
  const log = [];
  try {
    await browserCall(browser.wsUrl, 'Target.createTarget', { url: 'https://seven.sliccy.ai/' });
    const loaded = await Promise.race([bridge.connected, lapse(30000).then(() => null)]);
    if (loaded !== true) {
      const { targetInfos } = await browserCall(browser.wsUrl, 'Target.getTargets');
      assert.fail(
        `slicc-extension ${extension.version} gave seven no CDP (${loaded}): ${JSON.stringify(targetInfos.map(({ type, url }) => ({ type, url })))}`
      );
    }
    runner =
      transport === 'native'
        ? nativeRunner(nativeCli, nativeCurl, bridge.wsUrl)
        : await kernelRunner(
            path.join(wasmDir, 'playwright-cli.wasm'),
            path.join(wasmDir, 'curlwright.wasm'),
            bridge.wsUrl
          );
    const cwd = await runner.fresh();
    const pw = async (...args) => {
      const result = await runner.playwright(args, cwd);
      log.push(
        `$ playwright-cli ${args.join(' ')}\n${result.stdout}${result.stderr}exit ${result.code}`
      );
      return result;
    };
    const context = () =>
      `${log.join('\n')}\n--- CDP through slicc-extension ${extension.version} ---\n${summary(bridge.traffic)}`;
    const opened = await pw('open', 'about:blank');
    assert.equal(opened.code, 0, context());
    await pw('goto', local('/hop.html'));
    await delay(2000);
    assert.ok(
      !hits.some((hit) => hit.startsWith('vault.sliccy.ai')),
      `the vault was served after a hop: ${hits.join(' ')}\n${context()}`
    );
    const after = await pw('goto', local('/after.html'));
    assert.doesNotMatch(`${after.stdout}${after.stderr}`, /slicc: refused/, context());
    assert.equal(after.code, 0, context());
    const title = await pw('eval', 'document.title');
    assert.match(title.stdout, /after/, context());
    const snapshot = await pw('snapshot');
    assert.match(snapshot.stdout, /heading "Drivable again"/, context());
    const ref = /link "again" \[ref=(e\d+)\]/.exec(snapshot.stdout)?.[1];
    assert.ok(ref, context());
    const click = await pw('click', ref);
    assert.equal(click.code, 0, context());
    const hash = await pw('eval', 'location.hash');
    assert.match(hash.stdout, /#again/, context());
    const back = await pw('goto', vault);
    assert.notEqual(back.code, 0, context());
    assert.match(`${back.stdout}${back.stderr}`, /slicc: refused/, context());
    assert.ok(
      !hits.some((hit) => hit.startsWith('vault.sliccy.ai')),
      `the vault was served: ${hits.join(' ')}\n${context()}`
    );
  } finally {
    const dir = path.join(root, 'artifacts', 'real-extension');
    await mkdir(dir, { recursive: true }).catch(() => {});
    await writeFile(
      path.join(dir, `${extension.version}-${transport}.log`),
      `${log.join('\n')}\n${bridge.traffic.map((entry) => JSON.stringify(entry)).join('\n')}\n`
    ).catch(() => {});
    await runner?.close();
    await browser.close();
    await bridge.close();
    tls.closeAllConnections?.();
    await new Promise((resolve) => tls.close(resolve));
    ordinary.closeAllConnections?.();
    await new Promise((resolve) => ordinary.close(resolve));
  }
}

for (const extension of EXTENSIONS) {
  for (const transport of ['native', 'wasi']) {
    test(
      `real slicc-extension ${extension.version} over ${transport}: after a blocked meta-refresh hop toward the vault, a goto to an ordinary page works, the tab is drivable again, and the vault stays refused`,
      {
        timeout: 300000,
      },
      () => scenario(extension, transport)
    );
  }
}
