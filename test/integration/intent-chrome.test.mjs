import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const native = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.join(root, 'target/wasm32-wasip1/debug', `${name}.wasm`);

const formPage = `<!doctype html><title>Order</title>
<button onclick="fetch('/api/order')">Order</button>
<input placeholder="Name">`;

const watchedPage = `<!doctype html><title>Watched</title><button>Bait</button><script>
window.__changes = [];
new MutationObserver((records) => {
  for (const record of records) window.__changes.push(record.type + ':' + (record.target.localName ?? '') + ':' + [...record.addedNodes].map((node) => node.nodeName).join(','));
}).observe(document, { childList: true, subtree: true, attributes: true, characterData: true });
</script>`;

function site() {
  const server = createServer((req, res) => {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    const send = (type, body) => {
      res.writeHead(200, { 'content-type': type });
      res.end(body);
    };
    if (url.pathname === '/form') return send('text/html', formPage);
    if (url.pathname === '/watched') return send('text/html', watchedPage);
    if (url.pathname.startsWith('/api/')) return send('text/plain', 'ok');
    res.writeHead(404);
    res.end();
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

const ok = (result, label) =>
  assert.equal(result.code, 0, `${label}\n${result.stdout}\n${result.stderr}`);
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function refOf(stdout, role, name) {
  const found = stdout.match(new RegExp(`${role} "${name}"(?: \\[[^\\]]+\\])*? \\[ref=(e\\d+)\\]`));
  assert.ok(found, `${role} "${name}" in\n${stdout}`);
  return found[1];
}

async function withIntentEnv(value, work) {
  const before = process.env.SLICC_INTENT;
  process.env.SLICC_INTENT = value;
  try {
    return await work();
  } finally {
    if (before === undefined) delete process.env.SLICC_INTENT;
    else process.env.SLICC_INTENT = before;
  }
}

function holder(wsUrl, args, cwd) {
  const env = { ...process.env };
  delete env.SLICC_CDP_URL;
  delete env.SLICC_INTENT;
  const child = spawn(native('playwright-cli'), ['--cdp', wsUrl, ...args], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stdout = '';
  let stderr = '';
  const waiting = [];
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    for (const wake of waiting.splice(0)) wake();
  });
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const timer = setTimeout(() => child.kill('SIGKILL'), 120000);
  const ended = new Promise((resolve) => {
    child.on('close', (code) => {
      clearTimeout(timer);
      for (const wake of waiting.splice(0)) wake();
      resolve({ code, stdout, stderr });
    });
  });
  const ready = async () => {
    for (;;) {
      if (stdout.split('\n').some((line) => line.includes('"event":"ready"'))) return;
      if (child.exitCode !== null) throw new Error(`holder exited before ready: ${stderr}`);
      await new Promise((resolve) => waiting.push(resolve));
    }
  };
  return { child, ended, ready };
}

async function opened(runner, cwd, url) {
  const result = await runner.playwright(['open', url, '--foreground'], cwd);
  ok(result, `open ${url}`);
  const tab = result.stdout.match(/targetId: ([0-9A-F]+)/)?.[1];
  assert.ok(tab, result.stdout);
  return tab;
}

async function untouchedFlow(runner, server, raw) {
  const cwd = await runner.fresh();
  const tab = await opened(runner, cwd, `${server.origin}/watched`);
  const pw = (args) => runner.playwright([...args, '--tab', tab], cwd);
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId: tab, flatten: true });
  const read = async () =>
    (
      await raw.send(
        'Runtime.evaluate',
        {
          expression:
            '({ changes: window.__changes, children: document.documentElement.children.length, html: document.documentElement.outerHTML })',
          returnByValue: true,
        },
        sessionId
      )
    ).result.value;
  const before = await read();
  const plain = await pw(['snapshot']);
  ok(plain, 'snapshot');
  const labelled = await pw(['snapshot', '--intent', 'Reading the watched page']);
  ok(labelled, 'snapshot --intent');
  assert.equal(labelled.stdout, plain.stdout);
  const bait = refOf(plain.stdout, 'button', 'Bait');
  ok(await pw(['hover', bait, '--intent', 'Pointing at the bait']), 'hover');
  ok(await pw(['eval', '1', '--intent', 'Evaluating nothing']), 'eval');
  ok(await pw(['screenshot', '--filename', 'shot.png', '--intent', 'Capturing']), 'screenshot');
  await pause(300);
  const after = await read();
  assert.deepEqual(after, before);
  assert.doesNotMatch(after.html, /slicc|Reading|Pointing|Evaluating|Capturing/);
}

async function nativeFlows(t, runner, server, browser) {
  const cwd = await runner.fresh();
  const tab = await opened(runner, cwd, `${server.origin}/form`);
  const pw = (args) => runner.playwright([...args, '--tab', tab], cwd);

  await t.test('a recording carries each action’s intent as a comment', async () => {
    const held = holder(browser.wsUrl, ['recording-start', '--tab', tab], cwd);
    await held.ready();
    let snapshot = await pw(['snapshot']);
    ok(
      await pw([
        'click',
        refOf(snapshot.stdout, 'button', 'Order'),
        '--intent',
        'Placing the order',
      ]),
      'click'
    );
    snapshot = await pw(['snapshot']);
    ok(
      await pw([
        'fill',
        refOf(snapshot.stdout, 'textbox', 'Name'),
        'Lars',
        '--intent',
        'Typing the name',
      ]),
      'fill'
    );
    ok(await pw(['press', 'Tab']), 'press');
    await pause(700);
    const stopped = await runner.playwright(['recording-stop'], cwd);
    ok(stopped, 'recording-stop');
    assert.ok(
      stopped.stdout.includes(
        "// Placing the order\nawait page.getByRole('button', { name: 'Order' }).click();\n"
      ),
      stopped.stdout
    );
    assert.match(
      stopped.stdout,
      /\n\/\/ Typing the name\n(?:await page\.getByRole\('textbox', \{ name: 'Name' \}\)\.click\(\);\n)?await page\.getByRole\('textbox', \{ name: 'Name' \}\)\.fill\('Lars'\);\nawait page\.getByRole\('textbox', \{ name: 'Name' \}\)\.press\('Tab'\);\n/
    );
    assert.equal(stopped.stdout.match(/^\/\/ /gm).length, 2, stopped.stdout);
    assert.equal((await held.ended).code, 0);
  });

  await t.test('HAR entries carry the intent of the command that caused them', async () => {
    const file = path.join(cwd, 'intent.har');
    const held = holder(browser.wsUrl, ['har-start', file, '--tab', tab], cwd);
    await held.ready();
    const fetchTo = (name) => `fetch('/api/${name}').then((r) => r.status)`;
    const snapshot = await pw(['snapshot']);
    ok(
      await pw([
        'click',
        refOf(snapshot.stdout, 'button', 'Order'),
        '--intent',
        'Placing the order',
      ]),
      'click'
    );
    await pause(300);
    ok(await pw(['eval', fetchTo('late')]), 'late');
    await withIntentEnv('Following the environment', async () => {
      ok(await pw(['eval', fetchTo('flag'), '--intent', 'Fetching by flag']), 'flag');
      ok(await pw(['eval', fetchTo('env')]), 'env');
      ok(await pw(['eval', fetchTo('off'), '--intent=']), 'off');
    });
    await pause(1100);
    await withIntentEnv('Fetching\u202e the\u200b  cleaned\u2066 one\u2069\n', async () => {
      ok(await pw(['eval', fetchTo('cleaned')]), 'cleaned');
    });
    await withIntentEnv('x'.repeat(400), async () => {
      ok(await pw(['eval', fetchTo('long')]), 'long');
    });
    await pause(1000);
    ok(await runner.playwright(['har-stop'], cwd), 'har-stop');
    assert.equal((await held.ended).code, 0);
    const entries = JSON.parse(await readFile(file, 'utf8')).log.entries;
    const comment = (name) => {
      const found = entries.find((entry) => new URL(entry.request.url).pathname === `/api/${name}`);
      assert.ok(found, `${name} in ${JSON.stringify(entries.map((entry) => entry.request.url))}`);
      return found.comment;
    };
    assert.equal(comment('order'), 'Placing the order');
    assert.equal(comment('late'), undefined);
    assert.equal(comment('flag'), 'Fetching by flag');
    assert.equal(comment('env'), 'Following the environment');
    assert.equal(comment('off'), undefined);
    assert.equal(comment('cleaned'), 'Fetching the cleaned one');
    assert.equal(comment('long'), 'x'.repeat(120));
  });
}

for (const kind of ['native', 'kernel']) {
  test(`${kind} playwright-cli records the intent and leaves pages alone`, {
    timeout: 300000,
  }, async (t) => {
    delete process.env.SLICC_INTENT;
    const server = await site();
    let browser;
    let runner;
    let raw;
    try {
      browser = await launchBrowser();
      raw = await rawSocket(browser.wsUrl);
      runner =
        kind === 'native'
          ? nativeRunner(native('playwright-cli'), native('curlwright'), browser.wsUrl)
          : await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
      await t.test('an intent injects nothing into the page', () =>
        untouchedFlow(runner, server, raw)
      );
      if (kind === 'native') await nativeFlows(t, runner, server, browser);
    } finally {
      raw?.close();
      await Promise.resolve(runner?.close()).catch(() => {});
      await browser?.close().catch(() => {});
      await server.close().catch(() => {});
    }
  });
}
