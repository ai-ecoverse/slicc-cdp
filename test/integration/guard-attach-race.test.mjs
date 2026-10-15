import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const guardTimeoutMs = 10000;

test('the guard attach race through a delaying CDP proxy', async () => {
  let browser;
  const proxies = [];
  try {
    browser = await launchBrowser();
    const pass = await cdpProxy(browser.wsUrl);
    const delay = await cdpProxy(browser.wsUrl, { delayGuardAttachMs: guardTimeoutMs + 1000 });
    const noClose = await cdpProxy(browser.wsUrl, {
      delayGuardAttachMs: guardTimeoutMs + 1000,
      failCloseTarget: true,
    });
    proxies.push(pass, delay, noClose);
    const result = await cargo(
      [
        'test',
        '-p',
        'slicc-cdp-client',
        '--lib',
        '--',
        '--ignored',
        'race::live_',
        '--test-threads=1',
      ],
      {
        SLICC_RACE_PASS_WS: pass.url,
        SLICC_RACE_DELAY_WS: delay.url,
        SLICC_RACE_NOCLOSE_WS: noClose.url,
      }
    );
    assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
    assert.match(result.stdout, /3 passed/);
  } finally {
    for (const proxy of proxies) await proxy.close();
    await browser?.close();
  }
});

function cargo(args, extra) {
  return new Promise((resolve, reject) => {
    const env = { ...process.env, ...extra };
    delete env.SLICC_CDP_URL;
    const child = spawn('cargo', args, { cwd: root, env, detached: process.platform !== 'win32' });
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
