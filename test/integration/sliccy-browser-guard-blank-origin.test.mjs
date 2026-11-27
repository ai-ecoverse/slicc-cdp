import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import { after, test } from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import './watchdog.mjs';

const discovery = new URL('../../packages/sliccy-browser/src/discover.js', import.meta.url).href;
const chosen = { url: '' };
globalThis.sliccBlankOriginEndpoint = chosen;
const hook = registerHooks({
  load(url, context, nextLoad) {
    if (url !== discovery) return nextLoad(url, context);
    return {
      format: 'module',
      shortCircuit: true,
      source:
        'export async function resolveCdpEndpoint() { const { url } = globalThis.sliccBlankOriginEndpoint; if (!url) throw new Error("no fake endpoint"); return { url, tried: ["fake endpoint"] }; }',
    };
  },
});
const { default: browser, REFUSAL } = await import('../../packages/sliccy-browser/src/browser.js');
hook.deregister();

const guardedOrigins = [
  'https://lease-pilot.sliccy.ai',
  'https://seven.sliccy.ai',
  'https://vault.sliccy.ai',
];
const blanks = guardedOrigins.map((origin, index) => ({
  targetId: `BLANK${index + 1}`,
  url: 'about:blank',
  frame: {
    id: `BLANK${index + 1}`,
    loaderId: `LB${index + 1}`,
    url: 'about:blank',
    securityOrigin: origin,
  },
}));
const ordinary = {
  targetId: 'ORDINARY',
  url: 'https://example.test/',
  frame: {
    id: 'ORDINARY',
    loaderId: 'LO',
    url: 'https://example.test/',
    securityOrigin: 'https://example.test',
  },
};

function evaluated(message, send) {
  if (message.method !== 'Runtime.evaluate') return false;
  const expression =
    typeof message.params?.expression === 'string' ? message.params.expression : '';
  let value = 'page-value';
  if (expression.trim() === 'document.baseURI') value = 'https://example.test/';
  if (expression.includes('await fetch(')) {
    value = {
      status: 200,
      statusText: 'OK',
      headers: {},
      body: 'page-body',
      url: 'https://example.test/data',
    };
  }
  send({
    id: message.id,
    ...(message.sessionId ? { sessionId: message.sessionId } : {}),
    result: { result: { type: typeof value === 'string' ? 'string' : 'object', value } },
  });
  return true;
}

const fake = await fakeCdp({
  attachEvents: true,
  intercept: evaluated,
  onCommand: (message, event) => {
    if (message.method !== 'Page.enable' || !message.sessionId) return;
    const owner = fake.calls.at(-1)?.target;
    const tab = [...blanks, ordinary].find((item) => item.targetId === owner);
    if (tab) event('Page.frameNavigated', { frame: tab.frame, type: 'Navigation' });
  },
});
fake.targets.push(...blanks, ordinary);
chosen.url = fake.url;
const previous = process.env.SLICC_CDP_URL;
delete process.env.SLICC_CDP_URL;
after(async () => {
  if (previous === undefined) delete process.env.SLICC_CDP_URL;
  else process.env.SLICC_CDP_URL = previous;
  await fake.close();
});

function evaluations(from, targetId) {
  return fake.calls
    .slice(from)
    .filter((call) => call.method === 'Runtime.evaluate' && call.target === targetId)
    .map((call) => call.params?.expression);
}

test('a discovered blank tab with a protected main-frame origin is refused before any evaluation reaches the browser', async () => {
  for (const blank of blanks) {
    const from = fake.calls.length;
    await assert.rejects(() => browser.eval({ targetId: blank.targetId }, "'blank-probe'"), {
      message: REFUSAL,
    });
    assert.deepEqual(evaluations(from, blank.targetId), [], blank.frame.securityOrigin);
  }
  const from = fake.calls.length;
  assert.equal(
    await browser.eval({ targetId: ordinary.targetId }, "'ordinary-probe'"),
    'page-value'
  );
  assert.ok(evaluations(from, ordinary.targetId).includes("'ordinary-probe'"));
});

test('a discovered blank tab with a protected main-frame origin is refused for eval, cookie, fetch and websocket forwarding', async () => {
  for (const blank of blanks) {
    const tab = { targetId: blank.targetId };
    const origin = blank.frame.securityOrigin;
    await assert.rejects(() => browser.eval(tab, "'blank-probe'"), { message: REFUSAL }, origin);
    await assert.rejects(() => browser.cookie(tab, 'sid'), { message: REFUSAL }, origin);
    await assert.rejects(
      () => browser.fetch(tab, 'https://example.test/data'),
      { message: REFUSAL },
      origin
    );
    await assert.rejects(
      () => browser.websocket.on(tab).forward({ sink: 'log' }),
      { message: REFUSAL },
      origin
    );
  }
  const tab = { targetId: ordinary.targetId };
  assert.equal(await browser.eval(tab, "'ordinary-probe'"), 'page-value');
  assert.equal(await browser.cookie(tab, 'sid'), null);
  assert.equal((await browser.fetch(tab, 'https://example.test/data')).status, 200);
  const forwarding = await browser.websocket.on(tab).forward({ sink: 'log' });
  await forwarding.close();
});
