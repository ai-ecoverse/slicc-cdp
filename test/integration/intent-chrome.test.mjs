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
<input placeholder="Name">
<a href="http://seven.sliccy.ai/os/">Seven</a>`;

const STATE = `(() => {
  const hosts = Array.from(document.documentElement.children).filter((node) => node.localName === 'x-slicc-intent');
  const host = hosts[0];
  return {
    count: hosts.length,
    open: host ? host.matches(':popover-open') : false,
    pointer: host ? getComputedStyle(host).pointerEvents : null,
    hidden: host ? host.getAttribute('aria-hidden') : null,
    reachable: host ? host.shadowRoot !== null : false,
  };
})()`;

function site() {
  const server = createServer((req, res) => {
    const host = req.headers.host ?? '';
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    const send = (type, body) => {
      res.writeHead(200, { 'content-type': type });
      res.end(body);
    };
    if (/sliccy\.ai/.test(host)) return send('text/html', '<!doctype html><title>Seven</title>');
    if (url.pathname === '/form') return send('text/html', formPage);
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

async function overlay(raw, sessionId) {
  const { result } = await raw.send(
    'Runtime.evaluate',
    { expression: STATE, returnByValue: true },
    sessionId
  );
  const state = result.value;
  if (!state.count) return { ...state, text: null, mode: null };
  const { root: tree } = await raw.send('DOM.getDocument', { depth: -1, pierce: true }, sessionId);
  const find = (node) =>
    node.localName === 'x-slicc-intent' ? node : (node.children ?? []).map(find).find(Boolean);
  const host = find(tree);
  assert.ok(host, 'the overlay went away while it was read');
  const texts = [];
  const walk = (node) => {
    if (node.nodeType === 3) texts.push(node.nodeValue);
    for (const child of [...(node.shadowRoots ?? []), ...(node.children ?? [])]) walk(child);
  };
  walk(host);
  return { ...state, text: texts.join(''), mode: host.shadowRoots?.[0]?.shadowRootType ?? null };
}

async function count(raw, sessionId) {
  const { result } = await raw.send(
    'Runtime.evaluate',
    { expression: STATE, returnByValue: true },
    sessionId
  );
  return result.value.count;
}

async function gone(raw, sessionId, label) {
  const deadline = Date.now() + 5000;
  for (;;) {
    if ((await count(raw, sessionId)) === 0) return;
    assert.ok(Date.now() < deadline, `${label}: the overlay never went away`);
    await pause(100);
  }
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

async function attach(raw, targetId) {
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  return sessionId;
}

async function overlayFlow(runner, server, raw) {
  const cwd = await runner.fresh();
  const pw = (args) => runner.playwright(args, cwd);
  const opened = await pw(['open', `${server.origin}/form`, '--intent', 'Opening the order form']);
  ok(opened, 'open');
  const tab = opened.stdout.match(/targetId: ([0-9A-F]+)/)?.[1];
  assert.ok(tab, opened.stdout);
  const session = await attach(raw, tab);
  const shown = await overlay(raw, session);
  assert.deepEqual(shown, {
    count: 1,
    open: true,
    pointer: 'none',
    hidden: 'true',
    reachable: false,
    text: 'Opening the order form',
    mode: 'closed',
  });
  await gone(raw, session, 'open');

  const plain = await pw(['snapshot']);
  ok(plain, 'snapshot');
  const labelled = await pw(['snapshot', '--intent=Reading the order form']);
  ok(labelled, 'snapshot --intent');
  assert.equal(labelled.stdout, plain.stdout);
  assert.doesNotMatch(labelled.stdout, /Reading the order form|x-slicc/);
  assert.equal((await overlay(raw, session)).text, 'Reading the order form');
  await gone(raw, session, 'snapshot');

  ok(await pw(['tab-list', '--intent', 'Listing tabs']), 'tab-list');
  assert.equal(await count(raw, session), 0);
  return { cwd, tab, session };
}

async function nativeFlows(t, runner, server, browser, raw) {
  const { cwd, tab, session } = await overlayFlow(runner, server, raw);
  const pw = (args) => runner.playwright([...args, '--tab', tab], cwd);

  await t.test('the flag wins over SLICC_INTENT, which is cleaned and cut', async () => {
    const snapshot = await pw(['snapshot']);
    const order = refOf(snapshot.stdout, 'button', 'Order');
    await withIntentEnv('Following the environment', async () => {
      ok(await pw(['hover', order, '--intent', 'Pointing at the order button']), 'hover');
      assert.equal((await overlay(raw, session)).text, 'Pointing at the order button');
      ok(await pw(['eval', '1']), 'eval');
      assert.equal((await overlay(raw, session)).text, 'Following the environment');
    });
    await withIntentEnv('Pressing\u202e\u200b  the\u2066 key\u2069\n', async () => {
      ok(await pw(['press', 'Shift']), 'press');
      assert.equal((await overlay(raw, session)).text, 'Pressing the key');
    });
    await withIntentEnv('x'.repeat(400), async () => {
      ok(await pw(['eval', '2']), 'eval long');
      assert.equal((await overlay(raw, session)).text, 'x'.repeat(120));
    });
    await gone(raw, session, 'long');
    await withIntentEnv('Ignored by the flag', async () => {
      ok(await pw(['eval', '3', '--intent=']), 'eval empty');
      assert.equal(await count(raw, session), 0);
    });
    await gone(raw, session, 'env');
  });

  await t.test('a screenshot leaves the overlay out', async () => {
    ok(await pw(['eval', 'document.activeElement?.blur()']), 'blur');
    await gone(raw, session, 'blur');
    ok(await pw(['screenshot', '--filename', path.join(cwd, 'before.png')]), 'baseline');
    ok(await pw(['eval', '1', '--intent', 'Showing the overlay']), 'eval');
    assert.equal(await count(raw, session), 1);
    ok(await pw(['screenshot', '--filename', path.join(cwd, 'after.png')]), 'screenshot');
    assert.equal(await count(raw, session), 0);
    assert.ok(
      (await readFile(path.join(cwd, 'before.png'))).equals(
        await readFile(path.join(cwd, 'after.png'))
      )
    );
    ok(
      await pw(['screenshot', '--filename', path.join(cwd, 'own.png'), '--intent', 'Capturing']),
      'screenshot --intent'
    );
    assert.ok(
      (await readFile(path.join(cwd, 'before.png'))).equals(
        await readFile(path.join(cwd, 'own.png'))
      )
    );
    assert.equal((await overlay(raw, session)).text, 'Capturing');
    await gone(raw, session, 'screenshot');
  });

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
    ok(await pw(['eval', "fetch('/api/late').then((r) => r.status)"]), 'late');
    ok(
      await pw(['eval', "fetch('/api/after').then((r) => r.status)", '--intent', 'Fetching again']),
      'after'
    );
    await pause(1000);
    ok(await runner.playwright(['har-stop'], cwd), 'har-stop');
    assert.equal((await held.ended).code, 0);
    const entries = JSON.parse(await readFile(file, 'utf8')).log.entries;
    const comment = (pathname) => {
      const found = entries.find((entry) => new URL(entry.request.url).pathname === pathname);
      assert.ok(
        found,
        `${pathname} in ${JSON.stringify(entries.map((entry) => entry.request.url))}`
      );
      return found.comment;
    };
    assert.equal(comment('/api/order'), 'Placing the order');
    assert.equal(comment('/api/late'), undefined);
    assert.equal(comment('/api/after'), 'Fetching again');
  });

  await t.test('nothing is injected into a protected page', async () => {
    const seven = await raw.open('http://seven.sliccy.ai/os/');
    const sevenSession = await attach(raw, seven);
    const refused = await runner.playwright(
      ['eval', '1', '--tab', seven, '--intent', 'Peeking at seven'],
      cwd
    );
    assert.equal(refused.code, 1, refused.stdout);
    const plain = await runner.playwright(['eval', '1', '--tab', seven], cwd);
    assert.deepEqual(refused, plain);
    const snapshot = await pw(['snapshot']);
    const clicked = await pw([
      'click',
      refOf(snapshot.stdout, 'link', 'Seven'),
      '--intent',
      'Leaving for seven',
    ]);
    const deadline = Date.now() + 5000;
    while (!(await raw.targetUrl(tab)).includes('sliccy.ai') && Date.now() < deadline)
      await pause(100);
    assert.match(await raw.targetUrl(tab), /seven\.sliccy\.ai/, clicked.stderr);
    await pause(300);
    assert.equal(await count(raw, sevenSession), 0);
    assert.equal(await count(raw, session), 0);
    assert.equal(await count(raw, sevenSession), 0);
  });
}

for (const kind of ['native', 'kernel']) {
  test(`${kind} playwright-cli shows and records the intent`, { timeout: 300000 }, async (t) => {
    delete process.env.SLICC_INTENT;
    const server = await site();
    let browser;
    let runner;
    let raw;
    try {
      browser = await launchBrowser([
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${server.port}`,
        '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
      ]);
      raw = await rawSocket(browser.wsUrl);
      runner =
        kind === 'native'
          ? nativeRunner(native('playwright-cli'), native('curlwright'), browser.wsUrl)
          : await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
      if (kind === 'native') await nativeFlows(t, runner, server, browser, raw);
      else
        await t.test('the overlay appears, goes and stays out of the snapshot', () =>
          overlayFlow(runner, server, raw)
        );
    } finally {
      raw?.close();
      await Promise.resolve(runner?.close()).catch(() => {});
      await browser?.close().catch(() => {});
      await server.close().catch(() => {});
    }
  });
}
