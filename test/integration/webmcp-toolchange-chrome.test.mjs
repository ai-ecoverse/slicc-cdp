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

function environment(cwd) {
  const env = { ...process.env, HOME: cwd, USERPROFILE: cwd, APPDATA: cwd, XDG_CONFIG_HOME: cwd };
  delete env.SLICC_CDP_URL;
  return env;
}

function stream(endpoint, cwd, args) {
  const child = spawn(binary, ['--cdp', endpoint, ...args], {
    cwd,
    env: environment(cwd),
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let pending = '';
  let stderr = '';
  const lines = [];
  const waiters = new Set();
  child.stdout.on('data', (bytes) => {
    pending += bytes;
    const parts = pending.split('\n');
    pending = parts.pop();
    for (const part of parts) {
      lines.push(JSON.parse(part));
      for (const waiter of [...waiters]) waiter();
    }
  });
  child.stderr.on('data', (bytes) => {
    stderr += bytes;
  });
  let closed = false;
  const ended = new Promise((resolve) =>
    child.once('close', (code) => {
      closed = true;
      resolve(code);
      for (const waiter of [...waiters]) waiter();
    })
  );
  const next = (predicate, from, timeout = 20000) =>
    new Promise((resolve, reject) => {
      const check = () => {
        const found = lines.slice(from).find(predicate);
        if (!found && !closed) return;
        clearTimeout(timer);
        waiters.delete(check);
        if (found) resolve(found);
        else reject(new Error(`holder exited before event: ${JSON.stringify(lines)} ${stderr}`));
      };
      const timer = setTimeout(() => {
        waiters.delete(check);
        reject(new Error(`event did not arrive: ${JSON.stringify(lines)} ${stderr}`));
      }, timeout);
      waiters.add(check);
      check();
    });
  return { child, ended, lines, next, stderr: () => stderr };
}

function once(endpoint, cwd, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, ['--cdp', endpoint, ...args], { cwd, env: environment(cwd) });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (bytes) => {
      stdout += bytes;
    });
    child.stderr.on('data', (bytes) => {
      stderr += bytes;
    });
    child.once('error', reject);
    child.once('close', (code) => resolve({ code, stdout, stderr }));
    child.stdin.end();
  });
}

async function navigate(raw, session, url) {
  const loaded = raw.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === session,
    15000
  );
  await Promise.all([raw.send('Page.navigate', { url }, session), loaded]).catch((error) => {
    throw new Error(`Page.loadEventFired did not arrive for ${new URL(url).pathname}`, {
      cause: error,
    });
  });
}

async function evaluate(raw, session, expression) {
  const result = await raw.send('Runtime.evaluate', { expression, returnByValue: true }, session);
  assert.equal(result.exceptionDetails, undefined, JSON.stringify(result.exceptionDetails));
  return result.result.value;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const changed = (frameId) => (line) => line.event === 'tools-changed' && line.frameId === frameId;
const names = (event) => event.tools.map((tool) => tool.name);

await test('Chrome: the WebMCP holder reports ordinary tool changes, recovers cross-origin frames after navigation and excludes protected frames', {
  timeout: 240000,
}, async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-toolchange-chrome-'));
  let browser;
  let raw;
  let held;
  let watcher;
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
        'subjectAltName=DNS:localhost,DNS:seven.sliccy.ai,DNS:foreign.test',
      ],
      { stdio: 'ignore' }
    );
    let port;
    let ordinary;
    server = createServer(
      { key: await readFile(keyPath), cert: await readFile(certPath) },
      (req, res) => {
        res.writeHead(200, { 'content-type': 'text/html' });
        const url = new URL(req.url, 'https://localhost');
        const tool = (name) =>
          `{name:${JSON.stringify(name)},description:${JSON.stringify(`${name} tool`)},inputSchema:{type:'object'},exposedTo:[${JSON.stringify(ordinary)}],annotations:{readOnly:true},execute:async()=>${JSON.stringify(name)}}`;
        if (url.pathname === '/cross') {
          res.end(
            `<!doctype html><script>const crossController = new AbortController(); window.addCross = () => document.modelContext.registerTool(${tool('cross_late')}, { signal: crossController.signal }); window.removeCross = () => crossController.abort();</script>`
          );
          return;
        }
        if (url.pathname === '/protected') {
          res.end(
            `<!doctype html><script>const tools = []; document.modelContext = {getTools: async () => tools, executeTool: async () => 'protected'}; window.addProtected = () => tools.push(${tool('protected_tool')});</script>`
          );
          return;
        }
        const frames = ['/frames', '/ordinary-frames'].includes(url.pathname)
          ? `<iframe src="https://foreign.test:${port}/cross" allow="tools *"></iframe>${url.pathname === '/frames' ? `<iframe src="https://seven.sliccy.ai:${port}/protected"></iframe>` : ''}`
          : '';
        res.end(`<!doctype html><title>toolchange</title>${frames}<script>
const controller = new AbortController();
if (document.modelContext) document.modelContext.registerTool(${tool('initial')});
window.addLate = () => document.modelContext.registerTool(${tool('late')}, { signal: controller.signal });
window.removeLate = () => controller.abort();
addEventListener('pagehide', () => document.modelContext.registerTool(${tool('stale')}));
</script>`);
      }
    );
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    port = server.address().port;
    ordinary = `https://localhost:${port}`;
    browser = await launchBrowser([
      '--ignore-certificate-errors',
      '--no-proxy-server',
      '--disable-features=WebMCP,WebMCPTesting',
      '--site-per-process',
      '--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1, MAP foreign.test 127.0.0.1',
    ]);
    raw = await observer(browser.wsUrl);
    const { targetId } = await raw.send('Target.createTarget', {
      url: 'about:blank',
      background: true,
    });
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
    await raw.send('Page.enable', {}, sessionId);
    await navigate(raw, sessionId, `${ordinary}/tools`);
    held = stream(browser.wsUrl, cwd, ['webmcp-polyfill', '--tab', targetId, '--reload']);
    const ready = await held.next((line) => line.event === 'ready', 0);
    assert.equal(ready.mode, 'polyfill');
    watcher = stream(browser.wsUrl, cwd, ['discover', '--json', '--no-probe']);
    await sleep(3000);

    let from = held.lines.length;
    await evaluate(raw, sessionId, 'addLate()');
    const added = await held.next(changed(targetId), from);
    assert.equal(added.tab, targetId);
    assert.equal(added.target, targetId);
    assert.equal(added.loaderId, ready.loaderId);
    assert.deepEqual(names(added), ['initial', 'late']);
    assert.ok(added.tools.every((tool) => tool.polyfilled === true && tool.policy === 'allowed'));
    const listed = await once(browser.wsUrl, cwd, ['webmcp-list', '--tab', targetId, '--json']);
    assert.equal(listed.code, 0, listed.stderr);
    assert.deepEqual(added.tools, JSON.parse(listed.stdout));
    const reported = await watcher.next(
      (line) => line.standard === 'webmcp' && line.summary?.event === 'tools-changed',
      0
    );
    assert.equal(reported.targetId, targetId);
    assert.equal(reported.summary.loaderId, added.loaderId);
    assert.deepEqual(reported.summary.tools, added.tools);

    from = held.lines.length;
    await evaluate(raw, sessionId, 'removeLate()');
    const removed = await held.next(changed(targetId), from);
    assert.equal(removed.loaderId, ready.loaderId);
    assert.deepEqual(names(removed), ['initial']);

    const navigated = held.lines.length;
    const framesReady = held.next(
      (line) => line.event === 'document-ready' && line.loaderId !== ready.loaderId,
      navigated
    );
    const [, document] = await Promise.all([
      navigate(raw, sessionId, `${ordinary}/frames`),
      framesReady,
    ]);
    await sleep(1500);
    assert.equal(
      held.lines
        .slice(navigated)
        .some(
          (line) =>
            line.event === 'tools-changed' &&
            (line.loaderId === ready.loaderId || names(line).includes('stale'))
        ),
      false,
      JSON.stringify(held.lines)
    );
    const targets = (await raw.send('Target.getTargets', {})).targetInfos;
    const cross = targets.find(
      (target) => target.type === 'iframe' && target.url === `https://foreign.test:${port}/cross`
    );
    assert.ok(cross, 'cross-origin OOPIF was not created');
    const sealed = targets.find(
      (target) =>
        target.type === 'iframe' && target.url === `https://seven.sliccy.ai:${port}/protected`
    );
    assert.ok(sealed, 'protected OOPIF was not created');
    const protectedSession = (
      await raw.send('Target.attachToTarget', { targetId: sealed.targetId, flatten: true })
    ).sessionId;
    assert.equal(await evaluate(raw, protectedSession, 'typeof addProtected'), 'undefined');
    await raw.send('Target.detachFromTarget', { sessionId: protectedSession });
    const crossSession = (
      await raw.send('Target.attachToTarget', { targetId: cross.targetId, flatten: true })
    ).sessionId;
    await evaluate(raw, crossSession, 'addCross()');
    const crossListed = await once(browser.wsUrl, cwd, [
      'webmcp-list',
      '--tab',
      targetId,
      '--json',
      '--from-origin',
      `https://foreign.test:${port}`,
    ]);
    assert.notEqual(crossListed.code, 0);
    assert.equal(crossListed.stdout, '');
    assert.match(crossListed.stderr, /slicc: refused:/);
    await sleep(1500);
    assert.deepEqual(held.lines.slice(navigated).filter(changed(cross.targetId)), []);
    assert.equal(
      (await raw.send('Target.getTargetInfo', { targetId: sealed.targetId })).targetInfo.url,
      `https://seven.sliccy.ai:${port}/protected`
    );
    assert.equal(JSON.stringify(held.lines.slice(navigated)).includes('cross_late'), false);
    assert.equal(JSON.stringify(watcher.lines).includes('cross_late'), false);
    await raw.send('Target.detachFromTarget', { sessionId: crossSession });
    assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    assert.equal(JSON.stringify(held.lines).includes('protected_tool'), false);
    assert.equal(JSON.stringify(watcher.lines).includes('protected_tool'), false);
    assert.ok(document.loaderId);

    const crossControl = async (stale) => {
      const child = (await raw.send('Target.getTargets', {})).targetInfos.find(
        (target) =>
          target.type === 'iframe' &&
          target.url === `https://foreign.test:${port}/cross` &&
          target.parentFrameId === targetId &&
          !stale.includes(target.targetId)
      );
      assert.ok(child, 'ordinary OOPIF was not created');
      const childSession = (
        await raw.send('Target.attachToTarget', { targetId: child.targetId, flatten: true })
      ).sessionId;
      const fromOrigin = async () => {
        const listed = await once(browser.wsUrl, cwd, [
          'webmcp-list',
          '--tab',
          targetId,
          '--json',
          '--from-origin',
          `https://foreign.test:${port}`,
        ]);
        assert.equal(listed.code, 0, listed.stderr);
        return JSON.parse(listed.stdout).filter((tool) => tool.frameId === child.targetId);
      };
      let start = held.lines.length;
      await evaluate(raw, childSession, 'addCross()');
      const added = await held.next(changed(child.targetId), start);
      assert.equal(added.tab, targetId);
      assert.equal(added.target, child.targetId);
      assert.equal(added.origin, `https://foreign.test:${port}`);
      assert.deepEqual(names(added), ['cross_late']);
      assert.deepEqual(added.tools, await fromOrigin());
      start = held.lines.length;
      await evaluate(raw, childSession, 'removeCross()');
      const removed = await held.next(changed(child.targetId), start);
      assert.equal(removed.target, child.targetId);
      assert.equal(removed.loaderId, added.loaderId);
      assert.deepEqual(removed.tools, []);
      assert.deepEqual(await fromOrigin(), []);
      await raw.send('Target.detachFromTarget', { sessionId: childSession });
      return child.targetId;
    };

    const ordinaryReady = held.next(
      (line) => line.event === 'document-ready' && line.loaderId !== document.loaderId,
      held.lines.length
    );
    await Promise.all([navigate(raw, sessionId, `${ordinary}/ordinary-frames`), ordinaryReady]);
    const navigatedCross = await crossControl([cross.targetId]);
    assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    assert.equal(JSON.stringify(held.lines).includes('protected_tool'), false);
    held.child.stdin.end();
    assert.equal(await held.ended, 0, held.stderr());

    held = stream(browser.wsUrl, cwd, ['webmcp-polyfill', '--tab', targetId, '--reload']);
    const restarted = await held.next((line) => line.event === 'ready', 0);
    assert.equal(restarted.mode, 'polyfill');
    assert.equal(restarted.reloaded, true);
    await crossControl([cross.targetId, navigatedCross]);
    assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    held.child.stdin.end();
    assert.equal(await held.ended, 0, held.stderr());
    held = undefined;
  } finally {
    held?.child.kill('SIGKILL');
    watcher?.child.kill('SIGKILL');
    await raw?.close();
    await browser?.close();
    await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
    await rm(cwd, { recursive: true, force: true });
  }
});
