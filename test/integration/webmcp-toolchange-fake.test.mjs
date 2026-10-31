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

async function unavailableChild(backend, held, targetInfo) {
  const peer = [...backend.scripts.keys()].find(
    (session) => backend.sessions.get(session) === 'TAB'
  );
  const refused = new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      held.child.stderr.off('data', check);
      reject(new Error('child unavailable error did not arrive'));
    }, 5000);
    const check = () => {
      if (!held.stderr().includes('cross-origin child tools are unavailable')) return;
      clearTimeout(timer);
      held.child.stderr.off('data', check);
      resolve();
    };
    held.child.stderr.on('data', check);
    check();
  });
  backend.inject({
    method: 'Target.attachedToTarget',
    sessionId: peer,
    params: { sessionId: targetInfo.targetId, targetInfo, waitingForDebugger: true },
  });
  await refused;
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

test('fake: a live provisional child with opaque frame origin retains only its verified parent installer', async () => {
  await scenario(
    { children: [{ id: 'PROVISIONAL', origin: 'https://foreign.test', oopif: true }] },
    async ({ backend, held }) => {
      const from = backend.calls.length;
      await unavailableChild(backend, held, {
        targetId: 'PROVISIONAL',
        type: 'iframe',
        url: 'about:blank',
      });
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
      assert.equal(backend.pages.get('PROVISIONAL').document.modelContext, undefined);
      assert.doesNotMatch(held.stderr(), /target closed/);
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
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }],
    },
    async ({ backend, held }) => {
      await unavailableChild(backend, held, {
        targetId: 'SEVEN_CHILD',
        type: 'iframe',
        url: 'https://seven.sliccy.ai/',
      });
      assert.match(held.stderr(), /webmcp-polyfill: cross-origin child tools are unavailable/);
      await unavailableChild(backend, held, {
        targetId: 'CROSS',
        type: 'iframe',
        url: 'https://foreign.test/',
      });
      assert.equal(backend.pages.get('CROSS').document.modelContext, undefined);
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

test('fake: a change held back by a protected frame is reported only after the frame leaves', async () => {
  await scenario(
    {
      definitions: [{ name: 'top', execute: () => 'top' }],
      children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }],
    },
    async ({ backend, cwd, held }) => {
      await unavailableChild(backend, held, {
        targetId: 'CROSS',
        type: 'iframe',
        url: 'https://foreign.test/',
      });
      const sealed = {
        targetId: 'SEALED',
        type: 'iframe',
        parentFrameId: 'TAB',
        url: 'https://seven.sliccy.ai/',
      };
      backend.targets.push(sealed);
      const from = held.lines.length;
      backend.page.document.modelContext.registerTool({
        name: 'foreign',
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
      assert.equal(backend.pages.get('CROSS').document.modelContext, undefined);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    }
  );
});

test('fake: late protected frame evidence does not revoke a fresh ordinary frame navigation', async () => {
  await scenario(
    { children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }] },
    async ({ backend, held }) => {
      await unavailableChild(backend, held, {
        targetId: 'CROSS',
        type: 'iframe',
        url: 'https://foreign.test/',
      });
      const guard = [...backend.sessions.keys()].find(
        (session) => backend.sessions.get(session) === 'TAB' && !backend.scripts.has(session)
      );
      assert.ok(guard);
      backend.inject({
        method: 'Page.frameNavigated',
        sessionId: guard,
        params: {
          frame: {
            id: 'CROSS',
            parentId: 'TAB',
            loaderId: 'STALE',
            url: 'https://seven.sliccy.ai/',
            securityOrigin: 'https://seven.sliccy.ai',
          },
        },
      });
      await new Promise((resolve) => setTimeout(resolve, 500));
      assert.equal(backend.pages.get('CROSS').document.modelContext, undefined);
      assert.equal(JSON.stringify(held.lines).includes('seven.sliccy.ai'), false);
    }
  );
});

test('fake: refused navigation traces contain only sanitized frame evidence and event order', async () => {
  await scenario(
    { children: [{ id: 'CROSS', origin: 'https://foreign.test', allow: 'tools *', oopif: true }] },
    async ({ backend, held }) => {
      await unavailableChild(backend, held, {
        targetId: 'CROSS',
        type: 'iframe',
        url: 'https://foreign.test/',
      });
      const peer = [...backend.scripts.keys()].find(
        (session) => backend.sessions.get(session) === 'TAB'
      );
      const page = backend.pages.get('TAB');
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
        (session) => backend.sessions.get(session) === 'TAB' && !backend.scripts.has(session)
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
            type: 'page',
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
      assert.equal(reported.targetType, 'page');
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
