import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner } from './runners.mjs';
import './watchdog.mjs';

const binary =
  process.env.SLICC_INPUT_BIN ||
  path.resolve(
    'target/debug',
    process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
  );
const wasm = path.resolve('target/wasm32-wasip1/debug');
const code = (text) => `### Ran Playwright code\n\`\`\`js\n${text}\n\`\`\`\n`;
const notAccepted =
  'Error: Drop target did not accept the drop \u2014 its dragover handler did not call preventDefault()\n';

function run(args, home) {
  return new Promise((resolve, reject) => {
    const env = {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      APPDATA: home,
      XDG_CONFIG_HOME: home,
      XDG_STATE_HOME: home,
      SLICC_CDP_PORTS: HARNESS_PORTS,
    };
    delete env.SLICC_CDP_URL;
    const child = spawn(binary, args, { cwd: home, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`input command timed out: ${args.join(' ')}`));
    }, 20000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (status) => {
      clearTimeout(timer);
      resolve({ code: status, stdout, stderr });
    });
  });
}

function page(script) {
  return (message, send) => {
    const { id, method, params = {}, sessionId } = message;
    const reply = (result) => send({ id, ...(sessionId ? { sessionId } : {}), result });
    const fail = (text) =>
      send({ id, ...(sessionId ? { sessionId } : {}), error: { code: -32000, message: text } });
    if (script.intercept?.(message, reply, fail)) return true;
    if (method === 'DOM.getNodeForLocation') {
      if (script.miss) fail('No node found at given location');
      else reply(script.hit ?? { backendNodeId: 5, frameId: 'TAB' });
      return true;
    }
    if (method === 'DOM.describeNode' && params.backendNodeId === 5) {
      reply({ node: script.hitNode ?? { nodeType: 1, nodeName: 'DIV', localName: 'div' } });
      return true;
    }
    if (method === 'DOM.describeNode' && params.objectId === 'FOCUSED') {
      reply({ node: script.focusNode ?? { nodeType: 1, nodeName: 'INPUT', localName: 'input' } });
      return true;
    }
    if (method === 'Runtime.evaluate' && params.expression?.includes('document.activeElement')) {
      reply({ result: script.focus ?? { type: 'object', subtype: 'node', objectId: 'FOCUSED' } });
      return true;
    }
    if (method === 'Runtime.evaluate' && params.expression === '[innerWidth, innerHeight]') {
      reply({ result: { type: 'object', subtype: 'array', value: script.inner ?? [800, 600] } });
      return true;
    }
    if (method === 'Runtime.evaluate' && params.expression === 'document.visibilityState') {
      reply({ result: { type: 'string', value: script.visibility ?? 'visible' } });
      return true;
    }
    if (method === 'Page.getLayoutMetrics') {
      reply(
        script.metrics ?? {
          cssLayoutViewport: { pageX: 0, pageY: 0, clientWidth: 800, clientHeight: 600 },
        }
      );
      return true;
    }
    if (method === 'Browser.getWindowForTarget') {
      reply({
        windowId: 3,
        bounds: { left: 0, top: 0, width: 900, height: 700, windowState: 'normal' },
      });
      return true;
    }
    if (method === 'Browser.getVersion') {
      reply({
        product: 'Chrome/140.0.0.0',
        userAgent: script.agent ?? 'Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0.0.0',
      });
      return true;
    }
    if (
      method === 'Runtime.callFunctionOn' &&
      params.functionDeclaration?.includes('DataTransfer')
    ) {
      reply({ result: { type: 'string', value: script.drop ?? 'accepted' } });
      return true;
    }
    return false;
  };
}

async function withFake(script, body) {
  const home = await realpath(await mkdtemp(path.join(tmpdir(), 'slicc-input-')));
  const fake = await fakeCdp({ intercept: page(script) });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  try {
    await body(fake, (args) => run(['--cdp', fake.url, ...args], home), home);
  } finally {
    await fake.close();
    await rm(home, { recursive: true, force: true });
  }
}

const sent = (fake, method) => fake.calls.filter((call) => call.method === method);
const params = (fake, method) => sent(fake, method).map((call) => call.params);

test('keydown and keyup keep Playwright key definitions and held modifiers across commands', async () => {
  await withFake({}, async (fake, pw) => {
    const steps = [
      ['keydown', 'Shift', "await page.keyboard.down('Shift');"],
      ['keydown', 'KeyA', "await page.keyboard.down('KeyA');"],
      ['keydown', 'KeyA', "await page.keyboard.down('KeyA');"],
      ['keyup', 'KeyA', "await page.keyboard.up('KeyA');"],
      ['keyup', 'Shift', "await page.keyboard.up('Shift');"],
      ['keydown', 'Numpad1', "await page.keyboard.down('Numpad1');"],
      ['keyup', "'", "await page.keyboard.up('\\'');"],
    ];
    for (const [command, key, expected] of steps) {
      const result = await pw([command, key]);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(result.stdout, code(expected));
    }
    assert.deepEqual(params(fake, 'Input.dispatchKeyEvent'), [
      {
        type: 'rawKeyDown',
        modifiers: 8,
        windowsVirtualKeyCode: 16,
        code: 'ShiftLeft',
        commands: [],
        key: 'Shift',
        text: '',
        unmodifiedText: '',
        autoRepeat: false,
        location: 1,
        isKeypad: false,
      },
      {
        type: 'keyDown',
        modifiers: 8,
        windowsVirtualKeyCode: 65,
        code: 'KeyA',
        commands: [],
        key: 'A',
        text: 'A',
        unmodifiedText: 'A',
        autoRepeat: false,
        location: 0,
        isKeypad: false,
      },
      {
        type: 'keyDown',
        modifiers: 8,
        windowsVirtualKeyCode: 65,
        code: 'KeyA',
        commands: [],
        key: 'A',
        text: 'A',
        unmodifiedText: 'A',
        autoRepeat: true,
        location: 0,
        isKeypad: false,
      },
      {
        type: 'keyUp',
        modifiers: 8,
        key: 'A',
        windowsVirtualKeyCode: 65,
        code: 'KeyA',
        location: 0,
        isKeypad: false,
      },
      {
        type: 'keyUp',
        modifiers: 0,
        key: 'Shift',
        windowsVirtualKeyCode: 16,
        code: 'ShiftLeft',
        location: 1,
        isKeypad: false,
      },
      {
        type: 'rawKeyDown',
        modifiers: 0,
        windowsVirtualKeyCode: 35,
        code: 'Numpad1',
        commands: [],
        key: 'End',
        text: '',
        unmodifiedText: '',
        autoRepeat: false,
        location: 3,
        isKeypad: true,
      },
      {
        type: 'keyUp',
        modifiers: 0,
        key: "'",
        windowsVirtualKeyCode: 222,
        code: 'Quote',
        location: 0,
        isKeypad: false,
      },
    ]);
  });
});

test('macOS browsers get editing commands and ControlOrMeta resolves to Meta', async () => {
  await withFake({ agent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' }, async (fake, pw) => {
    for (const [command, key] of [
      ['keydown', 'ControlOrMeta'],
      ['keydown', 'a'],
      ['keyup', 'a'],
      ['keyup', 'ControlOrMeta'],
    ]) {
      const result = await pw([command, key]);
      assert.equal(result.code, 0, result.stderr);
    }
    const events = params(fake, 'Input.dispatchKeyEvent');
    assert.deepEqual(
      events.map((event) => [event.type, event.key, event.code, event.modifiers, event.commands]),
      [
        ['rawKeyDown', 'Meta', 'MetaLeft', 4, []],
        ['rawKeyDown', 'a', 'KeyA', 4, ['selectAll']],
        ['keyUp', 'a', 'KeyA', 4, undefined],
        ['keyUp', 'Meta', 'MetaLeft', 0, undefined],
      ]
    );
  });
});

test('mouse commands keep the pointer, buttons and held modifiers between invocations', async () => {
  await withFake({}, async (fake, pw) => {
    const steps = [
      [['keydown', 'Control'], "await page.keyboard.down('Control');"],
      [
        ['mousemove', '10.5', '20'],
        '// Move mouse to (10.5, 20)\nawait page.mouse.move(10.5, 20);',
      ],
      [
        ['mousedown', 'right'],
        "// Press mouse down\nawait page.mouse.down({\n  button: 'right'\n});",
      ],
      [
        ['mousewheel', '--', '0', '-100'],
        '// Scroll mouse wheel\nawait page.mouse.wheel(0, -100);',
      ],
      [['mouseup', 'right'], "// Press mouse up\nawait page.mouse.up({\n  button: 'right'\n});"],
      [['keyup', 'Control'], "await page.keyboard.up('Control');"],
      [['mousedown'], '// Press mouse down\nawait page.mouse.down();'],
      [
        ['mousemove', '1e21', '0x10'],
        '// Move mouse to (1e+21, 16)\nawait page.mouse.move(1e+21, 16);',
      ],
      [['mouseup'], '// Press mouse up\nawait page.mouse.up();'],
    ];
    for (const [args, expected] of steps) {
      const result = await pw(args);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      assert.equal(result.stdout, code(expected));
    }
    assert.deepEqual(params(fake, 'Input.dispatchMouseEvent'), [
      { type: 'mouseMoved', button: 'none', buttons: 0, x: 10.5, y: 20, modifiers: 2, force: 0 },
      {
        type: 'mousePressed',
        button: 'right',
        buttons: 2,
        x: 10.5,
        y: 20,
        modifiers: 2,
        clickCount: 1,
        force: 0.5,
      },
      { type: 'mouseWheel', x: 10.5, y: 20, modifiers: 2, deltaX: 0, deltaY: -100 },
      {
        type: 'mouseReleased',
        button: 'right',
        buttons: 0,
        x: 10.5,
        y: 20,
        modifiers: 2,
        clickCount: 1,
      },
      {
        type: 'mousePressed',
        button: 'left',
        buttons: 1,
        x: 10.5,
        y: 20,
        modifiers: 0,
        clickCount: 1,
        force: 0.5,
      },
      { type: 'mouseMoved', button: 'left', buttons: 1, x: 1e21, y: 16, modifiers: 0, force: 0.5 },
      {
        type: 'mouseReleased',
        button: 'left',
        buttons: 0,
        x: 1e21,
        y: 16,
        modifiers: 0,
        clickCount: 1,
      },
    ]);
    const hits = params(fake, 'DOM.getNodeForLocation');
    assert.deepEqual(
      hits.slice(0, 2).map((hit) => [hit.x, hit.y]),
      [
        [10, 20],
        [11, 20],
      ]
    );
    assert.ok(hits.every((hit) => hit.includeUserAgentShadowDOM === false));
  });
});

test('pointer input over an embedded frame or outside the main document is refused', async () => {
  for (const script of [
    { hitNode: { nodeType: 1, nodeName: 'IFRAME', localName: 'iframe' } },
    { hitNode: { nodeType: 1, nodeName: 'EMBED', localName: 'embed' } },
    { hitNode: { nodeType: 1, nodeName: 'FENCEDFRAME' } },
    { hit: { backendNodeId: 5, frameId: 'CHILD' } },
    { hit: { frameId: 'TAB' } },
    {
      intercept: (message, _reply, fail) => {
        if (message.method !== 'DOM.getNodeForLocation') return false;
        fail('Target closed');
        return true;
      },
    },
  ]) {
    await withFake(script, async (fake, pw) => {
      for (const args of [
        ['mousemove', '3', '4'],
        ['mousedown'],
        ['mouseup', 'middle'],
        ['mousewheel', '0', '5'],
      ]) {
        const result = await pw(args);
        assert.equal(result.code, 1, `${JSON.stringify(script)} ${args.join(' ')}`);
        assert.equal(result.stdout, '');
      }
      assert.equal(sent(fake, 'Input.dispatchMouseEvent').length, 0, JSON.stringify(script));
    });
  }
  await withFake({ hitNode: { nodeType: 1, nodeName: 'IFRAME' } }, async (_fake, pw) => {
    const result = await pw(['mousemove', '3', '4']);
    assert.equal(
      result.stderr,
      'slicc: refused: mouse input at (3, 4) would reach an embedded frame\n'
    );
  });
  await withFake(
    { metrics: { cssLayoutViewport: { pageX: 100.5, pageY: 40 } } },
    async (fake, pw) => {
      const result = await pw(['mousemove', '10', '20']);
      assert.equal(result.code, 0, result.stderr);
      assert.deepEqual(
        params(fake, 'DOM.getNodeForLocation').map((hit) => [hit.x, hit.y]),
        [
          [110, 60],
          [111, 60],
        ]
      );
      assert.deepEqual(params(fake, 'Input.dispatchMouseEvent')[0].x, 10);
    }
  );
  for (const metrics of [{}, { cssLayoutViewport: { pageX: 0 } }]) {
    await withFake({ metrics }, async (fake, pw) => {
      const result = await pw(['mousemove', '10', '20']);
      assert.equal(result.code, 1);
      assert.equal(params(fake, 'DOM.getNodeForLocation').length, 0);
      assert.equal(params(fake, 'Input.dispatchMouseEvent').length, 0);
    });
  }
  await withFake({ miss: true }, async (fake, pw) => {
    const result = await pw(['mousemove', '5000', '5000']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(sent(fake, 'Input.dispatchMouseEvent').length, 1);
  });
});

test('mousewheel is refused in a background tab instead of waiting for an answer Chrome never sends', async () => {
  await withFake({ visibility: 'hidden' }, async (fake, pw) => {
    const result = await pw(['mousewheel', '0', '100']);
    assert.equal(result.code, 1);
    assert.equal(
      result.stderr,
      'Error: mousewheel needs the tab in the foreground, because Chrome never answers wheel events in a background tab. Run "playwright-cli tab-select <index> --foreground" first.\n'
    );
    assert.equal(sent(fake, 'Input.dispatchMouseEvent').length, 0);
  });
});

test('keyboard input is refused while focus is inside a frame or a closed shadow root', async () => {
  for (const script of [
    { focusNode: { nodeType: 1, nodeName: 'IFRAME', localName: 'iframe' } },
    { focusNode: { nodeType: 1, nodeName: 'OBJECT' } },
    {
      focusNode: {
        nodeType: 1,
        nodeName: 'X-WIDGET',
        localName: 'x-widget',
        shadowRoots: [{ nodeType: 11, shadowRootType: 'closed' }],
      },
    },
    { focus: { type: 'string', value: 'fake-eval' } },
    { focus: { type: 'object', subtype: 'node' } },
    { focusNode: { nodeType: 1, nodeName: 'X-ODD', shadowRoots: [{ nodeType: 11 }] } },
  ]) {
    await withFake(script, async (fake, pw) => {
      for (const command of ['keydown', 'keyup']) {
        const result = await pw([command, 'Enter']);
        assert.equal(result.code, 1, `${JSON.stringify(script)} ${command}`);
        assert.equal(
          result.stderr,
          'slicc: refused: keyboard focus is inside an embedded frame or a closed shadow root\n'
        );
      }
      assert.equal(sent(fake, 'Input.dispatchKeyEvent').length, 0);
    });
  }
  for (const script of [
    { focus: { type: 'object', subtype: 'null', value: null } },
    {
      focusNode: {
        nodeType: 1,
        nodeName: 'X-OPEN',
        shadowRoots: [{ nodeType: 11, shadowRootType: 'open' }],
      },
    },
    {
      focusNode: {
        nodeType: 1,
        nodeName: 'INPUT',
        localName: 'input',
        shadowRoots: [{ nodeType: 11, shadowRootType: 'user-agent' }],
      },
    },
  ]) {
    await withFake(script, async (fake, pw) => {
      const result = await pw(['keydown', 'Enter']);
      assert.equal(result.code, 0, result.stderr);
      assert.equal(sent(fake, 'Input.dispatchKeyEvent')[0].params.text, '\r');
    });
  }
});

test('argument errors match Microsoft before any browser is touched', async () => {
  await withFake({}, async (fake, pw) => {
    for (const [args, stderr] of [
      [
        ['mousemove'],
        "error: 'x' argument: expected number, received 'undefined'\nerror: 'y' argument: expected number, received 'undefined'\n",
      ],
      [['mousemove', 'a', '1'], "error: 'x' argument: expected number, received 'a'\n"],
      [['mousewheel', '1', 'Infinity'], "error: 'dy' argument: expected number, received number\n"],
      [['resize', '800'], "error: 'h' argument: expected number, received 'undefined'\n"],
      [
        ['resize', '800.5', '600'],
        'Error: resize needs whole widths and heights from 1 to 10000000\n',
      ],
      [['resize', '0', '600'], 'Error: resize needs whole widths and heights from 1 to 10000000\n'],
      [['keydown'], "error: 'key' argument: expected string, received undefined\n"],
      [['keyup', 'Shift+a'], 'Error: Unknown key: "Shift+a"\n'],
      [
        ['mousedown', 'back'],
        'Invalid arguments for tool "browser_mouse_down":\n\u2716 Invalid option: expected one of "left"|"right"|"middle"\n  \u2192 at button\n',
      ],
      [
        ['mouseup', 'forward'],
        'Invalid arguments for tool "browser_mouse_up":\n\u2716 Invalid option: expected one of "left"|"right"|"middle"\n  \u2192 at button\n',
      ],
      [['drop'], "error: 'target' argument: expected string, received undefined\n"],
      [['drop', 'e2'], 'Error: At least one of "paths" or "data" must be provided.\n'],
      [
        ['drop', 'e2', '--data', 'plain'],
        '--data must be in "mime/type=value" format, got: plain\n',
      ],
    ]) {
      const result = await pw(args);
      assert.equal(result.code, 1, args.join(' '));
      assert.equal(result.stdout, '');
      assert.equal(result.stderr, stderr, args.join(' '));
    }
    const result = await pw(['mousemove', '1', '2', '3']);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, 'error: too many arguments: expected 2, received 3\n');
    assert.equal(fake.calls.length, 0);
  });
});

test('resize sizes the window around the measured viewport', async () => {
  await withFake({ inner: [880, 620] }, async (fake, pw) => {
    const result = await pw(['resize', '1024', '768']);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(result.stdout, code('await page.setViewportSize({ width: 1024, height: 768 });'));
    assert.deepEqual(params(fake, 'Browser.setWindowBounds'), [
      { windowId: 3, bounds: { width: 1044, height: 848 } },
    ]);
    const windows = sent(fake, 'Browser.getWindowForTarget');
    assert.ok(windows.length >= 2);
    assert.ok(windows.every((call) => call.sessionId && call.params.targetId === undefined));
  });
});

test('resize is refused while a protected or unverified tab shares the browser profile', async () => {
  for (const url of [
    'https://seven.sliccy.ai/?secret=hidden',
    'http://vault.sliccy.ai/',
    '',
    'chrome://settings/',
  ]) {
    await withFake({}, async (fake, pw) => {
      fake.targets.push({ targetId: 'OTHER', url });
      const result = await pw(['resize', '1024', '768', '--tab', 'TAB']);
      assert.equal(result.code, 1, url);
      assert.match(result.stderr, /refused/);
      assert.ok(!result.stderr.includes('hidden'));
      assert.equal(sent(fake, 'Browser.setWindowBounds').length, 0, url);
      assert.deepEqual(
        fake.calls
          .filter(
            (call) =>
              call.method.startsWith('Browser.') && (call.params?.targetId || call.target !== 'TAB')
          )
          .map((call) => call.method),
        []
      );
    });
  }
  const elsewhere = {
    intercept: (message, reply) => {
      if (message.method !== 'Target.getTargets') return false;
      reply({
        targetInfos: [
          {
            targetId: 'TAB',
            type: 'page',
            url: 'https://example.test/',
            title: '',
            attached: true,
            browserContextId: 'CTX',
          },
          {
            targetId: 'SEVEN',
            type: 'page',
            url: 'https://seven.sliccy.ai/',
            title: '',
            attached: false,
            browserContextId: 'OTHER',
          },
          {
            targetId: 'SW',
            type: 'service_worker',
            url: 'https://seven.sliccy.ai/sw.js',
            title: '',
            attached: false,
            browserContextId: 'CTX',
          },
        ],
      });
      return true;
    },
  };
  await withFake(elsewhere, async (fake, pw) => {
    const result = await pw(['resize', '640', '480']);
    assert.equal(result.code, 0, result.stderr);
    assert.deepEqual(params(fake, 'Browser.setWindowBounds'), [
      { windowId: 3, bounds: { width: 740, height: 580 } },
    ]);
  });
});

test('drop reads only the named files and dispatches synthetic drag events like Microsoft', async () => {
  await withFake({}, async (fake, pw, home) => {
    await writeFile(path.join(home, 'a.txt'), 'drop me');
    await writeFile(path.join(home, 'b.png'), Buffer.from([1, 2, 3]));
    await writeFile(path.join(home, 'unnamed.txt'), 'UNNAMED-SECRET');
    const snapshot = await pw(['snapshot']);
    assert.equal(snapshot.code, 0, snapshot.stderr);
    const result = await pw([
      'drop',
      'e2',
      '--path',
      'a.txt',
      '--data',
      "text/plain=it's",
      '--data',
      'text/uri-list=https://example.test/',
    ]);
    assert.equal(result.code, 0, result.stderr);
    assert.equal(
      result.stdout,
      code(
        `await page.locator('aria-ref=e2').drop({\n  data: {\n  text/plain: 'it\\'s',\n  text/uri-list: 'https://example.test/'\n},\n  files: '${path.join(home, 'a.txt')}'\n});`
      )
    );
    const dropped = params(fake, 'Runtime.callFunctionOn').filter((call) =>
      call.functionDeclaration.includes('DataTransfer')
    );
    assert.equal(dropped.length, 1);
    const [payloads, entries] = dropped[0].arguments.map((arg) => arg.value);
    assert.deepEqual(
      payloads.map(({ name, mimeType, buffer }) => [
        name,
        mimeType,
        Buffer.from(buffer, 'base64').toString(),
      ]),
      [['a.txt', 'text/plain', 'drop me']]
    );
    assert.equal(typeof payloads[0].lastModifiedMs, 'number');
    assert.deepEqual(entries, [
      { mimeType: 'text/plain', value: "it's" },
      { mimeType: 'text/uri-list', value: 'https://example.test/' },
    ]);
    assert.ok(
      !JSON.stringify(fake.calls).includes(Buffer.from('UNNAMED-SECRET').toString('base64'))
    );
    await pw(['snapshot']);
    const many = await pw(['drop', 'e2', '--path', 'a.txt', '--path', `${home}/sub/../b.png`]);
    assert.equal(many.code, 0, many.stderr);
    assert.ok(
      many.stdout.includes(`files: ['${path.join(home, 'a.txt')}', '${path.join(home, 'b.png')}']`)
    );
  });
});

test('drop refusals and file errors exit non-zero without dispatching', async () => {
  await withFake({ drop: 'not-accepted' }, async (fake, pw) => {
    await pw(['snapshot']);
    const result = await pw(['drop', 'e2', '--data', 'text/plain=x']);
    assert.equal(result.code, 1);
    assert.equal(result.stderr, notAccepted);
  });
  await withFake({}, async (fake, pw, home) => {
    await mkdir(path.join(home, 'dir'));
    await mkdir(path.join(home, 'other'));
    await writeFile(path.join(home, 'a.txt'), 'a');
    for (const [args, stderr] of [
      [
        ['--path', 'missing.txt'],
        `Error: ENOENT: no such file or directory, stat '${path.join(home, 'missing.txt')}'\n`,
      ],
      [
        ['--path', 'dir'],
        'Error: Dropping a directory is not supported \u2014 pass individual files.\n',
      ],
      [['--path', 'dir', '--path', 'other'], 'Error: Multiple directories are not supported\n'],
      [
        ['--path', 'a.txt', '--path', 'dir'],
        'Error: File paths must be all files or a single directory\n',
      ],
    ]) {
      await pw(['snapshot']);
      const result = await pw(['drop', 'e2', ...args]);
      assert.equal(result.code, 1, args.join(' '));
      assert.equal(result.stderr, stderr);
    }
    const unknown = await pw(['drop', 'e9', '--data', 'text/plain=x']);
    assert.equal(unknown.code, 1);
    assert.match(unknown.stderr, /Unknown ref "e9"|No snapshot/);
    assert.equal(
      params(fake, 'Runtime.callFunctionOn').filter((call) =>
        call.functionDeclaration.includes('DataTransfer')
      ).length,
      0
    );
  });
});

test('kernel input commands send the same CDP and print the same text', async () => {
  const script = { inner: [880, 620] };
  const fake = await fakeCdp({ intercept: page(script) });
  fake.targets.push({ targetId: 'TAB', url: 'https://example.test/' });
  let runner;
  try {
    runner = await kernelRunner(
      path.join(wasm, 'playwright-cli.wasm'),
      path.join(wasm, 'curlwright.wasm'),
      fake.url
    );
    const cwd = await runner.fresh();
    await runner.write(cwd, 'a.txt', 'kernel drop');
    for (const [args, expected] of [
      [['keydown', 'Shift'], "await page.keyboard.down('Shift');"],
      [['mousemove', '7', '8'], '// Move mouse to (7, 8)\nawait page.mouse.move(7, 8);'],
      [['mousedown'], '// Press mouse down\nawait page.mouse.down();'],
      [['mouseup'], '// Press mouse up\nawait page.mouse.up();'],
      [['mousewheel', '0', '40'], '// Scroll mouse wheel\nawait page.mouse.wheel(0, 40);'],
      [['keyup', 'Shift'], "await page.keyboard.up('Shift');"],
      [['resize', '1024', '768'], 'await page.setViewportSize({ width: 1024, height: 768 });'],
    ]) {
      const result = await runner.playwright(args, cwd);
      assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
      assert.equal(result.stdout, code(expected));
    }
    assert.deepEqual(
      params(fake, 'Input.dispatchMouseEvent').map((event) => [
        event.type,
        event.x,
        event.y,
        event.modifiers,
      ]),
      [
        ['mouseMoved', 7, 8, 8],
        ['mousePressed', 7, 8, 8],
        ['mouseReleased', 7, 8, 8],
        ['mouseWheel', 7, 8, 8],
      ]
    );
    assert.deepEqual(params(fake, 'Browser.setWindowBounds'), [
      { windowId: 3, bounds: { width: 1044, height: 848 } },
    ]);
    const snapshot = await runner.playwright(['snapshot'], cwd);
    assert.equal(snapshot.code, 0, snapshot.stderr);
    const dropped = await runner.playwright(['drop', 'e2', '--path', 'a.txt'], cwd);
    assert.equal(dropped.code, 0, dropped.stderr);
    assert.equal(
      dropped.stdout,
      code(`await page.locator('aria-ref=e2').drop({\n  files: '${cwd}/a.txt'\n});`)
    );
    const call = params(fake, 'Runtime.callFunctionOn').find((item) =>
      item.functionDeclaration.includes('DataTransfer')
    );
    assert.equal(
      Buffer.from(call.arguments[0].value[0].buffer, 'base64').toString(),
      'kernel drop'
    );
    fake.targets.push({ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/' });
    const refused = await runner.playwright(['resize', '800', '600', '--tab', 'TAB'], cwd);
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /refused/);
    assert.equal(sent(fake, 'Browser.setWindowBounds').length, 1);
  } finally {
    await Promise.resolve(runner?.close()).catch(() => {});
    await fake.close();
  }
});
