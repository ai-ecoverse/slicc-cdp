import assert from 'node:assert/strict';
import { appendFileSync, mkdirSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createServer as createTlsServer } from 'node:https';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser } from './browser.mjs';
import { guarded } from './guard-client.mjs';

const marker = 'P0_REDIRECT_SECRET_7F9A';
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

for (const transport of ['native', 'wasi']) {
  test(`protected redirects redact secrets through ${transport}`, {
    timeout: 180000,
    skip: !process.env.SLICC_REDIRECT_KEY || !process.env.SLICC_REDIRECT_CERT,
  }, async () => {
    const directory = process.env.SLICC_REDIRECT_EVIDENCE;
    if (directory) mkdirSync(directory, { recursive: true });
    const messages = [];
    const results = [];
    const handler = (req, res) => {
      const parsed = new URL(req.url, 'http://example.test');
      const host = parsed.searchParams.get('host') ?? 'seven';
      const secret = `https://${host}.sliccy.ai/${host === 'seven' ? 'auth/callback' : 'oauth/mcp-callback'}?code=${marker}&state=${marker}#access_token=${marker}`;
      if (parsed.pathname === '/redirect') {
        const kind = parsed.searchParams.get('kind');
        res.statusCode = 302;
        res.setHeader('Location', kind === 'location' ? secret : '/ordinary');
        if (kind === 'link') res.setHeader('Link', `<${secret}>; rel=preconnect`);
        if (kind === 'refresh') res.setHeader('Refresh', `0;url=${secret}`);
        res.end(`Redirect to ${secret}`);
      } else {
        res.setHeader('Content-Type', 'text/html');
        res.end(
          req.headers.host?.includes('sliccy.ai')
            ? `<html><body>${marker}</body></html>`
            : '<html><body>ordinary</body></html>'
        );
      }
    };
    const server = createServer(handler);
    const tls = createTlsServer(
      {
        key: readFileSync(process.env.SLICC_REDIRECT_KEY),
        cert: readFileSync(process.env.SLICC_REDIRECT_CERT),
      },
      handler
    );
    await new Promise((resolve) => tls.listen(0, '127.0.0.1', resolve));
    const tlsPort = tls.address().port;
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const port = server.address().port;
    const browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${tlsPort}, MAP vault.sliccy.ai 127.0.0.1:${tlsPort}`,
      '--ignore-certificate-errors',
      '--no-proxy-server',
    ]);
    let client;
    process.env.SLICC_GUARD_TRANSPORT = transport;
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
      client = await guarded(browser.wsUrl, 'https://fixture-protected.invalid', {
        variant: 'prompt',
        detachDelay: 0,
        onMessage: record,
      });
      for (const host of ['seven', 'vault']) {
        for (const kind of ['location', 'link', 'refresh']) {
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
              await call(method, {}, sessionId);
            await call(
              'Fetch.enable',
              { patterns: [{ urlPattern: '*', requestStage: 'Response' }] },
              sessionId
            );
            const start = messages.length;
            const url = `http://127.0.0.1:${port}/redirect?host=${host}&kind=${kind}`;
            await call(
              navigate,
              navigate === 'Page.navigate'
                ? { url }
                : { expression: `location.href=${JSON.stringify(url)}` },
              sessionId
            );
            await delay(300);
            const paused = client.events.filter(
              (event) => event.sessionId === sessionId && event.method === 'Fetch.requestPaused'
            );
            for (const event of paused) {
              await call('Fetch.getResponseBody', { requestId: event.params.requestId }, sessionId);
              await call(
                'Fetch.continueResponse',
                { requestId: event.params.requestId },
                sessionId
              );
            }
            await delay(300);
            const requests = client.events.filter(
              (event) =>
                event.sessionId === sessionId && event.method === 'Network.requestWillBeSent'
            );
            for (const event of requests) {
              await call(
                'Network.getResponseBody',
                { requestId: event.params.requestId },
                sessionId
              );
              await call('Network.replayXHR', { requestId: event.params.requestId }, sessionId);
            }
            const tree = await call('Page.getFrameTree', {}, sessionId);
            if (tree?.frameTree?.frame?.id)
              await call(
                'Page.getResourceContent',
                { frameId: tree.frameTree.frame.id, url },
                sessionId
              );
            await call('DOMSnapshot.captureSnapshot', { computedStyles: [] }, sessionId);
            await call('Page.captureSnapshot', { format: 'mhtml' }, sessionId);
            const leaked = messages
              .slice(start)
              .filter((message) => JSON.stringify(message).includes(marker));
            console.log(
              JSON.stringify({
                transport,
                host,
                kind,
                navigate,
                leaks: leaked.map((message) => message.event?.method ?? 'response'),
              })
            );
            await call('Target.closeTarget', { targetId });
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
        console.log(JSON.stringify({ transport, host, loadNetworkResource: loaded }));
        await call('Target.closeTarget', { targetId });
      }
    } finally {
      await client?.close();
      await browser.close();
      await new Promise((resolve) => server.close(resolve));
      await new Promise((resolve) => tls.close(resolve));
      if (directory)
        appendFileSync(
          path.join(directory, `${transport}-results.jsonl`),
          `${results.map((result) => JSON.stringify(result)).join('\n')}\n`
        );
    }
    assert.equal(
      messages.filter((message) => JSON.stringify(message).includes(marker)).length,
      0,
      'protected redirect marker reached client'
    );
  });
}
