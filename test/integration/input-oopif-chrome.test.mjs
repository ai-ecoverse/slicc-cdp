import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);

const GRID = `<!doctype html>
<html><body style="margin:0">
<canvas id="grid" width="400" height="300" style="display:block;background:#e8eef7"></canvas>
<textarea id="cell" style="position:absolute;left:-9999px;top:0;width:1px;height:1px;opacity:0"></textarea>
<pre id="log" hidden>[]</pre>
<script>
const grid = document.getElementById('grid');
const cell = document.getElementById('cell');
const log = document.getElementById('log');
const note = (type, event) => {
  const rows = JSON.parse(log.textContent || '[]');
  rows.push({
    type,
    trusted: event.isTrusted,
    x: event.clientX,
    y: event.clientY,
    key: event.key || null,
    deltaY: event.deltaY || 0,
  });
  log.textContent = JSON.stringify(rows);
};
for (const type of ['pointerdown', 'mousedown', 'mouseup', 'click', 'dblclick', 'wheel', 'mousemove']) {
  grid.addEventListener(type, (event) => note(type, event));
}
for (const type of ['keydown', 'keyup', 'beforeinput', 'input']) {
  cell.addEventListener(type, (event) => note(type, event));
}
grid.addEventListener('mousedown', (event) => {
  event.preventDefault();
  cell.focus({ preventScroll: true });
});
</script>
</body></html>`;

function site() {
  let port = 0;
  const pages = {
    '/top.html': () =>
      `<h1>Sheet</h1><iframe id="app" src="http://grid.test:${port}/grid.html" width="420" height="320"></iframe>`,
    '/grid.html': () => GRID,
    '/guarded.html': () =>
      `<h1>Sheet</h1><iframe src="http://seven.sliccy.ai:${port}/os/secret.html"></iframe>`,
    '/os/secret.html': () => '<button>SECRET</button>',
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

async function childFrame(runner, cwd) {
  const listed = await runner.playwright(['frames'], cwd);
  assert.equal(listed.code, 0, listed.stderr);
  const match = listed.stdout.match(/\[child\] frameId=(\S+)/);
  assert.ok(match, listed.stdout);
  return match[1];
}

async function readLog(runner, cwd, frameId) {
  const result = await runner.playwright(
    ['eval', '--frame', frameId, '(() => document.getElementById("log")?.textContent || "[]")()'],
    cwd
  );
  if (result.code !== 0) {
    return null;
  }
  try {
    return JSON.parse(result.stdout.trim());
  } catch {
    return null;
  }
}

async function clearLog(runner, cwd, frameId) {
  const result = await runner.playwright(
    [
      'eval',
      '--frame',
      frameId,
      '(() => { const log = document.getElementById("log"); if (log) log.textContent = "[]"; return "ok"; })()',
    ],
    cwd
  );
  assert.equal(result.code, 0, result.stderr);
}

function trusted(log, type) {
  return log.some((event) => event.type === type && event.trusted === true);
}

async function iframePoint(runner, cwd) {
  const result = await runner.playwright(
    [
      'eval',
      '(() => { const r = document.getElementById("app").getBoundingClientRect(); return JSON.stringify({ x: r.x + Math.min(80, r.width / 2), y: r.y + Math.min(90, r.height / 2) }); })()',
    ],
    cwd
  );
  assert.equal(result.code, 0, result.stderr);
  return JSON.parse(result.stdout.trim());
}

async function flow(runner, port) {
  const cwd = await runner.fresh();
  const run = async (args) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}\n${result.stdout}`);
    return result;
  };
  await run(['open', '--foreground', `http://sheet.test:${port}/top.html`]);
  let shot = '';
  for (let attempt = 0; attempt < 30 && !/iframe/.test(shot); attempt += 1) {
    if (attempt) await new Promise((resolve) => setTimeout(resolve, 250));
    shot = (await run(['snapshot'])).stdout;
  }
  assert.match(shot, /iframe \[ref=e\d+\]/);
  const iframe = shot.match(/iframe \[ref=(e\d+)\]/)?.[1];
  assert.ok(iframe, shot);
  const frameId = await childFrame(runner, cwd);
  const point = await iframePoint(runner, cwd);
  const x = String(Math.round(point.x));
  const y = String(Math.round(point.y));
  const x2 = String(Math.round(point.x + 40));
  const y2 = String(Math.round(point.y + 30));

  await clearLog(runner, cwd, frameId);
  await run(['mousemove', x, y]);
  await run(['mousedown']);
  await run(['mouseup']);
  let log = await readLog(runner, cwd, frameId);
  assert.ok(log, 'could not read frame input log');
  assert.ok(
    trusted(log, 'mousedown') || trusted(log, 'pointerdown') || trusted(log, 'click'),
    JSON.stringify(log)
  );

  await clearLog(runner, cwd, frameId);
  await run(['mousemove', x, y]);
  await run(['mousedown']);
  await run(['mousemove', x2, y2]);
  await run(['mouseup']);
  log = await readLog(runner, cwd, frameId);
  assert.ok(trusted(log ?? [], 'mousemove') || trusted(log ?? [], 'mouseup'), JSON.stringify(log));

  await clearLog(runner, cwd, frameId);
  await run(['mousemove', x, y]);
  await run(['mousewheel', '0', '40']);
  log = await readLog(runner, cwd, frameId);
  assert.ok(trusted(log ?? [], 'wheel'), JSON.stringify(log));

  await clearLog(runner, cwd, frameId);
  await run(['mousemove', x, y]);
  await run(['mousedown']);
  await run(['mouseup']);
  await run(['type', 'ab']);
  await run(['press', 'Enter']);
  await run(['keydown', 'x']);
  await run(['keyup', 'x']);
  log = await readLog(runner, cwd, frameId);
  assert.ok(
    trusted(log ?? [], 'keydown') ||
      trusted(log ?? [], 'input') ||
      trusted(log ?? [], 'beforeinput'),
    JSON.stringify(log)
  );

  await clearLog(runner, cwd, frameId);
  shot = (await run(['snapshot'])).stdout;
  const iframeAgain = shot.match(/iframe \[ref=(e\d+)\]/)?.[1] ?? iframe;
  await run(['click', iframeAgain]);
  log = await readLog(runner, cwd, frameId);
  assert.ok(
    log?.some((event) => event.trusted),
    JSON.stringify(log)
  );

  await clearLog(runner, cwd, frameId);
  await run(['mousemove', x, y]);
  await run(['mousedown']);
  await run(['mouseup']);
  await run(['mousedown']);
  await run(['mouseup']);
  log = await readLog(runner, cwd, frameId);
  assert.ok(trusted(log ?? [], 'dblclick') || trusted(log ?? [], 'click'), JSON.stringify(log));

  await run(['goto', `http://sheet.test:${port}/guarded.html`]);
  await new Promise((resolve) => setTimeout(resolve, 1000));
  const blocked = await runner.playwright(['mousemove', '80', '90'], cwd);
  assert.notEqual(blocked.code, 0);
  assert.match(blocked.stderr, /slicc: refused:/);
  assert.doesNotMatch(blocked.stdout + blocked.stderr, /SECRET|secret\.html|\/os\//);
}

test('Excel-like cross-site canvas frame takes clicks, drag, wheel and typing', async (t) => {
  const server = await site();
  let browser;
  let kernel;
  try {
    browser = await launchBrowser([
      '--site-per-process',
      `--host-resolver-rules=MAP sheet.test 127.0.0.1:${server.port}, MAP grid.test 127.0.0.1:${server.port}, MAP seven.sliccy.ai 127.0.0.1:${server.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => flow(native, server.port));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('kernel', () => flow(kernel, server.port));
  } finally {
    await settle(kernel?.close());
    await settle(browser?.close());
    await settle(server.close());
  }
});
