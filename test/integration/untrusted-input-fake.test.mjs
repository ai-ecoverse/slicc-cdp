import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { mousePage } from './mouse-page.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const runners = {
  native: async (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: async (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};
const undelivered = (verb) =>
  `Error: ${verb} did not reach the page: this browser can't make trusted input, and the page didn't take the events slicc dispatched itself.\n`;

function page(overrides = {}) {
  const answers = {
    arm: 'untrusted',
    visible: 'untrusted',
    focus: 'none',
    pointer: 'done',
    mouse: 'done',
    key: 'pressed',
    ...overrides,
  };
  return (mode) => answers[mode] ?? 'done';
}

const unknownLocation = (message, send) => {
  if (message.method === 'Page.getLayoutMetrics') {
    const result = {
      cssLayoutViewport: { pageX: 0, pageY: 0, clientWidth: 800, clientHeight: 600 },
    };
    send({ id: message.id, sessionId: message.sessionId, result });
    return true;
  }
  if (message.method !== 'DOM.getNodeForLocation') return false;
  send({
    id: message.id,
    sessionId: message.sessionId,
    error: { code: -32601, message: "'DOM.getNodeForLocation' wasn't found" },
  });
  return true;
};

async function withPage(runner, overrides, body) {
  const fake = await fakeCdp({ landing: page(overrides), intercept: unknownLocation });
  const pw = await runner(fake.url);
  try {
    const cwd = await pw.fresh();
    const run = (args) => pw.playwright(args, cwd);
    assert.equal((await run(['open', 'http://example.test/page'])).code, 0);
    const shot = await run(['snapshot']);
    const ref = shot.stdout.match(/button "Go" \[ref=(e\d+)\]/)?.[1];
    assert.ok(ref, shot.stdout);
    await body({ fake, run, ref });
  } finally {
    await pw.close();
    await fake.close();
  }
}

const input = (fake) => fake.calls.filter((call) => call.method.startsWith('Input.'));

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: a browser without trusted input gets no Input commands`, async () => {
    await withPage(runner, {}, async ({ fake, run, ref }) => {
      for (const args of [
        ['click', ref],
        ['hover', ref],
        ['dblclick', ref],
        ['press', 'Tab'],
        ['mousemove', '5', '5'],
        ['mousedown'],
        ['mouseup'],
      ]) {
        const result = await run(args);
        assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
        const snapshot = await run(['snapshot']);
        assert.equal(snapshot.code, 0, snapshot.stderr);
      }
      assert.deepEqual(input(fake), []);
    });
  });

  test(`${transport}: input a page will not take says so and does not switch tabs`, async () => {
    await withPage(runner, { pointer: 'missing' }, async ({ fake, run, ref }) => {
      const result = await run(['click', ref]);
      assert.equal(result.code, 1);
      assert.equal(result.stderr, undelivered('click'));
      assert.deepEqual(input(fake), []);
      assert.ok(!fake.calls.some((call) => call.method === 'Target.activateTarget'));
    });
  });

  test(`${transport}: a key the page refuses is not reported as a background tab`, async () => {
    await withPage(runner, { key: 'unsupported' }, async ({ fake, run }) => {
      const result = await run(['press', 'Tab']);
      assert.equal(result.code, 1);
      assert.equal(result.stderr, undelivered('press'));
      assert.deepEqual(input(fake), []);
    });
  });
}

test('a missing location check still refuses on a browser that makes trusted input', async () => {
  const trusted = { arm: 'armed', visible: 'visible', read: ['landed'] };
  await withPage(runners.native, trusted, async ({ fake, run }) => {
    const moved = await run(['mousemove', '5', '5']);
    assert.equal(moved.code, 1);
    assert.match(moved.stderr, /DOM\.getNodeForLocation/);
    assert.deepEqual(input(fake), []);
  });
});

test('the page itself says it cannot be trusted, and takes one press and release', async () => {
  const page = mousePage('Apple Computer, Inc.');
  const landing = (message, send) => {
    if (!message.params?.expression?.includes("Symbol.for('slicc-landing')")) {
      return unknownLocation(message, send);
    }
    const value = page.evaluate(message.params.expression);
    send({ id: message.id, sessionId: message.sessionId, result: { result: { value } } });
    return true;
  };
  const fake = await fakeCdp({ intercept: landing });
  const pw = await runners.native(fake.url);
  try {
    const cwd = await pw.fresh();
    assert.equal((await pw.playwright(['open', 'http://example.test/page'], cwd)).code, 0);
    for (const args of [['mousemove', '70', '50'], ['mousedown'], ['mouseup']]) {
      const result = await pw.playwright(args, cwd);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
    }
    const count = (type) => page.events.filter((event) => event.type === type).length;
    assert.deepEqual([count('mousedown'), count('mouseup'), count('click')], [1, 1, 1]);
    assert.deepEqual(input(fake), []);
  } finally {
    await pw.close();
    await fake.close();
  }
});
