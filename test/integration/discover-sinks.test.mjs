import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
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

test('playwright-cli discover sinks', { timeout: 120_000 }, async (t) => {
  const site = await startDiscoverServer();
  let browser;
  let tempHome;
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

    await t.test('--exec records stdin and env vars', async () => {
      const tmp = await mkdtemp(path.join(cwd, 'exec-'));
      const record = path.join(tmp, 'stdin.json');
      const envFile = path.join(tmp, 'env.txt');
      const cmd = `cat >> ${record} && echo >> ${record} && echo "$SLICC_DISCOVER_STANDARD|$SLICC_DISCOVER_ORIGIN|$SLICC_DISCOVER_URL" >> ${envFile}`;

      const result = await runner.playwright(
        ['discover', '--once', `${site.origin}/`, '--exec', cmd],
        cwd
      );
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);

      const stdin = await readFile(record, 'utf8');
      const events = stdin
        .trim()
        .split('\n')
        .filter(Boolean)
        .map((line) => JSON.parse(line));
      assert.ok(events.length >= 1, `exec script must receive at least one event: ${stdin}`);
      assert.equal(events[0].v, 1);

      const env = (await readFile(envFile, 'utf8')).trim();
      const parts = env.split('|');
      assert.ok(parts[0].length > 0, `SLICC_DISCOVER_STANDARD must be set: ${env}`);
      assert.ok(parts[1].startsWith('http'), `SLICC_DISCOVER_ORIGIN must be a URL: ${env}`);
      assert.ok(parts[2].startsWith('http'), `SLICC_DISCOVER_URL must be a URL: ${env}`);
    });

    await t.test('--exec with hostile URL does not expand shell metacharacters', async () => {
      const sentinel = path.join(cwd, 'pwned-sentinel');
      const hostileServer = createServer((req, res) => {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.pathname === '/') {
          res.writeHead(200, {
            'content-type': 'text/html; charset=utf-8',
            link: `</$(touch ${sentinel})>; rel="api-catalog"`,
          });
          res.end('<!doctype html><title>Hostile</title>');
          return;
        }
        res.writeHead(404);
        res.end('nope');
      });
      try {
        await new Promise((r) => hostileServer.listen(0, '127.0.0.1', r));
        const hostileOrigin = `http://127.0.0.1:${hostileServer.address().port}`;

        const tmp = await mkdtemp(path.join(cwd, 'hostile-'));
        const record = path.join(tmp, 'stdin.json');
        const envFile = path.join(tmp, 'env.txt');
        const cmd = `cat > ${record} && echo "$SLICC_DISCOVER_URL" > ${envFile}`;

        const result = await runner.playwright(
          ['discover', '--once', `${hostileOrigin}/`, '--no-probe', '--exec', cmd],
          cwd
        );
        assert.equal(result.code, 0, `stderr: ${result.stderr}`);

        const content = await readFile(record, 'utf8');
        assert.ok(content.length > 0, 'exec must receive stdin');
        const event = JSON.parse(content.trim());
        assert.ok(
          event.url.includes('$(touch'),
          `event URL must contain hostile literal: ${event.url}`
        );

        const envUrl = (await readFile(envFile, 'utf8')).trim();
        assert.ok(
          envUrl.includes('$(touch'),
          `env var must contain hostile literal unexpanded: ${envUrl}`
        );

        const { stat } = await import('node:fs/promises');
        try {
          await stat(sentinel);
          assert.fail(`sentinel file ${sentinel} must not exist — injection happened`);
        } catch (e) {
          assert.equal(e.code, 'ENOENT', 'sentinel must not exist');
        }
      } finally {
        hostileServer.closeAllConnections();
        hostileServer.close();
      }
    });

    await t.test('--lick writes discovery file and watches/discover.json', async () => {
      tempHome = await mkdtemp(path.join(cwd, 'home-'));

      const env = { ...process.env, HOME: tempHome };
      delete env.SLICC_CDP_URL;

      const result = await new Promise((resolve, reject) => {
        const child = spawn(
          cli,
          ['--cdp', browser.wsUrl, 'discover', '--once', `${site.origin}/`, '--lick'],
          { cwd, env }
        );
        const out = [];
        const err = [];
        child.stdout.on('data', (chunk) => out.push(chunk));
        child.stderr.on('data', (chunk) => err.push(chunk));
        const timer = setTimeout(() => {
          child.kill('SIGKILL');
          reject(new Error('lick test timed out'));
        }, 45000);
        child.on('close', (code) => {
          clearTimeout(timer);
          resolve({
            code: code ?? 1,
            stdout: Buffer.concat(out).toString('utf8'),
            stderr: Buffer.concat(err).toString('utf8'),
          });
        });
      });
      assert.equal(result.code, 0, `stderr: ${result.stderr}`);

      const sliccDir = path.join(tempHome, '.slicc');
      const discoveries = await readdir(path.join(sliccDir, 'discoveries'));
      const jsonFiles = discoveries.filter((f) => f.endsWith('.json'));
      assert.ok(jsonFiles.length >= 1, `expected at least one discovery file: ${discoveries}`);

      for (const file of jsonFiles) {
        const content = JSON.parse(
          await readFile(path.join(sliccDir, 'discoveries', file), 'utf8')
        );
        assert.equal(content.v, 1);
        assert.ok(content.standard, 'discovery must have a standard field');
        assert.ok(content.origin, 'discovery must have an origin field');

        assert.ok(
          /^[0-9a-f]{64}\.json$/.test(file),
          `file name must be a sha256 hex hash: ${file}`
        );
      }

      const watchPath = path.join(sliccDir, 'watches', 'discover.json');
      const watch = JSON.parse(await readFile(watchPath, 'utf8'));
      assert.equal(watch.glob, '*.json');
      assert.equal(watch.target, 'cone');
      assert.ok(watch.path.endsWith('/discoveries'));
      assert.ok(watch.debounce > 0);
    });
  } finally {
    await settle(browser?.close());
    await settle(site.close());
    if (tempHome) await settle(rm(tempHome, { recursive: true, force: true }));
  }
});
