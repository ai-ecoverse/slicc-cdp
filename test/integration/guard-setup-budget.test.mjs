import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { runProcess } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const playwright = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const unguarded = "slicc: refused: slicc can't keep seven's origin out of this target";

async function pages(wsUrl) {
  const listed = await fetch(`http://${new URL(wsUrl).host}/json/list`);
  return (await listed.json()).filter((info) => info.type === 'page').map((info) => info.id);
}

async function drive(proxy, site) {
  const cwd = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-budget-'));
  const cli = (...args) => runProcess(playwright, ['--cdp', proxy.url, ...args], cwd, 120000);
  try {
    const opened = await cli('open', `${site.origin}/one`);
    if (opened.code !== 0) return { opened };
    const snapshot = await cli('snapshot');
    return { opened, snapshot };
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
}

test('the guard setup shares one budget, tolerates slow optional steps and fails closed', async (t) => {
  const site = await startServer();
  const browser = await launchBrowser();
  try {
    const flows = {
      'a 12 s Page.enable is optional and the tab still works': {
        delays: { 'Page.enable': { ms: 12000 } },
      },
      'a 12 s Network.enable fits in the shared budget': {
        delays: { 'Network.enable': { ms: 12000 } },
      },
      'a 12 s Target.setDiscoverTargets fits in the shared budget': {
        delays: { 'Target.setDiscoverTargets': { ms: 12000 } },
      },
      'browser-level events never arrive and the tab still works': { browserEvents: false },
    };
    for (const [name, options] of Object.entries(flows)) {
      await t.test(name, async () => {
        const proxy = await cdpProxy(browser.wsUrl, options);
        try {
          const { opened, snapshot } = await drive(proxy, site);
          assert.equal(opened.code, 0, `${opened.stdout}\n${opened.stderr}`);
          assert.equal(snapshot.code, 0, `${snapshot.stdout}\n${snapshot.stderr}`);
          assert.match(snapshot.stdout, new RegExp(`Page URL: ${site.origin}/one`));
        } finally {
          await proxy.close();
        }
      });
    }
    await t.test(
      'a required step past the shared budget is refused, named, and closed',
      async () => {
        const before = await pages(browser.wsUrl);
        const proxy = await cdpProxy(browser.wsUrl, {
          delays: {
            'Fetch.enable': { ms: 12000 },
            'Network.enable': { ms: 12000 },
            'Network.setBypassServiceWorker': { ms: 12000 },
          },
        });
        try {
          const { opened, snapshot } = await drive(proxy, site);
          assert.equal(snapshot, undefined);
          assert.notEqual(opened.code, 0);
          assert.equal(opened.stdout, '');
          assert.equal(
            opened.stderr,
            `${unguarded}: Network.setBypassServiceWorker timed out\n`,
            opened.stderr
          );
          const after = await pages(browser.wsUrl);
          assert.deepEqual(
            after.filter((id) => !before.includes(id)),
            [],
            'the refused tab was left open'
          );
        } finally {
          await proxy.close();
        }
      }
    );
  } finally {
    await browser.close();
    await site.close();
  }
});
