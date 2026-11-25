import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { harCommand, harHolder } from './har-support.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

const ORIGINS = ['Embedded in certificate', 'TLS extension', 'OCSP response'];
const FRESH = 41;
const REUSED = 29;
const PROTECTED = 'https://seven.sliccy.ai/private';

function details(origins) {
  return {
    protocol: 'TLS 1.3',
    keyExchange: '',
    keyExchangeGroup: 'X25519MLKEM768',
    cipher: 'AES_128_GCM',
    certificateId: 0,
    subjectName: '*.example.test',
    sanList: ['*.example.test', 'example.test'],
    issuer: 'Ordinary CA',
    validFrom: 1700000000,
    validTo: 1800000000,
    signedCertificateTimestampList: origins.map((origin) => ({
      status: 'Verified',
      origin,
      logDescription: 'Ordinary log',
      logId: '00',
      timestamp: 1700000000000,
      hashAlgorithm: 'SHA-256',
      signatureAlgorithm: 'ECDSA',
      signatureData: '00',
    })),
    certificateTransparencyCompliance: 'compliant',
    encryptedClientHello: false,
  };
}

function traffic() {
  const rows = [];
  for (let index = 0; index < FRESH + REUSED; index++)
    rows.push({
      id: `R${index}`,
      url: `https://example.test/api/${index}`,
      details: index < FRESH ? details(ORIGINS.slice(index % 2, (index % 2) + 2)) : undefined,
    });
  const refused = (id, change) => {
    const value = details(ORIGINS);
    change(value);
    rows.push({ id, url: `https://example.test/refused/${id}`, details: value, refused: true });
  };
  refused('URL_AT_ORIGIN', (value) => {
    value.signedCertificateTimestampList[0].origin = PROTECTED;
  });
  refused('UNKNOWN_ORIGIN', (value) => {
    value.signedCertificateTimestampList[1].origin = 'Unknown origin';
  });
  refused('DETAILS_URL', (value) => {
    value.origin = PROTECTED;
  });
  return rows;
}

function replay(send, rows) {
  for (const row of rows) {
    send('Network.requestWillBeSent', {
      requestId: row.id,
      loaderId: 'L',
      timestamp: 1,
      wallTime: 1700000000,
      documentURL: 'https://example.test/',
      type: 'Fetch',
      request: { url: row.url, method: 'GET', headers: {} },
    });
    send('Network.responseReceived', {
      requestId: row.id,
      loaderId: 'L',
      timestamp: 1.1,
      type: 'Fetch',
      response: {
        url: row.url,
        status: 200,
        statusText: '',
        headers: { 'content-type': 'application/json' },
        mimeType: 'application/json',
        encodedDataLength: 0,
        protocol: 'h2',
        securityState: 'secure',
        ...(row.details ? { securityDetails: row.details } : {}),
      },
    });
    send('Network.loadingFinished', {
      requestId: row.id,
      timestamp: 1.2,
      encodedDataLength: 120,
    });
  }
  send('Page.frameNavigated', {
    frame: {
      id: 'TAB',
      loaderId: 'L',
      url: 'https://example.test/',
      securityOrigin: 'https://example.test',
    },
  });
}

function assertRecorded(har, rows) {
  const urls = har.log.entries.map((entry) => entry.request.url);
  const admitted = rows.filter((row) => !row.refused).map((row) => row.url);
  assert.equal(admitted.length, FRESH + REUSED);
  assert.deepEqual(urls, admitted);
  for (const row of rows.filter((row) => row.refused)) assert.ok(!urls.includes(row.url), row.id);
  assert.ok(!JSON.stringify(har).includes('seven.sliccy.ai'));
}

async function backend() {
  const emitters = new Map();
  const cdp = await fakeCdp({
    onCommand(message, emit) {
      if (message.sessionId) emitters.set(message.sessionId, emit);
    },
  });
  cdp.targets.push({ targetId: 'TAB', url: 'https://example.test/', loaderId: 'L' });
  const send = (method, params) => {
    for (const emit of emitters.values()) emit(method, params);
  };
  return { cdp, send };
}

async function until(what, check, ms = 60000) {
  const end = Date.now() + ms;
  for (;;) {
    const found = await check();
    if (found) return found;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

test('HAR keeps responses that arrive on a new TLS connection and still drops protected ones', async () => {
  const home = await mkdtemp(path.join(tmpdir(), 'slicc-har-tls-'));
  const { cdp, send } = await backend();
  const file = path.join(home, 'traffic.har');
  let held;
  try {
    held = harHolder(cdp.url, ['har-start', '--tab', 'TAB', file], home);
    await held.event('ready');
    const rows = traffic();
    replay(send, rows);
    await held.event('checkpoint', (event) => event.entries === FRESH + REUSED);
    const stopped = await harCommand(cdp.url, ['har-stop'], home);
    assert.equal(stopped.code, 0, stopped.stderr);
    assert.equal((await held.ended).code, 0);
    assertRecorded(JSON.parse(await readFile(file, 'utf8')), rows);
  } finally {
    held?.child.kill('SIGKILL');
    await cdp.close();
    await rm(home, { recursive: true, force: true });
  }
});

test('the kernel HAR holder keeps responses that arrive on a new TLS connection and still drops protected ones', async () => {
  const { cdp, send } = await backend();
  let runner;
  try {
    runner = await kernelRunner(
      path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
      path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
      cdp.url
    );
    await runner.install('@ai-ecoverse/wasm-bash');
    await runner.install('@ai-ecoverse/wasm-coreutils');
    const cwd = await runner.fresh();
    const log = async () =>
      new TextDecoder()
        .decode(await runner.readFile('/tmp/tls.log').catch(() => new Uint8Array()))
        .split('\n')
        .filter((line) => line.startsWith('{'))
        .map(JSON.parse);
    const started = await runner.run(
      [
        'bash',
        '-c',
        '(while sleep 5; do echo || break; done) | playwright-cli -s tls har-start --tab TAB /tmp/tls.har > /tmp/tls.log 2>&1 &',
      ],
      cwd
    );
    assert.equal(started.code, 0, started.stderr);
    await until('ready', async () => (await log()).find((event) => event.event === 'ready'));
    const rows = traffic();
    replay(send, rows);
    await until('checkpoint', async () =>
      (await log()).find(
        (event) => event.event === 'checkpoint' && event.entries === FRESH + REUSED
      )
    );
    const stopped = await runner.run(['bash', '-c', 'playwright-cli -s tls har-stop'], cwd);
    assert.equal(stopped.code, 0, stopped.stderr);
    assertRecorded(
      JSON.parse(new TextDecoder().decode(await runner.readFile('/tmp/tls.har'))),
      rows
    );
  } finally {
    runner?.close();
    await cdp.close();
  }
});
