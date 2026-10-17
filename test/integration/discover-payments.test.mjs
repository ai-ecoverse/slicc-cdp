import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');

const b64 = (value) => Buffer.from(JSON.stringify(value)).toString('base64');
const b64url = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');

const BIG = '123456789012345678901234567890';

const V2 = {
  x402Version: 2,
  resource: { url: 'https://paid.example/v2' },
  accepts: [
    {
      scheme: 'exact',
      network: 'eip155:8453',
      amount: BIG,
      asset: '0xUSDC',
      payTo: '0xPAY',
      maxTimeoutSeconds: 60,
    },
  ],
};

const V1 = {
  x402Version: 1,
  accepts: [
    {
      scheme: 'exact',
      network: 'base-sepolia',
      maxAmountRequired: '10000',
      asset: '0xUSDC',
      payTo: '0xV1',
      resource: 'https://paid.example/v1',
      maxTimeoutSeconds: 30,
    },
  ],
};

const MPP = `Payment id="c1", realm="shop", method="tempo", intent="charge", request="${b64url({ amount: '0.50', currency: 'usd' })}", expires="2030-01-01T00:00:00Z"`;

function parseEvents(stdout) {
  return stdout
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

async function settle(p) {
  try {
    await p;
  } catch {
    return undefined;
  }
}

function listen(server) {
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () => resolve(server.address().port))
  );
}

test('discover HTTP 402 payment challenges', { timeout: 120_000 }, async (t) => {
  const hits = new Map();
  const seenHeaders = [];
  const count = (key) => hits.set(key, (hits.get(key) ?? 0) + 1);
  let vaultHits = 0;
  const vault = createServer((req, res) => {
    vaultHits++;
    res.writeHead(402, {
      'content-type': 'application/json',
      'payment-required': b64(V2),
      'access-control-allow-origin': '*',
    });
    res.end(JSON.stringify(V1));
  });
  const vaultPort = await listen(vault);
  let site;
  let browser;
  try {
    site = createServer((req, res) => {
      const url = new URL(req.url || '/', 'http://127.0.0.1');
      count(url.pathname);
      seenHeaders.push(req.headers);
      const pay = (headers, body = '{}') => {
        res.writeHead(402, { 'access-control-allow-origin': '*', ...headers });
        res.end(body);
      };
      if (url.pathname === '/') {
        const fetches = [
          '/x402-v1',
          '/x402-v2',
          '/mpp',
          '/both',
          '/bad',
          `http://vault.sliccy.ai:${vaultPort}/pay`,
        ]
          .map((src) => `fetch(${JSON.stringify(src)}).then((r) => r.text()).catch(() => {});`)
          .join('');
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end(`<!doctype html><title>Paid</title><script>${fetches}</script>`);
        return;
      }
      if (url.pathname === '/x402-v1')
        return pay({ 'content-type': 'application/json' }, JSON.stringify(V1));
      if (url.pathname === '/x402-v2')
        return pay({ 'content-type': 'text/plain', 'payment-required': b64(V2) });
      if (url.pathname === '/mpp')
        return pay({ 'content-type': 'text/plain', 'www-authenticate': MPP });
      if (url.pathname === '/both')
        return pay({
          'content-type': 'text/plain',
          'payment-required': b64({ ...V2, resource: { url: 'https://paid.example/both' } }),
          'www-authenticate': MPP,
        });
      if (url.pathname === '/bad')
        return pay({ 'content-type': 'text/plain', 'payment-required': '%%%not-base64%%%' });
      res.writeHead(404);
      res.end();
    });
    const port = await listen(site);
    const origin = `http://127.0.0.1:${port}`;
    browser = await launchBrowser([
      `--host-resolver-rules=MAP vault.sliccy.ai 127.0.0.1:${vaultPort}`,
    ]);
    const runner = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    const cwd = await runner.fresh();
    await runner.clearSession(cwd);
    const result = await runner.playwright(
      ['discover', '--json', '--no-probe', '--once', `${origin}/`],
      cwd
    );
    assert.equal(
      result.code,
      0,
      `stderr: ${result.stderr} stdout: ${result.stdout} hits: ${JSON.stringify([...hits])}`
    );
    const events = parseEvents(result.stdout).filter(
      (e) => e.standard === 'x402' || e.standard === 'mpp'
    );
    const at = (route, standard) =>
      events.filter((e) => e.url === `${origin}${route}` && e.standard === standard);

    await t.test('x402 V1 from the JSON body', () => {
      const [v1] = at('/x402-v1', 'x402');
      assert.ok(v1, result.stdout);
      assert.equal(v1.status, 'valid', JSON.stringify(v1));
      assert.equal(v1.via, 'response');
      assert.equal(v1.summary.version, 1);
      assert.equal(v1.summary.resource, 'https://paid.example/v1');
      assert.deepEqual(v1.summary.offers[0], {
        scheme: 'exact',
        network: 'base-sepolia',
        amount: '10000',
        asset: '0xUSDC',
        payTo: '0xV1',
        expires: null,
        maxTimeoutSeconds: 30,
      });
    });

    await t.test('x402 V2 from PAYMENT-REQUIRED, with the amount exactly as sent', () => {
      const [v2] = at('/x402-v2', 'x402');
      assert.ok(v2, result.stdout);
      assert.equal(v2.status, 'valid');
      assert.equal(v2.summary.version, 2);
      assert.equal(v2.summary.resource, 'https://paid.example/v2');
      assert.equal(v2.summary.offers[0].amount, BIG);
      assert.equal(v2.summary.offers[0].network, 'eip155:8453');
    });

    await t.test('MPP from WWW-Authenticate: Payment', () => {
      const [mpp] = at('/mpp', 'mpp');
      assert.ok(mpp, result.stdout);
      assert.equal(mpp.status, 'valid');
      assert.deepEqual(mpp.summary.offers[0], {
        method: 'tempo',
        intent: 'charge',
        realm: 'shop',
        amount: '0.50',
        asset: 'usd',
        expires: '2030-01-01T00:00:00Z',
      });
    });

    await t.test('one response carrying both standards gives both events', () => {
      assert.equal(at('/both', 'x402').length, 1, result.stdout);
      assert.equal(at('/both', 'mpp').length, 1, result.stdout);
      assert.equal(at('/both', 'x402')[0].summary.resource, 'https://paid.example/both');
    });

    await t.test(
      'a challenge that does not decode is invalid, with the raw header only, and the rest still report',
      () => {
        const [bad] = at('/bad', 'x402');
        assert.ok(bad, result.stdout);
        assert.equal(bad.status, 'invalid');
        assert.equal(bad.reason, 'PAYMENT-REQUIRED is not base64 JSON');
        assert.deepEqual(bad.summary, { status: 402, challenge: '%%%not-base64%%%' });
        assert.equal(events.length, 6, result.stdout);
      }
    );

    await t.test('a protected origin is never reported', () => {
      assert.ok(!result.stdout.includes('vault.sliccy.ai'), result.stdout);
    });

    await t.test('observation only: one request per resource, and none carrying a payment', () => {
      for (const route of ['/x402-v1', '/x402-v2', '/mpp', '/both', '/bad'])
        assert.equal(hits.get(route), 1, route);
      for (const headers of seenHeaders) {
        assert.equal(headers['payment-signature'], undefined);
        assert.equal(headers['x-payment'], undefined);
        assert.ok(!String(headers.authorization ?? '').startsWith('Payment'));
      }
      assert.ok(vaultHits <= 1, `vault saw ${vaultHits}`);
    });
  } finally {
    site?.closeAllConnections();
    site?.close();
    vault.closeAllConnections();
    vault.close();
    await settle(browser?.close());
  }
});
