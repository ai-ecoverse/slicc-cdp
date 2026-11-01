import assert from 'node:assert/strict';
import { X509Certificate } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const fixture = (name) => path.resolve('test/integration/fixtures/tls', name);
const cert = readFileSync(fixture('cert.pem'));
const key = readFileSync(fixture('key.pem'));
const fingerprint = new X509Certificate(cert).fingerprint256;
const ENV = 'SLICC_CDP_CERT_SHA256';

async function withPort(pin, body) {
  let upgrades = 0;
  const fake = await fakeCdp({
    tls: { cert, key },
    version: true,
    onUpgrade: () => (upgrades += 1),
  });
  fake.targets.push({ targetId: 'TAB', url: 'http://example.test/' });
  const previous = process.env[ENV];
  if (pin === undefined) delete process.env[ENV];
  else process.env[ENV] = pin;
  const pw = nativeRunner(exe('playwright-cli'), exe('curlwright'), fake.url);
  try {
    const cwd = await pw.fresh();
    await body((args) => pw.playwright(args, cwd), { fake, upgrades: () => upgrades });
  } finally {
    if (previous === undefined) delete process.env[ENV];
    else process.env[ENV] = previous;
    await pw.close();
    await fake.close();
  }
}

for (const [name, pin] of [
  ['colons', fingerprint],
  ['bare hex', fingerprint.replaceAll(':', '').toLowerCase()],
]) {
  test(`a matching pin in ${name} reaches a self-signed port`, async () => {
    await withPort(pin, async (run) => {
      const result = await run(['tab-list']);
      assert.equal(result.code, 0, result.stderr);
      assert.match(result.stdout, /example\.test/);
    });
  });
}

test('discovery over https takes the same pin', async () => {
  await withPort(fingerprint, async (run, { fake }) => {
    const https = fake.url.replace('wss://', 'https://').replace(/\/devtools.*$/, '');
    const result = await run(['--cdp', https, 'tab-list']);
    assert.equal(result.code, 0, result.stderr);
  });
});

test('a pin that does not match sends nothing, so the token in the path stays private', async () => {
  await withPort('00'.repeat(32), async (run, { upgrades }) => {
    const result = await run(['tab-list']);
    assert.notEqual(result.code, 0);
    assert.match(result.stderr, /doesn't match SLICC_CDP_CERT_SHA256/);
    assert.equal(upgrades(), 0);
  });
});

test('without a pin a self-signed port fails verification', async () => {
  await withPort(undefined, async (run, { upgrades }) => {
    const result = await run(['tab-list']);
    assert.notEqual(result.code, 0);
    assert.match(result.stderr, /TLS handshake with 127\.0\.0\.1 failed/);
    assert.equal(upgrades(), 0);
  });
});

test('a malformed pin is an error, not an unpinned connection', async () => {
  await withPort('not-a-fingerprint', async (run, { upgrades }) => {
    const result = await run(['tab-list']);
    assert.notEqual(result.code, 0);
    assert.match(result.stderr, /SLICC_CDP_CERT_SHA256 must be the 64 hex digits/);
    assert.equal(upgrades(), 0);
  });
});
