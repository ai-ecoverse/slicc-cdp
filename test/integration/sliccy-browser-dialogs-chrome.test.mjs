import assert from 'node:assert/strict';
import browser from '../../packages/sliccy-browser/src/browser.js';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(check, label) {
  const deadline = Date.now() + 10000;
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await wait(50);
  }
}

test('sliccy:browser answers the dialogs a background tab opens between two evals', async () => {
  const site = await startServer();
  const chrome = await launchBrowser();
  const previous = process.env.SLICC_CDP_URL;
  process.env.SLICC_CDP_URL = chrome.wsUrl;
  const lines = [];
  const warn = console.warn;
  console.warn = (...args) => lines.push(args.join(' '));
  let raw;
  try {
    const tab = await browser.ensureTab(`${site.origin}/page`);
    assert.equal(
      await browser.eval(tab, () => {
        setTimeout(() => {
          window.kept = confirm('keep going?');
          alert('Actual End Date cannot be in Future');
        }, 300);
        return true;
      }),
      true
    );
    await until(() => browser.dialogs(tab).length === 2, 'the two dialogs');
    assert.equal(await browser.eval(tab, () => document.title), 'Fixture');
    assert.equal(await browser.eval(tab, () => window.kept), false);
    assert.deepEqual(
      browser.dialogs(tab).map(({ type, message, accepted, by, url }) => ({
        type,
        message,
        accepted,
        by,
        url,
      })),
      [
        {
          type: 'confirm',
          message: 'keep going?',
          accepted: false,
          by: 'default',
          url: `${site.origin}/page`,
        },
        {
          type: 'alert',
          message: 'Actual End Date cannot be in Future',
          accepted: true,
          by: 'default',
          url: `${site.origin}/page`,
        },
      ]
    );
    assert.equal(lines.length, 2, lines.join('\n'));
    assert.match(lines[1], /alert dialog nobody answered, so it was accepted/);

    const handle = await browser.onDialog(tab, (dialog) => dialog.accept('Lars'));
    await browser.eval(tab, () => {
      setTimeout(() => {
        window.named = prompt('name?', 'nobody');
      }, 200);
    });
    await until(() => browser.dialogs(tab).length === 3, 'the handled prompt');
    assert.equal(await browser.eval(tab, () => window.named), 'Lars');
    assert.equal(browser.dialogs(tab)[2].by, 'handler');
    await handle.close();

    await browser.eval(tab, () => {
      const frame = document.createElement('iframe');
      frame.srcdoc = '<script>setTimeout(() => alert("FRAME-SECRET"), 200)</script>';
      document.body.append(frame);
    });
    await until(() => browser.dialogs(tab).some((record) => record.refused), 'the refusal');
    await assert.rejects(
      browser.eval(tab, () => 1),
      /may not answer/
    );
    assert.ok(!JSON.stringify(browser.dialogs(tab)).includes('FRAME-SECRET'));
    assert.ok(!lines.join('\n').includes('FRAME-SECRET'), lines.join('\n'));
    raw = await rawSocket(chrome.wsUrl);
    await raw.send('Target.closeTarget', { targetId: tab.targetId });
  } finally {
    console.warn = warn;
    raw?.close();
    if (previous === undefined) delete process.env.SLICC_CDP_URL;
    else process.env.SLICC_CDP_URL = previous;
    await chrome.close();
    await site.close();
  }
});
