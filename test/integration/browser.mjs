import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import './ports.mjs';

const { findBrowser } = await import(
  process.env.SLICC_FIXTURE_BROWSER_API ?? '../../src/browser.js'
);

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

export async function launchBrowser(extraArgs = [], options = {}) {
  let port = options.port ?? (await freePort());
  while (port >= 9222 && port <= 9229) port = await freePort();
  const userDataDir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-chrome-'));
  let chromeLog = '';
  const child = spawn(
    process.env.CHROMIUM_EXECUTABLE || (await findBrowser())?.executable,
    [
      `--remote-debugging-port=${port}`,
      '--remote-debugging-address=127.0.0.1',
      `--user-data-dir=${userDataDir}`,
      '--headless=new',
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-gpu',
      '--disable-dev-shm-usage',
      '--no-sandbox',
      '--disable-setuid-sandbox',
      '--remote-allow-origins=*',
      ...extraArgs,
      'about:blank',
    ],
    { stdio: ['ignore', 'ignore', 'pipe'], detached: process.platform !== 'win32' }
  );
  const exited = new Promise((resolve) => child.once('exit', resolve));
  child.stderr.on('data', (chunk) => {
    chromeLog = (chromeLog + chunk.toString('utf8')).slice(-4000);
  });
  const stop = async () => {
    const running = child.exitCode === null && !child.signalCode;
    try {
      if (process.platform === 'win32') child.kill('SIGKILL');
      else process.kill(-child.pid, 'SIGKILL');
    } catch {
      if (running) child.kill('SIGKILL');
    }
    if (running) {
      let timer;
      await Promise.race([exited, new Promise((resolve) => (timer = setTimeout(resolve, 5000)))]);
      clearTimeout(timer);
    }
    child.stderr.destroy();
  };
  try {
    const version = await waitVersion(port, () => chromeStatus(child, chromeLog));
    const wsUrl = version.webSocketDebuggerUrl;
    if (typeof wsUrl !== 'string' || !wsUrl.startsWith('ws://') || /:922[2-9]\//.test(wsUrl)) {
      throw new Error(`unexpected debugger url ${wsUrl}`);
    }
    return {
      port,
      wsUrl,
      async close() {
        await stop();
        await rm(userDataDir, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await stop();
    await rm(userDataDir, { recursive: true, force: true });
    throw error;
  }
}

function chromeStatus(child, log) {
  const tail = log.trim().slice(-2000);
  if (child.exitCode !== null || child.signalCode) {
    const how = child.signalCode ? `signal ${child.signalCode}` : `code ${child.exitCode}`;
    return `chromium exited ${how}${tail ? `\n${tail}` : ''}`;
  }
  return tail;
}

async function waitVersion(port, status) {
  const deadline = Date.now() + 20000;
  let last = 'no response';
  while (Date.now() < deadline) {
    const early = status();
    if (early.startsWith('chromium exited')) throw new Error(early);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return await response.json();
      last = `status ${response.status}`;
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    await delay(100);
  }
  const detail = status();
  throw new Error(
    `debugging port ${port} did not answer /json/version (${last})${detail ? `\n${detail}` : ''}`
  );
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

export async function rawSocket(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', () => resolve(), { once: true });
    ws.addEventListener('error', () => reject(new Error('browser websocket failed')), {
      once: true,
    });
  });
  const pending = new Map();
  const watchers = new Set();
  let next = 0;
  ws.addEventListener('message', (event) => {
    const message = JSON.parse(socketText(event.data));
    if (message.method) {
      for (const watcher of watchers) watcher(message);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result ?? {});
  });
  const send = (method, params = {}, sessionId) => {
    const id = ++next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 30000);
      pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      const body = { id, method, params };
      if (sessionId) body.sessionId = sessionId;
      ws.send(JSON.stringify(body));
    });
  };
  const targetUrl = async (targetId) => {
    const { targetInfo } = await send('Target.getTargetInfo', { targetId });
    return targetInfo.url;
  };
  const event = (method, sessionId, matches = () => true) => {
    let watcher;
    const seen = new Promise((resolve) => {
      watcher = (message) => {
        if (message.method !== method || message.sessionId !== sessionId) return;
        if (!matches(message.params ?? {})) return;
        watchers.delete(watcher);
        resolve(message.params ?? {});
      };
      watchers.add(watcher);
    });
    return { seen, cancel: () => watchers.delete(watcher) };
  };
  return {
    send,
    targetUrl,
    event,
    async open(url) {
      const { targetId } = await send('Target.createTarget', { url, background: true });
      const deadline = Date.now() + 15000;
      while (!(await targetUrl(targetId)).startsWith(url.split('?')[0])) {
        if (Date.now() > deadline) throw new Error(`${url} never opened`);
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      return targetId;
    },
    close: () => ws.close(),
  };
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
