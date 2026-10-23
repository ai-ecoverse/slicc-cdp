import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { before } from 'node:test';
import { fixtureServer } from '@ai-ecoverse/slicc-browsers/fixture';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { requireRedirectRegistration } from './guard-c2-support.mjs';
import { browserArgs, origins } from './guard-fixture.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const binary = (name) =>
  path.join(root, 'target/debug', `${name}${process.platform === 'win32' ? '.exe' : ''}`);
const wasm = (name) => path.join(root, 'target/wasm32-wasip1/debug', `${name}.wasm`);
const privateMarker = 'CLI_REDIRECT_TERMINAL_PRIVATE_93B7';
const control = 'CLI_REDIRECT_TERMINAL_ORDINARY_6E2A';
const limit = 20000;

before(() => {
  for (const target of [[], ['--target', 'wasm32-wasip1']])
    execFileSync('cargo', ['build', '--bin', 'playwright-cli', '--bin', 'curlwright', ...target], {
      stdio: 'inherit',
    });
});

for (const transport of ['native', 'wasi']) {
  test(`playwright-cli redirect terminal navigation through ${transport}`, {
    timeout: 120000,
  }, async (t) => {
    const fixture = requireRedirectRegistration(await fixtureServer());
    const targets = origins(fixture);
    const rows = [];
    let browser;
    let runner;
    let raw;
    try {
      browser = await launchBrowser(browserArgs());
      raw = await rawSocket(browser.wsUrl);
      runner =
        transport === 'native'
          ? nativeRunner(binary('playwright-cli'), binary('curlwright'), browser.wsUrl)
          : await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
      const protectedURL = `${targets.seven}/auth/callback?code=${privateMarker}#access_token=${privateMarker}`;
      for (const recovery of [true, false]) {
        await t.test(
          recovery
            ? 'ordinary → protected refusal → ordinary recovery on the same page'
            : 'ordinary → protected fails cleanly before timeout',
          async () => {
            const cwd = await runner.fresh();
            const start = `${targets.ordinary}/doc?n=${control}`;
            const hop = `/cli-terminal-${recovery ? 'recover' : 'refuse'}`;
            await fixture.redirectSecret({
              path: hop,
              to: protectedURL,
              code: 302,
              body: privateMarker,
            });
            const opened = await runner.playwright(['open', start], cwd);
            assert.equal(opened.code, 0, `${opened.stdout}\n${opened.stderr}`);
            const listed = await raw.send('Target.getTargets');
            const target = listed.targetInfos.find((info) => info.url === start);
            assert.ok(target, 'ordinary CLI page was not created');
            const started = performance.now();
            let timer;
            let refused;
            try {
              refused = await Promise.race([
                runner.playwright(
                  ['goto', `${targets.ordinary}${hop}`, '--tab', target.targetId],
                  cwd
                ),
                new Promise((_, reject) => {
                  timer = setTimeout(
                    () =>
                      reject(new Error('protected redirect goto exceeded terminal-event deadline')),
                    limit
                  );
                }),
              ]);
            } finally {
              clearTimeout(timer);
            }
            const elapsed = performance.now() - started;
            const output = `${refused.stdout}\n${refused.stderr}`;
            assert.notEqual(refused.code, 0, output);
            assert.match(output, /slicc: refused:|net::ERR_BLOCKED_BY_CLIENT|net::ERR_ABORTED/);
            assert.doesNotMatch(output, /timed out|timeout/i);
            assert.ok(elapsed < limit, `protected redirect took ${elapsed}ms`);
            assert.ok(
              !output.includes(privateMarker),
              'protected URL or body leaked through CLI output'
            );
            const row = {
              transport,
              recovery,
              refused: true,
              elapsedMs: Math.round(elapsed),
              ordinaryControl: false,
            };
            if (recovery) {
              const destination = `${targets.ordinary}/doc?n=${control}-recovered`;
              const result = await runner.playwright(
                ['goto', destination, '--tab', target.targetId],
                cwd
              );
              assert.equal(result.code, 0, `${result.stdout}\n${result.stderr}`);
              const current = await raw.send('Target.getTargetInfo', { targetId: target.targetId });
              assert.equal(current.targetInfo.url, destination);
              const body = await runner.playwright(
                ['eval', 'document.body.textContent', '--tab', target.targetId],
                cwd
              );
              assert.equal(body.code, 0, body.stderr);
              assert.ok(
                body.stdout.includes(control),
                'ordinary positive control was not readable after recovery'
              );
              row.ordinaryControl = true;
            } else {
              row.ordinaryControl = opened.code === 0;
            }
            rows.push(row);
            console.log(`CLI-REDIRECT-TERMINAL ${JSON.stringify(row)}`);
            await raw.send('Target.closeTarget', { targetId: target.targetId });
          }
        );
      }
      assert.equal(rows.length, 2, 'native/WASI navigation row did not complete');
    } finally {
      const prefix =
        process.env.SLICC_GUARD_REPORT ?? path.join(tmpdir(), 'slicc-cdp-guard-runner-report.md');
      await writeFile(
        `${prefix}.cli-redirect-${transport}.json`,
        JSON.stringify({ transport, rows }, null, 2)
      );
      raw?.close();
      await runner?.close();
      await browser?.close();
      await fixture.close();
    }
  });
}
