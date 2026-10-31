import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const SEVEN = "slicc: refused: target is on seven's origin\n";
const UNVERIFIED =
  'slicc: refused: slicc could not finish checking this target, so it left it alone; the browser may not answer the check\n';

async function withFake(options, body) {
  const fake = await fakeCdp(options);
  const pw = nativeRunner(exe('playwright-cli'), exe('curlwright'), fake.url);
  try {
    const cwd = await pw.fresh();
    await body((args) => pw.playwright(args, cwd), fake);
  } finally {
    await pw.close();
    await fake.close();
  }
}

test('a check the browser never answers is not called seven', async () => {
  await withFake({ refuse: ['Page.getFrameTree'] }, async (run, fake) => {
    fake.targets.push({ targetId: 'TAB', url: 'http://example.test/' });
    const result = await run(['eval', '1']);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, UNVERIFIED);
  });
});

test('a page on seven is still called seven', async () => {
  await withFake({}, async (run, fake) => {
    fake.targets.push({ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/' });
    const result = await run(['eval', '1', '--tab', 'SEVEN']);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, SEVEN);
  });
});

test('a page that stops answering is named, not blamed on seven', async () => {
  const silent = (message) => message.method === 'Network.setBypassServiceWorker';
  await withFake({ intercept: silent }, async (run, fake) => {
    fake.targets.push({ targetId: 'TAB', url: 'http://example.test/' });
    const result = await run(['eval', '1']);
    assert.equal(result.code, 1);
    assert.match(result.stderr, /the page isn't answering/);
    assert.match(
      result.stderr,
      /^slicc: refused: slicc can't keep seven's origin out of this target/
    );
    assert.match(result.stderr, /Network\.setBypassServiceWorker timed out\n$/);
  });
});

test('a failed check reads the same whether or not a protected tab is open', async () => {
  const read = async (extra) => {
    let text;
    await withFake({ refuse: ['Page.getFrameTree'] }, async (run, fake) => {
      fake.targets.push({ targetId: 'TAB', url: 'http://example.test/' });
      if (extra) fake.targets.push({ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/' });
      const listed = await run(['tab-list']);
      assert.ok(!listed.stdout.includes('seven.sliccy.ai'));
      text = (await run(['eval', '1', '--tab', 'TAB'])).stderr;
    });
    return text;
  };
  assert.equal(await read(true), await read(false));
});
