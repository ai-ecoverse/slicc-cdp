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
import { webmcpWire } from './webmcp-wire.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const binary = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

function environment(cwd) {
  const env = {
    ...process.env,
    HOME: cwd,
    USERPROFILE: cwd,
    APPDATA: cwd,
    XDG_CONFIG_HOME: cwd,
    SLICC_WEBMCP_TRACE: '1',
  };
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
        const revokes = (
          stderr.match(/webmcp-polyfill: \{[^\n]*"event":"revoke"[^\n]*/g) || []
        ).join('\n');
        const parked = (stderr.match(/\[diag:parked:[^\]]+\][^\n]*/g) || []).slice(-8).join('\n');
        reject(
          new Error(
            `event did not arrive: ${JSON.stringify(lines)}\nstderr: ${stderr}\ntrace-revokes: ${revokes}\nparked-diag: ${parked}`
          )
        );
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

async function waitForShim(raw, session, timeout = 15000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if ((await hasShim(raw, session)).api) return;
    await sleep(50);
  }
  throw new Error('child shim did not appear');
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const changed = (frameId) => (line) => line.event === 'tools-changed' && line.frameId === frameId;
const names = (event) => event.tools.map((tool) => tool.name);

for (const protectedFrames of [true, false]) {
  await test(protectedFrames
    ? 'Chrome: the WebMCP holder reports ordinary tool changes, recovers cross-origin frames after navigation and excludes protected frames'
    : 'Chrome: ordinary to ordinary navigation retains admitted cross-origin tool changes', {
    timeout: 240000,
  }, async () => {
    const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-toolchange-chrome-'));
    let browser;
    let raw;
    let held;
    let watcher;
    let server;
    let wire;
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
      const crossReady = [];
      let crossReadyOpen = false;
      const releaseCrossReady = () => {
        crossReadyOpen = true;
        for (const pending of crossReady.splice(0)) {
          pending.writeHead(200, { 'content-type': 'text/plain' });
          pending.end('ok');
        }
      };
      server = createServer(
        { key: await readFile(keyPath), cert: await readFile(certPath) },
        (req, res) => {
          const url = new URL(req.url, 'https://localhost');
          if (url.pathname === '/cross-child-ready') {
            if (crossReadyOpen) {
              res.writeHead(200, { 'content-type': 'text/plain' });
              res.end('ok');
              return;
            }
            crossReady.push(res);
            return;
          }
          const headers = { 'content-type': 'text/html' };
          if (['/cross', '/frames', '/ordinary-frames', '/ordinary-start'].includes(url.pathname)) {
            headers['permissions-policy'] = 'tools=*';
          }
          res.writeHead(200, headers);
          const tool = (name) =>
            `{name:${JSON.stringify(name)},description:${JSON.stringify(`${name} tool`)},inputSchema:{type:'object'},exposedTo:[${JSON.stringify(ordinary)}],annotations:{readOnly:true},execute:async()=>${JSON.stringify(name)}}`;
          if (url.pathname === '/cross') {
            res.end(
              `<!doctype html><script>const crossController = new AbortController(); window.addCross = async () => { document.modelContext?.registerTool(${tool('cross_late')}, { signal: crossController.signal }); }; window.removeCross = () => crossController.abort(); void (async () => { await fetch('/cross-child-ready'); await window.addCross(); })();</script>`
            );
            return;
          }
          if (url.pathname === '/protected') {
            res.end(
              `<!doctype html><script>const tools = []; document.modelContext = {getTools: async () => tools, executeTool: async () => 'protected'}; window.addProtected = () => tools.push(${tool('protected_tool')});</script>`
            );
            return;
          }
          const frames = ['/frames', '/ordinary-frames', '/ordinary-start'].includes(url.pathname)
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
      wire = await webmcpWire(browser.wsUrl);
      raw = await observer(browser.wsUrl);
      const { targetId } = await raw.send('Target.createTarget', {
        url: 'about:blank',
        background: true,
      });
      const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
      await raw.send('Page.enable', {}, sessionId);
      await navigate(raw, sessionId, `${ordinary}/${protectedFrames ? 'tools' : 'ordinary-start'}`);
      const foreignOrigin = `https://foreign.test:${port}`;
      held = stream(wire.url, cwd, [
        'webmcp-polyfill',
        '--tab',
        targetId,
        '--reload',
        '--from-origin',
        foreignOrigin,
      ]);
      const ready = await held.next((line) => line.event === 'ready', 0);
      assert.equal(ready.mode, 'polyfill');
      watcher = stream(wire.url, cwd, ['discover', '--json', '--no-probe']);
      await sleep(3000);

      let from = held.lines.length;
      await evaluate(raw, sessionId, 'addLate()');
      const added = await held.next(changed(targetId), from);
      assert.equal(added.tab, targetId);
      assert.equal(added.target, targetId);
      assert.equal(added.loaderId, ready.loaderId);
      assert.deepEqual(names(added), ['initial', 'late']);
      assert.ok(added.tools.every((tool) => tool.polyfilled === true && tool.policy === 'allowed'));
      const listed = await once(wire.url, cwd, ['webmcp-list', '--tab', targetId, '--json']);
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

      if (!protectedFrames) {
        const initial = (await raw.send('Target.getTargets', {})).targetInfos;
        assert.equal(
          initial.some(
            (target) => target.type === 'iframe' && target.url.includes('seven.sliccy.ai')
          ),
          false
        );
        const foreign = initial.find(
          (target) =>
            target.type === 'iframe' && target.url === `https://foreign.test:${port}/cross`
        );
        assert.ok(foreign);
        const childSession = (
          await raw.send('Target.attachToTarget', { targetId: foreign.targetId, flatten: true })
        ).sessionId;
        assert.equal(
          await evaluate(raw, childSession, 'typeof document.modelContext'),
          'object',
          'start-time --from-origin installs the child shim before listing'
        );
        await raw.send('Target.detachFromTarget', { sessionId: childSession });
      }
      const navigated = held.lines.length;
      const framesReady = protectedFrames
        ? held.ended
        : held.next(
            (line) => line.event === 'document-ready' && line.loaderId !== ready.loaderId,
            navigated
          );
      const [, document] = await Promise.all([
        navigate(raw, sessionId, `${ordinary}/${protectedFrames ? 'frames' : 'ordinary-frames'}`),
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
      if (protectedFrames) {
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
        const crossListed = await once(wire.url, cwd, [
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
      }
      if (protectedFrames) assert.notEqual(document, 0);
      else assert.ok(document.loaderId);

      const crossControl = async (stale) => {
        crossReadyOpen = false;
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
        await waitForShim(raw, childSession);
        const from = held.lines.length;
        releaseCrossReady();
        const late = await held.next(
          (line) =>
            line.event === 'tools-changed' &&
            line.frameId === child.targetId &&
            names(line).includes('cross_late'),
          from
        );
        assert.ok(names(late).includes('cross_late'), JSON.stringify(late));
        const after = await once(wire.url, cwd, [
          'webmcp-list',
          '--tab',
          targetId,
          '--json',
          '--from-origin',
          foreignOrigin,
        ]);
        assert.equal(
          after.code,
          0,
          `list after gate failed\nlist stderr: ${after.stderr}\n${held.stderr()}`
        );
        assert.equal(
          after.stderr.includes('cross-origin child tools are unavailable'),
          false,
          `guarded-route refusal\nlist stderr: ${after.stderr}\n${held.stderr()}`
        );
        const tools = JSON.parse(after.stdout);
        assert.ok(
          tools.some((tool) => tool.name === 'initial'),
          `initial missing\n${held.stderr()}`
        );
        assert.ok(
          tools.some((tool) => tool.name === 'cross_late' && tool.frameId === child.targetId),
          `cross_late missing from ${JSON.stringify(tools)}\n${held.stderr()}`
        );
        const called = await once(wire.url, cwd, [
          'webmcp-call',
          'cross_late',
          '{}',
          '--tab',
          targetId,
          '--from-origin',
          foreignOrigin,
        ]);
        assert.equal(
          called.code,
          0,
          `call failed\ncall stderr: ${called.stderr}\n${held.stderr()}`
        );
        assert.equal(called.stdout.trim(), 'cross_late');
        await raw.send('Target.detachFromTarget', { sessionId: childSession });
        return child.targetId;
      };

      if (protectedFrames) {
        await navigate(raw, sessionId, `${ordinary}/ordinary-frames`);
        held = stream(wire.url, cwd, [
          'webmcp-polyfill',
          '--tab',
          targetId,
          '--reload',
          '--from-origin',
          foreignOrigin,
        ]);
        assert.equal((await held.next((line) => line.event === 'ready', 0)).mode, 'polyfill');
      } else {
        const ordinaryReady = held.next(
          (line) => line.event === 'document-ready' && line.loaderId !== document.loaderId,
          held.lines.length
        );
        await Promise.all([navigate(raw, sessionId, `${ordinary}/ordinary-frames`), ordinaryReady]);
      }
      const navigatedCross = await crossControl([cross.targetId]);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
      assert.equal(JSON.stringify(held.lines).includes('protected_tool'), false);
      held.child.stdin.end();
      assert.equal(await held.ended, 0, held.stderr());

      crossReadyOpen = false;
      held = stream(wire.url, cwd, [
        'webmcp-polyfill',
        '--tab',
        targetId,
        '--reload',
        '--from-origin',
        foreignOrigin,
      ]);
      const restarted = await held.next((line) => line.event === 'ready', 0);
      assert.equal(restarted.mode, 'polyfill');
      assert.equal(restarted.reloaded, true);
      await crossControl([cross.targetId, navigatedCross]);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
      held.child.stdin.end();
      assert.equal(await held.ended, 0, held.stderr());
      held = undefined;
    } catch (error) {
      throw new Error(
        `${error.message}\nholder: ${held?.stderr()}\nwire: ${JSON.stringify(wire?.snapshot())}`,
        { cause: error }
      );
    } finally {
      held?.child.kill('SIGKILL');
      watcher?.child.kill('SIGKILL');
      await wire?.close();
      await raw?.close();
      await browser?.close();
      await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
      await rm(cwd, { recursive: true, force: true });
    }
  });
}

await test('Chrome: lazy --from-origin installs after list; tools registered before install need a reload', {
  timeout: 180000,
}, async () => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-lazy-chrome-'));
  let browser;
  let raw;
  let held;
  let server;
  let wire;
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
        'subjectAltName=DNS:localhost,DNS:foreign.test',
      ],
      { stdio: 'ignore' }
    );
    let port;
    let ordinary;
    server = createServer(
      { key: await readFile(keyPath), cert: await readFile(certPath) },
      (req, res) => {
        const url = new URL(req.url, 'https://localhost');
        const headers = { 'content-type': 'text/html', 'permissions-policy': 'tools=*' };
        res.writeHead(200, headers);
        const tool = (name) =>
          `{name:${JSON.stringify(name)},description:${JSON.stringify(`${name} tool`)},inputSchema:{type:'object'},exposedTo:[${JSON.stringify(ordinary)}],annotations:{readOnly:true},execute:async()=>${JSON.stringify(name)}}`;
        if (url.pathname === '/cross-lazy') {
          const awaitShim = url.searchParams.has('await');
          res.end(`<!doctype html><script>
let gateResolve;
const gate = new Promise((resolve) => { gateResolve = resolve; });
window.openGate = () => gateResolve();
const early = ${tool('cross_early')};
const late = ${tool('cross_late')};
const awaitShim = ${awaitShim ? 'true' : 'false'};
if (awaitShim) {
  void (async () => {
    await new Promise((resolve) => setTimeout(resolve, 50));
    document.modelContext?.registerTool(early);
  })();
} else if (document.modelContext) {
  document.modelContext.registerTool(early);
}
gate.then(() => { document.modelContext?.registerTool(late); });
</script>`);
          return;
        }
        const childQuery = url.searchParams.has('reload') ? '?await=1' : '';
        res.end(`<!doctype html><title>lazy</title><iframe src="https://foreign.test:${port}/cross-lazy${childQuery}" allow="tools *"></iframe><script>
if (document.modelContext) document.modelContext.registerTool(${tool('initial')});
</script>`);
      }
    );
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    port = server.address().port;
    ordinary = `https://localhost:${port}`;
    const foreignOrigin = `https://foreign.test:${port}`;
    browser = await launchBrowser([
      '--ignore-certificate-errors',
      '--no-proxy-server',
      '--disable-features=WebMCP,WebMCPTesting',
      '--site-per-process',
      '--host-resolver-rules=MAP foreign.test 127.0.0.1',
    ]);
    wire = await webmcpWire(browser.wsUrl);
    raw = await observer(browser.wsUrl);
    const { targetId } = await raw.send('Target.createTarget', {
      url: 'about:blank',
      background: true,
    });
    const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
    await raw.send('Page.enable', {}, sessionId);
    await navigate(raw, sessionId, `${ordinary}/lazy`);
    held = stream(wire.url, cwd, ['webmcp-polyfill', '--tab', targetId, '--reload']);
    assert.equal((await held.next((line) => line.event === 'ready', 0)).mode, 'polyfill');
    const child = (await raw.send('Target.getTargets', {})).targetInfos.find(
      (target) =>
        target.type === 'iframe' && target.url === `https://foreign.test:${port}/cross-lazy`
    );
    assert.ok(child, 'lazy OOPIF was not created');
    const childSession = (
      await raw.send('Target.attachToTarget', { targetId: child.targetId, flatten: true })
    ).sessionId;
    assert.equal(await evaluate(raw, childSession, 'typeof document.modelContext'), 'undefined');
    const before = await once(wire.url, cwd, [
      'webmcp-list',
      '--tab',
      targetId,
      '--json',
      '--from-origin',
      foreignOrigin,
    ]);
    assert.equal(before.code, 0, `lazy list failed\n${before.stderr}\nholder: ${held.stderr()}`);
    assert.equal(
      before.stderr.includes('cross-origin child tools are unavailable'),
      false,
      before.stderr
    );
    const beforeTools = JSON.parse(before.stdout);
    assert.equal(
      beforeTools.some((tool) => tool.name === 'cross_early'),
      false,
      'tool registered before install must stay absent until reload'
    );
    assert.equal(
      beforeTools.some((tool) => tool.name === 'cross_late'),
      false
    );
    assert.equal(await evaluate(raw, childSession, 'typeof document.modelContext'), 'object');
    await evaluate(raw, childSession, 'openGate()');
    const after = await once(wire.url, cwd, [
      'webmcp-list',
      '--tab',
      targetId,
      '--json',
      '--from-origin',
      foreignOrigin,
    ]);
    assert.equal(
      after.code,
      0,
      `list after gate failed\n${after.stderr}\nholder: ${held.stderr()}`
    );
    const afterTools = JSON.parse(after.stdout);
    assert.ok(
      afterTools.some((tool) => tool.name === 'cross_late'),
      `cross_late missing from ${JSON.stringify(afterTools)}\nholder: ${held.stderr()}`
    );
    assert.equal(
      afterTools.some((tool) => tool.name === 'cross_early'),
      false,
      'pre-install registration stays missing until reload'
    );
    await raw.send('Target.detachFromTarget', { sessionId: childSession });
    const ready = held.next((line) => line.event === 'document-ready', held.lines.length);
    await Promise.all([navigate(raw, sessionId, `${ordinary}/lazy?reload=1`), ready]);
    await sleep(500);
    const reloaded = await once(wire.url, cwd, [
      'webmcp-list',
      '--tab',
      targetId,
      '--json',
      '--from-origin',
      foreignOrigin,
    ]);
    assert.equal(
      reloaded.code,
      0,
      `list after reload failed\n${reloaded.stderr}\nholder: ${held.stderr()}`
    );
    const reloadedTools = JSON.parse(reloaded.stdout);
    assert.ok(
      reloadedTools.some((tool) => tool.name === 'cross_early'),
      `cross_early missing after reload from ${JSON.stringify(reloadedTools)}\nholder: ${held.stderr()}`
    );
    held.child.stdin.end();
    assert.equal(await held.ended, 0, `holder exit\nholder: ${held.stderr()}`);
    held = undefined;
  } catch (error) {
    throw new Error(`${error.message}\nholder: ${held?.stderr()}`, { cause: error });
  } finally {
    held?.child.kill('SIGKILL');
    await wire?.close();
    await raw?.close();
    await browser?.close();
    await new Promise((resolve) => (server ? server.close(resolve) : resolve()));
    await rm(cwd, { recursive: true, force: true });
  }
});
