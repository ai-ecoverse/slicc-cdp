import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import path from 'node:path';
import { before } from 'node:test';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

before(() => {
  if (chromeSkipReason()) return;
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync('cargo', ['build', '--locked', '-p', 'slicc-cdp', '-p', 'curlwright', ...target], {
      stdio: 'inherit',
    });
});

for (const transport of ['native', 'wasi']) {
  test(`curlwright redirect hops never dispatch to protected hosts through ${transport}`, {
    timeout: 180000,
  }, async () => {
    const protectedHits = [];
    const protectedServer = createServer((req, res) => {
      protectedHits.push({ method: req.method, url: req.url, host: req.headers.host });
      res.setHeader('Access-Control-Allow-Origin', req.headers.origin ?? '*');
      res.setHeader('Access-Control-Allow-Credentials', 'true');
      res.end('protected');
    });
    const ordinaryHits = [];
    let base;
    let protectedPort;
    const ordinary = createServer((req, res) => {
      ordinaryHits.push({ method: req.method, url: req.url });
      const url = new URL(req.url, base);
      if (url.pathname !== '/hop') {
        res.setHeader('Content-Type', 'text/html');
        res.end(`${req.method}:ordinary`);
        return;
      }
      const remaining = url.searchParams.get('statuses').split(',').map(Number);
      const host = url.searchParams.get('host');
      const location =
        remaining.length > 1
          ? `${base}/hop?statuses=${remaining.slice(1).join(',')}&host=${host}`
          : host === 'ordinary'
            ? `${base}/landing`
            : `http://${host}:${protectedPort}/protected`;
      res.writeHead(remaining[0], { Location: location });
      res.end();
    });
    let browser;
    let raw;
    let runner;
    try {
      await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
      await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
      protectedPort = protectedServer.address().port;
      base = `http://127.0.0.1:${ordinary.address().port}`;
      browser = await launchBrowser([
        '--host-resolver-rules=MAP *.sliccy.ai 127.0.0.1',
        '--no-proxy-server',
      ]);
      raw = await rawSocket(browser.wsUrl);
      const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
      await raw.send('Page.enable', {}, sessionId);
      const loaded = raw.event('Page.loadEventFired', sessionId);
      await raw.send('Page.navigate', { url: `${base}/page` }, sessionId);
      await loaded.seen;
      runner =
        transport === 'native'
          ? nativeRunner(
              path.resolve('target/debug/playwright-cli'),
              path.resolve('target/debug/curlwright'),
              browser.wsUrl
            )
          : await kernelRunner(
              path.resolve('target/wasm32-wasip1/debug/playwright-cli.wasm'),
              path.resolve('target/wasm32-wasip1/debug/curlwright.wasm'),
              browser.wsUrl
            );
      const cwd = await runner.fresh();
      for (const statuses of [[301], [302], [303], [307], [308], [301, 302, 303, 307, 308]]) {
        const ordinaryResult = await runner.curl(
          [
            '--tab',
            targetId,
            '-d',
            'body',
            `${base}/hop?statuses=${statuses.join(',')}&host=ordinary`,
          ],
          cwd
        );
        assert.equal(ordinaryResult.code, 0, `${ordinaryResult.stdout}\n${ordinaryResult.stderr}`);
        assert.equal(
          ordinaryResult.stdout,
          `${statuses.some((status) => status < 307) ? 'GET' : 'POST'}:ordinary`
        );
        for (const host of ['seven.sliccy.ai', 'vault.sliccy.ai']) {
          const before = ordinaryHits.length;
          const result = await runner.curl(
            [
              '--tab',
              targetId,
              '-d',
              'body',
              `${base}/hop?statuses=${statuses.join(',')}&host=${host}`,
            ],
            cwd
          );
          assert.deepEqual(
            protectedHits,
            [],
            `${statuses.join(' → ')} into ${host} sent a protected request`
          );
          assert.notEqual(result.code, 0);
          assert.match(result.stderr, /slicc: refused:/);
          assert.equal(
            ordinaryHits.slice(before).filter((hit) => hit.url.startsWith('/hop?')).length,
            statuses.length,
            'every ordinary hop must be sent before the protected hop is refused'
          );
        }
      }
    } finally {
      await runner?.close();
      await raw?.close();
      await browser?.close();
      ordinary.closeAllConnections();
      protectedServer.closeAllConnections();
      await new Promise((resolve) => ordinary.close(resolve));
      await new Promise((resolve) => protectedServer.close(resolve));
    }
  });
}
