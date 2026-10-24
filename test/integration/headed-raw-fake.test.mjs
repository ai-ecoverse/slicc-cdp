import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const exe = (name) =>
  path.resolve('target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.resolve('target/wasm32-wasip1/debug', `${name}.wasm`);
const sevenTab = { targetId: 'SEVEN', url: 'https://seven.sliccy.ai/' };
const vaultTab = { targetId: 'VAULT', url: 'https://vault.sliccy.ai/' };
const background = (verb) =>
  `Error: ${verb} could not reach the background tab in place, and brings it to the front only when the tab that was in front can be switched back to. Run "playwright-cli tab-select <index> --foreground" first.\n`;
const focusRefusal =
  'slicc: refused: keyboard focus is inside an embedded frame or a closed shadow root\n';
const runners = {
  native: async (url) => nativeRunner(exe('playwright-cli'), exe('curlwright'), url),
  wasi: async (url) => kernelRunner(wasm('playwright-cli'), wasm('curlwright'), url),
};

function scene({ front = 'SEVEN', others = [sevenTab], reads = [], ...rest } = {}) {
  const state = { front, reads: [...reads], modes: [], activated: [] };
  const shared = { calls: [] };
  const answers = { mouse: rest.mouse ?? 'done', key: rest.key ?? 'pressed', arm: 'armed' };
  const options = {
    aria: rest.aria,
    visibility: (target) => (target?.targetId === state.front ? 'visible' : 'hidden'),
    landing: (mode, args, target) => {
      state.modes.push([mode, args, target?.targetId]);
      if (mode === 'visible') return target?.targetId === state.front ? 'visible' : 'hidden';
      if (mode === 'arm') state.armed = args.types;
      if (mode === 'read') return state.reads.length ? state.reads.shift() : state.armed;
      return answers[mode] ?? 'done';
    },
    onCommand: (message) => {
      if (message.method !== 'Target.activateTarget') return;
      state.activated.push(message.params.targetId);
      state.front = message.params.targetId;
    },
    intercept: (message, send) => {
      const reply = (result) => send({ id: message.id, sessionId: message.sessionId, result });
      if (message.method === 'Page.getLayoutMetrics') {
        reply({ cssLayoutViewport: { pageX: 0, pageY: 0, clientWidth: 800, clientHeight: 600 } });
        return true;
      }
      if (message.method === 'DOM.getNodeForLocation') {
        reply({ backendNodeId: 5, frameId: shared.calls.at(-1)?.target });
        return true;
      }
      if (message.params?.expression?.startsWith('(() => { let e = document.activeElement')) {
        reply({ result: { type: 'object', subtype: 'null', value: null } });
        return true;
      }
      return false;
    },
  };
  return { state, options, others, shared, tree: rest.tree };
}

async function withScene(runner, setup, body) {
  const fake = await fakeCdp(setup.options);
  setup.shared.calls = fake.calls;
  for (const target of setup.others) fake.targets.push({ ...target });
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
    const own = fake.targets.find((target) => target.url.startsWith('http://example.test'));
    if (setup.tree) own.tree = setup.tree(own.targetId);
    await ok(['snapshot']);
    setup.state.modes.length = 0;
    setup.state.activated.length = 0;
    const sent = (method) => fake.calls.filter((call) => call.method === method);
    await body({ fake, state: setup.state, run, ok, tab: own.targetId, sent });
  } finally {
    await pw.close();
    await fake.close();
  }
}

const modes = (state, name) => state.modes.filter(([mode]) => mode === name);
const RAW = [
  ['mousemove', '10', '20'],
  ['mousedown'],
  ['mouseup'],
  ['keydown', 'a'],
  ['keyup', 'a'],
  ['drag', 'e1', 'e2'],
];

for (const [transport, runner] of Object.entries(runners)) {
  test(`${transport}: raw input and drag on the tab in front are checked and sent once`, async () => {
    await withScene(runner, scene(), async (s) => {
      s.state.front = s.tab;
      for (const args of RAW) await s.ok(args);
      assert.equal(modes(s.state, 'arm').length, RAW.length);
      assert.deepEqual(modes(s.state, 'mouse'), []);
      assert.equal(s.sent('Input.dispatchMouseEvent').length, 7);
      assert.equal(s.sent('Input.dispatchKeyEvent').length, 2);
      assert.deepEqual(s.state.activated, []);
      assert.deepEqual(
        s
          .sent('Input.dispatchMouseEvent')
          .slice(3)
          .map(({ params }) => [params.type, params.button, params.buttons]),
        [
          ['mouseMoved', 'none', 0],
          ['mousePressed', 'left', 1],
          ['mouseMoved', 'left', 1],
          ['mouseReleased', 'left', 0],
        ]
      );
    });
    await withScene(
      runner,
      scene({ mouse: 'missing', key: 'unsupported', reads: [[], []] }),
      async (s) => {
        s.state.front = s.tab;
        await s.ok(['mousemove', '10', '20']);
        assert.equal(s.sent('Input.dispatchMouseEvent').length, 1);
        const pressed = await s.run(['keydown', 'Tab']);
        assert.equal(pressed.code, 1);
        assert.equal(
          pressed.stderr,
          'Error: keydown did not reach the page: Chrome dropped the input even with the tab in front.\n'
        );
        assert.equal(s.sent('Input.dispatchKeyEvent').length, 1);
        assert.deepEqual(s.state.activated, []);
      }
    );
  });

  test(`${transport}: a press done in place is released in place, and a dropped release after Chrome's press still clicks`, async () => {
    await withScene(runner, scene({ reads: [[]] }), async (s) => {
      await s.ok(['mousedown']);
      await s.ok(['mouseup']);
      assert.equal(modes(s.state, 'arm').length, 1);
      assert.deepEqual(
        modes(s.state, 'mouse').map(([, args]) => args),
        [
          { steps: [['down', 0, 0, 1, 0]], from: [0, 0], press: null, modifiers: 0 },
          { steps: [['up', 0, 0, 0, 0]], from: [0, 0], press: [0, 0], modifiers: 0 },
        ]
      );
      assert.deepEqual(
        s.sent('Input.dispatchMouseEvent').map(({ params }) => params.type),
        ['mousePressed']
      );
    });
    await withScene(runner, scene({ reads: [['mousedown'], []] }), async (s) => {
      await s.ok(['mousedown']);
      await s.ok(['mouseup']);
      await s.ok(['mouseup']);
      assert.equal(modes(s.state, 'arm').length, 3);
      assert.deepEqual(
        modes(s.state, 'mouse').map(([, args]) => args),
        [{ steps: [['up', 0, 0, 0, 0]], from: [0, 0], press: [0, 0], modifiers: 0 }]
      );
      assert.deepEqual(
        s.sent('Input.dispatchMouseEvent').map(({ params }) => params.type),
        ['mousePressed', 'mouseReleased', 'mouseReleased']
      );
    });
  });

  test(`${transport}: a drag Chrome delivered in part is finished in place from the first missing step`, async () => {
    await withScene(
      runner,
      scene({ reads: [['mousemove'], ['mousemove', 'mousedown']] }),
      async (s) => {
        await s.ok(['drag', 'e1', 'e2']);
        await s.ok(['snapshot']);
        await s.ok(['drag', 'e1', 'e2']);
        assert.deepEqual(
          modes(s.state, 'mouse').map(([, args]) => args),
          [
            {
              steps: [
                ['down', 5, 5, 1, 0],
                ['move', 5, 5, 1, 0],
                ['up', 5, 5, 0, 0],
              ],
              from: [5, 5],
              press: null,
              modifiers: 0,
            },
            { steps: [['up', 5, 5, 0, 0]], from: [5, 5], press: [5, 5], modifiers: 0 },
          ]
        );
        assert.equal(s.sent('Input.dispatchMouseEvent').length, 8);
        assert.deepEqual(s.state.activated, []);
      }
    );
  });

  test(`${transport}: raw input and drag Chrome delivers to a background tab are not repeated`, async () => {
    await withScene(runner, scene(), async (s) => {
      for (const args of RAW) await s.ok(args);
      assert.deepEqual(
        modes(s.state, 'arm').map(([, args]) => args.types),
        [
          ['mousemove'],
          ['mousedown'],
          ['mouseup'],
          ['keydown'],
          ['keyup'],
          ['mousemove', 'mousedown', 'mouseup'],
        ]
      );
      assert.deepEqual(modes(s.state, 'mouse'), []);
      assert.deepEqual(modes(s.state, 'key'), []);
      assert.equal(s.sent('Input.dispatchMouseEvent').length, 7);
      assert.deepEqual(s.state.activated, []);
    });
  });

  test(`${transport}: dropped raw input and drags are dispatched in place with the saved pointer`, async () => {
    const steps = [
      ['keydown', 'Shift'],
      ['mousemove', '10', '20'],
      ['mousedown', 'right'],
      ['mousemove', '30', '40'],
      ['mouseup', 'right'],
      ['keyup', 'Shift'],
      ['keydown', 'KeyA'],
      ['keydown', 'KeyA'],
      ['keyup', 'KeyA'],
      ['drag', 'e1', 'e2'],
    ];
    const setup = scene({ reads: steps.map(() => []) });
    await withScene(runner, setup, async (s) => {
      for (const args of steps) await s.ok(args);
      assert.deepEqual(
        modes(s.state, 'mouse').map(([, args]) => args),
        [
          { steps: [['move', 10, 20, 0, 0]], from: [0, 0], press: null, modifiers: 8 },
          { steps: [['down', 10, 20, 2, 2]], from: [10, 20], press: null, modifiers: 8 },
          { steps: [['move', 30, 40, 2, 0]], from: [10, 20], press: [10, 20], modifiers: 8 },
          { steps: [['up', 30, 40, 0, 2]], from: [30, 40], press: [10, 20], modifiers: 8 },
          {
            steps: [
              ['move', 5, 5, 0, 0],
              ['down', 5, 5, 1, 0],
              ['move', 5, 5, 1, 0],
              ['up', 5, 5, 0, 0],
            ],
            from: null,
            press: null,
            modifiers: 0,
          },
        ]
      );
      const key = (key, code, text, modifiers, phase, repeat, location) => ({
        key,
        code,
        text,
        modifiers,
        phase,
        repeat,
        location,
      });
      assert.deepEqual(
        modes(s.state, 'key').map(([, args]) => args),
        [
          key('Shift', 'ShiftLeft', null, 8, 'down', false, 1),
          key('Shift', 'ShiftLeft', null, 0, 'up', false, 1),
          key('a', 'KeyA', 'a', 0, 'down', false, 0),
          key('a', 'KeyA', 'a', 0, 'down', true, 0),
          key('a', 'KeyA', 'a', 0, 'up', false, 0),
        ]
      );
      assert.equal(s.sent('Input.dispatchMouseEvent').length, 7);
      assert.equal(s.sent('Input.dispatchKeyEvent').length, 5);
      assert.deepEqual(s.state.activated, []);
    });
  });

  test(`${transport}: raw input that cannot land in place switches to the tab and back to seven`, async () => {
    await withScene(runner, scene({ key: 'unsupported', reads: [[], ['keydown']] }), async (s) => {
      await s.ok(['keydown', 'Tab']);
      assert.deepEqual(s.state.activated, [s.tab, 'SEVEN']);
      assert.equal(s.sent('Input.dispatchKeyEvent').length, 2);
    });
    await withScene(runner, scene({ mouse: 'missing', reads: [[]] }), async (s) => {
      await s.ok(['mousemove', '10', '20']);
      assert.deepEqual(s.state.activated, [s.tab, 'SEVEN']);
      assert.equal(s.sent('Input.dispatchMouseEvent').length, 2);
      assert.equal(modes(s.state, 'read').length, 1);
    });
    await withScene(runner, scene({ mouse: 'other', reads: [[], ['mouseup']] }), async (s) => {
      await s.ok(['mouseup']);
      assert.deepEqual(s.state.activated, [s.tab, 'SEVEN']);
      assert.equal(modes(s.state, 'read').length, 2);
    });
  });

  test(`${transport}: raw input still dropped in front fails and restores seven`, async () => {
    await withScene(runner, scene({ key: 'unsupported', reads: [[], []] }), async (s) => {
      const result = await s.run(['keyup', 'Space']);
      assert.equal(result.code, 1);
      assert.equal(
        result.stderr,
        'Error: keyup did not reach the page: Chrome dropped the input even with the tab in front.\n'
      );
      assert.deepEqual(s.state.activated, [s.tab, 'SEVEN']);
    });
  });

  test(`${transport}: raw input and drag refuse when no tab may be brought forward`, async () => {
    const setup = scene({
      front: 'NONE',
      others: [vaultTab],
      reads: RAW.map(() => []).concat([[], []]),
      mouse: 'missing',
      key: 'unsupported',
    });
    await withScene(runner, setup, async (s) => {
      for (const args of [...RAW, ['keydown', 'Tab'], ['keyup', 'Tab']]) {
        const result = await s.run(args);
        assert.equal(result.code, 1, args.join(' '));
        assert.equal(result.stderr, background(args[0]));
      }
      assert.deepEqual(s.state.activated, []);
    });
  });

  test(`${transport}: raw input or a drag that would reach a frame is refused`, async () => {
    await withScene(
      runner,
      scene({ mouse: 'frame:0', key: 'frame', reads: [[], []] }),
      async (s) => {
        const moved = await s.run(['mousemove', '10', '20']);
        assert.equal(moved.code, 1);
        assert.equal(
          moved.stderr,
          'slicc: refused: mouse input at (10, 20) would reach an embedded frame\n'
        );
        const pressed = await s.run(['keydown', 'a']);
        assert.equal(pressed.code, 1);
        assert.equal(pressed.stderr, focusRefusal);
        assert.deepEqual(s.state.activated, []);
      }
    );
    await withScene(runner, scene({ mouse: 'frame:3', reads: [[]] }), async (s) => {
      const dragged = await s.run(['drag', 'e1', 'e2']);
      assert.equal(dragged.code, 1);
      assert.equal(
        dragged.stderr,
        'slicc: refused: mouse input at (5, 5) would reach an embedded frame\n'
      );
    });
  });

  test(`${transport}: a drag from a frame is checked in the frame and only goes to the front`, async () => {
    const setup = scene({
      reads: [[], ['mousedown']],
      aria: (options) =>
        options.prefix === 'f1'
          ? {
              json: [{ role: 'button', name: 'Inner', ref: 'f1e1', cursor: 'pointer' }],
              refs: [{ ref: 'f1e1', selector: '#inner' }],
              base: 'http://example.test/child',
              nodes: [{ backendNodeId: 9 }],
            }
          : {
              json: [
                { role: 'button', name: 'Go', ref: 'e1', cursor: 'pointer' },
                { role: 'iframe', ref: 'e2' },
              ],
              refs: [
                { ref: 'e1', selector: '#go' },
                { ref: 'e2', selector: 'iframe' },
              ],
              base: 'http://example.test/',
              nodes: [{ backendNodeId: 1 }, { backendNodeId: 2, frameId: 'CHILD' }],
            },
      tree: (id) => ({
        frame: {
          id,
          loaderId: 'L',
          url: 'http://example.test/page',
          securityOrigin: 'http://example.test',
        },
        childFrames: [
          {
            frame: {
              id: 'CHILD',
              parentId: id,
              loaderId: 'L',
              url: 'http://example.test/child',
              securityOrigin: 'http://example.test',
            },
          },
        ],
      }),
    });
    await withScene(runner, setup, async (s) => {
      await s.ok(['drag', 'f1e1', 'e1']);
      assert.deepEqual(s.state.activated, [s.tab, 'SEVEN']);
      assert.deepEqual(modes(s.state, 'mouse'), []);
      assert.equal(s.sent('Input.dispatchMouseEvent').length, 8);
      assert.ok(
        s
          .sent('Page.createIsolatedWorld')
          .some(
            (call) => call.params.frameId === 'CHILD' && call.params.worldName === 'slicc-landing'
          )
      );
    });
  });

  test(`${transport}: raw input aimed at seven's or the vault's tab is refused before anything reaches it`, async () => {
    await withScene(runner, scene({ others: [sevenTab, vaultTab] }), async (s) => {
      const before = s.fake.calls.length;
      for (const tab of ['SEVEN', 'VAULT']) {
        for (const args of RAW.slice(0, 5)) {
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
