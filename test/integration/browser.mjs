import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { headlessByDefault, modeFlags, screenFor } from '@ai-ecoverse/slicc-shared-web/harness';
import { requireChrome } from './chrome-policy.mjs';
import { chromeLaunched } from './chrome-report.mjs';

const workerDeadline = Date.now() + Number(process.env.SLICC_ROW_TIMEOUT_MS ?? 605000);
const cleanupGrace = (deadlineAt = workerDeadline - 5000) =>
  Math.max(30000, deadlineAt - Date.now());
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

export async function waitExit(exited, timeoutMs = cleanupGrace()) {
  let timer;
  try {
    await Promise.race([
      exited,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error('chromium cleanup timed out')), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

const QUIET = [
  '--disable-background-networking',
  '--disable-component-update',
  '--disable-sync',
  '--disable-domain-reliability',
  '--disable-client-side-phishing-detection',
  '--disable-breakpad',
  '--disable-search-engine-choice-screen',
  '--metrics-recording-only',
  '--no-pings',
  '--no-service-autorun',
];
const QUIET_FEATURES = [
  'Translate',
  'MediaRouter',
  'DialMediaRouteProvider',
  'GlobalMediaControls',
  'OptimizationHints',
  'OptimizationHintsFetching',
  'OptimizationGuideModelDownloading',
  'OptimizationTargetPrediction',
  'LensOverlay',
  'AutofillServerCommunication',
  'CertificateTransparencyComponentUpdater',
  'InterestFeedContentSuggestions',
  'ZeroSuggestPrefetching',
];
const OFFLINE = ['google.com', '*.google.com', '*.googleapis.com', '*.gstatic.com', '*.gvt1.com'];

function quiet(extraArgs) {
  const merged = (prefix, own, separator) =>
    prefix +
    [
      ...extraArgs.filter((arg) => arg.startsWith(prefix)).map((arg) => arg.slice(prefix.length)),
      ...own,
    ]
      .filter(Boolean)
      .join(separator);
  const features = '--disable-features=';
  const rules = '--host-resolver-rules=';
  return [
    ...extraArgs.filter((arg) => !arg.startsWith(features) && !arg.startsWith(rules)),
    merged(features, QUIET_FEATURES, ','),
    merged(
      rules,
      OFFLINE.map((host) => `MAP ${host} ~NOTFOUND`),
      ', '
    ),
  ];
}

export async function launchBrowser(extraArgs = [], options = {}) {
  requireChrome();
  options.signal?.throwIfAborted();
  let port = options.port ?? 0;
  if (port >= 9222 && port <= 9229) throw new Error(`refusing debugging port ${port}`);
  const headless = options.headless ?? headlessByDefault(process.env);
  const executable = process.env.CHROMIUM_EXECUTABLE || (await findBrowser())?.executable;
  const userDataDir = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-chrome-'));
  let screen;
  try {
    screen = await screenFor(!headless, process.env);
  } catch (error) {
    await rm(userDataDir, { recursive: true, force: true });
    throw error;
  }
  let chromeLog = '';
  const child = spawn(
    executable,
    [
      `--remote-debugging-port=${port}`,
      '--remote-debugging-address=127.0.0.1',
      `--user-data-dir=${userDataDir}`,
      ...modeFlags(headless),
      '--no-first-run',
      '--no-default-browser-check',
      ...QUIET,
      ...(headless ? ['--disable-gpu'] : ['--window-size=1280,800']),
      '--disable-dev-shm-usage',
      '--no-sandbox',
      '--disable-setuid-sandbox',
      '--remote-allow-origins=*',
      ...quiet(extraArgs),
      'about:blank',
    ],
    {
      stdio: ['ignore', 'ignore', 'pipe'],
      detached: process.platform !== 'win32',
      env: screen.env,
    }
  );
  const exited = new Promise((resolve) => child.once('exit', resolve));
  child.stderr.on('data', (chunk) => {
    chromeLog = (chromeLog + chunk.toString('utf8')).slice(-4000);
  });
  let stopping;
  let diedFirst = null;
  const stop = () =>
    (stopping ??= (async () => {
      const running = child.exitCode === null && !child.signalCode;
      if (!running) diedFirst = chromeStatus(child, chromeLog);
      const sendSignal = (signal) => {
        try {
          if (process.platform === 'win32') child.kill(signal);
          else process.kill(-child.pid, signal);
        } catch {
          if (child.exitCode === null && !child.signalCode) child.kill(signal);
        }
      };
      try {
        if (running) {
          sendSignal('SIGTERM');
          try {
            await waitExit(exited, cleanupGrace((options.deadlineAt ?? workerDeadline) + 30000));
          } catch (error) {
            sendSignal('SIGKILL');
            process.stderr.write(
              'HARNESS-AVAILABILITY {"phase":"browser-close","message":"grace expired; forced SIGKILL"}\n'
            );
            await waitExit(exited, cleanupGrace((options.deadlineAt ?? workerDeadline) + 30000));
            throw error;
          }
        }
      } finally {
        child.stderr.destroy();
      }
    })());
  const abort = () => {
    void stop().catch(() => {});
  };
  options.signal?.addEventListener('abort', abort, { once: true });
  try {
    options.signal?.throwIfAborted();
    const status = () => chromeStatus(child, chromeLog);
    if (!port)
      port = await activePort(
        userDataDir,
        status,
        options.signal,
        options.deadlineAt ?? workerDeadline
      );
    const version = await waitVersion(
      port,
      status,
      options.signal,
      options.deadlineAt ?? workerDeadline
    );
    const wsUrl = version.webSocketDebuggerUrl;
    if (typeof wsUrl !== 'string' || !wsUrl.startsWith('ws://') || /:922[2-9]\//.test(wsUrl)) {
      throw new Error(`unexpected debugger url ${wsUrl}`);
    }
    chromeLaunched();
    return {
      port,
      wsUrl,
      died() {
        if (stopping) return diedFirst;
        return child.exitCode !== null || child.signalCode ? chromeStatus(child, chromeLog) : null;
      },
      headless,
      async close() {
        options.signal?.removeEventListener('abort', abort);
        try {
          await stop();
        } finally {
          await screen.close();
          await rm(userDataDir, { recursive: true, force: true });
        }
      },
    };
  } catch (error) {
    options.signal?.removeEventListener('abort', abort);
    try {
      await stop();
    } finally {
      await screen.close();
      await rm(userDataDir, { recursive: true, force: true });
    }
    throw error;
  }
}

export async function launchManagedBrowser(options) {
  requireChrome();
  const browser = await import('../../src/browser.js');
  const launched = await browser.launchBrowser(options);
  chromeLaunched();
  return launched;
}

function chromeStatus(child, log) {
  const tail = log.trim().slice(-2000);
  if (child.exitCode !== null || child.signalCode) {
    const how = child.signalCode ? `signal ${child.signalCode}` : `code ${child.exitCode}`;
    return `chromium exited ${how}${tail ? `\n${tail}` : ''}`;
  }
  return tail;
}

async function activePort(userDataDir, status, signal, deadlineAt) {
  const deadline = deadlineAt;
  while (Date.now() < deadline) {
    signal?.throwIfAborted();
    const early = status();
    if (early.startsWith('chromium exited')) throw new Error(early);
    const text = await readFile(path.join(userDataDir, 'DevToolsActivePort'), 'utf8').catch(
      () => ''
    );
    const port = Number(text.split('\n')[0]);
    if (Number.isInteger(port) && port > 0) {
      if (port >= 9222 && port <= 9229) throw new Error(`refusing debugging port ${port}`);
      return port;
    }
    await delay(100);
  }
  const detail = status();
  throw new Error(`chromium wrote no DevToolsActivePort${detail ? `\n${detail}` : ''}`);
}

async function waitVersion(port, status, signal, deadlineAt) {
  const deadline = deadlineAt;
  let last = 'no response';
  while (Date.now() < deadline) {
    signal?.throwIfAborted();
    const early = status();
    if (early.startsWith('chromium exited')) throw new Error(early);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`, {
        signal: AbortSignal.any([
          AbortSignal.timeout(Math.max(1, deadline - Date.now())),
          ...(signal ? [signal] : []),
        ]),
      });
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
