import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { evalCommand as command, evalHolder as holder } from './eval-support.mjs';
import './watchdog.mjs';

for (const world of ['main', 'isolated']) {
  test(`eval-attach Chrome ${world}: installs on navigation and new matching tabs, sends and detaches`, {
    timeout: 180000,
  }, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-chrome-'));
    const server = createServer((req, res) => {
      res.setHeader('Content-Type', 'text/html');
      res.end(
        `<!doctype html><title>eval fixture</title><body>${req.url}${req.url === '/child' ? '' : '<iframe src="/child"></iframe>'}`
      );
    });
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    let browser;
    let raw;
    let held;
    try {
      browser = await launchBrowser([
        '--host-resolver-rules=MAP vault.sliccy.ai 127.0.0.1',
        '--no-proxy-server',
      ]);
      raw = await rawSocket(browser.wsUrl);
      const open = async (url) => {
        const { targetId } = await raw.send('Target.createTarget', {
          url: 'about:blank',
          background: true,
        });
        const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
        await raw.send('Page.enable', {}, sessionId);
        const loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url }, sessionId);
        await loaded.seen;
        return { tab: targetId, sessionId };
      };
      const { tab, sessionId } = await open(`${origin}/allowed/one`);
      const { tab: miss } = await open(`${origin}/miss`);
      await writeFile(
        path.join(home, 'shim.js'),
        'globalThis.evalAttachCount = (globalThis.evalAttachCount || 0) + 1; send({href: location.href, top: window === top, count: globalThis.evalAttachCount}); setTimeout(() => send({later: true}), 50);'
      );
      const flags =
        world === 'main'
          ? ['--host', '127.0.0.1']
          : ['--match', '*://127.0.0.1/allowed/*', '--match', 'https://other.test/*'];
      held = holder(browser.wsUrl, ['eval-attach', 'shim.js', ...flags, '--world', world], home);
      const ready = await held.event('ready');
      const initial = await held.event('document-ready', (event) => event.tab === tab);
      assert.equal(initial.early, false);
      assert.equal(initial.world, world);
      const message = await held.event('message', (event) => event.tab === tab);
      assert.deepEqual(message.payload, { href: `${origin}/allowed/one`, top: true, count: 1 });
      await held.event('message', (event) => event.tab === tab && event.payload.later === true);
      const globals = await raw.send(
        'Runtime.evaluate',
        {
          expression:
            '({count: globalThis.evalAttachCount, queues: Object.keys(globalThis).filter(k => k.startsWith("__slicc_eval_"))})',
          returnByValue: true,
        },
        sessionId
      );
      assert.equal(globals.result.value.queues.length, world === 'main' ? 1 : 0);
      assert.equal(globals.result.value.count, world === 'main' ? 1 : undefined);
      const loaded = raw.event('Page.loadEventFired', sessionId);
      await raw.send('Page.navigate', { url: `${origin}/allowed/two` }, sessionId);
      await loaded.seen;
      const second = await held.event(
        'document-ready',
        (event) => event.tab === tab && event.loaderId !== initial.loaderId
      );
      assert.equal(second.url, `${origin}/allowed/two`);
      assert.equal(
        (await held.event('message', (event) => event.loaderId === second.loaderId)).payload.count,
        1
      );
      const { tab: next } = await open(`${origin}/allowed/new`);
      await held.event('message', (event) => event.tab === next);
      if (world === 'isolated') assert.ok(!held.events().some((event) => event.tab === miss));
      const detached = await command(browser.wsUrl, ['eval-detach', ready.attachmentId], home);
      assert.equal(detached.code, 0, detached.stderr);
      assert.equal((await held.ended).code, 0);
      assert.equal(held.events().at(-1).reason, 'detach');
      const stoppedCount = held.events().length;
      await raw.send('Page.navigate', { url: `${origin}/allowed/after` }, sessionId);
      assert.equal(held.events().length, stoppedCount);
      held = holder(
        browser.wsUrl,
        ['eval-attach', 'shim.js', '--tab', tab, '--world', world],
        home
      );
      await held.event('document-ready');
      await raw.send(
        'Page.navigate',
        { url: `http://vault.sliccy.ai:${server.address().port}/SECRET?q=SECRET` },
        sessionId
      );
      const refused = await held.event('refused');
      assert.equal(refused.url, undefined);
      assert.ok(!JSON.stringify(refused).includes('SECRET'));
      held.child.stdin.end();
      assert.equal((await held.ended).code, 0);
      assert.equal(held.events().at(-1).reason, 'stdin');
    } finally {
      held?.child.kill('SIGKILL');
      raw?.close();
      await browser?.close();
      await new Promise((resolve) => server.close(resolve));
      await rm(home, { recursive: true, force: true });
    }
  });
}
