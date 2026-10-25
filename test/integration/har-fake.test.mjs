import assert from 'node:assert/strict';
import { globSync, writeFileSync } from 'node:fs';
import { chmod, mkdir, mkdtemp, readFile, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import './watchdog.mjs';

export async function harFixture(body, options = {}) {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-'));
  const emitters = new Map();
  const backend = await fakeCdp({
    refuseWhen: options.refuseWhen,
    intercept: options.intercept,
    onCommand(message, emit) {
      if (message.sessionId) emitters.set(message.sessionId, emit);
    },
    answers: {
      'Network.getResponseBody': options.body ?? { body: 'ordinary-body', base64Encoded: false },
    },
  });
  const tab = { targetId: 'TAB', url: 'https://example.test/', loaderId: 'L' };
  backend.targets.push(tab);
  const file = path.join(home, 'traffic.har');
  let held;
  const start = async (flags = []) => {
    held = harHolder(backend.url, ['har-start', '--tab', 'TAB', ...flags, file], home);
    await held.event('ready');
    return held;
  };
  const send = (method, params) => {
    for (const [session, emit] of emitters) {
      if (
        !backend.calls.some(
          (call) => call.method === 'Target.detachFromTarget' && call.params.sessionId === session
        )
      )
        emit(method, params);
    }
  };
  const request = (id, url, headers = {}, extra = {}) =>
    send('Network.requestWillBeSent', {
      requestId: id,
      loaderId: 'L',
      timestamp: 1,
      wallTime: 1700000000,
      documentURL: tab.url,
      type: 'Fetch',
      request: { url, method: 'GET', headers },
      ...extra,
    });
  const response = (id, url, headers = {}, status = 200) => {
    send('Network.responseReceived', {
      requestId: id,
      timestamp: 1.1,
      response: {
        url,
        headers,
        status,
        statusText: 'OK',
        mimeType: 'text/plain',
        encodedDataLength: 13,
      },
    });
    send('Network.loadingFinished', { requestId: id, timestamp: 1.2 });
  };
  const checkpoint = async (count) => {
    send('Page.frameNavigated', {
      frame: { id: 'TAB', loaderId: 'L', url: tab.url, securityOrigin: new URL(tab.url).origin },
    });
    await held.event('checkpoint', (event) => count === undefined || event.entries === count);
    return JSON.parse(await readFile(file, 'utf8'));
  };
  try {
    await body({
      home,
      backend,
      tab,
      file,
      start,
      send,
      request,
      response,
      checkpoint,
      holder: () => held,
    });
  } finally {
    held?.child.kill('SIGKILL');
    await backend.close();
    await rm(home, { recursive: true, force: true });
  }
}

test('HAR stop retains traffic absorbed while finalizing another completed request', async () => {
  let finish;
  let inject = false;
  await harFixture(
    async ({ home, file, start, request, response, send }) => {
      const held = await start(['--content', 'embed']);
      const ready = await held.event('ready');
      finish = () => {
        const [record] = globSync(`${home}/**/*.har.json`);
        assert.ok(record);
        writeFileSync(record.replace(/\.json$/, '.stop'), ready.recordingId);
        request('TAIL', 'https://example.test/tail');
        response('TAIL', 'https://example.test/tail');
        send('Network.requestWillBeSent', {
          requestId: 'DENIED',
          loaderId: 'L',
          documentURL: 'https://vault.sliccy.ai/',
          type: 'Fetch',
          request: { url: 'https://example.test/protected-tail', method: 'GET', headers: {} },
        });
        response('DENIED', 'https://example.test/protected-tail');
      };
      request('FIRST', 'https://example.test/first');
      inject = true;
      response('FIRST', 'https://example.test/first');
      const ended = await held.ended;
      assert.equal(ended.code, 0, ended.stderr);
      assert.equal(held.events().at(-1).reason, 'stop');
      const har = JSON.parse(await readFile(file, 'utf8'));
      assert.deepEqual(
        har.log.entries.map((entry) => entry.request.url),
        ['https://example.test/first', 'https://example.test/tail']
      );
    },
    {
      intercept(message) {
        if (inject && message.method === 'Network.getResponseBody') {
          inject = false;
          finish();
        }
        return false;
      },
    }
  );
});

test('HAR captures ordinary traffic, atomically checkpoints, strips credentials and omits bodies', async () => {
  await harFixture(async ({ home, backend, file, start, request, response, checkpoint }) => {
    const held = await start();
    const ready = await held.event('ready');
    assert.match(ready.recordingId, /^rec-/);
    assert.equal(ready.path, file);
    const other = await harCommand(backend.url, ['-s=other', 'har-stop'], home);
    assert.equal(other.code, 1);
    assert.match(other.stderr, /no HAR recording/);
    const again = await harCommand(backend.url, ['har-start', file], home);
    assert.equal(again.code, 1);
    assert.match(again.stderr, /already in progress/);
    request('R', 'https://example.test/api?q=one', {
      Cookie: 'credential-cookie',
      Authorization: 'credential-authorization',
      'Proxy-Authorization': 'credential-proxy',
      Accept: 'text/plain',
    });
    response('R', 'https://example.test/api?q=one', {
      'Set-Cookie': 'credential-set-cookie',
      'X-OK': 'yes',
    });
    const har = await checkpoint(1);
    assert.equal(har.log.version, '1.2');
    assert.equal(har.log.entries.length, 1);
    const entry = har.log.entries[0];
    assert.equal(entry.startedDateTime, '2023-11-14T22:13:20.000Z');
    assert.deepEqual(entry.request.queryString, [{ name: 'q', value: 'one' }]);
    assert.deepEqual(entry.request.cookies, []);
    assert.deepEqual(entry.response.cookies, []);
    assert.equal(entry.response.content.text, undefined);
    assert.equal(entry.request.postData, undefined);
    assert.ok(!JSON.stringify(har).includes('credential-'));
    assert.ok(
      !backend.calls.some(
        (call) =>
          call.method === 'Network.getResponseBody' || call.method === 'Network.getRequestPostData'
      )
    );
    const wrong = await harCommand(backend.url, ['stop-recording', 'wrong'], home);
    assert.equal(wrong.code, 1);
    assert.match(wrong.stderr, /does not belong/);
    const stopped = await harCommand(backend.url, ['stop-recording', ready.recordingId], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal(JSON.parse(stopped.stdout).recordingId, ready.recordingId);
    assert.equal((await held.ended).code, 0);
    assert.equal((await harCommand(backend.url, ['har-stop'], home)).code, 1);
  });
});

test('HAR rejects protected and unknown URLs and bodies, redacts redirect carriers and resumes after protected navigation', async () => {
  await harFixture(async ({ home, backend, tab, start, send, request, response, checkpoint }) => {
    const held = await start(['--content', 'embed', '--include-credentials']);
    request('OK', 'https://example.test/public', { Cookie: 'ordinary-cookie' });
    response('OK', 'https://example.test/public');
    for (const [index, url] of [
      'https://seven.sliccy.ai/private?secret=seven-secret',
      'https://vault.sliccy.ai/private?secret=vault-secret',
      'file:///private/unknown',
      'unknown://private/secret',
      'not-a-url',
    ].entries()) {
      request(`BAD${index}`, url);
      response(`BAD${index}`, url, { 'Set-Cookie': 'protected-cookie' });
    }
    request('REDIRECT', 'https://example.test/redirect');
    send('Fetch.requestPaused', {
      requestId: 'FREDIRECT',
      networkId: 'REDIRECT',
      resourceType: 'Fetch',
      frameId: 'TAB',
      request: { url: 'https://example.test/redirect', method: 'GET', headers: {} },
      responseStatusCode: 302,
      responseHeaders: [
        { name: 'Location', value: 'https://seven.sliccy.ai/private?redirect-secret=1' },
        { name: 'Refresh', value: '0;url=https://vault.sliccy.ai/private?redirect-secret=1' },
        { name: 'Content-Location', value: 'https://vault.sliccy.ai/private?redirect-secret=1' },
        { name: 'Link', value: '<https://seven.sliccy.ai/private?redirect-secret=1>; rel=preload' },
      ],
    });
    send('Network.requestWillBeSent', {
      requestId: 'REDIRECT',
      timestamp: 1.1,
      wallTime: 1700000000,
      loaderId: 'L',
      documentURL: tab.url,
      type: 'Fetch',
      request: {
        url: 'https://seven.sliccy.ai/private?redirect-secret=1',
        method: 'GET',
        headers: {},
      },
      redirectResponse: {
        url: 'https://example.test/redirect',
        status: 302,
        headers: {
          Location: 'https://seven.sliccy.ai/private?redirect-secret=1',
          Refresh: '0; url=https://vault.sliccy.ai/private?redirect-secret=1',
          'Content-Location': 'https://vault.sliccy.ai/private?redirect-secret=1',
          Link: '<https://seven.sliccy.ai/private?redirect-secret=1>; rel=preload',
        },
      },
    });
    const har = await checkpoint(2);
    assert.equal(har.log.entries[0].response.content.text, 'ordinary-body');
    assert.ok(
      har.log.entries[0].request.headers.some((header) => header.value === 'ordinary-cookie')
    );
    const redirect = har.log.entries.find((entry) => entry.response.status === 302);
    assert.equal(redirect.request.url, 'https://example.test/redirect');
    assert.equal(redirect.response.redirectURL, 'https://seven.sliccy.ai');
    const carriers = Object.fromEntries(
      redirect.response.headers.map((header) => [header.name.toLowerCase(), header.value])
    );
    assert.equal(carriers.refresh, '0;url=https://vault.sliccy.ai');
    assert.equal(carriers['content-location'], 'https://vault.sliccy.ai');
    assert.equal(carriers.link, '<https://seven.sliccy.ai>; rel=preload');
    assert.ok(!JSON.stringify(har).includes('redirect-secret'));
    assert.ok(!JSON.stringify(har).includes('protected-cookie'));
    assert.ok(!JSON.stringify(har).includes('seven-secret'));
    assert.ok(!JSON.stringify(har).includes('vault-secret'));
    const bodyCalls = backend.calls.filter((call) => call.method === 'Network.getResponseBody');
    assert.deepEqual(
      bodyCalls.map((call) => call.params.requestId),
      ['OK']
    );
    tab.url = 'https://seven.sliccy.ai/private';
    send('Page.frameNavigated', {
      frame: { id: 'TAB', loaderId: 'P', url: tab.url, securityOrigin: new URL(tab.url).origin },
    });
    await held.event('paused');
    request('FROM_PROTECTED', 'https://example.test/leak');
    response('FROM_PROTECTED', 'https://example.test/leak');
    tab.url = 'https://example.test/back';
    await held.event('resumed');
    request('BACK', 'https://example.test/back-fetch');
    response('BACK', 'https://example.test/back-fetch');
    const after = await checkpoint(3);
    assert.ok(!JSON.stringify(after).includes('/leak'));
    const stopped = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
  });
});

test('a paused holder answers the old connection guard while it reattaches on a fresh one', async () => {
  const sockets = [];
  const guards = new Map();
  const answers = new Map();
  let held;
  let paused;
  const answered = new Promise((resolve) => {
    paused = resolve;
  });
  const pause = (send, sessionId, requestId, url) =>
    send({
      method: 'Fetch.requestPaused',
      sessionId,
      params: {
        requestId,
        networkId: requestId,
        frameId: 'TAB',
        resourceType: 'Document',
        request: { url, method: 'GET', headers: {} },
      },
    });
  await harFixture(
    async ({ tab, start, send }) => {
      const holder = await start();
      tab.url = 'https://seven.sliccy.ai/private';
      send('Page.frameNavigated', {
        frame: { id: 'TAB', loaderId: 'P', url: tab.url, securityOrigin: new URL(tab.url).origin },
      });
      await holder.event('paused');
      tab.url = 'https://example.test/back';
      const settled = await Promise.race([
        answered.then(() => 'answered'),
        new Promise((resolve) => setTimeout(() => resolve('stalled'), 8000)),
      ]);
      assert.equal(settled, 'answered', JSON.stringify([...answers]));
      assert.deepEqual(Object.fromEntries(answers), {
        'OLD-ORDINARY': { method: 'Fetch.continueRequest' },
        'OLD-PROTECTED': { method: 'Fetch.failRequest', errorReason: 'BlockedByClient' },
      });
      await holder.event('resumed');
      holder.child.stdin.end();
      assert.equal((await holder.ended).code, 0);
    },
    {
      intercept(message, send) {
        if (!sockets.includes(send)) sockets.push(send);
        const connection = sockets.indexOf(send);
        if (
          message.method === 'Fetch.enable' &&
          message.params.patterns?.some((pattern) => pattern.resourceType === 'Document')
        )
          guards.set(connection, message.sessionId);
        if (
          connection === 0 &&
          ['Fetch.continueRequest', 'Fetch.failRequest'].includes(message.method) &&
          message.params.requestId?.startsWith('OLD-')
        ) {
          answers.set(message.params.requestId, {
            method: message.method,
            ...(message.params.errorReason ? { errorReason: message.params.errorReason } : {}),
          });
          if (answers.size === 2) {
            paused();
            held?.();
          }
          return false;
        }
        if (
          connection === 1 &&
          !held &&
          message.method === 'Network.enable' &&
          message.sessionId === guards.get(1)
        ) {
          held = () => send({ id: message.id, sessionId: message.sessionId, result: {} });
          pause(sockets[0], guards.get(0), 'OLD-ORDINARY', 'https://example.test/back');
          pause(
            sockets[0],
            guards.get(0),
            'OLD-PROTECTED',
            'https://seven.sliccy.ai/private?old-secret=1'
          );
          return true;
        }
        return false;
      },
    }
  );
});

test('HAR admits only the ordinary main document ending a pause and omits unverified candidates', async () => {
  await harFixture(async ({ home, backend, tab, start, send, request, response, checkpoint }) => {
    tab.url = 'about:blank';
    const held = await start(['--content=embed']);
    await held.event('paused');
    request('PAUSED_FETCH', 'https://example.test/not-recorded');
    response('PAUSED_FETCH', 'https://example.test/not-recorded');
    request(
      'CHILD_DOCUMENT',
      'https://example.test/child',
      {},
      { type: 'Document', frameId: 'CHILD', documentURL: 'https://example.test/child' }
    );
    response('CHILD_DOCUMENT', 'https://example.test/child');
    request(
      'UNVERIFIED_DOCUMENT',
      'https://example.test/unverified',
      {},
      {
        type: 'Document',
        frameId: 'TAB',
        documentURL: 'https://example.test/unverified',
        loaderId: 'UNVERIFIED',
      }
    );
    response('UNVERIFIED_DOCUMENT', 'https://example.test/unverified');
    await checkpoint(0);
    request(
      'DOCUMENT',
      'https://example.test/page',
      {},
      { type: 'Document', frameId: 'TAB', documentURL: 'https://example.test/page', loaderId: 'L' }
    );
    tab.url = 'https://example.test/page';
    send('Page.frameNavigated', {
      frame: { id: 'TAB', loaderId: 'L', url: tab.url, securityOrigin: 'https://example.test' },
    });
    await held.event('resumed');
    response('DOCUMENT', 'https://example.test/page');
    const har = await checkpoint(1);
    assert.equal(har.log.entries[0].request.url, tab.url);
    assert.equal(har.log.entries[0].response.content.text, 'ordinary-body');
    assert.ok(!JSON.stringify(har).includes('not-recorded'));
    assert.ok(!JSON.stringify(har).includes('/child'));
    assert.ok(!JSON.stringify(har).includes('/unverified'));
    assert.deepEqual(
      backend.calls
        .filter((call) => call.method === 'Network.getResponseBody')
        .map((call) => call.params.requestId),
      ['DOCUMENT']
    );
    const stopped = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
  });
});

test('HAR keeps the main document of the resumed loader when its loading completes before the commit', async () => {
  await harFixture(async ({ home, backend, tab, file, start, send, request, response }) => {
    tab.url = 'about:blank';
    tab.loaderId = 'BLANK';
    const held = await start();
    await held.event('paused');
    const trees = () => backend.calls.filter((call) => call.method === 'Page.getFrameTree').length;
    const absorbed = async () => {
      const from = trees();
      while (trees() < from + 2) await new Promise((resolve) => setTimeout(resolve, 10));
    };
    const document = (id, url, loaderId, extra = {}) =>
      request(
        id,
        url,
        {},
        { type: 'Document', frameId: 'TAB', documentURL: url, loaderId, ...extra }
      );
    document('OTHER', 'https://example.test/unverified', 'OTHER');
    response('OTHER', 'https://example.test/unverified');
    document('DOCUMENT', 'https://example.test/start', 'NEXT');
    await absorbed();
    document('DOCUMENT', 'https://example.test/page', 'NEXT', {
      redirectResponse: {
        url: 'https://example.test/start',
        status: 302,
        statusText: 'Found',
        headers: { Location: 'https://example.test/page' },
      },
    });
    response('DOCUMENT', 'https://example.test/page');
    await absorbed();
    const before = held.events().filter((event) => event.event === 'checkpoint').length;
    tab.url = 'https://example.test/page';
    tab.loaderId = 'NEXT';
    send('Page.frameNavigated', {
      frame: { id: 'TAB', loaderId: 'NEXT', url: tab.url, securityOrigin: 'https://example.test' },
    });
    await held.event('resumed');
    await held.event(
      'checkpoint',
      () => held.events().filter((event) => event.event === 'checkpoint').length > before
    );
    const stopped = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
    const har = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(
      har.log.entries.map((entry) => [entry.request.url, entry.response.status]),
      [
        ['https://example.test/start', 302],
        ['https://example.test/page', 200],
      ],
      JSON.stringify(har)
    );
    assert.ok(!JSON.stringify(har).includes('/unverified'));
    assert.ok(!backend.calls.some((call) => call.method === 'Network.getResponseBody'));
  });
});

test('completed ordinary traffic survives a later protected navigation without reading its body', async () => {
  await harFixture(async ({ backend, tab, file, start, request, response, send }) => {
    const held = await start(['--content=embed']);
    request('COMPLETED_BEFORE_NAV', 'https://example.test/completed-before-nav');
    response('COMPLETED_BEFORE_NAV', 'https://example.test/completed-before-nav');
    tab.url = 'https://seven.sliccy.ai/private';
    send('Page.frameNavigated', {
      frame: {
        id: 'TAB',
        loaderId: 'PROTECTED',
        url: tab.url,
        securityOrigin: 'https://seven.sliccy.ai',
      },
    });
    await held.event('paused');
    const har = JSON.parse(await readFile(file, 'utf8'));
    assert.equal(har.log.entries.length, 1, JSON.stringify(har));
    assert.equal(har.log.entries[0].request.url, 'https://example.test/completed-before-nav');
    assert.equal(har.log.entries[0].response.content.text, undefined);
    assert.ok(!backend.calls.some((call) => call.method === 'Network.getResponseBody'));
    held.child.stdin.end();
    assert.equal((await held.ended).code, 0);
  });
});

test('HAR retains ordinary redirect sources delivered through Network extra-info before a guarded failure', async () => {
  await harFixture(async ({ home, backend, start, request, send, checkpoint }) => {
    const held = await start(['--content=embed']);
    request('REDIRECT_EXTRA', 'https://example.test/extra-redirect', {
      Cookie: 'ordinary-credential',
    });
    send('Network.responseReceivedExtraInfo', {
      requestId: 'REDIRECT_EXTRA',
      statusCode: 302,
      headers: {
        Location: 'https://seven.sliccy.ai/private?extra-secret=1',
        'Set-Cookie': 'ordinary-response-credential',
      },
    });
    send('Network.loadingFailed', {
      requestId: 'REDIRECT_EXTRA',
      timestamp: 1.1,
      errorText: 'net::ERR_BLOCKED_BY_CLIENT',
      blockedReason: 'inspector',
    });
    request('REDIRECT_FETCH_LATE', 'https://example.test/late-fetch');
    send('Network.loadingFailed', {
      requestId: 'REDIRECT_FETCH_LATE',
      timestamp: 1.1,
      errorText: 'net::ERR_BLOCKED_BY_CLIENT',
      blockedReason: 'inspector',
    });
    send('Fetch.requestPaused', {
      requestId: 'FETCH_LATE',
      networkId: 'REDIRECT_FETCH_LATE',
      frameId: 'TAB',
      resourceType: 'Fetch',
      request: { url: 'https://example.test/late-fetch', method: 'GET', headers: {} },
      responseStatusCode: 302,
      responseHeaders: [
        { name: 'Location', value: 'https://seven.sliccy.ai/private?late-secret=1' },
      ],
    });
    const har = await checkpoint();
    assert.equal(har.log.entries.length, 2, JSON.stringify(har));
    assert.ok(
      har.log.entries.some((entry) => entry.request.url === 'https://example.test/late-fetch')
    );
    assert.ok(!JSON.stringify(har).includes('late-secret'));
    assert.ok(
      backend.calls.some(
        (call) =>
          call.method === 'Fetch.enable' &&
          call.params.patterns?.some((pattern) => pattern.requestStage === 'Response')
      )
    );
    const entry = har.log.entries[0];
    assert.equal(entry.request.url, 'https://example.test/extra-redirect');
    assert.equal(entry.response.status, 302);
    assert.equal(entry.response.redirectURL, 'https://seven.sliccy.ai');
    assert.ok(!JSON.stringify(har).includes('extra-secret'));
    assert.ok(!JSON.stringify(har).includes('ordinary-credential'));
    assert.ok(!JSON.stringify(har).includes('ordinary-response-credential'));
    assert.ok(!backend.calls.some((call) => call.method === 'Network.getResponseBody'));
    const result = await harCommand(backend.url, ['har-stop'], home);
    assert.equal(result.code, 0, result.stderr);
    assert.equal((await held.ended).code, 0);
  });
});

test('HAR output preserves shared directory permissions and makes only the archive private', async () => {
  await harFixture(async ({ home, backend }) => {
    const parent = path.join(home, 'shared');
    const directory = path.join(parent, 'public');
    await mkdir(directory, { recursive: true });
    await chmod(parent, 0o755);
    await chmod(directory, 0o755);
    const file = path.join(directory, 'traffic.har');
    const result = await harCommand(backend.url, ['har-start', '--tab', 'TAB', file], home);
    assert.equal(result.code, 0, result.stderr);
    if (process.platform !== 'win32') {
      assert.equal((await stat(parent)).mode & 0o777, 0o755);
      assert.equal((await stat(directory)).mode & 0o777, 0o755);
      assert.equal((await stat(file)).mode & 0o777, 0o600);
    }
    assert.equal(JSON.parse(await readFile(file, 'utf8')).log.version, '1.2');
  });
});

test('a stale killed HAR holder is reclaimed automatically', async () => {
  await harFixture(async ({ start, home, backend, file }) => {
    const first = await start();
    first.child.kill('SIGKILL');
    await first.ended;
    await new Promise((resolve) => setTimeout(resolve, 5200));
    const next = await harCommand(backend.url, ['har-start', '--tab', 'TAB', file], home);
    assert.equal(next.code, 0, next.stderr);
    assert.equal(JSON.parse(next.stdout.trim().split('\n').at(-1)).reason, 'stdin');
  });
});

test('slow HAR setup cannot steal an active holder lease after the stale interval', async () => {
  let entered;
  let release;
  const waiting = new Promise((resolve) => {
    entered = resolve;
  });
  let intercepted = false;
  await harFixture(
    async ({ home, backend, file }) => {
      const slow = harHolder(
        backend.url,
        ['har-start', '--tab', 'TAB', path.join(home, 'slow.har')],
        home
      );
      let fast;
      try {
        await waiting;
        await new Promise((resolve) => setTimeout(resolve, 5200));
        fast = harHolder(backend.url, ['har-start', '--tab', 'TAB', file], home);
        await fast.event('ready');
        release();
        slow.child.stdin.end();
        const rejected = await slow.ended;
        assert.equal(rejected.code, 1, rejected.stdout);
        assert.match(rejected.stderr, /already in progress/);
        const stopped = await harCommand(backend.url, ['har-stop'], home);
        assert.equal(stopped.code, 0, stopped.stderr);
        assert.equal((await fast.ended).code, 0);
      } finally {
        slow.child.kill('SIGKILL');
        fast?.child.kill('SIGKILL');
      }
    },
    {
      intercept(message, send) {
        if (
          !intercepted &&
          message.method === 'Fetch.enable' &&
          message.params.patterns?.some((pattern) => pattern.requestStage === 'Response')
        ) {
          intercepted = true;
          release = () => send({ id: message.id, sessionId: message.sessionId, result: {} });
          entered();
          return true;
        }
        return false;
      },
    }
  );
});

test('HAR refuses protected and unclassifiable selected tabs before any body method reaches CDP', async () => {
  await harFixture(async ({ home, backend, file }) => {
    const ordinary = await harCommand(backend.url, ['har-start', '--tab', 'TAB', file], home);
    assert.equal(ordinary.code, 0, ordinary.stderr);
    for (const [index, url] of [
      'https://seven.sliccy.ai/private',
      'https://vault.sliccy.ai/private',
      'file:///private',
      'unclassifiable',
    ].entries()) {
      const targetId = `REFUSED${index}`;
      backend.targets.push({ targetId, url });
      const result = await harCommand(
        backend.url,
        ['har-start', '--tab', targetId, '--content=embed', '--include-credentials', file],
        home
      );
      assert.equal(result.code, 1, url);
      assert.ok(
        !backend.calls.some((call) =>
          /getResponseBody|getRequestPostData|getResourceContent/.test(call.method)
        )
      );
    }
  });
});

test('HAR validates its SLICC options before any browser connection', async () => {
  await harFixture(async ({ home, backend }) => {
    for (const [args, pattern] of [
      [['har-start'], /requires <file>/],
      [['har-start', '--content', 'attach', 'x.har'], /omit or embed/],
      [['har-start', '--mode', 'bad', 'x.har'], /full or minimal/],
      [['record', '--filter=(entry) => true'], /URL glob, not arbitrary JavaScript/],
      [['record', 'https://seven.sliccy.ai/private'], /ordinary http\(s\)/],
      [['record', '--filter={broken'], /URL glob/],
    ]) {
      const result = await harCommand(backend.url, args, home);
      assert.equal(result.code, 1, args.join(' '));
      assert.match(result.stderr, pattern);
    }
    assert.equal(backend.calls.length, 0);
    for (const name of ['har-start', 'har-stop', 'record', 'stop-recording']) {
      const result = await harCommand(backend.url, [name, '--help'], home);
      assert.equal(result.code, 0);
      assert.match(result.stdout, /HAR/);
    }
  });
});
