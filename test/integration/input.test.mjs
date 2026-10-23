import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);
const code = (text) => `### Ran Playwright code\n\`\`\`js\n${text}\n\`\`\`\n`;
const notAccepted =
  'Error: Drop target did not accept the drop \u2014 its dragover handler did not call preventDefault()\n';

const listen = `
window.events = [];
for (const type of ['keydown', 'keyup'])
  addEventListener(type, (e) => events.push({ type, key: e.key, code: e.code, shift: e.shiftKey, repeat: e.repeat }), true);
for (const type of ['mousedown', 'mouseup', 'mousemove'])
  addEventListener(type, (e) => events.push({ type, x: e.clientX, y: e.clientY, button: e.button, buttons: e.buttons, shift: e.shiftKey }), true);
addEventListener('wheel', (e) => events.push({ type: 'wheel', dx: e.deltaX, dy: e.deltaY }), { capture: true, passive: true });`;

const pages = {
  '/input': () => `<title>Input</title>
<style>body{margin:0}#pad{position:absolute;left:0;top:0;width:400px;height:300px;overflow:scroll;background:#eee}#tall{height:4000px}#field{position:absolute;left:0;top:320px}#zone{position:absolute;left:0;top:400px;width:200px;height:80px}#refuses{position:absolute;left:300px;top:400px;width:200px;height:80px}</style>
<div id="pad"><div id="tall"></div></div>
<input id="field" aria-label="Field">
<div id="zone" role="region" aria-label="Drop zone">Drop here</div>
<div id="refuses" role="region" aria-label="Refuses">No drops</div>
<script>${listen}
const zone = document.getElementById('zone');
zone.addEventListener('dragover', (e) => e.preventDefault());
zone.addEventListener('drop', async (e) => {
  e.preventDefault();
  const files = await Promise.all([...e.dataTransfer.files].map(async (f) => ({ name: f.name, type: f.type, body: await f.text() })));
  window.dropped = { text: e.dataTransfer.getData('text/plain'), files, trusted: e.isTrusted };
});
document.getElementById('field').focus();
</script>`,
  '/framed': () => `<title>Framed</title>
<style>body{margin:0}iframe{position:absolute;left:500px;top:300px;width:200px;height:200px;border:0}#spacer{height:3000px}</style>
<p>Outside</p>
<div id="spacer"></div>
<iframe src="http://seven.sliccy.ai/frame?k=FRAME_SECRET" onload="this.dataset.loaded = 'yes'"></iframe>
<script>${listen}</script>`,
  '/frame': () => '<title>Seven</title><input aria-label="Seven field">',
};

function site() {
  const server = createServer((req, res) => {
    const page = pages[new URL(req.url ?? '/', 'http://127.0.0.1').pathname];
    res.writeHead(page ? 200 : 404, { 'content-type': 'text/html' });
    res.end(page ? `<!doctype html>${page()}` : 'missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      resolve({
        port: server.address().port,
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
  const deadline = Date.now() + 10000;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (accept(last)) return last;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  assert.fail(`${label}: ${JSON.stringify(last)}`);
}

async function flow(runner, raw, port) {
  const cwd = await runner.fresh();
  const pw = (args) => runner.playwright(args, cwd);
  const run = async (args, expected) => {
    const result = await pw(args);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
    if (expected !== undefined) assert.equal(result.stdout, code(expected), args.join(' '));
    return result;
  };
  const value = async (expression) =>
    JSON.parse((await run(['eval', `JSON.stringify(${expression})`])).stdout);
  const drain = () => value('window.events.splice(0)');
  const sevenGone = async () => {
    const { targetInfos } = await raw.send('Target.getTargets');
    return !targetInfos.some((info) => info.url.includes('seven.sliccy.ai'));
  };

  await run(['open', '--foreground', `http://127.0.0.1:${port}/input`]);
  await drain();
  for (const [command, key] of [
    ['keydown', 'Shift'],
    ['keydown', 'KeyA'],
    ['keydown', 'KeyA'],
    ['keyup', 'KeyA'],
    ['keyup', 'Shift'],
  ])
    await run(
      [command, key],
      `await page.keyboard.${command === 'keydown' ? 'down' : 'up'}('${key}');`
    );
  assert.deepEqual(
    (await drain()).filter((event) => event.type.startsWith('key')),
    [
      { type: 'keydown', key: 'Shift', code: 'ShiftLeft', shift: true, repeat: false },
      { type: 'keydown', key: 'A', code: 'KeyA', shift: true, repeat: false },
      { type: 'keydown', key: 'A', code: 'KeyA', shift: true, repeat: true },
      { type: 'keyup', key: 'A', code: 'KeyA', shift: true, repeat: false },
      { type: 'keyup', key: 'Shift', code: 'ShiftLeft', shift: false, repeat: false },
    ]
  );

  await run(['keydown', 'Shift']);
  await run(['mousemove', '50', '60'], '// Move mouse to (50, 60)\nawait page.mouse.move(50, 60);');
  await run(['mousedown'], '// Press mouse down\nawait page.mouse.down();');
  await run(['mousemove', '80', '90']);
  await run(['mouseup'], '// Press mouse up\nawait page.mouse.up();');
  await run(['keyup', 'Shift']);
  const pointer = (await drain()).filter((event) => event.type.startsWith('mouse'));
  assert.deepEqual(
    pointer.filter((event) => event.type !== 'mousemove'),
    [
      { type: 'mousedown', x: 50, y: 60, button: 0, buttons: 1, shift: true },
      { type: 'mouseup', x: 80, y: 90, button: 0, buttons: 0, shift: true },
    ]
  );
  assert.ok(
    pointer.some(
      (event) =>
        event.type === 'mousemove' &&
        event.x === 50 &&
        event.y === 60 &&
        event.buttons === 0 &&
        event.shift
    )
  );
  assert.ok(
    pointer.some(
      (event) =>
        event.type === 'mousemove' && event.x === 80 && event.y === 90 && event.buttons === 1
    )
  );

  await run(['mousemove', '100', '100']);
  await run(['mousewheel', '0', '200'], '// Scroll mouse wheel\nawait page.mouse.wheel(0, 200);');
  await until(
    () => value("document.getElementById('pad').scrollTop"),
    (top) => top > 0,
    'the wheel scrolled the element under the pointer'
  );
  assert.ok((await drain()).some((event) => event.type === 'wheel' && event.dy === 200));
  const sevenTab = await raw.open('http://seven.sliccy.ai/frame?k=WHEEL_SECRET');
  const background = await raw.open(`http://127.0.0.1:${port}/input`);
  try {
    const on = async (expression) => {
      const { sessionId } = await raw.send('Target.attachToTarget', {
        targetId: background,
        flatten: true,
      });
      try {
        const { result } = await raw.send(
          'Runtime.evaluate',
          { expression, returnByValue: true },
          sessionId
        );
        return result.value;
      } finally {
        await raw.send('Target.detachFromTarget', { sessionId }).catch(() => {});
      }
    };
    assert.equal(await on('document.visibilityState'), 'hidden');
    await run(['mousemove', '100', '100', '--tab', background]);
    const started = Date.now();
    await run(
      ['mousewheel', '0', '200', '--tab', background],
      '// Scroll mouse wheel\nawait page.mouse.wheel(0, 200);'
    );
    assert.ok(Date.now() - started < 9000, 'a background wheel does not wait for Chrome');
    assert.ok(
      (await on("document.getElementById('pad').scrollTop")) > 0,
      'the element under the pointer scrolled'
    );
    assert.equal(await on('window.scrollY'), 0);
    assert.equal(await on('document.visibilityState'), 'hidden');
    assert.ok(
      (await on('window.events.splice(0)')).some(
        (event) => event.type === 'wheel' && event.dy === 200
      ),
      'the page saw the wheel event'
    );
    const { targetInfos } = await raw.send('Target.getTargets');
    assert.ok(targetInfos.some((info) => info.targetId === sevenTab));
  } finally {
    await raw.send('Target.closeTarget', { targetId: background });
    await raw.send('Target.closeTarget', { targetId: sevenTab });
  }
  await until(sevenGone, Boolean, 'protected tab gone after the wheel check');
  await runner.write(cwd, 'note.txt', 'dropped bytes');
  const shot = (await run(['snapshot'])).stdout;
  const zone = shot.match(/region "Drop zone" \[ref=(e\d+)\]/)?.[1];
  const refuses = shot.match(/region "Refuses" \[ref=(e\d+)\]/)?.[1];
  assert.ok(zone && refuses, shot);
  const dropped = await run(['drop', zone, '--path', 'note.txt', '--data', 'text/plain=hello']);
  assert.match(
    dropped.stdout,
    new RegExp(
      `^### Ran Playwright code\\n\`\`\`js\\nawait page\\.locator\\('aria-ref=${zone}'\\)\\.drop\\(\\{\\n  data: \\{\\n  text/plain: 'hello'\\n\\},\\n  files: '[^']*note\\.txt'\\n\\}\\);\\n\`\`\`\\n$`
    )
  );
  assert.deepEqual(await until(() => value('window.dropped ?? null'), Boolean, 'drop handler'), {
    text: 'hello',
    files: [{ name: 'note.txt', type: 'text/plain', body: 'dropped bytes' }],
    trusted: false,
  });
  await run(['snapshot']);
  const rejected = await pw(['drop', refuses, '--data', 'text/plain=x']);
  assert.equal(rejected.code, 1);
  assert.equal(rejected.stderr, notAccepted);

  await run(['goto', `http://127.0.0.1:${port}/framed`]);
  await until(
    () => value("document.querySelector('iframe').dataset.loaded || 'no'"),
    (loaded) => loaded === 'yes',
    'protected frame loaded'
  );
  await until(sevenGone, (gone) => !gone, 'protected frame target listed');
  assert.equal(await value('(scrollTo(0, 250), scrollY)'), 250);
  const over = await pw(['mousemove', '600', '100']);
  assert.equal(over.code, 1);
  assert.equal(
    over.stderr,
    'slicc: refused: mouse input at (600, 100) would reach an embedded frame\n'
  );
  await run(['mousemove', '10', '10']);
  assert.equal(
    (
      await run([
        'eval',
        "document.querySelector('iframe').focus(); document.activeElement.localName",
      ])
    ).stdout.trim(),
    'iframe'
  );
  const typed = await pw(['keydown', 'a']);
  assert.equal(typed.code, 1);
  assert.match(typed.stderr, /keyboard focus is inside an embedded frame/);
  const framedResize = await pw(['resize', '900', '650']);
  assert.equal(framedResize.code, 1);
  assert.match(framedResize.stderr, /refused/);
  for (const output of [over, typed, framedResize])
    assert.doesNotMatch(output.stdout + output.stderr, /FRAME_SECRET/);
  assert.ok(
    (await drain()).every((event) => !(event.type === 'mousemove' && event.x === 600)),
    'refused input never reached the page'
  );

  await run(['goto', `http://127.0.0.1:${port}/input`]);
  await until(sevenGone, Boolean, 'protected frame target gone');
  await run(['resize', '1000', '700'], 'await page.setViewportSize({ width: 1000, height: 700 });');
  await until(
    () => value('[innerWidth, innerHeight]'),
    (size) => size[0] === 1000 && size[1] === 700,
    'viewport resized'
  );
  const seven = await raw.open('http://seven.sliccy.ai/frame?k=TAB_SECRET');
  try {
    const refused = await pw(['resize', '800', '600']);
    assert.equal(refused.code, 1);
    assert.match(refused.stderr, /refused/);
    assert.doesNotMatch(refused.stderr, /TAB_SECRET/);
    assert.deepEqual(await value('[innerWidth, innerHeight]'), [1000, 700]);
  } finally {
    await raw.send('Target.closeTarget', { targetId: seven });
  }
  await until(sevenGone, Boolean, 'protected tab gone');
}

test('input commands drive Chrome like Playwright and refuse protected frames and tabs', async (t) => {
  const server = await site();
  let browser;
  let raw;
  let kernel;
  try {
    browser = await launchBrowser([
      '--site-per-process',
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${server.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => flow(native, raw, server.port));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('kernel', () => flow(kernel, raw, server.port));
  } finally {
    raw?.close();
    await settle(kernel?.close());
    await settle(browser?.close());
    await settle(server.close());
  }
});
