import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { copyFileSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { cdpProxy } from './cdp-proxy.mjs';
import { killTree } from './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const guardTimeoutMs = 10000;

test('the guard attach race through a delaying CDP proxy', async (t) => {
  const harness = path.join(root, '.build/guard-race');
  mkdirSync(harness, { recursive: true });
  writeFileSync(
    path.join(harness, 'Cargo.toml'),
    '[package]\nname = "cert-guard-race"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\n\n[dependencies]\ncdp-client = { package = "slicc-cdp-client", path = "../../crates/cdp-client" }\nserde_json = "1"\n\n[[test]]\nname = "guard-race"\npath = "../../test/integration/guard-race.rs"\n'
  );
  copyFileSync(path.join(root, 'Cargo.lock'), path.join(harness, 'Cargo.lock'));
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
    const env = {
      SLICC_RACE_PASS_WS: pass.url,
      SLICC_RACE_DELAY_WS: delay.url,
      SLICC_RACE_NOCLOSE_WS: noClose.url,
    };
    const cases = [
      [
        'guard.attach.caller-session',
        'live_a_guarded_create_or_attach_surfaces_only_the_callers_session',
      ],
      [
        'guard.attach.timeout',
        'live_b_a_timed_out_guard_attach_leaves_no_session_and_its_target_refused',
      ],
      [
        'guard.attach.failed-close',
        'live_c_a_timed_out_guard_attach_with_a_failed_close_stays_refused',
      ],
    ];
    for (const [id, name] of cases)
      await t.test(id, async () => {
        const result = await cargo(
          [
            'test',
            '--manifest-path',
            path.join(harness, 'Cargo.toml'),
            '--test',
            'guard-race',
            '--',
            '--ignored',
            '--exact',
            name,
            '--test-threads=1',
          ],
          env
        );
        assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
        assert.match(result.stdout, /1 passed/);
      });
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
