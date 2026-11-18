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
  test(`eval frames Chrome ${world}: requested OOPIF, nested, calls, navigation and protected ancestors`, {
    timeout: 180000,
  }, async () => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-eval-frames-chrome-'));
    const server = createServer((req, res) => {
      const port = server.address().port;
      res.setHeader('Content-Type', 'text/html');
      res.setHeader('Permissions-Policy', 'tools=*');
      const body =
        req.url === '/top'
          ? `<iframe id="same" src="/same" allow="tools *"></iframe><iframe id="foreign" src="http://foreign.test:${port}/cross" allow="tools *"></iframe><iframe src="http://miss.test:${port}/miss"></iframe>`
          : req.url === '/same'
            ? '<iframe src="/nested" allow="tools *"></iframe>'
            : req.url === '/protected'
              ? `<iframe src="http://foreign.test:${port}/descendant" allow="tools *"></iframe>`
              : '';
      res.end(
        `<!doctype html><html><head><script>globalThis.headRan = true;</script></head><body>${body}</body></html>`
      );
    });
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    const foreign = `http://foreign.test:${server.address().port}`;
    let browser;
    let raw;
    let held;
    try {
      browser = await launchBrowser([
        '--site-per-process',
        '--host-resolver-rules=MAP *.test 127.0.0.1, MAP vault.sliccy.ai 127.0.0.1',
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
      const loaded = raw.event('Page.loadEventFired', sessionId);
      await raw.send('Page.navigate', { url: `${origin}/top` }, sessionId);
      await loaded.seen;
      await writeFile(
        path.join(home, 'shim.js'),
        'globalThis.frameCall = () => { send({called: true}); return location.href; }; send({href: location.href, headRan: !!globalThis.headRan});'
      );
      await writeFile(path.join(home, 'call.js'), '() => globalThis.frameCall()');
      held = holder(
        browser.wsUrl,
        [
          'eval-attach',
          'shim.js',
          '--tab',
          tab,
          '--world',
          world,
          ...(world === 'isolated' ? ['--early'] : []),
          '--frames',
          'same-origin',
          '--frames',
          foreign,
        ],
        home
      );
      const ready = await held.event('ready');
      for (const suffix of ['/same', '/nested', '/cross']) {
        const installed = await held.event('document-ready', (event) => event.url.endsWith(suffix));
        assert.equal(installed.early, false);
        assert.equal(installed.origin, suffix === '/cross' ? foreign : origin);
        assert.equal(installed.ancestors[0].origin, origin);
        assert.equal(installed.ancestors.length, suffix === '/nested' ? 2 : 1);
        assert.equal(installed.allow, 'tools *', `${suffix}: ${held.output().stderr}`);
        assert.equal(installed.srcOrigin, suffix === '/cross' ? foreign : origin);
        const message = await held.event('message', (event) => event.frameId === installed.frameId);
        assert.equal(message.payload.href, installed.url);
        assert.equal(message.payload.headRan, world === 'main');
        const called = await command(
          browser.wsUrl,
          [
            'eval-file',
            'call.js',
            '--tab',
            tab,
            '--frame',
            installed.frameId,
            '--attachment',
            ready.attachmentId,
            '--loader',
            installed.loaderId,
          ],
          home
        );
        assert.equal(called.code, 0, `${suffix}: ${called.stderr}`);
        assert.ok(called.stdout.includes(installed.url), called.stdout);
        await held.event(
          'message',
          (event) => event.frameId === installed.frameId && event.payload.called === true
        );
      }
      const first = await held.event('document-ready', (event) => event.url.endsWith('/cross'));
      const frameFlags = [
        '--tab',
        tab,
        '--frame',
        first.frameId,
        '--attachment',
        ready.attachmentId,
        '--loader',
        first.loaderId,
      ];
      for (const [source, expected] of [
        ['function helper() { return 7; }; helper()', '7'],
        ['function () { return 8; }', '8'],
        ['async function () { return 9; }', '9'],
      ]) {
        const result = await command(browser.wsUrl, ['eval', source, ...frameFlags], home);
        assert.equal(result.code, 0, result.stderr);
        assert.equal(result.stdout.trim(), expected, source);
      }
      await writeFile(
        path.join(home, 'declaration.js'),
        'function helper() { return 10; }; helper()'
      );
      const declaration = await command(
        browser.wsUrl,
        ['eval-file', 'declaration.js', ...frameFlags],
        home
      );
      assert.equal(declaration.code, 0, declaration.stderr);
      assert.equal(declaration.stdout.trim(), '10');
      assert.ok(
        (await raw.send('Target.getTargets')).targetInfos.some(
          (info) => info.type === 'iframe' && info.targetId === first.frameId
        )
      );
      await held.event('refused', (event) => event.reason === 'origin-not-requested');
      await raw.send(
        'Runtime.evaluate',
        {
          expression: `document.querySelector('#foreign').src = ${JSON.stringify(`${foreign}/next`)}`,
        },
        sessionId
      );
      const next = await held.event('document-ready', (event) => event.url === `${foreign}/next`);
      assert.notEqual(next.loaderId, first.loaderId);
      assert.equal(next.early, false);
      assert.equal(next.permissionsPolicy, 'tools=*');
      const stale = await command(
        browser.wsUrl,
        [
          'eval',
          '() => globalThis.frameCall()',
          '--tab',
          tab,
          '--frame',
          first.frameId,
          '--attachment',
          ready.attachmentId,
          '--loader',
          first.loaderId,
        ],
        home
      );
      assert.equal(stale.code, 1);
      await raw.send(
        'Runtime.evaluate',
        {
          expression: `document.querySelector('#foreign').src = ${JSON.stringify(`http://vault.sliccy.ai:${server.address().port}/protected`)}`,
        },
        sessionId
      );
      await held.event('refused', (event) => ['guard', 'relay'].includes(event.reason));
      assert.ok(
        !held
          .events()
          .some(
            (event) => event.event === 'document-ready' && /protected|descendant/.test(event.url)
          )
      );
      const recovered = raw.event('Page.loadEventFired', sessionId);
      await raw.send('Page.navigate', { url: `${origin}/recovered` }, sessionId);
      await recovered.seen;
      await held.event('document-ready', (event) => event.url === `${origin}/recovered`);
      assert.equal(held.events().filter((event) => event.event === 'ready').length, 1);
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
