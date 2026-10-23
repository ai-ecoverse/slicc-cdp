import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import path from 'node:path';
import { before, test } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { nativeRunner } from './runners.mjs';

before(() => {
  execFileSync('cargo', ['build', '--locked', '-p', 'curlwright'], { stdio: 'inherit' });
});

for (const host of ['ordinary', 'seven.sliccy.ai', 'vault.sliccy.ai']) {
  for (const statuses of [[301], [302], [303], [307], [308], [301, 302, 303, 307, 308]]) {
    test(`curlwright fake ${host === 'ordinary' ? 'follows' : 'refuses'} ${statuses.join(' → ')} into ${host}`, async () => {
      let hits = 0;
      const protectedServer = createServer((_, res) => {
        hits += 1;
        res.end('protected');
      });
      let base;
      const ordinary = createServer((req, res) => {
        const url = new URL(req.url, base);
        if (url.pathname === '/landing') {
          res.end('ordinary');
          return;
        }
        const remaining = url.searchParams.get('statuses').split(',').map(Number);
        res.writeHead(remaining[0], {
          Location:
            remaining.length === 1
              ? host === 'ordinary'
                ? `${base}/landing`
                : `http://${host}/protected`
              : `${base}/hop?statuses=${remaining.slice(1).join(',')}`,
        });
        res.end();
      });
      let backend;
      let runner;
      let requestId = 0;
      try {
        await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
        await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
        base = `http://127.0.0.1:${ordinary.address().port}`;
        const protectedBase = `http://127.0.0.1:${protectedServer.address().port}`;
        backend = await fakeCdp({
          intercept(message, send) {
            if (
              message.method !== 'Runtime.evaluate' ||
              !message.params.expression.includes('const __init =')
            )
              return false;
            const browserFetch = async (url, init) => {
              let source = url;
              let options = { ...init, redirect: 'manual' };
              for (let hop = 0; hop <= 20; hop += 1) {
                const id = `R${++requestId}`;
                const actual = new URL(source).hostname.endsWith('sliccy.ai')
                  ? `${protectedBase}/protected`
                  : source;
                send({
                  method: 'Network.requestWillBeSent',
                  sessionId: message.sessionId,
                  params: {
                    requestId: id,
                    request: { url: source, method: options.method, headers: {} },
                  },
                });
                const response = await fetch(actual, options);
                const location = response.headers.get('location');
                const redirected = location && [301, 302, 303, 307, 308].includes(response.status);
                const metadata = {
                  url: source,
                  status: response.status,
                  headers: Object.fromEntries(response.headers),
                };
                send({
                  method: redirected ? 'Network.requestWillBeSent' : 'Network.responseReceived',
                  sessionId: message.sessionId,
                  params: {
                    requestId: id,
                    ...(redirected
                      ? {
                          request: {
                            url: new URL(location, source).href,
                            method: options.method,
                            headers: {},
                          },
                          redirectResponse: metadata,
                        }
                      : { response: metadata }),
                  },
                });
                if (redirected) {
                  if (init.redirect === 'manual') return { type: 'opaqueredirect' };
                  source = new URL(location, source).href;
                  if (response.status < 307)
                    options = { ...options, method: 'GET', body: undefined };
                  continue;
                }
                return response;
              }
              throw new Error('redirect limit');
            };
            const evaluate = new Function(
              'fetch',
              'location',
              `return ${message.params.expression}`
            );
            Promise.resolve()
              .then(() => evaluate(browserFetch, { href: `${base}/page` }))
              .then(
                (value) => send({ id: message.id, result: { result: { value } } }),
                (error) =>
                  send({ id: message.id, result: { exceptionDetails: { text: String(error) } } })
              );
            return true;
          },
        });
        backend.targets.push({ targetId: 'TAB', url: `${base}/page`, loaderId: 'L' });
        runner = nativeRunner(null, path.resolve('target/debug/curlwright'), backend.url);
        const cwd = await runner.fresh();
        const result = await runner.curl(
          ['--tab', 'TAB', `${base}/hop?statuses=${statuses.join(',')}`],
          cwd
        );
        assert.equal(hits, 0, 'protected fixture received a redirect request');
        if (host === 'ordinary') {
          assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
          assert.equal(result.stdout, 'ordinary');
        } else {
          assert.notEqual(result.code, 0);
          assert.match(result.stderr, /slicc: refused:/);
        }
      } finally {
        await runner?.close();
        await backend?.close();
        ordinary.closeAllConnections();
        protectedServer.closeAllConnections();
        await new Promise((resolve) => ordinary.close(resolve));
        await new Promise((resolve) => protectedServer.close(resolve));
      }
    });
  }
}
