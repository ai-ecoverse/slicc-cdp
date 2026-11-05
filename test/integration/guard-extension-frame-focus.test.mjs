import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest } from './chrome-policy.mjs';
import { cleanup } from './cleanup.mjs';
import { guarded, observer } from './guard-client.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';

async function value(client, session, expression) {
  const answer = await client.send(
    'Runtime.evaluate',
    {
      expression,
      awaitPromise: true,
      returnByValue: true,
    },
    session
  );
  assert.equal(answer.exceptionDetails, undefined, JSON.stringify(answer));
  return answer.result?.value;
}

for (const nested of [false, true]) {
  chromeTest(
    `hidden extension input requires no deep focus in ${nested ? 'nested documents and ' : ''}closed shadow roots`,
    { timeout: 120000 },
    async () => {
      execFileSync(
        'cargo',
        ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
        { stdio: 'inherit' }
      );
      const fixture = await fixtureServer();
      let browser;
      let raw;
      let client;
      try {
        browser = await launchBrowser([
          ...browserArgs({ privileged: true }),
          '--enable-unsafe-extension-debugging',
        ]);
        raw = await observer(browser.wsUrl);
        const path = fileURLToPath(
          new URL(
            './harness/extension',
            import.meta.resolve('@ai-ecoverse/slicc-shared-web/package.json')
          )
        );
        const extension = await raw.send('Extensions.loadUnpacked', { path });
        const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
        const page = (await raw.send('Target.attachToTarget', { targetId, flatten: true }))
          .sessionId;
        await raw.send('Page.enable', {}, page);
        const loaded = raw.waitEvent(
          (event) => event.method === 'Page.loadEventFired' && event.sessionId === page,
          10000
        );
        await raw.send('Page.navigate', { url: `${origins(fixture).ordinary}/blank` }, page);
        await loaded;
        await raw.send('Page.bringToFront', {}, page);
        assert.equal(
          await value(
            raw,
            page,
            `new Promise(resolve => {
      document.body.innerHTML = '<input id="ordinary"><div id="host"></div>';
      let parent = document;
      if (${nested}) {
        const container = document.createElement('iframe');
        container.id = 'container';
        document.body.append(container);
        parent = container.contentDocument;
        parent.body.innerHTML = '<div id="host"></div>';
      }
      const root = parent.getElementById('host').attachShadow({mode:'closed'});
      const frame = parent.createElement('iframe');
      frame.style.cssText = 'position:fixed;left:-999999px;visibility:hidden;opacity:0';
      frame.onload = () => resolve(true);
      frame.src = ${JSON.stringify(`chrome-extension://${extension.id}/frame.html`)};
      globalThis.focusExtension = () => frame.contentWindow.focus();
      globalThis.extensionAddress = () => frame.src;
      globalThis.showExtension = () => { frame.style.cssText = 'position:fixed;left:0;top:0;width:300px;height:200px'; };
      root.append(frame);
    })`
          ),
          true
        );
        const { targetInfos } = await raw.send('Target.getTargets');
        const frame = targetInfos.find(
          (info) =>
            info.type === 'iframe' && info.url.startsWith(`chrome-extension://${extension.id}/`)
        );
        assert.ok(frame);
        const privateSession = (
          await raw.send('Target.attachToTarget', { targetId: frame.targetId, flatten: true })
        ).sessionId;
        await value(
          raw,
          privateSession,
          `(() => {
      document.body.innerHTML = '<input id="private">';
      globalThis.keys = [];
      document.addEventListener('keydown', event => keys.push(event.key));
      return true;
    })()`
        );
        client = await guarded(browser.wsUrl, origins(fixture).client, {
          variant: 'prompt',
          detachDelay: 0,
        });
        const session = (await client.send('Target.attachToTarget', { targetId, flatten: true }))
          .sessionId;
        for (const [method, params] of [
          ['DOM.getDocument', { depth: -1, pierce: true }],
          ['DOMSnapshot.captureSnapshot', { computedStyles: [] }],
          ['Runtime.evaluate', { expression: 'extensionAddress()', returnByValue: true }],
        ]) {
          const answer = await client.send(method, params, session);
          assert.equal(
            JSON.stringify(answer).includes(`chrome-extension://${extension.id}`),
            false,
            `${method} exposed the extension origin`
          );
        }
        await value(client, session, 'document.getElementById("ordinary").focus()');
        await client.send('Input.insertText', { text: 'page' }, session);
        assert.equal(await value(raw, page, 'document.getElementById("ordinary").value'), 'page');
        await value(raw, privateSession, 'document.getElementById("private").focus()');
        await value(client, session, 'focusExtension()');
        assert.equal(
          await value(raw, page, 'document.activeElement.id'),
          nested ? 'container' : 'host'
        );
        assert.equal(await value(raw, privateSession, 'document.hasFocus()'), true);
        for (const [method, params] of [
          ['Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowDown', code: 'ArrowDown' }],
          ['Input.insertText', { text: 'private' }],
          ['Input.imeSetComposition', { text: 'private', selectionStart: 7, selectionEnd: 7 }],
          [
            'Input.dispatchMouseEvent',
            { type: 'mousePressed', x: 20, y: 20, button: 'left', clickCount: 1 },
          ],
        ]) {
          await assert.rejects(
            client.send(method, params, session),
            /slicc: refused:.*chrome-extension:\/\//
          );
        }
        assert.deepEqual(await value(raw, privateSession, 'keys'), []);
        assert.equal(
          await value(raw, privateSession, 'document.getElementById("private").value'),
          ''
        );
        await assert.rejects(
          client.send('Page.captureScreenshot', { format: 'png' }, session),
          /slicc: refused:.*chrome-extension:\/\//
        );
        await value(client, session, 'document.getElementById("ordinary").focus()');
        await client.send('Input.insertText', { text: '-again' }, session);
        assert.equal(
          await value(raw, page, 'document.getElementById("ordinary").value'),
          'page-again'
        );
        await value(client, session, 'showExtension()');
        await assert.rejects(
          client.send('Input.insertText', { text: 'blocked' }, session),
          /slicc: refused:.*chrome-extension:\/\//
        );
        await assert.rejects(
          client.send('Page.captureScreenshot', { format: 'png' }, session),
          /slicc: refused:.*chrome-extension:\/\//
        );
      } finally {
        await cleanup([
          () => client?.close(),
          () => raw?.close(),
          () => browser?.close(),
          () => fixture.close(),
        ]);
      }
    }
  );
}

async function captureWithInsertDuringReply(insertExpression) {
  execFileSync(
    'cargo',
    ['build', '--locked', '-p', 'slicc-cdp-client', '--example', 'guard_bridge'],
    { stdio: 'inherit' }
  );
  const fixture = await fixtureServer();
  let browser;
  let raw;
  let proxy;
  let client;
  try {
    browser = await launchBrowser([
      ...browserArgs({ privileged: true }),
      '--enable-unsafe-extension-debugging',
    ]);
    raw = await observer(browser.wsUrl);
    const path = fileURLToPath(
      new URL(
        './harness/extension',
        import.meta.resolve('@ai-ecoverse/slicc-shared-web/package.json')
      )
    );
    const extension = await raw.send('Extensions.loadUnpacked', { path });
    const { targetId } = await raw.send('Target.createTarget', { url: 'about:blank' });
    const page = (await raw.send('Target.attachToTarget', { targetId, flatten: true })).sessionId;
    await raw.send('Page.enable', {}, page);
    const loaded = raw.waitEvent(
      (event) => event.method === 'Page.loadEventFired' && event.sessionId === page,
      10000
    );
    await raw.send('Page.navigate', { url: `${origins(fixture).ordinary}/blank` }, page);
    await loaded;
    await raw.send('Page.bringToFront', {}, page);
    await value(raw, page, insertExpression(extension.id, origins(fixture)));
    let captureId;
    proxy = await cdpProxy(browser.wsUrl, {
      observe(direction, message) {
        if (direction === 'client' && message.method === 'Page.captureScreenshot')
          captureId = message.id;
      },
      responseBarrier(message) {
        if (!captureId || message.id !== captureId) return null;
        return value(raw, page, 'insertDuringCapture()').then(() => undefined);
      },
    });
    client = await guarded(proxy.url, origins(fixture).client, {
      variant: 'prompt',
      detachDelay: 0,
    });
    const session = (await client.send('Target.attachToTarget', { targetId, flatten: true }))
      .sessionId;
    return {
      send: () => client.send('Page.captureScreenshot', { format: 'png' }, session),
      cleanup: () =>
        cleanup([
          () => client?.close(),
          () => proxy?.close(),
          () => raw?.close(),
          () => browser?.close(),
          () => fixture.close(),
        ]),
    };
  } catch (error) {
    await cleanup([
      () => client?.close(),
      () => proxy?.close(),
      () => raw?.close(),
      () => browser?.close(),
      () => fixture.close(),
    ]);
    throw error;
  }
}

chromeTest(
  'shown extension frame inserted during capture is refused',
  { timeout: 120000 },
  async () => {
    const row = await captureWithInsertDuringReply(
      (extensionId) => `(() => {
      document.body.innerHTML = '<input id="ordinary">';
      globalThis.insertDuringCapture = () => new Promise(resolve => {
        const frame = document.createElement('iframe');
        frame.style.cssText = 'position:fixed;left:0;top:0;width:300px;height:200px';
        frame.onload = () => resolve(true);
        frame.src = ${JSON.stringify(`chrome-extension://${extensionId}/frame.html`)};
        document.body.append(frame);
      });
      return true;
    })()`
    );
    try {
      await assert.rejects(row.send(), /slicc: refused:.*chrome-extension:\/\//);
    } finally {
      await row.cleanup();
    }
  }
);

chromeTest(
  'protected frame inserted during capture keeps protected refusal',
  { timeout: 120000 },
  async () => {
    const row = await captureWithInsertDuringReply(
      (_extensionId, hosts) => `(() => {
      document.body.innerHTML = '<input id="ordinary">';
      globalThis.insertDuringCapture = () => new Promise(resolve => {
        const frame = document.createElement('iframe');
        frame.style.cssText = 'position:fixed;left:0;top:0;width:300px;height:200px';
        frame.onload = () => resolve(true);
        frame.onerror = () => resolve(true);
        frame.src = ${JSON.stringify(`${hosts.seven}/blank`)};
        document.body.append(frame);
      });
      return true;
    })()`
    );
    try {
      await assert.rejects(row.send(), /slicc: refused: target is on seven's origin/);
    } finally {
      await row.cleanup();
    }
  }
);
