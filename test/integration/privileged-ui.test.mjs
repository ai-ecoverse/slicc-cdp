import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { launchBrowser, rawSocket } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { browserArgs } from './guard-fixture.mjs';
import { nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';

const refusedUrls = [
  'chrome://settings',
  'about:settings',
  'view-source:chrome://settings',
  'file:///etc/hosts',
  'chrome-extension://abcdefghijklmnopabcdefghijklmnop/options.html',
];

function refused(result) {
  assert.notEqual(result.code, 0, result.stdout);
  assert.match(result.stderr, /slicc: refused:/);
}

function targetInfo(message, replacements) {
  if (!message || typeof message !== 'object') return message;
  if (typeof message.targetId === 'string' && typeof message.type === 'string') {
    const replacement = replacements.get(message.targetId);
    if (replacement) Object.assign(message, replacement);
  }
  for (const value of Object.values(message)) targetInfo(value, replacements);
  return message;
}

test('privileged UI protection through the public native CLIs', { timeout: 240000 }, async (t) => {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-c8-'));
  const site = await startServer();
  let browser;
  let raw;
  let proxy;
  let runner;
  try {
    browser = await launchBrowser(browserArgs({ privileged: true }));
    raw = await rawSocket(browser.wsUrl);
    const ordinary = await raw.open(`${site.origin}/page`);
    const ui = await raw.open(`${site.origin}/page?ui`);
    const worker = await raw.open(`${site.origin}/page?worker`);
    const extensionDocuments = [];
    const extensionOrigins = new Map();
    for (const origin of [
      'chrome-extension://akjjllgokmbgpbdbmafpiefnhidlmbgf',
      'chrome-extension://abcdefghijklmnopabcdefghijklmnop',
    ]) {
      const targetId = await raw.open('about:blank');
      const { sessionId } = await raw.send('Target.attachToTarget', {
        targetId,
        flatten: true,
      });
      const tree = await raw.send('Page.getFrameTree', {}, sessionId);
      assert.equal(tree.frameTree.frame.url, 'about:blank');
      extensionOrigins.set(tree.frameTree.frame.id, origin);
      extensionDocuments.push({ targetId, origin });
      await raw.send('Target.detachFromTarget', { sessionId });
    }
    const replacements = new Map([
      [ui, { type: 'browser_ui', url: `${site.origin}/page?ui`, title: 'Privileged UI' }],
      [worker, { type: 'service_worker', url: refusedUrls[4], title: 'Extension worker' }],
    ]);
    proxy = await cdpProxy(browser.wsUrl, {
      transform: (message) => {
        targetInfo(message, replacements);
        if (message.method === 'Runtime.executionContextCreated') {
          const context = message.params.context;
          const origin = extensionOrigins.get(context.auxData?.frameId);
          if (origin && context.auxData?.isDefault === true) context.origin = origin;
        }
        return message;
      },
    });
    runner = nativeRunner(
      path.resolve('target/debug/playwright-cli'),
      path.resolve('target/debug/curlwright'),
      proxy.url
    );
    await t.test(
      'new-tab pages remain listed, attachable and navigable with their NTP frame',
      async () => {
        const ntp = await raw.open('chrome://new-tab-page/');
        const { sessionId } = await raw.send('Target.attachToTarget', {
          targetId: ntp,
          flatten: true,
        });
        await raw.send('Page.enable', {}, sessionId);
        let ntpFrame = false;
        for (let attempt = 0; attempt < 50 && !ntpFrame; attempt += 1) {
          const tree = await raw.send('Page.getFrameTree', {}, sessionId);
          const { targetInfos } = await raw.send('Target.getTargets');
          ntpFrame =
            tree.frameTree.childFrames?.some(({ frame }) =>
              frame.url.startsWith('chrome-untrusted://new-tab-page/')
            ) ||
            targetInfos.some(
              (info) =>
                info.type === 'iframe' &&
                info.parentId === ntp &&
                info.url.startsWith('chrome-untrusted://new-tab-page/')
            );
          if (!ntpFrame) await new Promise((resolve) => setTimeout(resolve, 100));
        }
        assert(ntpFrame, 'the real NTP embeds its chrome-untrusted frame');
        await raw.send('Target.detachFromTarget', { sessionId });
        const list = await runner.playwright(['tab-list'], cwd);
        assert.equal(list.code, 0, list.stderr);
        assert(list.stdout.includes(ntp), list.stdout);
        const attached = await runner.playwright(['eval', 'location.href', '--tab', ntp], cwd);
        assert.equal(attached.code, 0, attached.stderr);
        assert.match(attached.stdout, /chrome:\/\/new-tab-page/);
        const navigated = await runner.playwright(
          ['goto', `${site.origin}/page`, '--tab', ntp],
          cwd
        );
        assert.equal(navigated.code, 0, navigated.stderr);
      }
    );
    await t.test(
      'browser_ui is hidden and refused by target id even with an HTTP URL',
      async () => {
        const list = await runner.playwright(['tab-list'], cwd);
        assert.equal(list.code, 0, list.stderr);
        assert(!list.stdout.includes(ui), list.stdout);
        refused(await runner.playwright(['snapshot', '--tab', ui], cwd));
      }
    );
    await t.test('extension service_worker is refused by target id', async () => {
      refused(await runner.playwright(['snapshot', '--tab', worker], cwd));
    });
    for (const { targetId, origin } of extensionDocuments) {
      await t.test(`an about:blank document with default origin ${origin} is refused`, async () => {
        const info = await raw.send('Target.getTargetInfo', { targetId });
        assert.equal(info.targetInfo.type, 'page');
        assert.equal(info.targetInfo.url, 'about:blank');
        assert.equal(info.targetInfo.openerId, undefined);
        for (const args of [
          ['snapshot', '--tab', targetId],
          ['eval', '"EXTENSION-DOCUMENT-MARK"', '--tab', targetId],
        ]) {
          const result = await runner.playwright(args, cwd);
          refused(result);
          assert.doesNotMatch(result.stdout, /EXTENSION-DOCUMENT-MARK/);
        }
      });
    }
    for (const url of refusedUrls) {
      await t.test(`goto refuses ${url}`, async () => {
        refused(await runner.playwright(['goto', url, '--tab', ordinary], cwd));
      });
      await t.test(`tab-new refuses ${url}`, async () => {
        refused(await runner.playwright(['tab-new', url], cwd));
      });
    }
    await t.test('curlwright refuses a local file', async () => {
      refused(await runner.curl(['file:///etc/hosts'], cwd));
    });
  } finally {
    await runner?.clearSession(cwd);
    await proxy?.close();
    raw?.close();
    await browser?.close();
    await site.close();
    await rm(cwd, { recursive: true, force: true });
  }
});
