import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
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
    await held.event('checkpoint', (event) => event.entries === count);
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
