import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { fakeCdp } from './fake-cdp.mjs';

const execute = promisify(execFile);
const cli = path.resolve(
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);

for (const failed of ['http://ordinary.invalid/', 'https://seven.sliccy.ai']) {
  test(`implicit main snapshots retain the guarded error-page decision for ${failed}`, async () => {
    const location = { href: 'https://example.test/', title: 'Fixture' };
    const backend = await fakeCdp({ location });
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-error-snapshot-'));
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      APPDATA: home,
      XDG_CONFIG_HOME: home,
      XDG_STATE_HOME: home,
      SLICC_CDP_PORTS: '9300-9309',
    };
    delete env.SLICC_CDP_URL;
    delete env.PLAYWRIGHT_CLI_SESSION;
    const run = (args) =>
      execute(cli, ['--cdp', backend.url, ...args], { cwd: home, env, timeout: 30000 });
    try {
      await run(['open', location.href]);
      const target = backend.targets[0];
      target.url = failed;
      target.frame = {
        id: target.targetId,
        url: 'chrome-error://chromewebdata/',
        securityOrigin: '://',
        unreachableUrl: failed,
      };
      target.history = {
        currentIndex: 1,
        entries: [location.href, failed].map((url, index) => ({
          id: index + 1,
          url,
          userTypedURL: url,
          transitionType: 'typed',
        })),
      };
      location.href = failed;
      const before = backend.calls.length;
      if (failed.startsWith('http://ordinary.')) {
        const shot = await run(['snapshot']);
        assert.match(shot.stdout, /button "Go"/);
        const captured = backend.calls
          .slice(before)
          .filter(
            (call) =>
              call.method === 'Runtime.evaluate' &&
              call.params.expression?.includes('sliccAriaSnapshot(')
          );
        assert.equal(captured.length, 1);
        assert.equal(captured[0].params.contextId, undefined);
        assert.equal(
          backend.calls.slice(before).some((call) => call.method === 'Page.createIsolatedWorld'),
          false
        );
        await assert.rejects(run(['snapshot', `--frame=${target.targetId}`]), /slicc: refused:/);
      } else {
        await assert.rejects(run(['snapshot']), /slicc: refused:/);
      }
      await run(['goto', 'https://example.test/recovered']);
    } finally {
      await backend.close();
      await rm(home, { recursive: true, force: true });
    }
  });
}
