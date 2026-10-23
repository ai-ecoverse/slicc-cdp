import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const SEVEN = 'https://seven.sliccy.ai/';
const background = (verb) =>
  `Error: ${verb} could not reach the background tab in place, and brings it to the front only when the tab that was in front can be switched back to. Run "playwright-cli tab-select <index> --foreground" first.\n`;

function scene({ front = 'SEVEN', others = [], reads = [[]], pointer = 'missing', ...rest } = {}) {
  const state = { front, reads: [...reads], modes: [], activated: [], lag: 0 };
  const fallback = { arm: 'armed', focus: 'editable', insert: 'unchanged', key: 'unsupported' };
  const options = {
    visibility: (target) => (target?.targetId === state.front ? 'visible' : 'hidden'),
    windows: (id) => ({
      windowId: rest.windows?.[id] ?? 1,
      bounds: { windowState: 'normal' },
    }),
    landing: (mode, args, target) => {
      state.modes.push([mode, args, target?.targetId]);
      if (mode === 'visible' && state.lag > 0) {
        state.lag -= 1;
        return 'hidden';
      }
      if (mode === 'visible') return target?.targetId === state.front ? 'visible' : 'hidden';
      if (mode === 'read') return state.reads.length ? state.reads.shift() : ['landed'];
      if (mode === 'pointer') return pointer;
      return rest[mode] ?? fallback[mode] ?? 'done';
    },
    onCommand: (message) => {
      if (message.method !== 'Target.activateTarget') return;
      state.activated.push(message.params.targetId);
      if (rest.stuck) return;
      state.front = message.params.targetId;
      state.lag = rest.lag ?? 0;
    },
  };
  return { state, options, others };
}

async function withScene(runner, setup, body) {
  const { state, options, others } = setup;
  const fake = await fakeCdp({ ...options, ...setup.fake });
  for (const target of others) fake.targets.push({ ...target });
  const pw = await runner(fake.url);
  try {
    const cwd = await pw.fresh();
    const run = (args) => pw.playwright(args, cwd);
    const ok = async (args) => {
      const result = await run(args);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      return result;
    };
    await ok(['open', 'http://example.test/page']);
    const shot = await ok(['snapshot']);
    const ref = shot.stdout.match(/button "Go" \[ref=(e\d+)\]/)?.[1];
    assert.ok(ref, shot.stdout);
    state.modes.length = 0;
    state.activated.length = 0;
    await body({ fake, state, run, ok, ref });
  } finally {
    await pw.close();
    await fake.close();
  }
}

const runners = {
  native: async (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: async (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};
const modes = (state, name) => state.modes.filter(([mode]) => mode === name);
const sevenTab = { targetId: 'SEVEN', url: SEVEN };

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: input that Chrome delivers is not repeated`, async () => {
    await withScene(runner, scene({ reads: [['mousedown', 'mouseup', 'click']] }), async (s) => {
      const clicked = await s.ok(['click', s.ref]);
      assert.equal(clicked.stdout.includes('Clicked'), true);
      assert.deepEqual(modes(s.state, 'pointer'), []);
      assert.deepEqual(modes(s.state, 'arm')[0][1], { types: ['mousedown', 'mouseup', 'click'] });
      assert.deepEqual(s.state.activated, []);
    });
  });

  test(`${transport}: dropped clicks, double clicks and hovers land in place`, async () => {
    const setup = scene({ reads: [[], [], []], pointer: 'done', others: [sevenTab] });
    await withScene(runner, setup, async (s) => {
      await s.ok(['click', s.ref]);
      await s.ok(['snapshot']);
      await s.ok(['dblclick', s.ref, 'right']);
      await s.ok(['snapshot']);
      await s.ok(['hover', s.ref]);
      const sent = modes(s.state, 'pointer').map(([, args]) => args.steps.map(([type]) => type));
      assert.deepEqual(sent, [
        ['mousedown', 'mouseup', 'click'],
        ['mousedown', 'mouseup', 'contextmenu', 'mousedown', 'mouseup', 'contextmenu'],
        ['mousemove'],
      ]);
      assert.equal(modes(s.state, 'pointer')[1][1].buttons, 2);
      assert.equal(modes(s.state, 'pointer')[1][1].button, 2);
      assert.deepEqual(s.state.activated, []);
      assert.ok(
        s.fake.calls.some(
          (call) => call.method === 'DOM.resolveNode' && call.params.executionContextId
        )
      );
    });
  });

  test(`${transport}: a click that cannot land in place switches to the tab and back to seven`, async () => {
    const setup = scene({
      reads: [[], ['click']],
      others: [sevenTab, { targetId: 'BLANK', url: 'about:blank' }],
    });
    await withScene(runner, setup, async (s) => {
      const tab = s.fake.targets.find((target) =>
        target.url.startsWith('http://example.test')
      ).targetId;
      await s.ok(['click', s.ref]);
      assert.deepEqual(s.state.activated, [tab, 'SEVEN']);
      assert.equal(s.state.front, 'SEVEN');
      const presses = s.fake.calls.filter(
        (call) => call.method === 'Input.dispatchMouseEvent' && call.params.type === 'mousePressed'
      );
      assert.equal(presses.length, 2);
    });
  });

  test(`${transport}: the tab that was in front comes back when it is an ordinary tab`, async () => {
    const setup = scene({
      front: 'OTHER',
      reads: [[], ['mousemove']],
      others: [sevenTab, { targetId: 'OTHER', url: 'http://other.test/' }],
    });
    await withScene(runner, setup, async (s) => {
      await s.ok(['hover', s.ref]);
      assert.deepEqual(s.state.activated, [own(s), 'OTHER']);
    });
  });

  test(`${transport}: input still dropped in front fails and restores seven`, async () => {
    const setup = scene({ reads: [[], []], others: [sevenTab] });
    await withScene(runner, setup, async (s) => {
      const result = await s.run(['click', s.ref]);
      assert.equal(result.code, 1);
      assert.equal(
        result.stderr,
        'Error: click did not reach the page: Chrome dropped the input even with the tab in front.\n'
      );
      assert.equal(s.state.activated.at(-1), 'SEVEN');
    });
  });

  for (const [name, others, windows] of [
    ['the vault is in front', [{ targetId: 'VAULT', url: 'https://vault.sliccy.ai/' }], {}],
    [
      'two protected tabs could be in front',
      [sevenTab, { targetId: 'VAULT', url: 'https://vault.sliccy.ai/' }],
      {},
    ],
    ['seven is in another window', [sevenTab], { SEVEN: 2 }],
    ['no tab is visibly in front', [{ targetId: 'HIDDEN', url: 'https://other.test/' }], {}],
  ]) {
    test(`${transport}: no tab is activated when ${name}`, async () => {
      const setup = scene({ front: 'NONE', others, windows });
      await withScene(runner, setup, async (s) => {
        const result = await s.run(['dblclick', s.ref]);
        assert.equal(result.code, 1);
        assert.equal(result.stderr, background('dblclick'));
        assert.deepEqual(s.state.activated, []);
      });
    });
  }

  test(`${transport}: a click into an embedded frame is refused`, async () => {
    await withScene(runner, scene({ pointer: 'frame', others: [sevenTab] }), async (s) => {
      const result = await s.run(['click', s.ref]);
      assert.equal(result.code, 1);
      assert.match(
        result.stderr,
        /^slicc: refused: mouse input at \(10, 10\) would reach an embedded frame\n$/
      );
      assert.deepEqual(s.state.activated, []);
    });
  });

  test(`${transport}: type lands in place and needs a focused field`, async () => {
    await withScene(runner, scene({ reads: [[]], insert: 'inserted' }), async (s) => {
      await s.ok(['type', 'hello']);
      assert.deepEqual(
        modes(s.state, 'insert').map(([, args]) => args),
        [{ text: 'hello' }]
      );
      assert.deepEqual(modes(s.state, 'arm').at(-1)[1], { types: ['beforeinput', 'input'] });
    });
    await withScene(runner, scene({ focus: 'none' }), async (s) => {
      const result = await s.run(['type', 'hello']);
      assert.equal(result.code, 1);
      assert.match(result.stderr, /type needs a focused editable element/);
      assert.ok(!s.fake.calls.some((call) => call.method === 'Input.insertText'));
    });
  });

  test(`${transport}: press lands in place or in front, and frames need the front`, async () => {
    await withScene(runner, scene({ reads: [[]], key: 'pressed' }), async (s) => {
      await s.ok(['press', 'Enter']);
      assert.deepEqual(modes(s.state, 'key')[0][1], {
        key: 'Enter',
        code: 'Enter',
        text: '\r',
        modifiers: 0,
      });
      assert.deepEqual(s.state.activated, []);
    });
    await withScene(runner, scene({ reads: [[], ['keydown']], others: [sevenTab] }), async (s) => {
      await s.ok(['press', 'Tab']);
      assert.equal(s.state.activated.at(-1), 'SEVEN');
    });
    await withScene(runner, scene({ focus: 'frame', others: [sevenTab] }), async (s) => {
      await s.ok(['press', 'a']);
      await s.ok(['type', 'b']);
      assert.equal(s.state.activated.length, 4);
      assert.deepEqual(modes(s.state, 'arm'), []);
    });
  });
}

const own = (s) =>
  s.fake.targets.find((target) => target.url.startsWith('http://example.test')).targetId;

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: a ref without a backend node lands by its selector`, async () => {
    const setup = scene({ reads: [[]], pointer: 'done' });
    setup.fake = {
      aria: () => ({
        json: [{ role: 'button', name: 'Go', ref: 'e1', cursor: 'pointer' }],
        refs: [{ ref: 'e1', selector: '#go' }],
        base: 'http://example.test/',
        nodes: [null],
      }),
    };
    await withScene(runner, setup, async (s) => {
      await s.ok(['click', s.ref]);
      assert.equal(modes(s.state, 'pointer')[0][1].selector, '#go');
    });
  });

  test(`${transport}: a page that cannot run the check fails the command`, async () => {
    await withScene(runner, scene({ arm: 'nope' }), async (s) => {
      const result = await s.run(['click', s.ref]);
      assert.equal(result.code, 1);
      assert.equal(result.stderr, 'Error: the input check could not start in the page.\n');
      assert.ok(!s.fake.calls.some((call) => call.method === 'Input.dispatchMouseEvent'));
    });
    await withScene(runner, scene({ focus: { exception: 'focus broke' } }), async (s) => {
      const result = await s.run(['type', 'x']);
      assert.equal(result.code, 1);
      assert.match(result.stderr, /focus broke/);
    });
  });

  test(`${transport}: a probe that vanished with the page counts as landed`, async () => {
    await withScene(runner, scene({ reads: [null] }), async (s) => {
      await s.ok(['click', s.ref]);
      assert.deepEqual(modes(s.state, 'pointer'), []);
    });
  });

  test(`${transport}: the visible tab gets the trusted input once and fails if it is dropped`, async () => {
    await withScene(runner, scene({ reads: [[]] }), async (s) => {
      s.state.front = own(s);
      const result = await s.run(['click', s.ref]);
      assert.equal(result.code, 1);
      assert.match(result.stderr, /click did not reach the page/);
      assert.deepEqual(s.state.activated, []);
    });
    await withScene(runner, scene({ focus: 'frame' }), async (s) => {
      s.state.front = own(s);
      await s.ok(['press', 'a']);
      assert.deepEqual(s.state.activated, []);
      assert.equal(
        s.fake.calls.filter((call) => call.method === 'Input.dispatchKeyEvent').length,
        2
      );
    });
  });

  test(`${transport}: the tab is waited for after activation and restored when it never shows`, async () => {
    await withScene(
      runner,
      scene({ reads: [[], ['click']], lag: 2, others: [sevenTab] }),
      async (s) => {
        await s.ok(['click', s.ref]);
        assert.deepEqual(s.state.activated, [own(s), 'SEVEN']);
      }
    );
    await withScene(runner, scene({ stuck: true, others: [sevenTab] }), async (s) => {
      const result = await s.run(['hover', s.ref]);
      assert.equal(result.code, 1);
      assert.equal(result.stderr, background('hover'));
      assert.deepEqual(s.state.activated, [own(s), 'SEVEN']);
    });
  });

  test(`${transport}: type falls back to the front when it cannot insert in place`, async () => {
    await withScene(runner, scene({ reads: [[], ['input']], others: [sevenTab] }), async (s) => {
      await s.ok(['type', 'x']);
      assert.deepEqual(s.state.activated, [own(s), 'SEVEN']);
      assert.equal(s.fake.calls.filter((call) => call.method === 'Input.insertText').length, 2);
    });
  });

  test(`${transport}: a refused trusted double click is reported`, async () => {
    const setup = scene();
    setup.fake = { refuse: ['Input.dispatchMouseEvent'] };
    await withScene(runner, setup, async (s) => {
      const result = await s.run(['dblclick', s.ref]);
      assert.equal(result.code, 1);
      assert.match(result.stderr, /Input\.dispatchMouseEvent refused/);
    });
  });
}

const vaultTab = { targetId: 'VAULT', url: 'https://vault.sliccy.ai/' };

for (const [transport, runner] of Object.entries(runners)) {
  for (const [name, others, front] of [
    ['a second protected tab shares the window', [sevenTab, vaultTab], 'NONE'],
    [
      'an ordinary tab in the window is visible',
      [sevenTab, { targetId: 'OTHER', url: 'http://other.test/' }],
      'OTHER',
    ],
    [
      'the window also holds a protected tab of unknown kind',
      [sevenTab, { targetId: 'APP', type: 'other', url: 'https://vault.sliccy.ai/app' }],
      'NONE',
    ],
  ]) {
    test(`${transport}: seven is never switched back to when ${name}`, async () => {
      const setup = scene({ front, others, reads: [[], ['click']] });
      await withScene(runner, setup, async (s) => {
        const result = await s.run(['click', s.ref]);
        assert.ok(!s.state.activated.includes('SEVEN'), JSON.stringify(s.state.activated));
        if (front === 'OTHER') {
          assert.equal(result.code, 0, result.stderr);
          assert.deepEqual(s.state.activated, [own(s), 'OTHER']);
        } else {
          assert.equal(result.code, 1);
          assert.equal(result.stderr, background('click'));
          assert.deepEqual(s.state.activated, []);
        }
      });
    });
  }

  test(`${transport}: input aimed at seven's or the vault's tab is refused before anything reaches it`, async () => {
    const setup = scene({ others: [sevenTab, vaultTab] });
    await withScene(runner, setup, async (s) => {
      const before = s.fake.calls.length;
      for (const tab of ['SEVEN', 'VAULT']) {
        for (const args of [
          ['click', s.ref],
          ['dblclick', s.ref],
          ['hover', s.ref],
          ['type', 'secret'],
          ['press', 'Tab'],
          ['mousewheel', '0', '100'],
        ]) {
          const result = await s.run([...args, '--tab', tab]);
          assert.equal(result.code, 1, `${args[0]} ${tab}: ${result.stdout}`);
          assert.match(result.stderr, /^slicc: refused:/, `${args[0]} ${tab}`);
        }
      }
      const reached = s.fake.calls
        .slice(before)
        .filter(
          (call) =>
            call.method.startsWith('Input.') ||
            call.method === 'Target.activateTarget' ||
            call.method === 'Browser.getWindowForTarget' ||
            (['SEVEN', 'VAULT'].includes(call.target) && call.method !== 'Page.getFrameTree')
        );
      assert.deepEqual(reached, []);
      assert.deepEqual(s.state.modes, []);
      assert.deepEqual(s.state.activated, []);
    });
  });
}
