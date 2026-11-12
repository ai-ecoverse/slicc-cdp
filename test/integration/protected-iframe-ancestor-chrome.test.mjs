import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const exe = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);

function site() {
  let port = 0;
  const pages = {
    '/os/': () => `<!doctype html><title>seven</title>
<iframe id="sprinkle" sandbox="allow-scripts" srcdoc="<p id='sprinkle'>srcdoc-secret</p>"></iframe>
<iframe id="cross" src="http://foreign.test:${port}/cross"></iframe>`,
    '/cross': () => '<!doctype html><title>cross</title><p id="cross">cross-secret</p>',
  };
  const server = createServer((req, res) => {
    const page = pages[new URL(req.url ?? '/', 'http://127.0.0.1').pathname];
    res.writeHead(page ? 200 : 404, { 'content-type': 'text/html' });
    res.end(page ? page() : 'missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      port = server.address().port;
      resolve({
        port,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

async function settle(value) {
  try {
    await value;
  } catch {
    return undefined;
  }
}

async function until(read, accept, label) {
  const deadline = Date.now() + 20000;
  do {
    const value = await read();
    if (accept(value)) return value;
    await new Promise((resolve) => setTimeout(resolve, 100));
  } while (Date.now() < deadline);
  throw new Error(label);
}

test('Chrome: protected-page srcdoc and cross-site OOPIF children stay unlistable and unattachable', {
  timeout: 180000,
}, async () => {
  const server = await site();
  let browser;
  let raw;
  let runner;
  let cwd;
  try {
    browser = await launchBrowser([
      '--site-per-process',
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${server.port},MAP foreign.test 127.0.0.1:${server.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await rawSocket(browser.wsUrl);
    runner = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    cwd = await runner.fresh();
    await runner.playwright(['open', 'about:blank'], cwd);
    const parentUrl = `http://seven.sliccy.ai:${server.port}/os/`;
    const parentId = await raw.open(parentUrl);
    const parent = await until(
      async () =>
        (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.find(
          (target) =>
            target.targetId === parentId &&
            target.type === 'page' &&
            String(target.url || '').includes('seven.sliccy.ai')
        ),
      Boolean,
      'protected parent page'
    );
    const attached = await raw.send('Target.attachToTarget', {
      targetId: parent.targetId,
      flatten: true,
    });
    await raw.send('Page.enable', {}, attached.sessionId);
    const tree = await raw.send('Page.getFrameTree', {}, attached.sessionId);
    const parentFrameId = tree.frameTree.frame.id;
    await raw.send('Target.detachFromTarget', { sessionId: attached.sessionId });
    const children = await until(
      async () => {
        const listed = (await raw.send('Target.getTargets', { filter: [{}] })).targetInfos.filter(
          (target) =>
            target.type === 'iframe' &&
            (target.parentFrameId === parentFrameId || target.parentFrameId === parent.targetId) &&
            (target.url === 'about:srcdoc' || String(target.url || '').includes('foreign.test'))
        );
        return listed.length >= 2 ? listed : null;
      },
      Boolean,
      'raw srcdoc and cross-site OOPIF children'
    );
    const tabs = await runner.playwright(['tab-list'], cwd);
    assert.equal(tabs.code, 0, tabs.stderr);
    for (const child of children) {
      assert.doesNotMatch(tabs.stdout, new RegExp(child.targetId));
      assert.doesNotMatch(
        tabs.stdout + tabs.stderr,
        /srcdoc-secret|cross-secret|foreign\.test|about:srcdoc/
      );
    }
    for (const child of children) {
      const frames = await runner.playwright(['frames', '--tab', child.targetId], cwd);
      assert.notEqual(frames.code, 0, child.url);
      assert.match(frames.stderr, /slicc: refused:/);
      assert.doesNotMatch(frames.stdout + frames.stderr, new RegExp(child.targetId));
      assert.doesNotMatch(frames.stdout + frames.stderr, /srcdoc-secret|cross-secret/);
      const evalResult = await runner.playwright(
        ['eval', '--tab', child.targetId, 'document.documentElement.outerHTML'],
        cwd
      );
      assert.notEqual(evalResult.code, 0, child.url);
      assert.match(evalResult.stderr, /slicc: refused:/);
      assert.doesNotMatch(
        evalResult.stdout + evalResult.stderr,
        /srcdoc-secret|cross-secret|foreign\.test/
      );
      const goto = await runner.playwright(
        ['goto', '--tab', child.targetId, `http://foreign.test:${server.port}/cross`],
        cwd
      );
      assert.notEqual(goto.code, 0, child.url);
      assert.match(goto.stderr, /slicc: refused:/);
      assert.doesNotMatch(goto.stdout + goto.stderr, /srcdoc-secret|cross-secret/);
    }
    const harFile = path.join(await mkdtemp(path.join(tmpdir(), 'slicc-iframe-har-')), 'out.har');
    const har = await runner.playwright(['har-start', '--tab', children[0].targetId, harFile], cwd);
    assert.notEqual(har.code, 0);
    assert.match(har.stderr, /slicc: refused:/);
    assert.doesNotMatch(har.stdout + har.stderr, /srcdoc-secret|cross-secret|foreign\.test/);
  } finally {
    await settle(raw?.close?.());
    await settle(browser?.close());
    await settle(server.close());
  }
});
