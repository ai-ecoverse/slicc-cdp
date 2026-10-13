import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { nativeRunner } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');

function startDiscoverServer() {
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    const pathname = url.pathname;

    if (pathname === '/') {
      res.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        link: '</api>; rel="api-catalog"',
      });
      res.end('<!doctype html><title>Fixture</title><p>hello</p>');
      return;
    }

    if (pathname === '/.well-known/ard.json') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ name: 'Test ARD', description: 'A test' }));
      return;
    }

    if (pathname === '/pay') {
      res.writeHead(402, {
        'content-type': 'text/html',
        'x-payment': 'lightning',
      });
      res.end('<!doctype html><title>Pay</title><p>payment required</p>');
      return;
    }

    if (pathname === '/webmcp') {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(`<!doctype html><title>WebMCP</title><script>
if (document.modelContext && document.modelContext.registerTool) {
  document.modelContext.registerTool({
    name: 'search',
    description: 'Search things',
    inputSchema: { type: 'object', properties: { q: { type: 'string' } } },
    execute: async () => 'ok'
  });
} else {
  document.modelContext = {
    async getTools() {
      return [{ name: 'search', description: 'Search things', inputSchema: { type: 'object' }, origin: location.origin }];
    },
    async executeTool() { return 'ok'; }
  };
}
</script>`);
      return;
    }

    if (pathname === '/plain') {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<!doctype html><title>Plain</title><p>nothing here</p>');
      return;
    }

    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not found');
  });

  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

test('playwright-cli discover --once finds link headers, ard, 402, and webmcp', async () => {
  const site = await startDiscoverServer();
  let browser;
  try {
    browser = await launchBrowser();
    const runner = nativeRunner(
      path.join(nativeDir, process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'),
      path.join(nativeDir, process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'),
      browser.wsUrl
    );

    const cwd = await runner.fresh();
    await runner.clearSession(cwd);

    {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/`],
        cwd
      );
      assert.equal(result.code, 0, `discover / failed: ${result.stderr}\n${result.stdout}`);
      const lines = result.stdout.trim().split('\n').filter(Boolean);
      assert.ok(
        lines.length >= 1,
        `expected at least 1 event, got ${lines.length}: ${result.stdout}`
      );
      const events = lines.map((line) => JSON.parse(line));
      assert.ok(
        events.every((e) => e.v === 1),
        'all events are v1'
      );

      const linkEvents = events.filter((e) => e.via === 'link-header');
      assert.ok(linkEvents.length >= 1, `expected link-header events: ${JSON.stringify(events)}`);
      const apiCatalog = linkEvents.find((e) => e.standard === 'api-catalog');
      assert.ok(apiCatalog, 'found api-catalog link-header event');

      const ardProbe = events.find((e) => e.via === 'well-known' && e.standard === 'ard');
      assert.ok(ardProbe, `found ard well-known probe: ${JSON.stringify(events)}`);
      assert.equal(ardProbe.status, 'valid');
      assert.equal(ardProbe.summary.name, 'Test ARD');
    }

    {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/pay`],
        cwd
      );
      assert.equal(result.code, 0, `discover /pay failed: ${result.stderr}`);
      const lines = result.stdout.trim().split('\n').filter(Boolean);
      const events = lines.map((line) => JSON.parse(line));
      const x402 = events.find((e) => e.standard === 'x402');
      assert.ok(x402, `found 402 event: ${JSON.stringify(events)}`);
      assert.equal(x402.via, 'response');
      assert.equal(x402.summary.status, 402);
    }

    {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/webmcp`],
        cwd
      );
      assert.equal(result.code, 0, `discover /webmcp failed: ${result.stderr}\n${result.stdout}`);
      const lines = result.stdout.trim().split('\n').filter(Boolean);
      const events = lines.map((line) => JSON.parse(line));
      const webmcp = events.find((e) => e.standard === 'webmcp');
      assert.ok(webmcp, `found webmcp event: ${JSON.stringify(events)}`);
      assert.equal(webmcp.via, 'page-api');
      assert.ok(webmcp.summary.tools.length > 0, 'webmcp has tools');
    }

    {
      const plainServer = createServer((req, res) => {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>Plain</title><p>nothing here</p>');
      });
      await new Promise((resolve) => plainServer.listen(0, '127.0.0.1', resolve));
      const plainPort = plainServer.address().port;
      const plainOrigin = `http://127.0.0.1:${plainPort}`;
      await runner.playwright(['tab-new', `${plainOrigin}/`], cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${plainOrigin}/`],
        cwd
      );
      plainServer.close();
      assert.equal(result.code, 1, `plain page should exit 1 (nothing found): ${result.stdout}`);
    }
  } finally {
    await browser?.close();
    await site.close();
  }
});
