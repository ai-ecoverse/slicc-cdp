import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchBrowser, rawSocket } from './browser.mjs';
import { chromeTest as test } from './chrome-policy.mjs';
import { kernelRunner, nativeRunner } from './runners.mjs';
import { startServer } from './server.mjs';
import './watchdog.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const nativeDir = path.join(root, 'target/debug');
const wasmDir = path.join(root, 'target/wasm32-wasip1/debug');
const exe = (name) => path.join(nativeDir, process.platform === 'win32' ? `${name}.exe` : name);
const code = (line) => `### Ran Playwright code\n\`\`\`js\n${line}\n\`\`\`\n`;
const canaries = [
  { name: 'sevencanary', value: 'SEVENCOOKIESECRET', domain: '.sliccy.ai', path: '/' },
  { name: 'vaultcanary', value: 'VAULTCOOKIESECRET', domain: 'vault.sliccy.ai', path: '/' },
];

async function pageEval(raw, targetId, expression) {
  const { sessionId } = await raw.send('Target.attachToTarget', { targetId, flatten: true });
  try {
    const { result, exceptionDetails } = await raw.send(
      'Runtime.evaluate',
      { expression, returnByValue: true, awaitPromise: true },
      sessionId
    );
    assert.equal(exceptionDetails, undefined, JSON.stringify(exceptionDetails));
    return result.value;
  } finally {
    await raw.send('Target.detachFromTarget', { sessionId });
  }
}

async function pages(raw) {
  const { targetInfos } = await raw.send('Target.getTargets');
  return targetInfos.filter((info) => info.type === 'page');
}

async function storageFlow(runner, site, raw, protectedTabs) {
  const cwd = await runner.fresh();
  const run = async (args, status = 0) => {
    const result = await runner.playwright(args, cwd);
    assert.equal(result.code, status, `${args.join(' ')}\n${result.stdout}${result.stderr}`);
    assert.doesNotMatch(result.stdout + result.stderr, /SECRET/, args.join(' '));
    if (status !== 0) assert.equal(result.stdout, '', args.join(' '));
    return status === 0 ? result.stdout : result.stderr;
  };
  const sealed = async () => {
    for (const { target, origin } of protectedTabs) {
      const state = JSON.parse(
        await pageEval(
          raw,
          target,
          'JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } })'
        )
      );
      const secret = origin.includes('vault') ? 'VAULTSTORAGESECRET' : 'SEVENSTORAGESECRET';
      assert.deepEqual(state, { local: { canary: secret }, session: { canary: secret } });
    }
    const { cookies } = await raw.send('Storage.getCookies');
    for (const canary of canaries)
      assert.ok(cookies.some((item) => item.name === canary.name && item.value === canary.value));
  };
  await run(['open', `${site.origin}/page`]);
  const tab = (await pages(raw)).find((info) =>
    info.url.startsWith(`${site.origin}/page`)
  ).targetId;
  await pageEval(raw, tab, 'localStorage.clear(); sessionStorage.clear()');
  assert.equal(await run(['localstorage-list']), 'No localStorage items found\n');
  assert.equal(
    await run(['localstorage-set', 'flavor', 'fixture-flavor']),
    code("await page.localStorage.setItem('flavor', 'fixture-flavor');")
  );
  assert.equal(await pageEval(raw, tab, "localStorage.getItem('flavor')"), 'fixture-flavor');
  assert.equal(await run(['localstorage-get', 'flavor']), 'flavor=fixture-flavor\n');
  assert.equal(await run(['localstorage-list']), 'flavor=fixture-flavor\n');
  assert.equal(
    await run(['localstorage-get', 'missing']),
    "localStorage key 'missing' not found\n"
  );
  assert.equal(
    await run(['sessionstorage-set', 'step', '2']),
    code("await page.sessionStorage.setItem('step', '2');")
  );
  assert.equal(await pageEval(raw, tab, "sessionStorage.getItem('step')"), '2');
  assert.equal(await run(['sessionstorage-list']), 'step=2\n');
  assert.equal(
    await run(['sessionstorage-delete', 'step']),
    code("await page.sessionStorage.removeItem('step');")
  );
  assert.equal(await run(['sessionstorage-get', 'step']), "sessionStorage key 'step' not found\n");
  for (const { target } of protectedTabs) {
    for (const args of [
      ['localstorage-list'],
      ['localstorage-get', 'canary'],
      ['localstorage-set', 'canary', 'forged'],
      ['localstorage-delete', 'canary'],
      ['localstorage-clear'],
      ['sessionstorage-list'],
      ['sessionstorage-get', 'canary'],
      ['sessionstorage-set', 'canary', 'forged'],
      ['sessionstorage-delete', 'canary'],
      ['sessionstorage-clear'],
    ]) {
      assert.match(await run([...args, '--tab', target], 1), /^slicc: refused/);
    }
  }
  await sealed();
  assert.equal(
    await run(['state-save', 'state.json']),
    `### Result\n- [Storage state](./state.json)\n${code("await page.context().storageState({ path: './state.json' });")}`
  );
  const text = new TextDecoder().decode(await runner.read(cwd, 'state.json'));
  assert.doesNotMatch(text, /SECRET|sliccy/);
  const saved = JSON.parse(text);
  assert.deepEqual(Object.keys(saved), ['cookies', 'origins']);
  assert.deepEqual(saved.origins, [
    { origin: site.origin, localStorage: [{ name: 'flavor', value: 'fixture-flavor' }] },
  ]);
  for (const item of saved.cookies) {
    assert.deepEqual(
      Object.keys(item).slice(0, 8),
      ['name', 'value', 'domain', 'path', 'expires', 'httpOnly', 'secure', 'sameSite'],
      JSON.stringify(item)
    );
  }
  assert.equal(text, JSON.stringify(saved, null, 2));
  const other = `http://localhost:${site.port}`;
  const before = (await pages(raw)).length;
  await runner.write(
    cwd,
    'load.json',
    JSON.stringify({
      cookies: [
        {
          name: 'loaded',
          value: 'fixture-loaded',
          domain: '127.0.0.1',
          path: '/',
          expires: -1,
          httpOnly: false,
          secure: false,
          sameSite: 'Lax',
        },
      ],
      origins: [
        { origin: site.origin, localStorage: [{ name: 'restored', value: 'fixture-restored' }] },
        { origin: other, localStorage: [{ name: 'elsewhere', value: 'fixture-elsewhere' }] },
      ],
    })
  );
  assert.equal(
    await run(['state-load', 'load.json']),
    `### Result\nStorage state restored from load.json\n${code("await page.context().setStorageState('load.json');")}`
  );
  assert.equal((await pages(raw)).length, before);
  assert.equal(
    await pageEval(raw, tab, 'JSON.stringify({ ...localStorage })'),
    '{"restored":"fixture-restored"}'
  );
  assert.match(await run(['cookie-get', 'loaded']), /^loaded=fixture-loaded /);
  assert.equal(await run(['cookie-get', 'session']), "Cookie 'session' not found\n");
  const otherTab = await raw.open(`${other}/page`);
  assert.equal(
    await run(['localstorage-get', 'elsewhere', '--tab', otherTab]),
    'elsewhere=fixture-elsewhere\n'
  );
  await raw.send('Target.closeTarget', { targetId: otherTab });
  await sealed();
  for (const [name, state] of [
    [
      'seven.json',
      {
        origins: [
          { origin: 'http://seven.sliccy.ai', localStorage: [{ name: 'canary', value: 'forged' }] },
        ],
      },
    ],
    [
      'vault.json',
      { cookies: [{ name: 'vaultcanary', value: 'forged', domain: 'vault.sliccy.ai', path: '/' }] },
    ],
  ]) {
    await runner.write(cwd, name, JSON.stringify(state));
    assert.match(await run(['state-load', name], 1), /^slicc: refused/);
  }
  await sealed();
  await run(['tab-close', '--tab', tab]);
}

test('storage commands and state files stay away from seven and the vault in Chrome', async (t) => {
  const site = await startServer();
  let browser;
  let raw;
  let kernel;
  try {
    browser = await launchBrowser([
      `--host-resolver-rules=MAP seven.sliccy.ai 127.0.0.1:${site.port}, MAP vault.sliccy.ai 127.0.0.1:${site.port}`,
      '--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable',
    ]);
    raw = await rawSocket(browser.wsUrl);
    const protectedTabs = [];
    for (const [origin, secret] of [
      ['http://seven.sliccy.ai', 'SEVENSTORAGESECRET'],
      ['http://vault.sliccy.ai', 'VAULTSTORAGESECRET'],
    ]) {
      const target = await raw.open(`${origin}/landing`);
      await pageEval(
        raw,
        target,
        `localStorage.setItem('canary', '${secret}'); sessionStorage.setItem('canary', '${secret}')`
      );
      protectedTabs.push({ target, origin });
    }
    await raw.send('Storage.setCookies', { cookies: canaries });
    const native = nativeRunner(exe('playwright-cli'), exe('curlwright'), browser.wsUrl);
    await t.test('native', () => storageFlow(native, site, raw, protectedTabs));
    kernel = await kernelRunner(
      path.join(wasmDir, 'playwright-cli.wasm'),
      path.join(wasmDir, 'curlwright.wasm'),
      browser.wsUrl
    );
    await t.test('kernel', () => storageFlow(kernel, site, raw, protectedTabs));
  } finally {
    kernel?.close();
    raw?.close();
    await browser?.close();
    await site.close();
  }
});
