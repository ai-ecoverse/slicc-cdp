import assert from 'node:assert/strict';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);

test('live dialog commands answer in-flight Chrome clicks, and refuse pre-existing dialogs', async (t) => {
  const site = await startServer();
  let browser;
  let raw;
  let sessionId;
  let proxy;
  try {
    browser = await launchBrowser();
    raw = await rawSocket(browser.wsUrl);
    const target = await raw.open(`${site.origin}/cb`);
    ({ sessionId } = await raw.send('Target.attachToTarget', { targetId: target, flatten: true }));
    await raw.send('Page.enable', {}, sessionId);
    const runner = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    const cwd = await runner.fresh();
    const value = async (expression) => {
      const result = await runner.playwright(['eval', '--tab', target, expression], cwd);
      assert.equal(result.code, 0, result.stderr);
      return result.stdout.trim();
    };

    for (const [command, text, expression, expected] of [
      ['dialog-accept', 'Lars', "window.answer = prompt('name?')", 'Lars'],
      ['dialog-dismiss', null, "window.answer = confirm('sure?')", 'false'],
      ['dialog-accept', null, "alert('hi'); window.answer = 'alerted'", 'alerted'],
    ]) {
      await t.test(command + ':' + expected, async () => {
        await raw.send(
          'Runtime.evaluate',
          {
            expression: `document.body.innerHTML='<button>Ask</button>'; document.querySelector('button').onclick=()=>{${expression}}`,
            returnByValue: true,
          },
          sessionId
        );
        const snapshot = await runner.playwright(['snapshot', '--tab', target], cwd);
        assert.equal(snapshot.code, 0, snapshot.stderr);
        const ref = snapshot.stdout.match(/button "Ask".*\[ref=(e\d+)\]/)?.[1];
        assert.ok(ref, snapshot.stdout);
        let ready;
        const armed = new Promise((resolve) => {
          ready = resolve;
        });
        let enabled;
        proxy = await cdpProxy(browser.wsUrl, {
          observe: (direction, message) => {
            if (direction === 'client' && message.method === 'Page.enable') enabled = message.id;
            if (direction === 'browser' && message.id === enabled && message.result) ready();
          },
        });
        const observer = nativeRunner(exe('playwright-cli'), exe('curlwright'), proxy.url);
        const accepted = observer.playwright(
          [command, ...(text ? [text] : []), '--tab', target],
          cwd
        );
        await Promise.race([
          armed,
          accepted.then((result) => {
            throw new Error(`dialog observer stopped before arming: ${result.stderr}`);
          }),
        ]);
        const clicked = runner.playwright(['click', ref, '--tab', target], cwd);
        const [dialog, click] = await Promise.all([accepted, clicked]);
        assert.equal(dialog.code, 0, dialog.stderr);
        assert.equal(click.code, 0, click.stderr);
        assert.equal(
          dialog.stdout,
          command === 'dialog-accept' ? 'Accepted the dialog\n' : 'Dismissed the dialog\n'
        );
        assert.equal(await value('String(window.answer)'), expected);
        await proxy.close();
        proxy = undefined;
      });
    }

    await t.test('an already-open dialog needs the foreground holder follow-up', async () => {
      const opening = raw.event('Page.javascriptDialogOpening', sessionId);
      const pending = raw.send(
        'Runtime.evaluate',
        { expression: "window.answer = confirm('already open?')", returnByValue: true },
        sessionId
      );
      await opening.seen;
      const result = await runner.playwright(['dialog-dismiss', '--tab', target], cwd);
      assert.equal(result.code, 1);
      assert.equal(result.stdout, '');
      assert.match(result.stderr, /already open before attachment/);
      assert.match(result.stderr, /foreground dialog holder.*#529/);
      await raw.send('Page.handleJavaScriptDialog', { accept: false }, sessionId);
      await pending;
    });

    await t.test('no observed dialog exits nonzero without guessing ownership', async () => {
      const result = await runner.playwright(['dialog-accept', '--tab', target], cwd);
      assert.equal(result.code, 1);
      assert.match(result.stderr, /No dialog visible to this session/);
    });
  } finally {
    if (raw) {
      await raw.send('Page.handleJavaScriptDialog', { accept: false }, sessionId).catch(() => {});
      raw.close();
    }
    await proxy?.close();
    await browser?.close();
    await site.close();
  }
});
