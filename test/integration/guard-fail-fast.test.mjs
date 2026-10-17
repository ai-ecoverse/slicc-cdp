import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import http from 'node:http';
import test from 'node:test';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const refusal = /^slicc: refused: /;
const prompt = 1000;
const guard = { variant: 'prompt', detachDelay: 2000 };

async function loaded(client, sessionId, url) {
  const load = client.waitEvent(
    (event) => event.method === 'Page.loadEventFired' && event.sessionId === sessionId,
    15000
  );
  await client.send('Page.navigate', { url }, sessionId);
  await load;
}

async function open(client, url) {
  const { targetId } = await client.send('Target.createTarget', {
    url: 'about:blank',
    background: true,
  });
  const { sessionId } = await client.send('Target.attachToTarget', { targetId, flatten: true });
  await client.send('Page.enable', {}, sessionId);
  await loaded(client, sessionId, url);
  return { targetId, sessionId };
}

function timed(promise) {
  const settled = { done: false };
  const call = promise
    .then(
      (value) => ({ value }),
      (error) => ({ error })
    )
    .finally(() => {
      settled.done = true;
      settled.at = Date.now();
    });
  return { call, settled };
}

async function pendingEvaluate(client, sessionId, expression) {
  const { result } = await client.send('Runtime.evaluate', { expression }, sessionId);
  assert.equal(result.subtype, 'promise');
  const pending = timed(
    client.send(
      'Runtime.awaitPromise',
      { promiseObjectId: result.objectId, returnByValue: true },
      sessionId
    )
  );
  await sleep(500);
  assert.equal(pending.settled.done, false, 'the call must still be pending before the trigger');
  return pending;
}

async function refusedFast({ call, settled }, fired) {
  const { value, error } = await call;
  const spent = settled.at - fired;
  assert.equal(value, undefined, JSON.stringify(value));
  assert.match(error.message, refusal);
  assert(spent < prompt, `the call took ${spent} ms after the trigger: ${error.message}`);
}

async function hanging() {
  const sockets = new Set();
  const server = http.createServer(() => {});
  server.on('connection', (socket) => sockets.add(socket));
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    port: server.address().port,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

test('a pending call fails at once when the guard blocks its session or Chrome detaches it', {
  timeout: 300000,
}, async (t) => {
  execFileSync('cargo', ['build', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'], {
    stdio: 'inherit',
  });
  const fixture = await fixtureServer();
  const targets = origins(fixture);
  const stall = await hanging();
  const each = async (body) => {
    const browser = await launchBrowser(browserArgs());
    const raw = await observer(browser.wsUrl);
    let client;
    try {
      await body({
        raw,
        connect: async () => {
          client = await guarded(browser.wsUrl, targets.client, guard);
          return client;
        },
      });
    } finally {
      await raw.close();
      await client?.close();
      await browser.close();
    }
  };
  try {
    await t.test('Chrome closes the target under an awaited promise', () =>
      each(async ({ raw, connect }) => {
        const client = await connect();
        const { targetId, sessionId } = await open(client, `${targets.ordinary}/doc`);
        const pending = await pendingEvaluate(client, sessionId, 'new Promise(() => {})');
        const fired = Date.now();
        await raw.send('Target.closeTarget', { targetId });
        await refusedFast(pending, fired);
      })
    );
    await t.test('a back navigation restores the protected stand-in from the bfcache', () =>
      each(async ({ raw, connect }) => {
        const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
        const { sessionId: watched } = await raw.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
        await raw.send('Page.enable', {}, watched);
        await loaded(raw, watched, `${targets.client}/doc`);
        await loaded(raw, watched, `${targets.ordinary}/doc`);
        const client = await connect();
        const { sessionId } = await client.send('Target.attachToTarget', {
          targetId,
          flatten: true,
        });
        await client.send('Page.enable', {}, sessionId);
        const { frameTree } = await client.send('Page.getFrameTree', {}, sessionId);
        const pending = timed(
          client.send(
            'Network.loadNetworkResource',
            {
              frameId: frameTree.frame.id,
              url: `${targets.ordinary.replace(/:\d+$/, '')}:${stall.port}/never`,
              options: { disableCache: true, includeCredentials: false },
            },
            sessionId
          )
        );
        await sleep(500);
        assert.equal(pending.settled.done, false, 'the load must still be pending');
        const history = await raw.send('Page.getNavigationHistory', {}, watched);
        const back = history.entries[history.currentIndex - 1];
        assert.equal(back.url, `${targets.client}/doc`);
        const fired = Date.now();
        await raw.send('Page.navigateToHistoryEntry', { entryId: back.id }, watched);
        await refusedFast(pending, fired);
        const after = await client
          .send('Runtime.evaluate', { expression: 'location.href' }, sessionId)
          .then(
            (value) => ({ value }),
            (error) => ({ error })
          );
        assert.match(after.error?.message ?? '', refusal, JSON.stringify(after.value));
      })
    );
    await t.test('another session detaching leaves the call to its own reply', () =>
      each(async ({ raw, connect }) => {
        const client = await connect();
        const { sessionId } = await open(client, `${targets.ordinary}/doc`);
        const other = await open(client, `${targets.ordinary}/doc`);
        const { call } = await pendingEvaluate(
          client,
          sessionId,
          "new Promise((resolve) => setTimeout(() => resolve('own reply'), 2500))"
        );
        await raw.send('Target.closeTarget', { targetId: other.targetId });
        const { value, error } = await call;
        assert.equal(error, undefined, error?.message);
        assert.equal(value.result.value, 'own reply');
      })
    );
  } finally {
    await stall.close();
    await fixture.close();
  }
});
