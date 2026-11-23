import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const SEVEN = "slicc: refused: target is on seven's origin\n";
const UNVERIFIED =
  'slicc: refused: slicc could not finish checking this target, so it left it alone; the browser may not answer the check\n';

const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const runners = {
  native: (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};

async function withFake(options, body, runner = runners.native) {
  const fake = await fakeCdp(options);
  const pw = await runner(fake.url);
  try {
    const cwd = await pw.fresh();
    await body((args) => pw.playwright(args, cwd), fake);
  } finally {
    await pw.close();
    await fake.close();
  }
}

const url = 'https://ordinary.test/article';
const deniedTrace = {
  refuse: ['Network.getAllCookies'],
  refusalMessage: 'slicc: refused: trace read',
  traffic: [{ url: 'https://ordinary.test/unsaved' }],
};

for (const [kind, runner] of Object.entries(runners)) {
  test(`${kind}: tab-new reports the opened tab, not a failed trace read`, async () => {
    await withFake(
      deniedTrace,
      async (run, fake) => {
        const opened = await run(['tab-new', url]);
        assert.equal(opened.code, 0, opened.stderr);
        assert.equal(opened.stdout, `Opened ${url} in new tab [targetId: TAB1]\n`);
        assert.match(opened.stderr, /console and request trace was not saved/);
        assert.equal(fake.targets.length, 1);
        assert.equal(fake.calls.filter(({ method }) => method === 'Page.navigate').length, 1);
        const listed = await run(['tab-list']);
        assert.match(listed.stdout, /TAB1.*\(active\)/);
        const requests = await run(['requests']);
        assert.match(requests.stderr, /No network requests/);
      },
      runner
    );
  });
}

test('open does not mistake an accepted navigation without a document commit for an opened page', async () => {
  const options = { ...deniedTrace, answers: { 'Page.navigate': { frameId: 'TAB1' } } };
  await withFake(options, async (run) => {
    const opened = await run(['open', url]);
    assert.equal(opened.code, 1);
    assert.match(opened.stderr, /slicc: refused: trace read/);
    assert.equal(opened.stdout, '');
  });
});

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
      const result = await run(['eval', '1', '--tab', 'TAB']);
      assert.equal(result.code, 1);
      text = result.stderr;
    });
    return text;
  };
  assert.equal(await read(true), UNVERIFIED);
  assert.equal(await read(false), UNVERIFIED);
});
