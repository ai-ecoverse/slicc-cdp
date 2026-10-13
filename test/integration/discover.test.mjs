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

async function settle(p) {
  try {
    await p;
  } catch {
    return undefined;
  }
}

test('playwright-cli discover --once', { timeout: 60_000 }, async (t) => {
  const site = await startDiscoverServer();
  let browser;
  try {
    browser = await launchBrowser();
    const cli = path.join(
      nativeDir,
      process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
    );
    const curl = path.join(
      nativeDir,
      process.platform === 'win32' ? 'curlwright.exe' : 'curlwright'
    );
    const runner = nativeRunner(cli, curl, browser.wsUrl);
    const cwd = await runner.fresh();
    await runner.clearSession(cwd);

    await t.test('link headers and ard well-known', async () => {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      assert.ok(events.length >= 1, `events: ${result.stdout}`);
      assert.ok(events.every((e) => e.v === 1));
      const apiCatalog = events.find(
        (e) => e.via === 'link-header' && e.standard === 'api-catalog'
      );
      assert.ok(apiCatalog, `api-catalog: ${result.stdout}`);
      const ard = events.find((e) => e.via === 'well-known' && e.standard === 'ard');
      assert.ok(ard, `ard: ${result.stdout}`);
      assert.equal(ard.status, 'valid');
      assert.equal(ard.summary.name, 'Test ARD');
    });

    await t.test('402 response', async () => {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/pay`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const x402 = events.find((e) => e.standard === 'x402');
      assert.ok(x402, `x402: ${result.stdout}`);
      assert.equal(x402.via, 'response');
      assert.equal(x402.summary.status, 402);
    });

    await t.test('webmcp page signal', async () => {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/webmcp`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}\n${result.stdout}`);
      const events = parseEvents(result.stdout);
      const webmcp = events.find((e) => e.standard === 'webmcp');
      assert.ok(webmcp, `webmcp: ${result.stdout}`);
      assert.equal(webmcp.via, 'page-api');
      assert.ok(webmcp.summary.tools.length > 0);
    });

    await t.test('plain page yields nothing', async () => {
      const plainServer = createServer((req, res) => {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>Plain</title>');
      });
      try {
        await new Promise((r) => plainServer.listen(0, '127.0.0.1', r));
        const plainOrigin = `http://127.0.0.1:${plainServer.address().port}`;
        await runner.playwright(['tab-new', `${plainOrigin}/`], cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${plainOrigin}/`],
          cwd
        );
        assert.equal(result.code, 1, `expected exit 1: ${result.stdout}`);
      } finally {
        plainServer.closeAllConnections();
        plainServer.close();
      }
    });

    await t.test('well-known redirect to protected origin produces no finding', async () => {
      const rdServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>Redirect</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(302, { location: 'https://vault.sliccy.ai/.well-known/ard.json' });
          res.end('');
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      try {
        await new Promise((r) => rdServer.listen(0, '127.0.0.1', r));
        const rdOrigin = `http://127.0.0.1:${rdServer.address().port}`;
        await runner.playwright(['tab-new', `${rdOrigin}/`], cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${rdOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout);
        const fromProtected = events.filter(
          (e) =>
            e.url?.includes('vault.sliccy.ai') ||
            e.origin?.includes('vault.sliccy.ai') ||
            e.url?.includes('sliccy.ai')
        );
        assert.equal(
          fromProtected.length,
          0,
          `no findings from protected origin: ${result.stdout}`
        );
      } finally {
        rdServer.closeAllConnections();
        rdServer.close();
      }
    });
  } finally {
    await settle(browser?.close());
    await settle(site.close());
  }
});

function parseEvents(stdout) {
  return stdout
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}
