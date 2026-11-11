import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import './watchdog.mjs';

async function waitFor(predicate, label) {
  const deadline = Date.now() + 5000;
  for (;;) {
    if (predicate()) return;
    if (Date.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function workerFixture(body, options = {}) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-worker-'));
  const emitters = new Map();
  const backend = await fakeCdp({
    refuseWhen: options.refuseWhen,
    refusalMessage: options.refusalMessage,
    onCommand(message, emit) {
      if (message.sessionId) emitters.set(message.sessionId, emit);
    },
    answers: {
      'Network.getResponseBody': { body: 'ordinary-body', base64Encoded: false },
    },
  });
  const tab = { targetId: 'TAB', url: 'https://example.test/', loaderId: 'L' };
  backend.targets.push(tab);
  const file = path.join(home, 'traffic.har');
  let held;
  try {
    held = harHolder(backend.url, ['har-start', '--tab', 'TAB', file], home);
    await held.event('ready');
    await body({ home, backend, tab, file, held, emitters });
  } finally {
    held?.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
}

test('HAR admits a dedicated worker, enables Network, and records its fetch', async () => {
  await workerFixture(async ({ home, backend, file, held, emitters }) => {
    await waitFor(
      () =>
        backend.calls.some(
          (call) =>
            call.method === 'Target.setAutoAttach' &&
            call.params?.autoAttach === true &&
            call.params?.waitForDebuggerOnStart === true &&
            Array.isArray(call.params?.filter) &&
            call.params.filter.some((item) => item.type === 'worker' && item.exclude === false)
        ),
      'worker setAutoAttach'
    );
    const pageSession = backend.calls.find(
      (call) => call.method === 'Target.setAutoAttach' && call.params?.autoAttach === true
    )?.sessionId;
    assert.ok(
      pageSession,
      JSON.stringify(backend.calls.filter((call) => call.method === 'Target.setAutoAttach'))
    );
    const pageEmit = emitters.get(pageSession);
    assert.ok(pageEmit, `missing emitter for ${pageSession}`);
    const beforeAttach = backend.calls.length;
    pageEmit('Target.attachedToTarget', {
      sessionId: 'WORKER',
      targetInfo: {
        targetId: 'WORKER-TARGET',
        type: 'worker',
        url: 'https://example.test/worker.js',
      },
      waitingForDebugger: true,
    });
    await waitFor(
      () =>
        backend.calls.some(
          (call) => call.method === 'Runtime.runIfWaitingForDebugger' && call.sessionId === 'WORKER'
        ),
      'worker runIfWaitingForDebugger'
    );
    const afterAttach = backend.calls.slice(beforeAttach);
    const enableAt = afterAttach.findIndex(
      (call) => call.method === 'Network.enable' && call.sessionId === 'WORKER'
    );
    const resumeAt = afterAttach.findIndex(
      (call) => call.method === 'Runtime.runIfWaitingForDebugger' && call.sessionId === 'WORKER'
    );
    assert.ok(enableAt >= 0 && resumeAt >= 0 && enableAt < resumeAt, JSON.stringify(afterAttach));
    const workerEmit = emitters.get('WORKER');
    assert.ok(workerEmit, 'worker session emitter');
    workerEmit('Network.requestWillBeSent', {
      requestId: 'W1',
      loaderId: '',
      timestamp: 1,
      wallTime: 1700000000,
      documentURL: 'https://example.test/worker.js',
      type: 'Fetch',
      request: { url: 'https://example.test/from-worker', method: 'GET', headers: {} },
    });
    workerEmit('Network.responseReceived', {
      requestId: 'W1',
      timestamp: 1.1,
      response: {
        url: 'https://example.test/from-worker',
        headers: {},
        status: 200,
        statusText: 'OK',
        mimeType: 'text/plain',
        encodedDataLength: 13,
      },
    });
    workerEmit('Network.loadingFinished', { requestId: 'W1', timestamp: 1.2 });
    pageEmit('Page.frameNavigated', {
      frame: {
        id: 'TAB',
        loaderId: 'L',
        url: 'https://example.test/',
        securityOrigin: 'https://example.test',
      },
    });
    await held.event('checkpoint', (event) => event.entries >= 1);
    const har = JSON.parse(await readFile(file, 'utf8'));
    assert.ok(
      har.log.entries.some((entry) => entry.request.url === 'https://example.test/from-worker'),
      JSON.stringify(har)
    );
    const stopped = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
    assert.ok(
      backend.calls.some(
        (call) => call.method === 'Target.detachFromTarget' && call.params?.sessionId === 'WORKER'
      ),
      JSON.stringify(backend.calls.filter((call) => call.method === 'Target.detachFromTarget'))
    );
    assert.ok(
      backend.calls.some(
        (call) =>
          call.method === 'Target.setAutoAttach' &&
          call.params?.autoAttach === false &&
          call.sessionId === pageSession
      ),
      'auto-attach disabled on stop'
    );
  });
});

test('HAR detaches a worker of a protected page without Network.enable', async () => {
  await workerFixture(async ({ backend, emitters, held }) => {
    await waitFor(
      () => backend.calls.some((call) => call.method === 'Target.setAutoAttach'),
      'setAutoAttach'
    );
    const pageSession = backend.calls.find(
      (call) => call.method === 'Target.setAutoAttach' && call.params?.autoAttach === true
    )?.sessionId;
    const pageEmit = emitters.get(pageSession);
    const before = backend.calls.length;
    pageEmit('Page.frameNavigated', {
      frame: {
        id: 'TAB',
        loaderId: 'L',
        url: 'https://seven.sliccy.ai/',
        securityOrigin: 'https://seven.sliccy.ai',
      },
    });
    pageEmit('Target.attachedToTarget', {
      sessionId: 'PROTECTED-WORKER',
      targetInfo: {
        targetId: 'PROTECTED-WORKER-TARGET',
        type: 'worker',
        url: 'https://ordinary.test/worker.js',
      },
      waitingForDebugger: true,
    });
    await held.event('paused');
    await waitFor(
      () =>
        backend.calls
          .slice(before)
          .some(
            (call) =>
              call.method === 'Target.detachFromTarget' &&
              call.params?.sessionId === 'PROTECTED-WORKER'
          ),
      'detach protected worker'
    );
    assert.ok(
      !backend.calls
        .slice(before)
        .some((call) => call.method === 'Network.enable' && call.sessionId === 'PROTECTED-WORKER')
    );
    assert.ok(
      !backend.calls
        .slice(before)
        .some(
          (call) =>
            call.method === 'Runtime.runIfWaitingForDebugger' &&
            call.sessionId === 'PROTECTED-WORKER'
        )
    );
  });
});

test('HAR detaches a blob worker created by a protected page', async () => {
  await workerFixture(async ({ backend, emitters, held }) => {
    await waitFor(
      () => backend.calls.some((call) => call.method === 'Target.setAutoAttach'),
      'setAutoAttach'
    );
    const pageSession = backend.calls.find(
      (call) => call.method === 'Target.setAutoAttach' && call.params?.autoAttach === true
    )?.sessionId;
    const pageEmit = emitters.get(pageSession);
    const before = backend.calls.length;
    pageEmit('Page.frameNavigated', {
      frame: {
        id: 'TAB',
        loaderId: 'L',
        url: 'https://seven.sliccy.ai/',
        securityOrigin: 'https://seven.sliccy.ai',
      },
    });
    pageEmit('Target.attachedToTarget', {
      sessionId: 'BLOB-WORKER',
      targetInfo: {
        targetId: 'BLOB-WORKER-TARGET',
        type: 'worker',
        url: 'blob:https://seven.sliccy.ai/1',
      },
      waitingForDebugger: true,
    });
    await held.event('paused');
    await waitFor(
      () =>
        backend.calls
          .slice(before)
          .some(
            (call) =>
              call.method === 'Target.detachFromTarget' && call.params?.sessionId === 'BLOB-WORKER'
          ),
      'detach blob worker'
    );
    assert.ok(
      !backend.calls
        .slice(before)
        .some((call) => call.method === 'Network.enable' && call.sessionId === 'BLOB-WORKER')
    );
    assert.ok(
      !backend.calls
        .slice(before)
        .some(
          (call) =>
            call.method === 'Runtime.runIfWaitingForDebugger' && call.sessionId === 'BLOB-WORKER'
        )
    );
  });
});

test('after har-stop a newly created worker is never paused or stranded', async () => {
  await workerFixture(async ({ home, backend, emitters, held }) => {
    await waitFor(
      () =>
        backend.calls.some(
          (call) => call.method === 'Target.setAutoAttach' && call.params?.autoAttach === true
        ),
      'worker setAutoAttach'
    );
    const pageSession = backend.calls.find(
      (call) => call.method === 'Target.setAutoAttach' && call.params?.autoAttach === true
    )?.sessionId;
    const pageEmit = emitters.get(pageSession);
    assert.ok(pageEmit, `missing emitter for ${pageSession}`);
    const stopped = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
    assert.ok(
      backend.calls.some(
        (call) =>
          call.method === 'Target.setAutoAttach' &&
          call.params?.autoAttach === false &&
          call.sessionId === pageSession &&
          !(Array.isArray(call.params?.filter) && call.params.filter.length > 0)
      ),
      'auto-attach disabled without a filter'
    );
    const before = backend.calls.length;
    pageEmit('Target.attachedToTarget', {
      sessionId: 'LATE-WORKER',
      targetInfo: {
        targetId: 'LATE-WORKER-TARGET',
        type: 'worker',
        url: 'https://example.test/late.js',
      },
      waitingForDebugger: true,
    });
    await new Promise((resolve) => setTimeout(resolve, 200));
    const after = backend.calls.slice(before);
    assert.ok(
      !after.some((call) => call.method === 'Network.enable' && call.sessionId === 'LATE-WORKER'),
      JSON.stringify(after)
    );
    assert.ok(
      !after.some(
        (call) =>
          call.method === 'Runtime.runIfWaitingForDebugger' && call.sessionId === 'LATE-WORKER'
      ),
      JSON.stringify(after)
    );
  });
});

test('extension-style setAutoAttach refusal prints one notice and keeps page recording', async () => {
  await workerFixture(
    async ({ home, backend, file, held, emitters }) => {
      await waitFor(
        () => held.output().stderr.includes('dedicated worker traffic is not captured'),
        'worker notice'
      );
      assert.equal(
        held.output().stderr.split('dedicated worker traffic is not captured').length - 1,
        1
      );
      const pageSession = backend.calls.find(
        (call) => call.method === 'Target.setAutoAttach'
      )?.sessionId;
      const pageEmit = emitters.get(pageSession);
      assert.ok(pageEmit, `missing emitter for ${pageSession}`);
      pageEmit('Network.requestWillBeSent', {
        requestId: 'P1',
        loaderId: 'L',
        timestamp: 1,
        wallTime: 1700000000,
        documentURL: 'https://example.test/',
        type: 'Fetch',
        request: { url: 'https://example.test/page-api', method: 'GET', headers: {} },
      });
      pageEmit('Network.responseReceived', {
        requestId: 'P1',
        timestamp: 1.1,
        response: {
          url: 'https://example.test/page-api',
          headers: {},
          status: 200,
          statusText: 'OK',
          mimeType: 'text/plain',
          encodedDataLength: 13,
        },
      });
      pageEmit('Network.loadingFinished', { requestId: 'P1', timestamp: 1.2 });
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: 'L',
          url: 'https://example.test/',
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      const har = JSON.parse(await readFile(file, 'utf8'));
      assert.equal(har.log.entries[0].request.url, 'https://example.test/page-api');
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
    },
    {
      refuseWhen: (message) =>
        message.method === 'Target.setAutoAttach' && message.params?.autoAttach === true,
      refusalMessage:
        'slicc: refused: Target.setAutoAttach is not supported: attach to each target',
    }
  );
});
