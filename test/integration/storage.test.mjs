import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { fakeCdp } from './fake-cdp.mjs';
import { HARNESS_PORTS } from './ports.mjs';
import { kernelRunner, kernelSessionFile } from './runners.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const native = path.join(
  root,
  'target/debug',
  process.platform === 'win32' ? 'playwright-cli.exe' : 'playwright-cli'
);
const wasm = path.join(root, 'target/wasm32-wasip1/debug');
const cookieRefusal = "slicc: refused: cookies on seven's origin\n";
const secrets = [
  'fixture-seven-local',
  'fixture-vault-local',
  'fixture-seven-session',
  'fixture-seven-secret',
  'fixture-vault-secret',
  'fixture-chip-secret',
  'fixture-error-secret',
];
const protectedOrigins = ['https://seven.sliccy.ai', 'https://vault.sliccy.ai'];
const shop = 'https://shop.example.test';
const docs = 'https://docs.example.test';

function cookie(name, value, domain, extra = {}) {
  return {
    name,
    value,
    domain,
    path: '/',
    expires: -1,
    size: name.length + value.length,
    httpOnly: false,
    secure: true,
    session: true,
    priority: 'Medium',
    sourceScheme: 'Secure',
    sourcePort: 443,
    ...extra,
  };
}

const jar = [
  cookie('sid', 'fixture-sid-1', 'shop.example.test', { httpOnly: true, sameSite: 'Strict' }),
  cookie('theme', 'fixture-dark', '.example.test', {
    path: '/app',
    expires: 1924992000.5,
    session: false,
    secure: false,
  }),
  cookie('part', 'fixture-part', 'widget.example.test', {
    partitionKey: { topLevelSite: 'https://example.test', hasCrossSiteAncestor: false },
  }),
  cookie('seven', 'fixture-seven-secret', '.sliccy.ai'),
  cookie('vault', 'fixture-vault-secret', 'vault.sliccy.ai'),
  cookie('chip', 'fixture-chip-secret', 'widget.example.test', {
    partitionKey: { topLevelSite: 'https://sliccy.ai', hasCrossSiteAncestor: true },
  }),
];

function storageModel() {
  const local = new Map([
    [shop, new Map([['cart', 'fixture-cart']])],
    ['https://seven.sliccy.ai', new Map([['token', 'fixture-seven-local']])],
    ['https://vault.sliccy.ai', new Map([['vault', 'fixture-vault-local']])],
  ]);
  const session = new Map([['SEVEN', new Map([['token', 'fixture-seven-session']])]]);
  const touched = [];
  const area = (kind, target, origin) => {
    const map = kind === 'local' ? local : session;
    const key = kind === 'local' ? origin : target.targetId;
    if (!map.has(key)) map.set(key, new Map());
    return map.get(key);
  };
  const tail = (expression) => expression.slice(expression.lastIndexOf('})(') + 3, -1);
  const evaluate = (target, expression) => {
    const origin = new URL(target.url).origin;
    if (expression.includes('storage.getItem(request.key)')) {
      const request = JSON.parse(tail(expression));
      touched.push({ origin, target: target.targetId, op: `${request.kind}-${request.op}` });
      const storage = area(request.kind, target, origin);
      if (request.op === 'list')
        return { origin, items: [...storage].map(([name, value]) => ({ name, value })) };
      if (request.op === 'get') return { origin, value: storage.get(request.key) ?? null };
      if (request.op === 'set') storage.set(request.key, request.value);
      else if (request.op === 'delete') storage.delete(request.key);
      else storage.clear();
      return { origin };
    }
    const [expected, items] = JSON.parse(`[${tail(expression)}]`);
    touched.push({ origin, target: target.targetId, op: 'restore' });
    if (origin !== expected) return { origin };
    local.set(origin, new Map(items.map(({ name, value }) => [name, value])));
    area('session', target, origin).clear();
    return { origin };
  };
  return { local, session, touched, evaluate };
}

function nativeRun(args, home, url) {
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
    delete env.PLAYWRIGHT_CLI_SESSION;
    const child = spawn(native, ['--cdp', url, ...args], {
      cwd: home,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (chunk) => out.push(chunk));
    child.stderr.on('data', (chunk) => err.push(chunk));
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`timed out: ${args.join(' ')}`));
    }, 30000);
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({
        code: code ?? 1,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

const runners = {
  native: async (fake) => {
    const home = await mkdtemp(path.join(tmpdir(), 'slicc-cdp-storage-'));
    return {
      run: (args) => nativeRun(args, home, fake.url),
      read: (name) => readFile(path.join(home, name), 'utf8'),
      write: (name, text) => writeFile(path.join(home, name), text),
      list: async (dir) => readdir(path.join(home, dir)),
      mode: async (name) => (await stat(path.join(home, name))).mode & 0o777,
      saved: async () => {
        const files = await readdir(home, { recursive: true });
        const file = files.find((name) => name.includes('sessions') && name.endsWith('.json'));
        return file ? readFile(path.join(home, file), 'utf8') : '';
      },
      close: () => rm(home, { recursive: true, force: true }),
    };
  },
  wasi: async (fake) => {
    const runner = await kernelRunner(
      path.join(wasm, 'playwright-cli.wasm'),
      path.join(wasm, 'curlwright.wasm'),
      fake.url
    );
    const cwd = await runner.fresh();
    const decode = (bytes) => new TextDecoder().decode(bytes);
    return {
      run: (args) => runner.playwright(args, cwd),
      read: async (name) => decode(await runner.read(cwd, name)),
      write: (name, text) => runner.write(cwd, name, text),
      list: async () => [],
      mode: async () => undefined,
      saved: async () =>
        decode(
          await runner.readFile(kernelSessionFile('/root', cwd)).catch(() => new Uint8Array())
        ),
      close: () => runner.close(),
    };
  },
};

async function withBrowser(kind, body, options = {}) {
  const model = storageModel();
  let fake;
  fake = await fakeCdp({
    cookies: jar,
    pauseNavigation: true,
    ...options,
    intercept: (message, send) => {
      const expression = message.params?.expression ?? '';
      if (
        message.method !== 'Runtime.evaluate' ||
        !/storage\.getItem\(request\.key\)|indexedDB\.databases/.test(expression)
      )
        return false;
      const id = fake.calls.at(-1)?.target;
      const target = fake.targets.find((item) => item.targetId === id);
      const value = model.evaluate(target, expression);
      send({
        id: message.id,
        result: { result: { type: 'object', value } },
        ...(message.sessionId ? { sessionId: message.sessionId } : {}),
      });
      return true;
    },
  });
  fake.targets.push({ targetId: 'SEVEN', url: 'https://seven.sliccy.ai/os', loaderId: 'LS' });
  const runner = await runners[kind](fake);
  const seen = [];
  const run = async (args) => {
    const result = await runner.run(args);
    seen.push(result.stdout, result.stderr);
    return result;
  };
  const ok = async (args) => {
    const result = await run(args);
    assert.equal(result.code, 0, `${args.join(' ')}: ${result.stderr}`);
    assert.equal(result.stderr, '', args.join(' '));
    return result.stdout;
  };
  const refused = async (args) => {
    const result = await run(args);
    assert.equal(result.code, 1, `${args.join(' ')}: ${result.stdout}`);
    assert.equal(result.stdout, '', args.join(' '));
    return result.stderr;
  };
  try {
    await body({ fake, model, runner, run, ok, refused });
    seen.push(await runner.saved());
    for (const text of seen) {
      for (const secret of secrets) assert.ok(!text.includes(secret), `${secret} leaked: ${text}`);
    }
    for (const entry of model.touched) {
      assert.ok(!protectedOrigins.includes(entry.origin), JSON.stringify(entry));
      assert.notEqual(entry.target, 'SEVEN');
    }
    assert.deepEqual(
      [...model.local.get('https://seven.sliccy.ai')],
      [['token', 'fixture-seven-local']]
    );
    assert.deepEqual(
      [...model.local.get('https://vault.sliccy.ai')],
      [['vault', 'fixture-vault-local']]
    );
    assert.deepEqual([...model.session.get('SEVEN')], [['token', 'fixture-seven-session']]);
    for (const call of fake.calls) {
      if (
        /^(Runtime\.(evaluate|callFunctionOn)|Page\.createIsolatedWorld|DOMStorage\..*)$/.test(
          call.method
        )
      )
        assert.notEqual(call.target, 'SEVEN', JSON.stringify(call));
      if (/^Network\.(setCookie|deleteCookies)$/.test(call.method))
        assert.ok(!/sliccy\.ai$/.test(call.params.domain ?? ''), JSON.stringify(call));
    }
    assert.ok(
      fake.calls.every(
        (call) =>
          ![
            'Network.clearBrowserCookies',
            'Storage.clearCookies',
            'Network.clearBrowserCache',
          ].includes(call.method)
      )
    );
  } finally {
    await runner.close();
    await fake.close();
  }
}

const count = (fake, method) => fake.calls.filter((call) => call.method === method).length;
const code = (line) => `### Ran Playwright code\n\`\`\`js\n${line}\n\`\`\`\n`;
const mutations = (fake) =>
  ['Network.setCookie', 'Network.deleteCookies', 'Target.createTarget', 'Page.navigate'].map(
    (method) => count(fake, method)
  );

const expectedState = JSON.stringify(
  {
    cookies: [
      {
        name: 'sid',
        value: 'fixture-sid-1',
        domain: 'shop.example.test',
        path: '/',
        expires: -1,
        httpOnly: true,
        secure: true,
        sameSite: 'Strict',
      },
      {
        name: 'theme',
        value: 'fixture-dark',
        domain: '.example.test',
        path: '/app',
        expires: 1924992000.5,
        httpOnly: false,
        secure: false,
        sameSite: 'Lax',
      },
      {
        name: 'part',
        value: 'fixture-part',
        domain: 'widget.example.test',
        path: '/',
        expires: -1,
        httpOnly: false,
        secure: true,
        sameSite: 'Lax',
        _crHasCrossSiteAncestor: false,
        partitionKey: 'https://example.test',
      },
    ],
    origins: [
      { origin: shop, localStorage: [{ name: 'cart', value: 'fixture-cart' }] },
      {
        origin: docs,
        localStorage: [
          { name: 'note', value: 'fixture "quoted"\n' },
          { name: 'emoji', value: 'π' },
        ],
      },
    ],
  },
  null,
  2
);

for (const kind of ['native', 'wasi']) {
  test(`${kind}: storage help pages match upstream playwright-cli 0.1.22`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ ok }) => {
      const pages = {
        'localstorage-list':
          'playwright-cli localstorage-list \n\nList all localStorage key-value pairs\n\n',
        'localstorage-get':
          'playwright-cli localstorage-get <key>\n\nGet a localStorage item by key\n\nArguments:\n  <key>                       key to get\n',
        'localstorage-set':
          'playwright-cli localstorage-set <key> <value>\n\nSet a localStorage item\n\nArguments:\n  <key>                       key to set\n  <value>                     value to set\n',
        'localstorage-delete':
          'playwright-cli localstorage-delete <key>\n\nDelete a localStorage item\n\nArguments:\n  <key>                       key to delete\n',
        'localstorage-clear': 'playwright-cli localstorage-clear \n\nClear all localStorage\n\n',
        'sessionstorage-list':
          'playwright-cli sessionstorage-list \n\nList all sessionStorage key-value pairs\n\n',
        'sessionstorage-get':
          'playwright-cli sessionstorage-get <key>\n\nGet a sessionStorage item by key\n\nArguments:\n  <key>                       key to get\n',
        'sessionstorage-set':
          'playwright-cli sessionstorage-set <key> <value>\n\nSet a sessionStorage item\n\nArguments:\n  <key>                       key to set\n  <value>                     value to set\n',
        'sessionstorage-delete':
          'playwright-cli sessionstorage-delete <key>\n\nDelete a sessionStorage item\n\nArguments:\n  <key>                       key to delete\n',
        'sessionstorage-clear':
          'playwright-cli sessionstorage-clear \n\nClear all sessionStorage\n\n',
        'state-save':
          'playwright-cli state-save [filename]\n\nSaves the current storage (authentication) state to a file\n\nArguments:\n  [filename]                  file name to save the storage state to.\n',
        'state-load':
          'playwright-cli state-load <filename>\n\nLoads browser storage (authentication) state from a file\n\nArguments:\n  <filename>                  file name to load the storage state from.\n',
      };
      for (const [command, text] of Object.entries(pages)) {
        assert.equal(await ok([command, '--help']), text, command);
      }
      const global = await ok(['--help']);
      assert.ok(
        global.includes(
          'Storage:\n  state-load <filename>       loads browser storage (authentication) state from a file\n  state-save [filename]       saves the current storage (authentication) state to a file\n  cookie-list                 list all cookies (optionally filtered by domain/path)\n'
        ),
        global
      );
      assert.ok(
        global.includes(
          '  cookie-clear                clear all cookies\n  localstorage-list           list all localstorage key-value pairs\n  localstorage-get <key>      get a localstorage item by key\n  localstorage-set <key> <value> set a localstorage item\n  localstorage-delete <key>   delete a localstorage item\n  localstorage-clear          clear all localstorage\n  sessionstorage-list         list all sessionstorage key-value pairs\n  sessionstorage-get <key>    get a sessionstorage item by key\n  sessionstorage-set <key> <value> set a sessionstorage item\n  sessionstorage-delete <key> delete a sessionstorage item\n  sessionstorage-clear        clear all sessionstorage\n'
        ),
        global
      );
      const pending = global.split('Not implemented yet:')[1].split('\n')[1];
      for (const name of Object.keys(pages)) assert.ok(!pending.includes(name), pending);
    });
  });

  test(`${kind}: web storage commands print ordinary items and match upstream output`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, model, run, ok }) => {
      await ok(['open', `${shop}/app`]);
      assert.equal(await ok(['localstorage-list']), 'cart=fixture-cart\n');
      assert.equal(await ok(['localstorage-get', 'cart']), 'cart=fixture-cart\n');
      assert.equal(
        await ok(['localstorage-get', 'missing']),
        "localStorage key 'missing' not found\n"
      );
      assert.equal(
        await ok(['localstorage-set', "it's", 'a "b"\n']),
        code(`await page.localStorage.setItem('it\\'s', 'a "b"\\n');`)
      );
      assert.equal(await ok(['localstorage-list']), `cart=fixture-cart\nit's=a "b"\n\n`);
      assert.equal(
        await ok(['localstorage-delete', 'cart']),
        code("await page.localStorage.removeItem('cart');")
      );
      assert.equal(await ok(['localstorage-get', 'cart']), "localStorage key 'cart' not found\n");
      assert.equal(await ok(['localstorage-clear']), code('await page.localStorage.clear();'));
      assert.equal(await ok(['localstorage-list']), 'No localStorage items found\n');
      assert.equal(await ok(['sessionstorage-list']), 'No sessionStorage items found\n');
      assert.equal(
        await ok(['sessionstorage-set', 'step', '2']),
        code("await page.sessionStorage.setItem('step', '2');")
      );
      assert.equal(await ok(['sessionstorage-get', 'step']), 'step=2\n');
      assert.equal(
        await ok(['sessionstorage-get', 'nope']),
        "sessionStorage key 'nope' not found\n"
      );
      assert.equal(
        await ok(['sessionstorage-delete', 'step']),
        code("await page.sessionStorage.removeItem('step');")
      );
      assert.equal(
        await ok(['sessionstorage-set', 'a', '1']),
        code("await page.sessionStorage.setItem('a', '1');")
      );
      assert.equal(await ok(['sessionstorage-clear']), code('await page.sessionStorage.clear();'));
      assert.equal(await ok(['sessionstorage-list']), 'No sessionStorage items found\n');
      assert.equal(
        await ok(['localstorage-set', 'esc', '\u001b[31m']),
        code("await page.localStorage.setItem('esc', '\\u001b[31m');")
      );
      assert.equal(await ok(['localstorage-get', 'esc']), 'esc=\\x1b[31m\n');
      assert.deepEqual([...model.local.get(shop)], [['esc', '\u001b[31m']]);
      for (const [args, stderr] of [
        [['localstorage-get'], "error: 'key' argument: expected string, received undefined\n"],
        [
          ['sessionstorage-set', 'k'],
          "error: 'value' argument: expected string, received undefined\n",
        ],
        [
          ['localstorage-set'],
          "error: 'key' argument: expected string, received undefined\nerror: 'value' argument: expected string, received undefined\n",
        ],
        [['state-load'], "error: 'filename' argument: expected string, received undefined\n"],
      ]) {
        const result = await run(args);
        assert.equal(result.code, 1, args.join(' '));
        assert.equal(result.stdout, '');
        assert.equal(result.stderr, stderr);
      }
      const extra = await run(['localstorage-set', 'a', 'b', 'c']);
      assert.equal(extra.code, 1);
      assert.match(extra.stderr, /error: too many arguments: expected 2, received 3/);
      assert.ok(
        fake.calls.some(
          (call) =>
            call.method === 'Page.createIsolatedWorld' && call.params.worldName === 'slicc-storage'
        )
      );
      assert.ok(
        fake.calls
          .filter(
            (call) =>
              call.method === 'Page.createIsolatedWorld' &&
              call.params.worldName === 'slicc-storage'
          )
          .every((call) => call.params.grantUniveralAccess === undefined)
      );
    });
  });

  test(`${kind}: web storage never touches protected, error-page or unknown tabs`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, model, ok, refused }) => {
      await ok(['open', `${shop}/app`]);
      const commands = [
        ['localstorage-list'],
        ['localstorage-get', 'token'],
        ['localstorage-set', 'token', 'fixture-forged'],
        ['localstorage-delete', 'token'],
        ['localstorage-clear'],
        ['sessionstorage-list'],
        ['sessionstorage-get', 'token'],
        ['sessionstorage-set', 'token', 'fixture-forged'],
        ['sessionstorage-delete', 'token'],
        ['sessionstorage-clear'],
      ];
      const touched = model.touched.length;
      for (const args of commands) {
        const stderr = await refused([...args, '--tab', 'SEVEN']);
        assert.ok(stderr.startsWith('slicc: refused'), stderr);
        assert.match(await refused([...args, '--tab', 'NOPE']), /^Unknown tab "NOPE"/);
      }
      assert.equal(model.touched.length, touched);
      const tab = fake.targets.find((item) => item.url.startsWith(shop));
      const failed = 'https://vault.sliccy.ai/landing?code=fixture-error-secret';
      tab.url = 'https://vault.sliccy.ai';
      tab.frame = {
        id: tab.targetId,
        loaderId: 'LERR',
        url: 'chrome-error://chromewebdata/',
        securityOrigin: '://',
        unreachableUrl: failed,
      };
      tab.history = {
        currentIndex: 1,
        entries: [`${shop}/app`, failed].map((url, index) => ({
          id: index + 1,
          url,
          userTypedURL: url,
          title: url,
          transitionType: 'typed',
        })),
      };
      for (const args of [...commands, ['state-save', 'error.json']]) {
        const stderr = await refused([...args, '--tab', tab.targetId]);
        assert.ok(stderr.startsWith('slicc: refused'), `${args.join(' ')}: ${stderr}`);
        assert.ok(!stderr.includes('fixture-error-secret'), stderr);
      }
      assert.equal(model.touched.length, touched);
      for (const frame of [
        {
          id: tab.targetId,
          loaderId: 'LERR2',
          url: 'chrome-error://chromewebdata/',
          securityOrigin: '://',
          unreachableUrl: failed,
        },
        {
          id: tab.targetId,
          loaderId: 'LSTALE',
          url: 'https://vault.sliccy.ai/landing',
          securityOrigin: 'https://vault.sliccy.ai',
        },
      ]) {
        tab.url = `${shop}/app`;
        tab.frame = frame;
        for (const args of [...commands, ['state-save', 'stale.json']]) {
          const result = await refused([...args, '--tab', tab.targetId]);
          assert.ok(
            result.startsWith('slicc: refused'),
            `${frame.loaderId} ${args.join(' ')}: ${result}`
          );
        }
        assert.equal(model.touched.length, touched, frame.loaderId);
      }
    });
  });

  test(`${kind}: state-save writes Microsoft's storageState JSON without protected cookies or origins`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, model, runner, ok }) => {
      await ok(['open', `${shop}/app`]);
      await ok(['tab-new', `${docs}/guide`]);
      const docsTab = fake.targets.find((item) => item.url.startsWith(docs)).targetId;
      await ok(['localstorage-set', 'note', 'fixture "quoted"\n', '--tab', docsTab]);
      await ok(['localstorage-set', 'emoji', 'π', '--tab', docsTab]);
      await ok(['sessionstorage-set', 'skip', 'fixture-session-only', '--tab', docsTab]);
      fake.targets.push({ targetId: 'VAULT', url: 'https://vault.sliccy.ai/x', loaderId: 'LV' });
      fake.targets.push({
        targetId: 'STALE',
        url: 'https://other.example.test/',
        loaderId: 'LO',
        frame: {
          id: 'STALE',
          loaderId: 'LO',
          url: 'https://vault.sliccy.ai/landing',
          securityOrigin: 'https://vault.sliccy.ai',
        },
      });
      assert.equal(
        await ok(['state-save', 'state.json']),
        `### Result\n- [Storage state](./state.json)\n${code("await page.context().storageState({ path: './state.json' });")}`
      );
      assert.ok(
        fake.calls.some(
          (call) => call.method === 'Target.attachToTarget' && call.params.targetId === 'STALE'
        )
      );
      const written = await runner.read('state.json');
      assert.equal(written, expectedState);
      assert.ok(!written.includes('fixture-session-only'));
      if (kind === 'native' && process.platform !== 'win32')
        assert.equal(await runner.mode('state.json'), 0o600);
      const listed = model.touched
        .filter((entry) => entry.op === 'local-list')
        .map((entry) => entry.origin);
      assert.ok(listed.includes(shop) && listed.includes(docs), JSON.stringify(listed));
      fake.targets.splice(
        fake.targets.findIndex((item) => item.targetId === 'VAULT'),
        1
      );
      const fallback = await ok(['state-save']);
      const name = fallback.match(
        /^### Result\n- \[Storage state\]\((\.playwright-cli\/storage-state-\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d-\d{3}Z\.json)\)\n/
      )?.[1];
      assert.ok(name, fallback);
      assert.ok(fallback.endsWith(code(`await page.context().storageState({ path: '${name}' });`)));
      assert.equal(await runner.read(name), expectedState);
      assert.equal(count(fake, 'Network.setCookie'), 0);
    });
  });

  test(`${kind}: state-load refuses protected or unsupported entries before any change`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, run, ok, refused, runner }) => {
      await ok(['open', `${shop}/app`]);
      const before = mutations(fake);
      const ordinary = { name: 'ok', value: 'fixture-ok', domain: 'shop.example.test', path: '/' };
      const cases = [
        [
          {
            cookies: [
              ordinary,
              { name: 'seven', value: 'fixture-forged', domain: '.sliccy.ai', path: '/' },
            ],
          },
          cookieRefusal,
        ],
        [
          { cookies: [{ name: 'seven', value: 'fixture-forged', domain: 'sliccy.ai', path: '/' }] },
          cookieRefusal,
        ],
        [
          {
            cookies: [{ name: 'vault', value: 'fixture-forged', url: 'https://vault.sliccy.ai/x' }],
          },
          cookieRefusal,
        ],
        [
          { cookies: [{ name: 'host', value: 'fixture-forged', domain: 'ai', path: '/' }] },
          cookieRefusal,
        ],
        [
          { cookies: [{ name: 'bad', value: 'fixture-forged', domain: 'a b', path: '/' }] },
          cookieRefusal,
        ],
        [
          {
            cookies: [
              {
                ...ordinary,
                name: 'chip',
                partitionKey: 'https://sliccy.ai',
                _crHasCrossSiteAncestor: true,
              },
            ],
          },
          cookieRefusal,
        ],
        [{ origins: [{ origin: 'https://seven.sliccy.ai', localStorage: [] }] }, /^slicc: refused/],
        [
          {
            origins: [
              { origin: 'https://vault.sliccy.ai', localStorage: [{ name: 'a', value: 'b' }] },
            ],
          },
          /^slicc: refused/,
        ],
        [{ origins: [{ origin: 'chrome://settings', localStorage: [] }] }, /^slicc: refused/],
        [{ origins: [{ origin: 'file:///etc', localStorage: [] }] }, /^slicc: refused/],
        [{ origins: [{ origin: 'null', localStorage: [] }] }, /^slicc: refused/],
        [
          { origins: [{ origin: docs, localStorage: [], indexedDB: [{ name: 'db' }] }] },
          'Error: Error setting storage state:\nstorageState.origins[0].indexedDB: restoring indexedDB is not supported\n',
        ],
        [
          { origins: [{ origin: docs, localStorage: [], opfs: [{ name: 'f' }] }] },
          'Error: Error setting storage state:\nstorageState.origins[0].opfs: restoring opfs is not supported\n',
        ],
        [
          { credentials: [{ id: 'x' }] },
          'Error: Error setting storage state:\nstorageState.credentials: restoring credentials is not supported\n',
        ],
        [
          { cookies: [{ name: 'x', value: 'y' }] },
          'Error: Error setting storage state:\nCookie should have a url or a domain/path pair\n',
        ],
        [
          {
            cookies: [
              { name: 'x', value: 'y', url: 'https://a.example.test/', domain: 'a.example.test' },
            ],
          },
          'Error: Error setting storage state:\nCookie should have either url or domain\n',
        ],
        [
          { cookies: [{ name: 'x', value: 'y', url: 'https://a.example.test/', path: '/' }] },
          'Error: Error setting storage state:\nCookie should have either url or path\n',
        ],
        [
          { cookies: [{ ...ordinary, expires: -5 }] },
          'Error: Error setting storage state:\nCookie should have a valid expires, only -1 or a positive number for the unix timestamp in seconds is allowed\n',
        ],
        [
          { cookies: [{ name: 'x', value: 'y', url: 'about:blank' }] },
          'Error: Error setting storage state:\nBlank page can not have cookie "x"\n',
        ],
        [
          { cookies: [{ name: 'x', value: 'y', url: 'data:text/plain,a' }] },
          'Error: Error setting storage state:\nData URL page can not have cookie "x"\n',
        ],
        [
          { cookies: [{ ...ordinary, sameSite: 'Bogus' }] },
          'Error: Error setting storage state:\nstorageState.cookies[0].sameSite: expected one of (Strict|Lax|None)\n',
        ],
        [
          { cookies: [{ ...ordinary, name: 3 }] },
          'Error: Error setting storage state:\nstorageState.cookies[0].name: expected string\n',
        ],
        [
          { origins: [{ origin: docs, localStorage: [{ name: 'a' }] }] },
          'Error: Error setting storage state:\nstorageState.origins[0].localStorage[0].value: expected string\n',
        ],
        [
          { cookies: {} },
          'Error: Error setting storage state:\nstorageState.cookies: expected array\n',
        ],
        [[], 'Error: Error setting storage state:\nstorageState: expected object\n'],
      ];
      for (const [index, [state, expected]] of cases.entries()) {
        await runner.write(`bad${index}.json`, JSON.stringify(state));
        const stderr = await refused(['state-load', `bad${index}.json`]);
        if (typeof expected === 'string') assert.equal(stderr, expected, JSON.stringify(state));
        else assert.match(stderr, expected, JSON.stringify(state));
      }
      await runner.write('broken.json', '{');
      const broken = await run(['state-load', 'broken.json']);
      assert.equal(broken.code, 1);
      assert.match(broken.stderr, /^Error: Error reading storage state from broken\.json:\n/);
      const missing = await run(['state-load', 'missing.json']);
      assert.equal(missing.code, 1);
      assert.match(missing.stderr, /^Error: Error reading storage state from missing\.json:\n/);
      assert.deepEqual(mutations(fake), before);
    });
  });

  test(`${kind}: state-load replaces ordinary cookies and storage through a guarded blank tab`, {
    timeout: 120_000,
  }, async () => {
    await withBrowser(kind, async ({ fake, model, runner, ok }) => {
      await ok(['open', `${shop}/app`]);
      const state = {
        cookies: [
          {
            name: 'loaded',
            value: 'fixture-loaded',
            domain: 'shop.example.test',
            path: '/',
            expires: -1,
            httpOnly: false,
            secure: true,
            sameSite: 'Lax',
          },
          { name: 'byurl', value: 'fixture-byurl', url: `${docs}/a/b` },
          {
            name: 'part',
            value: 'fixture-part',
            domain: 'widget.example.test',
            path: '/',
            expires: 1924992000,
            partitionKey: 'https://example.test',
          },
        ],
        origins: [
          { origin: docs, localStorage: [{ name: 'restored', value: 'fixture-restored' }] },
          { origin: `${shop}:443`, localStorage: [] },
        ],
      };
      await runner.write('state.json', JSON.stringify(state));
      const tabs = fake.targets.length;
      assert.equal(
        await ok(['state-load', 'state.json']),
        `### Result\nStorage state restored from state.json\n${code("await page.context().setStorageState('state.json');")}`
      );
      assert.equal(fake.targets.length, tabs);
      const set = fake.calls
        .filter((call) => call.method === 'Network.setCookie')
        .map((call) => call.params);
      assert.deepEqual(set, [
        {
          name: 'loaded',
          value: 'fixture-loaded',
          domain: 'shop.example.test',
          path: '/',
          httpOnly: false,
          secure: true,
          sameSite: 'Lax',
        },
        {
          name: 'byurl',
          value: 'fixture-byurl',
          domain: 'docs.example.test',
          path: '/a/',
          secure: true,
        },
        {
          name: 'part',
          value: 'fixture-part',
          domain: 'widget.example.test',
          path: '/',
          expires: 1924992000,
          partitionKey: { topLevelSite: 'https://example.test', hasCrossSiteAncestor: true },
        },
      ]);
      assert.deepEqual(fake.jar.map((item) => item.value).sort(), [
        'fixture-byurl',
        'fixture-chip-secret',
        'fixture-loaded',
        'fixture-part',
        'fixture-seven-secret',
        'fixture-vault-secret',
      ]);
      assert.deepEqual([...model.local.get(docs)], [['restored', 'fixture-restored']]);
      assert.deepEqual([...model.local.get(shop)], []);
      const created = fake.calls.filter((call) => call.method === 'Target.createTarget');
      assert.deepEqual(created.at(-1).params, { url: 'about:blank', background: true });
      const enabled = fake.calls.filter((call) => call.method === 'Fetch.enable').at(-1);
      assert.deepEqual(
        enabled.params.patterns.map((pattern) => pattern.urlPattern),
        [`${docs}/*`, `${shop}/*`]
      );
      assert.ok(count(fake, 'Fetch.fulfillRequest') >= 2);
      const navigated = fake.calls.filter((call) => call.method === 'Page.navigate').slice(-2);
      assert.deepEqual(
        navigated.map((call) => call.params.url),
        [docs, shop]
      );
      assert.ok(fake.calls.some((call) => call.method === 'Target.closeTarget'));
      assert.equal(
        await ok(['cookie-get', 'loaded']),
        'loaded=fixture-loaded (domain: .shop.example.test, path: /, httpOnly: false, secure: true, sameSite: Lax)\n'
      );
    });
  });
}
