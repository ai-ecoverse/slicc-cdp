import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { createServer as createTlsServer } from 'node:https';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test, { before } from 'node:test';
import { launchBrowser } from './browser.mjs';
import { guarded } from './guard-client.mjs';

const marker = 'P0_REDIRECT_SECRET_7F9A';
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const forms = [
  marker,
  [...Buffer.from(marker)]
    .map((byte) => `%${byte.toString(16).padStart(2, '0').toUpperCase()}`)
    .join(''),
  [...Buffer.from(marker)].map((byte) => `\\u${byte.toString(16).padStart(4, '0')}`).join(''),
  Buffer.from(marker).toString('base64'),
];

function containsMarker(value) {
  const text = JSON.stringify(value);
  if (forms.some((form) => text.includes(form))) return true;
  if (typeof value === 'string') {
    try {
      if (decodeURIComponent(value).includes(marker)) return true;
    } catch {}
    return Buffer.from(value, 'base64').toString().includes(marker);
  }
  if (Array.isArray(value)) return value.some(containsMarker);
  if (value && typeof value === 'object') return Object.values(value).some(containsMarker);
  return false;
}

before(() => {
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync(
      'cargo',
      ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      {
        stdio: 'inherit',
      }
    );
});

for (const transport of ['native', 'wasi']) {
  test(`protected redirects redact secrets through ${transport}`, { timeout: 300000 }, async () => {
    const scratch = mkdtempSync(path.join(tmpdir(), 'slicc-redirect-427-'));
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
    const directory = process.env.SLICC_REDIRECT_EVIDENCE;
    if (directory) mkdirSync(directory, { recursive: true });
    const messages = [];
    const results = [];
    const failures = [];
    const check = (condition, message) => {
      if (!condition) failures.push(message);
    };
    const handler = (req, res) => {
      const parsed = new URL(req.url, 'http://example.test');
      const host = parsed.searchParams.get('host') ?? 'seven';
      const secret = `https://${host}.sliccy.ai/${marker}/${host === 'seven' ? 'auth/callback' : 'oauth/mcp-callback'}?code=${marker}&state=${marker}#access_token=${marker}`;
      if (parsed.pathname === '/redirect') {
        const kind = parsed.searchParams.get('kind');
        res.statusCode = 302;
        res.setHeader('Location', kind === 'location' ? secret : '/ordinary');
        if (kind === 'link') res.setHeader('Link', `<${secret}>; rel=preconnect`);
        if (kind === 'refresh') res.setHeader('Refresh', `0;url=${secret}`);
        res.setHeader('X-Control', 'ordinary-header');
        res.end(kind === 'control' ? 'ordinary body' : `Redirect to ${secret}`);
      } else {
        res.setHeader('Content-Type', 'text/html');
        res.end(
          req.headers.host?.includes('sliccy.ai')
            ? `<html><body>${marker}</body></html>`
            : '<html><body>ordinary body</body></html>'
        );
      }
    };
    const server = createServer(handler);
    const tls = createTlsServer({ key: readFileSync(key), cert: readFileSync(cert) }, handler);
    let browser;
    let client;
    const previousTransport = process.env.SLICC_GUARD_TRANSPORT;
    const record = (message) => {
      messages.push(message);
      if (directory)
        appendFileSync(
          path.join(directory, `${transport}-messages.jsonl`),
          `${JSON.stringify(message)}\n`
        );
    };
    const call = async (method, params, session) => {
      try {
        const result = await client.send(method, params, session);
        results.push({ method, result });
        return result;
      } catch (error) {
        results.push({ method, error: error.message });
        return null;
      }
    };
    try {
      await new Promise((resolve) => tls.listen(0, '127.0.0.1', resolve));
      await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
      const port = server.address().port;
      const tlsPort = tls.address().port;
      browser = await launchBrowser([
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tlsPort}, MAP vault.sliccy.ai 127.0.0.1:${tlsPort}`,
        '--ignore-certificate-errors',
        '--no-proxy-server',
      ]);
      process.env.SLICC_GUARD_TRANSPORT = transport;
      client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        onMessage: record,
      });
      for (const host of ['seven', 'vault']) {
        for (const kind of ['location', 'link', 'refresh', 'control']) {
          for (const navigate of ['Page.navigate', 'Runtime.evaluate']) {
            const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
            const { sessionId } = await client.send('Target.attachToTarget', {
              targetId,
              flatten: true,
            });
            for (const method of [
              'Network.enable',
              'Page.enable',
              'Runtime.enable',
              'Log.enable',
              'Audits.enable',
            ])
              await client.send(method, {}, sessionId);
            await client.send(
              'Fetch.enable',
              { patterns: [{ urlPattern: '*', requestStage: 'Response' }] },
              sessionId
            );
            const start = messages.length;
            const url = `http://127.0.0.1:${port}/redirect?host=${host}&kind=${kind}`;
            const navigating = call(
              navigate,
              navigate === 'Page.navigate'
                ? { url }
                : { expression: `location.href=${JSON.stringify(url)}` },
              sessionId
            );
            const answered = new Set();
            const deadline = Date.now() + 13000;
            while (Date.now() < deadline) {
              for (const event of client.events.filter(
                (event) => event.sessionId === sessionId && event.method === 'Fetch.requestPaused'
              )) {
                if (answered.has(event.params.requestId)) continue;
                answered.add(event.params.requestId);
                const body = await call(
                  'Fetch.getResponseBody',
                  { requestId: event.params.requestId },
                  sessionId
                );
                if (kind !== 'control' && event.params.request.url.includes('/redirect'))
                  check(body === null, 'Fetch redirect body must be refused');
                await call(
                  'Fetch.continueResponse',
                  { requestId: event.params.requestId },
                  sessionId
                );
              }
              if (
                client.events.some(
                  (event) =>
                    event.sessionId === sessionId && event.method === 'Network.loadingFinished'
                ) ||
                (kind === 'location' && answered.size)
              )
                break;
              await delay(50);
            }
            await navigating;
            check(answered.size > 0, 'Response-stage redirect event must be delivered');
            const requests = client.events.filter(
              (event) =>
                event.sessionId === sessionId && event.method === 'Network.requestWillBeSent'
            );
            check(requests.length > 0, 'ordinary request metadata must be delivered');
            for (const event of requests) {
              const body = await call(
                'Network.getResponseBody',
                { requestId: event.params.requestId },
                sessionId
              );
              const replay = await call(
                'Network.replayXHR',
                { requestId: event.params.requestId },
                sessionId
              );
              if (kind === 'control')
                check(body?.body?.includes('ordinary body'), 'ordinary body must remain readable');
              else {
                check(body === null, 'tainted Network body must be refused');
                check(replay === null, 'tainted replayXHR must be refused');
              }
            }
            const tree = await call('Page.getFrameTree', {}, sessionId);
            await call('Page.getResourceTree', {}, sessionId);
            await call('Page.getNavigationHistory', {}, sessionId);
            if (tree?.frameTree?.frame?.id) {
              const content = await call(
                'Page.getResourceContent',
                { frameId: tree.frameTree.frame.id, url },
                sessionId
              );
              if (kind !== 'control')
                check(content === null, 'tainted resource content must be refused');
            }
            await call('DOMSnapshot.captureSnapshot', { computedStyles: [] }, sessionId);
            check(
              (await call('Page.captureSnapshot', { format: 'mhtml' }, sessionId)) === null,
              'captureSnapshot must be refused'
            );
            const leaked = messages.slice(start).filter(containsMarker);
            console.log(
              JSON.stringify({
                transport,
                host,
                kind,
                navigate,
                leaks: leaked.map((message) => message.event?.method ?? 'response'),
              })
            );
            await client.send('Target.closeTarget', { targetId });
          }
        }
        const { targetId } = await client.send('Target.createTarget', { url: 'about:blank' });
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
        const tree = await client.send('Page.getFrameTree', {}, sessionId);
        const loaded = await call(
          'Network.loadNetworkResource',
          {
            frameId: tree.frameTree.frame.id,
            url: `http://127.0.0.1:${port}/redirect?host=${host}&kind=location`,
            options: { includeCredentials: true, disableCache: true },
          },
          sessionId
        );
        if (loaded?.resource?.stream)
          await call('IO.read', { handle: loaded.resource.stream }, sessionId);
        check(loaded === null, 'loadNetworkResource must be refused');
        await client.send('Runtime.enable', {}, sessionId);
        await client.send(
          'Runtime.evaluate',
          {
            expression: `document.body.innerHTML='<iframe src="http://127.0.0.1:${port}/redirect?host=${host}&kind=location"></iframe>'`,
          },
          sessionId
        );
        await delay(300);
        for (const method of [
          'Page.getFrameTree',
          'Page.getResourceTree',
          'Page.getNavigationHistory',
          'DOMSnapshot.captureSnapshot',
        ])
          await call(
            method,
            method === 'DOMSnapshot.captureSnapshot' ? { computedStyles: [] } : {},
            sessionId
          );
        check(
          (await call('Page.captureSnapshot', { format: 'mhtml' }, sessionId)) === null,
          'iframe captureSnapshot must be refused'
        );
        await client.send(
          'Fetch.enable',
          { patterns: [{ urlPattern: '*', requestStage: 'Request' }] },
          sessionId
        );
        await client.send(
          'Runtime.evaluate',
          { expression: `fetch('http://127.0.0.1:${port}/ordinary').catch(()=>{})` },
          sessionId
        );
        await delay(300);
        const pauses = client.events.filter(
          (event) => event.sessionId === sessionId && event.method === 'Fetch.requestPaused'
        );
        check(pauses.length > 0, 'ordinary Request stage must be handed out for override check');
        for (const event of pauses) {
          const url = `https://${host}.sliccy.ai/auth/callback#access_token=${marker}`;
          check(
            (await call(
              'Fetch.continueRequest',
              { requestId: event.params.requestId, url },
              sessionId
            )) === null,
            'protected URL override must be refused'
          );
          await call('Fetch.continueRequest', { requestId: event.params.requestId }, sessionId);
        }
        await client.send('Target.closeTarget', { targetId });
      }
    } finally {
      try {
        await client?.close();
      } finally {
        await browser?.close();
        server.closeAllConnections();
        tls.closeAllConnections();
        await new Promise((resolve) => server.close(resolve));
        await new Promise((resolve) => tls.close(resolve));
        rmSync(scratch, { recursive: true, force: true });
        if (previousTransport === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
        else process.env.SLICC_GUARD_TRANSPORT = previousTransport;
        if (directory)
          appendFileSync(
            path.join(directory, `${transport}-results.jsonl`),
            `${results.map((result) => JSON.stringify(result)).join('\n')}\n`
          );
      }
    }
    assert.deepEqual(failures, [], 'redirect policy or ordinary control failed');
    assert.equal(
      messages.filter(containsMarker).length,
      0,
      'protected redirect marker reached client in any encoding'
    );
  });
}
