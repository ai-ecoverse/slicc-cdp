import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { before } from 'node:test';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeSkipReason, chromeTest as test } from './chrome-policy.mjs';
import { guarded } from './guard-client.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

before(() => {
  if (chromeSkipReason()) return;
  for (const target of [[], ['--target', 'wasm32-wasip1']]) {
    execFileSync(
      'cargo',
      ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge', ...target],
      { stdio: 'inherit' }
    );
    execFileSync('cargo', ['build', '--locked', '-p', 'slicc-cdp', '-p', 'curlwright', ...target], {
      stdio: 'inherit',
    });
  }
});

function operation(kind, url) {
  const target = JSON.stringify(url);
  const sleep = 'await new Promise(r=>setTimeout(r,300));';
  if (kind === 'xhr')
    return `(async()=>{await new Promise(r=>{const x=new XMLHttpRequest();x.onload=x.onerror=x.ontimeout=r;x.timeout=5000;x.open('GET',${target});x.send()});return 'done'})()`;
  if (kind === 'image')
    return `(async()=>{await new Promise(r=>{const i=new Image();i.onload=i.onerror=r;i.src=${target}});return 'done'})()`;
  if (kind === 'beacon')
    return `(async()=>{navigator.sendBeacon(${target},'ping');${sleep}return 'done'})()`;
  if (kind === 'worker')
    return `(async()=>{await new Promise((r,j)=>{const w=new Worker('/worker.js?to='+encodeURIComponent(${target}));w.onmessage=()=>{w.terminate();r()};w.onerror=j});return 'done'})()`;
  const init =
    kind === 'preflight'
      ? "{method:'PUT',headers:{'X-Guard-Test':'yes'},credentials:'omit'}"
      : "{mode:'no-cors',credentials:'omit'}";
  return `(async()=>{await fetch(${target},${init}).catch(()=>{});return 'done'})()`;
}

for (const transport of ['native', 'wasi']) {
  test(`protected request dispatch stays zero through ${transport}`, {
    timeout: 300000,
  }, async () => {
    const protectedHits = [];
    const ordinaryHits = [];
    const protectedServer = createServer((req, res) => {
      protectedHits.push({ method: req.method, url: req.url, host: req.headers.host });
      res.setHeader('Access-Control-Allow-Origin', '*');
      res.setHeader('Access-Control-Allow-Methods', 'GET,POST,PUT,OPTIONS');
      res.setHeader('Access-Control-Allow-Headers', 'X-Guard-Test');
      res.end('protected');
    });
    let protectedOrigin;
    const ordinary = createServer((req, res) => {
      ordinaryHits.push({
        method: req.method,
        url: req.url,
        authorization: req.headers.authorization,
      });
      const url = new URL(req.url, 'http://ordinary.test');
      res.setHeader('Access-Control-Allow-Origin', '*');
      if (url.pathname === '/redirect') {
        res.writeHead(Number(url.searchParams.get('status') ?? 302), {
          Location: url.searchParams.get('to') ?? `${protectedOrigin}/hop`,
        });
        res.end();
      } else if (url.pathname === '/worker.js') {
        res.setHeader('Content-Type', 'text/javascript');
        res.end(
          `fetch(${JSON.stringify(url.searchParams.get('to'))},{mode:'no-cors'}).catch(()=>{}).then(()=>postMessage('done'))`
        );
      } else if (url.pathname === '/landing') res.end(`${req.method}:ordinary`);
      else {
        res.setHeader('Content-Type', 'text/html');
        res.end('<html><body>ordinary</body></html>');
      }
    });
    let browser;
    let client;
    let raw;
    let runner;
    const previous = process.env.SLICC_GUARD_TRANSPORT;
    try {
      await new Promise((resolve) => protectedServer.listen(0, '127.0.0.1', resolve));
      await new Promise((resolve) => ordinary.listen(0, '127.0.0.1', resolve));
      protectedOrigin = `http://127.0.0.1:${protectedServer.address().port}`;
      const base = `http://127.0.0.1:${ordinary.address().port}`;
      browser = await launchBrowser([
        '--host-resolver-rules=MAP *.sliccy.ai 127.0.0.1',
        '--no-proxy-server',
      ]);
      raw = await rawSocket(browser.wsUrl);
      const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
      const { sessionId: rawSession } = await raw.send('Target.attachToTarget', {
        targetId,
        flatten: true,
      });
      await raw.send('Page.enable', {}, rawSession);
      const loaded = raw.event('Page.loadEventFired', rawSession);
      await raw.send('Page.navigate', { url: `${base}/page` }, rawSession);
      await loaded.seen;
      const batch = `(async()=>{const samples=[];for(let i=0;i<100;i++){const t=performance.now();await fetch('/control?batch='+i,{cache:'no-store'});samples.push(performance.now()-t)}return samples})()`;
      const percentile = (samples, fraction) =>
        [...samples].sort((a, b) => a - b)[Math.ceil(samples.length * fraction) - 1];
      const start = performance.now();
      const rawBatch = await raw.send(
        'Runtime.evaluate',
        { expression: batch, awaitPromise: true, returnByValue: true },
        rawSession
      );
      const rawMs = performance.now() - start;
      process.env.SLICC_GUARD_TRANSPORT = transport;
      client = await guarded(browser.wsUrl, protectedOrigin, { variant: 'prompt', detachDelay: 0 });
      const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
      const evaluate = async (expression) => {
        const result = await client.send(
          'Runtime.evaluate',
          { expression, awaitPromise: true, returnByValue: true },
          sessionId
        );
        assert.equal(result.exceptionDetails, undefined, JSON.stringify(result));
        assert.equal(result.result.value, 'done');
      };
      const guardedStart = performance.now();
      const guardedBatch = await client.send(
        'Runtime.evaluate',
        { expression: batch, awaitPromise: true, returnByValue: true },
        sessionId
      );
      assert.equal(rawBatch.exceptionDetails, undefined);
      assert.equal(guardedBatch.exceptionDetails, undefined);
      const rawSamples = rawBatch.result.value;
      const guardedSamples = guardedBatch.result.value;
      assert.equal(rawSamples.length, 100);
      assert.equal(guardedSamples.length, 100);
      const guardedMs = performance.now() - guardedStart;
      await mkdir('artifacts', { recursive: true });
      await writeFile(
        `artifacts/protected-request-performance-${transport}.json`,
        JSON.stringify({
          transport,
          requests: 100,
          rawMs,
          guardedMs,
          deltaMs: guardedMs - rawMs,
          rawMedianMs: percentile(rawSamples, 0.5),
          guardedMedianMs: percentile(guardedSamples, 0.5),
          medianOverheadMs: percentile(guardedSamples, 0.5) - percentile(rawSamples, 0.5),
          rawP95Ms: percentile(rawSamples, 0.95),
          guardedP95Ms: percentile(guardedSamples, 0.95),
          p95OverheadMs: percentile(guardedSamples, 0.95) - percentile(rawSamples, 0.95),
          rawSamples,
          guardedSamples,
          interceptionPattern: '*',
        })
      );
      assert.equal(ordinaryHits.filter((hit) => hit.url.startsWith('/control?batch=')).length, 200);
      for (const origin of [
        protectedOrigin,
        `http://seven.sliccy.ai:${protectedServer.address().port}`,
        `http://vault.sliccy.ai:${protectedServer.address().port}`,
      ]) {
        for (const kind of ['redirect', 'fetch', 'xhr', 'image', 'beacon', 'preflight', 'worker']) {
          const url =
            kind === 'redirect'
              ? `${base}/redirect?to=${encodeURIComponent(`${origin}/${kind}`)}`
              : `${origin}/${kind}`;
          await evaluate(operation(kind, url));
          assert.deepEqual(protectedHits, [], `${kind} dispatched a protected request`);
          const control = `${base}/control?kind=${kind}`;
          await evaluate(operation(kind === 'redirect' ? 'fetch' : kind, control));
          assert.ok(
            ordinaryHits.some((hit) => hit.url === `/control?kind=${kind}`),
            `${kind} ordinary control did not reach its server`
          );
        }
      }
      await client.close();
      client = null;
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
      for (const status of [301, 302, 303, 307, 308]) {
        const to = `${base}/landing`;
        const result = await runner.curl(
          [
            '--tab',
            targetId,
            '-d',
            'body',
            `${base}/redirect?status=${status}&to=${encodeURIComponent(to)}`,
          ],
          cwd
        );
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.equal(result.stdout, `${status < 307 ? 'GET' : 'POST'}:ordinary`);
      }
      const refused = await runner.curl(
        [
          '--tab',
          targetId,
          `${base}/redirect?to=${encodeURIComponent(`http://seven.sliccy.ai:${protectedServer.address().port}/curl-hop`)}`,
        ],
        cwd
      );
      assert.notEqual(refused.code, 0);
      assert.deepEqual(protectedHits, [], 'curlwright dispatched a protected redirect');
    } finally {
      await runner?.close();
      await client?.close();
      await raw?.close();
      await browser?.close();
      ordinary.closeAllConnections();
      protectedServer.closeAllConnections();
      await new Promise((resolve) => ordinary.close(resolve));
      await new Promise((resolve) => protectedServer.close(resolve));
      if (previous === undefined) delete process.env.SLICC_GUARD_TRANSPORT;
      else process.env.SLICC_GUARD_TRANSPORT = previous;
    }
  });
}
