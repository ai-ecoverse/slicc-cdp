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

    await t.test('stalled body probe gives up within 6s', async () => {
      const stallServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, { 'content-type': 'text/html' });
          res.end('<!doctype html><title>Stall</title>');
          return;
        }
        if (url.pathname === '/.well-known/ard.json') {
          res.writeHead(200, {
            'content-type': 'application/json',
            'content-length': '10000',
          });
          res.write('{"name":"x');
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
        const start = Date.now();
        await runner.playwright(['discover', '--json', '--once', `${stallOrigin}/`], cwd);
        const elapsed = Date.now() - start;
        assert.ok(elapsed < 6000, `probe must give up within 6s, took ${elapsed}ms`);
      } finally {
        stallServer.closeAllConnections();
        stallServer.close();
      }
    });
  } finally {
    await settle(browser?.close());
  }
});

test('discover ARD', { timeout: 120_000 }, async (t) => {
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
