import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { startServer } from './server.mjs';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

test('the cdp-client Fetch API against chromium', async () => {
  const site = await startServer();
  let browser;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const result = await cargo(
      [
        'test',
        '-p',
        'slicc-cdp-client',
        '--lib',
        '--',
        '--ignored',
        '--exact',
        'fetch::tests::live_chromium_interception',
        'shield::tests::live_protected_targets',
        '--test-threads=1',
      ],
      {
        SLICC_FETCH_LIVE_WS: browser.wsUrl,
        SLICC_FETCH_LIVE_APP: site.origin,
        SLICC_FETCH_LIVE_IDP: `http://localhost:${site.port}`,
        SLICC_FETCH_LIVE_SEVEN: 'http://seven.sliccy.ai',
        SLICC_FETCH_LIVE_VAULT: 'http://vault.sliccy.ai',
      }
    );
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(result.stdout, /2 passed/);
  } finally {
    await browser?.close();
    await site.close();
  }
});

test('interception that turns active after Fetch.enable answers is waited for', async () => {
  await throughProxy({ early: [{ method: 'Fetch.enable', match: '/api/headers', ms: 1500 }] });
});

test('interception waits until the browser has committed a navigation the renderer already reported', async () => {
  await throughProxy({
    before: [
      {
        method: 'Page.navigate',
        match: '/js-redirect',
        send: { method: 'Emulation.setCPUThrottlingRate', params: { rate: 50 } },
      },
    ],
  });
});

test('a guard armed while the browser has not committed a navigation still fails seven', async () => {
  const proxy = await throughProxy(
    {
      before: [
        {
          method: 'Fetch.enable',
          match: '"resourceType":"Document"',
          send: { method: 'Page.getNavigationHistory' },
        },
      ],
    },
    'shield::tests::live_guard_armed_while_a_navigation_commits'
  );
  assert.equal(proxy.replies.length, 1);
  assert.match(JSON.stringify(proxy.replies[0]), /Not attached to an active page/);
});

async function throughProxy(options, name = 'fetch::tests::live_chromium_interception') {
  const site = await startServer();
  let browser;
  let proxy;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    proxy = await cdpProxy(browser.wsUrl, options);
    const result = await cargo(
      [
        'test',
        '-p',
        'slicc-cdp-client',
        '--lib',
        '--',
        '--ignored',
        '--exact',
        name,
        '--test-threads=1',
      ],
      {
        SLICC_FETCH_LIVE_WS: proxy.url,
        SLICC_FETCH_LIVE_APP: site.origin,
        SLICC_FETCH_LIVE_IDP: `http://localhost:${site.port}`,
        SLICC_FETCH_LIVE_SEVEN: 'http://seven.sliccy.ai',
        SLICC_FETCH_LIVE_VAULT: 'http://vault.sliccy.ai',
      }
    );
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(result.stdout, /1 passed/);
    return proxy;
  } finally {
    await proxy?.close();
    await browser?.close();
    await site.close();
  }
}

function cargo(args, extra) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, ...extra };
    delete env.SLICC_CDP_URL;
    const child = spawn('cargo', args, {
      cwd: root,
      env,
      detached: process.platform !== 'win32',
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      killTree(child);
      reject(new Error('cargo test timed out'));
    }, 600000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}
