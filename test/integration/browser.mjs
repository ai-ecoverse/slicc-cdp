import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright-core';

export function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

export async function launchBrowser() {
  let port = await freePort();
  if (port === 9222) port = await freePort();
  if (port === 9222) throw new Error('refusing to bind the kernel CDP port');
  const userDataDir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-chrome-'));
  const child = spawn(
    chromium.executablePath(),
    [
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${userDataDir}`,
      '--headless=new',
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-gpu',
      '--disable-dev-shm-usage',
      '--remote-allow-origins=*',
      'about:blank',
    ],
    { stdio: 'ignore' }
  );
  try {
    const version = await waitVersion(port);
    const wsUrl = version.webSocketDebuggerUrl;
    if (typeof wsUrl !== 'string' || !wsUrl.startsWith('ws://') || wsUrl.includes(':9222/')) {
      throw new Error(`unexpected debugger url ${wsUrl}`);
    }
    return {
      port,
      wsUrl,
      async close() {
        child.kill('SIGKILL');
        await rm(userDataDir, { recursive: true, force: true });
      },
    };
  } catch (error) {
    child.kill('SIGKILL');
    await rm(userDataDir, { recursive: true, force: true });
    throw error;
  }
}

async function waitVersion(port) {
  const deadline = Date.now() + 20000;
  let last = 'no response';
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return await response.json();
      last = `status ${response.status}`;
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    await delay(100);
  }
  throw new Error(`debugging port ${port} did not answer /json/version (${last})`);
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export function bridge(wsUrl) {
  return async () => openSocket(wsUrl);
}

async function openSocket(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', () => resolve(), { once: true });
    ws.addEventListener('error', () => reject(new Error('browser websocket failed')), {
      once: true,
    });
  });
  const queued = [];
  let onmessage = null;
  let onclose = null;
  const conn = {
    send(message) {
      ws.send(message);
    },
    close() {
      ws.close();
    },
    get onmessage() {
      return onmessage;
    },
    set onmessage(handler) {
      onmessage = handler;
      if (!handler) return;
      for (const message of queued.splice(0)) handler(message);
    },
    get onclose() {
      return onclose;
    },
    set onclose(handler) {
      onclose = handler;
    },
  };
  ws.addEventListener('message', (event) => {
    const text = socketText(event.data);
    if (conn.onmessage) conn.onmessage(text);
    else queued.push(text);
  });
  ws.addEventListener('close', () => {
    conn.onclose?.('browser socket closed');
  });
  return conn;
}

function socketText(data) {
  if (typeof data === 'string') return data;
  return new TextDecoder().decode(data);
}

export async function frameId(wsUrl, targetId) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', () => resolve(), { once: true });
    ws.addEventListener('error', () => reject(new Error('browser websocket failed')), {
      once: true,
    });
  });
  const pending = new Map();
  let next = 0;
  ws.addEventListener('message', (event) => {
    const message = JSON.parse(socketText(event.data));
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result ?? {});
  });
  const send = (method, params = {}, sessionId) => {
    const id = ++next;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      const body = { id, method, params };
      if (sessionId) body.sessionId = sessionId;
      ws.send(JSON.stringify(body));
    });
  };
  try {
    const attached = await send('Target.attachToTarget', { targetId, flatten: true });
    await send('Page.enable', {}, attached.sessionId);
    const tree = await send('Page.getFrameTree', {}, attached.sessionId);
    const child = tree.frameTree?.childFrames?.[0]?.frame?.id;
    if (!child) throw new Error(`no child frame on ${targetId}`);
    await send('Target.detachFromTarget', { sessionId: attached.sessionId });
    return child;
  } finally {
    ws.close();
  }
}
