import assert from 'node:assert/strict';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import browser, { REFUSAL } from '../../packages/sliccy-browser/src/browser.js';
import { isProtectedUrl } from '../../packages/sliccy-browser/src/guard.js';
import { fakeCdp } from './fake-cdp.mjs';
import './watchdog.mjs';

const scratch = await mkdtemp(path.join(tmpdir(), 'sliccy-guard-'));
after(() => rm(scratch, { recursive: true, force: true }));

function info(target) {
  return {
    targetId: target.targetId,
    type: 'page',
    url: target.url,
    title: target.title ?? target.url,
    attached: false,
    canAccessOpener: false,
    browserContextId: 'CTX',
  };
}

async function fakeSlicc(wsUrl) {
  const parsed = new URL(wsUrl);
  const port = Number(parsed.port);
  const bin = path.join(scratch, `bin-${Math.random().toString(36).slice(2)}`);
  await mkdir(bin);
  const file = path.join(bin, 'slicc');
  const body = JSON.stringify({
    schema: 1,
    minChromium: 116,
    entries: [
      {
        id: `cdp:127.0.0.1:${port}`,
        kind: 'endpoint',
        tier: 1,
        name: 'Chrome',
        endpoint: {
          cdp: `http://127.0.0.1:${port}`,
          webSocketDebuggerUrl: wsUrl,
          name: 'Chrome',
          source: 'flag',
          sevenOpen: true,
          sevenTabs: [{ id: 't', url: 'https://lars.sliccy.ai/', title: 'seven' }],
        },
      },
    ],
  });
  await writeFile(file, `#!/bin/sh\nprintf '%s' '${body}'\n`);
  await chmod(file, 0o755);
  return bin;
}

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 20));
}

const seven = { targetId: 'SEVEN', url: 'https://lars.sliccy.ai/', title: 'seven' };
const vault = { targetId: 'VAULT', url: 'https://vault.sliccy.ai/', title: 'vault' };
const extension = {
  targetId: 'EXT',
  url: 'chrome-extension://pejdijmoenmkgeppbflobdenhhabjlaj/popup.html',
  title: 'ext',
};
const errorPage = { targetId: 'ERR', url: 'chrome-error://chromewebdata/', title: 'error' };
const ok = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };

function guardedAnswers() {
  return {
    'Target.getTargets': { targetInfos: [seven, vault, extension, errorPage, ok].map(info) },
    'Target.attachToTarget': { sessionId: 'S1' },
    'Page.enable': {},
    'Runtime.enable': {},
    'Network.enable': {},
    'Network.getCookies': {
      cookies: [{ name: 'seven_session', value: 'secret', domain: '.sliccy.ai', path: '/' }],
    },
  };
}

function guardedIntercept(message, send) {
  if (message.method !== 'Runtime.evaluate') return false;
  const expression =
    typeof message.params?.expression === 'string' ? message.params.expression : '';
  const payload = expression.includes('await fetch(')
    ? {
        result: {
          type: 'object',
          value: {
            status: 200,
            statusText: 'OK',
            headers: {},
            body: 'ok',
            url: 'https://example.test/',
          },
        },
      }
    : { result: { type: 'string', value: 'leaked' } };
  send({
    id: message.id,
    ...(message.sessionId ? { sessionId: message.sessionId } : {}),
    result: payload,
  });
  return true;
}

test('isProtectedUrl covers seven, vault, extension, error pages; fail closed', () => {
  assert.equal(isProtectedUrl('https://lars.sliccy.ai/'), true);
  assert.equal(isProtectedUrl('https://vault.sliccy.ai/'), true);
  assert.equal(isProtectedUrl('http://www.sliccy.ai/x'), true);
  assert.equal(isProtectedUrl('https://sliccy.ai./'), true);
  assert.equal(isProtectedUrl('chrome-extension://abcd/page.html'), true);
  assert.equal(isProtectedUrl('chrome-error://chromewebdata/'), true);
  assert.equal(isProtectedUrl('https://example.test/'), false);
  assert.equal(isProtectedUrl('about:blank'), false);
  assert.equal(isProtectedUrl('ftp://sliccy.ai/'), false);
  assert.equal(isProtectedUrl(''), true);
  assert.equal(isProtectedUrl('not a url'), true);
  assert.equal(isProtectedUrl(null), true);
});

test('discovered endpoint refuses protected findTab/eval/cookie/localStorage/fetch', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  const bin = await fakeSlicc(fake.url);
  const previous = process.env.SLICC_CDP_URL;
  const previousPath = process.env.PATH;
  const previousHome = process.env.HOME;
  delete process.env.SLICC_CDP_URL;
  process.env.PATH = `${bin}${path.delimiter}/nonexistent`;
  process.env.HOME = path.join(scratch, 'discovered-home');
  await settle();
  try {
    await assert.rejects(() => browser.findTab({ domain: 'lars.sliccy.ai' }), {
      message: REFUSAL,
    });
    await assert.rejects(() => browser.findTab({ urlMatch: 'chrome-extension:' }), {
      message: REFUSAL,
    });
    await assert.rejects(() => browser.findTab({ urlMatch: 'chrome-error:' }), {
      message: REFUSAL,
    });
    const allowed = await browser.findTab({ domain: 'example.test' });
    assert.equal(allowed?.targetId, 'OK');
    await assert.rejects(() => browser.eval(seven, '1'), { message: REFUSAL });
    await assert.rejects(() => browser.eval('SEVEN', '1'), { message: REFUSAL });
    await assert.rejects(() => browser.evalAsync(vault, '1'), { message: REFUSAL });
    await assert.rejects(() => browser.cookie(seven, 'seven_session'), { message: REFUSAL });
    await assert.rejects(() => browser.localStorage(extension, 'k'), { message: REFUSAL });
    await assert.rejects(() => browser.fetch(ok, 'https://lars.sliccy.ai/api'), {
      message: REFUSAL,
    });
    await assert.rejects(() => browser.fetch(errorPage, 'https://example.test/'), {
      message: REFUSAL,
    });
    const fetched = await browser.fetch(ok, 'https://example.test/');
    assert.equal(fetched?.status, 200);
  } finally {
    process.env.PATH = previousPath;
    process.env.HOME = previousHome;
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('explicit SLICC_CDP_URL stays unguarded', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  try {
    const tab = await browser.findTab({ domain: 'lars.sliccy.ai' });
    assert.equal(tab?.targetId, 'SEVEN');
    assert.equal(await browser.eval(seven, '1'), 'leaked');
    assert.equal(await browser.cookie(seven, 'seven_session'), 'secret');
  } finally {
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('openClient clears guarded when connect fails after discovery', async () => {
  const bin = await fakeSlicc('ws://127.0.0.1:1/devtools/browser/dead');
  const previous = process.env.SLICC_CDP_URL;
  const previousPath = process.env.PATH;
  const previousHome = process.env.HOME;
  delete process.env.SLICC_CDP_URL;
  process.env.PATH = `${bin}${path.delimiter}/nonexistent`;
  process.env.HOME = path.join(scratch, 'fail-home');
  await settle();
  try {
    await assert.rejects(() => browser.findTab({ domain: 'example.test' }));
  } finally {
    process.env.PATH = previousPath;
    process.env.HOME = previousHome;
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});
