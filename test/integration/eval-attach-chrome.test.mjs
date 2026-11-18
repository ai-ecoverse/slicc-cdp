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
  for (const scope of ['tab', 'host', 'match']) {
    test(`eval-attach early Chrome ${world} ${scope}: finishes before inline head and refuses protected redirects`, {
      timeout: 180000,
    }, async () => {
      const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-early-'));
      const server = createServer((req, res) => {
        if (req.url === '/redirect') {
          res.writeHead(302, {
            location: `http://vault.sliccy.ai:${server.address().port}/secret`,
          });
          res.end();
          return;
        }
        if (req.url === '/static') {
          res.setHeader('Content-Type', 'text/html');
          res.end('<!doctype html><p>no scripts</p>');
          return;
        }
        res.setHeader('Content-Type', 'text/html');
        res.end(
          '<!doctype html><html><head><script>document.documentElement.dataset.observed = document.documentElement.dataset.attached || "missing"; globalThis.headRan = true;</script></head><body>early fixture</body></html>'
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
        const { targetId: tab } = await raw.send('Target.createTarget', {
          url: 'about:blank',
          background: true,
        });
        const { sessionId } = await raw.send('Target.attachToTarget', {
          targetId: tab,
          flatten: true,
        });
        await raw.send('Page.enable', {}, sessionId);
        let loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url: `${origin}/initial` }, sessionId).catch((error) => {
          throw new Error(`initial navigation: ${error.message}`);
        });
        await loaded.seen;
        await writeFile(
          path.join(home, 'shim.js'),
          'document.documentElement.dataset.attached = "installed"; globalThis.earlyMarker = true; send({early: true}); setTimeout(() => send({later: true}), 10);'
        );
        const flags =
          scope === 'tab'
            ? ['--tab', tab]
            : scope === 'host'
              ? ['--host', '127.0.0.1']
              : ['--match', '*://127.0.0.1/*'];
        held = holder(
          browser.wsUrl,
          ['eval-attach', 'shim.js', ...flags, '--world', world, '--early'],
          home
        );
        assert.equal((await held.event('ready')).early, true);
        assert.equal((await held.event('refused')).reason, 'late');
        const inspect = async () =>
          (
            await raw.send(
              'Runtime.evaluate',
              {
                expression:
                  '({observed: document.documentElement.dataset.observed, attached: document.documentElement.dataset.attached, marker: !!globalThis.earlyMarker})',
                returnByValue: true,
              },
              sessionId
            )
          ).result.value;
        assert.deepEqual(await inspect(), { observed: 'missing', marker: false });
        loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url: `${origin}/next` }, sessionId).catch((error) => {
          throw new Error(`next navigation: ${error.message} ${JSON.stringify(held.output())}`);
        });
        await loaded.seen;
        const installed = await held.event('document-ready');
        assert.equal(installed.early, true);
        assert.equal(installed.world, world);
        assert.equal(installed.url, `${origin}/next`);
        assert.deepEqual(await inspect(), {
          observed: 'installed',
          attached: 'installed',
          marker: world === 'main',
        });
        await held.event('message', (event) => event.payload.early === true);
        await held.event('message', (event) => event.payload.later === true);
        loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url: `${origin}/static` }, sessionId);
        await loaded.seen;
        await held.event(
          'refused',
          (event) =>
            event.reason === 'late' &&
            held.events().filter((item) => item.event === 'refused').length === 2
        );
        assert.deepEqual(await inspect(), { marker: false }, JSON.stringify(held.events()));
        const before = held.events().filter((event) => event.event === 'refused').length;
        loaded = raw.event('Page.loadEventFired', sessionId);
        await raw.send('Page.navigate', { url: `${origin}/redirect` }, sessionId).catch((error) => {
          throw new Error(`redirect navigation: ${error.message} ${JSON.stringify(held.output())}`);
        });
        await loaded.seen;
        await held.event('refused', (event) => event.reason === 'guard');
        assert.equal((await inspect()).marker, false);
        assert.equal((await inspect()).attached, undefined);
        const protectedFrame = (await raw.send('Page.getFrameTree', {}, sessionId)).frameTree.frame;
        assert.ok(
          [protectedFrame.url, protectedFrame.unreachableUrl].some((url) =>
            /^https?:\/\/vault\.sliccy\.ai:/.test(url ?? '')
          )
        );
        assert.equal(held.events().filter((event) => event.event === 'document-ready').length, 1);
        assert.ok(held.events().filter((event) => event.event === 'refused').length > before);
        held.child.stdin.end();
        assert.equal((await held.ended).code, 0);
      } finally {
        held?.child.kill('SIGKILL');
        raw?.close();
        await browser?.close();
        await new Promise((resolve) => server.close(resolve));
        await rm(home, { recursive: true, force: true });
      }
    });
  }
}

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
