import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const native = (name) =>
  path.join(root, 'target/debug', process.platform === 'win32' ? `${name}.exe` : name);
const wasm = (name) => path.join(root, 'target/wasm32-wasip1/debug', `${name}.wasm`);
const marker = 'PAGE_TOOLS_PROTECTED_7C1E';

const toolsPage = `<!doctype html><title>Tools</title>
<button>Submit</button>
<input placeholder="Email address">
<button data-testid="save">Save</button>
<button>Twin</button><button>Twin</button>
<a href="/next">Next page</a>
<iframe name="inner" src="/inner"></iframe>`;

function site() {
  const protectedHits = [];
  const server = createServer((req, res) => {
    const host = req.headers.host ?? '';
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    if (/sliccy\.ai/.test(host)) {
      protectedHits.push(`${host}${url.pathname}`);
      res.writeHead(200, { 'content-type': 'text/html', link: `</${marker}>; rel="next"` });
      res.end(`<html><body>${marker}</body></html>`);
      return;
    }
    const send = (status, type, body, headers = {}) => {
      res.writeHead(status, { 'content-type': type, ...headers });
      res.end(body);
    };
    if (url.pathname === '/tools') return send(200, 'text/html', toolsPage);
    if (url.pathname === '/inner')
      return send(200, 'text/html', '<!doctype html><button>Inside</button>');
    if (url.pathname === '/ping') return send(200, 'text/plain', 'pong');
    if (url.pathname === '/links')
      return send(200, 'text/plain', 'body never read', {
        link: [
          '</next?token=LINKTOKEN>; rel="next"',
          `<http://seven.sliccy.ai/os/?code=${marker}>; rel="preload"`,
          '</skills/demo>; rel="https://www.sliccy.ai/rel/upskill"; title="Install the demo"',
        ].join(', '),
      });
    if (url.pathname === '/to-seven')
      return send(302, 'text/plain', `Redirect ${marker}`, {
        location: `http://seven.sliccy.ai/auth/callback?code=${marker}#access_token=${marker}`,
      });
    if (url.pathname === '/to-vault')
      return send(302, 'text/plain', `Redirect ${marker}`, {
        location: `http://vault.sliccy.ai/oauth/mcp-callback?code=${marker}`,
      });
    return send(404, 'text/plain', 'missing');
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        port,
        origin: `http://127.0.0.1:${port}`,
        protectedHits,
        close: () =>
          new Promise((done) => {
            server.closeAllConnections();
            server.close(() => done());
          }),
      });
    });
  });
}

function refOf(stdout, role, name, nth = 0) {
  const pattern = new RegExp(
    `(?<![A-Za-z])${role} "${name}"(?: \\[[^\\]]+\\])*? \\[ref=((?:f\\d+)?e\\d+)\\]`,
    'g'
  );
  const found = [...stdout.matchAll(pattern)].map((match) => match[1]);
  assert.ok(found[nth], `${role} "${name}" #${nth} in\n${stdout}`);
  return found[nth];
}

const ok = (result, label) =>
  assert.equal(result.code, 0, `${label}\n${result.stdout}\n${result.stderr}`);
const clean = (result) =>
  assert.doesNotMatch(`${result.stdout}${result.stderr}`, new RegExp(marker));

async function locatorFlow(runner, server) {
  const cwd = await runner.fresh();
  const pw = (args) => runner.playwright(args, cwd);
  ok(await pw(['open', `${server.origin}/tools`]), 'open');
  const snapshot = await pw(['snapshot']);
  ok(snapshot, 'snapshot');
  const expected = [
    [refOf(snapshot.stdout, 'button', 'Submit'), "getByRole('button', { name: 'Submit' })"],
    [
      refOf(snapshot.stdout, 'textbox', 'Email address'),
      "getByRole('textbox', { name: 'Email address' })",
    ],
    [refOf(snapshot.stdout, 'button', 'Save'), "getByTestId('save')"],
    [refOf(snapshot.stdout, 'button', 'Twin'), "getByRole('button', { name: 'Twin' }).first()"],
    [refOf(snapshot.stdout, 'button', 'Twin', 1), "getByRole('button', { name: 'Twin' }).nth(1)"],
    [refOf(snapshot.stdout, 'link', 'Next page'), "getByRole('link', { name: 'Next page' })"],
  ];
  for (const [ref, locator] of expected) {
    const result = await pw(['generate-locator', ref]);
    ok(result, `generate-locator ${ref}`);
    assert.equal(result.stdout, `### Result\n${locator}\n`);
  }
  const inside = refOf(snapshot.stdout, 'button', 'Inside');
  const framed = await pw(['generate-locator', inside]);
  ok(framed, 'generate-locator in a frame');
  assert.match(
    framed.stdout,
    /^### Result\nlocator\('iframe\[name="inner"\]'\)\.contentFrame\(\)\.getByRole\('button', \{ name: 'Inside' \}\)\n$/
  );
  const submit = expected[0][0];
  const save = expected[2][0];
  const count =
    "(() => { const host = Array.from(document.documentElement.children).find((node) => node.localName === 'x-slicc-highlight'); return host ? host.shadowRoot.children.length : 0; })()";
  const shown = await pw(['highlight', submit, '--style', 'outline: 2px dashed red']);
  ok(shown, 'highlight');
  assert.equal(shown.stdout, `### Result\nHighlighted locator('aria-ref=${submit}')\n`);
  ok(await pw(['highlight', save]), 'highlight save');
  assert.equal((await pw(['eval', count])).stdout, '2\n');
  const sized = await pw([
    'eval',
    "(() => { const host = Array.from(document.documentElement.children).find((node) => node.localName === 'x-slicc-highlight'); const box = host.shadowRoot.children[0]; return box.style.width !== '0px' && box.style.outline.includes('dashed'); })()",
  ]);
  assert.equal(sized.stdout, 'true\n');
  const hidden = await pw(['highlight', submit, '--hide']);
  ok(hidden, 'highlight --hide ref');
  assert.equal(hidden.stdout, `### Result\nHid highlight for locator('aria-ref=${submit}')\n`);
  assert.equal((await pw(['eval', count])).stdout, '1\n');
  const cleared = await pw(['highlight', '--hide']);
  ok(cleared, 'highlight --hide');
  assert.equal(cleared.stdout, '### Result\nHid page highlight\n');
  assert.equal((await pw(['eval', count])).stdout, '0\n');
  await runner.write(cwd, 'title.js', 'async () => { await null; return document.title; }');
  const file = await pw(['eval-file', 'title.js']);
  ok(file, 'eval-file');
  assert.equal(file.stdout, 'Tools\n');
}

async function networkFlow(runner, server) {
  const cwd = await runner.fresh();
  const pw = (args) => runner.playwright(args, cwd);
  ok(await pw(['open', `${server.origin}/tools`]), 'open');
  const offline = await pw(['network-state-set', 'offline']);
  ok(offline, 'offline');
  assert.equal(
    offline.stdout,
    '### Result\nNetwork is now offline\n### Ran Playwright code\n```js\nawait page.context().setOffline(true);\n```\n'
  );
  assert.equal((await pw(['eval', 'navigator.onLine'])).stdout, 'false\n');
  const blocked = await pw(['eval', "fetch('/ping').then((r) => r.status, () => 'failed')"]);
  assert.equal(blocked.stdout, 'failed\n');
  ok(await pw(['network-state-set', 'online']), 'online');
  assert.equal((await pw(['eval', 'navigator.onLine'])).stdout, 'true\n');
  const restored = await pw(['eval', "fetch('/ping').then((r) => r.text())"]);
  assert.equal(restored.stdout, 'pong\n');
}

async function fetchFlow(runner, server) {
  const cwd = await runner.fresh();
  const pw = (args) => runner.playwright(args, cwd);
  ok(await pw(['open', `${server.origin}/tools`]), 'open');
  const listed = await pw(['fetch', '/links']);
  ok(listed, 'fetch /links');
  clean(listed);
  const payload = JSON.parse(listed.stdout);
  assert.equal(payload.status, 200);
  assert.equal(payload.url, `${server.origin}/links`);
  assert.deepEqual(
    payload.links.map((link) => [link.href, link.rel]),
    [
      [`${server.origin}/next?token=\u2026`, 'next'],
      [`${server.origin}/skills/demo`, 'https://www.sliccy.ai/rel/upskill'],
    ]
  );
  assert.deepEqual(payload.handoff, {
    verb: 'upskill',
    target: `${server.origin}/skills/demo`,
    instruction: 'Install the demo',
  });
  const head = await pw(['fetch', `${server.origin}/ping`, '--method', 'HEAD']);
  ok(head, 'fetch HEAD');
  assert.equal(JSON.parse(head.stdout).status, 200);
  for (const hop of ['/to-seven', '/to-vault']) {
    const redirected = await pw(['fetch', hop]);
    assert.equal(redirected.code, 1, `${hop}\n${redirected.stdout}\n${redirected.stderr}`);
    clean(redirected);
    assert.doesNotMatch(redirected.stdout, /"status": 200/);
  }
  for (const url of ['http://seven.sliccy.ai/os/', `http://vault.sliccy.ai/?code=${marker}`]) {
    const refused = await pw(['fetch', url]);
    assert.equal(refused.code, 1, url);
    assert.match(refused.stderr, /^slicc: refused: /);
    clean(refused);
  }
  assert.deepEqual(server.protectedHits, []);
}

for (const kind of ['native', 'kernel']) {
  test(`${kind} page tools against Chrome`, { timeout: 300000 }, async (t) => {
    const server = await site();
    let browser;
    let runner;
    try {
      browser = await launchBrowser([
        `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${server.port}, MAP vault.sliccy.ai 127.0.0.1:${server.port}`,
        '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
      ]);
      runner =
        kind === 'native'
          ? nativeRunner(native('playwright-cli'), native('curlwright'), browser.wsUrl)
          : await kernelRunner(wasm('playwright-cli'), wasm('curlwright'), browser.wsUrl);
      await t.test('generate-locator, highlight and eval-file', () => locatorFlow(runner, server));
      await t.test('network-state-set', () => networkFlow(runner, server));
      await t.test('fetch keeps protected origins out', () => fetchFlow(runner, server));
    } finally {
      await Promise.resolve(runner?.close()).catch(() => {});
      await browser?.close().catch(() => {});
      await server.close().catch(() => {});
    }
  });
}
