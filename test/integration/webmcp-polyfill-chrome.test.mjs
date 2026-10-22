import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { observer } from './guard-client.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const binary = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function command(endpoint, cwd, args) {
  const env = { ...process.env, HOME: cwd, USERPROFILE: cwd, APPDATA: cwd, XDG_CONFIG_HOME: cwd };
  delete env.SLICC_CDP_URL;
  const child = spawn(binary, ['--cdp', endpoint, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const documentWaiters = new Set();
  let parsed = 0;
  child.stdout.on('data', (data) => {
    stdout += data;
    const lines = stdout.split('\n');
    while (parsed < lines.length - 1) {
      const value = JSON.parse(lines[parsed++]);
      if (value.event === 'document-ready')
        for (const resolve of [...documentWaiters]) resolve(value);
    }
  });
  child.stderr.on('data', (data) => {
    stderr += data;
  });
  const ended = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('close', (code) => resolve({ code, stdout, stderr }));
  });
  const ready = new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`holder did not report readiness: ${stderr}`)),
      20000
    );
    child.stdout.on('data', () => {
      if (!stdout.includes('\n')) return;
      clearTimeout(timer);
      try {
        resolve(JSON.parse(stdout.split('\n')[0]));
      } catch (error) {
        reject(error);
      }
    });
    ended.then((result) => {
      clearTimeout(timer);
      if (!stdout.includes('\n')) reject(new Error(result.stderr));
    }, reject);
  });
  ready.catch(() => {});
  const documentReady = () =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        documentWaiters.delete(done);
        reject(new Error(`document readiness did not arrive: ${stderr}`));
      }, 15000);
      const done = (value) => {
        clearTimeout(timer);
        documentWaiters.delete(done);
        resolve(value);
      };
      documentWaiters.add(done);
      ended.then((result) => {
        if (!documentWaiters.has(done)) return;
        clearTimeout(timer);
        documentWaiters.delete(done);
        reject(new Error(`holder ended before document readiness: ${result.stderr}`));
      }, reject);
    });
  return { child, ended, ready, documentReady, stderr: () => stderr };
}

async function once(endpoint, cwd, args) {
  const process = command(endpoint, cwd, args);
  process.child.stdin.end();
  return process.ended;
}

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await raw.send('Page.navigate', { url }, session);
  await loaded;
}

async function hasShim(raw, session) {
  return (
    await raw.send(
      'Runtime.evaluate',
      {
        expression:
          '({secure:isSecureContext,api:!!document.modelContext,alias:!!navigator.modelContext})',
        returnByValue: true,
      },
      session
    )
  ).result.value;
}

await test('Chrome: guarded WebMCP holder registers load-time tools, enforces policy and excludes protected documents', {
  timeout: 180000,
}, async (t) => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-chrome-'));
  let browser;
  let raw;
  let holder;
  let server;
  try {
    const keyPath = path.join(cwd, 'key.pem');
    const certPath = path.join(cwd, 'cert.pem');
    execFileSync(
      'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-keyout',
        keyPath,
        '-out',
        certPath,
        '-days',
        '1',
        '-subj',
        '/CN=localhost',
        '-addext',
        'subjectAltName=DNS:localhost,DNS:seven.sliccy.ai,DNS:vault.sliccy.ai',
      ],
      { stdio: 'ignore' }
    );
    server = createServer(
      { key: await readFile(keyPath), cert: await readFile(certPath) },
      (req, res) => {
        if (req.url === '/error') {
          req.socket.destroy();
          return;
        }
        const policy = req.url === '/deny' ? { 'permissions-policy': 'tools=()' } : {};
        res.writeHead(200, { 'content-type': 'text/html', ...policy });
        const child = req.url === '/same' || req.url === '/cross';
        const names = child
          ? [req.url === '/same' ? 'same_child' : 'cross_child']
          : [
              'get_inbox_state',
              'list_threads',
              'open_thread',
              'list_folders',
              'search_threads',
              'get_preferences',
              'set_preferences',
              'send_reply',
            ];
        const frames =
          req.url === '/frames'
            ? `<iframe src="${ordinary}/same"></iframe><iframe src="https://foreign.test:${port}/cross" allow="tools *"></iframe><iframe src="${protectedUrl}"></iframe>`
            : '';
        res.end(`<!doctype html><title>WebMCP local fixture</title>${frames}<script>
if (document.modelContext) {
  for (const name of ${JSON.stringify(names)}) document.modelContext.registerTool({name,description:name,inputSchema:{type:'object'},exposedTo:[${JSON.stringify(ordinary)}],annotations:{readOnly:name !== 'send_reply',consequential:name === 'send_reply',untrustedContent:true},execute:async(input,client)=>({name,signal:!!client.signal,input})});
}
</script>`);
      }
    );
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const port = server.address().port;
    const ordinary = `https://localhost:${port}`;
    const protectedUrl = `https://seven.sliccy.ai:${port}/protected`;
    browser = await launchBrowser([
      '--ignore-certificate-errors',
      '--no-proxy-server',
      '--disable-features=WebMCP,WebMCPTesting',
      '--site-per-process',
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1, MAP vault.sliccy.ai 127.0.0.1, MAP foreign.test 127.0.0.1',
    ]);
    raw = await observer(browser.wsUrl);
    const { targetId } = await raw.send('Target.createTarget', {
      url: 'about:blank',
      background: true,
    });
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
    await raw.send('Page.enable', {}, sessionId);
    await navigate(raw, sessionId, protectedUrl);
    assert.deepEqual(await hasShim(raw, sessionId), { secure: true, api: false, alias: false });
    const protectedEntry = (
      await raw.send('Page.getNavigationHistory', {}, sessionId)
    ).entries.find((entry) => entry.url === protectedUrl);
    const baselineSamples = [];
    for (let sample = 0; sample < 5; sample++) {
      const started = performance.now();
      await navigate(raw, sessionId, `${ordinary}/tools?baseline=${sample}`);
      baselineSamples.push(performance.now() - started);
    }
    assert.equal((await hasShim(raw, sessionId)).api, false);
    holder = command(browser.wsUrl, cwd, ['webmcp-polyfill', '--tab', targetId, '--reload']);
    const status = await holder.ready;
    assert.equal(status.mode, 'polyfill');
    assert.ok(status.pauseMicros.length >= 2);
    assert.equal(JSON.stringify(status).includes('__wmcp_'), false);
    const holderSamples = [];
    for (let sample = 0; sample < 5; sample++) {
      const started = performance.now();
      const complete = holder.documentReady();
      await navigate(raw, sessionId, `${ordinary}/tools?holder=${sample}`);
      await complete;
      holderSamples.push(performance.now() - started);
    }
    const median = (samples) => [...samples].sort((a, b) => a - b)[2];
    t.diagnostic(
      `webmcp-gate-measurement ${JSON.stringify({ baselineNavigationMs: baselineSamples, holderNavigationMs: holderSamples, medianNavigationOverheadMs: median(holderSamples) - median(baselineSamples), pauseMicros: status.pauseMicros })}`
    );
    assert.deepEqual(await hasShim(raw, sessionId), { secure: true, api: true, alias: true });
    const listed = await once(browser.wsUrl, cwd, ['webmcp-list', '--tab', targetId, '--json']);
    assert.equal(listed.code, 0, listed.stderr);
    assert.equal(JSON.parse(listed.stdout).length, 8);
    const called = await once(browser.wsUrl, cwd, [
      'webmcp-call',
      'get_inbox_state',
      '{}',
      '--tab',
      targetId,
    ]);
    assert.equal(called.code, 0, called.stderr);
    assert.equal(JSON.parse(called.stdout).signal, true);
    const consequential = await once(browser.wsUrl, cwd, [
      'webmcp-call',
      'send_reply',
      '{}',
      '--tab',
      targetId,
    ]);
    assert.equal(consequential.code, 2);
    const denyReady = holder.documentReady();
    await navigate(raw, sessionId, `${ordinary}/deny`);
    await denyReady;
    const denied = await once(browser.wsUrl, cwd, ['webmcp-list', '--tab', targetId, '--json']);
    assert.equal(denied.code, 0, denied.stderr);
    assert.deepEqual(JSON.parse(denied.stdout), []);
    const framesReady = holder.documentReady();
    await navigate(raw, sessionId, `${ordinary}/frames`);
    await framesReady;
    const same = await once(browser.wsUrl, cwd, ['webmcp-list', '--tab', targetId, '--json']);
    assert.equal(same.code, 0, same.stderr);
    assert.equal(JSON.parse(same.stdout).length, 9);
    const cross = await once(browser.wsUrl, cwd, [
      'webmcp-list',
      '--tab',
      targetId,
      '--json',
      '--from-origin',
      `https://foreign.test:${port}`,
    ]);
    assert.equal(cross.code, 0, cross.stderr);
    assert.equal(JSON.parse(cross.stdout).length, 10, holder.stderr());
    const crossCall = await once(browser.wsUrl, cwd, [
      'webmcp-call',
      'cross_child',
      '{}',
      '--tab',
      targetId,
      '--from-origin',
      `https://foreign.test:${port}`,
    ]);
    assert.equal(crossCall.code, 0, crossCall.stderr);
    assert.equal(JSON.parse(crossCall.stdout).name, 'cross_child');
    const targetInfos = (await raw.send('Target.getTargets', {})).targetInfos;
    const protectedChild = targetInfos.find(
      (target) => target.type === 'iframe' && target.url === protectedUrl
    );
    assert.ok(protectedChild, 'protected OOPIF was not created');
    const childSession = (
      await raw.send('Target.attachToTarget', { targetId: protectedChild.targetId, flatten: true })
    ).sessionId;
    assert.equal((await hasShim(raw, childSession)).api, false);
    await raw.send('Target.detachFromTarget', { sessionId: childSession });
    const restored = raw.waitEvent(
      (event) => event.method === 'Page.frameNavigated' && event.sessionId === sessionId,
      15000
    );
    await raw.send('Page.navigateToHistoryEntry', { entryId: protectedEntry.id }, sessionId);
    await restored;
    assert.equal((await hasShim(raw, sessionId)).api, false);
    const refused = await once(browser.wsUrl, cwd, ['webmcp-list', '--tab', targetId, '--json']);
    assert.notEqual(refused.code, 0);
    assert.equal(refused.stdout, '');
    await holder.ended;
    holder = undefined;
    await navigate(raw, sessionId, `${ordinary}/tools`);
    assert.equal((await hasShim(raw, sessionId)).api, false);
    for (const destination of [protectedUrl, `https://vault.sliccy.ai:${port}/error`]) {
      holder = command(browser.wsUrl, cwd, ['webmcp-polyfill', '--tab', targetId, '--reload']);
      assert.equal((await holder.ready).mode, 'polyfill');
      await navigate(raw, sessionId, destination);
      assert.equal((await hasShim(raw, sessionId)).api, false);
      const deniedCall = await once(browser.wsUrl, cwd, [
        'webmcp-call',
        'get_inbox_state',
        '{}',
        '--tab',
        targetId,
      ]);
      assert.notEqual(deniedCall.code, 0);
      assert.equal(deniedCall.stdout, '');
      await holder.ended;
      holder = undefined;
      await navigate(raw, sessionId, `${ordinary}/tools`);
      assert.equal((await hasShim(raw, sessionId)).api, false);
    }
  } finally {
    holder?.child.kill('SIGKILL');
    await raw?.close();
    await browser?.close();
    await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
    await rm(cwd, { recursive: true, force: true });
  }
});
