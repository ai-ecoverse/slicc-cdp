import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');

function startDiscoverServer() {
  const server = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    const pathname = url.pathname;

    if (pathname === '/') {
      res.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        link: '</api>; rel="api-catalog", </llms.txt>; rel="https://llmstxt.org/rel/llms-txt", <https://github.com/o/r>; rel="https://www.sliccy.ai/rel/upskill"; branch=main; path="skills/foo"',
      });
      res.end('<!doctype html><title>Fixture</title><p>hello</p>');
      return;
    }

    if (pathname === '/.well-known/api-catalog') {
      res.writeHead(200, { 'content-type': 'application/linkset+json' });
      res.end(JSON.stringify({ linkset: [{ anchor: '/' }] }));
      return;
    }

    if (pathname === '/llms.txt') {
      res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('# Test Site\n> A test summary\n## Section One\n## Section Two\nBody text.\n');
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

    if (pathname === '/auth.md') {
      const accept = req.headers['accept'] || '';
      if (url.searchParams.get('html') === '1') {
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        res.end('<!doctype html><title>Auth</title><p>not markdown</p>');
        return;
      }
      res.writeHead(200, { 'content-type': 'text/markdown; charset=utf-8' });
      res.end(
        '# Auth\n\n- OAuth 2.0 with PKCE\n- API Key for testing\n\nclaim_window: 7 days\n\nhttps://auth.example.com/.well-known/oauth-authorization-server\n'
      );
      return;
    }

    if (pathname === '/api/protected') {
      res.writeHead(401, {
        'content-type': 'application/json',
        'www-authenticate':
          'Bearer realm="api", resource_metadata="' +
          `http://127.0.0.1:${server.address().port}/.well-known/oauth-protected-resource` +
          '"',
        link: '</auth.md>; rel="auth-md"',
        'access-control-allow-origin': '*',
        'access-control-expose-headers': 'www-authenticate, link',
      });
      res.end('{"error":"unauthorized"}');
      return;
    }

    if (pathname === '/auth-page') {
      const port = server.address().port;
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(
        `<!doctype html><title>Auth Page</title><script>fetch('http://127.0.0.1:${port}/api/protected').catch(()=>{})</script>`
      );
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
      assert.ok(apiCatalog, `api-catalog link-header: ${result.stdout}`);
      const ard = events.find((e) => e.via === 'well-known' && e.standard === 'ard');
      assert.ok(ard, `ard: ${result.stdout}`);
      assert.equal(ard.status, 'valid');
      assert.equal(ard.summary.name, 'Test ARD');

      const handoff = events.find((e) => e.standard === 'slicc-handoff');
      assert.ok(handoff, `slicc-handoff: ${result.stdout}`);
      assert.equal(handoff.summary.verb, 'upskill');
      assert.equal(handoff.summary.branch, 'main');
      assert.equal(handoff.summary.path, 'skills/foo');

      const llmsLink = events.find((e) => e.via === 'link-header' && e.standard === 'llms-txt');
      assert.ok(llmsLink, `llms-txt link-header: ${result.stdout}`);

      const wellKnownStandards = events
        .filter((e) => e.via === 'well-known')
        .map((e) => e.standard);
      assert.ok(
        wellKnownStandards.includes('llms-txt') || wellKnownStandards.includes('api-catalog'),
        `llms-txt or api-catalog well-known probe expected: ${result.stdout}`
      );
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
      assert.equal(x402.status, 'invalid');
      assert.equal(x402.reason, 'no x402 payload');
      assert.equal(x402.summary.challenge, 'lightning');
    });

    await t.test('401 response with auth-md pointer', async () => {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/auth-page`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const authMdProbe = events.find((e) => e.standard === 'auth-md' && e.via === 'well-known');
      assert.ok(authMdProbe, `auth-md well-known probe expected: ${result.stdout}`);
      assert.equal(authMdProbe.status, 'valid');
      const authMdResponse = events.find((e) => e.standard === 'auth-md' && e.via === 'response');
      if (authMdResponse) {
        assert.equal(authMdResponse.summary.status, 401);
        if (authMdResponse.summary.schemes) {
          assert.equal(authMdResponse.summary.schemes[0].scheme, 'Bearer');
        }
      }
      assert.ok(
        !result.stdout.includes('Authorization') && !result.stdout.includes('Cookie'),
        'must not leak request headers'
      );
    });

    await t.test('auth.md probe on page origin', async () => {
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${site.origin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const authProbe = events.find((e) => e.standard === 'auth-md' && e.via === 'well-known');
      assert.ok(authProbe, `auth-md well-known probe: ${result.stdout}`);
      assert.equal(authProbe.status, 'valid');
    });

    await t.test('auth.md HTML probe rejected', async () => {
      const htmlAuthServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>Site</title><p>hello</p>');
          return;
        }
        if (url.pathname === '/auth.md') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>Auth</title><p>not markdown</p>');
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      try {
        await new Promise((r) => htmlAuthServer.listen(0, '127.0.0.1', r));
        const htmlOrigin = `http://127.0.0.1:${htmlAuthServer.address().port}`;
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${htmlOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout);
        const authProbe = events.find(
          (e) => e.standard === 'auth-md' && e.via === 'well-known' && e.status === 'valid'
        );
        assert.ok(!authProbe, `HTML auth.md must not be classified as valid: ${result.stdout}`);
      } finally {
        await new Promise((r) => {
          htmlAuthServer.closeAllConnections();
          htmlAuthServer.close(r);
        });
      }
    });

    await t.test('401 from stand-in: no probe with --no-probe', async () => {
      let standInHits = 0;
      const standIn = createServer((req, res) => {
        standInHits++;
        res.writeHead(401, {
          'content-type': 'application/json',
          'www-authenticate': 'Bearer realm="api"',
          'access-control-allow-origin': '*',
          'access-control-expose-headers': 'www-authenticate',
        });
        res.end('{"error":"unauthorized"}');
      });
      await new Promise((r) => standIn.listen(0, '127.0.0.1', r));
      const standInOrigin = `http://127.0.0.1:${standIn.address().port}`;
      const pageServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end(
            `<!doctype html><title>No-Probe 401</title><script>fetch('${standInOrigin}/api').catch(()=>{})</script>`
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => pageServer.listen(0, '127.0.0.1', r));
      const pageOrigin = `http://127.0.0.1:${pageServer.address().port}`;
      try {
        const freshCwd = await runner.fresh();
        await runner.clearSession(freshCwd);
        const result = await runner.playwright(
          ['discover', '--json', '--no-probe', '--once', `${pageOrigin}/`],
          freshCwd
        );
        const events = parseEvents(result.stdout);
        const probeEvents = events.filter(
          (e) => e.origin === standInOrigin && e.via === 'well-known'
        );
        assert.equal(
          probeEvents.length,
          0,
          `no probes to stand-in with --no-probe: ${result.stdout}`
        );
        assert.ok(
          standInHits <= 1,
          `stand-in must see at most the page fetch (got ${standInHits} hits)`
        );
      } finally {
        pageServer.closeAllConnections();
        pageServer.close();
        standIn.closeAllConnections();
        standIn.close();
      }
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

    await t.test('html-link ard without probe', async () => {
      const ardLinkServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>ARD Link</title><link rel="ard" href="/ard.json">');
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      try {
        await new Promise((r) => ardLinkServer.listen(0, '127.0.0.1', r));
        const ardOrigin = `http://127.0.0.1:${ardLinkServer.address().port}`;
        const result = await runner.playwright(
          ['discover', '--json', '--no-probe', '--once', `${ardOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}\n${result.stdout}`);
        const events = parseEvents(result.stdout);
        const ardLink = events.find((e) => e.via === 'html-link' && e.standard === 'ard');
        assert.ok(ardLink, `ard html-link event expected: ${result.stdout}`);
        const wellKnown = events.filter((e) => e.via === 'well-known');
        assert.equal(wellKnown.length, 0, `no well-known request expected: ${result.stdout}`);
      } finally {
        ardLinkServer.closeAllConnections();
        ardLinkServer.close();
      }
    });

    await t.test('html-link api-catalog without probe', async () => {
      const apiCatServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end(
            '<!doctype html><title>API Catalog</title><link rel="api-catalog" href="/catalog.json">'
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      try {
        await new Promise((r) => apiCatServer.listen(0, '127.0.0.1', r));
        const apiCatOrigin = `http://127.0.0.1:${apiCatServer.address().port}`;
        const result = await runner.playwright(
          ['discover', '--json', '--no-probe', '--once', `${apiCatOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}\n${result.stdout}`);
        const events = parseEvents(result.stdout);
        const apiCat = events.find((e) => e.via === 'html-link' && e.standard === 'api-catalog');
        assert.ok(apiCat, `api-catalog html-link event expected: ${result.stdout}`);
        const wellKnown = events.filter((e) => e.via === 'well-known');
        assert.equal(wellKnown.length, 0, `no well-known request expected: ${result.stdout}`);
      } finally {
        apiCatServer.closeAllConnections();
        apiCatServer.close();
      }
    });

    await t.test('webmcp page signal in once mode', async () => {
      const webmcpOnlyServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end(`<!doctype html><title>WebMCP Only</title><script>
if (document.modelContext && document.modelContext.registerTool) {
  document.modelContext.registerTool({
    name: 'lookup',
    description: 'Lookup something',
    inputSchema: { type: 'object', properties: { q: { type: 'string' } } },
    execute: async () => 'ok'
  });
} else {
  document.modelContext = {
    async getTools() {
      return [{ name: 'lookup', description: 'Lookup something', inputSchema: { type: 'object' }, origin: location.origin }];
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
      try {
        await new Promise((r) => webmcpOnlyServer.listen(0, '127.0.0.1', r));
        const webmcpOrigin = `http://127.0.0.1:${webmcpOnlyServer.address().port}`;
        const result = await runner.playwright(
          ['discover', '--json', '--no-probe', '--once', `${webmcpOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}\n${result.stdout}`);
        const events = parseEvents(result.stdout);
        const webmcp = events.find((e) => e.standard === 'webmcp');
        assert.ok(webmcp, `webmcp event expected: ${result.stdout}`);
        assert.equal(webmcp.via, 'page-api');
        assert.ok(webmcp.summary.tools.length > 0);
        const wellKnown = events.filter((e) => e.via === 'well-known');
        assert.equal(wellKnown.length, 0, `no well-known request expected: ${result.stdout}`);
      } finally {
        webmcpOnlyServer.closeAllConnections();
        webmcpOnlyServer.close();
      }
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

test('discover hardening', { timeout: 180_000 }, async (t) => {
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

    await t.test('302 redirect to protected origin produces no event', async () => {
      let standInHits = 0;
      const standIn = createServer((req, res) => {
        standInHits++;
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ name: 'Protected' }));
      });
      await new Promise((r) => standIn.listen(0, '127.0.0.1', r));
      const standInOrigin = `http://127.0.0.1:${standIn.address().port}`;
      const rdServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>R302</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(302, { location: `${standInOrigin}/.well-known/ard.json` });
          res.end('');
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => rdServer.listen(0, '127.0.0.1', r));
      const rdOrigin = `http://127.0.0.1:${rdServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${rdOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout).filter((e) => e.standard === 'ard');
        assert.equal(events.length, 0, `no ard event expected: ${result.stdout}`);
        assert.equal(standInHits, 0, 'protected stand-in must see 0 requests');
      } finally {
        rdServer.closeAllConnections();
        rdServer.close();
        standIn.closeAllConnections();
        standIn.close();
      }
    });

    await t.test('307 redirect to protected origin produces no event', async () => {
      let standInHits = 0;
      const standIn = createServer((req, res) => {
        standInHits++;
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ name: 'Protected307' }));
      });
      await new Promise((r) => standIn.listen(0, '127.0.0.1', r));
      const standInOrigin = `http://127.0.0.1:${standIn.address().port}`;
      const rdServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>R307</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(307, { location: `${standInOrigin}/.well-known/ard.json` });
          res.end('');
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => rdServer.listen(0, '127.0.0.1', r));
      const rdOrigin = `http://127.0.0.1:${rdServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${rdOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout).filter((e) => e.standard === 'ard');
        assert.equal(events.length, 0, `no ard event expected: ${result.stdout}`);
        assert.equal(standInHits, 0, 'protected stand-in must see 0 requests');
      } finally {
        rdServer.closeAllConnections();
        rdServer.close();
        standIn.closeAllConnections();
        standIn.close();
      }
    });

    await t.test('probe carries no Cookie or Authorization header', async () => {
      let probeHeaders = null;
      const cookieServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            'set-cookie': 'httpcookie=secret; Path=/',
          });
          res.end(
            `<!doctype html><title>Cookie</title><script>document.cookie='jscookie=jsval';</script>`
          );
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          probeHeaders = { cookie: req.headers.cookie, authorization: req.headers.authorization };
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ name: 'Cookie Test' }));
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => cookieServer.listen(0, '127.0.0.1', r));
      const cookieOrigin = `http://127.0.0.1:${cookieServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        await runner.playwright(['tab-new', `${cookieOrigin}/`], cwd);
        await new Promise((r) => setTimeout(r, 1000));
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${cookieOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        assert.ok(probeHeaders !== null, 'probe must have reached the server');
        assert.equal(
          probeHeaders.cookie,
          undefined,
          `Cookie must not be sent: ${probeHeaders.cookie}`
        );
        assert.equal(probeHeaders.authorization, undefined, `Authorization must not be sent`);
      } finally {
        cookieServer.closeAllConnections();
        cookieServer.close();
      }
    });

    await t.test('html-link to protected origin produces no event', async () => {
      const htmlLinkServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end(
            '<!doctype html><title>HtmlLink</title><link rel="ai-catalog" href="https://vault.sliccy.ai/catalog.json">'
          );
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => htmlLinkServer.listen(0, '127.0.0.1', r));
      const hlOrigin = `http://127.0.0.1:${htmlLinkServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${hlOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout).filter(
          (e) => e.url?.includes('vault.sliccy.ai') || e.summary?.href?.includes('vault.sliccy.ai')
        );
        assert.equal(events.length, 0, `no event with protected href: ${result.stdout}`);
      } finally {
        htmlLinkServer.closeAllConnections();
        htmlLinkServer.close();
      }
    });

    await t.test('userinfo is stripped from all event fields', async () => {
      const uiServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            link: '</api>; rel="api-catalog"',
          });
          res.end('<!doctype html><title>UserInfo</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ name: 'UI Test' }));
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => uiServer.listen(0, '127.0.0.1', r));
      const uiPort = uiServer.address().port;
      const uiOriginWithCreds = `http://u:PW@127.0.0.1:${uiPort}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${uiOriginWithCreds}/`],
          cwd
        );
        const stdout = result.stdout;
        assert.ok(!stdout.includes('PW'), `no event field must contain PW: ${stdout}`);
        assert.ok(!stdout.includes('u:PW'), `no event field must contain u:PW: ${stdout}`);
        if (stdout.trim()) {
          const events = parseEvents(stdout);
          for (const e of events) {
            const json = JSON.stringify(e);
            assert.ok(!json.includes('PW'), `event must not contain PW: ${json}`);
          }
        }
      } finally {
        uiServer.closeAllConnections();
        uiServer.close();
      }
    });

    await t.test('--once from A to B probes B not A', async () => {
      let aProbed = false;
      let bProbed = false;
      const siteA = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/.well-known/ard.json') {
          aProbed = true;
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ name: 'A' }));
          return;
        }
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>A</title>');
      });
      const siteB = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/.well-known/ard.json') {
          bProbed = true;
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ name: 'B' }));
          return;
        }
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>B</title>');
      });
      await new Promise((r) => siteA.listen(0, '127.0.0.1', r));
      await new Promise((r) => siteB.listen(0, '127.0.0.1', r));
      const originA = `http://127.0.0.1:${siteA.address().port}`;
      const originB = `http://127.0.0.1:${siteB.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        await runner.playwright(['tab-new', `${originA}/`], cwd);
        await new Promise((r) => setTimeout(r, 1000));
        aProbed = false;
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${originB}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        assert.ok(bProbed, 'B must be probed');
        assert.ok(!aProbed, 'A must NOT be probed during the --once B run');
      } finally {
        siteA.closeAllConnections();
        siteA.close();
        siteB.closeAllConnections();
        siteB.close();
      }
    });

    if (process.platform !== 'win32') {
      await t.test('lick dirs are 0700 and files 0600 under permissive umask', async () => {
        const { spawn: nodeSpawn } = await import('node:child_process');
        const { mkdtemp: mkd, readdir: rd, stat: st } = await import('node:fs/promises');
        const { tmpdir: td } = await import('node:os');
        const tempHome = await mkd(path.join(td(), 'slicc-cdp-perm-'));
        const lickServer = createServer((req, res) => {
          const url = new URL(req.url || '/', 'http://127.0.0.1');
          if (url.pathname === '/') {
            res.writeHead(200, {
              'content-type': 'text/html; charset=utf-8',
              link: '</api>; rel="api-catalog"',
            });
            res.end('<!doctype html><title>Perm</title>');
            return;
          }
          if (url.pathname === '/.well-known/ard.json') {
            res.writeHead(200, { 'content-type': 'application/json' });
            res.end(JSON.stringify({ name: 'Perm Test' }));
            return;
          }
          res.writeHead(404);
          res.end('nope');
        });
        await new Promise((r) => lickServer.listen(0, '127.0.0.1', r));
        const lickOrigin = `http://127.0.0.1:${lickServer.address().port}`;
        try {
          const result = await new Promise((resolve, reject) => {
            const env = { ...process.env, HOME: tempHome };
            delete env.SLICC_CDP_URL;
            const child = nodeSpawn(
              'sh',
              [
                '-c',
                `umask 000 && exec "$1" --cdp "$2" discover --once "$3" --lick`,
                '--',
                cli,
                browser.wsUrl,
                `${lickOrigin}/`,
              ],
              { cwd: tempHome, env, stdio: ['ignore', 'pipe', 'pipe'] }
            );
            const out = [];
            const err = [];
            child.stdout.on('data', (c) => out.push(c));
            child.stderr.on('data', (c) => err.push(c));
            const timer = setTimeout(() => {
              child.kill('SIGKILL');
              reject(new Error('timeout'));
            }, 45000);
            child.on('close', (code) => {
              clearTimeout(timer);
              resolve({
                code: code ?? 1,
                stdout: Buffer.concat(out).toString(),
                stderr: Buffer.concat(err).toString(),
              });
            });
          });
          assert.equal(result.code, 0, `stderr: ${result.stderr}`);
          const sliccDir = path.join(tempHome, '.slicc');
          for (const dir of ['discoveries', 'watches']) {
            const s = await st(path.join(sliccDir, dir));
            assert.equal(
              s.mode & 0o777,
              0o700,
              `${dir} must be 0700, got ${(s.mode & 0o777).toString(8)}`
            );
          }
          const discos = await rd(path.join(sliccDir, 'discoveries'));
          for (const f of discos.filter((n) => n.endsWith('.json'))) {
            const s = await st(path.join(sliccDir, 'discoveries', f));
            assert.equal(
              s.mode & 0o777,
              0o600,
              `${f} must be 0600, got ${(s.mode & 0o777).toString(8)}`
            );
          }
          const watchStat = await st(path.join(sliccDir, 'watches', 'discover.json'));
          assert.equal(watchStat.mode & 0o777, 0o600, `discover.json must be 0600`);
        } finally {
          lickServer.closeAllConnections();
          lickServer.close();
          const { rm: rmf } = await import('node:fs/promises');
          await rmf(tempHome, { recursive: true, force: true }).catch(() => {});
        }
      });
    }

    await t.test('eval throw with ESC/BEL/DEL/C1 is sanitized in stderr', async () => {
      const escServer = createServer((req, res) => {
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        res.end(`<!doctype html><title>EvalThrow</title><script>
Object.defineProperty(document, 'modelContext', {
  value: {
    async getTools() {
      return [{ name: 'evil-eval', description: 'throws evil', inputSchema: { type: 'object' }, origin: location.origin }];
    },
    async executeTool() { throw new Error('boom\x1b[31m\x07\x7f\u009b'); }
  },
  configurable: true
});
</script>`);
      });
      await new Promise((r) => escServer.listen(0, '127.0.0.1', r));
      const escOrigin = `http://127.0.0.1:${escServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        await runner.playwright(['open', `${escOrigin}/`], cwd);
        const result = await runner.playwright(['webmcp-call', 'evil-eval', '{}'], cwd);
        assert.equal(result.code, 1);
        const raw = Buffer.from(result.stderr, 'utf8');
        for (let i = 0; i < raw.length; i++) {
          assert.notEqual(raw[i], 0x1b, `stderr byte ${i} is raw ESC`);
          assert.notEqual(raw[i], 0x07, `stderr byte ${i} is raw BEL`);
          assert.notEqual(raw[i], 0x7f, `stderr byte ${i} is raw DEL`);
        }
        for (const ch of result.stderr) {
          const cp = ch.codePointAt(0);
          assert.ok(
            !(cp >= 0x80 && cp <= 0x9f),
            `stderr contains raw C1 U+${cp.toString(16).padStart(4, '0')}`
          );
        }
      } finally {
        escServer.closeAllConnections();
        escServer.close();
      }
    });

    await t.test('playwright-cli eval throw with ESC/C1 is sanitized in stderr', async () => {
      const evalEscServer = createServer((req, res) => {
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
        res.end('<!doctype html><title>EvalEsc</title>');
      });
      await new Promise((r) => evalEscServer.listen(0, '127.0.0.1', r));
      const evalEscOrigin = `http://127.0.0.1:${evalEscServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        await runner.playwright(['open', `${evalEscOrigin}/`], cwd);
        const result = await runner.playwright(
          ['eval', `(function(){ throw new Error('evil\x1b[31m\x07\x7f\u009b'); })()`],
          cwd
        );
        assert.notEqual(result.code, 0);
        const raw = Buffer.from(result.stderr, 'utf8');
        for (let i = 0; i < raw.length; i++) {
          assert.notEqual(raw[i], 0x1b, `stderr byte ${i} is raw ESC`);
          assert.notEqual(raw[i], 0x07, `stderr byte ${i} is raw BEL`);
          assert.notEqual(raw[i], 0x7f, `stderr byte ${i} is raw DEL`);
        }
        for (const ch of result.stderr) {
          const cp = ch.codePointAt(0);
          assert.ok(
            !(cp >= 0x80 && cp <= 0x9f),
            `stderr contains raw C1 U+${cp.toString(16).padStart(4, '0')}`
          );
        }
      } finally {
        evalEscServer.closeAllConnections();
        evalEscServer.close();
      }
    });

    await t.test('5 MiB well-known body produces no valid ard event', async () => {
      const bigSize = 5 * 1024 * 1024;
      const bigServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>Big</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(200, {
            'content-type': 'application/json',
            'content-length': String(bigSize),
          });
          const chunk = Buffer.alloc(256 * 1024, 0x58);
          const write = () => {
            let sent = 0;
            while (sent < bigSize) {
              const ok = res.write(chunk);
              sent += chunk.length;
              if (!ok) {
                res.once('drain', write);
                return;
              }
            }
            res.end();
          };
          req.on('close', () => {});
          write();
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => bigServer.listen(0, '127.0.0.1', r));
      const bigOrigin = `http://127.0.0.1:${bigServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${bigOrigin}/`],
          cwd
        );
        const events = parseEvents(result.stdout);
        const ardValid = events.filter((e) => e.standard === 'ard' && e.status === 'valid');
        assert.equal(
          ardValid.length,
          0,
          `5 MiB body must not produce a valid ard event: ${JSON.stringify(ardValid)}`
        );
      } finally {
        bigServer.closeAllConnections();
        bigServer.close();
      }
    });

    await t.test('stalled body probe gives up within its own timeout', async () => {
      const probeTimeoutMs = 5000;
      const schedulingMarginMs = 2000;
      const hangMs = 30000;
      let requested;
      let abandoned;
      const mark = () => {
        if (requested !== undefined) abandoned ??= performance.now();
      };
      const stallServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/.well-known/ard.json') {
          mark();
          res.writeHead(200, {
            'content-type': 'application/json',
            'content-length': '10000',
          });
          requested ??= performance.now();
          req.socket.once('close', mark);
          res.write('{"name":"x');
          return;
        }
        mark();
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>Stall</title>');
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      await new Promise((r) => stallServer.listen(0, '127.0.0.1', r));
      const stallOrigin = `http://127.0.0.1:${stallServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        await runner.playwright(['discover', '--json', '--once', `${stallOrigin}/`], cwd);
        const exited = performance.now();
        mark();
        assert.ok(requested !== undefined, 'the probe must request /.well-known/ard.json');
        const gaveUp = Math.round(abandoned - requested);
        const finished = Math.round(exited - requested);
        assert.ok(
          gaveUp < probeTimeoutMs + schedulingMarginMs,
          `probe must give up within its ${probeTimeoutMs}ms timeout plus ${schedulingMarginMs}ms of scheduling, took ${gaveUp}ms`
        );
        assert.ok(
          finished < hangMs,
          `discover must finish well before the ${hangMs}ms default call timeout, took ${finished}ms`
        );
      } finally {
        stallServer.closeAllConnections();
        stallServer.close();
      }
    });

    await t.test('well-known ard.json with MCP, skill and squatted URN', async () => {
      const ardServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>ARD Fixture</title><p>hello</p>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          const origin = `http://127.0.0.1:${ardServer.address().port}`;
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Fixture Site',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:weather`,
                  displayName: 'Weather MCP',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/mcp/weather.json`,
                  capabilities: ['WeatherTool'],
                  representativeQueries: ['what is the weather'],
                },
                {
                  identifier: `urn:air:127.0.0.1:skill:pptx`,
                  displayName: 'PPTX Skill',
                  type: 'application/agent-skills+md',
                  url: `${origin}/skills/pptx`,
                },
                {
                  identifier: 'urn:air:google.com:server:search',
                  displayName: 'Squatted Google',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/fake-google`,
                },
              ],
            })
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => ardServer.listen(0, '127.0.0.1', r));
      const ardOrigin = `http://127.0.0.1:${ardServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${ardOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        const events = parseEvents(result.stdout);
        const ardEvents = events.filter((e) => e.standard === 'ard' && e.via === 'well-known');
        assert.equal(ardEvents.length, 1, `exactly one ard well-known event: ${result.stdout}`);
        const ard = ardEvents[0];
        assert.equal(ard.status, 'valid');
        const entries = ard.summary.entries;
        assert.ok(Array.isArray(entries), 'summary.entries must be an array');
        assert.equal(
          entries.length,
          2,
          `two usable entries (squatted excluded): ${JSON.stringify(entries)}`
        );
        for (const entry of entries) {
          assert.ok(entry.type, 'each entry has a type field');
          assert.ok(entry.identifier, 'each entry has an identifier');
          assert.ok(entry.displayName, 'each entry has a displayName');
        }
        const mcpEntry = entries.find((e) => e.type === 'application/mcp-server-card+json');
        assert.ok(mcpEntry, 'MCP server card entry present');
        assert.ok(mcpEntry.capabilities, 'MCP entry has capabilities');
        const skillEntry = entries.find((e) => e.type === 'application/agent-skills+md');
        assert.ok(skillEntry, 'skill entry present');
        const warnings = ard.summary.warnings;
        assert.ok(Array.isArray(warnings) && warnings.length > 0, 'warnings present');
        assert.ok(
          warnings.some((w) => w.includes('google.com')),
          `squatted URN warning mentions google.com: ${JSON.stringify(warnings)}`
        );
      } finally {
        ardServer.closeAllConnections();
        ardServer.close();
      }
    });

    await t.test('link rel=ard in HTML head found from page scan', async () => {
      const linkServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            link: '</ard-manifest.json>; rel="ard"',
          });
          res.end(
            '<!doctype html><html><head><title>Link ARD</title></head><body><p>hi</p></body></html>'
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => linkServer.listen(0, '127.0.0.1', r));
      const linkOrigin = `http://127.0.0.1:${linkServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${linkOrigin}/`],
          cwd
        );
        assert.equal(
          result.code,
          0,
          `exit ${result.code}, stderr: ${result.stderr}, stdout: ${result.stdout}`
        );
        const events = parseEvents(result.stdout);
        const ardLink = events.find((e) => e.standard === 'ard' && e.via === 'link-header');
        assert.ok(ardLink, `ard link-header event from Link header: ${result.stdout}`);
        assert.ok(ardLink.url.includes('/ard-manifest.json'), `href resolved: ${ardLink.url}`);
        const ardWellKnown = events.find((e) => e.standard === 'ard' && e.via === 'well-known');
        assert.ok(!ardWellKnown, 'well-known probe returned 404, so no well-known ard event');
      } finally {
        linkServer.closeAllConnections();
        linkServer.close();
      }
    });

    await t.test('--probe robots fetches Agentmap, omitted without flag', async () => {
      let robotsHitCount = 0;
      const robotsServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>Robots</title><p>hi</p>');
          return;
        }
        if (url.pathname === '/robots.txt') {
          robotsHitCount++;
          const ardUrl = `http://127.0.0.1:${robotsServer.address().port}/entries.json`;
          res.writeHead(200, { 'content-type': 'text/plain' });
          res.end(`User-agent: *\nDisallow: /private\nAgentmap: ${ardUrl}\n`);
          return;
        }
        if (url.pathname === '/entries.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Robots ARD',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:from-robots`,
                  displayName: 'From Robots',
                  type: 'application/mcp-server-card+json',
                  url: `http://127.0.0.1:${robotsServer.address().port}/mcp.json`,
                },
              ],
            })
          );
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(404, { 'content-type': 'text/plain' });
          res.end('not found');
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => robotsServer.listen(0, '127.0.0.1', r));
      const robotsOrigin = `http://127.0.0.1:${robotsServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        robotsHitCount = 0;
        const withoutFlag = await runner.playwright(
          ['discover', '--json', '--once', `${robotsOrigin}/`],
          cwd
        );
        const eventsWithout = parseEvents(withoutFlag.stdout);
        const robotsArdWithout = eventsWithout.filter(
          (e) => e.standard === 'ard' && e.summary?.name === 'Robots ARD'
        );
        assert.equal(
          robotsArdWithout.length,
          0,
          `no robots ARD without --probe robots: ${withoutFlag.stdout}`
        );
        assert.equal(robotsHitCount, 0, 'robots.txt not fetched without flag');

        const cwd2 = await runner.fresh();
        await runner.clearSession(cwd2);
        robotsHitCount = 0;
        const withFlag = await runner.playwright(
          ['discover', '--json', '--once', `${robotsOrigin}/`, '--probe', 'robots'],
          cwd2
        );
        assert.equal(withFlag.code, 0, `stderr: ${withFlag.stderr}`);
        const eventsWith = parseEvents(withFlag.stdout);
        const robotsArd = eventsWith.filter(
          (e) => e.standard === 'ard' && e.summary?.name === 'Robots ARD'
        );
        assert.equal(robotsArd.length, 1, `one ARD from robots: ${withFlag.stdout}`);
        assert.ok(robotsHitCount > 0, 'robots.txt was fetched with --probe robots');
      } finally {
        robotsServer.closeAllConnections();
        robotsServer.close();
      }
    });

    await t.test('--follow fetches nested catalog one level only', async () => {
      let level2Hit = false;
      const catalogServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        const origin = `http://127.0.0.1:${catalogServer.address().port}`;
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>Catalog</title><p>hi</p>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Root',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:root-tool`,
                  displayName: 'Root Tool',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/mcp.json`,
                },
                {
                  identifier: `urn:air:127.0.0.1:catalog:nested`,
                  displayName: 'Nested Catalog',
                  type: 'application/ai-catalog+json',
                  url: `${origin}/nested.json`,
                },
              ],
            })
          );
          return;
        }
        if (url.pathname === '/nested.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Nested',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:nested-tool`,
                  displayName: 'Nested Tool',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/nested-mcp.json`,
                },
                {
                  identifier: `urn:air:127.0.0.1:catalog:deeper`,
                  displayName: 'Deeper Catalog',
                  type: 'application/ai-catalog+json',
                  url: `${origin}/deeper.json`,
                },
              ],
            })
          );
          return;
        }
        if (url.pathname === '/deeper.json') {
          level2Hit = true;
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Deeper',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:deep-tool`,
                  displayName: 'Deep Tool',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/deep-mcp.json`,
                },
              ],
            })
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => catalogServer.listen(0, '127.0.0.1', r));
      const catalogOrigin = `http://127.0.0.1:${catalogServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${catalogOrigin}/`, '--follow'],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        const events = parseEvents(result.stdout);
        const ardEvents = events.filter((e) => e.standard === 'ard');
        assert.ok(ardEvents.length >= 2, `at least root + nested ARD events: ${result.stdout}`);
        const rootArd = ardEvents.find((e) => e.summary?.name === 'Root');
        assert.ok(rootArd, `root ARD event: ${result.stdout}`);
        assert.ok(rootArd.summary.catalogs?.length > 0, 'root reports nested catalogs');
        const nestedArd = ardEvents.find((e) => e.summary?.name === 'Nested');
        assert.ok(nestedArd, `nested catalog followed: ${result.stdout}`);
        assert.ok(
          !level2Hit,
          'deeper.json (level 2) must NOT be fetched — follow is one level only'
        );
      } finally {
        catalogServer.closeAllConnections();
        catalogServer.close();
      }
    });
  } finally {
    await settle(browser?.close());
  }
});

test('discover ARD html-link without probe', { timeout: 60_000 }, async () => {
  let wellKnownHit = false;
  const htmlLinkServer = createServer((req, res) => {
    const url = new URL(req.url || '/', 'http://127.0.0.1');
    if (url.pathname === '/') {
      res.writeHead(200, {
        'content-type': 'text/html; charset=utf-8',
        link: '</ard-manifest.json>; rel="ard"',
      });
      res.end(
        '<!doctype html><html><head><link rel="ard" href="/ard-manifest.json"><title>HTML Link ARD</title></head><body><p>hi</p></body></html>'
      );
      return;
    }
    if (url.pathname === '/.well-known/ard.json') {
      wellKnownHit = true;
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ name: 'Should Not Reach' }));
      return;
    }
    res.writeHead(404, { 'content-type': 'text/plain' });
    res.end('not found');
  });
  await new Promise((r) => htmlLinkServer.listen(0, '127.0.0.1', r));
  const htmlLinkOrigin = `http://127.0.0.1:${htmlLinkServer.address().port}`;
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
    const result = await runner.playwright(
      ['discover', '--json', '--no-probe', '--once', `${htmlLinkOrigin}/`],
      cwd
    );
    assert.equal(
      result.code,
      0,
      `exit ${result.code}, stderr: ${result.stderr}, stdout: ${result.stdout}`
    );
    const events = parseEvents(result.stdout);
    const ardEvent = events.find((e) => e.standard === 'ard');
    assert.ok(ardEvent, `ard event expected: ${result.stdout}`);
    assert.ok(
      ardEvent.via === 'html-link' || ardEvent.via === 'link-header',
      `via must be html-link or link-header: ${ardEvent.via}`
    );
    assert.ok(ardEvent.url.includes('/ard-manifest.json'), `href resolved: ${ardEvent.url}`);
    assert.ok(!wellKnownHit, '/.well-known/ard.json must not be fetched when --no-probe is set');
  } finally {
    await settle(browser?.close());
    htmlLinkServer.closeAllConnections();
    htmlLinkServer.close();
  }
});

test('discover ai-catalog', { timeout: 120_000 }, async (t) => {
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

    await t.test('well-known ai-catalog.json probe', async () => {
      const catServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        const origin = `http://127.0.0.1:${catServer.address().port}`;
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>AI Catalog</title><p>hello</p>');
          return;
        }
        if (url.pathname === '/.well-known/ai-catalog.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Catalog Only',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:search`,
                  displayName: 'Search MCP',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/mcp/search.json`,
                  capabilities: ['SearchTool'],
                },
                {
                  identifier: `urn:air:127.0.0.1:dataset:prices`,
                  displayName: 'Price Dataset',
                  type: 'application/dataset+json',
                  url: `${origin}/data/prices.json`,
                },
              ],
            })
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => catServer.listen(0, '127.0.0.1', r));
      const catOrigin = `http://127.0.0.1:${catServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${catOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        const events = parseEvents(result.stdout);
        const aiCat = events.find((e) => e.standard === 'ai-catalog' && e.via === 'well-known');
        assert.ok(aiCat, `ai-catalog well-known event: ${result.stdout}`);
        assert.equal(aiCat.status, 'valid');
        assert.ok(Array.isArray(aiCat.summary.entries), 'summary.entries is array');
        assert.equal(aiCat.summary.entries.length, 2, 'two entries');
        const types = aiCat.summary.types;
        assert.ok(types, 'types present');
        assert.ok(types['application/mcp-server-card+json'], 'MCP server type counted');
        assert.ok(types['application/dataset+json'], 'dataset type counted');
      } finally {
        catServer.closeAllConnections();
        catServer.close();
      }
    });

    await t.test(
      'alias rule: identical ard and ai-catalog give one event with aliases',
      async () => {
        const sharedPayload = JSON.stringify({
          name: 'Shared',
          entries: [
            {
              identifier: 'urn:air:127.0.0.1:server:tool1',
              displayName: 'Tool One',
              type: 'application/mcp-server-card+json',
              url: 'http://127.0.0.1/mcp.json',
            },
          ],
        });
        const aliasServer = createServer((req, res) => {
          const url = new URL(req.url || '/', 'http://127.0.0.1');
          if (url.pathname === '/') {
            res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
            res.end('<!doctype html><title>Alias</title><p>hi</p>');
            return;
          }
          if (
            url.pathname === '/.well-known/ard.json' ||
            url.pathname === '/.well-known/ai-catalog.json'
          ) {
            res.writeHead(200, { 'content-type': 'application/json' });
            res.end(sharedPayload);
            return;
          }
          res.writeHead(404, { 'content-type': 'text/plain' });
          res.end('not found');
        });
        await new Promise((r) => aliasServer.listen(0, '127.0.0.1', r));
        const aliasOrigin = `http://127.0.0.1:${aliasServer.address().port}`;
        try {
          const cwd = await runner.fresh();
          await runner.clearSession(cwd);
          const result = await runner.playwright(
            ['discover', '--json', '--once', `${aliasOrigin}/`],
            cwd
          );
          assert.equal(result.code, 0, `stderr: ${result.stderr}`);
          const events = parseEvents(result.stdout);
          const ardEvents = events.filter((e) => e.standard === 'ard' && e.via === 'well-known');
          const aiCatEvents = events.filter(
            (e) => e.standard === 'ai-catalog' && e.via === 'well-known'
          );
          assert.equal(ardEvents.length, 1, `exactly one ard event: ${result.stdout}`);
          assert.equal(
            aiCatEvents.length,
            0,
            `no separate ai-catalog event when alias: ${result.stdout}`
          );
          const ard = ardEvents[0];
          assert.ok(
            Array.isArray(ard.aliases) && ard.aliases.includes('ai-catalog'),
            `aliases must include ai-catalog: ${JSON.stringify(ard.aliases)}`
          );
        } finally {
          aliasServer.closeAllConnections();
          aliasServer.close();
        }
      }
    );

    await t.test('alias rule: different ard and ai-catalog give two events', async () => {
      const diffServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        const origin = `http://127.0.0.1:${diffServer.address().port}`;
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end('<!doctype html><title>Diff</title><p>hi</p>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'ARD Only',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:server:ard-tool`,
                  displayName: 'ARD Tool',
                  type: 'application/mcp-server-card+json',
                  url: `${origin}/ard-mcp.json`,
                },
              ],
            })
          );
          return;
        }
        if (url.pathname === '/.well-known/ai-catalog.json') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              name: 'Catalog Only',
              entries: [
                {
                  identifier: `urn:air:127.0.0.1:agent:cat-agent`,
                  displayName: 'Cat Agent',
                  type: 'application/a2a-agent-card+json',
                  url: `${origin}/cat-agent.json`,
                },
              ],
            })
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => diffServer.listen(0, '127.0.0.1', r));
      const diffOrigin = `http://127.0.0.1:${diffServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${diffOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        const events = parseEvents(result.stdout);
        const ardEvents = events.filter((e) => e.standard === 'ard' && e.via === 'well-known');
        const aiCatEvents = events.filter(
          (e) => e.standard === 'ai-catalog' && e.via === 'well-known'
        );
        assert.equal(ardEvents.length, 1, `one ard event: ${result.stdout}`);
        assert.equal(aiCatEvents.length, 1, `one ai-catalog event: ${result.stdout}`);
        assert.equal(ardEvents[0].summary.name, 'ARD Only');
        assert.equal(aiCatEvents[0].summary.name, 'Catalog Only');
        assert.ok(
          !ardEvents[0].aliases || ardEvents[0].aliases.length === 0,
          'no aliases when different'
        );
      } finally {
        diffServer.closeAllConnections();
        diffServer.close();
      }
    });

    await t.test('Link rel=ai-catalog header', async () => {
      const linkCatServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            link: '</.well-known/ai-catalog.json>; rel="ai-catalog"',
          });
          res.end('<!doctype html><title>Link AI Cat</title><p>hi</p>');
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => linkCatServer.listen(0, '127.0.0.1', r));
      const linkCatOrigin = `http://127.0.0.1:${linkCatServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--once', `${linkCatOrigin}/`],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);
        const events = parseEvents(result.stdout);
        const aiCatLink = events.find(
          (e) => e.standard === 'ai-catalog' && e.via === 'link-header'
        );
        assert.ok(aiCatLink, `ai-catalog link-header event: ${result.stdout}`);
        assert.ok(
          aiCatLink.url.includes('/.well-known/ai-catalog.json'),
          `href resolved: ${aiCatLink.url}`
        );
      } finally {
        linkCatServer.closeAllConnections();
        linkCatServer.close();
      }
    });

    await t.test('html link rel=ai-catalog from page scan', async () => {
      const htmlCatServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            link: '</catalog.json>; rel="ai-catalog"',
          });
          res.end(
            '<!doctype html><html><head><link rel="ai-catalog" href="/catalog.json"><title>HTML Cat</title></head><body><p>hi</p></body></html>'
          );
          return;
        }
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
      });
      await new Promise((r) => htmlCatServer.listen(0, '127.0.0.1', r));
      const htmlCatOrigin = `http://127.0.0.1:${htmlCatServer.address().port}`;
      try {
        const cwd = await runner.fresh();
        await runner.clearSession(cwd);
        const result = await runner.playwright(
          ['discover', '--json', '--no-probe', '--once', `${htmlCatOrigin}/`],
          cwd
        );
        assert.equal(
          result.code,
          0,
          `exit ${result.code}, stderr: ${result.stderr}, stdout: ${result.stdout}`
        );
        const events = parseEvents(result.stdout);
        const catEvent = events.find((e) => e.standard === 'ai-catalog');
        assert.ok(catEvent, `ai-catalog event expected: ${result.stdout}`);
        assert.ok(
          catEvent.via === 'html-link' || catEvent.via === 'link-header',
          `via must be html-link or link-header: ${catEvent.via}`
        );
        assert.ok(catEvent.url.includes('/catalog.json'), `href resolved: ${catEvent.url}`);
      } finally {
        htmlCatServer.closeAllConnections();
        htmlCatServer.close();
      }
    });
  } finally {
    await settle(browser?.close());
  }
});

test('discover poppy', { timeout: 120_000 }, async (t) => {
  let browser;
  let poppyServer;
  let redirectServer;
  let crossSiteServer;
  let protectedServer;
  let mismatchServer;
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

    const validPoppy = JSON.stringify({
      protocol_version: '0.1',
      organization: { name: 'Acme Corp', domain: '127.0.0.1' },
      web: { home: '/' },
    });

    poppyServer = createServer((req, res) => {
      if (req.url === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Poppy</title>');
        return;
      }
      if (req.url === '/.well-known/poppy.json') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(validPoppy);
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => poppyServer.listen(0, '127.0.0.1', resolve));
    const poppyOrigin = `http://127.0.0.1:${poppyServer.address().port}`;

    await t.test('direct poppy.json hit', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${poppyOrigin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const poppy = events.find((e) => e.standard === 'poppy' && e.via === 'well-known');
      assert.ok(poppy, `poppy event expected: ${result.stdout}`);
      assert.equal(poppy.status, 'valid');
      assert.equal(poppy.summary.protocolVersion, '0.1');
      assert.equal(poppy.summary.organization.name, 'Acme Corp');
    });

    redirectServer = createServer((req, res) => {
      if (req.url === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Redirect</title>');
        return;
      }
      if (req.url === '/.well-known/poppy.json') {
        res.writeHead(302, { location: `${poppyOrigin}/.well-known/poppy.json` });
        res.end();
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => redirectServer.listen(0, '127.0.0.1', resolve));
    const redirectOrigin = `http://127.0.0.1:${redirectServer.address().port}`;

    await t.test('same-site redirect hit', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${redirectOrigin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const poppy = events.find((e) => e.standard === 'poppy');
      assert.ok(poppy, `poppy after redirect expected: ${result.stdout}`);
      assert.equal(poppy.status, 'valid');
    });

    let crossSiteRequests = 0;
    crossSiteServer = createServer((req, res) => {
      if (req.url === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Cross</title>');
        return;
      }
      if (req.url === '/.well-known/poppy.json') {
        res.writeHead(302, { location: 'http://evil.test/.well-known/poppy.json' });
        res.end();
        return;
      }
      crossSiteRequests++;
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => crossSiteServer.listen(0, '127.0.0.1', resolve));
    const crossSiteOrigin = `http://127.0.0.1:${crossSiteServer.address().port}`;

    await t.test('cross-site redirect refused', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${crossSiteOrigin}/`],
        cwd
      );
      const events = parseEvents(result.stdout);
      const poppy = events.find((e) => e.standard === 'poppy');
      assert.ok(!poppy, `no poppy event after cross-site redirect: ${result.stdout}`);
    });

    protectedServer = createServer((req, res) => {
      if (req.url === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Protected</title>');
        return;
      }
      if (req.url === '/.well-known/poppy.json') {
        res.writeHead(302, { location: 'http://vault.sliccy.ai/.well-known/poppy.json' });
        res.end();
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
    const protectedOrigin = `http://127.0.0.1:${protectedServer.address().port}`;

    await t.test('redirect to protected origin refused', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${protectedOrigin}/`],
        cwd
      );
      const events = parseEvents(result.stdout);
      const poppy = events.find((e) => e.standard === 'poppy');
      assert.ok(!poppy, `no poppy after redirect to protected: ${result.stdout}`);
    });

    const mismatchPoppy = JSON.stringify({
      protocol_version: '0.1',
      organization: { name: 'Evil', domain: 'evil.com' },
      web: { home: '/' },
    });
    mismatchServer = createServer((req, res) => {
      if (req.url === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Mismatch</title>');
        return;
      }
      if (req.url === '/.well-known/poppy.json') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(mismatchPoppy);
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => mismatchServer.listen(0, '127.0.0.1', resolve));
    const mismatchOrigin = `http://127.0.0.1:${mismatchServer.address().port}`;

    await t.test('domain mismatch flagged', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${mismatchOrigin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const poppy = events.find((e) => e.standard === 'poppy');
      assert.ok(poppy, `poppy event expected: ${result.stdout}`);
      assert.equal(poppy.status, 'invalid');
      assert.ok(poppy.reason.includes('does not match'), `reason: ${poppy.reason}`);
    });
  } finally {
    poppyServer?.closeAllConnections();
    poppyServer?.close();
    redirectServer?.closeAllConnections();
    redirectServer?.close();
    crossSiteServer?.closeAllConnections();
    crossSiteServer?.close();
    protectedServer?.closeAllConnections();
    protectedServer?.close();
    mismatchServer?.closeAllConnections();
    mismatchServer?.close();
    await settle(browser?.close());
  }
});

test('discover NLWeb', { timeout: 120_000 }, async (t) => {
  let browser;
  let nlwebServer;
  let catalogServer;
  let plainMcpServer;
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

    nlwebServer = createServer((req, res) => {
      const url = new URL(req.url || '/', 'http://127.0.0.1');
      if (url.pathname === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>NLWeb</title>');
        return;
      }
      if (url.pathname === '/ask') {
        if (!url.searchParams.get('query')) {
          res.writeHead(400, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ error: 'query is required' }));
          return;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ response: 'hello', results: [] }));
        return;
      }
      if (url.pathname === '/mcp' && req.method === 'POST') {
        const accept = req.headers.accept || '';
        if (!accept.includes('application/json') || !accept.includes('text/event-stream')) {
          res.writeHead(406, { 'content-type': 'text/plain' });
          res.end('not acceptable');
          return;
        }
        let body = '';
        req.on('data', (c) => (body += c));
        req.on('end', () => {
          const result = JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            result: {
              serverInfo: { name: 'NLWeb Server', version: '0.5' },
              protocolVersion: '2025-03-26',
            },
          });
          res.writeHead(200, { 'content-type': 'text/event-stream' });
          const note = JSON.stringify({
            jsonrpc: '2.0',
            method: 'notifications/message',
            params: { level: 'info', data: '"result"' },
          });
          res.write(`event: message\ndata: ${note}\n\n`);
          setTimeout(() => res.write(`event: message\ndata: ${result}\n\n`), 200);
        });
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => nlwebServer.listen(0, '127.0.0.1', resolve));
    const nlwebOrigin = `http://127.0.0.1:${nlwebServer.address().port}`;

    catalogServer = createServer((req, res) => {
      const url = new URL(req.url || '/', 'http://127.0.0.1');
      if (url.pathname === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Catalog</title>');
        return;
      }
      if (url.pathname === '/.well-known/ard.json') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            name: 'NLWeb Catalog',
            entries: [
              {
                identifier: 'urn:nlweb:search',
                displayName: 'NLWeb Search',
                type: 'application/nlweb+json',
                url: `${nlwebOrigin}/ask`,
              },
            ],
          })
        );
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => catalogServer.listen(0, '127.0.0.1', resolve));
    const catalogOrigin = `http://127.0.0.1:${catalogServer.address().port}`;

    plainMcpServer = createServer((req, res) => {
      const url = new URL(req.url || '/', 'http://127.0.0.1');
      if (url.pathname === '/') {
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<title>Plain MCP</title>');
        return;
      }
      if (url.pathname === '/mcp' && req.method === 'POST') {
        let body = '';
        req.on('data', (c) => (body += c));
        req.on('end', () => {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              jsonrpc: '2.0',
              id: 1,
              result: {
                serverInfo: { name: 'Generic MCP', version: '1.0' },
                protocolVersion: '2025-03-26',
              },
            })
          );
        });
        return;
      }
      res.writeHead(404, { 'content-type': 'text/plain' });
      res.end('not found');
    });
    await new Promise((resolve) => plainMcpServer.listen(0, '127.0.0.1', resolve));
    const plainMcpOrigin = `http://127.0.0.1:${plainMcpServer.address().port}`;

    await t.test('catalog-pointed NLWeb hit', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${catalogOrigin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const nlweb = events.find((e) => e.standard === 'nlweb');
      assert.ok(nlweb, `nlweb event expected: ${result.stdout}`);
      assert.equal(nlweb.via, 'catalog');
      assert.ok(nlweb.summary.askUrl, 'askUrl expected');
    });

    await t.test('--probe nlweb hit', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--probe', 'nlweb', '--once', `${nlwebOrigin}/`],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);
      const events = parseEvents(result.stdout);
      const nlweb = events.find((e) => e.standard === 'nlweb' && e.via === 'well-known');
      assert.ok(nlweb, `nlweb probe event expected: ${result.stdout}`);
      assert.equal(nlweb.summary.askUrl, `${nlwebOrigin}/ask`, result.stdout);
      assert.equal(nlweb.url, `${nlwebOrigin}/ask`);
      assert.ok(nlweb.summary.mcpUrl, `mcpUrl expected: ${result.stdout}`);
      assert.equal(nlweb.summary.serverInfo.name, 'NLWeb Server');
    });

    await t.test('no probe without the flag', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--once', `${nlwebOrigin}/`],
        cwd
      );
      const events = result.stdout.trim() ? parseEvents(result.stdout) : [];
      const nlwebProbe = events.find((e) => e.standard === 'nlweb' && e.via === 'well-known');
      assert.ok(!nlwebProbe, `nlweb probe should not fire without --probe nlweb: ${result.stdout}`);
    });

    await t.test('plain MCP server is not NLWeb', async () => {
      const cwd = await runner.fresh();
      await runner.clearSession(cwd);
      const result = await runner.playwright(
        ['discover', '--json', '--probe', 'nlweb', '--once', `${plainMcpOrigin}/`],
        cwd
      );
      const events = result.stdout.trim() ? parseEvents(result.stdout) : [];
      const nlweb = events.find((e) => e.standard === 'nlweb');
      assert.ok(!nlweb, `plain MCP should not produce nlweb event: ${result.stdout}`);
    });
  } finally {
    nlwebServer?.closeAllConnections();
    nlwebServer?.close();
    catalogServer?.closeAllConnections();
    catalogServer?.close();
    plainMcpServer?.closeAllConnections();
    plainMcpServer?.close();
    await settle(browser?.close());
  }
});
