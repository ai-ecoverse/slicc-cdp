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
  const pageEmitters = new Map();
  const backend = await fakeCdp({
    refuseWhen: options.refuseWhen,
    refusalMessage: options.refusalMessage,
    intercept: options.intercept
      ? (message, send, close) =>
          options.intercept(message, send, close, { emitters, pageEmitters, backend })
      : undefined,
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
    held = harHolder(
      backend.url,
      ['har-start', '--tab', 'TAB', ...(options.flags ?? []), file],
      home
    );
    await held.event('ready');
    await body({ home, backend, tab, file, held, emitters, pageEmitters });
  } finally {
    held?.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
}

async function admitWorker(backend, emitters, held, session = 'WORKER') {
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
  pageEmit('Target.attachedToTarget', {
    sessionId: session,
    targetInfo: {
      targetId: `${session}-TARGET`,
      type: 'worker',
      url: 'https://example.test/worker.js',
    },
    waitingForDebugger: true,
  });
  await waitFor(
    () =>
      backend.calls.some(
        (call) => call.method === 'Runtime.runIfWaitingForDebugger' && call.sessionId === session
      ),
    'worker runIfWaitingForDebugger'
  );
  const workerEmit = emitters.get(session);
  assert.ok(workerEmit, 'worker session emitter');
  return { pageSession, pageEmit, workerEmit };
}

function workerNetwork(workerEmit, id, url, options = {}) {
  const params = {
    requestId: id,
    loaderId: '',
    timestamp: 1,
    wallTime: 1700000000,
    documentURL: 'https://example.test/worker.js',
    type: 'Fetch',
    request: {
      url,
      method: 'GET',
      headers: options.headers ?? {},
      ...(options.headersText ? { headersText: options.headersText } : {}),
    },
  };
  if (options.initiator) params.initiator = options.initiator;
  if (options.redirectResponse) params.redirectResponse = options.redirectResponse;
  workerEmit('Network.requestWillBeSent', params);
  if (options.extraInfoBeforeResponse) {
    workerEmit('Network.requestWillBeSentExtraInfo', {
      requestId: id,
      headers: options.extraInfoBeforeResponse,
    });
  }
  workerEmit('Network.responseReceived', {
    requestId: id,
    timestamp: 1.1,
    response: {
      url,
      headers: {},
      status: 200,
      statusText: 'OK',
      mimeType: 'text/plain',
      encodedDataLength: 13,
    },
  });
  if (options.extraInfoAfterResponse) {
    workerEmit('Network.requestWillBeSentExtraInfo', {
      requestId: id,
      headers: options.extraInfoAfterResponse,
    });
  }
  if (options.responseExtraInfo) {
    workerEmit('Network.responseReceivedExtraInfo', {
      requestId: id,
      statusCode: 200,
      headers: options.responseExtraInfo,
    });
  }
  if (!options.skipLoadingFinished) {
    workerEmit('Network.loadingFinished', { requestId: id, timestamp: 1.2 });
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

test('HAR --content embed embeds ordinary worker bodies and omits protected or unrecorded ones', async () => {
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const { pageEmit, workerEmit } = await admitWorker(backend, emitters, held);
      workerNetwork(workerEmit, 'ORD', 'https://example.test/body');
      workerNetwork(workerEmit, 'BAD', 'https://seven.sliccy.ai/secret');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: tab.loaderId,
          url: tab.url,
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      let har = JSON.parse(await readFile(file, 'utf8'));
      const ordinary = har.log.entries.find(
        (entry) => entry.request.url === 'https://example.test/body'
      );
      assert.ok(ordinary, JSON.stringify(har));
      assert.equal(ordinary.response.content.text, 'ordinary-body');
      assert.equal(
        har.log.entries.filter((entry) => entry.request.url === 'https://seven.sliccy.ai/secret')
          .length,
        0
      );
      assert.deepEqual(
        backend.calls
          .filter((call) => call.method === 'Network.getResponseBody')
          .map((call) => call.params.requestId),
        ['ORD']
      );
      const beforeProtected = backend.calls.filter(
        (call) => call.method === 'Network.getResponseBody'
      ).length;
      tab.url = 'https://seven.sliccy.ai/private';
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: 'P',
          url: tab.url,
          securityOrigin: 'https://seven.sliccy.ai',
        },
      });
      await held.event('paused');
      workerNetwork(workerEmit, 'LATE', 'https://example.test/after-protected');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: 'P',
          url: tab.url,
          securityOrigin: 'https://seven.sliccy.ai',
        },
      });
      await new Promise((resolve) => setTimeout(resolve, 200));
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
      har = JSON.parse(await readFile(file, 'utf8'));
      assert.equal(
        har.log.entries.filter(
          (entry) => entry.request.url === 'https://example.test/after-protected'
        ).length,
        0,
        JSON.stringify(har)
      );
      assert.equal(
        backend.calls.filter((call) => call.method === 'Network.getResponseBody').length,
        beforeProtected
      );
    },
    { flags: ['--content', 'embed'] }
  );
});

test('HAR --content embed omits worker bodies with protected Referer, Origin, or initiator stack', async () => {
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const { pageEmit, workerEmit } = await admitWorker(backend, emitters, held);
      workerNetwork(workerEmit, 'REF', 'https://example.test/referer', {
        headers: { Referer: 'https://seven.sliccy.ai/secret' },
      });
      workerNetwork(workerEmit, 'ORG', 'https://example.test/origin', {
        headers: { Origin: 'https://seven.sliccy.ai' },
      });
      workerNetwork(workerEmit, 'STACK', 'https://example.test/stack', {
        initiator: {
          type: 'script',
          stack: {
            callFrames: [{ url: 'https://seven.sliccy.ai/x.js', lineNumber: 0, columnNumber: 0 }],
          },
        },
      });
      workerNetwork(workerEmit, 'OK', 'https://example.test/ok');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: tab.loaderId,
          url: tab.url,
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      const har = JSON.parse(await readFile(file, 'utf8'));
      const byUrl = Object.fromEntries(har.log.entries.map((entry) => [entry.request.url, entry]));
      assert.equal(byUrl['https://example.test/ok']?.response.content.text, 'ordinary-body');
      for (const url of [
        'https://example.test/referer',
        'https://example.test/origin',
        'https://example.test/stack',
      ]) {
        assert.equal(byUrl[url]?.response.content.text, undefined, url);
      }
      assert.deepEqual(
        backend.calls
          .filter((call) => call.method === 'Network.getResponseBody')
          .map((call) => call.params.requestId),
        ['OK']
      );
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
    },
    { flags: ['--content', 'embed'] }
  );
});

test('HAR --content embed omits worker bodies when protected Referer or Origin arrives only via ExtraInfo, redirectResponse.headers, or headersText', async () => {
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const { pageEmit, workerEmit } = await admitWorker(backend, emitters, held);
      workerNetwork(workerEmit, 'EXTRA', 'https://example.test/extra', {
        extraInfoAfterResponse: { Referer: 'https://seven.sliccy.ai/secret' },
      });
      workerNetwork(workerEmit, 'EXTRAORG', 'https://example.test/extra-origin', {
        extraInfoAfterResponse: { Origin: 'https://seven.sliccy.ai' },
      });
      workerNetwork(workerEmit, 'REDIRRESP', 'https://example.test/redirect-next', {
        redirectResponse: {
          url: 'https://example.test/redirect-start',
          status: 302,
          headers: { Referer: 'https://seven.sliccy.ai/secret' },
        },
      });
      workerNetwork(workerEmit, 'TEXT', 'https://example.test/headers-text', {
        headersText:
          'GET /headers-text HTTP/1.1\r\nReferer: https://seven.sliccy.ai/secret\r\n\r\n',
      });
      workerNetwork(workerEmit, 'OK', 'https://example.test/ok-shapes');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: tab.loaderId,
          url: tab.url,
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      const har = JSON.parse(await readFile(file, 'utf8'));
      const byUrl = Object.fromEntries(har.log.entries.map((entry) => [entry.request.url, entry]));
      assert.equal(byUrl['https://example.test/ok-shapes']?.response.content.text, 'ordinary-body');
      for (const url of [
        'https://example.test/extra',
        'https://example.test/extra-origin',
        'https://example.test/redirect-next',
        'https://example.test/headers-text',
      ]) {
        assert.equal(byUrl[url]?.response.content.text, undefined, url);
      }
      assert.deepEqual(
        backend.calls
          .filter((call) => call.method === 'Network.getResponseBody')
          .map((call) => call.params.requestId),
        ['OK']
      );
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
    },
    { flags: ['--content', 'embed'] }
  );
});

test('HAR --content embed refuses ExtraInfo that arrives after responseReceived and before loadingFinished', async () => {
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const { pageEmit, workerEmit } = await admitWorker(backend, emitters, held);
      workerNetwork(workerEmit, 'ORDER', 'https://example.test/order', {
        extraInfoAfterResponse: { Referer: 'https://seven.sliccy.ai/secret' },
      });
      workerNetwork(workerEmit, 'OK', 'https://example.test/order-ok');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: tab.loaderId,
          url: tab.url,
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      const har = JSON.parse(await readFile(file, 'utf8'));
      const byUrl = Object.fromEntries(har.log.entries.map((entry) => [entry.request.url, entry]));
      assert.equal(byUrl['https://example.test/order']?.response.content.text, undefined);
      assert.equal(byUrl['https://example.test/order-ok']?.response.content.text, 'ordinary-body');
      assert.deepEqual(
        backend.calls
          .filter((call) => call.method === 'Network.getResponseBody')
          .map((call) => call.params.requestId),
        ['OK']
      );
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
    },
    { flags: ['--content', 'embed'] }
  );
});

test('HAR --content embed drops a worker body if the creator flips during getResponseBody', async () => {
  let pageSession;
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const admitted = await admitWorker(backend, emitters, held);
      pageSession = admitted.pageSession;
      const { pageEmit, workerEmit } = admitted;
      workerNetwork(workerEmit, 'RACE', 'https://example.test/race');
      pageEmit('Page.frameNavigated', {
        frame: {
          id: 'TAB',
          loaderId: tab.loaderId,
          url: tab.url,
          securityOrigin: 'https://example.test',
        },
      });
      await held.event('checkpoint', (event) => event.entries >= 1);
      const har = JSON.parse(await readFile(file, 'utf8'));
      const entry = har.log.entries.find(
        (item) => item.request.url === 'https://example.test/race'
      );
      assert.ok(entry, JSON.stringify(har));
      assert.equal(entry.response.content.text, undefined, JSON.stringify(entry));
      assert.ok(
        backend.calls.some(
          (call) => call.method === 'Network.getResponseBody' && call.params.requestId === 'RACE'
        )
      );
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
    },
    {
      flags: ['--content', 'embed'],
      intercept(message, send) {
        if (message.method === 'Network.getResponseBody' && message.params?.requestId === 'RACE') {
          send({
            method: 'Page.frameNavigated',
            sessionId: pageSession,
            params: {
              frame: {
                id: 'TAB',
                loaderId: 'P',
                url: 'https://seven.sliccy.ai/',
                securityOrigin: 'https://seven.sliccy.ai',
              },
            },
          });
        }
        return false;
      },
    }
  );
});

const BLANK = { id: 'TAB', loaderId: 'BLANK', url: 'about:blank', securityOrigin: '://' };
const PAGE = ['https://example.test/page', 'NEXT'];

function frameOf(url, loaderId) {
  return { id: 'TAB', loaderId, url, securityOrigin: new URL(url).origin };
}

async function settle(predicate, ms) {
  const deadline = Date.now() + ms;
  while (!predicate() && Date.now() < deadline)
    await new Promise((resolve) => setTimeout(resolve, 5));
}

function pageDocument(event, url, loaderId) {
  return {
    start() {
      event('Network.requestWillBeSent', {
        requestId: 'DOC',
        loaderId,
        frameId: 'TAB',
        timestamp: 1,
        wallTime: 1700000000,
        documentURL: url,
        type: 'Document',
        request: { url, method: 'GET', headers: {} },
      });
      event('Network.responseReceived', {
        requestId: 'DOC',
        timestamp: 1.1,
        type: 'Document',
        response: { url, headers: {}, status: 200, statusText: 'OK', mimeType: 'text/html' },
      });
    },
    finish() {
      event('Network.loadingFinished', { requestId: 'DOC', timestamp: 1.2 });
    },
  };
}

function staleCheck(plan) {
  const state = { stage: 'idle', trees: [], injected: false, storm: false, checks: 0 };
  const at = plan.at ?? {};
  const inject = (emitters) => {
    if (state.injected) return;
    state.injected = true;
    workerNetwork(emitters.get('WORKER'), 'BODY', 'https://example.test/body');
  };
  const intercept = (message, send, close, { backend, emitters }) => {
    if (
      state.stage === 'drained' &&
      at.body === 'inject' &&
      message.method === 'Network.getResponseBody' &&
      message.params?.requestId === 'DOC'
    )
      inject(emitters);
    if (message.method !== 'Page.getFrameTree') return false;
    const tab = backend.targets.find((item) => item.targetId === 'TAB');
    const event = (method, params) => send({ method, sessionId: message.sessionId, params });
    const reply = (frame, delay = 0) =>
      setTimeout(
        () =>
          send({ id: message.id, sessionId: message.sessionId, result: { frameTree: { frame } } }),
        delay
      );
    if (state.storm) {
      if (state.trees.length % 2 === 0) {
        state.checks += 1;
        state.trees.push('stale');
        event('Page.frameNavigated', { frame: frameOf(...PAGE), type: 'Navigation' });
        reply(BLANK, 25);
      } else {
        const frame = plan.storm === 'verified' ? frameOf(...PAGE) : BLANK;
        state.trees.push(frame.url);
        reply(frame);
      }
      return true;
    }
    if (state.stage === 'armed') {
      state.stage = 'drained';
      state.trees.push('stale');
      if (plan.commit) {
        tab.url = plan.commit[0];
        tab.loaderId = plan.commit[1];
      }
      const document = plan.document && pageDocument(event, tab.url, tab.loaderId);
      document?.start();
      if (plan.navigate)
        event('Page.frameNavigated', { frame: frameOf(tab.url, tab.loaderId), type: 'Navigation' });
      document?.finish();
      reply(BLANK, 25);
      return true;
    }
    if (state.stage !== 'drained') return false;
    const step = at[state.trees.length] ?? {};
    if (step.flip) {
      tab.url = step.flip[0];
      tab.loaderId = step.flip[1];
    }
    if (step.inject) inject(emitters);
    if (step.reply) {
      state.trees.push(step.reply.url);
      reply(step.reply);
      return true;
    }
    state.trees.push(tab.url);
    return false;
  };
  return { state, intercept, inject };
}

async function staleRow(plan, check) {
  const { state, intercept, inject } = staleCheck(plan);
  await workerFixture(
    async ({ home, backend, tab, file, held, emitters }) => {
      const { pageEmit } = await admitWorker(backend, emitters, held);
      tab.url = 'about:blank';
      tab.loaderId = 'BLANK';
      let armedAt = held.events().length;
      let rounds;
      let early;
      if (plan.storm) {
        state.storm = true;
        await settle(
          () =>
            held
              .events()
              .slice(armedAt)
              .some((item) => item.event === 'paused'),
          1500
        );
        await new Promise((resolve) => setTimeout(resolve, 100));
        rounds = state.checks;
        state.storm = false;
        inject(emitters);
      } else {
        pageEmit('Page.frameNavigated', { frame: BLANK, type: 'Navigation' });
        await held.event('paused');
        armedAt = held.events().length;
        state.stage = 'armed';
        await waitFor(() => state.stage === 'drained', 'stale page check');
        if (plan.late) {
          if (plan.resumes) await held.event('resumed');
          await settle(() => state.trees.length >= 3, 100);
          await new Promise((resolve) => setTimeout(resolve, 30));
          early = state.trees.length;
          inject(emitters);
        }
      }
      await waitFor(() => state.injected, 'worker request');
      await new Promise((resolve) => setTimeout(resolve, 150));
      const stopped = await harCommand(backend.url, ['har-stop'], home);
      assert.equal(stopped.code, 0, stopped.stderr);
      assert.equal((await held.ended).code, 0);
      const har = JSON.parse(await readFile(file, 'utf8'));
      const states = held
        .events()
        .slice(armedAt)
        .filter((event) => event.event === 'paused' || event.event === 'resumed')
        .map((event) => event.event);
      const bodies = backend.calls
        .filter((call) => call.method === 'Network.getResponseBody')
        .map((call) => call.params.requestId);
      const urls = har.log.entries.map((entry) => entry.request.url);
      const body = har.log.entries.find(
        (entry) => entry.request.url === 'https://example.test/body'
      );
      const detail = JSON.stringify({ states, rounds, trees: state.trees.slice(0, 40), har });
      check({ states, bodies, body, urls, rounds, early, detail });
    },
    { flags: plan.flags ?? ['--content', 'embed'], intercept }
  );
}

test('a stale failed page check does not pause the holder or drop the worker fetch that follows the resume', async () => {
  await staleRow(
    { commit: PAGE, document: true, navigate: true, at: { 2: { inject: true } }, flags: [] },
    ({ states, bodies, body, urls, detail }) => {
      assert.deepEqual(states, ['resumed'], detail);
      assert.deepEqual(urls, ['https://example.test/page', 'https://example.test/body'], detail);
      assert.equal(body.response.content.text, undefined, detail);
      assert.deepEqual(bodies, [], detail);
    }
  );
});

test('a stale failed page check does not pause the holder or drop the worker body that follows the resume', async () => {
  await staleRow(
    { commit: PAGE, document: true, navigate: true, at: { body: 'inject' } },
    ({ states, bodies, body, urls, detail }) => {
      assert.deepEqual(states, ['resumed'], detail);
      assert.deepEqual(urls, ['https://example.test/page', 'https://example.test/body'], detail);
      assert.equal(body.response.content.text, 'ordinary-body', detail);
      assert.deepEqual(bodies, ['DOC', 'BODY'], detail);
    }
  );
});

test('a check made stale only by a verified main document is checked again before any pause', async () => {
  await staleRow(
    { commit: PAGE, document: true, navigate: false, at: { 1: { inject: true } } },
    ({ states, bodies, body, urls, detail }) => {
      assert.deepEqual(states, ['resumed'], detail);
      assert.deepEqual(urls, ['https://example.test/page', 'https://example.test/body'], detail);
      assert.equal(body.response.content.text, 'ordinary-body', detail);
      assert.deepEqual(bodies, ['BODY'], detail);
    }
  );
});

test('a check made stale only by a handled navigation is checked again before any pause', async () => {
  await staleRow(
    { commit: PAGE, navigate: true, at: { 1: { inject: true, reply: BLANK } } },
    ({ states, bodies, body, detail }) => {
      assert.deepEqual(states, ['resumed'], detail);
      assert.equal(body?.response.content.text, 'ordinary-body', detail);
      assert.deepEqual(bodies, ['BODY'], detail);
    }
  );
});

test('a check that is stale in every round pauses the holder after the last round', async () => {
  await staleRow({ storm: 'verified' }, ({ states, bodies, body, rounds, detail }) => {
    assert.equal(rounds, 4, detail);
    assert.deepEqual(states, ['paused'], detail);
    assert.equal(body, undefined, detail);
    assert.deepEqual(bodies, [], detail);
  });
});

test('a check kept stale by navigations that never verify stops after the last round and stays paused', async () => {
  await staleRow({ storm: 'unverified' }, ({ states, bodies, body, rounds, detail }) => {
    assert.equal(rounds, 4, detail);
    assert.deepEqual(states, ['paused'], detail);
    assert.equal(body, undefined, detail);
    assert.deepEqual(bodies, [], detail);
  });
});

test('a stale failed page check still pauses when the drain shows a protected page', async () => {
  await staleRow(
    { commit: ['https://seven.sliccy.ai/private', 'P'], navigate: true, late: true },
    ({ states, bodies, body, detail }) => {
      assert.ok(!states.includes('resumed'), detail);
      assert.equal(body, undefined, detail);
      assert.ok(!bodies.includes('BODY'), detail);
    }
  );
});

test('a page that turns protected between the drain and the re-check pauses the holder', async () => {
  await staleRow(
    {
      commit: PAGE,
      navigate: true,
      at: { 2: { flip: ['https://seven.sliccy.ai/private', 'P'] } },
      late: true,
      resumes: true,
    },
    ({ states, bodies, body, detail }) => {
      assert.deepEqual(states, ['resumed', 'paused'], detail);
      assert.equal(body, undefined, detail);
      assert.ok(!bodies.includes('BODY'), detail);
    }
  );
});

test('a failed page check with no state change keeps the holder paused and drops worker requests', async () => {
  await staleRow({ late: true }, ({ states, bodies, body, early, detail }) => {
    assert.deepEqual(states, [], detail);
    assert.equal(early, 1, detail);
    assert.equal(body, undefined, detail);
    assert.ok(!bodies.includes('BODY'), detail);
  });
});
