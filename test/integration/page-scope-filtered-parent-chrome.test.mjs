import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer as http } from 'node:http';
import { createServer as https } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { observer } from './guard-client.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const body = `<!doctype html><title>Ordinary controls</title><button onclick="document.title='Clicked'">Click me</button><input aria-label="Name"><select aria-label="Flavor"><option value="a">Apple</option><option value="b">Berry</option></select><input type="file" aria-label="Attachment"><button draggable="true">Drag source</button><button ondragover="event.preventDefault()" ondrop="document.body.dataset.dropped='yes'">Drop target</button>`;
const ref = (snapshot, name) => {
  const found = new RegExp(`"${name}"[^\\n]*\\[ref=(e\\d+)\\]`).exec(snapshot)?.[1];
  assert.ok(found, snapshot);
  return found;
};

async function until(run) {
  const deadline = Date.now() + 15000;
  do {
    const result = await run();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  throw new Error('filtered-parent fixture deadline');
}

const listen = (server) => new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const close = (server) =>
  new Promise((resolve) => {
    if (!server) return resolve();
    server.closeAllConnections();
    server.close(resolve);
  });

test('Chrome: ordinary controls retain isolated extension worlds with filtered background ancestry', {
  timeout: 180000,
}, async () => {
  const dir = await mkdtemp(path.join(tmpdir(), 'slicc-filtered-parent-'));
  let browser;
  let raw;
  let proxy;
  let plain;
  let secure;
  let runner;
  let cwd;
  try {
    await writeFile(
      path.join(dir, 'manifest.json'),
      JSON.stringify({
        manifest_version: 3,
        name: 'Ordinary world fixture',
        version: '1.0',
        content_scripts: [
          { matches: ['https://localhost/*'], js: ['inject.js'], run_at: 'document_end' },
        ],
      })
    );
    await writeFile(
      path.join(dir, 'inject.js'),
      'document.documentElement.dataset.extensionFixture = chrome.runtime.id;'
    );
    execFileSync(
      'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-days',
        '1',
        '-subj',
        '/CN=localhost',
        '-keyout',
        path.join(dir, 'key.pem'),
        '-out',
        path.join(dir, 'cert.pem'),
      ],
      { stdio: 'ignore' }
    );
    let port;
    plain = http((req, res) => {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end(
        req.url === '/background'
          ? `<!doctype html><iframe src="http://localhost:${port}/child"></iframe>`
          : '<!doctype html><title>Background frame</title>'
      );
    });
    await listen(plain);
    port = plain.address().port;
    secure = https(
      {
        key: await readFile(path.join(dir, 'key.pem')),
        cert: await readFile(path.join(dir, 'cert.pem')),
      },
      (_req, res) => {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end(body);
      }
    );
    await listen(secure);
    browser = await launchBrowser([
      `--disable-extensions-except=${dir}`,
      `--load-extension=${dir}`,
      '--site-per-process',
      '--ignore-certificate-errors',
      '--no-proxy-server',
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1',
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await observer(browser.wsUrl);
    const background = (
      await raw.send('Target.createTarget', {
        url: `http://seven.sliccy.ai:${port}/background`,
      })
    ).targetId;
    const child = await until(async () =>
      (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.find(
        (target) => target.type === 'iframe' && target.url === `http://localhost:${port}/child`
      )
    );
    assert.ok(child.parentFrameId);
    const wire = [];
    const peers = new Map();
    const lifecycle = () =>
      [...peers].map(([connection, peer]) => ({
        connection,
        closed: peer.closed,
        sessions: [...peer.sessions.values()],
        pauses: [...peer.pauses.values()],
      }));
    const identities = new Map();
    const identity = (value) => {
      if (!value) return 0;
      if (!identities.has(value)) identities.set(value, identities.size + 1);
      return identities.get(value);
    };
    proxy = await cdpProxy(browser.wsUrl, {
      connectionClosed(connection) {
        const peer = peers.get(connection);
        if (peer) peer.closed = true;
      },
      observe(direction, message, connection) {
        if (!peers.has(connection))
          peers.set(connection, {
            closed: false,
            pending: new Map(),
            sessions: new Map(),
            pauses: new Map(),
          });
        const peer = peers.get(connection);
        const session = identity(message.sessionId);
        if (direction === 'client') {
          const entry = {
            connection,
            session,
            target: identity(message.params?.targetId),
            detached: identity(message.params?.sessionId),
            request: identity(message.params?.requestId),
            method: message.method,
            type: message.params?.type,
            flush: message.method === 'Runtime.evaluate' && message.params?.expression === '0',
            sent: Date.now(),
          };
          peer.pending.set(message.id, entry);
          wire.push(entry);
          if (wire.length > 40) wire.shift();
        } else if (message.id && peer.pending.has(message.id)) {
          const entry = peer.pending.get(message.id);
          peer.pending.delete(message.id);
          entry.elapsed = Date.now() - entry.sent;
          entry.reply = message.error ? 'error' : 'success';
          if (!message.error) {
            if (entry.method === 'Target.attachToTarget') {
              const attached = identity(message.result?.sessionId);
              peer.sessions.set(attached, {
                session: attached,
                target: entry.target,
                fetch: false,
              });
              entry.attached = attached;
            }
            if (entry.method === 'Target.detachFromTarget') peer.sessions.delete(entry.detached);
            if (entry.method === 'Fetch.enable' && peer.sessions.has(entry.session))
              peer.sessions.get(entry.session).fetch = true;
            if (entry.method === 'Fetch.disable') {
              if (peer.sessions.has(entry.session)) peer.sessions.get(entry.session).fetch = false;
              for (const [key, pause] of peer.pauses)
                if (pause.session === entry.session) peer.pauses.delete(key);
            }
            if (
              /^Fetch\.(continueRequest|continueResponse|fulfillRequest|failRequest)$/.test(
                entry.method
              )
            )
              peer.pauses.delete(`${entry.session}:${entry.request}`);
          }
          const frame = message.result?.frameTree?.frame;
          if (frame) {
            entry.frame = identity(frame.id);
            entry.loader = identity(frame.loaderId);
            entry.parent = identity(frame.parentId);
            entry.knownUrl = Boolean(frame.url);
            entry.knownOrigin = Boolean(frame.securityOrigin && frame.securityOrigin !== '://');
          }
        } else {
          if (message.method === 'Target.detachedFromTarget')
            peer.sessions.delete(identity(message.params?.sessionId));
          if (message.method === 'Fetch.requestPaused') {
            const request = identity(message.params?.requestId);
            peer.pauses.set(`${session}:${request}`, {
              session,
              request,
              type: message.params?.resourceType,
            });
            wire.push({ connection, event: message.method, session, request });
          } else if (message.method?.startsWith('Page.frame')) {
            wire.push({ connection, event: message.method, session });
          }
          if (wire.length > 40) wire.shift();
        }
      },
      transform(message) {
        if (message.result?.targetInfos)
          message.result.targetInfos = message.result.targetInfos.filter(
            (target) => target.targetId !== background
          );
        return message;
      },
    });
    runner = nativeRunner(exe('playwright-cli'), exe('curlwright'), proxy.url);
    cwd = await runner.fresh();
    const run = async (...args) => {
      const result = await runner.playwright(args, cwd);
      assert.equal(
        result.code,
        0,
        `${args.join(' ')}: ${result.stderr} ${JSON.stringify({ lifecycle: lifecycle(), wire })}`
      );
      return result.stdout;
    };
    const userUrl = `https://localhost:${secure.address().port}/ordinary`;
    await run('open', '--foreground', userUrl);
    const user = (await raw.send('Target.getTargets', {})).targetInfos.find(
      (target) => target.type === 'page' && target.url === userUrl
    );
    assert.ok(user);
    const userSession = (
      await raw.send('Target.attachToTarget', { targetId: user.targetId, flatten: true })
    ).sessionId;
    await until(async () => {
      const value = await run('eval', 'document.documentElement.dataset.extensionFixture');
      return /[a-p]{32}/.test(value);
    });
    const snapshot = await run('snapshot');
    assert.match(snapshot, /Ordinary controls/);
    await run('click', ref(snapshot, 'Click me'));
    assert.match(await run('eval', 'document.title'), /Clicked/);
    await run('fill', ref(await run('snapshot'), 'Name'), 'Ada');
    await run('select', ref(await run('snapshot'), 'Flavor'), 'b');
    await runner.write(cwd, 'attachment.txt', 'fixture bytes');
    await run('upload', ref(await run('snapshot'), 'Attachment'), 'attachment.txt');
    const drag = await run('snapshot');
    await raw.send('Target.activateTarget', { targetId: user.targetId });
    await until(
      async () =>
        (
          await raw.send(
            'Runtime.evaluate',
            { expression: 'document.visibilityState', returnByValue: true },
            userSession
          )
        ).result.value === 'visible'
    );
    const beforeDrag = lifecycle();
    wire.length = 0;
    const dragged = await runner.playwright(
      ['drag', ref(drag, 'Drag source'), ref(drag, 'Drop target')],
      cwd
    );
    assert.equal(
      dragged.code,
      0,
      `drag: ${dragged.stderr} ${JSON.stringify({ beforeDrag, afterDrag: lifecycle(), wire })}`
    );
    await raw.send('Target.detachFromTarget', { sessionId: userSession });
    const denied = await runner.playwright(['eval', '--tab', background, 'document.title'], cwd);
    assert.notEqual(denied.code, 0);
    assert.match(denied.stderr, /refused/);
  } finally {
    await runner?.close();
    if (cwd) {
      await runner.clearSession(cwd);
      await rm(cwd, { recursive: true, force: true });
    }
    await proxy?.close();
    await raw?.close();
    await browser?.close();
    await close(secure);
    await close(plain);
    await rm(dir, { recursive: true, force: true });
  }
});
