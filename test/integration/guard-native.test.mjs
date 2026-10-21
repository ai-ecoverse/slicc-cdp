import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import test from 'node:test';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const variants = { prompt: 2000, lagging: 8000 };

async function until(check, ms, what) {
  const deadline = Date.now() + ms;
  while (!(await check())) {
    assert(Date.now() < deadline, `timed out waiting for ${what}`);
    await sleep(10);
  }
}

async function open(client, url) {
  const { targetId } = await client.send('Target.createTarget', {
    url: 'about:blank',
    background: true,
  });
  const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
  await client.send('Page.enable', {}, sessionId);
  const loaded = client.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === sessionId,
    15000
  );
  await client.send('Page.navigate', { url }, sessionId);
  await loaded;
  return { targetId, sessionId };
}

function intercept(client, sessionId, url, body) {
  let stopped = false;
  const replies = [];
  const seen = new Set();
  const pumping = (async () => {
    while (!stopped) {
      for (const event of client.events) {
        if (event.method !== 'Fetch.requestPaused' || seen.has(event)) continue;
        seen.add(event);
        const match =
          event.sessionId === sessionId &&
          event.params.request.url === url &&
          !event.params.responseStatusCode;
        replies.push(
          client.send(
            match ? 'Fetch.fulfillRequest' : 'Fetch.continueRequest',
            {
              requestId: event.params.requestId,
              ...(match
                ? {
                    responseCode: 200,
                    responseHeaders: [
                      { name: 'content-type', value: 'text/javascript' },
                      { name: 'access-control-allow-origin', value: '*' },
                    ],
                    body: Buffer.from(body).toString('base64'),
                  }
                : {}),
            },
            event.sessionId
          )
        );
      }
      await sleep(10);
    }
  })();
  return async () => {
    stopped = true;
    await pumping;
    await Promise.all(replies);
  };
}

test('native-only C5.script and X1.after-exit', { timeout: 180000 }, async (t) => {
  execFileSync('cargo', ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'], {
    stdio: 'inherit',
  });
  const fixture = await fixtureServer();
  const targets = origins(fixture);
  try {
    for (const [variant, detachDelay] of Object.entries(variants)) {
      for (const target of ['ordinary', 'seven', 'vault']) {
        await t.test(`C5.script ${target} ${variant}`, async () => {
          const browser = await launchBrowser(browserArgs());
          const client = await guarded(browser.wsUrl, targets.client, { variant, detachDelay });
          let stop;
          try {
            const tag = `script-${target}-${variant}`;
            const url = `${targets[target]}/script?n=${tag}`;
            const { sessionId } = await open(client, `${targets.ordinary}/doc`);
            await client.send(
              'Fetch.enable',
              {
                patterns: [
                  { urlPattern: '*', requestStage: 'Request' },
                  { urlPattern: '*', requestStage: 'Response' },
                ],
              },
              sessionId
            );
            stop = intercept(client, sessionId, url, "globalThis.vectorScript='fulfilled'");
            const result = await client.send(
              'Runtime.evaluate',
              {
                expression: `new Promise(resolve=>{const s=document.createElement('script');s.onload=()=>resolve(String(globalThis.vectorScript));s.onerror=()=>resolve('error');s.src=${JSON.stringify(url)};document.head.append(s);})`,
                awaitPromise: true,
                returnByValue: true,
              },
              sessionId
            );
            assert.equal(result.exceptionDetails, undefined);
            assert.equal(result.result.value, target === 'ordinary' ? 'fulfilled' : `real-${tag}`);
            const paused = client.events.filter(
              (event) => event.method === 'Fetch.requestPaused' && event.params.request.url === url
            );
            assert.equal(paused.length > 0, target === 'ordinary');
            if (target === 'ordinary') {
              assert(!Array.isArray(paused[0].params.request.headers));
              assert.equal(typeof paused[0].params.request.headers, 'object');
              assert(
                Object.keys(paused[0].params.request.headers).some(
                  (name) => name.toLowerCase() === 'user-agent'
                )
              );
            }
            if (target !== 'ordinary') assert(fixture.log().some((entry) => entry.url === url));
          } finally {
            await stop?.();
            await client.close();
            await browser.close();
          }
        });
        await t.test(`X1.after-exit ${target} ${variant}`, async () => {
          const browser = await launchBrowser(browserArgs());
          const client = await guarded(browser.wsUrl, targets.client, { variant, detachDelay });
          const raw = await observer(browser.wsUrl);
          let stop;
          try {
            const tag = `exit-${target}-${variant}`;
            const dest = `${targets[target]}/canary/${tag}`;
            const { targetId, sessionId } = await open(client, `${targets.ordinary}/doc`);
            await client.send(
              'Page.addScriptToEvaluateOnNewDocument',
              {
                source: `navigator.sendBeacon(${JSON.stringify(`${targets.ordinary}/beacon?tag=${tag}`)},location.href)`,
              },
              sessionId
            );
            await client.send('Network.enable', {}, sessionId);
            await client.send(
              'Network.setExtraHTTPHeaders',
              { headers: { 'x-probe': tag } },
              sessionId
            );
            await client.send('Fetch.enable', { patterns: [{ urlPattern: '*' }] }, sessionId);
            stop = intercept(client, sessionId, dest, 'FULFILLED');
            const trigger = `${targets.ordinary}/page?k=href&to=${encodeURIComponent(dest)}&tag=${tag}`;
            const beacon = (entry) =>
              new URL(entry.url).pathname === '/beacon' &&
              new URL(entry.url).searchParams.get('tag') === tag;
            const loaded = client.waitEvent(
              (event) => event.method === 'Page.loadEventFired' && event.sessionId === sessionId,
              15000
            );
            await client.send(
              'Runtime.evaluate',
              {
                expression: `setTimeout(()=>{location.href=${JSON.stringify(trigger)}},0);true`,
                returnByValue: true,
              },
              sessionId
            );
            await loaded;
            await until(() => fixture.log().some(beacon), 15000, 'beacon before exit');
            assert(
              client.events.some(
                (event) =>
                  event.method === 'Fetch.requestPaused' && event.params.request.url === trigger
              )
            );
            assert(
              fixture
                .log()
                .some((entry) => entry.url === trigger && entry.headers['x-probe'] === tag)
            );
            await stop();
            stop = undefined;
            const exited = Date.now();
            await client.close();
            await until(
              async () =>
                !(await raw.send('Target.getTargetInfo', { targetId })).targetInfo.attached,
              15000,
              'target detached after exit'
            );
            const { sessionId: observed } = await raw.send('Target.attachToTarget', {
              targetId,
              flatten: true,
            });
            await raw.send('Page.enable', {}, observed);
            const arrived = raw.waitEvent(
              (event) => event.method === 'Page.loadEventFired' && event.sessionId === observed,
              15000
            );
            await raw.send(
              'Runtime.evaluate',
              { expression: 'setTimeout(go,0);true', returnByValue: true },
              observed
            );
            await arrived;
            const read = await raw.send(
              'Runtime.evaluate',
              {
                expression: '({href:location.href,body:document.body.innerText})',
                returnByValue: true,
              },
              observed
            );
            assert.equal(read.result.value.href, dest);
            assert(!read.result.value.body.includes('FULFILLED'));
            const hits = fixture.log().filter((entry) => entry.url === dest);
            assert(hits.length > 0);
            assert(hits.every((entry) => entry.time >= exited && !entry.headers['x-probe']));
            assert(!fixture.log().some((entry) => beacon(entry) && entry.time >= exited));
            assert(
              !client.events.some(
                (event) =>
                  event.method === 'Fetch.requestPaused' && event.params.request.url === dest
              )
            );
          } finally {
            await stop?.();
            await client.close();
            await raw.close();
            await browser.close();
          }
        });
      }
    }
  } finally {
    await fixture.close();
  }
});
