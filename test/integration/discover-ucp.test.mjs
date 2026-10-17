import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const nativeDir = path.join(root, 'target/debug');
const fixture = (name) => readFileSync(path.join(here, 'fixtures', name), 'utf8');

const SPEC_BUSINESS = fixture('ucp-spec-business.json');
const SPEC_AP2 = fixture('ucp-spec-ap2.json');
const STOREFRONT = fixture('ucp-storefront.json');

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

function site(profile) {
  const requests = [];
  const server = createServer((req, res) => {
    requests.push({ method: req.method, url: req.url });
    if (req.url === '/') {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<!doctype html><title>Shop</title>');
      return;
    }
    if (req.url === '/.well-known/ucp' && profile !== undefined) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(profile);
      return;
    }
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not found');
  });
  return new Promise((resolve) =>
    server.listen(0, '127.0.0.1', () =>
      resolve({ server, requests, origin: `http://127.0.0.1:${server.address().port}` })
    )
  );
}

test('discover UCP (/.well-known/ucp) with the AP2 extension', { timeout: 120_000 }, async (t) => {
  let browser;
  const servers = [];
  try {
    browser = await launchBrowser();
    const runner = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );
    const ucpOf = async (profile) => {
      const s = await site(profile);
      servers.push(s.server);
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(['discover', '--json', '--once', `${s.origin}/`], cwd);
      const events = result.stdout.trim() ? parseEvents(result.stdout) : [];
      return { ucp: events.filter((e) => e.standard === 'ucp'), result, requests: s.requests };
    };

    await t.test(
      'the ucp.dev business-profile example (https://ucp.dev/specification/overview/, Apache-2.0) validates',
      async () => {
        const { ucp, result } = await ucpOf(SPEC_BUSINESS);
        assert.equal(ucp.length, 1, result.stdout);
        const [e] = ucp;
        assert.equal(e.status, 'valid', JSON.stringify(e));
        assert.equal(e.via, 'well-known');
        assert.equal(e.summary.version, '2026-08-25');
        assert.deepEqual(e.summary.mcp, ['https://business.example.com/ucp/mcp']);
        assert.deepEqual(e.summary.a2a, [
          'https://business.example.com/.well-known/agent-card.json',
        ]);
        assert.deepEqual(
          e.summary.services['dev.ucp.shopping'].map((s) => s.transport),
          ['rest', 'mcp', 'a2a', 'embedded']
        );
        assert.ok(e.summary.capabilities.includes('dev.ucp.shopping.checkout@2026-08-25'));
        assert.deepEqual(e.summary.paymentHandlers, ['com.example.processor_tokenizer']);
        assert.equal(e.summary.hasKeys, true);
        assert.equal(e.summary.ap2, false);
        assert.equal(e.summary.note, undefined);
        assert.ok(!JSON.stringify(e).includes('JrQLj5P_89iXES9'), 'no key material');
      }
    );

    await t.test(
      'the ucp.dev AP2 mandates profile example (https://ucp.dev/specification/payment/extensions/ap2-mandates/, Apache-2.0) sets ap2',
      async () => {
        const { ucp, result } = await ucpOf(SPEC_AP2);
        assert.equal(ucp[0]?.status, 'valid', result.stdout);
        assert.equal(ucp[0].summary.ap2, true);
        assert.deepEqual(ucp[0].summary.services, {});
        assert.equal(ucp[0].summary.hasKeys, false);
      }
    );

    await t.test(
      'a profile modelled on a public Shopify storefront profile (2026-10) validates with its MCP endpoint',
      async () => {
        const { ucp, result, requests } = await ucpOf(STOREFRONT);
        assert.equal(ucp[0]?.status, 'valid', result.stdout);
        assert.deepEqual(ucp[0].summary.mcp, [
          'https://example-store.myshopify.example.test/api/ucp/mcp',
        ]);
        assert.deepEqual(ucp[0].summary.paymentHandlers, [
          'com.google.pay',
          'dev.shopify.card',
          'dev.shopify.shop_pay',
        ]);
        assert.equal(ucp[0].summary.capabilities.length, 11);
        assert.equal(ucp[0].summary.ap2, false);
        const probes = requests.filter((r) => r.url === '/.well-known/ucp');
        assert.deepEqual(probes, [{ method: 'GET', url: '/.well-known/ucp' }]);
        assert.ok(
          requests.every((r) => !r.url.includes('/api/ucp/mcp')),
          'the MCP endpoint is never called'
        );
      }
    );

    await t.test('an unknown version is valid with a note', async () => {
      const profile = JSON.stringify({
        ucp: { version: '2027-02-02', services: {}, payment_handlers: {} },
      });
      const { ucp, result } = await ucpOf(profile);
      assert.equal(ucp[0]?.status, 'valid', result.stdout);
      assert.equal(ucp[0].summary.note, 'unknown version');
    });

    await t.test('a profile without payment_handlers is invalid', async () => {
      const profile = JSON.stringify({ ucp: { version: '2026-08-25', services: {} } });
      const { ucp, result } = await ucpOf(profile);
      assert.equal(ucp[0]?.status, 'invalid', result.stdout);
      assert.equal(ucp[0].reason, 'missing ucp.payment_handlers');
    });

    await t.test('a site without /.well-known/ucp reports nothing', async () => {
      const { ucp } = await ucpOf(undefined);
      assert.deepEqual(ucp, []);
    });
  } finally {
    for (const server of servers) {
      server.closeAllConnections();
      server.close();
    }
    await settle(browser?.close());
  }
});
