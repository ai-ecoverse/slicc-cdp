import assert from 'node:assert/strict';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import browser, { REFUSAL } from '../../packages/sliccy-browser/src/browser.js';
import {
  cookieProtected,
  credentialCookie,
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
  let payload;
  if (expression.includes('await fetch(')) {
    payload = {
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
    };
  } else if (expression.trim() === 'document.baseURI') {
    payload = { result: { type: 'string', value: 'https://example.test/' } };
  } else {
    payload = { result: { type: 'string', value: 'leaked' } };
  }
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

test('credential-origin cookies are withheld by cookieProtected and cookie() on an IdP tab', async () => {
  for (const cookie of [
    { domain: 'ims-na1.adobelogin.com' },
    { domain: '.adobelogin.com' },
    { domain: 'IMS-NA1-STG1.ADOBELOGIN.COM.' },
    { url: 'https://ims-na1.adobelogin.com:8443/x' },
    { url: 'https://auth.adobelogin.com/' },
    { domain: 'widget.example.test', partitionKey: { topLevelSite: 'https://adobelogin.com' } },
  ]) {
    assert.equal(credentialCookie(cookie), true, JSON.stringify(cookie));
    assert.equal(cookieProtected(cookie), true, JSON.stringify(cookie));
  }
  for (const cookie of [
    { domain: 'x.ims-na1.adobelogin.com' },
    { domain: 'evil-adobelogin.com' },
    { domain: 'adobelogin.com.evil.test' },
    { domain: '.adobe.com' },
    { domain: 'auth.services.adobe.com' },
    { url: 'https://www.example.com/' },
    { domain: 'widget.example.test', partitionKey: { topLevelSite: 'https://adobe.com' } },
  ]) {
    assert.equal(credentialCookie(cookie), false, JSON.stringify(cookie));
    assert.equal(cookieProtected(cookie), false, JSON.stringify(cookie));
  }
  const idp = {
    targetId: 'IDP',
    url: 'https://ims-na1.adobelogin.com/ims/authorize/v2',
    title: 'Sign in',
  };
  const fake = await fakeCdp({
    answers: {
      ...guardedAnswers(),
      'Target.getTargets': { targetInfos: [idp, ok].map(info) },
      'Network.getCookies': {
        cookies: [
          { name: 'host', value: 'idp-secret', domain: 'ims-na1.adobelogin.com', path: '/' },
          { name: 'parent', value: 'idp-parent', domain: '.adobelogin.com', path: '/' },
          { name: 'adobe', value: 'adobe-kept', domain: '.adobe.com', path: '/' },
        ],
      },
    },
    intercept: guardedIntercept,
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'ims-na1.adobelogin.com' });
    assert.equal(tab?.targetId, 'IDP');
    assert.equal(await browser.eval(tab, '1'), 'leaked');
    assert.equal(await browser.cookie(tab, 'host'), null);
    assert.equal(await browser.cookie(tab, 'parent'), null);
    assert.equal(await browser.cookie(tab, 'adobe'), 'adobe-kept');
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

test('a guarded subscription drops frames of a socket the browser did not announce', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let productEvaluates = 0;
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"unannounced-socket"') {
        send({
          method: 'Network.webSocketFrameSent',
          params: {
            requestId: 'R9',
            response: { opcode: 1, payloadData: 'ping' },
          },
          ...(sessionId ? { sessionId } : {}),
        });
        send({
          method: 'Network.webSocketFrameReceived',
          params: {
            requestId: 'R9',
            response: { opcode: 1, payloadData: '{"kind":"tick","n":3}' },
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
      productEvaluates += 1;
      send({
        id: message.id,
        ...(sessionId ? { sessionId } : {}),
        result: {
          result:
            productEvaluates === 1
              ? { type: 'boolean', value: true }
              : { type: 'string', value: 'wss://seven.sliccy.ai/ws' },
        },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const logged = [];
    const write = console.log;
    console.log = (...args) => {
      logged.push(args.map(String).join(' '));
    };
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /example\.test|sliccy\.ai/u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"unannounced-socket"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":3')),
        false
      );
    } finally {
      console.log = write;
      await handle?.close();
    }
  });
});

test('discovered websocket drops frames when socket URL is unknown', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (
        message.method === 'Runtime.evaluate' &&
        message.params?.expression === '"emit-unknown-ws"'
      ) {
        const sessionId = message.sessionId;
        send({
          method: 'Network.webSocketFrameReceived',
          params: {
            requestId: 'PREEXISTING',
            response: { opcode: 1, payloadData: '{"kind":"canary","n":7}' },
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
    const tab = await browser.findTab({ domain: 'example.test' });
    const logged = [];
    const write = console.log;
    console.log = (...args) => {
      logged.push(args.map(String).join(' '));
    };
    let bare;
    let matched;
    try {
      bare = await browser.websocket.on(tab).filter({ parseAs: 'json' }).forward({ sink: 'log' });
      matched = await browser.websocket
        .on(tab, { urlMatch: /sliccy\.ai/u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"emit-unknown-ws"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('canary')),
        false
      );
    } finally {
      console.log = write;
      await bare?.close();
      await matched?.close();
    }
  });
});

test('discovered websocket warns once and counts drops for unknown-URL sockets', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (
        message.method === 'Runtime.evaluate' &&
        message.params?.expression === '"emit-unknown-twice"'
      ) {
        const sessionId = message.sessionId;
        for (const requestId of ['PRE1', 'PRE2']) {
          send({
            method: 'Network.webSocketFrameReceived',
            params: {
              requestId,
              response: {
                opcode: 1,
                payloadData: '{"kind":"preattach","secret":"do-not-leak"}',
              },
            },
            ...(sessionId ? { sessionId } : {}),
          });
        }
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
    const tab = await browser.findTab({ domain: 'example.test' });
    const warnings = [];
    const warn = console.warn;
    console.warn = (...args) => {
      warnings.push(args.map(String).join(' '));
    };
    let handle;
    try {
      handle = await browser.websocket.on(tab).filter({ parseAs: 'json' }).forward({ sink: 'log' });
      assert.equal(handle.dropped, 0);
      assert.equal(await browser.eval(tab, '"emit-unknown-twice"'), 'ok');
      await settle();
      assert.equal(handle.dropped, 2);
      assert.equal(warnings.length, 1);
      assert.match(warnings[0], /before the first attach/);
      assert.match(warnings[0], /reload the tab|reconnect the socket/);
      assert.equal(warnings[0].includes('preattach'), false);
      assert.equal(warnings[0].includes('do-not-leak'), false);
      assert.equal(warnings[0].includes('PRE1'), false);
      assert.equal(String(handle.dropped).includes('preattach'), false);
    } finally {
      console.warn = warn;
      await handle?.close();
    }
  });
});

test('discovered websocket does not warn when nothing was dropped', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: guardedIntercept,
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const warnings = [];
    const warn = console.warn;
    console.warn = (...args) => {
      warnings.push(args.map(String).join(' '));
    };
    let handle;
    try {
      handle = await browser.websocket.on(tab).forward({ sink: 'log' });
      await settle();
      assert.equal(handle.dropped, 0);
      assert.deepEqual(warnings, []);
    } finally {
      console.warn = warn;
      await handle?.close();
    }
  });
});

test('explicit SLICC_CDP_URL websocket does not warn on unknown-URL frames', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (
        message.method === 'Runtime.evaluate' &&
        message.params?.expression === '"emit-unknown-explicit"'
      ) {
        const sessionId = message.sessionId;
        send({
          method: 'Network.webSocketFrameReceived',
          params: {
            requestId: 'PREEXISTING',
            response: { opcode: 1, payloadData: '{"kind":"explicit-canary","n":1}' },
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
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  const warnings = [];
  const warn = console.warn;
  console.warn = (...args) => {
    warnings.push(args.map(String).join(' '));
  };
  let handle;
  try {
    handle = await browser.websocket.on(page).filter({ parseAs: 'json' }).forward({ sink: 'log' });
    assert.equal(await browser.eval(page, '"emit-unknown-explicit"'), 'ok');
    await settle();
    assert.equal(handle.dropped, 0);
    assert.deepEqual(warnings, []);
  } finally {
    console.warn = warn;
    await handle?.close();
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
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
      if (expression.trim() === 'document.baseURI') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'string', value: slack.url } },
        });
        return true;
      }
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
    assert.ok(
      seenFetch.some((arg) => arg.includes(JSON.stringify('https://app.slack.com/api/auth.test')))
    );
    assert.ok(!seenFetch.some((arg) => arg.includes(JSON.stringify('/api/auth.test'))));
    const absolute = await browser.fetch(found, 'https://edgeapi.slack.com/cache/T1/users/info');
    assert.equal(absolute?.status, 200);
    assert.ok(
      seenFetch.some((arg) =>
        arg.includes(JSON.stringify('https://edgeapi.slack.com/cache/T1/users/info'))
      )
    );
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

test('discovered fetch surfaces baseURI and page-fetch evaluate exceptions', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let phase = 'base';
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      if (expression.trim() === 'document.baseURI') {
        if (phase === 'base') {
          send({
            id: message.id,
            ...(message.sessionId ? { sessionId: message.sessionId } : {}),
            result: {
              result: { type: 'object', value: null },
              exceptionDetails: {
                text: 'baseURI blew up',
                exception: { description: 'baseURI blew up' },
              },
            },
          });
          return true;
        }
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'string', value: page.url } },
        });
        return true;
      }
      if (expression.includes('await fetch(')) {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            result: { type: 'object', value: null },
            exceptionDetails: {
              text: 'page fetch blew up',
              exception: { description: 'page fetch blew up' },
            },
          },
        });
        return true;
      }
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: { result: { type: 'string', value: 'unexpected' } },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.fetch(page, '/probe'), /baseURI blew up/);
    phase = 'fetch';
    await assert.rejects(() => browser.fetch(page, '/probe'), /page fetch blew up/);
  });
});

test('discovered fetch refuses relative when document.baseURI is protected; no page fetch', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/a', title: 'ok' };
  const seenFetch = [];
  let baseReads = 0;
  const fake = await fakeCdp({
    answers: {
      'Target.getTargets': { targetInfos: [info(page)] },
      'Target.attachToTarget': { sessionId: 'S1' },
      'Page.enable': {},
      'Runtime.enable': {},
      'Network.enable': {},
    },
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      if (expression.trim() === 'document.baseURI') {
        baseReads += 1;
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'string', value: 'https://lars.sliccy.ai/' } },
        });
        return true;
      }
      if (expression.includes('await fetch(')) {
        seenFetch.push(expression);
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
                body: 'leak',
                url: 'https://lars.sliccy.ai/probe',
              },
            },
          },
        });
        return true;
      }
      send({
        id: message.id,
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
        result: { result: { type: 'string', value: 'unexpected' } },
      });
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    await assert.rejects(() => browser.fetch(page, '/probe'), { message: REFUSAL });
    assert.equal(baseReads, 1);
    assert.deepEqual(seenFetch, []);
  });
});

test('explicit SLICC_CDP_URL fetch passes the original string and skips baseURI', async () => {
  const seenFetch = [];
  let baseReads = 0;
  const fake = await fakeCdp({
    answers: guardedAnswers(),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      if (expression.trim() === 'document.baseURI') {
        baseReads += 1;
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { result: { type: 'string', value: 'https://example.test/' } },
        });
        return true;
      }
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
                url: 'https://example.test/plain',
              },
            },
          },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  try {
    const result = await browser.fetch(ok, '/plain');
    assert.equal(result?.status, 200);
    assert.equal(baseReads, 0);
    assert.ok(seenFetch.some((arg) => arg.includes(JSON.stringify('/plain'))));
    assert.ok(!seenFetch.some((arg) => arg.includes('https://example.test/plain')));
  } finally {
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

const ALLOWED_WS = 'ws://127.0.0.1:9/ws';
const PROTECTED_WS = 'wss://seven.sliccy.ai/ws';

function captureLog() {
  const logged = [];
  const write = console.log;
  console.log = (...args) => {
    logged.push(args.map(String).join(' '));
  };
  return {
    logged,
    restore() {
      console.log = write;
    },
  };
}

function wsAnswers(...pages) {
  const list = pages.flat();
  return {
    'Target.getTargets': { targetInfos: list.map(info) },
    'Target.attachToTarget': { sessionId: 'S1' },
    'Page.enable': {},
    'Runtime.enable': {},
    'Network.enable': {},
  };
}

function replyEvaluate(message, send, value) {
  send({
    id: message.id,
    ...(message.sessionId ? { sessionId: message.sessionId } : {}),
    result: { result: { type: typeof value === 'boolean' ? 'boolean' : 'string', value } },
  });
}

function emitFrame(send, sessionId, requestId, payloadData, kind) {
  send({
    method: kind,
    params: {
      requestId,
      response: { opcode: 1, payloadData },
    },
    ...(sessionId ? { sessionId } : {}),
  });
}

function emitMainFrame(send, sessionId, targetId, url) {
  let origin = '';
  try {
    origin = new URL(url).origin;
  } catch {
    origin = '';
  }
  send({
    method: 'Page.frameNavigated',
    params: {
      frame: {
        id: targetId,
        url,
        loaderId: `L-${targetId}`,
        securityOrigin: origin,
      },
    },
    ...(sessionId ? { sessionId } : {}),
  });
}

function emitChildFrame(send, sessionId, frameId, parentId, url) {
  let origin = '';
  try {
    origin = new URL(url).origin;
  } catch {
    origin = '';
  }
  send({
    method: 'Page.frameNavigated',
    params: {
      frame: {
        id: frameId,
        parentId,
        url,
        loaderId: `L-${frameId}`,
        securityOrigin: origin,
      },
    },
    ...(sessionId ? { sessionId } : {}),
  });
}

function emitFrameAttached(send, sessionId, frameId, parentId) {
  send({
    method: 'Page.frameAttached',
    params: { frameId, parentFrameId: parentId },
    ...(sessionId ? { sessionId } : {}),
  });
}

function emitFrameDetached(send, sessionId, frameId, reason) {
  const params = { frameId };
  if (reason !== undefined) params.reason = reason;
  send({
    method: 'Page.frameDetached',
    params,
    ...(sessionId ? { sessionId } : {}),
  });
}

function announceSocketCreated(send, sessionId, requestId, url, initiator) {
  const params = { requestId, url };
  if (initiator !== undefined) params.initiator = initiator;
  send({
    method: 'Network.webSocketCreated',
    params,
    ...(sessionId ? { sessionId } : {}),
  });
}

function emitTargetInfo(send, method, targetId, url) {
  send({
    method,
    params: {
      targetInfo: {
        targetId,
        type: 'page',
        url,
        title: url,
      },
    },
  });
}

function announceSocket(send, sessionId, requestId, url, frame) {
  send({
    method: 'Network.webSocketCreated',
    params: { requestId, url },
    ...(sessionId ? { sessionId } : {}),
  });
  emitFrame(send, sessionId, requestId, frame, 'Network.webSocketFrameReceived');
}

function answerProductEvaluate(message, send, evaluations, valueForLater, delayLaterMs = 0) {
  evaluations.push(1);
  const value = evaluations.length === 1 ? true : valueForLater;
  if (evaluations.length > 1 && delayLaterMs > 0) {
    setTimeout(() => replyEvaluate(message, send, value), delayLaterMs);
    return;
  }
  replyEvaluate(message, send, value);
}

test('a socket opened before the first attach is not routed in guarded mode', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const orderings = ['early', 'middle', 'late'];
  for (const ordering of orderings) {
    const evaluations = [];
    const fake = await fakeCdp({
      answers: wsAnswers(page),
      intercept: (message, send) => {
        if (message.method !== 'Runtime.evaluate') return false;
        const expression =
          typeof message.params?.expression === 'string' ? message.params.expression : '';
        const sessionId = message.sessionId;
        if (expression === '"preattach-ping"') {
          if (ordering === 'early') {
            emitFrame(
              send,
              sessionId,
              'R1',
              '{"kind":"tick","n":1}',
              'Network.webSocketFrameReceived'
            );
            emitFrame(send, sessionId, 'R1', 'ping', 'Network.webSocketFrameSent');
          } else if (ordering === 'middle') {
            emitFrame(send, sessionId, 'R1', 'ping', 'Network.webSocketFrameSent');
            emitFrame(
              send,
              sessionId,
              'R1',
              '{"kind":"tick","n":1}',
              'Network.webSocketFrameReceived'
            );
          } else {
            emitFrame(send, sessionId, 'R1', 'ping', 'Network.webSocketFrameSent');
            setTimeout(() => {
              emitFrame(
                send,
                sessionId,
                'R1',
                '{"kind":"tick","n":1}',
                'Network.webSocketFrameReceived'
              );
            }, 40);
          }
          replyEvaluate(message, send, 'ok');
          return true;
        }
        answerProductEvaluate(
          message,
          send,
          evaluations,
          ALLOWED_WS,
          ordering === 'middle' ? 40 : 0
        );
        return true;
      },
    });
    after(() => fake.close());
    await withDiscovered(fake, async () => {
      const tab = await browser.findTab({ domain: 'example.test' });
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /127\.0\.0\.1/u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, '"preattach-ping"'), 'ok');
        await new Promise((resolve) => setTimeout(resolve, 80));
        assert.equal(
          logged.some((line) => line.includes('"n":1') || line.includes('tick')),
          false,
          ordering
        );
        assert.ok(handle.dropped > 0, ordering);
        assert.equal(evaluations.length, 0, ordering);
      } finally {
        restore();
        await handle?.close();
      }
    });
  }
});

test('sockets the browser has not announced are not routed in guarded mode', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  for (const first of ['allowed', 'protected']) {
    const second = first === 'allowed' ? 'protected' : 'allowed';
    const order = [first, second];
    const evaluations = [];
    const fake = await fakeCdp({
      answers: wsAnswers(page),
      intercept: (message, send) => {
        if (message.method !== 'Runtime.evaluate') return false;
        const expression =
          typeof message.params?.expression === 'string' ? message.params.expression : '';
        const sessionId = message.sessionId;
        if (expression === '"two-sockets"') {
          for (const which of order) {
            const requestId = which === 'allowed' ? 'A1' : 'P1';
            emitFrame(send, sessionId, requestId, 'body-1', 'Network.webSocketFrameSent');
          }
          setTimeout(() => {
            for (const which of order) {
              const requestId = which === 'allowed' ? 'A1' : 'P1';
              emitFrame(
                send,
                sessionId,
                requestId,
                `{"kind":"tick","from":"${which}"}`,
                'Network.webSocketFrameReceived'
              );
            }
          }, 40);
          replyEvaluate(message, send, 'ok');
          return true;
        }
        answerProductEvaluate(message, send, evaluations, ALLOWED_WS);
        return true;
      },
    });
    after(() => fake.close());
    await withDiscovered(fake, async () => {
      const tab = await browser.findTab({ domain: 'example.test' });
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, '"two-sockets"'), 'ok');
        await new Promise((resolve) => setTimeout(resolve, 80));
        assert.equal(
          logged.some(
            (line) => line.includes('"from":"protected"') || line.includes('from":"protected')
          ),
          false,
          first
        );
        assert.equal(
          logged.some(
            (line) => line.includes('"from":"allowed"') || line.includes('from":"allowed')
          ),
          false,
          first
        );
        assert.ok(handle.dropped > 0, first);
      } finally {
        restore();
        await handle?.close();
      }
    });
  }
});

test("the url of a socket comes from the browser's event", async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const evaluations = [];
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"preattach-r3"') {
        emitFrame(send, sessionId, 'R3', 'ping', 'Network.webSocketFrameSent');
        setTimeout(() => {
          emitFrame(
            send,
            sessionId,
            'R3',
            '{"kind":"tick","n":11}',
            'Network.webSocketFrameReceived'
          );
        }, 40);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      answerProductEvaluate(message, send, evaluations, ALLOWED_WS);
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /127\.0\.0\.1/u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"preattach-r3"'), 'ok');
      await new Promise((resolve) => setTimeout(resolve, 80));
      assert.equal(
        logged.some((line) => line.includes('"n":11')),
        false
      );
      assert.ok(handle.dropped > 0);
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a guarded urlMatch subscription routes only sockets the browser announced', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const evaluations = [];
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"preattach-r4"') {
        emitFrame(send, sessionId, 'R4', 'ping', 'Network.webSocketFrameSent');
        setTimeout(() => {
          emitFrame(
            send,
            sessionId,
            'R4',
            '{"kind":"tick","n":12}',
            'Network.webSocketFrameReceived'
          );
        }, 40);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      answerProductEvaluate(message, send, evaluations, ALLOWED_WS);
      return true;
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /127\.0\.0\.1/u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(evaluations.length, 0);
      assert.equal(await browser.eval(tab, '"preattach-r4"'), 'ok');
      await new Promise((resolve) => setTimeout(resolve, 80));
      assert.equal(
        logged.some((line) => line.includes('"n":12')),
        false
      );
      assert.ok(handle.dropped > 0);
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a socket announced after attach routes by the url the browser announced', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"announce-allowed"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'R5', ALLOWED_WS, '{"kind":"tick","n":21}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"announce-protected"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'R6', PROTECTED_WS, '{"kind":"tick","n":22}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"announce-allowed"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":21')),
        true
      );
      assert.equal(handle.dropped, 0);
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"announce-protected"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":22')),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an explicit endpoint routes sockets as before', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const evaluations = [];
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"explicit-next"') {
        emitFrame(send, sessionId, 'R7', 'ping', 'Network.webSocketFrameSent');
        emitFrame(
          send,
          sessionId,
          'R7',
          '{"kind":"tick","n":31}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      answerProductEvaluate(message, send, evaluations, ALLOWED_WS);
      return true;
    },
  });
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  const { logged, restore } = captureLog();
  let handle;
  try {
    handle = await browser.websocket
      .on(page, { urlMatch: /127\.0\.0\.1/u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    assert.equal(await browser.eval(page, '"explicit-next"'), 'ok');
    await settle();
    assert.equal(
      logged.some((line) => line.includes('"n":31')),
      true
    );
  } finally {
    restore();
    await handle?.close();
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('subscription frames follow the tab document across navigations', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const docB = 'https://lars.sliccy.ai/';
  const docABack = 'https://example.test/back';
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R1"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'N1', ALLOWED_WS, '{"kind":"tick","n":41}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R2"') {
        emitMainFrame(send, sessionId, page.targetId, docB);
        announceSocket(send, sessionId, 'N2', ALLOWED_WS, '{"kind":"tick","n":42}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R3"') {
        emitMainFrame(send, sessionId, page.targetId, docABack);
        announceSocket(send, sessionId, 'N3', ALLOWED_WS, '{"kind":"tick","n":43}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R1"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":41')),
        true
      );
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"R2"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":42')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"R3"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":43')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an open socket stops delivering after the tab document changes', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const docB = 'https://lars.sliccy.ai/';
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R4"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        send({
          method: 'Network.webSocketCreated',
          params: { requestId: 'OPEN1', url: ALLOWED_WS },
          ...(sessionId ? { sessionId } : {}),
        });
        emitFrame(
          send,
          sessionId,
          'OPEN1',
          '{"kind":"tick","n":51}',
          'Network.webSocketFrameReceived'
        );
        emitMainFrame(send, sessionId, page.targetId, docB);
        emitFrame(
          send,
          sessionId,
          'OPEN1',
          '{"kind":"tick","n":52}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R4"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":51')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":52')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('unknown tab document class does not route; ordinary class does', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let attached = false;
  let seedEmpty = true;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attached = true;
        send({ id: message.id, result: { sessionId: 'S1' } });
        return true;
      }
      if (message.method === 'Target.getTargets') {
        if (attached && seedEmpty) {
          seedEmpty = false;
          send({
            id: message.id,
            result: {
              targetInfos: [
                {
                  targetId: page.targetId,
                  type: 'page',
                  url: '',
                  title: '',
                  attached: false,
                  canAccessOpener: false,
                  browserContextId: 'CTX',
                },
              ],
            },
          });
          return true;
        }
        send({
          id: message.id,
          result: { targetInfos: [info(page)] },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R5"') {
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, page.url);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R6"') {
        announceSocket(send, sessionId, 'OCLASS', ALLOWED_WS, '{"kind":"tick","n":62}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
    const { logged, restore } = captureLog();
    let handle;
    try {
      assert.equal(await browser.eval(tab, '"R5"'), 'ok');
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R6"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":62')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('ordinary navigations keep an ordinary subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const next = 'https://example.test/next';
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R22"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'O1', ALLOWED_WS, '{"kind":"tick","n":71}');
        emitMainFrame(send, sessionId, page.targetId, next);
        announceSocket(send, sessionId, 'O2', ALLOWED_WS, '{"kind":"tick","n":72}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R22"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":71')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":72')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('targetInfoChanged and targetCreated update the tab document class', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const docB = 'https://lars.sliccy.ai/';
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R7"') {
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, page.url);
        announceSocket(send, sessionId, 'TIC1', ALLOWED_WS, '{"kind":"tick","n":81}');
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, docB);
        announceSocket(send, sessionId, 'TIC2', ALLOWED_WS, '{"kind":"tick","n":82}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R8"') {
        emitTargetInfo(send, 'Target.targetCreated', page.targetId, page.url);
        announceSocket(send, sessionId, 'TC1', ALLOWED_WS, '{"kind":"tick","n":83}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R7"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":81')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":82')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      logged.length = 0;
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R8"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":83')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a same-document navigation keeps the subscription when the document stays ordinary', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R23"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'W1', ALLOWED_WS, '{"kind":"tick","n":91}');
        send({
          method: 'Page.navigatedWithinDocument',
          params: { frameId: page.targetId, url: `${page.url}#section` },
          ...(sessionId ? { sessionId } : {}),
        });
        announceSocket(send, sessionId, 'W2', ALLOWED_WS, '{"kind":"tick","n":92}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R23"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":91')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":92')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an explicit endpoint does not send Target.setDiscoverTargets', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const discover = [];
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Target.setDiscoverTargets') {
        discover.push(message.params);
        send({ id: message.id, result: {} });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R24"') {
        announceSocket(send, sessionId, 'UD1', ALLOWED_WS, '{"kind":"tick","n":101}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = fake.url;
  await settle();
  const { logged, restore } = captureLog();
  let handle;
  try {
    handle = await browser.websocket
      .on(page, { urlMatch: /./u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    assert.equal(await browser.eval(page, '"R24"'), 'ok');
    await settle();
    assert.equal(discover.length, 0);
    assert.equal(
      logged.some((line) => line.includes('"n":101')),
      true
    );
  } finally {
    restore();
    await handle?.close();
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await settle();
  }
});

test('a detached session drops its subscriptions and forgets the tab document', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let attaches = 0;
  const fake = await fakeCdp({
    answers: {
      ...wsAnswers(page),
      'Target.attachToTarget': () => {
        attaches += 1;
        return { sessionId: attaches === 1 ? 'S1' : 'S2' };
      },
    },
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attaches += 1;
        send({
          id: message.id,
          result: { sessionId: attaches === 1 ? 'S1' : 'S2' },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R9"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'D1', ALLOWED_WS, '{"kind":"tick","n":111}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R10"') {
        send({
          method: 'Target.detachedFromTarget',
          params: { sessionId: 'S1', targetId: page.targetId },
        });
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R11"') {
        announceSocket(send, sessionId, 'D2', ALLOWED_WS, '{"kind":"tick","n":112}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R9"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":111')),
        true
      );
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"R10"'), 'ok');
      await settle();
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      assert.equal(await browser.eval(tab, '"R11"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":112')),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a destroyed target drops its subscriptions and forgets the tab document', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R12"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'X1', ALLOWED_WS, '{"kind":"tick","n":121}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R13"') {
        send({
          method: 'Target.targetDestroyed',
          params: { targetId: page.targetId },
        });
        announceSocket(send, sessionId, 'X2', ALLOWED_WS, '{"kind":"tick","n":122}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R12"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":121')),
        true
      );
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"R13"'), 'ok');
      await settle();
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      assert.equal(
        logged.some((line) => line.includes('"n":122')),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('attach sets the tab document from Target.getTargets when unknown', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R14"') {
        announceSocket(send, sessionId, 'SEED1', ALLOWED_WS, '{"kind":"tick","n":131}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      await settle();
      assert.equal(await browser.eval(tab, '"R14"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":131')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a late Target.getTargets answer does not overwrite a newer document event', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let attached = false;
  let seedSeen = false;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attached = true;
        send({ id: message.id, result: { sessionId: 'S1' } });
        return true;
      }
      if (message.method === 'Target.getTargets') {
        if (attached && !seedSeen) {
          seedSeen = true;
          send({
            method: 'Target.targetInfoChanged',
            params: {
              targetInfo: {
                targetId: page.targetId,
                type: 'page',
                url: 'https://lars.sliccy.ai/',
                title: 'seven',
              },
            },
          });
          setTimeout(() => {
            send({
              id: message.id,
              result: { targetInfos: [info(page)] },
            });
          }, 40);
          return true;
        }
        send({
          id: message.id,
          result: { targetInfos: [info(page)] },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
  });
});

test('a new session for a tab starts with an unknown document class', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let listing = page.url;
  let seedUrl = page.url;
  let attaches = 0;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attaches += 1;
        if (seedUrl !== page.url) {
          emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, page.url);
        }
        listing = seedUrl;
        send({
          id: message.id,
          result: { sessionId: `S${attaches}` },
        });
        return true;
      }
      if (message.method === 'Target.getTargets') {
        send({
          id: message.id,
          result: {
            targetInfos: [info({ ...page, url: listing, title: listing })],
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R15"') {
        announceSocket(send, sessionId, 'N1', ALLOWED_WS, '{"kind":"tick","n":141}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    let handle = await browser.websocket
      .on(tab, { urlMatch: /./u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    await handle.close();
    await settle();
    listing = page.url;
    seedUrl = 'https://lars.sliccy.ai/';
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
    await settle();
    listing = page.url;
    seedUrl = page.url;
    const { logged, restore } = captureLog();
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R15"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":141')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a chrome-error document drops the subscription and does not route', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R16"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'CE1', ALLOWED_WS, '{"kind":"tick","n":151}');
        send({
          method: 'Page.frameNavigated',
          params: {
            frame: {
              id: page.targetId,
              url: 'chrome-error://chromewebdata/',
              unreachableUrl: 'https://example.test/missing',
              loaderId: 'Lerr',
              securityOrigin: '://',
            },
          },
          ...(sessionId ? { sessionId } : {}),
        });
        announceSocket(send, sessionId, 'CE2', ALLOWED_WS, '{"kind":"tick","n":152}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R16"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":151')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":152')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a socket announced while the tab document is protected does not route after return', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const other = { targetId: 'TWO', url: 'https://other.test/', title: 'other' };
  const docB = 'https://lars.sliccy.ai/';
  const docABack = 'https://example.test/back';
  const sessions = { OK: 'S-OK', TWO: 'S-TWO' };
  let attaches = 0;
  const fake = await fakeCdp({
    answers: wsAnswers(page, other),
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attaches += 1;
        const targetId = message.params?.targetId;
        send({
          id: message.id,
          result: { sessionId: sessions[targetId] ?? `S-${attaches}` },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R17"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        send({
          method: 'Network.webSocketCreated',
          params: { requestId: 'PS1', url: ALLOWED_WS },
          ...(sessionId ? { sessionId } : {}),
        });
        emitMainFrame(send, sessionId, page.targetId, docB);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R18"') {
        send({
          method: 'Network.webSocketCreated',
          params: { requestId: 'PS2', url: ALLOWED_WS },
          ...(sessionId ? { sessionId } : {}),
        });
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R29"') {
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, docABack);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R30"') {
        emitFrame(
          send,
          sessionId,
          'PS1',
          '{"kind":"tick","n":161}',
          'Network.webSocketFrameReceived'
        );
        emitFrame(
          send,
          sessionId,
          'PS2',
          '{"kind":"tick","n":162}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const otherTab = await browser.findTab({ domain: 'other.test' });
    const hold = await browser.websocket
      .on(otherTab, { urlMatch: /./u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    let handle = await browser.websocket
      .on(tab, { urlMatch: /./u })
      .filter({ parseAs: 'json' })
      .forward({ sink: 'log' });
    const attachesBefore = attaches;
    assert.equal(await browser.eval(tab, '"R17"'), 'ok');
    await settle();
    assert.equal(
      (await browser.websocket.list()).some((sub) => sub.id === handle.id),
      false
    );
    assert.equal(
      (await browser.websocket.list()).some((sub) => sub.id === hold.id),
      true
    );
    assert.equal(attaches, attachesBefore);
    assert.equal(await browser.eval(tab, '"R18"'), 'ok');
    await settle();
    assert.equal(await browser.eval(tab, '"R29"'), 'ok');
    await settle();
    const { logged, restore } = captureLog();
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R30"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":161')),
        false
      );
      assert.equal(
        logged.some((line) => line.includes('"n":162')),
        false
      );
      assert.equal(attaches, attachesBefore);
    } finally {
      restore();
      await handle?.close();
      await hold.close();
    }
  });
});

test('an inheriting or malformed document event leaves an unknown tab document unknown', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let attached = false;
  let seedEmpty = true;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Target.attachToTarget') {
        attached = true;
        send({ id: message.id, result: { sessionId: 'S1' } });
        return true;
      }
      if (message.method === 'Target.getTargets') {
        if (attached && seedEmpty) {
          seedEmpty = false;
          send({
            id: message.id,
            result: {
              targetInfos: [
                {
                  targetId: page.targetId,
                  type: 'page',
                  url: '',
                  title: '',
                  attached: false,
                  canAccessOpener: false,
                  browserContextId: 'CTX',
                },
              ],
            },
          });
          const sessionId = 'S1';
          emitMainFrame(send, sessionId, page.targetId, 'about:blank');
          emitMainFrame(send, sessionId, page.targetId, 'about:srcdoc');
          send({
            method: 'Page.navigatedWithinDocument',
            params: { frameId: page.targetId, url: 'about:blank' },
            sessionId,
          });
          emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, 'about:blank');
          emitTargetInfo(send, 'Target.targetCreated', page.targetId, 'about:blank');
          send({
            method: 'Page.frameNavigated',
            params: {
              frame: {
                id: page.targetId,
                loaderId: 'Lbad',
                securityOrigin: '',
              },
            },
            sessionId,
          });
          send({
            method: 'Page.frameNavigated',
            params: {
              frame: {
                id: page.targetId,
                url: 42,
                loaderId: 'Lnum',
                securityOrigin: '',
              },
            },
            sessionId,
          });
          send({
            method: 'Page.navigatedWithinDocument',
            params: { frameId: page.targetId },
            sessionId,
          });
          return true;
        }
        send({
          id: message.id,
          result: { targetInfos: [info(page)] },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
  });
});

test('frames are not routed after the tab document class leaves ordinary', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const docB = 'https://lars.sliccy.ai/';
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R19"') {
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, page.url);
        send({
          method: 'Network.webSocketCreated',
          params: { requestId: 'RG1', url: ALLOWED_WS },
          ...(sessionId ? { sessionId } : {}),
        });
        emitFrame(
          send,
          sessionId,
          'RG1',
          '{"kind":"tick","n":171}',
          'Network.webSocketFrameReceived'
        );
        emitTargetInfo(send, 'Target.targetInfoChanged', page.targetId, docB);
        emitFrame(
          send,
          sessionId,
          'RG1',
          '{"kind":"tick","n":172}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R19"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":171')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":172')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a same-document navigation to a protected url drops the subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R20"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocket(send, sessionId, 'WP1', ALLOWED_WS, '{"kind":"tick","n":181}');
        send({
          method: 'Page.navigatedWithinDocument',
          params: { frameId: page.targetId, url: 'https://lars.sliccy.ai/#x' },
          ...(sessionId ? { sessionId } : {}),
        });
        announceSocket(send, sessionId, 'WP2', ALLOWED_WS, '{"kind":"tick","n":182}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R20"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":181')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":182')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a protected targetCreated drops the subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R21"') {
        emitTargetInfo(send, 'Target.targetCreated', page.targetId, page.url);
        announceSocket(send, sessionId, 'TCP1', ALLOWED_WS, '{"kind":"tick","n":191}');
        emitTargetInfo(send, 'Target.targetCreated', page.targetId, 'https://lars.sliccy.ai/');
        announceSocket(send, sessionId, 'TCP2', ALLOWED_WS, '{"kind":"tick","n":192}');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R21"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":191')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":192')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

function sessionEndRow(title, expression, emitEnd) {
  test(title, async () => {
    const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
    const fake = await fakeCdp({
      answers: wsAnswers(page),
      intercept: (message, send) => {
        if (message.method !== 'Runtime.evaluate') return false;
        const text =
          typeof message.params?.expression === 'string' ? message.params.expression : '';
        const sessionId = message.sessionId;
        if (text === `"${expression}-before"`) {
          emitMainFrame(send, sessionId, page.targetId, page.url);
          announceSocket(send, sessionId, 'SE1', ALLOWED_WS, '{"kind":"tick","n":201}');
          replyEvaluate(message, send, 'ok');
          return true;
        }
        if (text === `"${expression}-end"`) {
          emitEnd(send, sessionId, page.targetId);
          announceSocket(send, sessionId, 'SE2', ALLOWED_WS, '{"kind":"tick","n":202}');
          replyEvaluate(message, send, 'ok');
          return true;
        }
        return guardedIntercept(message, send);
      },
    });
    after(() => fake.close());
    await withDiscovered(fake, async () => {
      const tab = await browser.findTab({ domain: 'example.test' });
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, `"${expression}-before"`), 'ok');
        await settle();
        assert.equal(
          logged.some((line) => line.includes('"n":201')),
          true
        );
        logged.length = 0;
        assert.equal(await browser.eval(tab, `"${expression}-end"`), 'ok');
        await settle();
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          false
        );
        assert.equal(
          logged.some((line) => line.includes('"n":202')),
          false
        );
      } finally {
        restore();
        await handle?.close();
      }
    });
  });
}

sessionEndRow(
  'an Inspector.detached session drops its subscriptions and forgets the tab document',
  'R26',
  (send, sessionId) => {
    send({
      method: 'Inspector.detached',
      params: { reason: 'target_closed', sessionId },
      sessionId,
    });
  }
);

sessionEndRow(
  'an Inspector.targetCrashed session drops its subscriptions and forgets the tab document',
  'R27',
  (send, sessionId) => {
    send({
      method: 'Inspector.targetCrashed',
      params: {},
      sessionId,
    });
  }
);

sessionEndRow(
  'a Target.targetCrashed target drops its subscriptions and forgets the tab document',
  'R28',
  (send, _sessionId, targetId) => {
    send({
      method: 'Target.targetCrashed',
      params: { targetId, status: 'crashed', errorCode: 1 },
    });
  }
);

const CHILD_PROTECTED = 'https://lars.sliccy.ai/';
const CHILD_ORDINARY = 'https://child.example.test/';
const SIBLING = { targetId: 'TWO', url: 'https://other.test/', title: 'two' };

function stackAt(url) {
  return {
    type: 'script',
    stack: {
      callFrames: [{ url, lineNumber: 1, columnNumber: 1, functionName: '' }],
    },
  };
}

function wsAnswersWithSibling(page) {
  return {
    'Target.getTargets': { targetInfos: [info(page), info(SIBLING)] },
    'Page.enable': {},
    'Runtime.enable': {},
    'Network.enable': {},
  };
}

function attachByTarget(message, send, counts) {
  if (message.method !== 'Target.attachToTarget') return false;
  counts.n += 1;
  send({ id: message.id, result: { sessionId: `S${counts.n}` } });
  return true;
}

async function holdSecondTab() {
  const sibling = await browser.findTab({ domain: 'other.test' });
  return browser.websocket
    .on(sibling, { urlMatch: /./u })
    .filter({ parseAs: 'json' })
    .forward({ sink: 'log' });
}

function attachPair(message, send, counts, pending) {
  if (message.method !== 'Target.attachToTarget') return false;
  counts.n += 1;
  const sessionId = `S${counts.n}`;
  if (pending && counts.n <= 2) {
    pending.push({ id: message.id, sessionId });
    if (pending.length === 2) {
      for (const item of pending) {
        send({ id: item.id, result: { sessionId: item.sessionId } });
      }
      pending.length = 0;
    }
    return true;
  }
  send({ id: message.id, result: { sessionId } });
  return true;
}

test('a child frame that is not ordinary stops websocket routing for the tab', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R1a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'CHILD', page.targetId, CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'CF1', ALLOWED_WS, stackAt(CHILD_PROTECTED));
        emitFrame(
          send,
          sessionId,
          'CF1',
          '{"kind":"tick","n":201}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R1b"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocketCreated(send, sessionId, 'CF2', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'CF2',
          '{"kind":"tick","n":202}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'CHILD2', page.targetId, CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'CF3', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'CF3',
          '{"kind":"tick","n":203}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R1c"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'CHILD3', page.targetId, CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'CF4', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'CF4',
          '{"kind":"tick","n":204}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    {
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, '"R1a"'), 'ok');
        await settle();
        assert.equal(
          logged.some((line) => line.includes('"n":201')),
          false
        );
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          false
        );
      } finally {
        restore();
        await handle?.close();
      }
    }
    await settle();
    {
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, '"R1b"'), 'ok');
        await settle();
        assert.equal(
          logged.some((line) => line.includes('"n":202')),
          true
        );
        assert.equal(
          logged.some((line) => line.includes('"n":203')),
          false
        );
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          false
        );
      } finally {
        restore();
        await handle?.close();
      }
    }
    await settle();
    {
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, '"R1c"'), 'ok');
        await settle();
        assert.equal(
          logged.some((line) => line.includes('"n":204')),
          false
        );
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          false
        );
      } finally {
        restore();
        await handle?.close();
      }
    }
  });
});

test('an ordinary child frame keeps websocket routing for the tab', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R2"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'ORD', page.targetId, CHILD_ORDINARY);
        announceSocketCreated(send, sessionId, 'OC1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'OC1',
          '{"kind":"tick","n":211}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R2"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":211')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a child frame returning to ordinary does not revive a dropped subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R3"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocketCreated(send, sessionId, 'RV1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'RV1',
          '{"kind":"tick","n":221}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'RVCHILD', page.targetId, CHILD_PROTECTED);
        emitFrame(
          send,
          sessionId,
          'RV1',
          '{"kind":"tick","n":222}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'RVCHILD', page.targetId, CHILD_ORDINARY);
        emitFrame(
          send,
          sessionId,
          'RV1',
          '{"kind":"tick","n":223}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R3"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":221')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":222')),
        false
      );
      assert.equal(
        logged.some((line) => line.includes('"n":223')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an attached child frame without a navigation closes the tab for routing', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: page.url,
                loaderId: 'L-OK',
                securityOrigin: 'https://example.test',
              },
              childFrames: [],
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R4"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'ATT', page.targetId);
        announceSocketCreated(send, sessionId, 'ATT1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'ATT1',
          '{"kind":"tick","n":231}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R4"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":231')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an unknown child that becomes ordinary resumes routing for a surviving subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: page.url,
                loaderId: 'L-OK',
                securityOrigin: 'https://example.test',
              },
              childFrames: [],
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R5"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'U1', page.targetId);
        announceSocketCreated(send, sessionId, 'UR1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'UR1',
          '{"kind":"tick","n":281}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'U1', page.targetId, CHILD_ORDINARY);
        announceSocketCreated(send, sessionId, 'UR2', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'UR2',
          '{"kind":"tick","n":282}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R5"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":281')),
        false
      );
      assert.equal(
        logged.some((line) => line.includes('"n":282')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('an ordinary child resolved after attach keeps the same subscription routing', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let tree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { frameTree: tree },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R6a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'R7', page.targetId);
        tree = {
          frame: tree.frame,
          childFrames: [
            {
              frame: {
                id: 'R7',
                parentId: page.targetId,
                url: CHILD_ORDINARY,
                loaderId: 'L-R7',
                securityOrigin: 'https://child.example.test',
              },
              childFrames: [],
            },
          ],
        };
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R6b"') {
        announceSocketCreated(send, sessionId, 'TR1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'TR1',
          '{"kind":"tick","n":291}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R6a"'), 'ok');
      await settle();
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
      assert.equal(await browser.eval(tab, '"R6b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":291')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a child the tree cannot resolve keeps the tab closed without dropping the subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let includeEmpty = false;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: page.url,
                loaderId: 'L-OK',
                securityOrigin: 'https://example.test',
              },
              childFrames: includeEmpty
                ? [
                    {
                      frame: {
                        id: 'EMPTY',
                        parentId: page.targetId,
                        url: '',
                        loaderId: 'L-EMPTY',
                        securityOrigin: '://',
                      },
                      childFrames: [],
                    },
                  ]
                : [],
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R7"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        includeEmpty = true;
        emitFrameAttached(send, sessionId, 'EMPTY', page.targetId);
        announceSocketCreated(send, sessionId, 'TU1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'TU1',
          '{"kind":"tick","n":301}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R7"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":301')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a child resolved as protected by the tree read drops the subscription', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let tree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { frameTree: tree },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R8"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'BAD', page.targetId);
        tree = {
          frame: tree.frame,
          childFrames: [
            {
              frame: {
                id: 'BAD',
                parentId: page.targetId,
                url: CHILD_PROTECTED,
                loaderId: 'L-BAD',
                securityOrigin: 'https://lars.sliccy.ai',
              },
              childFrames: [],
            },
          ],
        };
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R8"'), 'ok');
      await settle();
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      await handle?.close();
    }
  });
});

test('a late child-frame listing does not overwrite a newer child frame class', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let releaseTree;
  let delayTree = false;
  const treeWait = new Promise((resolve) => {
    releaseTree = resolve;
  });
  const emptyTree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        const sessionId = message.sessionId;
        if (!delayTree) {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: { frameTree: emptyTree },
          });
          return true;
        }
        void treeWait.then(() => {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: {
              frameTree: {
                frame: emptyTree.frame,
                childFrames: [
                  {
                    frame: {
                      id: 'LATE',
                      parentId: page.targetId,
                      url: CHILD_PROTECTED,
                      loaderId: 'L-LATE',
                      securityOrigin: 'https://lars.sliccy.ai',
                    },
                    childFrames: [],
                  },
                ],
              },
            },
          });
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R9a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        delayTree = true;
        emitFrameAttached(send, sessionId, 'LATE', page.targetId);
        emitChildFrame(send, sessionId, 'LATE', page.targetId, CHILD_ORDINARY);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R9b"') {
        announceSocketCreated(send, sessionId, 'LT1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'LT1',
          '{"kind":"tick","n":311}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R9a"'), 'ok');
      await settle();
      releaseTree();
      await settle();
      assert.equal(await browser.eval(tab, '"R9b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":311')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a failed child-frame listing still allows a later listing', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let treeCalls = 0;
  let boom = false;
  let listed = [];
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        treeCalls += 1;
        const sessionId = message.sessionId;
        if (boom) {
          boom = false;
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            error: { code: -32000, message: 'frame tree failed' },
          });
          return true;
        }
        send({
          id: message.id,
          ...(sessionId ? { sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: page.url,
                loaderId: 'L-OK',
                securityOrigin: 'https://example.test',
              },
              childFrames: listed.map((id) => ({
                frame: {
                  id,
                  parentId: page.targetId,
                  url: CHILD_ORDINARY,
                  loaderId: `L-${id}`,
                  securityOrigin: 'https://child.example.test',
                },
                childFrames: [],
              })),
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R10a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        boom = true;
        listed = ['CF'];
        emitFrameAttached(send, sessionId, 'CF', page.targetId);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R10b"') {
        listed = ['CF'];
        emitFrameAttached(send, sessionId, 'CF', page.targetId);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R10c"') {
        announceSocketCreated(send, sessionId, 'TT1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'TT1',
          '{"kind":"tick","n":321}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      const afterSeed = treeCalls;
      assert.equal(await browser.eval(tab, '"R10a"'), 'ok');
      await settle();
      assert.equal(treeCalls > afterSeed, true);
      const afterBoom = treeCalls;
      assert.equal(await browser.eval(tab, '"R10b"'), 'ok');
      await settle();
      assert.equal(treeCalls > afterBoom, true);
      assert.equal(await browser.eval(tab, '"R10c"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":321')),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('child-frame listings coalesce across a burst of child frame events', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let treeCalls = 0;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        treeCalls += 1;
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: page.url,
                loaderId: 'L-OK',
                securityOrigin: 'https://example.test',
              },
              childFrames: [],
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R11"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'A', page.targetId);
        emitFrameAttached(send, sessionId, 'B', page.targetId);
        emitFrameAttached(send, sessionId, 'C', page.targetId);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      const afterSeed = treeCalls;
      assert.equal(await browser.eval(tab, '"R11"'), 'ok');
      await settle();
      const added = treeCalls - afterSeed;
      assert.equal(added >= 1, true);
      assert.equal(added <= 2, true);
    } finally {
      await handle?.close();
    }
  });
});

test('a nested child frame that is not ordinary stops websocket routing for the tab', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R12"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'MID', page.targetId, CHILD_ORDINARY);
        emitChildFrame(send, sessionId, 'GRAND', 'MID', CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'GC1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'GC1',
          '{"kind":"tick","n":241}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R12"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":241')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('detaching a non-ordinary child allows a new subscription only', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const attaches = { n: 0 };
  const fake = await fakeCdp({
    answers: wsAnswersWithSibling(page),
    intercept: (message, send) => {
      if (attachByTarget(message, send, attaches)) return true;
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R13a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocketCreated(send, sessionId, 'DT1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'DT1',
          '{"kind":"tick","n":251}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'DTCHILD', page.targetId, CHILD_PROTECTED);
        emitFrame(
          send,
          sessionId,
          'DT1',
          '{"kind":"tick","n":252}',
          'Network.webSocketFrameReceived'
        );
        emitFrameDetached(send, sessionId, 'DTCHILD', 'remove');
        emitFrame(
          send,
          sessionId,
          'DT1',
          '{"kind":"tick","n":253}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R13b"') {
        announceSocketCreated(send, sessionId, 'DT2', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'DT2',
          '{"kind":"tick","n":254}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let keep;
    let handle;
    let next;
    try {
      keep = await holdSecondTab();
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R13a"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":251')),
        true
      );
      assert.equal(
        logged.some((line) => line.includes('"n":252')),
        false
      );
      assert.equal(
        logged.some((line) => line.includes('"n":253')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      logged.length = 0;
      next = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R13b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":254')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === next.id),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === keep.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
      await next?.close();
      await keep?.close();
    }
  });
});

test('attach seeds child frames from the browser frame listing', async () => {
  const page = {
    targetId: 'OK',
    url: 'https://example.test/',
    title: 'ok',
    childFrames: [
      {
        frame: {
          id: 'SEEDED',
          parentId: 'OK',
          url: CHILD_PROTECTED,
          loaderId: 'L-SEEDED',
          securityOrigin: 'https://lars.sliccy.ai',
        },
        childFrames: [],
      },
    ],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    childFrames: page.childFrames,
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R14"') {
        announceSocketCreated(send, sessionId, 'SD1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'SD1',
          '{"kind":"tick","n":261}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
  });
});

test('a child frame event without a main frameNavigated still closes the tab for routing', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R15"') {
        emitChildFrame(send, sessionId, 'EARLY', page.targetId, CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'EF1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'EF1',
          '{"kind":"tick","n":271}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R15"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":271')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test("a frame in another process is outside the tab's frame set", async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let tree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { frameTree: tree },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R20"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitFrameAttached(send, sessionId, 'CHILD', page.targetId);
        tree = {
          frame: tree.frame,
          childFrames: [
            {
              frame: {
                id: 'CHILD',
                parentId: page.targetId,
                url: CHILD_PROTECTED,
                loaderId: 'L-CHILD',
                securityOrigin: 'https://lars.sliccy.ai',
              },
              childFrames: [],
            },
          ],
        };
        emitFrameDetached(send, sessionId, 'CHILD', 'swap');
        announceSocketCreated(send, sessionId, 'SW1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'SW1',
          '{"kind":"tick","n":401}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R20"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":401')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a frame that moved to another process does not keep the tab closed', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let tree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        send({
          id: message.id,
          ...(message.sessionId ? { sessionId: message.sessionId } : {}),
          result: { frameTree: tree },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R21"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'CHILD', page.targetId, CHILD_ORDINARY);
        tree = {
          frame: tree.frame,
          childFrames: [
            {
              frame: {
                id: 'CHILD',
                parentId: page.targetId,
                url: CHILD_ORDINARY,
                loaderId: 'L-CHILD',
                securityOrigin: 'https://child.example.test',
              },
              childFrames: [],
            },
          ],
        };
        emitFrameDetached(send, sessionId, 'CHILD', 'swap');
        announceSocketCreated(send, sessionId, 'SW2', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'SW2',
          '{"kind":"tick","n":402}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R21"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":402')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a frame id that reappears on the page session after leaving is classified fresh', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R22a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        emitChildFrame(send, sessionId, 'CHILD', page.targetId, CHILD_ORDINARY);
        announceSocketCreated(send, sessionId, 'RF1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'RF1',
          '{"kind":"tick","n":403}',
          'Network.webSocketFrameReceived'
        );
        emitFrameDetached(send, sessionId, 'CHILD', 'swap');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R22b"') {
        emitFrameAttached(send, sessionId, 'CHILD', page.targetId);
        emitChildFrame(send, sessionId, 'CHILD', page.targetId, CHILD_PROTECTED);
        announceSocketCreated(send, sessionId, 'RF2', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'RF2',
          '{"kind":"tick","n":404}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R22a"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":403')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
      logged.length = 0;
      assert.equal(await browser.eval(tab, '"R22b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":404')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a protected grandchild under an unknown parent is not overwritten by a stale ordinary tree', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let releaseTree;
  const treeWait = new Promise((resolve) => {
    releaseTree = resolve;
  });
  let hold = false;
  const emptyTree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        const sessionId = message.sessionId;
        if (!hold) {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: { frameTree: emptyTree },
          });
          return true;
        }
        void treeWait.then(() => {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: {
              frameTree: {
                frame: emptyTree.frame,
                childFrames: [
                  {
                    frame: {
                      id: 'MID',
                      parentId: page.targetId,
                      url: CHILD_ORDINARY,
                      loaderId: 'L-MID',
                      securityOrigin: 'https://child.example.test',
                    },
                    childFrames: [
                      {
                        frame: {
                          id: 'GRAND',
                          parentId: 'MID',
                          url: CHILD_ORDINARY,
                          loaderId: 'L-GRAND',
                          securityOrigin: 'https://child.example.test',
                        },
                        childFrames: [],
                      },
                    ],
                  },
                ],
              },
            },
          });
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R22"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        hold = true;
        emitFrameAttached(send, sessionId, 'MID', page.targetId);
        emitChildFrame(send, sessionId, 'GRAND', 'MID', CHILD_PROTECTED);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R22b"') {
        announceSocketCreated(send, sessionId, 'NG1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'NG1',
          '{"kind":"tick","n":411}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R22"'), 'ok');
      await settle();
      releaseTree();
      await settle();
      assert.equal(await browser.eval(tab, '"R22b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":411')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a child frameNavigated with a missing, non-string, or empty url does not change its class', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const cases = [
    { id: 'R23a', frameId: 'M1', url: undefined, n: 421 },
    { id: 'R23b', frameId: 'M2', url: 42, n: 422 },
    { id: 'R23c', frameId: 'M3', url: '', n: 423 },
  ];
  for (const entry of cases) {
    const fake = await fakeCdp({
      answers: wsAnswers(page),
      intercept: (message, send) => {
        if (message.method !== 'Runtime.evaluate') return false;
        const expression =
          typeof message.params?.expression === 'string' ? message.params.expression : '';
        const sessionId = message.sessionId;
        if (expression === `"${entry.id}"`) {
          emitMainFrame(send, sessionId, page.targetId, page.url);
          emitFrameAttached(send, sessionId, entry.frameId, page.targetId);
          const frame = {
            id: entry.frameId,
            parentId: page.targetId,
            loaderId: `L-${entry.frameId}`,
            securityOrigin: '://',
          };
          if (entry.url !== undefined) frame.url = entry.url;
          send({
            method: 'Page.frameNavigated',
            params: { frame },
            ...(sessionId ? { sessionId } : {}),
          });
          announceSocketCreated(send, sessionId, entry.frameId, ALLOWED_WS);
          emitFrame(
            send,
            sessionId,
            entry.frameId,
            `{"kind":"tick","n":${entry.n}}`,
            'Network.webSocketFrameReceived'
          );
          replyEvaluate(message, send, 'ok');
          return true;
        }
        return guardedIntercept(message, send);
      },
    });
    after(() => fake.close());
    await withDiscovered(fake, async () => {
      const tab = await browser.findTab({ domain: 'example.test' });
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, `"${entry.id}"`), 'ok');
        await settle();
        assert.equal(
          logged.some((line) => line.includes(`"n":${entry.n}`)),
          false
        );
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          true
        );
      } finally {
        restore();
        await handle?.close();
      }
    });
  }
});

test('a same-document protected url on an unknown child is recorded and kept against a stale tree', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let releaseTree;
  const treeWait = new Promise((resolve) => {
    releaseTree = resolve;
  });
  let hold = false;
  const emptyTree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        const sessionId = message.sessionId;
        if (!hold) {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: { frameTree: emptyTree },
          });
          return true;
        }
        void treeWait.then(() => {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: {
              frameTree: {
                frame: emptyTree.frame,
                childFrames: [
                  {
                    frame: {
                      id: 'SD',
                      parentId: page.targetId,
                      url: CHILD_ORDINARY,
                      loaderId: 'L-SD',
                      securityOrigin: 'https://child.example.test',
                    },
                    childFrames: [],
                  },
                ],
              },
            },
          });
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"R24"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        hold = true;
        emitFrameAttached(send, sessionId, 'SD', page.targetId);
        send({
          method: 'Page.navigatedWithinDocument',
          params: { frameId: 'SD', url: CHILD_PROTECTED },
          ...(sessionId ? { sessionId } : {}),
        });
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"R24b"') {
        announceSocketCreated(send, sessionId, 'SD1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'SD1',
          '{"kind":"tick","n":431}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"R24"'), 'ok');
      await settle();
      releaseTree();
      await settle();
      assert.equal(await browser.eval(tab, '"R24b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":431')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('a seed frame tree answer does not apply after a main-frame document change', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let seeds = 0;
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        const sessionId = message.sessionId;
        seeds += 1;
        if (seeds === 1) {
          emitMainFrame(send, sessionId, page.targetId, 'https://example.test/next');
          emitFrameAttached(send, sessionId, 'REUSED', page.targetId);
          void Promise.resolve().then(() => {
            send({
              id: message.id,
              ...(sessionId ? { sessionId } : {}),
              result: {
                frameTree: {
                  frame: {
                    id: page.targetId,
                    url: page.url,
                    loaderId: 'L-OK',
                    securityOrigin: 'https://example.test',
                  },
                  childFrames: [
                    {
                      frame: {
                        id: 'REUSED',
                        parentId: page.targetId,
                        url: CHILD_ORDINARY,
                        loaderId: 'L-OLD',
                        securityOrigin: 'https://child.example.test',
                      },
                      childFrames: [],
                    },
                  ],
                },
              },
            });
          });
          return true;
        }
        send({
          id: message.id,
          ...(sessionId ? { sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: 'https://example.test/next',
                loaderId: 'L-NEXT',
                securityOrigin: 'https://example.test',
              },
              childFrames: [],
            },
          },
        });
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    await assert.rejects(
      () =>
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
      { message: REFUSAL }
    );
  });
});

test('a held child-frame listing does not apply after a main-frame document change', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  let releaseTree;
  const treeWait = new Promise((resolve) => {
    releaseTree = resolve;
  });
  let hold = false;
  let trees = 0;
  const emptyTree = {
    frame: {
      id: page.targetId,
      url: page.url,
      loaderId: 'L-OK',
      securityOrigin: 'https://example.test',
    },
    childFrames: [],
  };
  const fake = await fakeCdp({
    answers: wsAnswers(page),
    intercept: (message, send) => {
      if (message.method === 'Page.getFrameTree') {
        const sessionId = message.sessionId;
        trees += 1;
        if (!hold || trees === 1) {
          send({
            id: message.id,
            ...(sessionId ? { sessionId } : {}),
            result: { frameTree: emptyTree },
          });
          return true;
        }
        if (hold && trees === 2) {
          void treeWait.then(() => {
            send({
              id: message.id,
              ...(sessionId ? { sessionId } : {}),
              result: {
                frameTree: {
                  frame: {
                    id: page.targetId,
                    url: 'https://example.test/next',
                    loaderId: 'L-NEXT',
                    securityOrigin: 'https://example.test',
                  },
                  childFrames: [
                    {
                      frame: {
                        id: 'REUSED',
                        parentId: page.targetId,
                        url: CHILD_ORDINARY,
                        loaderId: 'L-OLD',
                        securityOrigin: 'https://child.example.test',
                      },
                      childFrames: [],
                    },
                  ],
                },
              },
            });
          });
          return true;
        }
        send({
          id: message.id,
          ...(sessionId ? { sessionId } : {}),
          result: {
            frameTree: {
              frame: {
                id: page.targetId,
                url: 'https://example.test/next',
                loaderId: 'L-NEXT',
                securityOrigin: 'https://example.test',
              },
              childFrames: [],
            },
          },
        });
        return true;
      }
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"C26"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        hold = true;
        emitFrameAttached(send, sessionId, 'OLD', page.targetId);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"C26b"') {
        emitMainFrame(send, sessionId, page.targetId, 'https://example.test/next');
        emitFrameAttached(send, sessionId, 'REUSED', page.targetId);
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"C26c"') {
        announceSocketCreated(send, sessionId, 'HR1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'HR1',
          '{"kind":"tick","n":451}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let handle;
    try {
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"C26"'), 'ok');
      await settle();
      assert.equal(await browser.eval(tab, '"C26b"'), 'ok');
      await settle();
      releaseTree();
      await settle();
      assert.equal(await browser.eval(tab, '"C26c"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":451')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
    }
  });
});

test('frames of a socket announced before a child frame changed class do not route for a later subscriber', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const closedChild = CHILD_PROTECTED;
  const attaches = { n: 0 };
  const fake = await fakeCdp({
    answers: wsAnswersWithSibling(page),
    intercept: (message, send) => {
      if (attachByTarget(message, send, attaches)) return true;
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"A30a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocketCreated(send, sessionId, 'CL1', ALLOWED_WS);
        emitFrame(
          send,
          sessionId,
          'CL1',
          '{"kind":"tick","n":501}',
          'Network.webSocketFrameReceived'
        );
        emitChildFrame(send, sessionId, 'CF1', page.targetId, closedChild);
        emitFrameDetached(send, sessionId, 'CF1', 'remove');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"A30b"') {
        emitFrame(
          send,
          sessionId,
          'CL1',
          '{"kind":"tick","n":502}',
          'Network.webSocketFrameReceived'
        );
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let keep;
    let handle;
    let next;
    try {
      keep = await holdSecondTab();
      handle = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"A30a"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":501')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      logged.length = 0;
      next = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"A30b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":502')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === next.id),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === keep.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
      await next?.close();
      await keep?.close();
    }
  });
});

test('frames of a socket from an earlier session do not route for a later subscriber after a child frame changed class', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const closedChild = CHILD_PROTECTED;
  const attaches = { n: 0 };
  const pending = [];
  const fake = await fakeCdp({
    answers: wsAnswersWithSibling(page),
    intercept: (message, send) => {
      if (attachPair(message, send, attaches, pending)) return true;
      if (message.method !== 'Runtime.evaluate') return false;
      const expression =
        typeof message.params?.expression === 'string' ? message.params.expression : '';
      const sessionId = message.sessionId;
      if (expression === '"A32a"') {
        emitMainFrame(send, sessionId, page.targetId, page.url);
        announceSocketCreated(send, 'S1', 'CL2', ALLOWED_WS);
        emitFrame(send, 'S1', 'CL2', '{"kind":"tick","n":521}', 'Network.webSocketFrameReceived');
        emitChildFrame(send, sessionId, 'CF2', page.targetId, closedChild);
        emitFrameDetached(send, sessionId, 'CF2', 'remove');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      if (expression === '"A32b"') {
        emitFrame(send, 'S1', 'CL2', '{"kind":"tick","n":522}', 'Network.webSocketFrameReceived');
        replyEvaluate(message, send, 'ok');
        return true;
      }
      return guardedIntercept(message, send);
    },
  });
  after(() => fake.close());
  await withDiscovered(fake, async () => {
    const tab = await browser.findTab({ domain: 'example.test' });
    const { logged, restore } = captureLog();
    let keep;
    let handle;
    let next;
    try {
      [handle] = await Promise.all([
        browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' }),
        browser.eval(tab, '1'),
      ]);
      assert.equal(attaches.n >= 2, true);
      keep = await holdSecondTab();
      assert.equal(await browser.eval(tab, '"A32a"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":521')),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === handle.id),
        false
      );
      logged.length = 0;
      next = await browser.websocket
        .on(tab, { urlMatch: /./u })
        .filter({ parseAs: 'json' })
        .forward({ sink: 'log' });
      assert.equal(await browser.eval(tab, '"A32b"'), 'ok');
      await settle();
      assert.equal(
        logged.some((line) => line.includes('"n":522')),
        false
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === next.id),
        true
      );
      assert.equal(
        (await browser.websocket.list()).some((sub) => sub.id === keep.id),
        true
      );
    } finally {
      restore();
      await handle?.close();
      await next?.close();
      await keep?.close();
    }
  });
});

test('a child same-document navigation without a usable url leaves routing unchanged', async () => {
  const page = { targetId: 'OK', url: 'https://example.test/', title: 'ok' };
  const openChild = CHILD_ORDINARY;
  const cases = [
    { id: 'A1', frameId: 'F1', listed: false, url: undefined, n: 511, routes: false },
    { id: 'A2', frameId: 'F2', listed: false, url: 42, n: 512, routes: false },
    { id: 'A3', frameId: 'F3', listed: false, url: '', n: 513, routes: false },
    { id: 'A4', frameId: 'F4', listed: true, url: undefined, n: 514, routes: true },
    { id: 'A5', frameId: 'F5', listed: true, url: 42, n: 515, routes: true },
    { id: 'A6', frameId: 'F6', listed: true, url: '', n: 516, routes: true },
  ];
  for (const entry of cases) {
    let listingCount = 0;
    const noListing = {
      frame: {
        id: page.targetId,
        url: page.url,
        loaderId: 'L-OK',
        securityOrigin: 'https://example.test',
      },
      childFrames: [],
    };
    const fake = await fakeCdp({
      answers: wsAnswers(page),
      intercept: (message, send) => {
        if (message.method === 'Page.getFrameTree') {
          listingCount += 1;
          send({
            id: message.id,
            ...(message.sessionId ? { sessionId: message.sessionId } : {}),
            result: { frameTree: noListing },
          });
          return true;
        }
        if (message.method !== 'Runtime.evaluate') return false;
        const expression =
          typeof message.params?.expression === 'string' ? message.params.expression : '';
        const sessionId = message.sessionId;
        if (expression === `"${entry.id}a"`) {
          emitMainFrame(send, sessionId, page.targetId, page.url);
          if (entry.listed) {
            emitChildFrame(send, sessionId, entry.frameId, page.targetId, openChild);
          } else {
            emitFrameAttached(send, sessionId, entry.frameId, page.targetId);
          }
          replyEvaluate(message, send, 'ok');
          return true;
        }
        if (expression === `"${entry.id}b"`) {
          const params = { frameId: entry.frameId };
          if (entry.url !== undefined) params.url = entry.url;
          send({
            method: 'Page.navigatedWithinDocument',
            params,
            ...(sessionId ? { sessionId } : {}),
          });
          announceSocketCreated(send, sessionId, entry.frameId, ALLOWED_WS);
          emitFrame(
            send,
            sessionId,
            entry.frameId,
            `{"kind":"tick","n":${entry.n}}`,
            'Network.webSocketFrameReceived'
          );
          replyEvaluate(message, send, 'ok');
          return true;
        }
        return guardedIntercept(message, send);
      },
    });
    after(() => fake.close());
    await withDiscovered(fake, async () => {
      const tab = await browser.findTab({ domain: 'example.test' });
      const { logged, restore } = captureLog();
      let handle;
      try {
        handle = await browser.websocket
          .on(tab, { urlMatch: /./u })
          .filter({ parseAs: 'json' })
          .forward({ sink: 'log' });
        assert.equal(await browser.eval(tab, `"${entry.id}a"`), 'ok');
        await settle();
        const before = listingCount;
        assert.equal(await browser.eval(tab, `"${entry.id}b"`), 'ok');
        await settle();
        assert.equal(listingCount > before, true);
        assert.equal(
          logged.some((line) => line.includes(`"n":${entry.n}`)),
          entry.routes
        );
        assert.equal(
          (await browser.websocket.list()).some((sub) => sub.id === handle.id),
          true
        );
      } finally {
        restore();
        await handle?.close();
      }
    });
  }
});
