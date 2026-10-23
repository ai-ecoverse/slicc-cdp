import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { createServer as createTlsServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { before } from 'node:test';
import { launchBrowser } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { guarded } from './guard-client.mjs';

const marker = 'USERINFO_CARRIER_SECRET_520';
const control = 'USERINFO_CARRIER_CONTROL_520';
const codes = ['%2F', '%5c', '%3F', '%23', '%252F'];
const kinds = ['refresh', 'location', 'content-location', 'link'];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

before(() => {
  if (chromeSkipReason()) return;
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync(
      'cargo',
      ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
});

for (const transport of ['native', 'wasi']) {
  test(`percent-encoded userinfo carriers stay guarded through ${transport}`, {
    timeout: 300000,
  }, async () => {
    const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-userinfo-520-'));
    const key = path.join(scratch, 'key.pem');
    const cert = path.join(scratch, 'cert.pem');
    execFileSync(
      'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-keyout',
        key,
        '-out',
        cert,
        '-days',
        '1',
        '-subj',
        '/CN=seven.sliccy.ai',
      ],
      { stdio: 'ignore' }
    );
    const handler = (req, res) => {
      const parsed = new URL(req.url, 'http://example.test');
      if (req.headers.host?.includes('sliccy.ai')) {
        res.setHeader('Content-Type', 'text/html');
        res.end(`<html><body>${marker}</body></html>`);
        return;
      }
      if (parsed.pathname !== '/carrier') {
        res.setHeader('Content-Type', 'text/html');
        res.end(`<html><body>ordinary ${parsed.searchParams.get('control') ?? ''}</body></html>`);
        return;
      }
      const kind = parsed.searchParams.get('kind');
      const code = parsed.searchParams.get('code');
      const ordinary = parsed.searchParams.get('ordinary') === '1';
      const target = ordinary
        ? `http://evil.test${code}@${req.headers.host}/landing?control=${control}`
        : `https://evil.test${code}@seven.sliccy.ai/landing?code=${marker}`;
      if (kind === 'location') {
        res.statusCode = 302;
        res.setHeader('Location', target);
      } else if (kind === 'refresh') res.setHeader('Refresh', `${ordinary ? 30 : 0};url=${target}`);
      else if (kind === 'content-location') res.setHeader('Content-Location', target);
      else res.setHeader('Link', `<${target}>; rel=preconnect`);
      res.setHeader('Content-Type', 'text/html');
      res.end(`<html><body>carrier ${ordinary ? control : marker}</body></html>`);
    };
    const server = createServer(handler);
    const tls = createTlsServer({ key: readFileSync(key), cert: readFileSync(cert) }, handler);
    const messages = [];
    const failures = [];
    let browser;
    let client;
    const call = async (method, params, session) => {
      try {
        return await client.send(method, params, session);
      } catch {
        return null;
      }
    };
    const previous = process.env.SLICC_GUARD_TRANSPORT;
    try {
      await new Promise((resolve) => tls.listen(0, '127.0.0.1', resolve));
      await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
      const port = server.address().port;
      browser = await launchBrowser([
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tls.address().port}`,
        '--ignore-certificate-errors',
        '--no-proxy-server',
      ]);
      process.env.SLICC_GUARD_TRANSPORT = transport;
      client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        onMessage: (message) => messages.push(message),
      });
      for (const kind of kinds) {
        for (const code of codes) {
          for (const ordinary of [false, true]) {
            const label = `${kind} ${code} ${ordinary ? 'control' : 'protected'}`;
            const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
            const { sessionId } = await client.send('Target.attachToTarget', {
              targetId,
              flatten: true,
            });
            for (const method of ['Network.enable', 'Page.enable'])
              await client.send(method, {}, sessionId);
            const start = messages.length;
            const url = `http://127.0.0.1:${port}/carrier?kind=${kind}&code=${encodeURIComponent(code)}${ordinary ? '&ordinary=1' : ''}`;
            await call('Page.navigate', { url, __guardBridgeNotify: true }, sessionId);
            await delay(1500);
            const requests = client.events.filter(
              (event) =>
                event.sessionId === sessionId &&
                event.method === 'Network.requestWillBeSent' &&
                event.params.request.url.includes('/carrier')
            );
            if (!requests.length && (ordinary || kind !== 'location'))
              failures.push(`${label}: no carrier request`);
            for (const event of requests) {
              const body = await call(
                'Network.getResponseBody',
                { requestId: event.params.requestId },
                sessionId
              );
              if (ordinary && kind !== 'location' && !body?.body?.includes(control))
                failures.push(`${label}: control body unreadable`);
              if (!ordinary && body) failures.push(`${label}: carrier body readable`);
            }
            if (messages.slice(start).some((message) => JSON.stringify(message).includes(marker)))
              failures.push(`${label}: protected marker reached the client`);
            await client.send('Target.closeTarget', { targetId });
          }
        }
      }
    } finally {
      await browser?.close();
      await client?.close();
      server.closeAllConnections();
      tls.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      await new Promise((resolve) => tls.close(resolve));
      rmSync(scratch, { recursive: true, force: true });
      if (previous === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
      else process.env.SLICC_GUARD_TRANSPORT = previous;
    }
    assert.deepEqual(failures, []);
  });
}
