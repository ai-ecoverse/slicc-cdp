import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { kernelRunner } from './runners.mjs';
import { webmcpBackend } from './webmcp-backend.mjs';

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

function holder(endpoint, cwd, args = ['webmcp-polyfill', '--tab', 'TAB', '--reload']) {
  const child = spawn(binary, ['--cdp', endpoint, ...args], {
    cwd,
    env: { ...environment(cwd), SLICC_WEBMCP_TRACE: '1' },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const lines = [];
  const waiters = new Set();
  child.stdout.on('data', (bytes) => {
    stdout += bytes;
    const parts = stdout.split('\n');
    stdout = parts.pop();
    for (const part of parts) {
      const value = JSON.parse(part);
      lines.push(value);
      for (const waiter of [...waiters]) waiter();
    }
  });
  child.stderr.on('data', (bytes) => {
    stderr += bytes;
  });
  const ended = new Promise((resolve) => child.once('close', (code) => resolve(code)));
  const next = (predicate, from, timeout = 15000) =>
    new Promise((resolve, reject) => {
      const check = () => {
        const found = lines.slice(from).find(predicate);
        if (!found) return;
        clearTimeout(timer);
        waiters.delete(check);
        resolve(found);
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

function run(endpoint, cwd, args) {
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

const changed = (frameId) => (value) =>
  value.event === 'tools-changed' && value.frameId === frameId;
const names = (event) => event.tools.map((tool) => tool.name);

function pauseBlank(backend, frameId) {
  const peer = [...backend.scripts.keys()].find(
    (session) => backend.sessions.get(session) === 'TAB'
  );
  const frame = backend.pages.get(frameId);
  frame.blank = true;
  frame.loader = `PENDING-${frameId}`;
  backend.inject({
    method: 'Page.frameNavigated',
    sessionId: peer,
    params: {
      frame: {
        id: frame.id,
        parentId: 'TAB',
        loaderId: frame.loader,
        url: 'about:blank',
        securityOrigin: frame.origin,
      },
    },
  });
  backend.inject({
    method: 'Runtime.executionContextCreated',
    sessionId: peer,
    params: {
      context: {
        id: frame.main,
        uniqueId: `main-${frame.loader}`,
        origin: frame.origin,
        auxData: { frameId: frame.id, isDefault: true },
      },
    },
  });
  backend.inject({
    method: 'Debugger.scriptParsed',
    sessionId: peer,
    params: { scriptId: `${peer}-${frameId}`, executionContextId: frame.main },
  });
  backend.inject({
    method: 'Debugger.paused',
    sessionId: peer,
    params: {
      callFrames: [
        {
          callFrameId: `${peer}-${frameId}`,
          functionName: '__slicc_webmcp_gate',
          location: { scriptId: `${peer}-${frameId}`, lineNumber: 0, columnNumber: 0 },
        },
      ],
    },
  });
}

async function scenario(options, check, args) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-webmcp-toolchange-'));
  const backend = await webmcpBackend(options);
  const held = holder(backend.url, cwd, args);
  try {
    await held.next((value) => value.event === 'ready', 0);
    await check({ backend, cwd, held });
    held.child.stdin.end();
    assert.equal(await held.ended, 0, held.stderr());
    assert.ok(backend.calls.every((call) => call.method !== 'Runtime.addBinding'));
  } finally {
    held.child.kill('SIGKILL');
    await backend.close();
    await rm(cwd, { recursive: true, force: true });
  }
}

test('fake: an abandoned local child gate during an ordinary process swap preserves its parent installer', async () => {
  let armed = false;
  let swapped = false;
  const resumed = Promise.withResolvers();
  await scenario(
    {
      children: [{ id: 'SWAP', origin: 'https://ordinary.test' }],
      onCommand(message, pages, send) {
        if (
          armed &&
          !swapped &&
          message.method === 'Debugger.getScriptSource' &&
          message.params.scriptId.endsWith('-SWAP')
        ) {
          swapped = true;
          pages.get('SWAP').blank = false;
          pages.get('SWAP').origin = 'https://foreign.test';
          send({
            method: 'Runtime.executionContextDestroyed',
            sessionId: message.sessionId,
            params: { executionContextId: pages.get('SWAP').main },
          });
          send({
            method: 'Page.frameDetached',
            sessionId: message.sessionId,
            params: { frameId: 'SWAP', reason: 'swap' },
          });
        }
        if (swapped && message.method === 'Debugger.resume') resumed.resolve();
      },
    },
    async ({ backend, held }) => {
      armed = true;
      pauseBlank(backend, 'SWAP');
      await resumed.promise;
      assert.equal(swapped, true);
      const from = held.lines.length;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', from);
      backend.page.document.modelContext.registerTool({ name: 'after_swap', execute: () => 'ok' });
      const change = await held.next(changed('TAB'), from);
      assert.deepEqual(names(change), ['after_swap']);
    }
  );
});

test('fake: a live provisional child with opaque frame origin retains only its verified parent installer', async () => {
  let opaque = false;
  let liveBackend;
  const resumed = Promise.withResolvers();
  await scenario(
    {
      children: [{ id: 'PROVISIONAL', origin: 'https://ordinary.test' }],
      onCommand(message, pages, send, sessions) {
        if (
          opaque &&
          message.method === 'Page.getFrameTree' &&
          sessions.get(message.sessionId) === 'TAB'
        ) {
          const frame = (page) => ({
            id: page.id,
            url: `${page.origin}/`,
            securityOrigin: page.origin,
            loaderId: page.loader,
          });
          send({
            id: message.id,
            result: {
              frameTree: {
                frame: frame(pages.get('TAB')),
                childFrames: [
                  {
                    frame: {
                      ...frame(pages.get('PROVISIONAL')),
                      parentId: 'TAB',
                      url: 'about:blank',
                      securityOrigin: '://',
                    },
                  },
                ],
              },
            },
          });
          return true;
        }
        if (opaque && message.method === 'Debugger.resume') {
          opaque = false;
          liveBackend.abandonLocalGate('PROVISIONAL');
          resumed.resolve();
        }
      },
    },
    async ({ backend, held }) => {
      liveBackend = backend;
      const from = backend.calls.length;
      opaque = true;
      pauseBlank(backend, 'PROVISIONAL');
      await resumed.promise;
      assert.equal(
        backend.calls
          .slice(from)
          .some(
            (call) =>
              call.method === 'Debugger.evaluateOnCallFrame' &&
              call.params.expression.endsWith('true')
          ),
        false
      );
      assert.equal(
        backend.calls
          .slice(from)
          .some((call) => call.method === 'Page.removeScriptToEvaluateOnNewDocument'),
        false
      );
      const start = held.lines.length;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', start);
      backend.page.document.modelContext.registerTool({
        name: 'ordinary_recovered',
        execute: () => 'ok',
      });
      assert.deepEqual(names(await held.next(changed('TAB'), start)), ['ordinary_recovered']);
      assert.doesNotMatch(held.stderr(), /target closed/);
    }
  );
});

test('fake: a child gate abandoned while queued retains the parent document installer', async () => {
  let armed = false;
  let queued = false;
  let liveBackend;
  await scenario(
    {
      children: [{ id: 'QUEUED', origin: 'https://ordinary.test' }],
      onCommand(message, pages, send) {
        if (
          armed &&
          !queued &&
          message.method === 'Debugger.getScriptSource' &&
          message.params.scriptId.endsWith('-TAB')
        ) {
          queued = true;
          pauseBlank(liveBackend, 'QUEUED');
          liveBackend.abandonLocalGate('QUEUED');
          pages.get('QUEUED').blank = false;
          pages.get('QUEUED').origin = 'https://foreign.test';
          send({
            method: 'Runtime.executionContextDestroyed',
            sessionId: message.sessionId,
            params: { executionContextId: pages.get('QUEUED').main },
          });
          send({
            method: 'Page.frameDetached',
            sessionId: message.sessionId,
            params: { frameId: 'QUEUED', reason: 'swap' },
          });
        }
      },
    },
    async ({ backend, held }) => {
      liveBackend = backend;
      armed = true;
      const start = held.lines.length;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', start);
      assert.equal(queued, true);
      backend.page.document.modelContext.registerTool({ name: 'queued_swap', execute: () => 'ok' });
      assert.deepEqual(names(await held.next(changed('TAB'), start)), ['queued_swap']);
    }
  );
});

test('fake: an OOPIF installer response waiting on its pause is resumed before page load completes', async () => {
  await scenario(
    {
      installerWaitsForResume: true,
      children: [
        { id: 'LOAD_CHILD', origin: 'https://foreign.test', allow: 'tools *', oopif: true },
      ],
    },
    async ({ backend, held }) => {
      assert.equal(typeof backend.pages.get('LOAD_CHILD').document.modelContext, 'object');
      const start = held.lines.length;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', start);
      assert.ok(
        backend.calls.some(
          (call) =>
            call.method === 'Debugger.resume' &&
            backend.sessions.get(call.sessionId) === 'LOAD_CHILD'
        )
      );
      backend.pages.get('LOAD_CHILD').document.modelContext.registerTool({
        name: 'loaded_child',
        exposedTo: ['https://ordinary.test'],
        execute: () => 'ok',
      });
      assert.deepEqual(names(await held.next(changed('LOAD_CHILD'), start)), ['loaded_child']);
    }
  );
});

test('fake: a tool registered after load produces one debounced tools-changed event', async () => {
  await scenario(
    { definitions: [{ name: 'first', description: 'First', execute: () => 'first' }] },
    async ({ backend, cwd, held }) => {
      const from = held.lines.length;
      const api = backend.page.document.modelContext;
      api.registerTool({ name: 'second', description: 'Second', execute: () => 'second' });
      api.registerTool({ name: 'third', description: 'Third\u001b[2J', execute: () => 'third' });
      const event = await held.next(changed('TAB'), from);
      assert.equal(event.tab, 'TAB');
      assert.equal(event.target, 'TAB');
      assert.equal(event.loaderId, backend.page.loader);
      assert.deepEqual(names(event), ['first', 'second', 'third']);
      assert.equal(event.tools[2].description, 'Third\\x1b[2J');
      const listed = await run(backend.url, cwd, ['webmcp-list', '--tab', 'TAB', '--json']);
      assert.equal(listed.code, 0, listed.stderr);
      assert.deepEqual(event.tools, JSON.parse(listed.stdout));
      await new Promise((resolve) => setTimeout(resolve, 1500));
      assert.equal(held.lines.slice(from).filter(changed('TAB')).length, 1);
    }
  );
});

test('fake: an unregistered tool produces an updated list without it', async () => {
  const controller = new AbortController();
  await scenario(
    {
      definitions: [
        { name: 'kept', execute: () => 'kept' },
        { name: 'removed', execute: () => 'removed' },
      ],
    },
    async ({ backend, held }) => {
      const api = backend.page.document.modelContext;
      api.registerTool(
        { name: 'temporary', execute: () => 'temporary' },
        { signal: controller.signal }
      );
      const added = await held.next(changed('TAB'), 0);
      assert.deepEqual(names(added), ['kept', 'removed', 'temporary']);
      const from = held.lines.length;
      controller.abort();
      const removed = await held.next(changed('TAB'), from);
      assert.deepEqual(names(removed), ['kept', 'removed']);
      assert.equal(removed.loaderId, added.loaderId);
    }
  );
});

test('fake: a cross-origin iframe registering after load reports its frame and origin, never a protected child', async () => {
  await scenario(
    {
      definitions: [{ name: 'top', execute: () => 'top' }],
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *' }],
    },
    async ({ backend, cwd, held }) => {
      const peer = [...backend.sessions].find(([, target]) => target === 'TAB')[0];
      backend.inject({
        method: 'Target.attachedToTarget',
        sessionId: peer,
        params: {
          sessionId: 'SEVEN_CHILD',
          targetInfo: { targetId: 'SEVEN_CHILD', type: 'iframe', url: 'https://seven.sliccy.ai/' },
          waitingForDebugger: true,
        },
      });
      await new Promise((resolve) => setTimeout(resolve, 500));
      assert.match(held.stderr(), /webmcp-polyfill: child refused at target/);
      const from = held.lines.length;
      backend.pages.get('CROSS').document.modelContext.registerTool({
        name: 'foreign',
        description: 'Foreign',
        exposedTo: ['https://ordinary.test'],
        execute: () => 'foreign',
      });
      backend.pages.get('CROSS').document.modelContext.registerTool({
        name: 'hidden',
        exposedTo: ['https://other.test'],
        execute: () => 'hidden',
      });
      const event = await held.next(changed('CROSS'), from);
      assert.equal(event.origin, 'https://foreign.test');
      assert.equal(event.loaderId, backend.pages.get('CROSS').loader);
      assert.deepEqual(names(event), ['foreign']);
      const listed = await run(backend.url, cwd, [
        'webmcp-list',
        '--tab',
        'TAB',
        '--json',
        '--from-origin',
        'https://foreign.test',
      ]);
      assert.equal(listed.code, 0, listed.stderr);
      assert.deepEqual(
        event.tools,
        JSON.parse(listed.stdout).filter((tool) => tool.frameId === 'CROSS')
      );
      await new Promise((resolve) => setTimeout(resolve, 1500));
      assert.equal(
        held.lines.some(
          (line) =>
            line.event === 'tools-changed' &&
            (line.frameId !== 'CROSS' || line.target === 'SEVEN_CHILD')
        ),
        false
      );
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    }
  );
});

test('fake: a frame change while a frame document is gated is checked again, not refused', async () => {
  let reloaded = false;
  let gated = false;
  let raced = 0;
  await scenario(
    {
      definitions: [{ name: 'top', execute: () => 'top' }],
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *' }],
      onCommand(message, _pages, send) {
        if (message.method === 'Page.reload') reloaded = true;
        if (
          reloaded &&
          message.method === 'Debugger.getScriptSource' &&
          message.params.scriptId.endsWith('-CROSS')
        )
          gated = raced === 0;
        if (!gated || message.method !== 'Page.getFrameTree') return;
        gated = false;
        raced += 1;
        send({
          method: 'Page.frameDetached',
          params: { frameId: 'SWAPPED', reason: 'swap' },
          sessionId: message.sessionId,
        });
      },
    },
    async ({ backend, cwd, held }) => {
      assert.equal(raced, 1);
      const from = held.lines.length;
      backend.pages.get('CROSS').document.modelContext.registerTool({
        name: 'foreign',
        exposedTo: ['https://ordinary.test'],
        execute: () => 'foreign',
      });
      const event = await held.next(changed('CROSS'), from);
      assert.equal(event.loaderId, backend.pages.get('CROSS').loader);
      assert.deepEqual(names(event), ['foreign']);
      const listed = await run(backend.url, cwd, [
        'webmcp-list',
        '--tab',
        'TAB',
        '--json',
        '--from-origin',
        'https://foreign.test',
      ]);
      assert.equal(listed.code, 0, listed.stderr);
      assert.deepEqual(
        event.tools,
        JSON.parse(listed.stdout).filter((tool) => tool.frameId === 'CROSS')
      );
      assert.doesNotMatch(held.stderr(), /refused|target closed/);
    }
  );
});

test('fake: a change held back by a protected frame is reported only after the frame leaves', async () => {
  await scenario(
    {
      definitions: [{ name: 'top', execute: () => 'top' }],
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *' }],
    },
    async ({ backend, cwd, held }) => {
      const sealed = {
        targetId: 'SEALED',
        type: 'iframe',
        parentFrameId: 'TAB',
        url: 'https://seven.sliccy.ai/',
      };
      backend.targets.push(sealed);
      const from = held.lines.length;
      backend.pages.get('CROSS').document.modelContext.registerTool({
        name: 'foreign',
        exposedTo: ['https://ordinary.test'],
        execute: () => 'foreign',
      });
      await new Promise((resolve) => setTimeout(resolve, 1500));
      assert.deepEqual(
        held.lines.slice(from).filter((line) => line.event === 'tools-changed'),
        []
      );
      const locked = await run(backend.url, cwd, [
        'webmcp-list',
        '--tab',
        'TAB',
        '--json',
        '--from-origin',
        'https://foreign.test',
      ]);
      assert.notEqual(locked.code, 0);
      assert.equal(locked.stdout, '');
      backend.targets.splice(backend.targets.indexOf(sealed), 1);
      const event = await held.next(changed('CROSS'), from);
      assert.deepEqual(names(event), ['foreign']);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    }
  );
});

for (const reload of [false, true]) {
  test(`fake: a preloaded OOPIF survives frame-tree churn with reload=${reload}`, async () => {
    let armed = false;
    let guard;
    let raced = 0;
    await scenario(
      {
        children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }],
        onCommand(message, _pages, send, sessions) {
          if (message.method === 'Target.setAutoAttach') armed = true;
          if (
            message.method === 'Runtime.enable' &&
            sessions.get(message.sessionId) === 'CROSS' &&
            !guard
          )
            guard = message.sessionId;
          if (
            !armed ||
            raced >= 2 ||
            message.method !== 'Page.getFrameTree' ||
            message.sessionId !== guard
          )
            return;
          raced += 1;
          send({
            method: 'Page.frameDetached',
            params: { frameId: `SWAPPED${raced}`, reason: 'swap' },
            sessionId: message.sessionId,
          });
        },
      },
      async ({ backend, cwd, held }) => {
        assert.ok(
          backend.calls.some(
            (call) =>
              call.method === 'Page.addScriptToEvaluateOnNewDocument' &&
              backend.sessions.get(call.sessionId) === 'CROSS'
          ),
          held.stderr()
        );
        assert.equal(raced, 2);
        const check = async () => {
          const controller = new AbortController();
          let from = held.lines.length;
          backend.pages
            .get('CROSS')
            .document.modelContext.registerTool(
              { name: 'foreign', exposedTo: ['https://ordinary.test'], execute: () => 'foreign' },
              { signal: controller.signal }
            );
          const added = await held.next(changed('CROSS'), from);
          const list = async () => {
            const result = await run(backend.url, cwd, [
              'webmcp-list',
              '--tab',
              'TAB',
              '--json',
              '--from-origin',
              'https://foreign.test',
            ]);
            assert.equal(result.code, 0, result.stderr);
            return JSON.parse(result.stdout).filter((tool) => tool.frameId === 'CROSS');
          };
          assert.deepEqual(names(added), ['foreign']);
          assert.equal(added.target, 'CROSS');
          assert.equal(added.loaderId, backend.pages.get('CROSS').loader);
          assert.deepEqual(added.tools, await list());
          from = held.lines.length;
          controller.abort();
          const removed = await held.next(changed('CROSS'), from);
          assert.deepEqual(removed.tools, []);
          assert.deepEqual(await list(), []);
        };
        assert.equal(
          typeof backend.pages.get('CROSS').document.modelContext.registerTool,
          'function'
        );
        if (reload) await check();
        const from = held.lines.length;
        backend.reload();
        await held.next((line) => line.event === 'document-ready', from);
        await check();
        assert.doesNotMatch(held.stderr(), /child refused|target closed/);
      },
      ['webmcp-polyfill', '--tab', 'TAB', ...(reload ? ['--reload'] : [])]
    );
  });
}

test('fake: a blank child pause received before the parent resume reply can rearm for an ordinary document', async () => {
  const child = { id: 'CROSS', origin: 'https://ordinary.test', blank: true, allow: 'tools *' };
  await scenario(
    {
      children: [child],
      reentrantPauses: true,
      onCommand(message) {
        if (message.method === 'Page.reload') child.blank = false;
      },
    },
    async ({ backend, held }) => {
      const from = held.lines.length;
      backend.pages
        .get('CROSS')
        .document.modelContext.registerTool({ name: 'child', execute: () => 'child' });
      assert.deepEqual(names(await held.next(changed('CROSS'), from)), ['child']);
      assert.doesNotMatch(held.stderr(), /refused|target closed/);
    }
  );
});

test('fake: an OOPIF attachment received inside blank-frame rearm waits for the parent installer', async () => {
  await scenario(
    {
      attachDuringRearm: true,
      children: [
        { id: 'BLANK', origin: 'https://ordinary.test', allow: 'tools *' },
        {
          id: 'CROSS',
          origin: 'https://foreign.test',
          allow: 'tools *',
          oopif: true,
          definitions: [
            { name: 'resumed', exposedTo: ['https://ordinary.test'], execute: () => 'resumed' },
          ],
        },
      ],
    },
    async ({ backend, cwd, held }) => {
      backend.pages.get('CROSS').blank = true;
      pauseBlank(backend, 'BLANK');
      await held
        .next(
          (line) => line.event === 'tools-changed' && line.frameId === 'CROSS',
          held.lines.length
        )
        .catch((error) => {
          assert.fail(`child gate did not run: ${error.message}`);
        });
      const listed = await run(backend.url, cwd, [
        'webmcp-list',
        '--tab',
        'TAB',
        '--json',
        '--from-origin',
        'https://foreign.test',
      ]);
      assert.equal(listed.code, 0, listed.stderr);
      assert.deepEqual(
        JSON.parse(listed.stdout)
          .filter((tool) => tool.frameId === 'CROSS')
          .map((tool) => tool.name),
        ['resumed']
      );
      assert.doesNotMatch(held.stderr(), /child refused|target closed/);
    }
  );
});

test('fake: frame churn during blank-frame rearm preserves the holder and its next document installer', async () => {
  let rearming = false;
  let raced = false;
  let backend;
  await scenario(
    {
      children: [{ id: 'BLANK', origin: 'https://ordinary.test', allow: 'tools *' }],
      onCommand(message, _pages, send) {
        if (message.method === 'Page.removeScriptToEvaluateOnNewDocument') rearming = true;
        if (rearming && !raced && message.method === 'Page.getFrameTree') {
          raced = true;
          send({
            method: 'Page.frameDetached',
            params: { frameId: 'SWAPPED', reason: 'swap' },
            sessionId: message.sessionId,
          });
        }
        if (
          rearming &&
          message.method === 'Page.addScriptToEvaluateOnNewDocument' &&
          !message.params.runImmediately
        )
          backend.page.document.modelContext.registerTool({
            name: 'recovered',
            execute: () => 'recovered',
          });
      },
    },
    async (context) => {
      backend = context.backend;
      const { held } = context;
      const from = held.lines.length;
      pauseBlank(backend, 'BLANK');
      assert.deepEqual(names(await held.next(changed('TAB'), from)), ['recovered']);
      assert.equal(raced, true);
      const generation = held.lines.length;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', generation);
      const changedFrom = held.lines.length;
      backend.pages
        .get('BLANK')
        .document.modelContext.registerTool({ name: 'next_child', execute: () => 'next_child' });
      assert.deepEqual(names(await held.next(changed('BLANK'), changedFrom)), ['next_child']);
      assert.doesNotMatch(held.stderr(), /refused|target closed/);
    }
  );
});

test('fake: late protected frame evidence does not revoke a fresh ordinary frame navigation', async () => {
  let active = false;
  let raced = false;
  let backend;
  await scenario(
    {
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *' }],
      beforeNavigateFrame(peer, page, event, sessions) {
        if (!active || raced || page.id !== 'CROSS' || !backend.scripts.has(peer)) return;
        const guard = [...sessions.keys()].find(
          (session) => sessions.get(session) === 'TAB' && !backend.scripts.has(session)
        );
        assert.ok(guard);
        raced = true;
        event(
          'Page.frameNavigated',
          {
            frame: {
              id: page.id,
              parentId: 'TAB',
              loaderId: 'STALE',
              url: 'https://seven.sliccy.ai/',
              securityOrigin: 'https://seven.sliccy.ai',
            },
          },
          guard
        );
      },
    },
    async (context) => {
      backend = context.backend;
      const { held, cwd } = context;
      const from = held.lines.length;
      active = true;
      backend.reload();
      await held.next((line) => line.event === 'document-ready', from);
      assert.equal(raced, true);
      const start = held.lines.length;
      backend.pages.get('CROSS').document.modelContext.registerTool({
        name: 'fresh_cross',
        exposedTo: ['https://ordinary.test'],
        execute: () => 'fresh_cross',
      });
      const event = await held.next(changed('CROSS'), start);
      assert.equal(event.loaderId, backend.pages.get('CROSS').loader);
      assert.deepEqual(names(event), ['fresh_cross']);
      const listed = await run(backend.url, cwd, [
        'webmcp-list',
        '--tab',
        'TAB',
        '--json',
        '--from-origin',
        'https://foreign.test',
      ]);
      assert.equal(listed.code, 0, listed.stderr);
      assert.deepEqual(
        event.tools,
        JSON.parse(listed.stdout).filter((tool) => tool.frameId === 'CROSS')
      );
      assert.doesNotMatch(held.stderr(), /refused|target closed/);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    }
  );
});

for (const url of ['about:blank', '']) {
  test(`fake: a provisional OOPIF root navigation without parentId stays gated for ${url || 'empty URL'}`, async () => {
    let provisional = false;
    await scenario(
      {
        children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }],
        onCommand(message, pages, send, sessions) {
          if (
            provisional ||
            message.method !== 'Page.addScriptToEvaluateOnNewDocument' ||
            sessions.get(message.sessionId) !== 'CROSS'
          )
            return;
          provisional = true;
          const page = pages.get('CROSS');
          send({
            method: 'Page.frameNavigated',
            sessionId: message.sessionId,
            params: {
              frame: { id: page.id, loaderId: page.loader, url, securityOrigin: page.origin },
            },
          });
        },
      },
      async ({ backend, held }) => {
        assert.equal(provisional, true);
        const from = held.lines.length;
        backend.pages.get('CROSS').document.modelContext.registerTool({
          name: 'after_provisional',
          exposedTo: ['https://ordinary.test'],
          execute: () => 'after_provisional',
        });
        assert.deepEqual(names(await held.next(changed('CROSS'), from)), ['after_provisional']);
        assert.doesNotMatch(held.stderr(), /refused|target closed/);
      }
    );
  });
}

test('fake: refused navigation traces contain only sanitized frame evidence and event order', async () => {
  await scenario(
    { children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }] },
    async ({ backend, held }) => {
      const peer = [...backend.scripts.keys()].find(
        (session) => backend.sessions.get(session) === 'CROSS'
      );
      const page = backend.pages.get('CROSS');
      const trace = new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          held.child.stderr.off('data', check);
          reject(new Error('sanitized navigation trace did not arrive'));
        }, 5000);
        const check = () => {
          const line = held
            .stderr()
            .split('\n')
            .find(
              (line) =>
                line.startsWith('webmcp-polyfill: {') && line.includes('guard-navigation-trace')
            );
          if (!line) return;
          clearTimeout(timer);
          held.child.stderr.off('data', check);
          resolve(JSON.parse(line.slice('webmcp-polyfill: '.length)));
        };
        held.child.stderr.on('data', check);
        check();
      });
      const guard = [...backend.sessions.keys()].find(
        (session) => backend.sessions.get(session) === 'CROSS' && !backend.scripts.has(session)
      );
      backend.inject({
        method: 'Fetch.requestPaused',
        sessionId: guard,
        params: {
          requestId: 'PRIVATE-REQUEST',
          frameId: page.id,
          resourceType: 'Document',
          request: { url: `${page.origin}/private`, method: 'GET', headers: {} },
        },
      });
      backend.inject({
        method: 'Target.targetInfoChanged',
        sessionId: peer,
        params: {
          targetInfo: {
            targetId: page.id,
            type: 'iframe',
            parentFrameId: 'TAB',
            url: `${page.origin}/`,
          },
        },
      });
      backend.inject({
        method: 'Page.frameNavigated',
        sessionId: peer,
        params: {
          frame: {
            id: page.id,
            loaderId: page.loader,
            url: 'about:srcdoc#private',
            securityOrigin: page.origin,
            unreachableUrl: 'https://user:secret@foreign.test/private',
          },
        },
      });
      const reported = await trace;
      assert.equal(reported.eventFrame.scheme, 'about:srcdoc');
      assert.equal(reported.eventFrame.parentIdPresent, false);
      assert.equal(reported.eventFrame.unreachableUrlNonempty, true);
      assert.equal(reported.frameInGuardTree, true);
      assert.equal(reported.guardFrame.scheme, 'https');
      assert.equal(reported.loaderEqual, true);
      assert.equal(reported.targetType, 'iframe');
      assert.equal(reported.documentFetch.decision, 'continue');
      assert.equal(reported.documentFetch.errorReason, null);
      assert.equal(reported.documentFetch.destinationProtected, false);
      assert.equal(reported.unreachableProtected, false);
      assert.ok(reported.documentFetch.pauseSequence <= reported.documentFetch.decisionSequence);
      assert.equal(JSON.stringify(reported).includes('PRIVATE-REQUEST'), false);
      assert.ok(reported.lifecycleBefore.attachedToTarget < reported.sequence);
      assert.ok(reported.lifecycleBefore.targetInfoChanged < reported.sequence);
      assert.equal(JSON.stringify(reported).includes('private'), false);
      assert.equal(JSON.stringify(reported).includes('secret'), false);
      assert.equal(JSON.stringify(reported).includes('foreign.test'), false);
    }
  );
});

test('fake: a pending change is dropped when the document navigates, and the next document reports its own generation', async () => {
  let pending = false;
  let backend;
  await scenario(
    {
      definitions: [{ name: 'top', execute: () => 'top' }],
      onCommand(command) {
        if (!pending || command.method !== 'Page.createIsolatedWorld') return;
        pending = false;
        backend.reload();
      },
    },
    async (context) => {
      backend = context.backend;
      const { held } = context;
      const old = backend.page.loader;
      pending = true;
      backend.page.document.modelContext.registerTool({ name: 'stale', execute: () => 'stale' });
      const ready = await held.next((line) => line.event === 'document-ready', 0);
      assert.notEqual(ready.loaderId, old);
      assert.equal(pending, false);
      const from = held.lines.length;
      backend.page.document.modelContext.registerTool({ name: 'fresh', execute: () => 'fresh' });
      const event = await held.next(changed('TAB'), from);
      assert.equal(event.loaderId, ready.loaderId);
      assert.deepEqual(names(event), ['fresh', 'top']);
      assert.equal(
        held.lines.some((line) => line.event === 'tools-changed' && line.loaderId === old),
        false
      );
      assert.equal(JSON.stringify(held.lines).includes('stale'), false);
    }
  );
});

test('fake: native WebMCP changes are watched from the isolated world', async () => {
  await scenario(
    { native: true, definitions: [{ name: 'native', execute: () => 'native' }] },
    async ({ backend, held }) => {
      assert.equal(held.lines[0].mode, 'native');
      const from = held.lines.length;
      const page = backend.page;
      page.nativeTools.push({
        name: 'later',
        window: page.window,
        origin: page.origin,
        execute: () => 'later',
      });
      const event = await held.next(changed('TAB'), from);
      assert.deepEqual(names(event), ['later', 'native']);
      assert.equal(event.tools[0].policy, 'allowed');
      const evaluated = backend.calls.filter(
        (call) =>
          call.method === 'Runtime.evaluate' &&
          call.params.expression.includes("'n' + JSON.stringify")
      );
      assert.ok(evaluated.length > 0);
      assert.ok(
        evaluated.every((call) => call.params.uniqueContextId === `isolated-${page.loader}`)
      );
    }
  );
});

test('fake: the WASI holder reports a tool registered after load', async () => {
  const backend = await webmcpBackend({ definitions: [{ name: 'first', execute: () => 'first' }] });
  let runner;
  try {
    runner = await kernelRunner(
      path.join(root, 'target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.join(root, 'target/wasm32-wasip1/debug/curlwright.wasm'),
      backend.url
    );
    await runner.writeFile(
      '/node_modules/toolchange-holder/package.json',
      JSON.stringify({
        name: 'toolchange-holder',
        slicc: { commands: { 'toolchange-holder': { abi: 'js', module: 'holder.mjs' } } },
      })
    );
    await runner.writeFile(
      '/node_modules/toolchange-holder/holder.mjs',
      `export default async function (ctx) {
        const child = await ctx.spawn({ argv: ['playwright-cli', ...ctx.argv.slice(1)], stdin: 'pipe', stdout: 'pipe' });
        const input = child.stdin.getWriter();
        const output = child.stdout.getReader();
        const decoder = new TextDecoder();
        const stop = setTimeout(() => input.close().catch(() => {}), 20000);
        let text = '';
        for (;;) {
          const { value, done } = await output.read();
          if (done) break;
          await ctx.write(1, value);
          text += decoder.decode(value, { stream: true });
          if (text.includes('"tools-changed"')) {
            clearTimeout(stop);
            await input.close().catch(() => {});
          }
        }
        clearTimeout(stop);
        return (await child.wait()).status;
      }`
    );
    const cwd = await runner.fresh();
    let registered = false;
    const held = await runner.run(
      ['toolchange-holder', 'webmcp-polyfill', '--tab', 'TAB', '--reload'],
      cwd,
      {
        onStdout(text) {
          if (registered || !text.includes('"ready"')) return;
          registered = true;
          backend.page.document.modelContext.registerTool({
            name: 'second',
            execute: () => 'second',
          });
        },
      }
    );
    assert.equal(held.code, 0, held.stderr);
    const lines = held.stdout
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
    assert.equal(lines[0].event, 'ready');
    const event = lines.find((line) => line.event === 'tools-changed');
    assert.ok(event, held.stdout);
    assert.equal(event.frameId, 'TAB');
    assert.equal(event.loaderId, backend.page.loader);
    assert.deepEqual(names(event), ['first', 'second']);
    assert.equal(backend.scripts.size, 0);
    assert.ok(backend.calls.every((call) => call.method !== 'Runtime.addBinding'));
  } finally {
    await runner?.close();
    await backend.close();
  }
});

test('fake: discover reports holder tool changes only while the holder runs', async () => {
  await scenario(
    { definitions: [{ name: 'first', execute: () => 'first' }] },
    async ({ backend, cwd, held }) => {
      backend.page.document.modelContext.registerTool({ name: 'before', execute: () => 'before' });
      await held.next(changed('TAB'), 0);
      const watcher = holder(backend.url, cwd, ['discover', '--json', '--no-probe']);
      try {
        const attached = Date.now();
        while (
          [...backend.sessions.values()].filter((target) => target === 'TAB').length < 2 &&
          Date.now() - attached < 10000
        )
          await new Promise((resolve) => setTimeout(resolve, 50));
        await new Promise((resolve) => setTimeout(resolve, 1000));
        const from = held.lines.length;
        backend.page.document.modelContext.registerTool({
          name: 'second',
          description: 'see https://user:secret@ordinary.test/path?token=hidden',
          execute: () => 'second',
        });
        const change = await held.next(changed('TAB'), from);
        const reported = await watcher.next(
          (line) => line.standard === 'webmcp' && line.summary?.event === 'tools-changed',
          0
        );
        assert.equal(reported.targetId, 'TAB');
        assert.equal(reported.via, 'page-api');
        assert.equal(reported.origin, 'https://ordinary.test');
        assert.equal(reported.summary.frameId, 'TAB');
        assert.equal(reported.summary.loaderId, change.loaderId);
        assert.deepEqual(
          reported.summary.tools.map((tool) => tool.name),
          ['before', 'first', 'second']
        );
        assert.equal(
          watcher.lines.filter((line) => line.summary?.event === 'tools-changed').length,
          1
        );
        assert.equal(JSON.stringify(reported).includes('secret'), false);
        assert.equal(JSON.stringify(change).includes('secret'), false);
        held.child.stdin.end();
        assert.equal(await held.ended, 0, held.stderr());
        const count = watcher.lines.length;
        backend.page.document.modelContext.registerTool({ name: 'after', execute: () => 'after' });
        await new Promise((resolve) => setTimeout(resolve, 1500));
        assert.equal(watcher.lines.length, count);
      } finally {
        watcher.child.kill('SIGKILL');
        await watcher.ended;
      }
    }
  );
});
