import assert from 'node:assert/strict';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import browser, { REFUSAL } from '../../packages/sliccy-browser/src/browser.js';
import {
  cookieProtected,
  dropProtectedCookies,
  isProtectedUrl,
} from '../../packages/sliccy-browser/src/guard.js';
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

async function withDiscovered(fake, work) {
  const bin = await fakeSlicc(fake.url);
  const previous = process.env.SLICC_CDP_URL;
  const previousPath = process.env.PATH;
  const previousHome = process.env.HOME;
  delete process.env.SLICC_CDP_URL;
  process.env.PATH = `${bin}${path.delimiter}/nonexistent`;
  process.env.HOME = path.join(scratch, `home-${Math.random().toString(36).slice(2)}`);
  await settle();
  try {
    return await work();
  } finally {
    process.env.PATH = previousPath;
    process.env.HOME = previousHome;
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
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
  assert.equal(isProtectedUrl('file:///tmp/x'), true);
  assert.equal(isProtectedUrl('chrome://settings/'), true);
  assert.equal(isProtectedUrl('devtools://devtools/bundled/'), true);
  assert.equal(isProtectedUrl('blob:https://seven.sliccy.ai/uuid'), true);
  assert.equal(isProtectedUrl('about:blank'), true);
  assert.equal(isProtectedUrl('about:srcdoc'), true);
  assert.equal(isProtectedUrl('https://example.test/'), false);
  assert.equal(isProtectedUrl(''), true);
  assert.equal(isProtectedUrl('not a url'), true);
  assert.equal(isProtectedUrl(null), true);
});

test('discovered endpoint refuses protected findTab/eval/cookie/localStorage/fetch', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
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
  });
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
    const ensured = await browser.ensureTab('https://lars.sliccy.ai/app');
    assert.equal(ensured?.url, 'https://lars.sliccy.ai/');
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

test('live urlOf refuses after handle navigates to seven; cookie value discarded', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
      'Network.getCookies': {
        cookies: [{ name: 'seven_session', value: 'secret', domain: '.sliccy.ai', path: '/' }],
      },
    },
    intercept: (message, send) => {
      if (message.method === 'Target.getTargets') {
        send({ id: message.id, result: { targetInfos: [info(page)] } });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      if (expression.includes("location.href='https://lars.sliccy.ai/'")) {
        page.url = 'https://lars.sliccy.ai/';
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'undefined' } },
        });
        return true;
      }
      const payload = expression.includes('await fetch(')
        ? {
            result: {
              type: 'object',
              value: { status: 200, statusText: 'OK', headers: {}, body: 'leak', url: page.url },
            },
          }
        : { result: { type: 'string', value: 'leaked' } };
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: payload,
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    assert.equal(tab?.targetId, 'OK');
    assert.equal(tab?.url, 'https://example.test/');
    await assert.rejects(() => browser.eval(tab, "location.href='https://lars.sliccy.ai/'"), {
      message: REFUSAL,
    });
    assert.equal(page.url, 'https://lars.sliccy.ai/');
    await assert.rejects(() => browser.eval(tab, '1'), { message: REFUSAL });
    await assert.rejects(() => browser.evalAsync(tab, '1'), { message: REFUSAL });
    await assert.rejects(() => browser.cookie(tab, 'seven_session'), { message: REFUSAL });
    assert.equal(fake.calls.filter((call) => call.method === 'Network.getCookies').length, 0);
    await assert.rejects(() => browser.localStorage(tab, 'k'), { message: REFUSAL });
    await assert.rejects(() => browser.fetch(tab, 'https://example.test/'), { message: REFUSAL });
  });
});

test('navigation during eval is refused and the result is discarded', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let evalResultSent = false;
  const fake = await fakeCdp({
    answers: {
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method === 'Target.getTargets') {
        send({ id: message.id, result: { targetInfos: [info(page)] } });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      page.url = 'https://vault.sliccy.ai/';
      evalResultSent = true;
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: { result: { type: 'string', value: 'should-not-return' } },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(() => browser.eval(tab, 'document.cookie'), { message: REFUSAL });
    assert.equal(evalResultSent, true);
    await assert.rejects(() => browser.evalAsync(tab, '1'), { message: REFUSAL });
  });
});

test('findTab skips a protected first match and never returns a protected tab', async () => {
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': {
        targetInfos: [seven, vault, ok].map(info),
      },
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const hit = await browser.findTab({ urlMatch: '.' });
    assert.equal(hit?.targetId, 'OK');
    assert.equal(isProtectedUrl(hit.url), false);
    await assert.rejects(() => browser.findTab({ urlMatch: 'sliccy\\.ai' }), {
      message: REFUSAL,
    });
  });
});

test('ensureTab refuses a protected url with no Target.createTarget', async () => {
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [ok].map(info) },
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.ensureTab('https://lars.sliccy.ai/app'), {
      message: REFUSAL,
    });
    assert.equal(fake.calls.filter((call) => call.method === 'Target.createTarget').length, 0);
  });
});

test('ensureTab that redirects to seven closes the created target and refuses', async () => {
  const fake = await fakeCdp({
    onCommand: (message) => {
      if (
        message.method === 'Page.navigate' &&
        message.params?.url === 'https://example.test/to-seven'
      ) {
        message.params.url = 'https://lars.sliccy.ai/';
      }
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.ensureTab('https://example.test/to-seven'), {
      message: REFUSAL,
    });
    const created = fake.calls.find((call) => call.method === 'Target.createTarget');
    const closed = fake.calls.find((call) => call.method === 'Target.closeTarget');
    assert.ok(created?.params);
    assert.equal(closed?.params?.targetId, 'TAB1');
  });
});

test('ensureTab match branches skip protected tabs and never return them', async () => {
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': {
        targetInfos: [seven, { targetId: 'OK', url: 'https://example.test/app', title: 'ok' }].map(
          info
        ),
      },
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const byMatch = await browser.ensureTab('https://example.test/other', {
      matchUrl: 'example\\.test|sliccy\\.ai',
    });
    assert.equal(byMatch?.targetId, 'OK');
    assert.equal(isProtectedUrl(byMatch.url), false);
    assert.equal(fake.calls.filter((call) => call.method === 'Target.createTarget').length, 0);
    const byOrigin = await browser.ensureTab('https://example.test/again');
    assert.equal(byOrigin?.targetId, 'OK');
    assert.equal(fake.calls.filter((call) => call.method === 'Target.createTarget').length, 0);
  });
});

test('findTab returns null when nothing matches', async () => {
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [ok].map(info) },
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    assert.equal(await browser.findTab({ domain: 'missing.test' }), null);
  });
});

test('eval surfaces Runtime.evaluate exceptionDetails under guard', async () => {
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [ok].map(info) },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: {
          result: { type: 'object' },
          exceptionDetails: { text: 'boom' },
        },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.eval(ok, 'throw 1'), { message: 'boom' });
  });
});

test('vendored guard matches protected.json vectors', async () => {
  const catalog = JSON.parse(
    await readFile(
      fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/catalog.json')),
      'utf8'
    )
  );
  const vectors = JSON.parse(
    await readFile(
      fileURLToPath(import.meta.resolve('@ai-ecoverse/slicc-browsers/protected.json')),
      'utf8'
    )
  );
  const { protectedOrigins } = await import('../../packages/sliccy-browser/src/protected.js');
  const { PROTECTED_ORIGINS } = await import(
    '../../packages/sliccy-browser/src/protected-origins.js'
  );
  assert.deepEqual(PROTECTED_ORIGINS, catalog.protectedOrigins);
  const reference = protectedOrigins(catalog.protectedOrigins, vectors.origins);
  const vendored = protectedOrigins(PROTECTED_ORIGINS, vectors.origins);
  for (const entry of vectors.urls) {
    assert.equal(vendored.url(entry.url), reference.url(entry.url), entry.url);
    assert.equal(vendored.url(entry.url), entry.protected, entry.url);
  }
  for (const entry of vectors.targets) {
    assert.equal(vendored.target(entry), reference.target(entry), JSON.stringify(entry));
    assert.equal(vendored.target(entry), entry.protected, JSON.stringify(entry));
  }
  for (const entry of vectors.cookieDomains) {
    assert.equal(
      vendored.cookieDomain(entry.domain),
      reference.cookieDomain(entry.domain),
      entry.domain
    );
    assert.equal(vendored.cookieDomain(entry.domain), entry.protected, entry.domain);
  }
  for (const entry of vectors.storageKeys) {
    assert.equal(vendored.storageKey(entry.key), reference.storageKey(entry.key), entry.key);
    assert.equal(vendored.storageKey(entry.key), entry.protected, entry.key);
  }
  assert.equal(
    vendored.extensionFrame('chrome-extension://abcd/page.html'),
    reference.extensionFrame('chrome-extension://abcd/page.html')
  );
  assert.equal(vendored.extensionFrame('chrome-extension://abcd/page.html'), true);
  assert.equal(vendored.extensionFrame('https://example.test/'), false);
  assert.equal(vendored.extensionFrame('not-a-url'), false);
  assert.equal(vendored.unparseable('http://[::1'), reference.unparseable('http://[::1'));
});

test('first cookie/eval/fetch call discovers and refuses a protected tab', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.cookie('SEVEN', 'canary'), { message: REFUSAL });
    await assert.rejects(() => browser.eval('SEVEN', 'location.href'), { message: REFUSAL });
    await assert.rejects(() => browser.evalAsync('VAULT', '1'), { message: REFUSAL });
    await assert.rejects(() => browser.localStorage('EXT', 'k'), { message: REFUSAL });
    await assert.rejects(() => browser.fetch('SEVEN', 'https://example.test/'), {
      message: REFUSAL,
    });
  });
});

test('after idle close, reopened discovery still refuses a protected tab', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    assert.equal((await browser.findTab({ domain: 'example.test' }))?.targetId, 'OK');
    await settle();
    await new Promise((resolve) => setTimeout(resolve, 50));
    await assert.rejects(() => browser.cookie('SEVEN', 'canary'), { message: REFUSAL });
    await assert.rejects(() => browser.eval('SEVEN', 'location.href'), { message: REFUSAL });
  });
});

test('first call with SLICC_CDP_URL stays unguarded for a protected tab', async () => {
  const fake = await fakeCdp({ answers: guardedAnswers(), intercept: guardedIntercept });
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  try {
    assert.equal(await browser.cookie('SEVEN', 'seven_session'), 'secret');
    assert.equal(await browser.eval('SEVEN', '1'), 'leaked');
  } finally {
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('missing and non-page targets are refused in discovered mode', async () => {
  const worker = {
    targetId: 'SW',
    type: 'service_worker',
    url: 'https://example.test/sw.js',
    title: 'sw',
  };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': {
        targetInfos: [info(ok), { ...info(worker), type: 'service_worker' }],
      },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
      'Network.getCookies': { cookies: [{ name: 'x', value: 'leak' }] },
    },
    intercept: guardedIntercept,
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.cookie('GONE', 'x'), { message: REFUSAL });
    await assert.rejects(() => browser.eval('SW', '1'), { message: REFUSAL });
    await assert.rejects(() => browser.cookie('SW', 'x'), { message: REFUSAL });
  });
});

test('discovered mode refuses file, chrome, devtools, blob-seven, about:blank via findTab and ops', async () => {
  const fileTab = { targetId: 'FILE', url: 'file:///tmp/secret.txt', title: 'file' };
  const settings = { targetId: 'SETTINGS', url: 'chrome://settings/', title: 'settings' };
  const devtools = {
    targetId: 'DEVTOOLS',
    url: 'devtools://devtools/bundled/inspector.html',
    title: 'devtools',
  };
  const blobSeven = {
    targetId: 'BLOB',
    url: 'blob:https://seven.sliccy.ai/11111111-1111-1111-1111-111111111111',
    title: 'blob',
  };
  const blank = { targetId: 'BLANK', url: 'about:blank', title: 'blank' };
  const pages = [fileTab, settings, devtools, blobSeven, blank, ok];
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: pages.map(info) },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
      'Network.getCookies': { cookies: [] },
    },
    intercept: guardedIntercept,
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.findTab({ urlMatch: '^file:' }), { message: REFUSAL });
    await assert.rejects(() => browser.findTab({ domain: 'settings' }), { message: REFUSAL });
    await assert.rejects(() => browser.findTab({ urlMatch: '^devtools:' }), { message: REFUSAL });
    await assert.rejects(() => browser.findTab({ urlMatch: '^blob:' }), { message: REFUSAL });
    await assert.rejects(() => browser.findTab({ urlMatch: '^about:blank' }), {
      message: REFUSAL,
    });
    for (const tab of [fileTab, settings, devtools, blobSeven, blank]) {
      await assert.rejects(() => browser.eval(tab, '1'), { message: REFUSAL });
      await assert.rejects(() => browser.cookie(tab, 'x'), { message: REFUSAL });
      await assert.rejects(() => browser.localStorage(tab, 'k'), { message: REFUSAL });
      await assert.rejects(() => browser.fetch(tab, 'https://example.test/'), {
        message: REFUSAL,
      });
    }
    await assert.rejects(() => browser.ensureTab('file:///tmp/x'), { message: REFUSAL });
    await assert.rejects(() => browser.ensureTab('chrome://settings/'), { message: REFUSAL });
    await assert.rejects(() => browser.ensureTab('about:blank'), { message: REFUSAL });
    assert.equal((await browser.findTab({ domain: 'example.test' }))?.targetId, 'OK');
  });
});

test('cookieProtected drops domain, url, and partitioned seven scopes', () => {
  assert.equal(cookieProtected(null), true);
  assert.equal(cookieProtected({ name: 'a', domain: '.sliccy.ai' }), true);
  assert.equal(cookieProtected({ name: 'a', domain: 'seven.sliccy.ai' }), true);
  assert.equal(cookieProtected({ name: 'a', domain: '.ai' }), true);
  assert.equal(cookieProtected({ name: 'a', domain: 'example.test' }), false);
  assert.equal(cookieProtected({ name: 'a', url: 'https://seven.sliccy.ai/' }), true);
  assert.equal(cookieProtected({ name: 'a', url: 'ftp://seven.sliccy.ai/' }), true);
  assert.equal(cookieProtected({ name: 'a', url: 'ftp://example.test./' }), false);
  assert.equal(isProtectedUrl('wss://seven.sliccy.ai/socket'), true);
  assert.equal(isProtectedUrl('ws://seven.sliccy.ai/socket'), true);
  assert.equal(
    cookieProtected({
      name: 'a',
      domain: 'cdn.example.test',
      partitionKey: { topLevelSite: 'https://seven.sliccy.ai' },
    }),
    true
  );
  assert.equal(
    cookieProtected({
      name: 'a',
      domain: 'cdn.example.test',
      partitionKey: 'https://seven.sliccy.ai',
    }),
    true
  );
  assert.equal(
    cookieProtected({
      name: 'a',
      domain: 'cdn.example.test',
      partitionKey: { topLevelSite: 'ftp://seven.sliccy.ai/' },
    }),
    true
  );
  assert.deepEqual(
    dropProtectedCookies([
      { name: 'ok', value: '1', domain: 'example.test' },
      { name: 'bad', value: '2', domain: '.sliccy.ai' },
    ]).map((c) => c.name),
    ['ok']
  );
  assert.deepEqual(dropProtectedCookies(null), []);
});

const iframeJar = [
  { name: 'ok', value: 'allowed', domain: 'example.test', path: '/' },
  { name: 'seven_session', value: 'secret', domain: '.sliccy.ai', path: '/' },
  { name: 'host_only', value: 'secret2', domain: 'seven.sliccy.ai', path: '/' },
  { name: 'parent', value: 'secret3', domain: '.ai', path: '/' },
  {
    name: 'part',
    value: 'secret4',
    domain: 'cdn.example.test',
    path: '/',
    partitionKey: { topLevelSite: 'https://seven.sliccy.ai' },
  },
];

async function cookieIframeFake() {
  const page = {
    targetId: 'OK',
    url: 'https://example.test/',
    title: 'ok',
    frames: [{ url: 'https://seven.sliccy.ai/embed' }],
  };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method === 'Network.getCookies') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { cookies: iframeJar.map((cookie) => ({ ...cookie })) },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  return { fake, page };
}

test('cookie with seven iframe never returns protected cookies; getCookies uses urls', async () => {
  const { fake, page } = await cookieIframeFake();
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    assert.equal(tab?.targetId, 'OK');
    assert.equal(await browser.cookie(tab, 'ok'), 'allowed');
    assert.equal(await browser.cookie(tab, 'seven_session'), null);
    assert.equal(await browser.cookie(tab, 'host_only'), null);
    assert.equal(await browser.cookie(tab, 'parent'), null);
    assert.equal(await browser.cookie(tab, 'part'), null);
    const gets = fake.calls.filter((call) => call.method === 'Network.getCookies');
    assert.ok(gets.length > 0);
    for (const call of gets) {
      assert.deepEqual(call.params?.urls, [page.url]);
    }
  });
});

test('cookie after eval-created seven iframe still drops protected cookies', async () => {
  const { fake, page } = await cookieIframeFake();
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await browser.eval(
      tab,
      "document.body.innerHTML='<iframe src=https://seven.sliccy.ai/x></iframe>'"
    );
    assert.equal(page.url, 'https://example.test/');
    assert.equal(await browser.cookie(tab, 'seven_session'), null);
    assert.equal(await browser.cookie(tab, 'ok'), 'allowed');
  });
});

test('explicit SLICC_CDP_URL cookie still sends bare getCookies and returns seven cookies', async () => {
  const { fake } = await cookieIframeFake();
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  try {
    assert.equal(await browser.cookie(ok, 'seven_session'), 'secret');
    const gets = fake.calls.filter((call) => call.method === 'Network.getCookies');
    assert.ok(gets.length > 0);
    for (const call of gets) {
      assert.equal(call.params?.urls, undefined);
    }
  } finally {
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('websocket refuses a protected tab and drops protected socket frames', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page), info(seven)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (
        message.method === 'Runtime.evaluate' &&
        message.params?.expression === '"emit-seven-ws"'
      ) {
        const sessionId = message.sessionId;
        send({
          method: 'Network.webSocketCreated',
          params: { requestId: 'R1', url: 'wss://seven.sliccy.ai/ws' },
          ...(sessionId ? { sessionId } : {}),
        });
        send({
          method: 'Network.webSocketFrameReceived',
          params: {
            requestId: 'R1',
            response: { opcode: 1, payloadData: '{"kind":"tick","n":9}' },
          },
          ...(sessionId ? { sessionId } : {}),
        });
        send({
          id: message.id,
          ...(sessionId ? { sessionId } : {}),
          result: { result: { type: 'string', value: 'ok' } },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.websocket.on('SEVEN').forward({ sink: 'log' }), {
      message: REFUSAL,
    });
    const tab = await browser.findTab({ domain: 'example.test' });
    const logged = [];
    const write = console.log;
    console.log = (...args) => {
      logged.push(args.map(String).join(' '));
    };
    let handle;
    try {
      handle = await browser.websocket.on(tab).filter({ parseAs: 'json' }).forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"emit-seven-ws"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":9') || line.includes('"n": 9')),
        false
      );
    } finally {
      console.log = write;
      await handle?.close();
    }
  });
});

test('Slack-shaped relative fetch works on discovered app.slack.com; seven relatives refuse', async () => {
  const slack = {
    targetId: 'SLACK',
    url: 'https://app.slack.com/client/T1/C1',
    title: 'Slack',
  };
  const seenFetch = [];
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(slack), info(seven)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
      'Network.getCookies': {
        cookies: [
          { name: 'd', value: 'slack-d', domain: '.slack.com', path: '/' },
          { name: 'seven_session', value: 'secret', domain: '.sliccy.ai', path: '/' },
        ],
      },
    },
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      if (expression.includes('await fetch(')) {
        const match = expression.match(/await fetch\(([^,]+),/u);
        if (match) seenFetch.push(match[1]);
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            result: {
              type: 'object',
              value: {
                status: 200,
                statusText: 'OK',
                headers: {},
                body: 'ok',
                url: 'https://app.slack.com/api/auth.test',
              },
            },
          },
        });
        return true;
      }
      if (expression.includes('localStorage.getItem')) {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'string', value: '{"ok":true}' } },
        });
        return true;
      }
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: { result: { type: 'string', value: 'async-ok' } },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const found = await browser.findTab({
      domain: 'app.slack.com',
      urlMatch: /\/client\/[A-Z0-9]+/u,
    });
    assert.equal(found?.targetId, 'SLACK');
    const relative = await browser.fetch(found, '/api/auth.test', { method: 'POST' });
    assert.equal(relative?.status, 200);
    assert.ok(seenFetch.some((arg) => arg.includes('/api/auth.test')));
    assert.ok(!seenFetch.some((arg) => arg.includes('https://app.slack.com/api/auth.test')));
    const absolute = await browser.fetch(found, 'https://edgeapi.slack.com/cache/T1/users/info');
    assert.equal(absolute?.status, 200);
    assert.equal(await browser.cookie(found, 'd'), 'slack-d');
    assert.equal(await browser.cookie(found, 'seven_session'), null);
    assert.equal(await browser.localStorage(found, 'localConfig_v2'), '{"ok":true}');
    assert.equal(await browser.evalAsync(found, '1'), 'async-ok');
    await assert.rejects(() => browser.fetch(found, '//lars.sliccy.ai/x'), {
      message: REFUSAL,
    });
    await assert.rejects(() => browser.fetch(seven, '/api/x'), { message: REFUSAL });
    await assert.rejects(() => browser.fetch(found, 'http://[::1'), /could not resolve/);
    const handle = await browser.websocket
      .on(found, { urlMatch: /slack\.com/u })
      .forward({ sink: 'log' });
    assert.equal(typeof handle.close, 'function');
    await handle.close();
    await assert.rejects(() => browser.websocket.on(seven).forward({ sink: 'log' }), {
      message: REFUSAL,
    });
  });
});
